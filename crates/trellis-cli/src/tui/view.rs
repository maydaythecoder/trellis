//! Drawing. Reads state, writes pixels, changes nothing.
//!
//! Every function here takes `&App` and a `&mut Frame`. The immutable borrow
//! is the guardrail: a view that could mutate is a view that can disagree
//! with the controller about what happened, and the disagreement shows up as
//! a frame that is one event stale.
//!
//! **Allocation.** `draw` runs on every tick. Anything that would allocate
//! per frame is either precomputed in the model or borrowed from it: log
//! lines are rendered as `&str` out of the buffer, and the only `String`s
//! built here are the few short ones whose inputs genuinely change each
//! frame, such as the spinner.

use super::model::{App, Focus, Job};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, Paragraph, Wrap};

/// Spinner frames. A `const` rather than a formatted string, so the animation
/// costs an index rather than an allocation.
const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;

pub(crate) fn draw(frame: &mut Frame, app: &App) {
    let [header, body, input, status] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(5),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    // The sidebar is dropped below a width where it would squeeze the
    // transcript into a column. Hiding it is better than rendering both
    // badly, and the information it carries is also in the status line.
    let show_side = body.width >= 90;
    let [log_area, side_area] = if show_side {
        Layout::horizontal([Constraint::Min(40), Constraint::Length(34)]).areas(body)
    } else {
        [body, Rect::ZERO]
    };

    draw_header(frame, app, header);
    draw_log(frame, app, log_area);
    if show_side {
        draw_side(frame, app, side_area);
    }
    draw_input(frame, app, input);
    draw_status(frame, app, status);
}

fn focus_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(ACCENT)
    } else {
        Style::default().fg(MUTED)
    }
}

fn panel(title: &str, focused: bool) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(focus_style(focused))
        .title(Span::styled(title, focus_style(focused)))
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let model = app.model.as_deref().unwrap_or("none — /model to set one");
    let line = Line::from(vec![
        Span::styled("TRELLIS ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(app.index_line.as_str(), Style::default().fg(MUTED)),
        Span::raw("  model "),
        Span::styled(model, Style::default().fg(ACCENT)),
    ]);
    frame.render_widget(Paragraph::new(line).block(panel("repository", false)), area);
}

/// The transcript.
///
/// Rendered from the tail backwards so the newest line is always the one
/// pinned to the bottom edge, which is where a reader's eye already is.
fn draw_log(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Log;
    let block = panel("transcript", focused);
    let inner = block.inner(area);
    let height = inner.height as usize;

    // Clamped here rather than in the model: the height is a property of the
    // frame, and a model that knew it would have to be told again on resize.
    let end = app.log.len().saturating_sub(app.scroll);
    let start = end.saturating_sub(height);

    let items: Vec<ListItem> = app
        .log
        .iter()
        .take(end)
        .skip(start)
        .map(|l| {
            let colour = match l.level {
                tracing::Level::ERROR => Color::Red,
                tracing::Level::WARN => Color::Yellow,
                tracing::Level::INFO => Color::Reset,
                _ => MUTED,
            };
            // Borrowed from the buffer: no per-frame String.
            ListItem::new(Line::from(Span::styled(
                l.text.as_str(),
                Style::default().fg(colour),
            )))
        })
        .collect();

    frame.render_widget(List::new(items).block(block), area);
}

fn draw_side(frame: &mut Frame, app: &App, area: Rect) {
    let [job_area, out_area] =
        Layout::vertical([Constraint::Length(5), Constraint::Min(3)]).areas(area);

    let job: Vec<Line> = match &app.job {
        Job::Idle => vec![Line::from(Span::styled(
            "idle — type a task below",
            Style::default().fg(MUTED),
        ))],
        Job::Running { task, frames } => vec![
            Line::from(vec![
                Span::styled(SPINNER[frames % SPINNER.len()], Style::default().fg(ACCENT)),
                Span::raw(" running"),
            ]),
            Line::from(Span::styled(task.as_str(), Style::default().fg(MUTED))),
        ],
        Job::Done(o) => vec![
            Line::from(if o.converged {
                Span::styled("converged", Style::default().fg(Color::Green))
            } else {
                Span::styled("not converged", Style::default().fg(Color::Yellow))
            }),
            Line::from(Span::styled(o.task.as_str(), Style::default().fg(MUTED))),
        ],
    };
    frame.render_widget(
        Paragraph::new(job)
            .wrap(Wrap { trim: true })
            .block(panel("run", false)),
        job_area,
    );

    let proposed: Vec<ListItem> = match &app.job {
        Job::Done(o) if !o.proposed.is_empty() => o
            .proposed
            .iter()
            .map(|p| ListItem::new(Line::from(p.as_str())))
            .collect(),
        _ => vec![ListItem::new(Line::from(Span::styled(
            "nothing proposed",
            Style::default().fg(MUTED),
        )))],
    };
    frame.render_widget(
        List::new(proposed).block(panel("files proposed", false)),
        out_area,
    );
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Input;
    let block = panel("task", focused);
    let inner = block.inner(area);
    frame.render_widget(
        Paragraph::new(Line::from(app.input.as_str())).block(block),
        area,
    );

    // A real cursor rather than a drawn block: it blinks the way the
    // terminal's own does, and a screen reader can find it.
    if focused {
        let x = inner.x + (app.input.chars().count() as u16).min(inner.width.saturating_sub(1));
        frame.set_cursor_position((x, inner.y));
    }
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let keys = match app.focus {
        Focus::Input => "enter run   tab transcript   ctrl-c quit",
        Focus::Log => "up/down scroll   end follow   tab task   ctrl-c quit",
    };
    let line = Line::from(vec![
        Span::styled(app.status.as_str(), Style::default().fg(ACCENT)),
        Span::raw("   "),
        Span::styled(keys, Style::default().fg(MUTED)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::events::LogLine;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(app: &App, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| draw(f, app)).unwrap();
        // One string of the whole screen, so assertions read as "is this on
        // screen" rather than as coordinates that break on any re-layout.
        t.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    fn app() -> App {
        App::new("451 files".into(), Some("scripted:x".into()))
    }

    #[test]
    fn the_whole_interface_renders_without_panicking_at_awkward_sizes() {
        // A TUI's most common crash is a layout that assumes room it does not
        // have. Ratatui panics on an out-of-bounds area, so rendering at all
        // is the assertion.
        let mut a = app();
        for i in 0..200 {
            a.push_log(LogLine {
                level: tracing::Level::INFO,
                text: format!("line {i}"),
            });
        }
        for (w, h) in [(120, 40), (90, 20), (80, 12), (40, 10), (20, 8)] {
            let _ = render(&a, w, h);
        }
    }

    #[test]
    fn the_sidebar_is_dropped_rather_than_squeezed_when_narrow() {
        let a = app();
        assert!(render(&a, 120, 30).contains("files proposed"));
        assert!(
            !render(&a, 70, 30).contains("files proposed"),
            "a squeezed sidebar costs the transcript the width it needs"
        );
    }

    #[test]
    fn the_transcript_shows_the_newest_lines_by_default() {
        // Following the tail is the default because the line a reader wants
        // is almost always the last one.
        let mut a = app();
        for i in 0..100 {
            a.push_log(LogLine {
                level: tracing::Level::INFO,
                text: format!("entry{i}"),
            });
        }
        let screen = render(&a, 100, 24);
        assert!(
            screen.contains("entry99"),
            "the newest line must be visible"
        );
        assert!(
            !screen.contains("entry0 "),
            "the oldest must have scrolled off"
        );
    }

    #[test]
    fn scrolling_back_moves_the_window_off_the_tail() {
        let mut a = app();
        for i in 0..100 {
            a.push_log(LogLine {
                level: tracing::Level::INFO,
                text: format!("entry{i}"),
            });
        }
        a.scroll_up(50);
        let screen = render(&a, 100, 24);
        assert!(
            !screen.contains("entry99"),
            "scrolled back, the tail is gone"
        );
        assert!(screen.contains("entry49"));
    }

    #[test]
    fn the_key_hints_follow_the_focus() {
        // The hints are the only discoverability a TUI has; showing the wrong
        // pane's keys is worse than showing none.
        let mut a = app();
        assert!(render(&a, 100, 20).contains("enter run"));
        a.toggle_focus();
        assert!(render(&a, 100, 20).contains("scroll"));
    }
}
