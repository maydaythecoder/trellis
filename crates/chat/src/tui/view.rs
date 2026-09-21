//! `render` — the whole frame, computed from `frame.area()` every time.
//!
//! **No `Rect` is cached.** A resize invalidates every one of them, and a
//! stale `Rect` does not fail loudly: it draws a pane at the old size, over
//! whatever is now there. Recomputing costs a few divisions per frame and
//! the frame only draws when something changed.

use crate::tui::app::{AppState, Tab};
use crate::tui::theme::Theme;
use crate::tui::widgets::{chat, input, stats};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Paragraph, Tabs, Wrap};
use tui_scrollview::ScrollViewState;

/// Below this the layout is not attempted. Seven bordered panes in eighty
/// columns is already tight; in forty it is unreadable rows of borders.
pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;

/// View-local state that must survive between frames.
///
/// The scroll offset belongs here rather than in `AppState` because it is a
/// property of the rendering, not of the conversation — and rebuilding it
/// each frame would snap the view back to the top on every token.
#[derive(Default)]
pub struct ViewState {
    pub conversation_scroll: ScrollViewState,
}

pub fn render(f: &mut Frame, state: &AppState, theme: &Theme, view: &mut ViewState) {
    let area = f.area();
    f.render_widget(Block::default().style(theme.background()), area);

    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        render_too_small(f, area, theme);
        return;
    }

    // The log gives up rows first on a short terminal. At exactly 80x24 a
    // fixed six leaves the tools pane one row — its top border and nothing
    // else — and a pane that renders as a line is worse than a shorter log,
    // because it reads as a layout fault rather than as a trade.
    let log_height = if area.height < 30 { 4 } else { 6 };
    let rows = Layout::vertical([
        Constraint::Length(1),          // tabs
        Constraint::Fill(1),            // the two columns
        Constraint::Length(log_height), // log
        Constraint::Length(3),          // input
        Constraint::Length(1),          // key hints
    ])
    .split(area);

    render_tabs(f, rows[0], state, theme);
    render_body(f, rows[1], state, theme, view);
    render_log(f, rows[2], state, theme);
    input::render(f, rows[3], state, theme);
    render_hints(f, rows[4], theme);

    // Floated last, over everything, in the order they take priority.
    input::render_popup(f, rows[3], state, theme);
    input::render_confirm(f, state, theme);
}

fn render_too_small(f: &mut Frame, area: Rect, theme: &Theme) {
    let text = Text::from(vec![
        Line::from(Span::styled(
            "terminal too small",
            Style::default()
                .fg(theme.failure)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(
                "{}x{} — need at least {MIN_WIDTH}x{MIN_HEIGHT}",
                area.width, area.height
            ),
            Style::default().fg(theme.dim),
        )),
    ]);
    let box_area = input::centred(area, 40, 2);
    f.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        box_area,
    );
}

fn render_tabs(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let titles: Vec<Line> = Tab::ALL
        .iter()
        .map(|t| Line::from(format!("[ {} ]", t.label())))
        .collect();
    let tabs = Tabs::new(titles)
        .select(state.tab.index())
        .style(Style::default().fg(theme.dim))
        .highlight_style(
            Style::default()
                .fg(theme.active)
                .add_modifier(Modifier::BOLD),
        )
        .divider(" ");
    f.render_widget(tabs, area);
}

fn render_body(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme, view: &mut ViewState) {
    // The right column is a fixed width rather than a percentage: the
    // key/value lines in it have a known length, and a percentage makes them
    // wrap on a narrow terminal and float in space on a wide one.
    let right = 34u16.min(area.width / 2);
    let cols = Layout::horizontal([Constraint::Fill(1), Constraint::Length(right)]).split(area);

    match state.tab {
        Tab::Chat => chat::render(f, cols[0], state, theme, &mut view.conversation_scroll),
        Tab::Tools => render_tools_tab(f, cols[0], state, theme),
        Tab::Logs => render_logs_tab(f, cols[0], state, theme),
        Tab::State => render_state_tab(f, cols[0], state, theme),
    }

    let side = Layout::vertical([
        Constraint::Length(6), // session
        Constraint::Length(3), // token sparkline
        Constraint::Length(3), // context gauge
        Constraint::Fill(1),   // tools
    ])
    .split(cols[1]);

    stats::render_session(f, side[0], state, theme);
    stats::render_tokens(f, side[1], state, theme);
    stats::render_context(f, side[2], state, theme);
    stats::render_tools(f, side[3], state, theme);
}

fn render_tools_tab(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let block = theme.pane("tool calls", false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let mut lines: Vec<Line> = state
        .tool_calls
        .iter()
        .map(|(n, c)| theme.kv(n, &c.to_string()))
        .collect();
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no tool has been called this session",
            Style::default().fg(theme.dim),
        )));
    }
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn render_logs_tab(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let block = theme.pane("log", false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let lines: Vec<Line> = state
        .log
        .iter()
        .rev()
        .take(inner.height as usize)
        .rev()
        .map(|l| {
            Line::from(Span::styled(
                format!("  {l}"),
                Style::default().fg(theme.text),
            ))
        })
        .collect();
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn render_state_tab(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let block = theme.pane("state", false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let lines = vec![
        theme.kv("model", &state.model),
        theme.kv("tier", &state.tier),
        theme.kv("turns", &state.turns.to_string()),
        theme.kv("stream", if state.streaming { "open" } else { "idle" }),
        theme.kv("cmds", &state.registry.all().len().to_string()),
        theme.kv("error", state.last_error.as_deref().unwrap_or("none")),
    ];
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

/// The log pane along the bottom.
///
/// Fed from `AppState::log` rather than from `tui_logger`'s own widget:
/// `TuiLoggerWidget` reads a global the tests cannot isolate, and a pane
/// whose contents depend on what else ran in the process is a pane whose
/// snapshot changes for reasons unrelated to the code under test. The
/// `tui-logger` subscriber is still installed in `chat.rs`, so `tracing`
/// output reaches this buffer.
fn render_log(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let block = theme.pane("log", false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let lines: Vec<Line> = state
        .log
        .iter()
        .rev()
        .take(inner.height as usize)
        .rev()
        .map(|l| {
            Line::from(Span::styled(
                format!(" {l}"),
                Style::default().fg(theme.text),
            ))
        })
        .collect();
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn render_hints(f: &mut Frame, area: Rect, theme: &Theme) {
    let hint = " q quit · / commands · ↑↓ scroll · tab switch · ←→ panes · ^c interrupt ";
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().fg(theme.dim),
        ))),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::message::Message;
    use crate::tui::theme::ColorDepth;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// A populated state, so a snapshot shows the layout with content in it
    /// rather than an empty frame that would pass whatever the panes did.
    fn populated() -> AppState {
        let mut s = AppState::default();
        s.model = "qwen3:8b".into();
        s.tier = "grammar".into();
        s.context_window = 32_768;
        s.update(Message::ContextUsed { tokens: 13_700 });
        s.conversation.push_back(crate::tui::app::Line {
            role: crate::tui::app::Role::User,
            text: "how does resolve_symbol work".into(),
        });
        s.update(Message::StreamToken(
            "It walks the scope chain outward from the call site.".into(),
        ));
        s.turns = 7;
        for name in ["read_file", "read_file", "run_tests"] {
            s.update(Message::ToolCalled { name: name.into() });
        }
        for n in [1u64, 3, 8, 14, 9, 4, 2, 6, 11] {
            s.token_rate.push_back(n);
        }
        s.log.push_back("12:45:02 INFO  stream opened".into());
        s.log.push_back("12:45:04 INFO  tool read_file".into());
        s
    }

    fn draw(state: &AppState, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let theme = Theme::new(ColorDepth::Ansi16);
        let mut view = ViewState::default();
        term.draw(|f| render(f, state, &theme, &mut view)).unwrap();
        // The text only. Styles are asserted in the widget tests; a
        // full-buffer snapshot would churn on every colour tweak.
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn snapshot_at_80x24() {
        insta::assert_snapshot!(draw(&populated(), 80, 24));
    }

    #[test]
    fn snapshot_at_120x40() {
        insta::assert_snapshot!(draw(&populated(), 120, 40));
    }

    #[test]
    fn snapshot_at_200x60() {
        insta::assert_snapshot!(draw(&populated(), 200, 60));
    }

    #[test]
    fn below_the_minimum_only_the_too_small_message_is_drawn() {
        let out = draw(&populated(), 60, 20);
        assert!(out.contains("terminal too small"), "{out}");
        assert!(
            !out.contains("C O N V E R S A T I O N"),
            "the layout must not be attempted at all:\n{out}"
        );
        assert!(
            out.contains("60x20"),
            "and it must say what is wrong:\n{out}"
        );
    }

    #[test]
    fn growing_back_past_the_minimum_recovers_the_layout() {
        let s = populated();
        assert!(draw(&s, 79, 23).contains("terminal too small"));
        let out = draw(&s, 80, 24);
        assert!(out.contains("C O N V E R S A T I O N"), "{out}");
    }

    #[test]
    fn every_pane_is_present_at_the_minimum_size() {
        // Eighty by twenty-four is the tightest supported layout, so it is
        // the one where a pane silently loses its row.
        let out = draw(&populated(), 80, 24);
        for title in [
            "C O N V E R S A T I O N",
            "S E S S I O N",
            "T O K E N S",
            "C O N T E X T",
            "T O O L S",
            "L O G",
            "I N P U T",
        ] {
            assert!(out.contains(title), "`{title}` is missing:\n{out}");
        }
        assert!(out.contains("[ chat ]"), "the tab bar:\n{out}");
        assert!(out.contains("quit"), "the key hints:\n{out}");
    }

    #[test]
    fn the_tools_pane_has_room_for_content_at_the_minimum_size() {
        // A pane rendered as a single line is its top border and nothing
        // else, which reads as a layout fault rather than as a trade. The
        // log gives up rows first, and this is the size where that matters.
        let mut s = populated();
        s.update(Message::ToolCalled {
            name: "read_file".into(),
        });
        let out = draw(&s, 80, 24);
        let after_tools: Vec<&str> = out
            .lines()
            .skip_while(|l| !l.contains("T O O L S"))
            .skip(1)
            .collect();
        assert!(
            after_tools.iter().any(|l| l.contains("read_file")),
            "the tools pane is squeezed to its border:
{out}"
        );
    }

    #[test]
    fn a_zero_sized_frame_does_not_panic() {
        // Terminals do report 0x0 briefly during some resizes, and a panic
        // there unwinds past the restore and leaves the terminal broken.
        let _ = draw(&populated(), 0, 0);
        let _ = draw(&populated(), 1, 1);
    }

    #[test]
    fn the_confirmation_modal_covers_the_interface_when_it_is_open() {
        let mut s = populated();
        s.confirm = Some(crate::tui::app::Confirm {
            command: crate::commands::Command::Clear,
            prompt: "drop the whole conversation?  [y/n]".into(),
        });
        let out = draw(&s, 80, 24);
        assert!(out.contains("C O N F I R M"), "{out}");
        assert!(out.contains("drop the whole conversation"), "{out}");
        assert!(
            out.contains("n / esc"),
            "both answers must be offered:\n{out}"
        );
    }

    #[test]
    fn the_completion_popup_shows_over_the_input_line() {
        let mut s = populated();
        s.update(Message::Key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Char('/'),
            ratatui::crossterm::event::KeyModifiers::NONE,
        )));
        let out = draw(&s, 80, 24);
        assert!(out.contains("C O M M A N D S"), "{out}");
        assert!(out.contains("/add"), "{out}");
    }
}
