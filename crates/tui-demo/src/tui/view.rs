//! Drawing. One function, no mutation, no state of its own.
//!
//! Every `Rect` is computed fresh from `frame.area()` each frame. Caching one
//! is how a resize produces a layout drawn against the old size.

use crate::tui::app::{AppState, Budget, EditStatus, Modal, Worktree};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Wrap};

/// Below this the six-region layout has no room to be legible.
const MIN_WIDTH: u16 = 80;
const MIN_HEIGHT: u16 = 24;
// Status 1 + budget 3 + input 3 + a body worth looking at.
/// The right column's floor: two borders and the radius pane's caption, "a
/// floor on reach, not a boundary", at 34 columns. The actions pane's widest
/// row is measured per frame (`actions_width`) and can raise it.
const RIGHT_MIN_WIDTH: u16 = 36;

/// The transcript pane's inner width at the narrowest layout drawn: its 60%
/// of `MIN_WIDTH`, less two borders. The transcript wraps to the most rows
/// here, so a scroll back by its row count at this width reaches the top at
/// any size the view lays out (`Home`, #660).
pub(crate) const MIN_TRANSCRIPT_WIDTH: u16 = MIN_WIDTH * 60 / 100 - 2;

pub fn render(state: &AppState, frame: &mut Frame) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        render_too_small(area, frame);
        return;
    }

    // The task entry grows by one row per `@` candidate, which it draws
    // under the typed line. Nothing is offered while a run is in flight,
    // because what is typed then is a steer, not a path request.
    let offered = if state.running {
        Vec::new()
    } else {
        state.at_file_candidates()
    };
    let entry_rows = 3 + u16::try_from(offered.len()).unwrap_or(u16::MAX);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),          // status
            Constraint::Min(0),             // body
            Constraint::Length(3),          // budget
            Constraint::Length(entry_rows), // task entry and `@` candidates
        ])
        .split(area);

    // The right column takes two fifths of the width, and never less than
    // its widest row needs: at 80 columns the two fifths were 32, and the
    // panes read "0 ac", "reac" and "not a boun" where their labels were
    // (#655). Below 100 columns the transcript gives the difference up,
    // since a cut figure is worse than a narrower transcript that wraps.
    let right_w = u16::try_from(u32::from(area.width) * 2 / 5)
        .unwrap_or(u16::MAX)
        .max(RIGHT_MIN_WIDTH)
        .max(actions_width(state).saturating_add(2));
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Fill(1), Constraint::Length(right_w)])
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
    render_input(state, &offered, rows[3], frame);

    // Last, so it is over everything, and `Clear` first so what is under it
    // is wiped rather than showing through the gaps.
    if let Some(Modal::Approval {
        question,
        diff,
        scroll,
    }) = &state.modal
    {
        render_modal(question, diff, *scroll, area, frame);
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
    let (green, yellow) = (
        Style::default().fg(Color::Green),
        Style::default().fg(Color::Yellow),
    );
    let worktree = match state.worktree() {
        Worktree::Clean => Span::styled("clean", green),
        Worktree::Pending(n) => Span::styled(format!("{n} edit(s) pending"), yellow),
        Worktree::Restored => Span::styled("restored", green),
        Worktree::LeftForReview(n) => Span::styled(format!("{n} file(s) left for review"), yellow),
    };
    let bold = Style::default().add_modifier(Modifier::BOLD);
    // The word for a tier nothing has reported, as `get_state` says it
    // (#519); a label never repeats the `tier ` written beside it.
    let tier = if state.tier.is_empty() {
        "unknown".to_string()
    } else {
        state.tier.clone()
    };
    let model = if state.model.is_empty() {
        "none".to_string()
    } else {
        state.model.clone()
    };
    // The worktree goes before the model, and the model is the span that is
    // cut, with "…": drawn last, the worktree state was pushed past column
    // 80 by a long spec and read "work" (#636).
    let mut spans = vec![
        Span::styled(
            " TRELLIS ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  worktree "),
        worktree,
        Span::raw("  model "),
    ];
    let tier_spans = [Span::raw("  tier "), Span::styled(tier, bold)];
    let used: usize = spans.iter().chain(tier_spans.iter()).map(Span::width).sum();
    let room = usize::from(area.width).saturating_sub(used).max(1);
    spans.push(Span::styled(truncate(&model, room), bold));
    spans.extend(tier_spans);
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Rows `text` takes when wrapped to `width` columns: greedy word wrap, as
/// the paragraph does, a word longer than the width breaking across rows.
/// Close enough to place the end of the transcript in view; it is a count
/// for scrolling, never a figure shown to anyone.
pub(crate) fn wrapped_rows(text: &str, width: u16) -> u16 {
    let width = usize::from(width.max(1));
    let mut rows: usize = 0;
    for line in text.split('\n') {
        let mut col = 0usize;
        let mut row_count = 1usize;
        for word in line.split(' ') {
            let w = Line::from(word).width();
            let need = if col == 0 { w } else { col + 1 + w };
            if need <= width {
                col = need;
            } else if w <= width {
                row_count += 1;
                col = w;
            } else {
                // A word wider than the row breaks across rows.
                let rest = if col == 0 { w } else { w + col + 1 };
                row_count += rest.div_ceil(width).saturating_sub(1);
                col = rest % width;
            }
        }
        rows += row_count;
    }
    u16::try_from(rows).unwrap_or(u16::MAX)
}

fn render_transcript(state: &AppState, area: Rect, frame: &mut Frame) {
    let body = state.transcript.join("\n");
    let inner_w = area.width.saturating_sub(2);
    let inner_h = area.height.saturating_sub(2);
    // Follow the end unless scrolled back (#244): the offset that puts the
    // last row at the bottom, less however far the person has scrolled.
    let total = wrapped_rows(&body, inner_w);
    let offset = total
        .saturating_sub(inner_h)
        .saturating_sub(state.scroll_back);
    let mut block = Block::default().borders(Borders::ALL).title(" transcript ");
    if state.scroll_back > 0 {
        block = block.title(" scrolled back · ↓ to follow ");
    }
    // The outcome of the last run, on the border, which does not scroll:
    // a run that ended is visible however long its transcript is.
    if let Some(outcome) = &state.outcome {
        block = block.title_bottom(Line::from(format!(" last run: {outcome} ")));
    }
    let p = Paragraph::new(body)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((offset, 0));
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

/// One node of the action graph: a tool, and its count as the row draws it.
struct ActionNode<'a> {
    name: &'a str,
    calls: usize,
    wrote: usize,
    /// Empty for an uncalled tool; `N` for one that only looked; `N · M
    /// wrote` for one that changed the tree, said in words and not only in
    /// yellow (#661), the way the desktop's graph says "3 calls · 1 wrote".
    count: String,
}

/// The tools on the graph, in the prompt's order, then any the model called
/// that `TOOLS` has not heard of: a node missing because a constant was out
/// of date would make the picture a lie about what ran.
fn action_nodes(state: &AppState) -> Vec<ActionNode<'_>> {
    let mut names: Vec<&str> = TOOLS.to_vec();
    for a in &state.actions {
        if !names.contains(&a.tool.as_str()) {
            names.push(&a.tool);
        }
    }
    names
        .into_iter()
        .map(|name| {
            let calls = state.actions.iter().filter(|a| a.tool == name).count();
            let wrote = state
                .actions
                .iter()
                .filter(|a| a.tool == name && a.wrote)
                .count();
            let count = match (calls, wrote) {
                (0, _) => String::new(),
                (n, 0) => n.to_string(),
                (n, w) => format!("{n} · {w} wrote"),
            };
            ActionNode {
                name,
                calls,
                wrote,
                count,
            }
        })
        .collect()
}

/// The columns the action graph's widest row takes, so the layout can give
/// the pane that much before it draws (#655): indent, label, count between
/// two spaces, glyph, edge, and the focus node with its action count.
fn actions_width(state: &AppState) -> u16 {
    let nodes = action_nodes(state);
    let label_w = nodes.iter().map(|n| n.name.len()).max().unwrap_or(0);
    let count_w = nodes
        .iter()
        .map(|n| n.count.chars().count())
        .max()
        .unwrap_or(0);
    let n = state.actions.len();
    let focus = format!("──◆ {n} action{}", if n == 1 { "" } else { "s" });
    let w = 2 + label_w + 1 + count_w + 1 + 1 + 2 + Span::raw(focus).width();
    u16::try_from(w).unwrap_or(u16::MAX)
}

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

    let nodes = action_nodes(state);
    let mid = nodes.len() / 2;
    let label_w = nodes.iter().map(|n| n.name.len()).max().unwrap_or(0);
    let count_w = nodes
        .iter()
        .map(|n| n.count.chars().count())
        .max()
        .unwrap_or(0);
    let mut lines: Vec<Line> = Vec::new();

    for (i, node) in nodes.iter().enumerate() {
        let (name, calls, wrote) = (node.name, node.calls, node.wrote);
        let connector = if i == 0 {
            '┐'
        } else if i == nodes.len() - 1 {
            '┘'
        } else {
            '┤'
        };
        let (glyph, colour) = if calls == 0 {
            ('○', Color::DarkGray)
        } else if wrote > 0 {
            // The node whose colour carries a fact rather than a status: this
            // tool changed the tree. The count says so in words too, for a
            // monochrome terminal or NO_COLOR (#661).
            ('●', Color::Yellow)
        } else {
            ('●', Color::Green)
        };
        let label_style = if calls == 0 {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::Gray)
        };

        // The counts are right-aligned to the widest, so the edge column
        // stays one column whatever a node's count says.
        let mut spans = vec![
            Span::styled(format!("  {name:<label_w$}"), label_style),
            Span::styled(
                format!(" {:>count_w$} ", node.count),
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
    // Progress against the ceiling the run reported (#243). Until it has,
    // no ceiling is drawn rather than a guessed one. The count can pass the
    // ceiling by one, during the loop's single escalation; the gauge is
    // full then and the label still says the real number.
    let ratio = if b.max_iterations == 0 {
        0.0
    } else {
        f64::from(b.iteration.min(b.max_iterations)) / f64::from(b.max_iterations)
    };
    // Turns count from 1: a 0 is a run that has not reported a turn, and
    // an unreported figure is never drawn as 0 (#641), here or below.
    let iteration = match (b.iteration, b.max_iterations) {
        (0, 0) => "no iteration yet".to_string(),
        (0, max) => format!("no iteration yet of {max}"),
        (n, 0) => format!("iteration {n}"),
        (n, max) => format!("iteration {n}/{max}"),
    };
    // A finished run does not keep presenting an iteration in progress
    // (#244): the counts stay, marked as the last run's.
    let iteration = if !state.running && state.outcome.is_some() {
        format!("finished · {iteration}")
    } else {
        iteration
    };
    let prompt = match b.prompt_bytes {
        Some(bytes) => format!("{bytes} B prompt"),
        None => "prompt not reported".to_string(),
    };
    let elapsed = match b.elapsed_secs {
        Some(secs) => format!("{} elapsed", clock(secs)),
        None => "not started".to_string(),
    };
    let (rate, queued) = turn_rate(&b);
    let block = Block::default().borders(Borders::ALL).title(" budget ");
    let label = budget_label(
        &iteration,
        &prompt,
        &elapsed,
        rate.as_deref(),
        queued.as_deref(),
        usize::from(block.inner(area).width),
    );
    let gauge = Gauge::default()
        .block(block)
        .gauge_style(Style::default().fg(Color::Cyan))
        .ratio(ratio)
        .label(label);
    frame.render_widget(gauge, area);
}

/// The budget line's segments joined with `  ·  `, dropping whole segments
/// from the tail until the line fits `width` (#655).
///
/// The gauge clips a long label mid-word ("· queu" at 80 columns), so a
/// segment that does not fit is left out rather than cut, in this order: the
/// token rate first, since it is the one figure derived from the others; then
/// the queue wait; then the prompt size. The iteration and the elapsed time
/// are never dropped, because they are what says the run is moving.
fn budget_label(
    iteration: &str,
    prompt: &str,
    elapsed: &str,
    rate: Option<&str>,
    queued: Option<&str>,
    width: usize,
) -> String {
    let mut keep = [true, true, true, true, true];
    let segments = [Some(iteration), Some(prompt), Some(elapsed), rate, queued];
    let join = |keep: &[bool; 5]| -> String {
        segments
            .iter()
            .zip(keep)
            .filter_map(|(s, k)| if *k { *s } else { None })
            .collect::<Vec<_>>()
            .join("  ·  ")
    };
    // Indices into `segments`, in the order they give way.
    for drop in [3, 4, 1] {
        if Span::raw(join(&keep)).width() <= width {
            break;
        }
        keep[drop] = false;
    }
    join(&keep)
}

/// Seconds as `m:ss`, so a minutes-long model call reads as time passing
/// rather than as a count (#641): "1:15" where "75s" needed arithmetic.
fn clock(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// The last turn's output rate and queue wait, as the budget line's tail.
///
/// So a slow turn can be told from a stuck one (#257). The rate is `coli
/// chat`'s: output tokens over the call's wall time, less the queue wait the
/// runtime reported separately (0 when it did not). The wall time still
/// includes prompt processing, so this reads below the decode rate. Nothing
/// is estimated: with no reported tokens the tail is empty, and with no model
/// time left after the wait there is no rate to draw. Two segments, so the
/// label can drop one without the other.
fn turn_rate(b: &Budget) -> (Option<String>, Option<String>) {
    let Some(tokens) = b.completion_tokens else {
        return (None, None);
    };
    let rate = b.model_ms.and_then(|model_ms| {
        let decoding_ms = model_ms.saturating_sub(b.queue_wait_ms.unwrap_or(0));
        (decoding_ms > 0).then(|| {
            let rate = tokens as f64 * 1000.0 / decoding_ms as f64;
            format!("{tokens} tok, {rate:.1} tok/s")
        })
    });
    let queued = b
        .queue_wait_ms
        .map(|queue_ms| format!("queued {:.1} s", queue_ms as f64 / 1000.0));
    (rate, queued)
}

fn render_input(state: &AppState, offered: &[&str], area: Rect, frame: &mut Frame) {
    let (title, lines, style) = if state.running {
        // Still typed into: Enter queues the line as a steer (#115).
        (
            " running — enter queues a steer for its next turn ",
            vec![Line::from(format!("{}█", state.input))],
            Style::default(),
        )
    } else {
        // A block cursor, so an empty line still shows where typing goes.
        let mut lines = vec![Line::from(format!("{}█", state.input))];
        // Each candidate on its own row, the first marked as the one Tab
        // inserts. Cut rather than wrapped: a wrapped path would take a row
        // the next candidate was given.
        let width = usize::from(area.width.saturating_sub(6));
        for (i, path) in offered.iter().enumerate() {
            let mark = if i == 0 { "tab " } else { "    " };
            lines.push(Line::from(Span::styled(
                format!("{mark}{}", truncate(path, width)),
                Style::default().fg(Color::DarkGray),
            )));
        }
        (
            " task  ·  enter to run  ·  /help  ·  esc to clear, esc again to quit ",
            lines,
            Style::default(),
        )
    };
    let p = Paragraph::new(lines)
        .style(style)
        .block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(p, area);
}

/// The approval box: the question on its top border, the answer keys on its
/// bottom one, and the diff between, scrolled from line `scroll` (#640).
///
/// The question used to come after the diff, and a long diff clipped it
/// along with its own tail, so a change was approved or declined unseen.
/// On the border it is drawn whatever the diff's length; the keys and
/// "lines a–b of n" are on rows the diff cannot push out either.
fn render_modal(question: &str, diff: &str, scroll: usize, area: Rect, frame: &mut Frame) {
    let modal = centred(60, 60, area);
    frame.render_widget(Clear, modal);
    // `edit` and `write_file` change the tree; anything else the gate asks
    // about (`bash`, `ask_human`, a custom tool) is a call.
    let what = if question.starts_with("edit ") || question.starts_with("write_file ") {
        "change"
    } else {
        "call"
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {question}? "))
        .title_bottom(format!(" [y] accept this {what}  [n] reject "));
    let inner = block.inner(modal);
    frame.render_widget(block, modal);

    let lines: Vec<&str> = diff.lines().collect();
    let rows = |l: &str| usize::from(wrapped_rows(l, inner.width));
    let height = usize::from(inner.height);
    let scrolls = lines.iter().map(|l| rows(l)).sum::<usize>() > height;
    // One row for the position when there is more than fits.
    let body_h = if scrolls {
        height.saturating_sub(1)
    } else {
        height
    };
    // The furthest down a view can start and still fill the box, so the
    // last page is a full one and `scroll` past it shows that page.
    let mut last_from = lines.len();
    let mut used = 0;
    while last_from > 0 && used + rows(lines[last_from - 1]) <= body_h {
        used += rows(lines[last_from - 1]);
        last_from -= 1;
    }
    let from = scroll.min(last_from);
    let mut to = from;
    let mut used = 0;
    while to < lines.len() && used + rows(lines[to]) <= body_h {
        used += rows(lines[to]);
        to += 1;
    }

    let body = Rect {
        height: u16::try_from(body_h).unwrap_or(inner.height),
        ..inner
    };
    // `from` is at most `last_from`, which is at most the line count.
    let shown = lines[from..].join("\n");
    frame.render_widget(Paragraph::new(shown).wrap(Wrap { trim: false }), body);
    if scrolls {
        let at = Rect {
            y: inner.y + body.height,
            height: 1,
            ..inner
        };
        let position = format!(
            "lines {}–{} of {} · ↑↓ PgUp PgDn",
            from + 1,
            to.max(from + 1),
            lines.len()
        );
        frame.render_widget(
            Paragraph::new(position).style(Style::default().add_modifier(Modifier::DIM)),
            at,
        );
    }
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
        s.update(Message::BudgetKnown { max_iterations: 40 });
        s.update(Message::BudgetUpdated {
            iteration: 2,
            prompt_bytes: Some(8400),
            elapsed_secs: 18,
            completion_tokens: None,
            model_ms: None,
            queue_wait_ms: None,
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

    /// A transcript several times taller than any pane, ending in a line
    /// that says so.
    fn long_transcript() -> AppState {
        let mut s = AppState::default().with_running(true);
        for i in 0..200 {
            s.update(Message::AgentToken(format!("step {i} of the run\n")));
        }
        s.update(Message::AgentToken("THE LAST LINE\n".into()));
        s
    }

    #[test]
    fn transcript_follows_its_end_while_new_lines_arrive() {
        // #244: a run's ending sat below the visible area, and a QA pass
        // read the frozen-looking pane as a hang for several minutes.
        let s = long_transcript();
        let out = drawn(&s, 100, 30);
        assert!(out.contains("THE LAST LINE"), "the end is in view");
        assert!(
            !out.contains("step 0 of the run"),
            "the start has scrolled off"
        );
    }

    #[test]
    fn transcript_scrolled_back_stops_following_and_says_so() {
        let mut s = long_transcript();
        for _ in 0..40 {
            s.update(Message::Key(ratatui::crossterm::event::KeyEvent::from(
                ratatui::crossterm::event::KeyCode::Up,
            )));
        }
        let out = drawn(&s, 100, 30);
        assert!(!out.contains("THE LAST LINE"), "scrolled back past the end");
        assert!(
            out.contains("scrolled back"),
            "and the border says how to return"
        );
    }

    #[test]
    fn worktree_state_is_whole_on_row_0_at_80_by_24_with_a_40_character_spec() {
        // #636: the worktree was drawn last, and at 80 columns a long spec
        // cut "worktree clean" to "work". It comes before the model now,
        // and the model is the span that gives way, saying it was cut.
        let spec = "llamacpp:http://127.0.0.1:8080#qwen3-8b";
        let spec = format!("{spec}{}", "x".repeat(40 - spec.len()));
        assert_eq!(spec.len(), 40);
        let mut s = AppState::with_model(spec.clone(), "unknown, probing…".into());
        let row = drawn_rows(&s, 80, 24).remove(0);
        assert!(row.contains("worktree clean"), "{row}");
        assert!(row.contains("tier unknown, probing…"), "{row}");
        assert!(row.contains('…') && !row.contains(&spec), "{row}");

        // The longest state the bar can say, after a converged run.
        s.edits = ["a.py", "b.py", "c.py"]
            .into_iter()
            .map(|p| crate::tui::app::Edit {
                path: p.into(),
                status: EditStatus::Proposed,
            })
            .collect();
        let row = drawn_rows(&s, 80, 24).remove(0);
        assert!(row.contains("worktree 3 file(s) left for review"), "{row}");
    }

    #[test]
    fn worktree_state_after_a_restored_run_has_no_edits_pending() {
        // #636: after a rolled-back run the bar said "N edit(s) pending" and
        // the edits pane listed files the rollback had already put back.
        let mut s = populated().with_running(true);
        assert!(drawn(&s, 80, 24).contains("1 edit(s) pending"));
        s.update(Message::RunOutcome {
            line: "did not converge; worktree restored: true".into(),
            restored: Some(true),
        });
        s.update(Message::RunFinished);
        let row = drawn_rows(&s, 80, 24).remove(0);
        assert!(!row.contains("edit(s) pending"), "{row}");
        assert!(row.contains("worktree restored"), "{row}");
    }

    #[test]
    fn transcript_outcome_of_a_finished_run_is_on_screen_without_scrolling() {
        let mut s = long_transcript();
        s.update(Message::BudgetKnown { max_iterations: 6 });
        s.update(Message::BudgetUpdated {
            iteration: 3,
            prompt_bytes: Some(2435),
            elapsed_secs: 42,
            completion_tokens: None,
            model_ms: None,
            queue_wait_ms: None,
        });
        s.update(Message::RunOutcome {
            line: "did not converge; worktree restored: true".into(),
            restored: Some(true),
        });
        s.update(Message::RunFinished);
        // Scrolled back too: the outcome does not depend on where the view is.
        for _ in 0..40 {
            s.update(Message::Key(ratatui::crossterm::event::KeyEvent::from(
                ratatui::crossterm::event::KeyCode::Up,
            )));
        }
        let out = drawn(&s, 120, 30);
        assert!(
            out.contains("last run: did not converge; worktree restored: true"),
            "{out}"
        );
        assert!(
            out.contains("finished · iteration 3/6"),
            "the budget line is not mid-run"
        );
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
        assert!(
            out.contains("8400 B prompt"),
            "the budget shows prompt bytes, as bytes"
        );
        assert!(
            out.contains("iteration 2/40"),
            "against the run's own ceiling"
        );
        assert!(out.contains("0:18 elapsed"), "{out}");
    }

    /// `populated()` after one turn that reported these, drawn wide enough
    /// that the gauge's centred label is not clipped.
    fn after_turn(tokens: Option<u64>, model_ms: Option<u64>, queue_ms: Option<u64>) -> String {
        let mut s = populated();
        s.update(Message::BudgetUpdated {
            iteration: 3,
            prompt_bytes: Some(8400),
            elapsed_secs: 40,
            completion_tokens: tokens,
            model_ms,
            queue_wait_ms: queue_ms,
        });
        drawn(&s, 140, 24)
    }

    #[test]
    fn budget_line_rate_excludes_queue_wait() {
        // The risk (#257): a rate over the whole call reads a turn that sat
        // in colibri's queue as a slow decoder. 30 tokens over 25 s less a
        // 10 s wait is 2.0 tok/s, not 1.2.
        let out = after_turn(Some(30), Some(25_000), Some(10_000));
        assert!(out.contains("30 tok, 2.0 tok/s"), "{out}");
        assert!(out.contains("queued 10.0 s"), "{out}");
    }

    #[test]
    fn budget_line_unknown_queue_wait_counts_as_zero_and_is_not_drawn() {
        // The risk: an unreported wait drawn as "queued 0.0 s" claims the
        // runtime said there was none.
        let out = after_turn(Some(30), Some(20_000), None);
        assert!(out.contains("30 tok, 1.5 tok/s"), "{out}");
        assert!(!out.contains("queued"), "{out}");
    }

    #[test]
    fn budget_line_without_reported_tokens_draws_no_rate_and_no_wait() {
        // The risk: a runtime that reports no usage shown as "0 tok, 0.0
        // tok/s", which reads as a stuck turn. Absent is drawn as absent.
        let out = after_turn(None, Some(25_000), Some(10_000));
        assert!(!out.contains("tok"), "{out}");
        assert!(!out.contains("queued"), "{out}");
        assert!(out.contains("iteration 3/40"), "the rest of the line stays");
    }

    #[test]
    fn budget_line_with_no_model_time_left_draws_no_rate() {
        // The risk: model_ms of 0, or a queue wait as long as the call,
        // divides by zero (or by a negative after subtraction) and draws
        // "inf" or a panic instead of nothing.
        for (model_ms, queue_ms) in [(0, None), (10_000, Some(10_000)), (5_000, Some(9_000))] {
            let out = after_turn(Some(30), Some(model_ms), queue_ms);
            assert!(!out.contains("tok/s"), "{model_ms} {queue_ms:?}: {out}");
            assert!(!out.contains("inf"), "{out}");
        }
        // And without a model time the tokens alone are no rate.
        let out = after_turn(Some(30), None, None);
        assert!(!out.contains("tok/s"), "{out}");
    }

    #[test]
    fn budget_unreported_figures_are_named_not_drawn_as_zero() {
        // #641: at launch the line read "iteration 0 · 0 B prompt · 0s
        // elapsed", three figures nothing had reported, and the desktop's
        // never-0 rule (render.js) was broken on the terminal.
        let out = drawn(&AppState::default(), 100, 24);
        assert!(!out.contains("0 B prompt"), "{out}");
        assert!(!out.contains("iteration 0"), "{out}");
        assert!(!out.contains("0s elapsed"), "{out}");
        assert!(out.contains("not reported"), "{out}");
        assert!(out.contains("no iteration yet"), "{out}");

        // A turn that measured nothing still has no size to draw, and the
        // ceiling is drawn without inventing a count against it.
        let mut s = AppState::default().with_running(true);
        s.update(Message::BudgetKnown { max_iterations: 40 });
        s.update(Message::BudgetUpdated {
            iteration: 2,
            prompt_bytes: None,
            elapsed_secs: 18,
            completion_tokens: None,
            model_ms: None,
            queue_wait_ms: None,
        });
        let out = drawn(&s, 120, 24);
        assert!(out.contains("prompt not reported"), "{out}");
        assert!(out.contains("iteration 2/40"), "{out}");
    }

    #[test]
    fn budget_unreported_elapsed_tick_is_drawn_as_a_clock() {
        // #641: the driver's tick between turns is what moves on screen
        // during a long model call; 75 s reads as 1:15.
        let mut s = AppState::default().with_running(true);
        s.update(Message::BudgetKnown { max_iterations: 40 });
        s.update(Message::Elapsed(75));
        let out = drawn(&s, 120, 24);
        assert!(out.contains("1:15 elapsed"), "{out}");
        assert!(out.contains("no iteration yet of 40"), "{out}");
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

    /// An approval of `question` with a diff of `n` numbered lines.
    fn asked(question: &str, n: usize) -> AppState {
        let mut s = populated().with_running(true);
        let diff: Vec<String> = (1..=n).map(|i| format!("+ line {i:02}")).collect();
        s.update(Message::ApprovalRequested {
            question: question.into(),
            diff: diff.join("\n"),
        });
        s
    }

    fn pressed(s: &mut AppState, code: ratatui::crossterm::event::KeyCode) {
        s.update(Message::Key(ratatui::crossterm::event::KeyEvent::from(
            code,
        )));
    }

    #[test]
    fn approval_modal_scroll_keeps_the_question_drawn_at_80_by_24_with_a_60_line_diff() {
        // #640: the question came after the diff, and a long diff clipped
        // it along with its own tail, so the change was answered unseen.
        let s = asked("edit a.py (+60 -0)", 60);
        let out = drawn(&s, 80, 24);
        assert!(out.contains(" edit a.py (+60 -0)? "), "{out}");
        assert!(out.contains("[y] accept this change  [n] reject"), "{out}");
        assert!(out.contains("+ line 01"), "{out}");
        assert!(out.contains("lines 1–") && out.contains(" of 60"), "{out}");
        assert!(!out.contains("+ line 60"), "the tail is below, not drawn");
    }

    #[test]
    fn approval_modal_scroll_page_down_reaches_the_last_line_and_y_still_answers() {
        use ratatui::crossterm::event::KeyCode;
        let mut s = asked("edit a.py (+60 -0)", 60);
        for _ in 0..10 {
            pressed(&mut s, KeyCode::PageDown);
        }
        let out = drawn(&s, 80, 24);
        assert!(out.contains("+ line 60"), "{out}");
        assert!(out.contains("–60 of 60"), "{out}");
        assert!(out.contains(" edit a.py (+60 -0)? "), "the question stays");

        pressed(&mut s, KeyCode::Char('q'));
        assert!(!s.should_quit && s.modal.is_some(), "q is still ignored");
        pressed(&mut s, KeyCode::Char('y'));
        assert!(s.modal.is_none());
        assert_eq!(s.answered, Some(true), "y still answers");
    }

    #[test]
    fn approval_modal_scroll_title_says_call_for_a_call_that_is_not_an_edit() {
        let out = drawn(&asked("bash", 3), 80, 24);
        assert!(out.contains(" bash? "), "{out}");
        assert!(out.contains("[y] accept this call  [n] reject"), "{out}");
        assert!(
            !out.contains("lines 1–"),
            "a diff that fits says no position: {out}"
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
            question: "edit a.py (+1 -1)".into(),
            diff: "--- a/src/index/resolve.rs\n+++ b/src/index/resolve.rs".into(),
        });
        let after = drawn(&s, 80, 24);
        assert!(after.contains("[y] accept"), "got:\n{after}");
        assert!(after.contains("+++ b/src/index/resolve.rs"));
    }

    #[test]
    fn approval_answer_keeps_edits_row_undecorated_and_the_decline_in_the_transcript() {
        // #635: after `n` the pane used to draw `- a.py` in red for a file
        // the loop had already written. The row keeps the mark it was
        // reported with, and the decline is the driver's transcript line.
        let mut s = AppState::default();
        s.update(Message::EditProposed {
            path: "a.py".into(),
        });
        s.update(Message::ApprovalRequested {
            question: "edit a.py (+1 -1)".into(),
            diff: "apply the edit?".into(),
        });
        s.update(Message::Key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Char('n'),
            ratatui::crossterm::event::KeyModifiers::NONE,
        )));
        // What `tui_run.rs` sends once the answer is read.
        s.update(Message::AgentToken("· apply the edit? declined\n".into()));
        // Tall enough for the edits pane to have a row inside its borders;
        // at 80x24 it is borders only, which is its own finding.
        let out = drawn(&s, 100, 30);
        assert!(!out.contains("- a.py"), "got:\n{out}");
        assert!(out.contains("~ a.py"), "got:\n{out}");
        assert!(out.contains("· apply the edit? declined"), "got:\n{out}");
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

    /// The rendered screen as rows, so a test can ask what is below what.
    fn drawn_rows(state: &AppState, w: u16, h: u16) -> Vec<String> {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| render(state, f)).unwrap();
        let buf = t.backend().buffer();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn at_file_candidates_are_drawn_under_the_input_line() {
        let paths = ["src/models.py", "app/models.py", "README.md"];
        let mut s = populated().with_paths(paths.iter().map(|p| (*p).to_string()).collect());
        s.input = "fix @models".into();
        let rows = drawn_rows(&s, 80, 24);
        let row_of = |needle: &str| {
            rows.iter()
                .position(|r| r.contains(needle))
                .unwrap_or_else(|| {
                    panic!(
                        "{needle} not on screen:
{}",
                        rows.join(
                            "
"
                        )
                    )
                })
        };
        let typed = row_of("fix @models");
        let first = row_of("app/models.py");
        let second = row_of("src/models.py");
        assert!(
            typed < first && first < second,
            "{}",
            rows.join(
                "
"
            )
        );
        assert!(rows[first].contains("tab"), "the one Tab inserts is marked");
        assert!(
            !rows
                .join(
                    "
"
                )
                .contains("README"),
            "a non-match is not drawn"
        );
        assert!(
            rows.iter().any(|r| r.contains("budget")),
            "the other regions still fit at the minimum size"
        );
    }

    #[test]
    fn at_file_candidates_are_not_drawn_while_a_run_holds_the_input() {
        let mut s = populated().with_paths(vec!["src/models.py".into()]);
        s.input = "fix @models".into();
        s.running = true;
        let out = drawn(&s, 80, 24);
        assert!(
            !out.contains("src/models.py"),
            "got:
{out}"
        );
    }

    #[test]
    fn while_running_the_task_line_shows_the_steer_being_typed() {
        // Keys still reach `input` during a run and Enter queues a steer,
        // so hiding the text would have the person type blind.
        let mut s = populated();
        s.running = true;
        s.input = "use the other module".into();
        let out = drawn(&s, 80, 24);
        assert!(out.contains("running"), "got:\n{out}");
        assert!(
            out.contains("queues a steer"),
            "and says what enter does now:\n{out}"
        );
        assert!(
            out.contains("use the other module█"),
            "the typed steer is drawn:\n{out}"
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
        // `render` never hands the pane less than MIN_WIDTH allows, so the
        // narrow guard is reached only by calling the pane directly. Inner
        // width 23 is under the 24 the graph needs: border only, no panic.
        let s = acted(&[("edit", "a.py", true)]);
        let (w, h) = (25u16, 12u16);
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| render_actions(&s, f.area(), f)).unwrap();
        let buf = t.backend().buffer();
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                assert_eq!(
                    buf[(x, y)].symbol(),
                    " ",
                    "a pane too narrow for the graph drew into it at ({x},{y})"
                );
            }
        }
        assert!(
            (0..w).any(|x| buf[(x, 0)].symbol() == "a"),
            "the border and title are still drawn"
        );
    }

    // ---- #655: whole words at the default 80 columns ----

    /// The rows of the pane titled `title`, between its borders, at 80x24.
    fn pane_rows(s: &AppState, title: &str) -> Vec<String> {
        let rows = drawn_rows(s, 80, 24);
        // The title sits on the top border, just after its corner; the text
        // of a pane can say the same word, so the corner is what is matched.
        let title: Vec<char> = format!("┌ {title} ").chars().collect();
        let (top, left) = rows
            .iter()
            .enumerate()
            .find_map(|(y, r)| {
                let row: Vec<char> = r.chars().collect();
                row.windows(title.len())
                    .position(|w| w == title.as_slice())
                    .map(|x| (y, x))
            })
            .unwrap_or_else(|| {
                panic!(
                    "no `{}` pane:\n{}",
                    title[2..].iter().collect::<String>(),
                    rows.join("\n")
                )
            });
        let row: Vec<char> = rows[top].chars().collect();
        let right = left + row[left..].iter().position(|c| *c == '┐').unwrap();
        rows[top + 1..]
            .iter()
            .take_while(|r| r.chars().nth(left) != Some('└'))
            .map(|r| r.chars().take(right).skip(left + 1).collect::<String>())
            .collect()
    }

    #[test]
    fn fits_80_columns_actions_and_radius_labels_are_whole() {
        // #655: at 80 columns the right column was 32 wide and read "0 ac",
        // "reac" and "a floor on reach, not a boun" where the figures are.
        let s = acted(&[
            ("read_file", "app/calc.py", false),
            ("edit", "app/calc.py", true),
        ]);
        let actions = pane_rows(&s, "actions").join("\n");
        assert!(actions.contains("2 actions"), "{actions}");
        let radius = pane_rows(&s, "radius").join("\n");
        assert!(radius.contains("◆ reach "), "{radius}");
        assert!(
            radius.contains("a floor on reach, not a boundary"),
            "{radius}"
        );
        // And the same with nothing called: the row the audit captured.
        let actions = pane_rows(&populated(), "actions").join("\n");
        assert!(actions.contains("0 actions"), "{actions}");
    }

    #[test]
    fn fits_80_columns_budget_line_ends_with_a_whole_segment() {
        // #655: one string, centred and clipped by the gauge, ended "· queu"
        // once a turn had reported a rate and a wait. The rate goes first.
        let mut s = populated();
        s.update(Message::BudgetUpdated {
            iteration: 4,
            prompt_bytes: Some(16800),
            elapsed_secs: 36,
            completion_tokens: Some(85),
            model_ms: Some(9_600),
            queue_wait_ms: Some(2_100),
        });
        let budget = pane_rows(&s, "budget").join("\n");
        assert!(budget.contains("iteration 4/40"), "{budget}");
        assert!(budget.contains("16800 B prompt"), "{budget}");
        assert!(budget.contains("0:36 elapsed"), "{budget}");
        assert!(budget.contains("queued 2.1 s"), "{budget}");
        assert!(!budget.contains("tok"), "the rate gave way first: {budget}");
        assert!(!budget.contains("queu "), "{budget}");

        // Wider, the same state draws the rate: nothing was dropped for good.
        let wide = drawn(&s, 140, 24);
        assert!(wide.contains("85 tok, 11.3 tok/s"), "{wide}");
        assert!(wide.contains("queued 2.1 s"), "{wide}");
    }

    #[test]
    fn fits_80_columns_help_has_no_row_ending_mid_word() {
        // #655: a row cut inside a word reads as a different word. Every
        // token on screen must be a token of the help text, so "stopp" or
        // "boun" fails here where a substring check would not.
        let mut s = AppState::default();
        s.input = "/help".into();
        pressed(&mut s, ratatui::crossterm::event::KeyCode::Enter);
        let help = s.transcript.join("\n");
        let words: std::collections::HashSet<&str> = help.split_whitespace().collect();
        let rows = pane_rows(&s, "transcript");
        assert!(
            rows.iter().any(|r| r.contains("scroll the transcript")),
            "{}",
            rows.join("\n")
        );
        for row in &rows {
            for token in row.split_whitespace() {
                assert!(
                    words.contains(token),
                    "`{token}` is not a word of the help text, in row `{row}`:\n{}",
                    rows.join("\n")
                );
            }
        }
    }

    // ---- #661: a write is said in words, not only in yellow ----

    #[test]
    fn wrote_in_text_on_the_edit_node_at_80_columns() {
        // #661: under NO_COLOR or a monochrome terminal the node that changed
        // the tree looked like the others. Three calls, one write, as the
        // desktop's graph says it; and whole at the default width.
        let s = acted(&[
            ("read_file", "app/calc.py", false),
            ("edit", "app/calc.py", false),
            ("edit", "app/calc.py", false),
            ("edit", "app/calc.py", true),
        ]);
        let rows = pane_rows(&s, "actions");
        let edit = rows
            .iter()
            .find(|r| r.contains("edit"))
            .unwrap_or_else(|| panic!("{}", rows.join("\n")));
        assert!(edit.contains("3 · 1 wrote"), "{edit}");
        assert!(edit.contains("◆ 4 actions"), "{}", rows.join("\n"));
        // A node that only looked says its count alone: "wrote" on it would
        // claim a write that did not happen.
        let read = rows.iter().find(|r| r.contains("read_file")).unwrap();
        assert!(!read.contains("wrote"), "{read}");
        // The edge column is still one column.
        let edges: std::collections::HashSet<usize> = rows
            .iter()
            .filter_map(|r| r.chars().position(|c| c == '●' || c == '○'))
            .collect();
        assert_eq!(edges.len(), 1, "{}", rows.join("\n"));
    }

    // ---- #519: the status bar's tier, one `tier` word, one vocabulary ----

    #[test]
    fn tier_status_bar_writes_the_word_once_before_the_label() {
        // #519: a hosted label began `tier 1, …` and the bar wrote `tier `
        // before it, so it read `tier tier 1, …`. A label begins with its
        // tier number; the word is the bar's.
        let s = AppState::with_model("m".into(), "1, native tool calls".into());
        let out = drawn(&s, 120, 30);
        assert!(out.contains("tier 1, native tool calls"), "{out}");
        assert!(!out.contains("tier tier"), "{out}");
    }

    #[test]
    fn tier_status_bar_with_no_label_says_unknown() {
        // A tier nothing has reported is `unknown`, the word `get_state`'s
        // vocabulary uses for it, not a dash a client cannot match.
        let out = drawn(&AppState::default(), 120, 30);
        assert!(out.contains("tier unknown"), "{out}");
    }
}
