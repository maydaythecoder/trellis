//! The right-hand column: session facts, token rate, context, tool counts.
//!
//! Every number that moves over time gets a `Sparkline` or a `Gauge` rather
//! than bare text. A figure printed as a number tells you where it is; a
//! figure drawn as a shape tells you where it is *going*, and on a streaming
//! interface that is the question actually being asked.

use crate::tui::app::AppState;
use crate::tui::theme::Theme;
use ratatui::prelude::*;
use ratatui::widgets::{Gauge, List, ListItem, Paragraph, Sparkline};

/// Model, tier, context and turn count as aligned key/value lines.
pub fn render_session(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let block = theme.pane("session", false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let lines = vec![
        theme.kv("model", &state.model),
        theme.kv("tier", &state.tier),
        theme.kv(
            "ctx",
            &format!(
                "{} / {}",
                compact(state.context_used),
                compact(state.context_window)
            ),
        ),
        theme.kv("turns", &state.turns.to_string()),
    ];
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

/// Tokens per sample period, as a sparkline over a bounded ring buffer.
pub fn render_tokens(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let block = theme.pane("tokens", false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // Only the samples that fit. Handing a Sparkline more points than it has
    // columns silently drops the oldest, which would make the window size
    // depend on the terminal width rather than on the buffer.
    let take = inner.width as usize;
    let data: Vec<u64> = state
        .token_rate
        .iter()
        .rev()
        .take(take)
        .rev()
        .copied()
        .collect();

    let style = if state.streaming {
        theme.outcome(None)
    } else {
        Style::default().fg(theme.structure)
    };
    f.render_widget(Sparkline::default().data(&data).style(style), inner);
}

/// The context window, as a gauge. The ratio is clamped by `context_ratio`.
pub fn render_context(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let block = theme.pane("context", false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let ratio = state.context_ratio();
    // The colour is the warning. A window at 90% is the thing that makes a
    // long agent conversation start dropping its own earlier turns, and a
    // uniformly coloured bar gives no notice of it.
    let style = if ratio > 0.9 {
        Style::default().fg(theme.failure)
    } else if ratio > 0.7 {
        Style::default().fg(theme.progress)
    } else {
        Style::default().fg(theme.success)
    };
    f.render_widget(
        Gauge::default()
            .ratio(ratio)
            .label(format!("{:.0}%", ratio * 100.0))
            .gauge_style(style),
        inner,
    );
}

/// Tool call counts, one row each.
pub fn render_tools(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let block = theme.pane("tools", false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    if state.tool_calls.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  none yet",
                Style::default().fg(theme.dim),
            ))),
            inner,
        );
        return;
    }

    let width = inner.width as usize;
    let items: Vec<ListItem> = state
        .tool_calls
        .iter()
        .map(|(name, count)| {
            let count = count.to_string();
            // Right-align the count against the pane edge: a ragged column
            // of numbers cannot be compared at a glance, which is the only
            // reason to show counts side by side.
            let pad = width.saturating_sub(4 + name.len() + count.len());
            ListItem::new(Line::from(vec![
                Span::styled("  ● ", theme.outcome(Some(true))),
                Span::styled(name.clone(), Style::default().fg(theme.text)),
                Span::raw(" ".repeat(pad)),
                Span::styled(count, Style::default().fg(theme.structure)),
            ]))
        })
        .collect();
    f.render_widget(List::new(items), inner);
}

/// `4200` as `4.2K`. Long conversations produce five- and six-digit figures,
/// and a column of those is unreadable at the width this pane gets.
fn compact(n: u32) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}K", f64::from(n) / 1000.0),
        _ => format!("{:.1}M", f64::from(n) / 1_000_000.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_abbreviated_once_they_stop_fitting() {
        assert_eq!(compact(0), "0");
        assert_eq!(compact(999), "999");
        assert_eq!(compact(4200), "4.2K");
        assert_eq!(compact(32_768), "32.8K");
        assert_eq!(compact(2_500_000), "2.5M");
    }

    #[test]
    fn a_full_context_window_does_not_produce_a_ratio_over_one() {
        // `Gauge::ratio` panics outside 0..=1, which would take the whole
        // interface down at exactly the moment the window filled.
        let mut s = AppState::default();
        s.context_window = 10;
        s.context_used = 99;
        assert_eq!(s.context_ratio(), 1.0);
        s.context_window = 0;
        assert_eq!(s.context_ratio(), 0.0, "and a zero window is not a divide");
    }
}
