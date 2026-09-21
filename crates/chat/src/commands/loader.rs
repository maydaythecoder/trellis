//! Reading `.harness/commands/*.md` back in.
//!
//! **These files are untrusted input.** They are prompts that execute with
//! the user's credentials in the user's repository — the same category as
//! any shared template, and they arrive by being dropped in a directory.
//! So:
//!
//! - parsed as **data**, with a hand-written frontmatter reader; nothing is
//!   evaluated, and no field can turn into code;
//! - a loaded command **cannot shadow a built-in** (enforced in
//!   `Registry::register_custom`) and cannot widen anything — the only
//!   fields read are a name, a description and parameter *names*;
//! - `destructive` is **not readable from the file**. A command cannot
//!   declare itself safe, and the body is shown before the first run
//!   (`AppState` holds `body` for exactly that);
//! - the command name comes from the **filename**, not the frontmatter, so
//!   a file cannot claim a name that is not its own.

use super::{ArgSpec, CommandSpec};
use std::path::Path;

/// A custom command, as read off disk.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub spec: CommandSpec,
    /// The steps, verbatim. Shown to the user before the first run, because
    /// "what will this do" has to be answerable without leaving the
    /// interface.
    pub body: String,
}

/// Parses one file. `filename` supplies the command name.
///
/// Returns `Err` with a reason rather than skipping: a command file that
/// silently failed to load leaves the user believing it is registered, and
/// they find out when `/thing` reports "unknown command".
pub fn parse_command_file(text: &str, filename: &str) -> Result<CommandSpec, String> {
    Ok(parse_loaded(text, filename)?.spec)
}

/// As `parse_command_file`, keeping the body.
pub fn parse_loaded(text: &str, filename: &str) -> Result<Loaded, String> {
    let name = filename
        .strip_suffix(".md")
        .ok_or_else(|| format!("{filename}: not a .md file"))?;
    if !super::eng::is_valid_name(name) {
        return Err(format!(
            "{filename}: `{name}` is not a usable command name (letters, digits, `-`, `_`)"
        ));
    }

    let rest = text
        .strip_prefix("---")
        .ok_or_else(|| format!("{filename}: no YAML frontmatter (expected a leading `---`)"))?;
    // Split on the closing fence at the start of a line, so a `---` inside a
    // prose line cannot end the frontmatter early.
    let (front, body) = split_frontmatter(rest)
        .ok_or_else(|| format!("{filename}: frontmatter is not closed with `---`"))?;

    let mut description = String::new();
    let mut declared = None;
    let mut parameters: Vec<String> = Vec::new();
    let mut in_parameters = false;

    for raw in front.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        // A nested key, which for this format only ever means a parameter.
        if in_parameters && (raw.starts_with("  ") || raw.starts_with('\t')) {
            if let Some((k, _)) = line.trim().split_once(':') {
                let k = k.trim();
                if !k.is_empty() {
                    parameters.push(k.to_string());
                }
            }
            continue;
        }
        in_parameters = false;
        let Some((key, value)) = line.split_once(':') else {
            return Err(format!("{filename}: `{}` is not `key: value`", line.trim()));
        };
        let value = value.trim().trim_matches('"').to_string();
        match key.trim() {
            "command" => declared = Some(value),
            "description" => description = value,
            "parameters" => in_parameters = true,
            // Unknown keys are ignored rather than rejected: a file written
            // for a later version should still load. Ignoring is safe here
            // precisely because no key can grant anything.
            _ => {}
        }
    }

    // The filename wins. A file that claims another name is refused rather
    // than quietly renamed, because the mismatch is the interesting part.
    if let Some(d) = &declared
        && d != name
    {
        return Err(format!(
            "{filename}: declares `command: {d}` but the file is `{name}.md`"
        ));
    }

    if description.is_empty() {
        description = "(no description)".to_string();
    }

    // Arity comes from the declared parameters, and only ever in the two
    // shapes the parser can check. Nothing here can express "run this".
    let args = match parameters.len() {
        0 => ArgSpec::None,
        1 => ArgSpec::Rest(Box::leak(parameters[0].clone().into_boxed_str())),
        _ => ArgSpec::Rest("arguments"),
    };

    Ok(Loaded {
        spec: CommandSpec {
            name: name.to_string(),
            description,
            args,
            // Never read from the file. A command cannot declare itself safe.
            destructive: false,
            custom: true,
        },
        body: body.trim_start_matches('\n').to_string(),
    })
}

fn split_frontmatter(rest: &str) -> Option<(&str, &str)> {
    let mut at = 0usize;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Some((&rest[..at], &rest[at + line.len()..]));
        }
        at += line.len();
    }
    None
}

/// Every `.md` in `dir`, in filename order.
///
/// Files that fail to parse are returned as errors beside the ones that
/// worked, so `/reload` can report both. A directory that does not exist is
/// not an error — most repositories have no custom commands.
pub fn load_dir(dir: &Path) -> (Vec<Loaded>, Vec<String>) {
    let mut loaded = Vec::new();
    let mut errors = Vec::new();

    let Ok(entries) = std::fs::read_dir(dir) else {
        return (loaded, errors);
    };
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    paths.sort();

    for p in paths {
        let filename = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        match std::fs::read_to_string(&p) {
            Ok(text) => match parse_loaded(&text, &filename) {
                Ok(l) => loaded.push(l),
                Err(e) => errors.push(e),
            },
            Err(e) => errors.push(format!("{filename}: {e}")),
        }
    }
    (loaded, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "---\n\
        command: test-loop\n\
        description: Autonomous compile-fix loop over the target directory\n\
        parameters:\n  \
          dir: \"target path to check\"\n\
        ---\n\
        1. Read the file layout in {{dir}}\n\
        2. Run the project's check command\n";

    #[test]
    fn a_well_formed_file_becomes_a_custom_spec() {
        let l = parse_loaded(GOOD, "test-loop.md").unwrap();
        assert_eq!(l.spec.name, "test-loop");
        assert!(l.spec.custom);
        assert!(l.spec.description.contains("compile-fix"));
        assert!(matches!(l.spec.args, ArgSpec::Rest("dir")));
        assert!(l.body.starts_with("1. Read the file layout"));
    }

    #[test]
    fn a_loaded_command_can_never_declare_itself_safe() {
        // `destructive` is the gate on losing work. A file that could set it
        // false would be a dropped-in prompt turning off its own
        // confirmation, which is exactly the shape of the attack these
        // files invite.
        let text = GOOD.replace("parameters:", "destructive: false\nparameters:");
        let l = parse_loaded(&text, "test-loop.md").unwrap();
        assert!(!l.spec.destructive);
        let text = GOOD.replace("parameters:", "destructive: true\nparameters:");
        let l = parse_loaded(&text, "test-loop.md").unwrap();
        assert!(
            !l.spec.destructive,
            "the field must be ignored in both directions, not trusted in one"
        );
    }

    #[test]
    fn the_filename_is_the_name_and_a_mismatch_is_refused() {
        // A file claiming another name is the interesting case: refused
        // rather than quietly renamed, so the mismatch reaches a human.
        let e = parse_loaded(GOOD, "something-else.md").unwrap_err();
        assert!(e.contains("declares"), "got {e}");
    }

    #[test]
    fn a_name_that_is_a_path_is_refused() {
        let e = parse_loaded(GOOD, "../escape.md").unwrap_err();
        assert!(e.contains("not a usable command name"), "got {e}");
    }

    #[test]
    fn a_file_without_frontmatter_is_an_error_rather_than_a_skip() {
        // A silent skip leaves the user believing a command is registered;
        // they find out when `/thing` says "unknown command".
        let e = parse_loaded("1. do a thing\n", "x.md").unwrap_err();
        assert!(e.contains("frontmatter"), "got {e}");
        let e = parse_loaded("---\ncommand: x\n1. unterminated\n", "x.md").unwrap_err();
        assert!(e.contains("not closed"), "got {e}");
    }

    #[test]
    fn a_three_dash_line_inside_the_body_does_not_end_the_frontmatter() {
        let text = "---\ndescription: d\n---\nstep one\n---\nstep two\n";
        let l = parse_loaded(text, "x.md").unwrap();
        assert_eq!(l.spec.description, "d");
        assert!(l.body.contains("step two"));
    }

    #[test]
    fn unknown_keys_load_rather_than_failing() {
        // Safe here precisely because no key grants anything: forward
        // compatibility costs nothing when the parser cannot be widened.
        let text = GOOD.replace("parameters:", "future_field: whatever\nparameters:");
        assert!(parse_loaded(&text, "test-loop.md").is_ok());
    }

    #[test]
    fn load_dir_reports_the_broken_files_beside_the_working_ones() {
        let d = std::env::temp_dir().join(format!("chat-loader-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("test-loop.md"), GOOD).unwrap();
        std::fs::write(d.join("broken.md"), "no frontmatter").unwrap();
        std::fs::write(d.join("ignored.txt"), GOOD).unwrap();

        let (loaded, errors) = load_dir(&d);
        assert_eq!(loaded.len(), 1, "{loaded:?}");
        assert_eq!(loaded[0].spec.name, "test-loop");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("broken.md"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_missing_directory_is_not_an_error() {
        // Most repositories have no custom commands, and reporting that as
        // a failure on every start would teach the user to ignore the pane
        // the real errors appear in.
        let (loaded, errors) = load_dir(Path::new("/no/such/place/at/all"));
        assert!(loaded.is_empty());
        assert!(errors.is_empty());
    }
}
