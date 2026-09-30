//! What ADR 0025 §6 adds to `trellis session`: `!cmd`, `/prompts`,
//! `/models`, and the model listing `rpc` shares; and `/name` and `/copy`
//! (ADR 0027, #109).
//!
//! `/skill` and `/<template>` are dispatched in `session.rs`, because they
//! start a run and the session owns how a run is started.

use anyhow::Result;
use serde_json::{Value, json};
use std::path::Path;

/// `!cmd` and `!!cmd`: runs a shell command in the repository root.
///
/// The output goes to this terminal and nowhere else. In pi, `!` also sends
/// the output to the model and `!!` does not. A trellis session has no
/// conversation to send it to, because each task is a fresh run, so the two
/// are the same.
///
/// A command that changes files leaves the worktree dirty, and the next task
/// refuses to start (ADR 0009). That is not prevented: the command is the
/// person's own, typed at their own prompt.
pub(crate) fn shell(root: &Path, line: &str) -> Result<()> {
    let cmd = line.trim();
    if cmd.is_empty() {
        anyhow::bail!("`!` needs a command, e.g. `!git log --oneline -3`");
    }
    let status = shell_command(cmd).current_dir(root).status();
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => {
            println!("exit {}", s.code().map_or("?".into(), |c| c.to_string()));
            Ok(())
        }
        Err(e) => anyhow::bail!("could not start the shell: {e}"),
    }
}

/// `cmd /C` with the line passed unmodified on Windows; `sh -c` elsewhere.
///
/// `raw_arg` because std's argument quoting targets programs that parse
/// their command line the C runtime's way, and `cmd.exe` does not. Quoted
/// arguments inside the line would otherwise arrive escaped twice.
///
/// Scrubbed like every other child (ADR 0024 §3). The line is the person's
/// own, but it runs inside trellis, and one rule for every spawn is the one
/// that survives the next spawn someone adds.
fn shell_command(cmd: &str) -> std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").raw_arg(cmd);
        trellis_core::secrets::scrub(&mut c);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(cmd);
        trellis_core::secrets::scrub(&mut c);
        c
    }
}

/// `/prompts`.
pub(crate) fn print_prompts(root: &Path) {
    let list = match crate::templates::list(root) {
        Ok(l) => l,
        Err(e) => {
            println!("templates not listed: {e:#}");
            return;
        }
    };
    if list.is_empty() {
        println!("no templates in .trellis/prompts/ or in an installed package");
        return;
    }
    for t in list {
        let hint = t.argument_hint.map(|h| format!(" {h}")).unwrap_or_default();
        println!("  /{}{}", t.name, hint);
        if !t.description.is_empty() {
            println!("      {}", t.description);
        }
    }
    println!();
    println!("A template runs as a task. Arguments split shell-style: /name \"two words\" x");
}

/// The catalog and every configured spec, as data. Shared by `/models` and
/// `rpc`'s `get_available_models`.
///
/// Configured specs are listed by where they come from, because the
/// repository's wins over the user's (ADR 0025 §7) and a person choosing a
/// model should see both.
pub(crate) fn models_json(root: &Path, session: Option<&str>) -> Value {
    let repo = crate::config::RepoConfig::load_with_user(root, None)
        .ok()
        .and_then(|c| c.model());
    let user = crate::user_config::file().and_then(|f| {
        let text = std::fs::read_to_string(f).ok()?;
        let v: toml::Value = toml::from_str(&text).ok()?;
        v.get("model")?.get("spec")?.as_str().map(str::to_string)
    });
    let catalog: Vec<Value> = trellis_model::CATALOG
        .iter()
        .map(|e| {
            json!({
                "name": e.name,
                "license": e.license,
                "download_gib": (e.gib() * 10.0).round() / 10.0,
                "default": e.name == trellis_model::DEFAULT,
            })
        })
        .collect();
    json!({
        "catalog": catalog,
        "configured": {"repository": repo, "user": user, "session": session},
    })
}

/// `/models`.
pub(crate) fn print_models(root: &Path, session: Option<&str>) {
    let v = models_json(root, session);
    let c = &v["configured"];
    let show = |label: &str, x: &Value| match x.as_str() {
        Some(s) => println!("  {label:<11}{}", crate::plain(s)),
        None => println!("  {label:<11}none"),
    };
    println!("configured specs");
    show("session", &c["session"]);
    show("repository", &c["repository"]);
    show("user", &c["user"]);
    println!();
    println!("catalog (weights `trellis model pull` can fetch)");
    for e in v["catalog"].as_array().into_iter().flatten() {
        println!(
            "  {}{}  {} GiB, {}",
            e["name"].as_str().unwrap_or(""),
            if e["default"] == true {
                " (default)"
            } else {
                ""
            },
            e["download_gib"],
            e["license"].as_str().unwrap_or("")
        );
    }
    println!();
    println!("/model <spec> switches; a catalog entry runs under a runtime such as llamacpp:.");
}

/// The longest `/name` label, in characters.
pub(crate) const LABEL_MAX: usize = 80;

/// Checks a `/name` label, returning it as it will be recorded.
///
/// The label goes into a committed trail, which is read back as untrusted
/// data, and is printed on this terminal. A control character would be a
/// terminal escape in both places, and a long label is a paragraph, not a
/// name. Refused rather than trimmed, so what is recorded is what was typed.
pub(crate) fn check_label(label: &str) -> Result<String, String> {
    let n = label.chars().count();
    if n > LABEL_MAX {
        return Err(format!(
            "a label is at most {LABEL_MAX} characters; this one is {n}. The label is unchanged."
        ));
    }
    if label.chars().any(char::is_control) {
        return Err(
            "a label may not contain a control character. The label is unchanged.".to_string(),
        );
    }
    Ok(label.to_string())
}

/// One clipboard program and its arguments. The text goes on its stdin.
pub(crate) type Clipboard = (&'static str, &'static [&'static str]);

/// The programs `/copy` tries, in order, on this platform. The first one
/// that starts is the one used.
#[cfg(windows)]
pub(crate) const CLIPBOARDS: &[Clipboard] = &[("clip", &[])];
#[cfg(target_os = "macos")]
pub(crate) const CLIPBOARDS: &[Clipboard] = &[("pbcopy", &[])];
#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) const CLIPBOARDS: &[Clipboard] =
    &[("wl-copy", &[]), ("xclip", &["-selection", "clipboard"])];

/// `/copy`: puts `text` on the clipboard through the first of `programs`
/// that starts, or prints it when none is reached.
///
/// A program rather than a clipboard crate (ADR 0027, #109): the platforms'
/// own tools already do this, and no dependency is added for it. `programs`
/// is a parameter so a test can name a program that does not exist and
/// reach the fallback without touching a real clipboard.
///
/// Printing is the fallback because the person asked for the text; a
/// clipboard that could not be reached is no reason to withhold it.
pub(crate) fn copy(
    text: Option<&str>,
    programs: &[Clipboard],
    out: &mut dyn std::io::Write,
) -> Result<()> {
    let Some(text) = text else {
        writeln!(out, "nothing to copy yet")?;
        return Ok(());
    };
    let reached = programs.iter().find_map(|(program, args)| {
        // Only the first that starts is used: one that starts and then
        // fails is reported, not papered over by the next.
        let child = clipboard_command(program, args).spawn().ok()?;
        Some((*program, feed(child, text)))
    });
    match reached {
        Some((program, true)) => {
            writeln!(
                out,
                "copied the last run's done summary ({} bytes) with {program}",
                text.len()
            )?;
        }
        other => {
            let why = match other {
                Some((program, _)) => format!("{program} did not take it"),
                None => "none of the clipboard programs started".to_string(),
            };
            writeln!(
                out,
                "no clipboard was reached ({why}); the summary is below"
            )?;
            writeln!(out, "{}", crate::plain(text))?;
        }
    }
    Ok(())
}

/// A clipboard program, scrubbed like every other child (ADR 0024 §3), with
/// stdout and stderr closed: `xclip` forks to hold the selection, and a
/// piped stdout would keep this side waiting on it.
fn clipboard_command(program: &str, args: &[&str]) -> std::process::Command {
    let mut c = std::process::Command::new(program);
    c.args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    trellis_core::secrets::scrub(&mut c);
    c
}

/// Writes `text` to the child's stdin, closes it, and waits. `true` only
/// when all of it was written and the program exited zero.
fn feed(mut child: std::process::Child, text: &str) -> bool {
    use std::io::Write;
    let written = match child.stdin.take() {
        // Dropped at the end of the arm, which is the end of input.
        Some(mut stdin) => stdin.write_all(text.as_bytes()).is_ok(),
        None => false,
    };
    let exited = child.wait().is_ok_and(|s| s.success());
    written && exited
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shell_command_runs_in_the_repository_root() {
        let d = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let cmd = "echo made> marker.txt";
        #[cfg(not(windows))]
        let cmd = "echo made > marker.txt";
        shell(d.path(), cmd).unwrap();
        assert!(d.path().join("marker.txt").is_file());
    }

    #[test]
    fn an_empty_bang_says_what_it_needs() {
        let d = tempfile::tempdir().unwrap();
        assert!(shell(d.path(), "  ").is_err());
    }

    #[test]
    fn models_names_the_repository_spec_and_the_catalog() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".trellis")).unwrap();
        std::fs::write(
            d.path().join(".trellis/config.toml"),
            "[model]\nspec = \"llamacpp:http://127.0.0.1:8080\"\n",
        )
        .unwrap();
        let v = models_json(d.path(), Some("scripted:x.json"));
        assert_eq!(
            v["configured"]["repository"],
            "llamacpp:http://127.0.0.1:8080"
        );
        assert_eq!(v["configured"]["session"], "scripted:x.json");
        assert_eq!(
            v["catalog"].as_array().unwrap().len(),
            trellis_model::CATALOG.len()
        );
    }

    // `/copy` (#109) --------------------------------------------------------

    /// A program no machine has, so the fallback is reached without touching
    /// a real clipboard.
    const NOWHERE: &[Clipboard] = &[("trellis-no-such-clipboard-program-109", &[])];

    fn copied(text: Option<&str>, programs: &[Clipboard]) -> String {
        let mut out = Vec::new();
        copy(text, programs, &mut out).expect("copy reports rather than failing");
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn slash_copy_prints_the_text_when_no_clipboard_program_starts() {
        // The risk: a machine with no clipboard tool (a server, a container)
        // swallowing the summary the person asked for.
        let said = copied(Some("renamed run() and fixed its callers"), NOWHERE);
        assert!(said.contains("no clipboard was reached"), "{said}");
        assert!(
            said.contains("renamed run() and fixed its callers"),
            "the text itself must be printed: {said}"
        );
    }

    #[test]
    fn slash_copy_prints_the_text_when_the_clipboard_program_fails() {
        // The risk: a program that starts and then fails (`wl-copy` with no
        // Wayland display) reported as a copy that happened.
        #[cfg(windows)]
        const FAILS: &[Clipboard] = &[("cmd", &["/C", "exit 3"])];
        #[cfg(not(windows))]
        const FAILS: &[Clipboard] = &[("sh", &["-c", "exit 3"])];
        let said = copied(Some("the summary"), FAILS);
        assert!(said.contains("no clipboard was reached"), "{said}");
        assert!(said.contains("did not take it"), "{said}");
        assert!(said.contains("the summary"), "{said}");
    }

    #[test]
    fn slash_copy_hands_the_text_to_the_first_program_that_starts() {
        // The risk: the text never reaching the program's stdin, or a later
        // program being used when an earlier one started. The first entry
        // does not exist; the second writes its stdin to a file.
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("clip.txt");
        #[cfg(windows)]
        // Unquoted: std escapes an inner quote as `\"`, which `cmd` does not
        // read. A temporary directory whose path has a space would break this
        // test's redirect, not the code under test.
        let (shell, line) = ("cmd", format!("more > {}", file.display()));
        #[cfg(not(windows))]
        let (shell, line) = ("sh", format!("cat > '{}'", file.display()));
        #[cfg(windows)]
        let flag = "/C";
        #[cfg(not(windows))]
        let flag = "-c";
        let line: &'static str = Box::leak(line.into_boxed_str());
        let args: &'static [&'static str] = Box::leak(vec![flag, line].into_boxed_slice());
        let programs = [NOWHERE[0], (shell, args)];
        let said = copied(Some("fixed the annotations"), &programs);
        assert!(said.contains("copied"), "{said}");
        assert!(!said.contains("no clipboard was reached"), "{said}");
        let got = std::fs::read_to_string(&file).unwrap();
        assert!(got.contains("fixed the annotations"), "{got:?}");
    }

    #[test]
    fn slash_copy_with_nothing_to_copy_says_so_and_starts_nothing() {
        // A failing program would be reported if anything were started.
        let said = copied(None, NOWHERE);
        assert_eq!(said.trim(), "nothing to copy yet");
    }

    // `/name` (#109) --------------------------------------------------------

    #[test]
    fn slash_name_label_limit_is_eighty_characters_not_bytes() {
        // The risk: a byte count refusing a short label in a script whose
        // characters take more than one byte each.
        let at_limit: String = "é".repeat(LABEL_MAX);
        assert_eq!(check_label(&at_limit), Ok(at_limit.clone()));
        let over: String = "a".repeat(LABEL_MAX + 1);
        let e = check_label(&over).unwrap_err();
        assert!(e.contains("at most 80"), "{e}");
        assert!(e.contains("unchanged"), "{e}");
    }

    #[test]
    fn slash_name_refuses_a_control_character() {
        // The risk: an escape sequence recorded in a committed trail and
        // replayed at every terminal that prints it.
        for bad in ["a\u{1b}[31mred", "tab\there", "bell\u{7}", "c1\u{9b}x"] {
            assert!(check_label(bad).is_err(), "{bad:?} was accepted");
        }
    }
}
