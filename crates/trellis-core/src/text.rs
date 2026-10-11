//! Text from a repository or a person, made safe to print.
//!
//! Lives here because trellis-loop and trellis-cli both print such text and
//! both depend on this crate; one copy means they cannot drift apart.

/// `s` with every control character replaced by its `escape_debug` form, so
/// the text stays on one line and cannot move the cursor or repaint the
/// screen.
///
/// Control characters are escaped rather than dropped: a path that holds one
/// is worth seeing, and dropping it would print a name that does not match
/// the file.
pub fn escape_controls(s: &str) -> String {
    // Most text has no control characters; skip the per-char rebuild for it.
    if !s.chars().any(char::is_control) {
        return s.to_string();
    }
    s.chars()
        .map(|c| {
            if c.is_control() {
                c.escape_debug().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_control_character_is_escaped_not_dropped() {
        assert_eq!(escape_controls("a\x1b[2Jb\n"), "a\\u{1b}[2Jb\\n");
    }

    #[test]
    fn plain_ascii_is_unchanged() {
        assert_eq!(escape_controls("src/app.py: ok"), "src/app.py: ok");
    }

    #[test]
    fn non_ascii_printable_text_is_unchanged() {
        assert_eq!(escape_controls("café — naïve"), "café — naïve");
    }
}
