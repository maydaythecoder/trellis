//! The activity graph: what this session has called, as nodes and edges.
//!
//! The reference for this is `ui_examples/preview.webp` — a network drawn
//! with dots and converging lines, with the stats stacked beside it. Asked
//! for on the side rather than in the centre, so it lives in the right
//! column where the conversation keeps the width it needs.
//!
//! Layered like the reference: tool nodes on the left, edges converging on
//! a focus node on the right. A dot is a tool the session has used; a ring
//! is one it has been offered and has not. That distinction is the point —
//! a list of counts says what happened, and a graph with unused nodes in it
//! also says what was available and did not.

use crate::tui::app::AppState;
use crate::tui::theme::Theme;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

/// Tools the interface knows about, so the graph can show one that has
/// never been called rather than only what has.
///
/// A fixed list rather than a live capability query: this crate does not
/// negotiate tools with the model yet, and inventing a negotiation to
/// populate a pane would be the pane driving the design.
pub const KNOWN_TOOLS: &[&str] = &["read_file", "run_tests", "edit", "blast_radius"];

/// One row of the graph: a tool, how often it was called, and the glyph.
struct Node {
    name: String,
    count: u32,
}

/// Renders the graph into `area`.
pub fn render(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let block = theme.pane("graph", false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width < 12 {
        return;
    }
    f.render_widget(
        Paragraph::new(Text::from(lines(state, theme, inner))),
        inner,
    );
}

/// The graph as styled lines.
///
/// Separated from the frame so the shape is testable: the thing worth
/// checking is that the edges connect and that an unused tool is
/// distinguishable from a used one, and neither needs a terminal.
pub fn lines<'a>(state: &AppState, theme: &Theme, inner: Rect) -> Vec<Line<'a>> {
    let mut nodes: Vec<Node> = KNOWN_TOOLS
        .iter()
        .map(|t| Node {
            name: (*t).to_string(),
            count: state
                .tool_calls
                .iter()
                .find(|(n, _)| n == t)
                .map(|(_, c)| *c)
                .unwrap_or(0),
        })
        .collect();
    // Anything the model called that is not in the known list still belongs
    // on the graph — a node missing because this crate had not heard of it
    // would make the picture a lie about what ran.
    for (name, count) in &state.tool_calls {
        if !KNOWN_TOOLS.contains(&name.as_str()) {
            nodes.push(Node {
                name: name.clone(),
                count: *count,
            });
        }
    }

    let width = inner.width as usize;
    // The widest name, so the edge column lines up and the graph reads as
    // one figure rather than as ragged rows.
    let label_w = nodes
        .iter()
        .map(|n| n.name.len())
        .max()
        .unwrap_or(0)
        .min(width.saturating_sub(10));

    let focus = format!("turn {}", state.turns);
    let mid = nodes.len() / 2;

    let dim = if theme.colored() {
        Style::default().fg(theme.dim)
    } else {
        Style::default().add_modifier(Modifier::DIM)
    };
    let edge = if theme.colored() {
        Style::default().fg(theme.structure)
    } else {
        Style::default()
    };

    let mut out: Vec<Line<'a>> = Vec::new();
    for (i, node) in nodes.iter().enumerate() {
        // Converging edges: the first row opens the bracket, the last
        // closes it, the middle row carries the junction into the focus.
        let connector = match (nodes.len(), i) {
            (1, _) => "──",
            (_, 0) => "─┐",
            (_, i) if i == nodes.len() - 1 => "─┘",
            (_, i) if i == mid => "─┤",
            _ => "─┤",
        };
        // A filled dot for a tool that ran, a ring for one that did not.
        let (glyph, glyph_style) = if node.count > 0 {
            ("●", theme.outcome(Some(true)))
        } else {
            ("○", dim)
        };
        let name_style = if node.count > 0 {
            Style::default().fg(theme.text)
        } else {
            dim
        };

        let mut spans = vec![
            Span::styled(" ", Style::default()),
            Span::styled(
                format!("{:<label_w$}", truncate(&node.name, label_w)),
                name_style,
            ),
            Span::styled(" ", Style::default()),
            Span::styled(glyph.to_string(), glyph_style),
            Span::styled(connector.to_string(), edge),
        ];
        if i == mid {
            spans.push(Span::styled("◆ ", theme.outcome(None)));
            spans.push(Span::styled(
                focus.clone(),
                Style::default().fg(theme.active),
            ));
        } else if node.count > 0 {
            // The count sits on its own row, right where the edge leaves,
            // so the number is attached to the node it belongs to.
            spans.push(Span::styled(format!(" {}", node.count), edge));
        }
        out.push(Line::from(spans));
    }

    if out.is_empty() {
        out.push(Line::from(Span::styled(" no tools", dim)));
    }
    out
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).chain(['…']).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::message::Message;
    use crate::tui::theme::ColorDepth;

    fn plain(state: &AppState) -> Vec<String> {
        let theme = Theme::new(ColorDepth::Ansi16);
        lines(state, &theme, Rect::new(0, 0, 32, 10))
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
    fn every_known_tool_appears_even_before_it_is_called() {
        // A graph that only showed what ran would say nothing about what
        // was available and was not chosen, which is half of what makes it
        // a graph rather than a list of counts.
        let out = plain(&AppState::default()).join("\n");
        for t in KNOWN_TOOLS {
            assert!(out.contains(t), "`{t}` missing:\n{out}");
        }
        assert!(out.contains('○'), "unused tools are rings:\n{out}");
        assert!(!out.contains('●'), "and nothing has run yet:\n{out}");
    }

    #[test]
    fn a_called_tool_becomes_a_filled_node_with_its_count() {
        let mut s = AppState::default();
        for _ in 0..3 {
            s.update(Message::ToolCalled {
                name: "read_file".into(),
            });
        }
        let out = plain(&s).join("\n");
        assert!(out.contains('●'), "a used tool is filled:\n{out}");
        assert!(out.contains('○'), "and unused ones stay rings:\n{out}");
        assert!(out.contains(" 3"), "with its count:\n{out}");
    }

    #[test]
    fn a_tool_this_crate_has_never_heard_of_still_reaches_the_graph() {
        // A node missing because the constant was out of date would make
        // the picture a lie about what actually ran.
        let mut s = AppState::default();
        s.update(Message::ToolCalled {
            name: "some_new_tool".into(),
        });
        let out = plain(&s).join("\n");
        assert!(out.contains("some_new_tool"), "{out}");
    }

    #[test]
    fn the_edges_converge_on_exactly_one_focus_node() {
        let s = AppState::default();
        let out = plain(&s);
        let focus_rows = out.iter().filter(|l| l.contains('◆')).count();
        assert_eq!(focus_rows, 1, "one focus, not one per row:\n{out:?}");
        // The bracket opens at the top and closes at the bottom.
        assert!(out.first().unwrap().contains('┐'), "{out:?}");
        assert!(out.last().unwrap().contains('┘'), "{out:?}");
    }

    #[test]
    fn a_long_tool_name_is_truncated_rather_than_wrapping_the_row() {
        // A wrapped label breaks the edge column, and the graph stops
        // reading as one figure.
        let mut s = AppState::default();
        s.update(Message::ToolCalled {
            name: "an_extremely_long_tool_name_that_will_not_fit".into(),
        });
        for l in plain(&s) {
            assert!(l.chars().count() <= 40, "row too wide: {l:?}");
        }
    }

    #[test]
    fn a_narrow_pane_does_not_panic() {
        let s = AppState::default();
        let theme = Theme::new(ColorDepth::None);
        for w in [0u16, 1, 5, 12, 13] {
            let _ = lines(&s, &theme, Rect::new(0, 0, w, 6));
        }
    }
}
