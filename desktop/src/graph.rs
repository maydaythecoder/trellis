//! The run's action graph, laid out for the page to draw as SVG (#303).
//!
//! The ratatui TUI's action graph (`render_actions` in its `view.rs`, retired
//! with #516) drew the tools the prompt offers as nodes converging
//! on a focus: filled for a tool that ran, a ring for one offered and not
//! called, its calls counted against it and its writes counted apart. The
//! desktop draws that picture. Everything the picture needs (where each
//! node sits in a fixed `viewBox`, fill or ring, the count text, which
//! target labels show and where) is decided here, where `cargo test` holds
//! it, and `ui/graph.js` only draws what this sends in the `view` event.
//!
//! The graph says what kind of work happened and what it was aimed at. It
//! says nothing about whether any of it was correct: that is the checks
//! pane's, and only an executed check can say it (ADR 0004). So no key here
//! names a verdict, and a `run_checks` node is drawn as any other tool.
//!
//! Every name and target is model-supplied text. It is cut to fit here (so
//! the label boxes below are the boxes drawn) and reaches the page as
//! strings the page writes through `textContent` (ADR 0007, S10).
//!
//! Nothing in the graph moves, so there is no motion to reduce.

use serde_json::{Value, json};

/// The tools the graph always draws, in the order the TUI's `TOOLS` lists
/// them. A fixed list rather than a tally of what was called, so a tool
/// that was available and not used shows as a ring: a run that never read
/// a file before editing it looks different from one that did.
pub const TOOLS: [&str; 4] = ["blast_radius", "read_file", "edit", "run_checks"];

/// The most target labels a picture keeps, as colibri's `cortex.ts` caps
/// its node labels: past six, labels stop being read and start being noise.
pub const MAX_LABELS: usize = 6;

/// The most rows a picture draws. A model may call a new tool name every
/// turn, bounded only by the turn budget, and a row per name would bury
/// the picture; past this many, the names first called later share the
/// last row, which counts them. Twenty-four rows is the four offered tools
/// and twenty others, about 700 units of `viewBox` height.
pub const MAX_NODES: usize = 24;

/// The layout's coordinate space. The page scales the SVG to its pane, so
/// these are units of the `viewBox`, not pixels.
const WIDTH: f64 = 800.0;
/// Room above the first row for its target label, and below the last for
/// the caption.
const TOP: f64 = 24.0;
const BOTTOM: f64 = 36.0;
/// The distance between rows: two lines of text, so a row's target label
/// fits above its edge without reaching the row above. A run with more
/// tools makes a taller picture, never a more crowded one.
const ROW: f64 = 28.0;

/// The columns, left to right: tool name, counts (right-aligned against the
/// node), node, target labels, the point where edges turn, the focus.
const NAME_X: f64 = 8.0;
const COUNTS_X: f64 = 268.0;
const NODE_X: f64 = 280.0;
const NODE_R: f64 = 5.0;
const LABEL_X: f64 = 292.0;
const BUS_X: f64 = 640.0;
const FOCUS_X: f64 = 700.0;
const FOCUS_R: f64 = 7.0;

/// The graph's text is 12-unit monospace (`style.css`), so a string's width
/// is its length in characters times this, which is what makes a label's
/// box a measurement rather than a guess.
pub(crate) const CHAR_W: f64 = 7.2;
/// A label's box: from its ascender above the baseline to its descender
/// below.
pub(crate) const LABEL_ASCENT: f64 = 11.0;
pub(crate) const LABEL_H: f64 = 14.0;
/// A tool name is cut to this many characters, so it ends before the
/// widest count text begins.
const NAME_CHARS: usize = 16;
/// A target is cut to this many characters, so its label ends before the
/// edges turn towards the focus.
const TARGET_CHARS: usize = 46;

/// One turn as the graph needs it: DESK-08's action (`state.rs`).
#[derive(Debug, Clone, Copy)]
pub struct Called<'a> {
    pub tool: &'a str,
    pub target: &'a str,
    /// Whether the turn changed the tree: an applied `edit`, or a
    /// `write_file` that created its file.
    pub wrote: bool,
}

/// A label's box in the layout's units: `x`, `y` its top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    /// Whether the two share any area. Boxes that only touch do not.
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }
}

/// Which of `boxes`, given in priority order, are drawn: each in turn is
/// kept unless it overlaps one already kept, and no more than
/// [`MAX_LABELS`] are kept. Returns their indices, in priority order.
///
/// colibri's `cortex.ts` (77-82) places node labels the same way; the
/// codebase view (#307) reuses this for its own labels.
pub fn place_labels(boxes: &[Rect]) -> Vec<usize> {
    let mut kept: Vec<usize> = Vec::new();
    for (i, b) in boxes.iter().enumerate() {
        if kept.len() == MAX_LABELS {
            break;
        }
        if kept.iter().all(|&k| !boxes[k].overlaps(b)) {
            kept.push(i);
        }
    }
    kept
}

/// A string the page draws at a point. `anchor` is SVG's `text-anchor`:
/// `start` draws rightwards from `x`, `end` leftwards.
#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub anchor: &'static str,
}

/// How a node is drawn: filled for a tool that ran, a ring for one offered
/// and not called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Fill,
    Ring,
}

impl Mark {
    fn key(self) -> &'static str {
        match self {
            Mark::Fill => "fill",
            Mark::Ring => "ring",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub tool: String,
    pub x: f64,
    pub y: f64,
    pub mark: Mark,
    /// How many turns called this tool.
    pub calls: u64,
    /// How many of those turns changed the tree. Counted apart from
    /// `calls`, never as a share of them.
    pub writes: u64,
    pub name: Text,
    /// Empty for a tool not called: a ring has nothing to count.
    pub counts: Text,
    /// How many tool names share this row: 0 for a tool's own row, and for
    /// the last row past [`MAX_NODES`] the names folded into it, whose calls
    /// and writes it counts.
    pub folded: u64,
}

/// The whole picture, in the layout's units.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub width: f64,
    pub height: f64,
    pub nodes: Vec<Node>,
    /// One per node, in node order: from the node, along its row to where
    /// edges turn, then to the focus.
    pub edges: Vec<[(f64, f64); 3]>,
    /// The target labels [`place_labels`] kept, newest turn first.
    pub labels: Vec<Text>,
    pub focus: (f64, f64),
    /// `n actions`, beside the focus.
    pub focus_label: Text,
    /// How many actions reached the tree, said as a count of actions, never
    /// as a fraction of them (ADR 0014).
    pub caption: Text,
    /// The picture said in words, for a screen reader: SVG text is read
    /// out of order or not at all.
    pub aria_label: String,
}

/// `v` to `places` decimals, so a layout's numbers come out the same on
/// every run and its JSON stays short: one place is enough for a `viewBox`
/// (this file's and `codebase.rs`'s), and the Atlas keeps six for its
/// fractions of the world (`atlas.rs`). The scale is an integer power, so
/// it is exact.
pub(crate) fn round(v: f64, places: u32) -> f64 {
    let scale = 10_u64.pow(places) as f64;
    (v * scale).round() / scale
}

/// `s` cut to `max` characters, saying so with an ellipsis rather than
/// letting it run under the next column.
pub(crate) fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max - 1).chain(['…']).collect()
}

/// "1 call", "2 calls": a count said with its noun, for every view's text.
pub(crate) fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A node's count text: its calls, then its writes apart from them.
fn counts_text(calls: u64, writes: u64) -> String {
    if calls == 0 {
        return String::new();
    }
    let mut t = plural(calls, "call", "calls");
    if writes > 0 {
        t.push_str(&format!(" · {writes} wrote"));
    }
    t
}

/// The graph for a run's actions, oldest first as `state.rs` keeps them.
pub fn layout<'a>(actions: impl IntoIterator<Item = Called<'a>>) -> Layout {
    let actions: Vec<Called<'a>> = actions.into_iter().collect();

    // Anything the model called that `TOOLS` has not heard of still belongs
    // on the graph, in the order it was first called: a node missing
    // because a constant was out of date would make the picture a lie
    // about what ran.
    let mut tools: Vec<&str> = TOOLS.to_vec();
    for a in &actions {
        if !tools.contains(&a.tool) {
            tools.push(a.tool);
        }
    }
    // Past the cap, the last row takes every name from there on. Each
    // action is counted on the row its tool is drawn on, so no call drops
    // out of the picture. `TOOLS` is shorter than the cap, so the offered
    // tools always have rows of their own.
    let folded = tools.split_off((MAX_NODES - 1).min(tools.len()));
    let folded = if folded.len() == 1 {
        tools.push(folded[0]);
        Vec::new()
    } else {
        folded
    };
    let rows = tools.len() + usize::from(!folded.is_empty());
    let row_of =
        |tool: &str| -> usize { tools.iter().position(|&t| t == tool).unwrap_or(tools.len()) };

    // `TOOLS` is never empty, so there is at least one gap.
    let gaps = (rows - 1) as f64;
    let height = TOP + BOTTOM + ROW * gaps;
    let row_y = |i: usize| round(TOP + ROW * i as f64, 1);
    let focus = (FOCUS_X, round(TOP + ROW * gaps / 2.0, 1));

    let node = |i: usize, label: String, n_folded: u64| {
        let y = row_y(i);
        let on_row: Vec<&Called> = actions.iter().filter(|a| row_of(a.tool) == i).collect();
        let calls = on_row.len() as u64;
        let writes = on_row.iter().filter(|a| a.wrote).count() as u64;
        Node {
            name: Text {
                text: cut(&label, NAME_CHARS),
                x: NAME_X,
                y: round(y + 4.0, 1),
                anchor: "start",
            },
            tool: label,
            x: NODE_X,
            y,
            mark: if calls == 0 { Mark::Ring } else { Mark::Fill },
            calls,
            writes,
            counts: Text {
                text: counts_text(calls, writes),
                x: COUNTS_X,
                y: round(y + 4.0, 1),
                anchor: "end",
            },
            folded: n_folded,
        }
    };
    let mut nodes: Vec<Node> = tools
        .iter()
        .enumerate()
        .map(|(i, &tool)| node(i, tool.to_string(), 0))
        .collect();
    if !folded.is_empty() {
        let n = folded.len() as u64;
        nodes.push(node(tools.len(), format!("{n} more tools"), n));
    }

    let edges = nodes
        .iter()
        .map(|n| {
            [
                (n.x + NODE_R, n.y),
                (BUS_X, n.y),
                (focus.0 - FOCUS_R, focus.1),
            ]
        })
        .collect();

    // Target labels, newest turn first: what the run is aimed at now
    // matters more than where it started. Each sits on its tool's row, so a
    // tool's older targets overlap its newest and are the ones dropped.
    let candidates: Vec<(Text, Rect)> = actions
        .iter()
        .rev()
        .filter(|a| !a.target.trim().is_empty())
        .map(|a| {
            let row = row_of(a.tool);
            let text = cut(a.target.trim(), TARGET_CHARS);
            let baseline = row_y(row) - 4.0;
            let w = text.chars().count() as f64 * CHAR_W;
            let rect = Rect {
                x: LABEL_X,
                y: baseline - LABEL_ASCENT,
                w,
                h: LABEL_H,
            };
            let t = Text {
                text,
                x: LABEL_X,
                y: round(baseline, 1),
                anchor: "start",
            };
            (t, rect)
        })
        .collect();
    let boxes: Vec<Rect> = candidates.iter().map(|(_, r)| *r).collect();
    let labels = place_labels(&boxes)
        .into_iter()
        .map(|i| candidates[i].0.clone())
        .collect();

    let n = actions.len() as u64;
    let wrote = actions.iter().filter(|a| a.wrote).count() as u64;
    let caption = if n == 0 {
        "nothing yet".to_string()
    } else if wrote == 0 {
        "nothing written to the tree".to_string()
    } else {
        format!("{wrote} wrote to the tree")
    };

    let called: Vec<String> = nodes
        .iter()
        .filter(|n| n.mark == Mark::Fill)
        .map(|n| format!("{} {}", n.tool, n.counts.text))
        .collect();
    let offered: Vec<&str> = nodes
        .iter()
        .filter(|n| n.mark == Mark::Ring)
        .map(|n| n.tool.as_str())
        .collect();
    let mut aria = format!("Action graph: {}.", plural(n, "action", "actions"));
    if called.is_empty() {
        aria.push_str(" Nothing called yet.");
    } else {
        aria.push_str(&format!(" Called: {}.", called.join("; ")));
    }
    if !offered.is_empty() {
        aria.push_str(&format!(" Not called: {}.", offered.join(", ")));
    }

    Layout {
        width: WIDTH,
        height: round(height, 1),
        nodes,
        edges,
        labels,
        focus,
        focus_label: Text {
            text: plural(n, "action", "actions"),
            x: FOCUS_X + FOCUS_R + 5.0,
            y: round(focus.1 + 4.0, 1),
            anchor: "start",
        },
        caption: Text {
            text: caption,
            x: NAME_X,
            y: round(height - 10.0, 1),
            anchor: "start",
        },
        aria_label: aria,
    }
}

impl Text {
    pub(crate) fn to_json(&self) -> Value {
        json!({"text": self.text, "x": self.x, "y": self.y, "anchor": self.anchor})
    }
}

impl Layout {
    /// The graph as the `view` event's `graph` key carries it.
    pub fn to_json(&self) -> Value {
        json!({
            "width": self.width,
            "height": self.height,
            "nodes": self.nodes.iter().map(|n| json!({
                "tool": n.tool,
                "x": n.x,
                "y": n.y,
                "r": NODE_R,
                "mark": n.mark.key(),
                "wrote": n.writes > 0,
                "calls": n.calls,
                "writes": n.writes,
                "name": n.name.to_json(),
                "counts": n.counts.to_json(),
                "folded": n.folded,
            })).collect::<Vec<_>>(),
            "edges": self.nodes.iter().zip(&self.edges).map(|(n, e)| json!({
                "mark": n.mark.key(),
                "points": e.iter().map(|&(x, y)| json!([x, y])).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "labels": self.labels.iter().map(Text::to_json).collect::<Vec<_>>(),
            "focus": {"x": self.focus.0, "y": self.focus.1, "r": FOCUS_R},
            "focus_label": self.focus_label.to_json(),
            "caption": self.caption.to_json(),
            "aria_label": self.aria_label,
        })
    }
}

/// For the page tests: how many rules in `ui/style.css` sit under a
/// selector containing `word` (an `@media` block's rules count under it),
/// and those of them that declare a `transition` or an `animation`.
#[cfg(test)]
pub(crate) fn motion_in_rules_naming(word: &str) -> (usize, Vec<String>) {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/style.css");
    let css = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let mut text = String::new();
    let mut rest = css.as_str();
    while let Some(i) = rest.find("/*") {
        text.push_str(&rest[..i]);
        let end = rest[i..].find("*/").expect("an unclosed CSS comment");
        rest = &rest[i + end + 2..];
    }
    text.push_str(rest);

    // Each open block's selector, outermost first, and the declarations
    // written directly inside the innermost.
    let mut open: Vec<String> = Vec::new();
    let mut segment = String::new();
    let (mut rules, mut moving) = (0, Vec::new());
    for c in text.chars() {
        match c {
            '{' => {
                open.push(segment.trim().to_string());
                segment.clear();
            }
            '}' => {
                if open.iter().any(|s| s.contains(word)) {
                    rules += 1;
                    let body = segment.to_lowercase();
                    if body.contains("transition") || body.contains("animation") {
                        moving.push(format!("{:?}: {}", open.last(), segment.trim()));
                    }
                }
                open.pop().expect("an unbalanced }");
                segment.clear();
            }
            _ => segment.push(c),
        }
    }
    assert!(open.is_empty(), "an unclosed CSS block");
    (rules, moving)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn plural_and_round_are_defined_once() {
        // The risk (#824): a second copy drifts, so two views word a count
        // or place a coordinate two ways. Every `.rs` file under src/ is
        // read, and `plural` and `round` are each defined in this file only.
        fn walk(dir: &Path, found: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let p = entry.unwrap().path();
                if p.is_dir() {
                    walk(&p, found);
                } else if p.extension().is_some_and(|e| e == "rs") {
                    let src = std::fs::read_to_string(&p).unwrap();
                    for line in src.lines() {
                        let t = line.trim_start();
                        let t = t.strip_prefix("pub(crate) ").unwrap_or(t);
                        let t = t.strip_prefix("pub ").unwrap_or(t);
                        for name in ["plural", "round"] {
                            if t.starts_with(&format!("fn {name}(")) {
                                let file = p.file_name().unwrap().to_string_lossy();
                                found.push(format!("{file}: {name}"));
                            }
                        }
                    }
                }
            }
        }
        let mut found = Vec::new();
        walk(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut found,
        );
        found.sort();
        assert_eq!(found, ["graph.rs: plural", "graph.rs: round"]);
        assert_eq!(plural(1, "call", "calls"), "1 call");
        assert_eq!(plural(0, "call", "calls"), "0 calls");
        assert_eq!(round(12.345_67, 1), 12.3);
        assert_eq!(round(0.123_456_789, 6), 0.123_457);
    }

    fn called<'a>(tool: &'a str, target: &'a str, wrote: bool) -> Called<'a> {
        Called {
            tool,
            target,
            wrote,
        }
    }

    fn node<'a>(l: &'a Layout, tool: &str) -> &'a Node {
        l.nodes
            .iter()
            .find(|n| n.tool == tool)
            .unwrap_or_else(|| panic!("no node for {tool}: {l:?}"))
    }

    fn ui_file(name: &str) -> String {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui").join(name);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }

    fn at(x: f64, y: f64) -> Rect {
        Rect {
            x,
            y,
            w: 40.0,
            h: 14.0,
        }
    }

    #[test]
    fn one_node_per_offered_tool_then_one_per_other_tool_called() {
        // The risk (the TUI's rule): a tool the model called that `TOOLS`
        // does not list is left off, so the picture says it never ran; or
        // the order shifts between views, so the same run looks different.
        let l = layout([
            called("grep", "def f", false),
            called("read_file", "a.py", false),
            called("bash", "pytest -q", false),
            called("grep", "def g", false),
            called("edit", "a.py", true),
        ]);
        let tools: Vec<&str> = l.nodes.iter().map(|n| n.tool.as_str()).collect();
        assert_eq!(
            tools,
            [
                "blast_radius",
                "read_file",
                "edit",
                "run_checks",
                "grep",
                "bash"
            ]
        );
        assert_eq!(l.edges.len(), l.nodes.len(), "one edge per node");

        // No action at all still draws the four, each with its own row.
        let empty = layout([]);
        let tools: Vec<&str> = empty.nodes.iter().map(|n| n.tool.as_str()).collect();
        assert_eq!(tools, TOOLS);
        let ys: Vec<f64> = empty.nodes.iter().map(|n| n.y).collect();
        assert!(ys.windows(2).all(|w| w[0] < w[1]), "rows in order: {ys:?}");

        // Every node and the focus sit inside the viewBox, however many
        // tools there are: past what fits, the picture grows taller.
        let names: Vec<String> = (0..30).map(|i| format!("custom_tool_{i}")).collect();
        let many = layout(names.iter().map(|n| called(n, "", false)));
        // Past MAX_NODES the rest fold into one node (#303 review).
        assert_eq!(many.nodes.len(), MAX_NODES);
        for n in &many.nodes {
            assert!(
                n.y > 0.0 && n.y < many.height,
                "{n:?} outside 0..{}",
                many.height
            );
        }
        let ys: Vec<f64> = many.nodes.iter().map(|n| n.y).collect();
        assert!(
            ys.windows(2).all(|w| w[1] - w[0] >= ROW - 0.1),
            "rows closer than two lines of text: {ys:?}"
        );
        assert!(many.focus.1 > 0.0 && many.focus.1 < many.height);
    }

    #[test]
    fn distinct_tools_past_the_cap_fold_into_one_other_node() {
        // The risk (#303 review): a node per distinct tool name the model
        // calls, bounded only by the turn budget, so a model that invents a
        // name each turn draws hundreds of rows and buries the picture.
        // Past MAX_NODES the newest names fold into one last node that
        // counts them; no call is dropped from the picture.
        assert_eq!(MAX_NODES, 24);
        let names: Vec<String> = (0..30).map(|i| format!("custom_{i}")).collect();
        let mut actions: Vec<Called> = names.iter().map(|n| called(n, "", false)).collect();
        actions.push(called("custom_29", "z.py", true));
        actions.push(called("read_file", "a.py", false));
        let l = layout(actions.iter().copied());
        assert_eq!(l.nodes.len(), MAX_NODES);
        assert_eq!(l.edges.len(), MAX_NODES, "one edge per node");
        let tools: Vec<&str> = l.nodes.iter().map(|n| n.tool.as_str()).collect();
        assert_eq!(&tools[..4], TOOLS, "the offered tools are always drawn");
        assert_eq!(tools[4], "custom_0", "then the others, first called first");
        assert_eq!(tools[MAX_NODES - 2], "custom_18");
        let other = l.nodes.last().unwrap();
        // custom_19 to custom_29: eleven names, twelve calls, one wrote.
        assert_eq!(other.folded, 11);
        assert_eq!(other.tool, "11 more tools");
        assert_eq!(other.name.text, "11 more tools");
        assert_eq!((other.calls, other.writes), (12, 1));
        assert_eq!(other.counts.text, "12 calls · 1 wrote");
        assert_eq!(other.mark, Mark::Fill);
        assert!(l.nodes[..MAX_NODES - 1].iter().all(|n| n.folded == 0));
        let calls: u64 = l.nodes.iter().map(|n| n.calls).sum();
        assert_eq!(calls, actions.len() as u64, "every call is on a node");
        // A folded tool's target is labelled on the fold's row.
        let row = other.y - 4.0;
        assert!(
            l.labels.iter().any(|t| t.text == "z.py" && t.y == row),
            "{:?}",
            l.labels
        );
        let v = l.to_json();
        assert_eq!(v["nodes"][MAX_NODES - 1]["folded"], 11);
        assert_eq!(v["nodes"][0]["folded"], 0);
        assert!(
            l.aria_label.contains("11 more tools 12 calls"),
            "{}",
            l.aria_label
        );

        // At the cap nothing folds; one past it folds two into the last row.
        let at: Vec<String> = (0..MAX_NODES - TOOLS.len())
            .map(|i| format!("t{i}"))
            .collect();
        let l = layout(at.iter().map(|n| called(n, "", false)));
        assert_eq!(l.nodes.len(), MAX_NODES);
        assert!(
            l.nodes.iter().all(|n| n.folded == 0),
            "nothing folded at the cap"
        );
        let past: Vec<String> = (0..MAX_NODES - TOOLS.len() + 1)
            .map(|i| format!("t{i}"))
            .collect();
        let l = layout(past.iter().map(|n| called(n, "", false)));
        assert_eq!(l.nodes.len(), MAX_NODES);
        assert_eq!(l.nodes.last().unwrap().folded, 2);
        // A model tool that happens to share the fold's text is its own row.
        let mut with_lookalike = past.clone();
        with_lookalike.insert(0, "2 more tools".to_string());
        let l = layout(with_lookalike.iter().map(|n| called(n, "", false)));
        assert_eq!(l.nodes[4].tool, "2 more tools");
        assert_eq!(l.nodes[4].folded, 0);
        assert_eq!(l.nodes.last().unwrap().folded, 3);
    }

    #[test]
    fn a_called_tool_is_filled_and_an_offered_one_is_a_ring() {
        // The risk: an uncalled tool drawn filled reads as work the run did.
        let l = layout([
            called("read_file", "a.py", false),
            called("read_file", "b.py", false),
        ]);
        assert_eq!(node(&l, "read_file").mark, Mark::Fill);
        for tool in ["blast_radius", "edit", "run_checks"] {
            let n = node(&l, tool);
            assert_eq!(n.mark, Mark::Ring, "{tool}");
            assert_eq!(n.calls, 0, "{tool}");
            assert_eq!(n.counts.text, "", "a ring has nothing to count: {tool}");
        }
        let v = l.to_json();
        let marks: Vec<&str> = v["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["mark"].as_str().unwrap())
            .collect();
        assert_eq!(marks, ["ring", "fill", "ring", "ring"]);
        assert_eq!(v["nodes"][1]["counts"]["text"], "2 calls");
    }

    #[test]
    fn edit_counts_applied_writes_apart_from_calls() {
        // The risk: a refused edit counted as a write, or
        // writes folded into calls, so the graph says the run changed the
        // tree more (or less) than it did.
        let l = layout([
            called("edit", "a.py", false),
            called("edit", "a.py", true),
            called("edit", "b.py", false),
            called("write_file", "c.py", true),
        ]);
        let e = node(&l, "edit");
        assert_eq!((e.calls, e.writes), (3, 1));
        assert_eq!(e.counts.text, "3 calls · 1 wrote");
        let w = node(&l, "write_file");
        assert_eq!((w.calls, w.writes), (1, 1));
        assert_eq!(w.counts.text, "1 call · 1 wrote");
        assert_eq!(l.caption.text, "2 wrote to the tree");
        assert_eq!(l.focus_label.text, "4 actions");
        let v = l.to_json();
        assert_eq!(v["nodes"][2]["calls"], 3);
        assert_eq!(v["nodes"][2]["writes"], 1);
        assert_eq!(v["nodes"][2]["wrote"], true);

        let none = layout([called("edit", "a.py", false)]);
        assert_eq!(none.caption.text, "nothing written to the tree");
        assert_eq!(node(&none, "edit").writes, 0);
        assert_eq!(node(&none, "edit").counts.text, "1 call");
        assert_eq!(layout([]).caption.text, "nothing yet");
        assert_eq!(layout([]).focus_label.text, "0 actions");
    }

    #[test]
    fn place_labels_keeps_at_most_six() {
        // The risk: a label for every target, so a long run's graph is
        // buried under text nobody can read.
        let boxes: Vec<Rect> = (0..10).map(|i| at(0.0, i as f64 * 20.0)).collect();
        assert_eq!(place_labels(&boxes), [0, 1, 2, 3, 4, 5]);
        assert_eq!(place_labels(&boxes[..3]), [0, 1, 2]);
        assert!(place_labels(&[]).is_empty());

        // Through the layout: ten tools each with a target, six labels.
        let names: Vec<String> = (0..10).map(|i| format!("tool_{i}")).collect();
        let l = layout(names.iter().map(|n| called(n, "a.py", false)));
        assert_eq!(l.labels.len(), MAX_LABELS);
    }

    #[test]
    fn place_labels_keeps_no_two_that_overlap() {
        // The risk: two labels drawn over each other, so neither can be
        // read, or a lower-priority label kept over a higher one.
        let boxes = [
            at(0.0, 0.0),
            at(20.0, 5.0),  // overlaps 0
            at(0.0, 30.0),  // clear
            at(30.0, 35.0), // overlaps 2
            at(40.0, 0.0),  // touches 0's right edge: not an overlap
            at(0.0, 14.0),  // touches 0's bottom edge, overlaps nothing kept
        ];
        let kept = place_labels(&boxes);
        assert_eq!(kept, [0, 2, 4, 5]);
        for (i, &a) in kept.iter().enumerate() {
            for &b in &kept[i + 1..] {
                assert!(!boxes[a].overlaps(&boxes[b]), "{a} and {b} overlap");
            }
        }

        // Through the layout: a tool's targets share its row, so only the
        // newest is drawn, and labels on different rows all are.
        let l = layout([
            called("read_file", "old.py", false),
            called("read_file", "new.py", false),
            called("edit", "a.py", true),
        ]);
        let texts: Vec<&str> = l.labels.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, ["a.py", "new.py"], "newest first, one per row");
        // A label ends before the edges turn, however long its target.
        let long = "d/".repeat(60);
        let l = layout([called("read_file", &long, false)]);
        let t = &l.labels[0];
        assert!(t.text.ends_with('…'), "{t:?}");
        assert!(
            t.x + t.text.chars().count() as f64 * CHAR_W < BUS_X,
            "{t:?} runs into the edges"
        );
    }

    /// Every key in `v`, at any depth.
    fn keys(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                for (k, x) in o {
                    out.push(k.clone());
                    keys(x, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| keys(x, out)),
            _ => {}
        }
    }

    #[test]
    fn graph_view_carries_no_verdict() {
        // The risk (ADR 0004): the graph becomes a second place a check's
        // result is shown, drawn from something other than the check.
        let l = layout([
            called("run_checks", "a.py", false),
            called("edit", "a.py", true),
            called("run_checks", "a.py", false),
        ]);
        let mut ks = Vec::new();
        keys(&l.to_json(), &mut ks);
        assert!(ks.len() > 10, "the view was read: {ks:?}");
        let bad: Vec<&String> = ks
            .iter()
            .filter(|k| {
                let k = k.to_lowercase();
                ["verdict", "passed", "failed"]
                    .iter()
                    .any(|w| k.contains(w))
            })
            .collect();
        assert!(bad.is_empty(), "the graph carries a verdict: {bad:?}");
    }

    #[test]
    fn graph_script_draws_no_check_mark() {
        // The risk: graph.js drawing a tick or a cross against run_checks,
        // a verdict the graph has no business giving (ADR 0004).
        let js = ui_file("graph.js");
        assert!(js.contains("export function drawGraph"), "graph.js read");
        for bad in ["✓", "✗", "passed", "failed"] {
            assert!(!js.contains(bad), "graph.js contains {bad:?}");
        }
    }

    #[test]
    fn graph_styles_have_no_motion() {
        // The risk: an animated graph, which a user who asked for reduced
        // motion still sees moving; nothing in it needs to move.
        let (rules, moving) = motion_in_rules_naming("graph");
        assert!(rules > 0, "style.css has no rule naming the graph");
        assert!(moving.is_empty(), "graph rules that move: {moving:?}");
    }
}
