//! Apex's reference primitives: `noteRefs.ts`, and the two link scanners
//! (`graphModel.ts#parseWikiRefsWithMultiplicity` for edges,
//! `appShell.ts#rewriteBodyWikiLinks` for rewrites).
//!
//! The two scanners differ on purpose, because Apex's do: the graph counts a
//! link inside a code fence, and a rewrite leaves it alone. Unifying them
//! would change either the weights Apex reports or the text Apex rewrites
//! (ADR 0022 §2).

use crate::js;
use unicode_normalization::UnicodeNormalization;

/// `String(v).normalize("NFKD")`, U+0300..U+036F removed, then `toLowerCase()`,
/// shared by `slugify` and the search normaliser.
pub(crate) fn fold(value: &str) -> String {
    value
        .nfkd()
        .filter(|c| !('\u{300}'..='\u{36f}').contains(c))
        .collect::<String>()
        .to_lowercase()
}

/// `noteRefs.ts#slugify`.
///
/// NFKD before the mark strip is what makes `Café` slug to `cafe` rather than
/// `caf`; a slug that differs from Apex's is an alias that resolves in one
/// tool and not the other.
pub fn slugify(value: &str) -> String {
    let folded = fold(value);
    let mut out = String::with_capacity(folded.len());
    let mut dash = false;
    for c in folded.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    // `.slice(0, 64)` runs after the dash trim, so a slug cut mid-word can
    // end in `-`. Reproduced, not corrected: the slug is a key, and keys
    // must agree.
    trimmed.chars().take(64).collect()
}

/// `^notes/` (case-insensitive) removed.
fn strip_notes_prefix(s: &str) -> &str {
    if s.len() >= 6 && s.is_char_boundary(6) && s[..6].eq_ignore_ascii_case("notes/") {
        &s[6..]
    } else {
        s
    }
}

/// `\.md$` (case-insensitive) removed.
pub(crate) fn strip_md(s: &str) -> &str {
    let n = s.len();
    if n >= 3 && s.is_char_boundary(n - 3) && s[n - 3..].eq_ignore_ascii_case(".md") {
        &s[..n - 3]
    } else {
        s
    }
}

pub(crate) fn has_md_ext(s: &str) -> bool {
    strip_md(s).len() != s.len()
}

/// `noteRefs.ts#normalizeKey`.
pub fn normalize_key(value: &str) -> String {
    js::trim(strip_md(strip_notes_prefix(value))).to_lowercase()
}

/// A parsed `[[target|label]]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiTarget {
    /// Cleaned: `notes/` and `.md` removed, trimmed. Empty when there is no
    /// target.
    pub target: String,
    /// The text after the first `|`, trimmed; else the last path segment of
    /// the target.
    pub label: String,
}

/// The first `\[\[([^\]]+)\]\]` in `s`: `(start, end, group)` in bytes.
///
/// The group cannot contain `]`, so a greedy match that fails cannot succeed
/// shorter; one forward attempt per `[[` is the whole regex.
fn next_link(s: &str, from: usize) -> Option<(usize, usize, &str)> {
    let mut i = from;
    while let Some(off) = s.get(i..)?.find("[[") {
        let p = i + off;
        let inner = &s[p + 2..];
        let len = inner.find(']').unwrap_or(inner.len());
        if len > 0 && inner[len..].starts_with("]]") {
            return Some((p, p + 2 + len + 2, &inner[..len]));
        }
        i = p + 1;
    }
    None
}

/// `noteRefs.ts#parseWikiTarget` for a string argument.
pub fn parse_wiki_target(value: &str) -> WikiTarget {
    // `referenceString`: an empty value or the literal "null" is nothing.
    let raw = if value == "null" { "" } else { value };
    // `unwrapWikiSyntax`: the anchored form and the unanchored form both
    // take the leftmost `[[…]]`, so one search covers both.
    let raw = next_link(raw, 0).map_or(raw, |(_, _, g)| g);
    let (first, rest) = match raw.split_once('|') {
        Some((a, b)) => (a, Some(b)),
        None => (raw, None),
    };
    let target = js::trim(strip_md(strip_notes_prefix(first))).to_string();
    let alias = rest.map(js::trim).unwrap_or("");
    let label = if !alias.is_empty() {
        alias.to_string()
    } else {
        match target.rsplit('/').next() {
            Some(last) if !last.is_empty() => last.to_string(),
            _ => target.clone(),
        }
    };
    WikiTarget { target, label }
}

/// `noteRefs.ts#cleanWikiRef`.
pub(crate) fn clean_wiki_ref(value: &str) -> Option<String> {
    let t = parse_wiki_target(value).target;
    (!t.is_empty() && t != "null").then_some(t)
}

/// `noteRefs.ts#getNoteAliasKeys`, in Apex's order, empties dropped.
pub(crate) fn alias_keys(path: &str, title: &str) -> Vec<String> {
    let no_ext = strip_md(path);
    let base = no_ext
        .rsplit('/')
        .next()
        .filter(|b| !b.is_empty())
        .unwrap_or(no_ext);
    [
        path.to_string(),
        no_ext.to_string(),
        base.to_string(),
        title.to_string(),
        slugify(no_ext),
        slugify(base),
        slugify(title),
    ]
    .into_iter()
    .filter(|k| !k.is_empty())
    .map(|k| normalize_key(&k))
    .collect()
}

/// Every body link, with multiplicity, as the graph counts them.
pub(crate) fn graph_links(body: &str) -> Vec<WikiTarget> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some((_, end, g)) = next_link(body, i) {
        let t = parse_wiki_target(g);
        if !t.target.is_empty() {
            out.push(t);
        }
        i = end;
    }
    out
}

/// `replaceFencedCodeWithSpaces`: each ```` ```…``` ```` span, non-greedy,
/// becomes spaces.
///
/// Spaces of the same *byte* length, where Apex uses the same UTF-16 length:
/// offsets into the masked text must be offsets into the original, and in
/// Rust those are bytes. The one place length is counted (the 240 limit)
/// counts the original's UTF-16 units, so the two agree.
fn mask_fences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(off) = text[i..].find("```") {
        let start = i + off;
        let Some(close) = text[start + 3..].find("```") else {
            break;
        };
        let end = start + 3 + close + 3;
        out.push_str(&text[i..start]);
        out.extend(std::iter::repeat_n(' ', end - start));
        i = end;
    }
    out.push_str(&text[i..]);
    out
}

/// One link as a rewrite sees it.
pub(crate) struct RewriteLink<'a> {
    pub(crate) parsed: WikiTarget,
    /// The text between the brackets, from the original.
    pub(crate) inner: &'a str,
}

/// `appShell.ts#rewriteBodyWikiLinks`. `transform` returns the replacement
/// markup, or `None` to leave the link as it is. Returns the new body and
/// the number of links replaced.
pub(crate) fn rewrite_links(
    body: &str,
    mut transform: impl FnMut(&RewriteLink<'_>) -> Option<String>,
) -> (String, usize) {
    let masked = mask_fences(body);
    let mut out = String::with_capacity(body.len());
    let mut last = 0;
    let mut count = 0;
    let mut i = 0;
    while let Some(off) = masked[i..].find("[[") {
        let p = i + off;
        let rest = &masked[p + 2..];
        let len = rest.find([']', '\n']).unwrap_or(rest.len());
        let units = body[p + 2..p + 2 + len].encode_utf16().count();
        if len == 0 || units > 240 || !rest[len..].starts_with("]]") {
            i = p + 1;
            continue;
        }
        let end = p + 2 + len + 2;
        // `exec` has already advanced past a match it then skips.
        i = end;
        if p > 0 && body.as_bytes()[p - 1] == b'!' {
            continue;
        }
        let original = &body[p..end];
        let inner = &body[p + 2..end - 2];
        let link = RewriteLink {
            parsed: parse_wiki_target(inner),
            inner,
        };
        match transform(&link) {
            Some(r) if r != original => {
                out.push_str(&body[last..p]);
                out.push_str(&r);
                last = end;
                count += 1;
            }
            _ => {}
        }
    }
    if count == 0 {
        return (body.to_string(), 0);
    }
    out.push_str(&body[last..]);
    (out, count)
}

/// `wikiAliasFromInner`.
pub(crate) fn alias_from_inner(inner: &str) -> &str {
    inner.split_once('|').map_or("", |(_, a)| js::trim(a))
}

/// `wikiLinkMarkup`.
pub(crate) fn link_markup(target: &str, alias: &str) -> String {
    let (t, a) = (js::trim(target), js::trim(alias));
    if a.is_empty() {
        format!("[[{t}]]")
    } else {
        format!("[[{t}|{a}]]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Vectors from Apex's own `web/noteRefs.ts` at f449644a, run under Node
    // v25.6.0 (see ADR 0022, "Evidence"):
    //   slugify("Café Crème")      -> "cafe-creme"
    //   slugify("  Hello, World! ") -> "hello-world"
    //   slugify("Ωmega ﬁle")        -> "mega-file"   (NFKD splits the ligature)
    //   normalizeKey("notes/Sub/A.MD") -> "sub/a"
    //   parseWikiTarget("[[notes/x/y.md | Label ]]") -> {ref:"x/y.md", label:"Label"}
    //     (the trim runs after the `.md` strip, so a trailing space keeps it)
    //   getNoteAliasKeys("sub/Task B.md", "Task B") ->
    //     ["sub/task b","sub/task b","task b","task b","sub-task-b","task-b","task-b"]
    //   parseWikiTarget("x/y") -> {ref:"x/y", label:"y"}
    #[test]
    fn slug_and_key_match_apex_vectors() {
        // A slug that disagrees with Apex's is an alias one tool resolves
        // and the other does not.
        assert_eq!(slugify("Café Crème"), "cafe-creme");
        assert_eq!(slugify("  Hello, World! "), "hello-world");
        assert_eq!(slugify("Ωmega ﬁle"), "mega-file");
        assert_eq!(normalize_key("notes/Sub/A.MD"), "sub/a");
        assert_eq!(
            parse_wiki_target("[[notes/x/y.md | Label ]]"),
            WikiTarget {
                target: "x/y.md".into(),
                label: "Label".into()
            }
        );
        assert_eq!(parse_wiki_target("x/y").label, "y");
        assert_eq!(
            alias_keys("sub/Task B.md", "Task B"),
            [
                "sub/task b",
                "sub/task b",
                "task b",
                "task b",
                "sub-task-b",
                "task-b",
                "task-b"
            ]
        );
        assert_eq!(clean_wiki_ref("null"), None);
        assert_eq!(clean_wiki_ref("[[null]]"), None);
    }

    #[test]
    fn slug_is_cut_after_the_dash_trim_as_apex_does() {
        let s = slugify(&format!("{} tail", "a".repeat(63)));
        assert_eq!(s.len(), 64);
        assert!(s.ends_with('-'), "{s}");
    }

    #[test]
    fn graph_links_count_repeats_and_ignore_fences() {
        // The graph weights a repeated link, and does not mask code fences:
        // Apex's graph counts a link inside a fence.
        let l = graph_links("[[a]] [[a|x]] ```[[b]]``` [[]] ![[c]]");
        let t: Vec<_> = l.iter().map(|w| w.target.as_str()).collect();
        assert_eq!(t, ["a", "a", "b", "c"]);
    }

    #[test]
    fn rewrites_skip_fences_embeds_and_overlong_links() {
        let body = format!(
            "[[a|keep]] ```\n[[a]]\n``` ![[a]] [[{}]] [[a]]",
            "a".repeat(241)
        );
        let (out, n) = rewrite_links(&body, |l| {
            (l.parsed.target == "a").then(|| link_markup("B", alias_from_inner(l.inner)))
        });
        assert_eq!(n, 2);
        assert!(
            out.starts_with("[[B|keep]] ```\n[[a]]\n``` ![[a]] "),
            "{out}"
        );
        assert!(out.ends_with(" [[B]]"), "{out}");
    }

    #[test]
    fn md_is_stripped_after_a_multibyte_character() {
        assert_eq!(strip_md("é.md"), "é");
        assert_eq!(strip_md("é.MD"), "é");
    }

    #[test]
    fn graph_links_resume_exactly_after_each_link() {
        // Resuming early can land inside a multi-byte character and end the
        // scan; resuming late skips a link that follows.
        let t = |s: &str| -> Vec<String> { graph_links(s).into_iter().map(|w| w.target).collect() };
        // `€` is three bytes, so two short of the `]]` is inside it.
        assert_eq!(t("[[a€]] [[b]]"), ["a€", "b"]);
        assert_eq!(t("[[abcde]][[f]]"), ["abcde", "f"]);
        assert_eq!(t("[[a]] [[b]] [[c]]"), ["a", "b", "c"]);
        // An unterminated or empty link is not one.
        assert!(t("[[a]x").is_empty());
        assert_eq!(parse_wiki_target("[[]]").target, "[[]]");
    }

    #[test]
    fn a_target_ending_in_a_slash_is_its_own_label() {
        assert_eq!(parse_wiki_target("x/").label, "x/");
    }

    #[test]
    fn a_fence_masks_exactly_through_its_closing_backticks() {
        // A mask that stopped short would leave the closing fence to open a
        // new one, hiding the link between two fences.
        let (out, n) = rewrite_links("```x``` [[a]] ```y```", |_| Some("B".into()));
        assert_eq!((out.as_str(), n), ("```x``` B ```y```", 1));
    }

    #[test]
    fn a_rewrite_skips_empty_and_overlong_links_and_counts_only_real_changes() {
        let body = format!("[[]] [[{}]] [[{}]] [[c]]", "a".repeat(241), "b".repeat(240));
        let (out, n) = rewrite_links(&body, |l| (l.parsed.target != "c").then(|| "X".to_string()));
        assert_eq!(n, 1);
        assert_eq!(out, format!("[[]] [[{}]] X [[c]]", "a".repeat(241)));
        // A transform that returns the link unchanged rewrote nothing.
        let (out, n) = rewrite_links("[[a]] [[b]]", |l| Some(format!("[[{}]]", l.inner)));
        assert_eq!((out.as_str(), n), ("[[a]] [[b]]", 0));
    }
}
