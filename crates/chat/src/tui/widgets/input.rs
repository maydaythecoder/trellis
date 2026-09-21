//! The input line, the completion popup, and the confirmation modal.
//!
//! The cursor is drawn rather than positioned. `Frame::set_cursor_position`
//! would be the tidier answer, but the popup and the modal both float over
//! this line, and a terminal cursor sitting under a `Clear`ed region is
//! visible in a way that reads as a rendering fault. A block glyph is also
//! the only cursor that shows on an empty line.

use crate::tui::app::{AppState, Focus};
use crate::tui::theme::Theme;
use ratatui::prelude::*;
use ratatui::widgets::{Clear, List, ListItem, Paragraph};

/// The `> _` line.
pub fn render(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let focused = matches!(state.focus, Focus::Input) && state.confirm.is_none();
    let block = theme.pane("input", focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let prompt_style = if theme.colored() {
        Style::default().fg(theme.active)
    } else {
        Style::default()
    };

    let mut spans = vec![Span::styled("> ", prompt_style)];
    if state.streaming && state.input.is_empty() {
        spans.push(Span::styled(
            "streaming — input held, ^c to interrupt",
            Style::default().fg(theme.dim),
        ));
    } else {
        spans.push(Span::styled(
            state.input.clone(),
            Style::default().fg(theme.text),
        ));
        if focused {
            spans.push(Span::styled("\u{2588}", prompt_style));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), inner);
}

/// The completion popup, floated above the input line.
///
/// Drawn upward from `input_area` so it does not cover the line being
/// typed — a popup that hides the input makes the thing it is completing
/// invisible.
pub fn render_popup(f: &mut Frame, input_area: Rect, state: &AppState, theme: &Theme) {
    if !state.completion.open {
        return;
    }
    let specs = state.completion_specs();
    if specs.is_empty() {
        return;
    }

    let rows = u16::try_from(specs.len()).unwrap_or(u16::MAX).min(8);
    let height = rows + 2;
    let width = input_area.width.min(64);
    // Above the input if there is room, below it if there is not.
    let y = input_area.y.checked_sub(height).unwrap_or(
        input_area
            .y
            .saturating_add(input_area.height)
            .min(f.area().height.saturating_sub(height)),
    );
    let area = Rect::new(input_area.x, y, width, height).intersection(f.area());
    if area.height <= 2 || area.width == 0 {
        return;
    }

    let block = theme.pane("commands", true);
    let inner = block.inner(area);
    // `Clear` first: without it the pane underneath shows through the gaps
    // between the popup's own glyphs.
    f.render_widget(Clear, area);
    f.render_widget(block, area);

    let name_width = specs.iter().map(|s| s.name.len()).max().unwrap_or(0) + 2;
    let items: Vec<ListItem> = specs
        .iter()
        .enumerate()
        .skip(state.completion.selected.saturating_sub(rows as usize - 1))
        .take(rows as usize)
        .map(|(i, spec)| {
            let selected = i == state.completion.selected;
            let name_style = match (selected, spec.custom) {
                (true, _) => Style::default()
                    .fg(theme.active)
                    .add_modifier(Modifier::BOLD),
                // Custom commands are visually distinct because they are
                // prompts someone else may have written, running with the
                // user's credentials in the user's repository.
                (false, true) => Style::default().fg(theme.progress),
                (false, false) => Style::default().fg(theme.text),
            };
            let marker = if spec.custom { "*" } else { " " };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{marker}/{:<w$}", spec.name, w = name_width),
                    name_style,
                ),
                Span::styled(spec.description.clone(), Style::default().fg(theme.dim)),
            ]))
        })
        .collect();
    f.render_widget(List::new(items), inner);
}

/// The destructive-action modal. `y`/`n`, and no default.
pub fn render_confirm(f: &mut Frame, state: &AppState, theme: &Theme) {
    let Some(confirm) = &state.confirm else {
        return;
    };
    let area = centred(f.area(), 64, 5);
    let block = theme.pane("confirm", true).border_style(
        Style::default()
            .fg(theme.failure)
            .add_modifier(Modifier::BOLD),
    );
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);

    let text = Text::from(vec![
        Line::from(Span::styled(
            confirm.prompt.clone(),
            Style::default().fg(theme.text),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  y  do it        n / esc  cancel",
            Style::default().fg(theme.dim),
        )),
    ]);
    f.render_widget(
        Paragraph::new(text).wrap(ratatui::widgets::Wrap { trim: true }),
        inner,
    );
}

/// A `w`×`h` box in the middle of `area`, clamped to fit.
pub fn centred(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width.saturating_sub(w)) / 2,
        area.y + (area.height.saturating_sub(h)) / 2,
        w,
        h,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_centred_box_never_leaves_the_area() {
        // Called with a fixed size against whatever the terminal happens to
        // be; an 80-wide modal in a 40-wide terminal would otherwise index
        // past the buffer and panic.
        let small = Rect::new(0, 0, 10, 3);
        let r = centred(small, 64, 5);
        assert!(r.width <= small.width && r.height <= small.height);
        assert!(r.right() <= small.right() && r.bottom() <= small.bottom());
    }

    #[test]
    fn a_centred_box_is_actually_centred_when_it_fits() {
        let r = centred(Rect::new(0, 0, 100, 40), 64, 5);
        assert_eq!(r.x, 18);
        assert_eq!(r.y, 17);
    }
}
