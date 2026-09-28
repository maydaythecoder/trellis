//! Prompt templates with arguments (ADR 0025 §3).
//!
//! `.trellis/prompts/<name>.md`, optionally with frontmatter (`description`,
//! `argument-hint`), expanded with pi's substitutions. The argument string is
//! split here, by one splitter, for all three surfaces that take one
//! (`run --args`, the session's `/<name> args`, RPC's `"args"`), so a
//! template expands the same way whichever shell or client sent it.
//!
//! A template is still a **task**, never configuration (see
//! `RepoConfig::prompt`): nothing in it can set a model or lift a rule.

use anyhow::Result;
use std::path::Path;

/// One template as listed by `/prompts` and `get_commands`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Listed {
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
}

/// The frontmatter and the body after it.
#[derive(Debug, Default, PartialEq, Eq)]
struct Parsed<'a> {
    has_frontmatter: bool,
    description: Option<String>,
    argument_hint: Option<String>,
    body: &'a str,
}

/// Splits `---` frontmatter off the top.
///
/// Deliberately small: `key: value` lines, the value plain or quoted. A
/// template's frontmatter holds two short strings, and a YAML parser for
/// them would be a dependency to read two lines. Anything else is ignored,
/// as ADR 0019 ignores unknown skill keys.
fn parse(text: &str) -> Parsed<'_> {
    let t = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = t
        .strip_prefix("---\n")
        .or_else(|| t.strip_prefix("---\r\n"))
    else {
        return Parsed {
            body: text,
            ..Parsed::default()
        };
    };
    // The closing fence is a line that is exactly `---`.
    let mut offset = 0;
    let mut close = None;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            close = Some((offset, offset + line.len()));
            break;
        }
        offset += line.len();
    }
    let Some((head_end, body_start)) = close else {
        // An opening fence with no close is not frontmatter; the file is
        // used as written rather than guessed at.
        return Parsed {
            body: text,
            ..Parsed::default()
        };
    };
    let mut p = Parsed {
        has_frontmatter: true,
        body: &rest[body_start..],
        ..Parsed::default()
    };
    for line in rest[..head_end].lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let v = v.trim();
        let v = v
            .strip_prefix('"')
            .and_then(|x| x.strip_suffix('"'))
            .or_else(|| v.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')))
            .unwrap_or(v)
            .to_string();
        match k.trim() {
            "description" if !v.is_empty() => p.description = Some(v),
            "argument-hint" if !v.is_empty() => p.argument_hint = Some(v),
            _ => {}
        }
    }
    p
}

/// A template's text as sent: frontmatter removed, arguments substituted.
///
/// `args` is `None` when none were given. A template with no frontmatter and
/// no arguments comes back trimmed and otherwise verbatim, exactly as before
/// this module existed, so a literal `$5` in an older template survives.
/// Arguments given to a template that uses none are appended, as task text
/// is: nothing typed is dropped.
pub(crate) fn render_text(text: &str, args: Option<&[String]>) -> String {
    let p = parse(text);
    let body = p.body.trim();
    if !p.has_frontmatter && args.is_none() {
        return body.to_string();
    }
    let given = args.unwrap_or(&[]);
    let (out, used) = substitute(body, given);
    if !used && !given.is_empty() {
        return format!("{out}\n\n{}", given.join(" "));
    }
    out
}

/// `.trellis/prompts/<name>.md` rendered with `args` split from a string.
pub(crate) fn render(root: &Path, name: &str, args: Option<&str>) -> Result<String> {
    let split = args.map(split_args).transpose()?;
    // The name guard and the "known templates" message live in
    // `RepoConfig::prompt`; read through it so there is one of each.
    let raw = raw(root, name)?;
    Ok(render_text(&raw, split.as_deref()))
}

/// The file as written, after `RepoConfig::prompt`'s name check.
fn raw(root: &Path, name: &str) -> Result<String> {
    crate::config::RepoConfig::prompt(root, name)?;
    Ok(std::fs::read_to_string(
        root.join(".trellis")
            .join("prompts")
            .join(format!("{name}.md")),
    )?)
}

/// Every template in `.trellis/prompts/`, by name.
pub(crate) fn list(root: &Path) -> Vec<Listed> {
    let Ok(rd) = std::fs::read_dir(root.join(".trellis").join("prompts")) else {
        return Vec::new();
    };
    let mut out: Vec<Listed> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            if path.extension()? != "md" {
                return None;
            }
            let name = path.file_stem()?.to_str()?.to_string();
            let text = std::fs::read_to_string(&path).ok()?;
            let p = parse(&text);
            // As in pi: no description falls back to the first non-empty
            // line. Control characters are dropped because this is printed
            // to a terminal and a template is repository content.
            let description = p
                .description
                .or_else(|| {
                    p.body
                        .lines()
                        .map(str::trim)
                        .find(|l| !l.is_empty())
                        .map(str::to_string)
                })
                .unwrap_or_default();
            Some(Listed {
                name,
                description: crate::plain(&description),
                argument_hint: p.argument_hint.map(|h| crate::plain(&h)),
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Splits an argument string the way a POSIX shell would, without expanding
/// anything: whitespace separates, `'…'` is literal, `"…"` honours `\"` and
/// `\\`, and outside quotes `\` escapes the next character.
///
/// An unterminated quote is an error. Guessing where it ends would hand the
/// template an argument the person did not type.
pub(crate) fn split_args(s: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(x) => cur.push(x),
                        None => anyhow::bail!("unterminated ' in the arguments: {s}"),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(x @ ('"' | '\\')) => cur.push(x),
                            Some(x) => {
                                cur.push('\\');
                                cur.push(x);
                            }
                            None => anyhow::bail!("unterminated \" in the arguments: {s}"),
                        },
                        Some(x) => cur.push(x),
                        None => anyhow::bail!("unterminated \" in the arguments: {s}"),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(x) = chars.next() {
                    cur.push(x);
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        out.push(cur);
    }
    Ok(out)
}

/// pi's substitutions. Returns the text and whether any placeholder was
/// present, which decides whether unused arguments are appended.
///
/// `$1`…`$9` take one digit, so `$10` is `$1` then `0`, as in sh. Inside
/// braces a number may have several digits. Any other `$` sequence is left
/// exactly as written.
fn substitute(body: &str, args: &[String]) -> (String, bool) {
    let mut out = String::with_capacity(body.len());
    let mut used = false;
    let mut rest = body;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let braces = after
            .strip_prefix('{')
            .and_then(|x| x.find('}').map(|end| &x[..end]))
            .and_then(|inner| braced(inner, args).map(|text| (text, inner.len() + 2)));
        let hit: Option<(String, usize)> = match after.as_bytes().first() {
            Some(d @ b'1'..=b'9') => Some((
                args.get(usize::from(d - b'1')).cloned().unwrap_or_default(),
                1,
            )),
            Some(b'@') => Some((args.join(" "), 1)),
            _ if after.starts_with("ARGUMENTS") => Some((args.join(" "), "ARGUMENTS".len())),
            _ => braces,
        };
        let consumed = match hit {
            Some((text, n)) => {
                out.push_str(&text);
                used = true;
                n
            }
            None => {
                out.push('$');
                0
            }
        };
        rest = &after[consumed..];
    }
    out.push_str(rest);
    (out, used)
}

/// `${N}`, `${N:-d}`, `${@:-d}`, `${@:N}`, `${@:N:L}`. `None` for anything
/// else, which the caller leaves as written.
fn braced(inner: &str, args: &[String]) -> Option<String> {
    let number = |s: &str| -> Option<usize> {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    };
    let (head, default) = match inner.split_once(":-") {
        Some((h, d)) => (h, Some(d)),
        None => (inner, None),
    };
    let pick = |v: String| match default {
        Some(d) if v.is_empty() => d.to_string(),
        _ => v,
    };
    if let Some(n) = number(head).filter(|n| *n >= 1) {
        return Some(pick(args.get(n - 1).cloned().unwrap_or_default()));
    }
    if head == "@" {
        return Some(pick(args.join(" ")));
    }
    if default.is_none()
        && let Some(slice) = head.strip_prefix("@:")
    {
        let (from, len) = match slice.split_once(':') {
            Some((f, l)) => (number(f)?, Some(number(l)?)),
            None => (number(slice)?, None),
        };
        let start = from.saturating_sub(1).min(args.len());
        let end = len.map_or(args.len(), |l| (start + l).min(args.len()));
        return Some(args[start..end].join(" "));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn every_substitution_pi_documents_expands() {
        let a = v(&["one", "two words", "three"]);
        let s = |t: &str| substitute(t, &a).0;
        assert_eq!(s("$1|$2|$3|$4"), "one|two words|three|");
        assert_eq!(s("$@"), "one two words three");
        assert_eq!(s("$ARGUMENTS"), "one two words three");
        assert_eq!(s("${2}"), "two words");
        assert_eq!(s("${4:-dflt}"), "dflt");
        assert_eq!(s("${1:-dflt}"), "one");
        assert_eq!(s("${@:2}"), "two words three");
        assert_eq!(s("${@:2:1}"), "two words");
        assert_eq!(s("${@:5}"), "");
        assert_eq!(substitute("${@:-none given}", &[]).0, "none given");
    }

    #[test]
    fn dollar_ten_is_dollar_one_then_zero_as_in_sh() {
        let a: Vec<String> = (1..=10).map(|i| format!("a{i}")).collect();
        assert_eq!(substitute("$10", &a).0, "a10");
        assert_eq!(substitute("${10}", &a).0, "a10");
        assert_eq!(substitute("$10", &v(&["x"])).0, "x0");
    }

    #[test]
    fn a_dollar_that_is_not_a_placeholder_is_left_alone() {
        let (out, used) = substitute("costs $ and $HOME and ${HOME} and $0 and $", &v(&["x"]));
        assert_eq!(out, "costs $ and $HOME and ${HOME} and $0 and $");
        assert!(!used);
    }

    #[test]
    fn an_older_template_with_no_frontmatter_and_no_args_is_byte_for_byte_unchanged() {
        // The risk: a literal `$5` in a template written before arguments
        // existed silently becoming an empty string.
        let t = "  Pay $5 to fix ${thing}. Use $1 carefully.\n";
        assert_eq!(render_text(t, None), t.trim());
    }

    #[test]
    fn frontmatter_never_reaches_the_model_and_defaults_apply_without_args() {
        let t = "---\ndescription: Review a module\nargument-hint: \"[focus]\"\n---\n\
                 Review it. Focus on ${1:-correctness}.\n";
        assert_eq!(render_text(t, None), "Review it. Focus on correctness.");
        assert_eq!(
            render_text(t, Some(&v(&["speed"]))),
            "Review it. Focus on speed."
        );
    }

    #[test]
    fn arguments_a_template_does_not_use_are_appended_not_dropped() {
        assert_eq!(
            render_text("Fix the annotations.", Some(&v(&["in", "app/x.py"]))),
            "Fix the annotations.\n\nin app/x.py"
        );
    }

    #[test]
    fn arguments_split_like_a_shell_and_an_open_quote_is_refused() {
        assert_eq!(
            split_args(r#"a "b c" 'd "e"' f\ g "h\"i" ''"#).unwrap(),
            v(&["a", "b c", "d \"e\"", "f g", "h\"i", ""])
        );
        assert!(split_args("\"open").is_err());
        assert!(split_args("'open").is_err());
        assert_eq!(split_args("   ").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn listing_reads_frontmatter_and_falls_back_to_the_first_line() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(".trellis/prompts");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(
            p.join("review.md"),
            "---\ndescription: Review\nargument-hint: <path>\n---\nbody\n",
        )
        .unwrap();
        std::fs::write(p.join("plain.md"), "\n\nFirst line\nsecond\n").unwrap();
        let l = list(d.path());
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].name, "plain");
        assert_eq!(l[0].description, "First line");
        assert_eq!(l[1].argument_hint.as_deref(), Some("<path>"));
        assert_eq!(render(d.path(), "review", Some("x")).unwrap(), "body\n\nx");
    }
}
