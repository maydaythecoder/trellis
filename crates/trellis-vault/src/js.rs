//! The JavaScript primitives the reference implementation leans on.
//!
//! Each one differs from its Rust counterpart on some input. None of the
//! differences show on the fixture, which is exactly why they are written
//! down here rather than left to be discovered.

use std::cmp::Ordering;

/// What JavaScript's `\s` and `String.prototype.trim` treat as whitespace.
///
/// Not `char::is_whitespace`: that includes U+0085 and excludes U+FEFF, and
/// JavaScript does the reverse.
pub(crate) fn is_ws(c: char) -> bool {
    const SINGLES: &[char] = &[
        '\t', '\n', '\u{0B}', '\u{0C}', '\r', ' ', '\u{A0}', '\u{1680}', '\u{2028}', '\u{2029}',
        '\u{202F}', '\u{205F}', '\u{3000}', '\u{FEFF}',
    ];
    SINGLES.contains(&c) || ('\u{2000}'..='\u{200A}').contains(&c)
}

/// Whether `.` in a JavaScript regex (no `s` flag) refuses this character.
pub(crate) fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

pub(crate) fn trim(s: &str) -> &str {
    s.trim_matches(is_ws)
}

/// JavaScript's default string order: UTF-16 code units, not bytes.
///
/// The two agree everywhere below U+E000 and disagree above it, where a
/// supplementary-plane character (a surrogate pair, 0xD8xx) sorts *before*
/// U+E000..=U+FFFF in UTF-16 and after them in UTF-8. The manifest's sort
/// check has to agree with the TypeScript twin on the same vault.
pub(crate) fn cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// Strips a leading BOM and turns CRLF and lone CR into LF.
///
/// CRLF comes from git's autocrlf on Windows; everything downstream sees LF
/// only, so a checkout's line-ending setting cannot change the bytes the
/// model is shown.
pub(crate) fn normalize(text: &str) -> String {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_is_utf16_not_utf8() {
        // U+1F600 is a surrogate pair in UTF-16 (0xD83D ...), U+FF21 is one
        // unit. Byte order puts the emoji last; JavaScript puts it first.
        assert_eq!(cmp("\u{1F600}", "\u{FF21}"), Ordering::Less);
        assert_eq!("\u{1F600}".cmp("\u{FF21}"), Ordering::Greater);
    }

    #[test]
    fn line_endings_and_bom_normalise() {
        assert_eq!(normalize("\u{FEFF}a\r\nb\rc\n"), "a\nb\nc\n");
    }

    #[test]
    fn whitespace_follows_javascript() {
        assert_eq!(trim("\u{FEFF} x \u{2028}"), "x");
        assert_eq!(trim("\u{85}x"), "\u{85}x");
    }
}
