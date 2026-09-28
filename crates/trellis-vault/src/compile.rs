//! One focus note's context: the selection rule of ADR 0017.
//!
//! Target body; its parent chain with bodies, root first; direct children as
//! titles; one hop of linked-note bodies; one line per unresolved link. Over
//! the cap, reference bodies are dropped last-first and their headings kept.
//! The target and its ancestors are never truncated — if they alone exceed
//! the cap, compilation fails rather than hand the model a silently shortened
//! lineage.

use crate::VaultError;
use crate::dates::Document;
use crate::graph::Resolved;
use crate::js;
use crate::validate::{Note, parse};
use std::path::Path;

/// The cap when none is configured, in bytes of UTF-8.
pub const DEFAULT_CAP: usize = 24_576;

/// A compiled context block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compiled {
    /// LF only, whatever the vault's line endings were.
    pub text: String,
    /// `text.len()`, stated so a caller reporting it cannot measure something
    /// else by mistake.
    pub bytes: usize,
    /// How many reference bodies were cut to fit the cap. Their headings are
    /// still in `text`; a caller should say this number out loud, because a
    /// heading with nothing under it is easy to read past.
    pub dropped_reference_bodies: usize,
}

/// `\[\[([^\]|#]+)(?:[#|][^\]]*)?\]\]`, global, capture 1 trimmed and
/// de-duplicated in first-seen order.
///
/// Hand-matched rather than a regex dependency. The pattern has no
/// backtracking that can succeed after the greedy attempt fails, so a single
/// forward pass per start position gives the same matches.
fn link_stems(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < body.len() {
        let rest = &body[i..];
        if let Some((stem, len)) = match_link(rest) {
            let stem = js::trim(stem);
            if !stem.is_empty() && !out.iter().any(|s| s == stem) {
                out.push(stem.to_string());
            }
            i += len;
        } else {
            i += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    out
}

/// A link at the start of `s`: the raw capture and the match length in bytes.
fn match_link(s: &str) -> Option<(&str, usize)> {
    let inner = s.strip_prefix("[[")?;
    let cap_len = inner.find([']', '|', '#']).unwrap_or(inner.len());
    if cap_len == 0 {
        return None;
    }
    let capture = &inner[..cap_len];
    let after = &inner[cap_len..];
    let tail = if after.starts_with(['|', '#']) {
        // `[#|][^\]]*` then `\]\]`; the only other option, skipping the
        // group, needs `]]` where a `|` or `#` is.
        let alias = &after[1..];
        let stop = alias.find(']').unwrap_or(alias.len());
        &alias[stop..]
    } else {
        after
    };
    if !tail.starts_with("]]") {
        return None;
    }
    let len = s.len() - tail.len() + 2;
    Some((capture, len))
}

/// A note's heading line, without its newline.
///
/// With `dates`, a note whose `dates.json` entry has a created day carries
/// it inside the parentheses, and `estimated` when Apex marked it so. A note
/// with no entry, or a null day, gets the plain heading: nothing is guessed.
/// Without `dates` this is the ADR 0017 heading, byte for byte.
fn heading(note: &Note, marker: &str, dates: Option<&Document>) -> String {
    let created = dates
        .and_then(|d| d.notes.get(&note.path))
        .and_then(|e| e.created.day.as_deref().map(|d| (d, e.created.estimated)));
    match created {
        None => format!("### {} ({}){marker}", note.title, note.path),
        Some((day, estimated)) => format!(
            "### {} ({}, created {day}{}){marker}",
            note.title,
            note.path,
            if estimated { ", estimated" } else { "" }
        ),
    }
}

fn section(note: &Note, marker: &str, dates: Option<&Document>) -> String {
    let body = note
        .body
        .trim_start_matches('\n')
        .trim_end_matches(js::is_ws);
    format!("{}\n\n{body}\n", heading(note, marker, dates))
}

/// Compiles the context block for the note whose stem is `stem`.
///
/// Validates first and refuses an invalid vault outright: a context built
/// from a vault with a dangling parent would show a lineage that stops for no
/// visible reason. Nothing is written anywhere; the caller decides what to do
/// with the text.
pub fn compile(root: &Path, stem: &str, cap: usize) -> Result<Compiled, VaultError> {
    compile_with_dates(root, stem, cap, None)
}

/// [`compile`], with each lineage and reference heading annotated with the
/// note's created day from `dates` (`[context] dates = true`, ADR 0022 §7).
///
/// `None` is exactly [`compile`], which is how the goldens stay the
/// specification of the selection rule: the annotation adds bytes to
/// headings and changes nothing about which notes are selected. It does
/// count toward the cap, since it is sent.
pub fn compile_with_dates(
    root: &Path,
    stem: &str,
    cap: usize,
    dates: Option<&Document>,
) -> Result<Compiled, VaultError> {
    let v = parse(root)?;
    if !v.violations.is_empty() {
        return Err(VaultError::Invalid {
            violations: v.violations,
            stats: v.stats,
        });
    }
    let target = match v.lookup(stem) {
        [one] => *one,
        [] => return Err(VaultError::UnknownStem(stem.to_string())),
        many => {
            return Err(VaultError::AmbiguousStem {
                stem: stem.to_string(),
                paths: many.iter().map(|&i| v.notes[i].path.clone()).collect(),
            });
        }
    };
    let notes = &v.notes;

    // Validation guarantees every parent resolves to exactly one note and
    // that there are no cycles, so this terminates.
    let mut lineage = vec![target];
    let mut cur = target;
    while let Some(p) = v.parents[cur] {
        cur = p;
        lineage.insert(0, cur);
    }

    // Children and references resolve as Apex resolves them, so a parent or
    // a link written as `[[Title]]` counts (ADR 0022 §1). Membership is by
    // index, not by stem: `[[Root]]` must not add a second copy of a note
    // already in the lineage.
    let mut children: Vec<&Note> = (0..notes.len())
        .filter(|&i| v.parents[i] == Some(target))
        .map(|i| &notes[i])
        .collect();
    children.sort_by(|a, b| js::cmp(&a.stem, &b.stem).then_with(|| js::cmp(&a.path, &b.path)));

    let mut references: Vec<&Note> = Vec::new();
    let mut seen: Vec<usize> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();
    for s in link_stems(&notes[target].body) {
        match v.resolver.resolve(&s) {
            Resolved::Note(one) => {
                // Already present with its body, or already referenced under
                // another spelling.
                if lineage.contains(&one) || seen.contains(&one) {
                    continue;
                }
                seen.push(one);
                references.push(&notes[one]);
            }
            Resolved::Missing => unresolved.push(format!("- [[{s}]]")),
            Resolved::Ambiguous(many) => unresolved.push(format!(
                "- [[{s}]] (ambiguous: {})",
                many.iter()
                    .map(|&i| notes[i].path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    let head = format!(
        "# Context: {}\n\n## Lineage\n\n{}",
        notes[target].title,
        lineage
            .iter()
            .map(|&i| section(&notes[i], if i == target { " [target]" } else { "" }, dates))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let child_part = if children.is_empty() {
        "\n## Children\n\n(none)\n".to_string()
    } else {
        format!(
            "\n## Children\n\n{}\n",
            children
                .iter()
                .map(|c| format!("- {} ({})", c.title, c.path))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let unresolved_part = if unresolved.is_empty() {
        String::new()
    } else {
        format!("\n## Unresolved\n\n{}\n", unresolved.join("\n"))
    };

    let render = |keep_bodies: usize| -> String {
        let ref_part = if references.is_empty() {
            String::new()
        } else {
            // A dropped body keeps its bare heading. Placeholder text would
            // cost bytes and can exceed a short body, making the drop
            // pointless; the heading with nothing under it is the signal, and
            // `dropped_reference_bodies` is the count.
            let sections: Vec<String> = references
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    if i < keep_bodies {
                        section(n, "", dates)
                    } else {
                        format!("{}\n", heading(n, "", dates))
                    }
                })
                .collect();
            format!("\n## References\n\n{}", sections.join("\n"))
        };
        js::normalize(&format!("{head}{child_part}{ref_part}{unresolved_part}"))
    };

    for keep in (0..=references.len()).rev() {
        let text = render(keep);
        if text.len() <= cap {
            return Ok(Compiled {
                bytes: text.len(),
                text,
                dropped_reference_bodies: references.len() - keep,
            });
        }
    }
    Err(VaultError::OverCap {
        stem: stem.to_string(),
        minimum: render(0).len(),
        cap,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_match_the_twin_s_pattern() {
        assert_eq!(
            link_stems("See [[glossary]] and [[missing-note]]."),
            ["glossary", "missing-note"]
        );
        // Alias and heading forms keep only the stem; repeats collapse.
        assert_eq!(link_stems("[[a|A]] [[a#h]] [[b#h|x]]"), ["a", "b"]);
        // `[` is a legal capture character, so a third bracket is part of the
        // stem, as it is in the regex.
        assert_eq!(link_stems("[[[a]]"), ["[a"]);
        // Empty, unterminated, and a `]` inside the alias all fail to match.
        assert!(link_stems("[[]] [[a [[a|b]c]] [[a#").is_empty());
        assert_eq!(link_stems("[[ spaced ]]"), ["spaced"]);
        assert_eq!(link_stems("é[[ü]]"), ["ü"]);
    }
}
