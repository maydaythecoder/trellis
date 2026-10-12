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

/// `s` with every SGR sequence (`ESC [ <parameters> m`, the colour and
/// style codes) removed, and nothing else touched.
///
/// For the output of a command a shell with `FORCE_COLOR` or `CLICOLOR_FORCE`
/// exported colours regardless of the pipe: the codes are not information
/// to a model and are noise in a trail. Only SGR goes. A cursor move or an
/// erase is left for [`escape_controls`] to show, since what a command did
/// to the screen is worth seeing where what colour it chose is not.
pub fn strip_sgr(s: &str) -> String {
    if !s.contains('\x1b') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('\x1b') {
        out.push_str(&rest[..at]);
        let seq = &rest[at..];
        // Parameters are digits, `;` and the `:` of the underline and colour
        // subparameters; anything else is not SGR and stays as it was.
        let past_sgr = seq.strip_prefix("\x1b[").and_then(|p| {
            p.trim_start_matches(|c: char| c.is_ascii_digit() || c == ';' || c == ':')
                .strip_prefix('m')
        });
        match past_sgr {
            Some(tail) => rest = tail,
            None => {
                out.push('\x1b');
                rest = &seq[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_colour_codes_are_removed_and_nothing_else_is() {
        // Done when: unchanged in bytes except for the SGR sequences. A
        // cursor move, an erase and a bare escape all stay, and so does
        // every non-ASCII byte around them.
        assert_eq!(strip_sgr("\x1b[31mred\x1b[0m"), "red");
        assert_eq!(
            strip_sgr("\x1b[1;38;5;208mbold orange\x1b[m"),
            "bold orange"
        );
        assert_eq!(strip_sgr("\x1b[4:3mcurly\x1b[0m café"), "curly café");
        assert_eq!(strip_sgr("a\x1b[2Kb\x1b[Ac\x1b"), "a\x1b[2Kb\x1b[Ac\x1b");
        assert_eq!(strip_sgr("\x1b[31"), "\x1b[31", "a cut sequence is kept");
        assert_eq!(strip_sgr("plain\n"), "plain\n");
    }

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
