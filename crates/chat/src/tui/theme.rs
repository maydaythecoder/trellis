//! Colours, borders and title styling, in one place.
//!
//! Held here rather than scattered through the views for a reason that is
//! not tidiness: a palette spread across five widget modules drifts, and the
//! drift shows up as one pane that does not match the others under a
//! terminal nobody tested.
//!
//! **Colour support is detected, not assumed.** `NO_COLOR` wins outright
//! (the convention is that *any* value, including the empty string, disables
//! colour). Below that, truecolor degrades to 256 and then to the sixteen
//! ANSI colours. tmux and older terminals will disappoint anyone who assumes
//! 24-bit, and the failure mode is not a fallback — it is unreadable text.

use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders};

/// How much colour the terminal can actually render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    /// `NO_COLOR` is set, or the terminal claims none.
    None,
    /// The sixteen ANSI colours. The safe floor.
    Ansi16,
    /// An indexed 256-colour palette.
    Indexed256,
    /// 24-bit RGB.
    TrueColor,
}

impl ColorDepth {
    /// Reads the environment. See the module note on `NO_COLOR`.
    pub fn detect() -> Self {
        Self::from_env(
            std::env::var_os("NO_COLOR").is_some(),
            std::env::var("COLORTERM").ok().as_deref(),
            std::env::var("TERM").ok().as_deref(),
        )
    }

    /// The decision, separated from the environment so it can be tested.
    pub fn from_env(no_color: bool, colorterm: Option<&str>, term: Option<&str>) -> Self {
        if no_color {
            return ColorDepth::None;
        }
        match term {
            // "dumb" is the one TERM that means *no* capabilities at all.
            Some("dumb") | None => return ColorDepth::None,
            _ => {}
        }
        if matches!(colorterm, Some("truecolor") | Some("24bit")) {
            return ColorDepth::TrueColor;
        }
        if term.is_some_and(|t| t.contains("256color")) {
            return ColorDepth::Indexed256;
        }
        ColorDepth::Ansi16
    }
}

/// The palette, resolved once for the detected depth.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub depth: ColorDepth,
    /// Structure: borders, rules, anything that frames rather than says.
    pub structure: Color,
    /// A finished thing that worked.
    pub success: Color,
    /// A thing still happening.
    pub progress: Color,
    /// A finished thing that did not work.
    pub failure: Color,
    /// The element the keyboard is pointed at.
    pub active: Color,
    /// Body text.
    pub text: Color,
    /// Text that is present but not the point.
    pub dim: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self::new(ColorDepth::detect())
    }
}

impl Theme {
    pub fn new(depth: ColorDepth) -> Self {
        // Every branch names the same seven roles. Degrading is choosing a
        // cheaper colour for a role, never dropping the role — a pane that
        // loses its "active" colour loses the only cue for where typing goes.
        match depth {
            ColorDepth::None => Self {
                depth,
                structure: Color::Reset,
                success: Color::Reset,
                progress: Color::Reset,
                failure: Color::Reset,
                active: Color::Reset,
                text: Color::Reset,
                dim: Color::Reset,
            },
            ColorDepth::Ansi16 => Self {
                depth,
                structure: Color::Cyan,
                success: Color::Green,
                progress: Color::Yellow,
                failure: Color::Red,
                active: Color::Magenta,
                text: Color::White,
                dim: Color::DarkGray,
            },
            ColorDepth::Indexed256 => Self {
                depth,
                structure: Color::Indexed(37),
                success: Color::Indexed(71),
                progress: Color::Indexed(179),
                failure: Color::Indexed(167),
                active: Color::Indexed(176),
                text: Color::Indexed(252),
                dim: Color::Indexed(243),
            },
            ColorDepth::TrueColor => Self {
                depth,
                structure: Color::Rgb(0x2d, 0x9c, 0xa8),
                success: Color::Rgb(0x6d, 0xc2, 0x7a),
                progress: Color::Rgb(0xd7, 0xa6, 0x5c),
                failure: Color::Rgb(0xd1, 0x66, 0x6a),
                active: Color::Rgb(0xc5, 0x8a, 0xd6),
                text: Color::Rgb(0xd8, 0xdc, 0xe0),
                dim: Color::Rgb(0x6b, 0x72, 0x7a),
            },
        }
    }

    /// Whether colour is being emitted at all. `NO_COLOR` sets this false.
    pub fn colored(&self) -> bool {
        self.depth != ColorDepth::None
    }

    /// Black, so the panes sit on a known ground rather than on whatever the
    /// terminal's default happens to be. Under `NO_COLOR` the terminal keeps
    /// its own.
    pub fn background(&self) -> Style {
        if self.colored() {
            Style::default().bg(Color::Black).fg(self.text)
        } else {
            Style::default()
        }
    }

    /// `" S E S S I O N "` — spaced uppercase, which is the whole visual
    /// signature of this layout. Built rather than written out at each call
    /// site so no pane can spell its own title a different way.
    pub fn title(&self, name: &str) -> Span<'static> {
        let spaced: String = name
            .to_uppercase()
            .chars()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        let style = if self.colored() {
            Style::default()
                .fg(self.structure)
                .add_modifier(Modifier::DIM)
        } else {
            Style::default()
        };
        Span::styled(format!(" {spaced} "), style)
    }

    /// A bordered pane. `focused` brightens the border, and under `NO_COLOR`
    /// it thickens it instead — focus has to be visible at a glance whether
    /// or not colour is available.
    pub fn pane(&self, name: &str, focused: bool) -> Block<'static> {
        let (border_style, border_type) = match (self.colored(), focused) {
            (true, true) => (
                Style::default()
                    .fg(self.active)
                    .add_modifier(Modifier::BOLD),
                BorderType::Plain,
            ),
            (true, false) => (
                Style::default()
                    .fg(self.structure)
                    .add_modifier(Modifier::DIM),
                BorderType::Plain,
            ),
            // No colour: the only channel left is the border glyph itself.
            (false, true) => (Style::default(), BorderType::Double),
            (false, false) => (Style::default(), BorderType::Plain),
        };
        Block::default()
            .borders(Borders::ALL)
            .border_type(border_type)
            .border_style(border_style)
            .title(self.title(name))
    }

    /// A key/value line, aligned by padding the key.
    pub fn kv(&self, key: &str, value: &str) -> Line<'static> {
        let (k, v) = if self.colored() {
            (
                Style::default().fg(self.dim),
                Style::default().fg(self.text),
            )
        } else {
            (Style::default(), Style::default())
        };
        Line::from(vec![
            Span::styled(format!("  {key:<7}"), k),
            Span::styled(value.to_string(), v),
        ])
    }

    /// The colour for a state that is running, finished well, or failed.
    pub fn outcome(&self, ok: Option<bool>) -> Style {
        if !self.colored() {
            return Style::default();
        }
        Style::default().fg(match ok {
            None => self.progress,
            Some(true) => self.success,
            Some(false) => self.failure,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_color_beats_every_other_signal() {
        // The convention is that any value disables colour, including an
        // empty one, and that it overrides terminal capability rather than
        // being weighed against it.
        assert_eq!(
            ColorDepth::from_env(true, Some("truecolor"), Some("xterm-256color")),
            ColorDepth::None
        );
    }

    #[test]
    fn capability_degrades_rather_than_dropping_to_nothing() {
        assert_eq!(
            ColorDepth::from_env(false, Some("truecolor"), Some("xterm")),
            ColorDepth::TrueColor
        );
        assert_eq!(
            ColorDepth::from_env(false, None, Some("xterm-256color")),
            ColorDepth::Indexed256
        );
        // A terminal that says only "xterm" still gets the sixteen ANSI
        // colours. Falling to none here would punish an ordinary tmux.
        assert_eq!(
            ColorDepth::from_env(false, None, Some("xterm")),
            ColorDepth::Ansi16
        );
        assert_eq!(
            ColorDepth::from_env(false, None, Some("dumb")),
            ColorDepth::None
        );
    }

    #[test]
    fn a_title_is_spaced_uppercase() {
        let t = Theme::new(ColorDepth::Ansi16);
        assert_eq!(t.title("session").content, " S E S S I O N ");
    }

    #[test]
    fn focus_is_visible_without_color() {
        // Under NO_COLOR every style is default, so the only channel left is
        // the border glyph. A focused pane that looked identical to an
        // unfocused one would leave no cue for where typing goes.
        let t = Theme::new(ColorDepth::None);
        let focused = t.pane("input", true);
        let plain = t.pane("input", false);
        assert_ne!(format!("{focused:?}"), format!("{plain:?}"));
    }
}
