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
            // The action graph gets the emphasis and the sidebar, both asked
            // for. `Fill(2)` against the panes below it: it is the pane that
            // answers "what is it doing", and the others qualify the answer.
            Constraint::Fill(2), // action graph
            Constraint::Fill(1), // edits
            Constraint::Fill(1), // checks
            // Six: three node rows, the caption, and two borders. At four
            // the graph was its title and one node, which reads as a broken
            // pane rather than as a small one.
            Constraint::Length(6), // radius graph
        ])
        .split(body[1]);

    render_status(state, rows[0], frame);
    render_transcript(state, body[0], frame);
    render_actions(state, right[0], frame);
    render_edits(state, right[1], frame);
    render_checks(state, right[2], frame);
    render_radius(state, right[3], frame);
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

/// The blast radius, drawn as a graph rather than as three numbers.
///
/// Built against `ui_examples/preview.webp`: nodes on the left, edges
/// converging on a focus node. Asked for on the side, which is where this
/// pane already was.
///
/// The three counts stay three counts. They come from different populations
/// and dividing them means nothing — doing so once printed "3.7% resolved"
/// for a resolver that hand-audits at 72% (ADR 0014). So the glyphs are
/// proportional *within* each row and never across them, and the caption
/// says what the whole figure is a floor on.
fn render_radius(state: &AppState, area: Rect, frame: &mut Frame) {
    let r = state.radius;
    let block = Block::default().borders(Borders::ALL).title(" radius ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width < 24 {
        return;
    }

    let rows: [(&str, usize, Color, char); 3] = [
        ("resolved", r.resolved, Color::Green, '●'),
        ("candidates", r.candidates, Color::Yellow, '◐'),
        ("unresolved", r.unresolved, Color::Red, '○'),
    ];
    // The junction each row contributes to the bracket. The middle row
    // carries it into the focus node.
    let connectors = ['┐', '┤', '┘'];

    let mut lines: Vec<Line> = Vec::new();
    for (i, (label, count, colour, glyph)) in rows.iter().enumerate() {
        let mut spans = vec![
            Span::styled(
                format!("  {label:<11}"),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("{count:>5} "),
                Style::default().fg(*colour).add_modifier(Modifier::BOLD),
            ),
            Span::styled(glyph.to_string(), Style::default().fg(*colour)),
            Span::styled(
                format!("─{}", connectors[i]),
                Style::default().fg(Color::DarkGray),
            ),
        ];
        if i == 1 {
            spans.push(Span::styled("──◆ ", Style::default().fg(Color::Magenta)));
            spans.push(Span::styled("reach", Style::default().fg(Color::Magenta)));
        }
        lines.push(Line::from(spans));
    }

    // Constraint 1, on the pane that would otherwise imply completeness.
    // Roughly three call edges in ten are statically invisible in Python, so
    // the resolved list is a floor and never a boundary.
    if inner.height as usize > lines.len() {
        lines.push(Line::from(Span::styled(
            "  a floor on reach, not a boundary",
            Style::default().fg(Color::DarkGray),
        )));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Tools the loop can call, in the order the prompt offers them.
///
/// A fixed list rather than a tally of what has been called, so the graph can
/// show a tool that was available and was *not* used. That difference is what
/// makes it a graph rather than a list of counts: a run that never read a file
/// before editing it looks different from one that did, and a pane built only
/// from what happened cannot show the difference.
const TOOLS: [&str; 4] = ["blast_radius", "read_file", "edit", "run_checks"];

/// The action graph: what this run has called, as converging edges.
///
/// The reference is `ui_examples/preview.webp` — nodes with edges meeting at a
/// focus — and the owner asked for it on the side rather than in the centre,
/// so it leads the right-hand column and the transcript keeps its width.
///
/// A filled node is a tool that ran, a ring one that did not. Counts sit
/// against the node they belong to. Nothing here says whether anything was
/// *correct*: that is the checks pane, and only an executed check can say it.
fn render_actions(state: &AppState, area: Rect, frame: &mut Frame) {
    let block = Block::default().borders(Borders::ALL).title(" actions ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width < 24 {
        return;
    }

    // Anything the model called that this list has not heard of still belongs
    // on the graph: a node missing because a constant was out of date would
    // make the picture a lie about what ran.
    let mut names: Vec<&str> = TOOLS.to_vec();
    for a in &state.actions {
        if !names.contains(&a.tool.as_str()) {
            names.push(&a.tool);
        }
    }

    let mid = names.len() / 2;
    let label_w = names.iter().map(|n| n.len()).max().unwrap_or(0);
    let mut lines: Vec<Line> = Vec::new();

    for (i, name) in names.iter().enumerate() {
        let calls = state.actions.iter().filter(|a| a.tool == *name).count();
        let wrote = state.actions.iter().any(|a| a.tool == *name && a.wrote);
        let connector = if i == 0 {
            '┐'
        } else if i == names.len() - 1 {
            '┘'
        } else {
            '┤'
        };
        let (glyph, colour) = if calls == 0 {
            ('○', Color::DarkGray)
        } else if wrote {
            // The one node whose colour carries a fact rather than a status:
            // this tool changed the tree.
            ('●', Color::Yellow)
        } else {
            ('●', Color::Green)
        };
        let label_style = if calls == 0 {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::Gray)
        };

        let mut spans = vec![
            Span::styled(format!("  {name:<label_w$}"), label_style),
            Span::styled(
                if calls == 0 {
                    "     ".to_string()
                } else {
                    format!("{calls:>4} ")
                },
                Style::default().fg(colour).add_modifier(Modifier::BOLD),
            ),
            Span::styled(glyph.to_string(), Style::default().fg(colour)),
            Span::styled(
                format!("─{connector}"),
                Style::default().fg(Color::DarkGray),
            ),
        ];
        if i == mid {
            let n = state.actions.len();
            spans.push(Span::styled("──◆ ", Style::default().fg(Color::Magenta)));
            spans.push(Span::styled(
                format!("{n} action{}", if n == 1 { "" } else { "s" }),
                Style::default().fg(Color::Magenta),
            ));
        }
        lines.push(Line::from(spans));
    }

    // What the graph cannot show in a node: how much of this reached the tree.
    // Said as a count of actions, never as a fraction of them — the two
    // populations a ratio would divide are not comparable (ADR 0014).
    if inner.height as usize > lines.len() {
        let wrote = state.actions.iter().filter(|a| a.wrote).count();
        lines.push(Line::from(Span::styled(
            if state.actions.is_empty() {
                "  nothing yet".to_string()
            } else if wrote == 0 {
                "  nothing written to the tree".to_string()
            } else {
                format!("  {wrote} wrote to the tree")
            },
            Style::default().fg(Color::DarkGray),
        )));
    }

    // The most recent actions, newest last, in whatever room is left. The
    // graph says what kind of work happened; this says what it was aimed at.
    let room = (inner.height as usize).saturating_sub(lines.len());
    if room > 0 && !state.actions.is_empty() {
        for a in state.actions.iter().rev().take(room).rev() {
            let mark = if a.wrote { '*' } else { '·' };
            lines.push(Line::from(Span::styled(
                truncate(
                    &format!("  {mark} {} {}", a.tool, a.target),
                    inner.width as usize,
                ),
                Style::default().fg(if a.wrote {
                    Color::Yellow
                } else {
                    Color::DarkGray
                }),
            )));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Cuts a line to the pane, saying so, rather than letting it wrap.
///
/// A wrapped row breaks the edge column and the graph stops reading as one
/// figure.
fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    s.chars()
        .take(width.saturating_sub(1))
        .chain(['…'])
        .collect()
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

    fn acted(actions: &[(&str, &str, bool)]) -> AppState {
        let mut s = populated();
        for (tool, target, wrote) in actions {
            s.update(Message::ActionTaken {
                tool: (*tool).into(),
                target: (*target).into(),
                wrote: *wrote,
            });
        }
        s
    }

    #[test]
    fn every_tool_is_a_node_before_any_of_them_is_called() {
        // A pane built only from what happened could not show what was
        // available and was not chosen, which is half of what makes this a
        // graph rather than a list of counts.
        let out = drawn(&populated(), 100, 40);
        for tool in TOOLS {
            assert!(out.contains(tool), "`{tool}` missing:\n{out}");
        }
        assert!(out.contains('○'), "an uncalled tool is a ring:\n{out}");
        assert!(out.contains("nothing yet"), "{out}");
    }

    #[test]
    fn a_refused_edit_is_on_the_graph_and_is_not_counted_as_a_write() {
        // The run this pane exists to show: four attempts, one write. Drawn
        // from successes alone it would look like a run that acted once.
        let s = acted(&[
            ("edit", "app/calc.py", false),
            ("edit", "app/calc.py", false),
            ("edit", "app/calc.py", true),
        ]);
        let out = drawn(&s, 100, 40);
        assert!(out.contains("3 actions"), "all three are actions:\n{out}");
        assert!(out.contains("1 wrote to the tree"), "one wrote:\n{out}");
    }

    #[test]
    fn a_run_that_only_looked_says_so_rather_than_leaving_it_blank() {
        let s = acted(&[("read_file", "app/calc.py", false)]);
        let out = drawn(&s, 100, 40);
        assert!(out.contains("1 action"), "{out}");
        assert!(!out.contains("1 actions"), "one action, not one actions");
        assert!(
            out.contains("nothing written to the tree"),
            "silence here reads as an unfinished pane:\n{out}"
        );
    }

    #[test]
    fn a_tool_this_view_has_never_heard_of_still_reaches_the_graph() {
        // A node missing because a constant was out of date would make the
        // picture a lie about what actually ran.
        let s = acted(&[("summon_kraken", "the deep", false)]);
        let out = drawn(&s, 100, 40);
        assert!(out.contains("summon_kraken"), "{out}");
    }

    #[test]
    fn the_edges_converge_on_one_focus() {
        let out = drawn(&acted(&[("edit", "a.py", true)]), 100, 40);
        assert_eq!(
            out.matches('◆').count(),
            2,
            "one focus for actions and one for radius, not one per row:\n{out}"
        );
    }

    #[test]
    fn a_long_target_is_cut_rather_than_wrapped_through_the_edges() {
        let long = "app/".to_string() + &"very_deep/".repeat(12) + "mod.py";
        let s = acted(&[("edit", &long, true)]);
        // Tall enough that the recent-action rows have room, narrow enough
        // that this path cannot fit in one.
        let out = drawn(&s, 80, 40);
        assert!(out.contains('…'), "a cut row says it was cut:\n{out}");
    }

    #[test]
    fn an_escape_sequence_in_a_transcript_never_reaches_the_terminal() {
        // The command-line half of this program escapes control characters
        // before printing trail content, because a terminal interprets what it
        // is given and a repaint can make a failed run look like a passed one.
        // The interface displays the same untrusted material — check output,
        // file paths, a previous run's context.
        //
        // It is safe, and for a reason outside this repository: ratatui builds
        // a cell buffer of graphemes and drops control characters on the way
        // in, so nothing this code renders can emit one. That is a dependency
        // behaviour the argument now rests on, so it is pinned here rather
        // than assumed — if an upgrade changes it, this fails instead of the
        // property silently disappearing.
        let mut s = populated();
        s.update(Message::AgentToken(
            "checking\x1b[2K\x1b[Aconverged\r\n\x07 done\n".into(),
        ));
        let out = drawn(&s, 100, 40);
        let escaped: Vec<u32> = out
            .chars()
            .filter(|c| c.is_control())
            .map(u32::from)
            .collect();
        assert!(
            escaped.is_empty(),
            "control characters reached the render buffer: {escaped:?}"
        );
        // And the legible part still arrives, so this is not passing because
        // the text was dropped wholesale.
        assert!(out.contains("checking"), "{out}");
    }

    #[test]
    fn the_graph_survives_a_pane_too_narrow_to_draw_it() {
        let s = acted(&[("edit", "a.py", true)]);
        for w in [80u16, 81, 90, 120] {
            let _ = drawn(&s, w, 24);
        }
    }
}
