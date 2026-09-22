//! The conversation pane.
//!
//! `Paragraph` with wrapping inside a `tui-scrollview`, so a long answer
//! scrolls without the pane resizing under it. Tool output arrives carrying
//! ANSI, and `ansi-to-tui` turns that into styled `Text` rather than
//! printing the escape bytes — a `[0;31m` in the middle of a line is the
//! visible symptom of not doing this.

use crate::tui::app::{AppState, Focus, Role};
use crate::tui::theme::Theme;
use ansi_to_tui::IntoText;
use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, Wrap};
use tui_scrollview::{ScrollView, ScrollViewState};

/// Renders the conversation into `area`.
///
/// `scroll_state` is owned by the caller rather than by this function: a
/// `ScrollViewState` rebuilt every frame would forget the offset and snap
/// back to the top on each token.
pub fn render(
    f: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    scroll: &mut ScrollViewState,
) {
    let block = theme.pane("conversation", matches!(state.focus, Focus::Conversation));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let lines = wrap_into_lines(state, theme, inner.width);
    // The surface must be as tall as the content *after wrapping*, not as
    // the number of logical lines.
    //
    // Sizing it by `lines.len()` looked right and clipped silently: eight
    // turns are about seventeen logical lines, which fits a 28-row pane, so
    // the offset stayed 0 while the `Paragraph` wrapped them into fifty
    // rows and drew the first twenty-eight. The newest turns were rendered
    // into a surface too short to hold them and simply vanished — with no
    // scrollbar movement to suggest there was more.
    let height = u16::try_from(wrapped_height(&lines, inner.width).max(inner.height as usize))
        .unwrap_or(u16::MAX);
    let mut view = ScrollView::new(Size::new(inner.width, height));

    let para = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
    view.render_widget(para, Rect::new(0, 0, inner.width, height));

    // Following the tail is the default, and the offset that does it is a
    // property of the render — only here are both the content height and
    // the visible height known. `AppState` holds the *intent*; this turns
    // it into a number.
    let max_offset = height.saturating_sub(inner.height);
    let offset = if state.follow {
        max_offset
    } else {
        // `scroll_back` counts rows above the newest line, so the view is
        // always anchored to the tail rather than to an absolute position
        // that the next token invalidates.
        max_offset.saturating_sub(state.scroll_back)
    };
    scroll.set_offset(Position::new(0, offset));
    f.render_stateful_widget(view, inner, scroll);
}

/// How many rows `lines` occupies once wrapped to `width`.
///
/// `Paragraph`'s own wrapping is not observable before it draws, so this
/// mirrors it: character width per line, divided by the available columns,
/// at least one row each. It counts characters rather than grapheme
/// clusters, which over-counts for combining marks — wrong in the safe
/// direction, since a surface one row too tall scrolls harmlessly and one
/// row too short hides content.
fn wrapped_height(lines: &[Line<'_>], width: u16) -> usize {
    if width == 0 {
        return lines.len();
    }
    let w = width as usize;
    lines
        .iter()
        .map(|l| {
            let chars: usize = l.spans.iter().map(|s| s.content.chars().count()).sum();
            chars.div_ceil(w).max(1)
        })
        .sum()
}

/// One `Line` per conversation line, prefixed by role and styled.
///
/// Separated out so the styling is testable without a frame — the thing
/// worth checking here is that a user turn and an assistant turn cannot be
/// confused, which is a property of the text, not of the terminal.
pub fn wrap_into_lines<'a>(state: &'a AppState, theme: &Theme, width: u16) -> Vec<Line<'a>> {
    let mut out: Vec<Line<'a>> = Vec::new();
    for entry in &state.conversation {
        let (prefix, style) = match entry.role {
            Role::User => (
                "> ",
                if theme.colored() {
                    Style::default()
                        .fg(theme.active)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().add_modifier(Modifier::BOLD)
                },
            ),
            Role::Assistant => ("  ", Style::default().fg(theme.text)),
            Role::System => (
                "  ",
                if theme.colored() {
                    Style::default().fg(theme.dim)
                } else {
                    Style::default().add_modifier(Modifier::DIM)
                },
            ),
        };

        // Tool output may carry ANSI. Parsed rather than printed; on a parse
        // failure the raw text is shown, because losing the output entirely
        // would be worse than showing an escape sequence.
        if entry.text.contains('\u{1b}')
            && let Ok(text) = entry.text.as_bytes().into_text()
        {
            for l in text.lines {
                out.push(l);
            }
            continue;
        }

        for (i, chunk) in entry.text.split('\n').enumerate() {
            let p = if i == 0 { prefix } else { "  " };
            out.push(Line::from(vec![
                Span::styled(p.to_string(), style),
                Span::styled(chunk.to_string(), style),
            ]));
        }
    }

    // A spinner on its own line while a stream is open, so "thinking" and
    // "finished with nothing to say" are not the same picture.
    if state.streaming {
        const FRAMES: [&str; 4] = ["-", "\\", "|", "/"];
        let frame = FRAMES[state.spinner % FRAMES.len()];
        out.push(Line::from(Span::styled(
            format!("  {frame}"),
            theme.outcome(None),
        )));
    }

    let _ = width;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::message::Message;
    use crate::tui::theme::ColorDepth;

    fn plain(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn a_user_turn_and_an_assistant_turn_are_distinguishable() {
        let mut s = AppState::default();
        s.update(Message::Notice("sys".into()));
        s.conversation.push_back(crate::tui::app::Line {
            role: Role::User,
            text: "how does resolve_symbol work".into(),
        });
        s.update(Message::StreamToken("It walks the scope chain".into()));

        let theme = Theme::new(ColorDepth::Ansi16);
        let out = plain(&wrap_into_lines(&s, &theme, 40));
        assert!(out.iter().any(|l| l.starts_with("> how does")));
        assert!(out.iter().any(|l| l.starts_with("  It walks")));
    }

    #[test]
    fn a_multi_line_answer_keeps_its_line_breaks() {
        let mut s = AppState::default();
        s.update(Message::Notice("one\ntwo".into()));
        let theme = Theme::new(ColorDepth::Ansi16);
        let out = plain(&wrap_into_lines(&s, &theme, 40));
        assert_eq!(out.len(), 2, "{out:?}");
    }

    #[test]
    fn ansi_in_tool_output_becomes_style_rather_than_visible_escapes() {
        // The symptom of not doing this is `[0;31m` in the middle of a line.
        let mut s = AppState::default();
        s.conversation.push_back(crate::tui::app::Line {
            role: Role::System,
            text: "\u{1b}[31mFAILED\u{1b}[0m tests".into(),
        });
        let theme = Theme::new(ColorDepth::Ansi16);
        let out = plain(&wrap_into_lines(&s, &theme, 40));
        let joined = out.join("");
        assert!(joined.contains("FAILED"), "{joined:?}");
        assert!(
            !joined.contains('\u{1b}'),
            "escapes reached the screen: {joined:?}"
        );
        assert!(!joined.contains("[31m"), "{joined:?}");
    }

    #[test]
    fn the_surface_is_measured_after_wrapping_and_not_before() {
        // The clip that hid the newest turns. Seventeen logical lines fit a
        // 28-row pane; the same seventeen wrapped to 40 columns do not, and
        // sizing the surface by the count rather than the wrap drew the
        // first 28 rows and threw the rest away.
        let long = Line::from(Span::raw("x".repeat(200)));
        let short = Line::from(Span::raw("hi"));
        assert_eq!(wrapped_height(std::slice::from_ref(&short), 40), 1);
        assert_eq!(wrapped_height(std::slice::from_ref(&long), 40), 5);
        assert_eq!(wrapped_height(&[long, short], 40), 6);
        // An empty line still occupies a row.
        assert_eq!(wrapped_height(&[Line::from("")], 40), 1);
        // A zero-width pane must not divide by zero.
        assert_eq!(wrapped_height(&[Line::from("abc")], 0), 1);
    }

    #[test]
    fn a_spinner_shows_only_while_a_stream_is_open() {
        let mut s = AppState::default();
        let theme = Theme::new(ColorDepth::Ansi16);
        let before = wrap_into_lines(&s, &theme, 40).len();
        s.streaming = true;
        let during = wrap_into_lines(&s, &theme, 40).len();
        assert_eq!(during, before + 1, "thinking must look different to done");
    }
}
