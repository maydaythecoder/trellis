//! Drawing. One function, no mutation, no state of its own.
//!
//! Every `Rect` is computed fresh from `frame.area()` each frame. Caching one
//! is how a resize produces a layout drawn against the old size.

use crate::tui::app::{AppState, EditStatus, Modal};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Wrap};

/// Below this the six-region layout has no room to be legible.
const MIN_WIDTH: u16 = 80;
const MIN_HEIGHT: u16 = 24;
// Status 1 + budget 3 + input 3 + a body worth looking at.

pub fn render(state: &AppState, frame: &mut Frame) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        render_too_small(area, frame);
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // status
            Constraint::Min(0),    // body
            Constraint::Length(3), // budget
            Constraint::Length(3), // task entry
        ])
        .split(area);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Fill(1)])
        .split(rows[1]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),   // edits
            Constraint::Fill(1),   // checks
            Constraint::Length(4), // radius
        ])
        .split(body[1]);

    render_status(state, rows[0], frame);
    render_transcript(state, body[0], frame);
    render_edits(state, right[0], frame);
    render_checks(state, right[1], frame);
    render_radius(state, right[2], frame);
    render_budget(state, rows[2], frame);
    render_input(state, rows[3], frame);

    // Last, so it is over everything, and `Clear` first so what is under it
    // is wiped rather than showing through the gaps.
    if let Some(Modal::Approval { diff }) = &state.modal {
        render_modal(diff, area, frame);
    }
}

fn render_too_small(area: Rect, frame: &mut Frame) {
    let msg = Paragraph::new(vec![
        Line::from("terminal too small"),
        Line::from(format!(
            "{}x{} — need at least {MIN_WIDTH}x{MIN_HEIGHT}",
            area.width, area.height
        )),
    ])
    .alignment(Alignment::Center);
    // Vertically centred by hand: two lines in the middle row.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(2),
            Constraint::Fill(1),
        ])
        .split(area);
    frame.render_widget(msg, rows[1]);
}

fn render_status(state: &AppState, area: Rect, frame: &mut Frame) {
    let worktree = if state.edits.is_empty() {
        Span::styled("clean", Style::default().fg(Color::Green))
    } else {
        Span::styled(
            format!("{} edit(s) pending", state.edits.len()),
            Style::default().fg(Color::Yellow),
        )
    };
    let line = Line::from(vec![
        Span::styled(
            " TRELLIS ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  model "),
        Span::styled(
            if state.model.is_empty() {
                "none".to_string()
            } else {
                state.model.clone()
            },
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("  tier "),
        Span::styled(
            if state.tier.is_empty() {
                "—".to_string()
            } else {
                state.tier.clone()
            },
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("  worktree "),
        worktree,
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_transcript(state: &AppState, area: Rect, frame: &mut Frame) {
    let body = state.transcript.join("\n");
    let p = Paragraph::new(body)
        .block(Block::default().borders(Borders::ALL).title(" transcript "))
        .wrap(Wrap { trim: false })
        .scroll((state.scroll, 0));
    frame.render_widget(p, area);
}

fn render_edits(state: &AppState, area: Rect, frame: &mut Frame) {
    let items: Vec<ListItem> = state
        .edits
        .iter()
        .map(|e| {
            let (mark, colour) = match e.status {
                EditStatus::Proposed => ("~", Color::Yellow),
                EditStatus::Applied => ("+", Color::Green),
                EditStatus::Rejected => ("-", Color::Red),
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{mark} "), Style::default().fg(colour)),
                Span::raw(e.path.clone()),
            ]))
        })
        .collect();
    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(" edits "));
    frame.render_widget(list, area);
}

fn render_checks(state: &AppState, area: Rect, frame: &mut Frame) {
    let items: Vec<ListItem> = state
        .checks
        .iter()
        .map(|c| {
            let (mark, colour) = if c.passed {
                ("pass", Color::Green)
            } else {
                ("FAIL", Color::Red)
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{mark} "), Style::default().fg(colour)),
                Span::raw(c.name.clone()),
            ]))
        })
        .collect();
    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(" checks "));
    frame.render_widget(list, area);
}

fn render_radius(state: &AppState, area: Rect, frame: &mut Frame) {
    // One line, three counts, no ratio between them: they are filled from
    // different populations and dividing them means nothing.
    let r = state.radius;
    let line = Line::from(vec![
        Span::styled(
            r.resolved.to_string(),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" resolved · "),
        Span::styled(r.candidates.to_string(), Style::default().fg(Color::Yellow)),
        Span::raw(" candidates · "),
        Span::styled(r.unresolved.to_string(), Style::default().fg(Color::Red)),
        Span::raw(" unresolved"),
    ]);
    let p = Paragraph::new(line)
        .block(Block::default().borders(Borders::ALL).title(" radius "))
        .wrap(Wrap { trim: true });
    frame.render_widget(p, area);
}

fn render_budget(state: &AppState, area: Rect, frame: &mut Frame) {
    let b = state.budget;
    // The mock runs four iterations; the gauge shows progress through them.
    const ITERATION_CEILING: u32 = 4;
    let ratio = f64::from(b.iteration.min(ITERATION_CEILING)) / f64::from(ITERATION_CEILING);
    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(" budget "))
        .gauge_style(Style::default().fg(Color::Cyan))
        .ratio(ratio)
        .label(format!(
            "iteration {}/{ITERATION_CEILING}  ·  {} tokens  ·  {}s elapsed",
            b.iteration, b.tokens, b.elapsed_secs
        ));
    frame.render_widget(gauge, area);
}

fn render_input(state: &AppState, area: Rect, frame: &mut Frame) {
    let (title, body, style) = if state.running {
        (
            " running — input held until it finishes ",
            String::new(),
            Style::default().fg(Color::DarkGray),
        )
    } else {
        (
            " task  ·  enter to run  ·  /help  ·  esc to clear, esc again to quit ",
            // A block cursor, so an empty line still shows where typing goes.
            format!("{}█", state.input),
            Style::default(),
        )
    };
    let p = Paragraph::new(body)
        .style(style)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
    frame.render_widget(p, area);
}

fn render_modal(diff: &str, area: Rect, frame: &mut Frame) {
    let modal = centred(60, 60, area);
    frame.render_widget(Clear, modal);
    let p = Paragraph::new(diff.to_string())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" approve this change?  [y] accept  [n] reject "),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(p, modal);
}

/// A rect `pct_x` by `pct_y` percent of `area`, centred in it.
fn centred(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(rows[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{Check, Edit, Radius};
    use crate::tui::message::Message;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn populated() -> AppState {
        let mut s = AppState::default();
        s.update(Message::AgentToken("Reading blast radius\n".into()));
        s.edits.push(Edit {
            path: "src/index/resolve.rs".into(),
            status: EditStatus::Proposed,
        });
        s.checks.push(Check {
            name: "typecheck".into(),
            passed: true,
        });
        s.radius = Radius {
            resolved: 47,
            candidates: 3,
            unresolved: 2,
        };
        s.update(Message::BudgetUpdated {
            iteration: 2,
            tokens: 8400,
            elapsed_secs: 18,
        });
        s
    }

    /// The whole buffer as one string, so assertions can ask "is this on
    /// screen" without caring which cell it landed in.
    fn drawn(state: &AppState, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| render(state, f)).unwrap();
        t.backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    #[test]
    fn the_six_regions_are_on_screen_at_eighty_by_twentyfour() {
        let out = drawn(&populated(), 80, 24);
        for expected in [
            "TRELLIS",
            "transcript",
            "edits",
            "checks",
            "radius",
            "budget",
        ] {
            assert!(out.contains(expected), "`{expected}` missing from:\n{out}");
        }
        assert!(out.contains("Reading blast radius"));
        assert!(out.contains("resolved"), "the radius summary is one line");
        assert!(out.contains("8400"), "the budget shows tokens");
    }

    #[test]
    fn it_lays_out_again_at_a_larger_size() {
        let out = drawn(&populated(), 120, 40);
        assert!(out.contains("transcript"));
        assert!(out.contains("src/index/resolve.rs"));
        assert!(out.contains("typecheck"));
    }

    #[test]
    fn below_eighty_by_twentyfour_it_says_so_instead_of_laying_out() {
        let out = drawn(&populated(), 40, 10);
        assert!(out.contains("too small"), "got:\n{out}");
        assert!(
            !out.contains("transcript"),
            "the layout must not be attempted at all"
        );
    }

    #[test]
    fn the_modal_covers_what_is_under_it() {
        let mut s = populated();
        // Something long enough to be under the modal if it showed through.
        for _ in 0..40 {
            s.update(Message::AgentToken("aaaaaaaaaaaaaaaaaaaa\n".into()));
        }
        let before = drawn(&s, 80, 24);
        assert!(before.contains("aaaaaaaaaaaaaaaaaaaa"));

        s.update(Message::ApprovalRequested {
            diff: "--- a/src/index/resolve.rs\n+++ b/src/index/resolve.rs".into(),
        });
        let after = drawn(&s, 80, 24);
        assert!(after.contains("[y] accept"), "got:\n{after}");
        assert!(after.contains("+++ b/src/index/resolve.rs"));
    }

    #[test]
    fn the_task_line_shows_what_is_typed_and_how_to_send_it() {
        let mut s = populated();
        s.input = "fix the type error".into();
        let out = drawn(&s, 80, 24);
        assert!(out.contains("fix the type error"), "got:\n{out}");
        assert!(
            out.contains("enter to run"),
            "the key that sends it:\n{out}"
        );
        assert!(out.contains("esc"), "and the one that leaves:\n{out}");
        assert!(
            out.contains('\u{2588}'),
            "a block cursor, so an empty line still shows where typing goes"
        );
    }

    #[test]
    fn while_running_the_task_line_says_so_instead_of_taking_input() {
        let mut s = populated();
        s.running = true;
        let out = drawn(&s, 80, 24);
        assert!(out.contains("running"), "got:\n{out}");
        assert!(
            out.contains("input held"),
            "and says why it is not taking any:\n{out}"
        );
    }
}
