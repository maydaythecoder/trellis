//! The codebase view's layout, from a `get_codebase` answer (#307).
//!
//! colibri's Brain view lays topic regions over a canvas, enters one, and
//! opens an item's details beside it. The same interaction suits trellis's
//! module graph: each top-level package is a region, and a package's modules
//! sit inside it. Everything the picture needs is decided here, where `cargo
//! test` holds it: each region's position and module count in the
//! overview's fixed `viewBox`; each package's module positions in its own
//! fixed `viewBox`, its internal edges, and its labels, at most six that do
//! not overlap (`graph::place_labels`), busiest module first; and each
//! module's edges in, edges out and doubt. The core emits the result as the
//! `codebase` event when a `get_codebase` answer arrives (`commands.rs`), and
//! `ui/codebase.js` draws it, owning only pan, zoom, search and which
//! package is open.
//!
//! What it shows is what trellis reported, no more. An edge is a call site
//! the resolver bound to a definition in another module; the absence of one
//! is the absence of a binding, not of a call. Edges are counted in
//! modules and doubt in call sites, and the two are never combined or
//! divided (CLAUDE.md rule 3, ADR 0014). A count the answer cannot give
//! exactly is said to be a floor (`edges_in_complete`), and a field the
//! answer left out is `null`, never 0.
//!
//! Module names and paths are repository text: the page writes them through
//! `textContent` only (ADR 0007, S10).

use crate::frames::Response;
use crate::graph::{self, CHAR_W, LABEL_ASCENT, LABEL_H, Rect, Text, round};
use serde_json::{Map, Value, json};

/// The overview's coordinate space: regions on a grid of cells, as many
/// rows as they need. The page scales it to its pane.
const OVERVIEW_W: f64 = 960.0;
const PAD: f64 = 10.0;
const CELL_H: f64 = 150.0;
/// A region's centre sits this far into its cell, with its name and count
/// below the largest circle a cell can hold.
const REGION_CY: f64 = 60.0;
const REGION_R_MAX: f64 = 40.0;
const REGION_R_MIN: f64 = 6.0;
const NAME_DY: f64 = REGION_R_MAX + 16.0;
const COUNT_DY: f64 = REGION_R_MAX + 30.0;

/// A package's coordinate space. Modules spread from its centre to at most
/// `SPREAD`, which leaves room either side for a label of `PATH_CHARS`
/// (`SPREAD + LABEL_GAP + PATH_CHARS * CHAR_W` is under half the width), so
/// every label kept is inside the picture.
const PACKAGE_W: f64 = 960.0;
const PACKAGE_H: f64 = 640.0;
const SPREAD: f64 = 260.0;
const MODULE_R: f64 = 5.0;
const LABEL_GAP: f64 = 9.0;
/// A label shows a module's path cut to this many characters from its end:
/// the end is what tells `pkg/commands/add.py` from `pkg/commands/remove.py`.
const PATH_CHARS: usize = 28;
/// The golden angle, π(3 − √5), in radians: successive modules turn by it,
/// so no two share a ray however many there are.
const GOLDEN_ANGLE: f64 = 2.399_963_229_728_653;

/// One module of the answer, as read. Every count is `None` when the answer
/// did not carry it as a count.
#[derive(Debug, Clone, PartialEq)]
struct Module {
    name: String,
    path: Option<String>,
    /// The modules it was bound to call, in the answer's order (busiest
    /// first). Not every one need be in the answer: those past its `limit`
    /// are named here and drawn nowhere.
    callees: Vec<String>,
    calls_not_drawn: Option<u64>,
    ambiguous: Option<u64>,
    unresolved: Option<u64>,
}

fn count(o: &Map<String, Value>, k: &str) -> Option<u64> {
    o.get(k).and_then(Value::as_u64)
}

/// The answer's modules. One without a string `module` is left out: there
/// is nothing to name it by. A call without a string `module` names no
/// edge.
fn read_modules(data: &Value) -> Vec<Module> {
    let Some(list) = data.get("modules").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(Value::as_object)
        .filter_map(|o| {
            let name = o.get("module")?.as_str()?.to_string();
            if name.is_empty() {
                return None;
            }
            let callees = o
                .get("calls")
                .and_then(Value::as_array)
                .map(|calls| {
                    calls
                        .iter()
                        .filter_map(|c| c.get("module").and_then(Value::as_str))
                        .filter(|c| *c != name)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            Some(Module {
                path: o.get("path").and_then(Value::as_str).map(str::to_string),
                callees,
                calls_not_drawn: count(o, "calls_not_drawn"),
                ambiguous: count(o, "ambiguous"),
                unresolved: count(o, "unresolved"),
                name,
            })
        })
        .collect()
}

/// The top-level package a dotted module name belongs to: `poetry` for
/// `poetry.console.commands.add` and for `poetry` itself.
pub fn package_of(module: &str) -> &str {
    module.split('.').next().unwrap_or(module)
}

/// `s` cut to `max` characters from its end, saying so with an ellipsis.
fn cut_start(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    std::iter::once('…')
        .chain(s.chars().skip(n - (max - 1)))
        .collect()
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A region of the overview: one top-level package.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub package: String,
    pub x: f64,
    pub y: f64,
    pub r: f64,
    /// How many of the answer's modules are in it.
    pub modules: usize,
    pub name: Text,
    pub count: Text,
}

/// One module placed in its package.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub module: String,
    pub path: Option<String>,
    pub x: f64,
    pub y: f64,
    /// How many of the answer's modules were bound to call this one: a
    /// floor unless the answer left nothing out ([`Codebase::edges_in_complete`]).
    pub edges_in: u64,
    /// How many modules this one was bound to call, those drawn and those
    /// the answer only counted; `None` when it did not count them.
    pub edges_out: Option<u64>,
    /// This module's call sites that bound to a candidate set, and to
    /// nothing first-party: its doubt, in call sites, never folded into an
    /// edge.
    pub ambiguous: Option<u64>,
    pub unresolved: Option<u64>,
}

impl Placed {
    /// Both kinds of call site the resolver did not bind to one definition,
    /// the set the pyright backstop is asked about (`doubt_sites`). Counted
    /// in call sites, apart from the edges, which are counted in modules.
    pub fn doubt(&self) -> Option<u64> {
        Some(self.ambiguous? + self.unresolved?)
    }
}

/// A package's own picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    pub package: String,
    pub modules: Vec<Placed>,
    /// Between two of its modules: from the caller's index to the callee's.
    pub edges: Vec<(usize, usize)>,
    /// The labels kept, busiest module first: the module's index, its
    /// text, and the box it takes.
    pub labels: Vec<(usize, Text, Rect)>,
}

/// The whole view.
#[derive(Debug, Clone, PartialEq)]
pub struct Codebase {
    /// The repository-wide figures, as the answer gave them: three counts
    /// in call sites and never a ratio (ADR 0014), and the module counts.
    pub modules_indexed: Option<u64>,
    pub modules_not_drawn: Option<u64>,
    pub bound_call_sites: Option<u64>,
    pub ambiguous: Option<u64>,
    pub unresolved: Option<u64>,
    /// Whether every module's edges in are exact: true only when the answer
    /// left out no module and no module's calls.
    pub edges_in_complete: bool,
    pub overview_height: f64,
    pub regions: Vec<Region>,
    /// Between two regions, as indices, the lower first: some module of
    /// one was bound to call some module of the other, among the calls the
    /// answer listed.
    pub region_edges: Vec<(usize, usize)>,
    /// One per region, in region order.
    pub packages: Vec<Package>,
}

/// The view for a `get_codebase` answer's `data`.
pub fn layout(data: &Value) -> Codebase {
    let modules = read_modules(data);
    let top = |k: &str| data.get(k).and_then(Value::as_u64);
    let modules_not_drawn = top("modules_not_drawn");
    let edges_in_complete =
        modules_not_drawn == Some(0) && modules.iter().all(|m| m.calls_not_drawn == Some(0));

    // Packages, busiest first: most modules, then by name, so the same
    // answer always draws the same picture.
    let mut names: Vec<&str> = Vec::new();
    for m in &modules {
        let p = package_of(&m.name);
        if !names.contains(&p) {
            names.push(p);
        }
    }
    let members = |p: &str| -> Vec<&Module> {
        modules
            .iter()
            .filter(|m| package_of(&m.name) == p)
            .collect()
    };
    let mut grouped: Vec<(&str, Vec<&Module>)> = names.iter().map(|&p| (p, members(p))).collect();
    grouped.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(b.0)));

    let edges_in = |name: &str| -> u64 {
        modules
            .iter()
            .filter(|m| m.callees.iter().any(|c| c == name))
            .count() as u64
    };

    let regions = place_regions(&grouped);
    let overview_height = {
        let cols = columns(grouped.len());
        let rows = grouped.len().div_ceil(cols).max(1);
        round(2.0 * PAD + CELL_H * rows as f64)
    };

    let mut region_edges: Vec<(usize, usize)> = Vec::new();
    for (i, (_, ms)) in grouped.iter().enumerate() {
        for m in ms {
            for c in &m.callees {
                let Some(j) = grouped.iter().position(|(p, _)| *p == package_of(c)) else {
                    continue;
                };
                let pair = (i.min(j), i.max(j));
                if i != j && !region_edges.contains(&pair) {
                    region_edges.push(pair);
                }
            }
        }
    }

    let packages = grouped
        .iter()
        .map(|(p, ms)| place_package(p, ms, &edges_in))
        .collect();

    Codebase {
        modules_indexed: top("modules_indexed"),
        modules_not_drawn,
        bound_call_sites: top("bound_call_sites"),
        ambiguous: top("ambiguous"),
        unresolved: top("unresolved"),
        edges_in_complete,
        overview_height,
        regions,
        region_edges,
        packages,
    }
}

/// The overview grid's column count: near square.
fn columns(n: usize) -> usize {
    ((n as f64).sqrt().ceil() as usize).max(1)
}

fn place_regions(grouped: &[(&str, Vec<&Module>)]) -> Vec<Region> {
    let cols = columns(grouped.len());
    let cell_w = (OVERVIEW_W - 2.0 * PAD) / cols as f64;
    // A circle no wider than its cell, and a name cut to the cell.
    let r_max = REGION_R_MAX.min(cell_w / 2.0 - 4.0);
    let name_chars = (((cell_w - 8.0) / CHAR_W).floor() as usize).max(4);
    let busiest = grouped.iter().map(|(_, m)| m.len()).max().unwrap_or(1) as f64;
    grouped
        .iter()
        .enumerate()
        .map(|(i, (p, ms))| {
            let (row, col) = (i / cols, i % cols);
            let x = round(PAD + cell_w * (col as f64 + 0.5));
            let y = round(PAD + CELL_H * row as f64 + REGION_CY);
            // Area follows the module count, so a package twice the size
            // does not look four times it.
            let r = REGION_R_MIN + (r_max - REGION_R_MIN) * (ms.len() as f64 / busiest).sqrt();
            Region {
                package: p.to_string(),
                x,
                y,
                r: round(r),
                modules: ms.len(),
                name: Text {
                    text: graph::cut(p, name_chars),
                    x,
                    y: round(y + NAME_DY),
                    anchor: "middle",
                },
                count: Text {
                    text: plural(ms.len(), "module", "modules"),
                    x,
                    y: round(y + COUNT_DY),
                    anchor: "middle",
                },
            }
        })
        .collect()
}

fn place_package(p: &str, ms: &[&Module], edges_in: &impl Fn(&str) -> u64) -> Package {
    let (cx, cy) = (PACKAGE_W / 2.0, PACKAGE_H / 2.0);
    let n = ms.len();
    // A sunflower: the busiest module at the centre and each next one a
    // little further out, so any number of modules fills the same disc
    // evenly and the layout of a module never depends on a random seed.
    let modules: Vec<Placed> = ms
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let radius = if n == 1 {
                0.0
            } else {
                SPREAD * ((i as f64 + 0.5) / n as f64).sqrt()
            };
            let angle = i as f64 * GOLDEN_ANGLE;
            Placed {
                module: m.name.clone(),
                path: m.path.clone(),
                x: round(cx + radius * angle.cos()),
                y: round(cy + radius * angle.sin()),
                edges_in: edges_in(&m.name),
                edges_out: m
                    .calls_not_drawn
                    .map(|not_drawn| m.callees.len() as u64 + not_drawn),
                ambiguous: m.ambiguous,
                unresolved: m.unresolved,
            }
        })
        .collect();

    let mut edges = Vec::new();
    for (i, m) in ms.iter().enumerate() {
        for c in &m.callees {
            if let Some(j) = ms.iter().position(|o| &o.name == c) {
                edges.push((i, j));
            }
        }
    }

    // Labels, busiest module first: each to the outside of its node, so a
    // label on the left half reads leftwards and never crosses the centre.
    let candidates: Vec<(usize, Text, Rect)> = modules
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let text = cut_start(m.path.as_deref().unwrap_or(&m.module), PATH_CHARS);
            let w = text.chars().count() as f64 * CHAR_W;
            let baseline = m.y + 4.0;
            let (x, anchor, left) = if m.x >= cx {
                (m.x + LABEL_GAP, "start", m.x + LABEL_GAP)
            } else {
                (m.x - LABEL_GAP, "end", m.x - LABEL_GAP - w)
            };
            let rect = Rect {
                x: left,
                y: baseline - LABEL_ASCENT,
                w,
                h: LABEL_H,
            };
            let t = Text {
                text,
                x: round(x),
                y: round(baseline),
                anchor,
            };
            (i, t, rect)
        })
        .collect();
    let boxes: Vec<Rect> = candidates.iter().map(|c| c.2).collect();
    let labels = graph::place_labels(&boxes)
        .into_iter()
        .map(|k| candidates[k].clone())
        .collect();

    Package {
        package: p.to_string(),
        modules,
        edges,
        labels,
    }
}

fn point(x: f64, y: f64) -> Value {
    json!([x, y])
}

impl Codebase {
    /// The view as the `codebase` event carries it.
    pub fn to_json(&self) -> Value {
        let regions: Vec<Value> = self
            .regions
            .iter()
            .map(|r| {
                json!({
                    "package": r.package,
                    "x": r.x,
                    "y": r.y,
                    "r": r.r,
                    "modules": r.modules,
                    "name": r.name.to_json(),
                    "count": r.count.to_json(),
                })
            })
            .collect();
        let region_edges: Vec<Value> = self
            .region_edges
            .iter()
            .map(|&(a, b)| {
                let (p, q) = (&self.regions[a], &self.regions[b]);
                json!({"from": a, "to": b, "points": [point(p.x, p.y), point(q.x, q.y)]})
            })
            .collect();
        let packages: Vec<Value> = self
            .packages
            .iter()
            .map(|p| {
                json!({
                    "package": p.package,
                    "width": PACKAGE_W,
                    "height": PACKAGE_H,
                    "modules": p.modules.iter().map(|m| json!({
                        "module": m.module,
                        "path": m.path,
                        "x": m.x,
                        "y": m.y,
                        "r": MODULE_R,
                        "edges_in": m.edges_in,
                        "edges_out": m.edges_out,
                        "doubt": m.doubt(),
                        "ambiguous": m.ambiguous,
                        "unresolved": m.unresolved,
                    })).collect::<Vec<_>>(),
                    "edges": p.edges.iter().map(|&(a, b)| {
                        let (s, t) = (&p.modules[a], &p.modules[b]);
                        json!({"from": a, "to": b, "points": [point(s.x, s.y), point(t.x, t.y)]})
                    }).collect::<Vec<_>>(),
                    "labels": p.labels.iter().map(|(i, t, _)| {
                        let mut v = t.to_json();
                        v["module"] = json!(i);
                        v
                    }).collect::<Vec<_>>(),
                })
            })
            .collect();
        json!({
            "modules_indexed": self.modules_indexed,
            "modules_listed": self.packages.iter().map(|p| p.modules.len()).sum::<usize>(),
            "modules_not_drawn": self.modules_not_drawn,
            "bound_call_sites": self.bound_call_sites,
            "ambiguous": self.ambiguous,
            "unresolved": self.unresolved,
            "edges_in_complete": self.edges_in_complete,
            "overview": {
                "width": OVERVIEW_W,
                "height": self.overview_height,
                "regions": regions,
                "edges": region_edges,
            },
            "packages": packages,
        })
    }
}

/// The `codebase` event for a `get_codebase` response: the view, or the
/// reason there is none (the no-`.git` refusal among them), as trellis
/// gave it.
pub fn event(r: &Response) -> Value {
    match (r.success, &r.data) {
        (Some(true), Some(data)) if data.is_object() => layout(data).to_json(),
        (Some(true), _) => json!({"error": "get_codebase answered with no graph"}),
        _ => json!({
            "error": r
                .error
                .clone()
                .unwrap_or_else(|| "get_codebase was refused, and trellis gave no reason".into()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One module as `get_codebase` sends it (`doctor_cmd::graph_json`).
    fn module(name: &str, calls: &[(&str, u64)], ambiguous: u64, unresolved: u64) -> Value {
        json!({
            "module": name,
            "path": format!("{}.py", name.replace('.', "/")),
            "defines": 1,
            "calls": calls.iter().map(|(m, c)| json!({"module": m, "call_sites": c}))
                .collect::<Vec<_>>(),
            "calls_not_drawn": 0,
            "ambiguous": ambiguous,
            "unresolved": unresolved,
        })
    }

    fn answer(modules: Vec<Value>, not_drawn: u64) -> Value {
        json!({
            "modules_indexed": modules.len() as u64 + not_drawn,
            "bound_call_sites": 9,
            "ambiguous": 2,
            "unresolved": 7,
            "modules": modules,
            "modules_not_drawn": not_drawn,
        })
    }

    #[test]
    fn packages_become_regions_with_module_counts() {
        // The risk: a module drawn in the wrong package, or a package's
        // count that is not how many of its modules trellis sent, so the
        // overview misstates the repository's shape.
        let data = answer(
            vec![
                module("tests.test_core", &[("poetry.core", 3)], 0, 4),
                module("poetry.core", &[], 0, 0),
                module("setup", &[], 0, 1),
                module("poetry", &[("poetry.core", 1)], 0, 0),
                module("poetry.console.commands.add", &[("poetry.core", 2)], 1, 0),
            ],
            0,
        );
        let c = layout(&data);
        let regions: Vec<(&str, usize)> = c
            .regions
            .iter()
            .map(|r| (r.package.as_str(), r.modules))
            .collect();
        // Busiest first, then by name.
        assert_eq!(regions, [("poetry", 3), ("setup", 1), ("tests", 1)]);
        assert_eq!(c.regions[0].count.text, "3 modules");
        assert_eq!(c.regions[1].count.text, "1 module");
        assert_eq!(c.packages.len(), c.regions.len(), "one package per region");
        let poetry: Vec<&str> = c.packages[0]
            .modules
            .iter()
            .map(|m| m.module.as_str())
            .collect();
        assert_eq!(
            poetry,
            ["poetry.core", "poetry", "poetry.console.commands.add"],
            "the answer's order, busiest first, kept"
        );
        // A bigger package is a bigger circle, and every region sits in the
        // overview's viewBox with its name and count.
        assert!(c.regions[0].r > c.regions[1].r);
        for r in &c.regions {
            assert!(r.x - r.r >= 0.0 && r.x + r.r <= OVERVIEW_W, "{r:?}");
            assert!(r.count.y < c.overview_height, "{r:?}");
        }
        // tests -> poetry, and nothing between setup and anything.
        assert_eq!(c.region_edges, [(0, 2)]);

        let v = c.to_json();
        assert_eq!(v["overview"]["regions"][0]["modules"], 3);
        assert_eq!(v["modules_listed"], 5);
        assert_eq!(v["modules_indexed"], 5);

        // Many packages still fit: the grid grows rows, never past a cell.
        let many: Vec<Value> = (0..50)
            .map(|i| module(&format!("pkg{i}.m"), &[], 0, 0))
            .collect();
        let c = layout(&answer(many, 0));
        assert_eq!(c.regions.len(), 50);
        for r in &c.regions {
            assert!(r.x - r.r >= 0.0 && r.x + r.r <= OVERVIEW_W, "{r:?}");
            assert!(r.count.y < c.overview_height, "{r:?}");
        }

        // An answer with no modules is an overview with no region, not an
        // error.
        let c = layout(&answer(vec![], 0));
        assert!(c.regions.is_empty() && c.packages.is_empty());
    }

    #[test]
    fn each_package_shows_at_most_six_labels_and_none_overlap() {
        // The risk: a label for every module, so a large package is a
        // block of text; or two labels drawn over each other.
        let mods: Vec<Value> = (0..40)
            .map(|i| module(&format!("big.sub.mod_{i:02}"), &[], 0, 0))
            .collect();
        let c = layout(&answer(mods, 0));
        let p = &c.packages[0];
        assert_eq!(p.modules.len(), 40);
        assert!(
            !p.labels.is_empty() && p.labels.len() <= 6,
            "{:?}",
            p.labels
        );
        for (i, a) in p.labels.iter().enumerate() {
            for b in &p.labels[i + 1..] {
                assert!(!a.2.overlaps(&b.2), "{a:?} and {b:?} overlap");
            }
            // Inside the package's viewBox, and the path, not the dotted
            // name (#559).
            assert!(a.2.x >= 0.0 && a.2.x + a.2.w <= PACKAGE_W, "{a:?}");
            assert!(a.1.text.ends_with(".py"), "{a:?}");
        }
        // Busiest first: the busiest module is always labelled.
        assert_eq!(p.labels[0].0, 0);
        assert_eq!(p.labels[0].1.text, "big/sub/mod_00.py");

        // A long path keeps its end.
        let long = format!("deep.{}.leaf", "x".repeat(60));
        let c = layout(&answer(vec![module(&long, &[], 0, 0)], 0));
        let t = &c.packages[0].labels[0].1;
        assert!(
            t.text.starts_with('…') && t.text.ends_with("leaf.py"),
            "{t:?}"
        );
        assert_eq!(t.text.chars().count(), PATH_CHARS);

        // Every module of every package sits inside its viewBox.
        for p in &c.packages {
            for m in &p.modules {
                assert!(m.x >= 0.0 && m.x <= PACKAGE_W && m.y >= 0.0 && m.y <= PACKAGE_H);
            }
        }
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
    fn module_edges_and_doubt_stay_separate_counts() {
        // The risk (CLAUDE.md rule 3, ADR 0014): doubt folded into the
        // edges, or the two divided into a figure that reads as how much of
        // the module is understood.
        let mut a = module("app.a", &[("app.b", 3), ("lib.c", 1)], 2, 5);
        a["calls_not_drawn"] = json!(1);
        let data = answer(
            vec![
                a,
                module("app.b", &[], 0, 0),
                module("lib.c", &[("app.b", 1)], 0, 3),
            ],
            0,
        );
        let c = layout(&data);
        let app = &c.packages[0];
        let a = &app.modules[0];
        assert_eq!(a.module, "app.a");
        assert_eq!(a.edges_out, Some(3), "two listed and one only counted");
        assert_eq!(a.edges_in, 0);
        assert_eq!(
            (a.ambiguous, a.unresolved, a.doubt()),
            (Some(2), Some(5), Some(7))
        );
        let b = &app.modules[1];
        assert_eq!(b.edges_in, 2, "app.a and lib.c call it");
        assert_eq!(b.edges_out, Some(0));
        assert_eq!(b.doubt(), Some(0));
        assert_eq!(app.edges, [(0, 1)], "only the edge inside the package");
        // One module's calls were only counted, so edges in are a floor.
        assert!(!c.edges_in_complete);

        let v = c.to_json();
        let m = &v["packages"][0]["modules"][0];
        assert_eq!(
            (&m["edges_in"], &m["edges_out"], &m["doubt"]),
            (&json!(0), &json!(3), &json!(7))
        );
        let mut ks = Vec::new();
        keys(&v, &mut ks);
        let bad: Vec<&String> = ks
            .iter()
            .filter(|k| {
                let k = k.to_lowercase();
                ["ratio", "percent", "fraction", "total"]
                    .iter()
                    .any(|w| k.contains(w))
            })
            .collect();
        assert!(bad.is_empty(), "the view derives a figure: {bad:?}");

        // A count the answer did not carry is not 0.
        let c = layout(&json!({"modules": [{"module": "x.y", "calls": "?"}]}));
        let m = &c.packages[0].modules[0];
        assert_eq!((m.edges_out, m.ambiguous, m.doubt()), (None, None, None));
        assert_eq!(c.modules_indexed, None);
        assert!(!c.edges_in_complete, "nothing says nothing was left out");
        let v = c.to_json();
        assert_eq!(v["packages"][0]["modules"][0]["doubt"], Value::Null);

        // With nothing left out, edges in are exact.
        let c = layout(&answer(vec![module("p.q", &[], 0, 0)], 0));
        assert!(c.edges_in_complete);
        let c = layout(&answer(vec![module("p.q", &[], 0, 0)], 4));
        assert!(!c.edges_in_complete, "four modules past the limit");
    }

    fn ui_file(name: &str) -> String {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("ui")
            .join(name);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }

    #[test]
    fn codebase_script_writes_text_and_adds_no_motion() {
        // The risk: a module name (repository text) parsed as markup, or a
        // package entered with an animated camera a reduced-motion user
        // still sees; or a script the page never loads.
        let js = ui_file("codebase.js");
        for want in [
            "createElementNS",
            "textContent",
            "viewBox",
            "rpc_send",
            "\"get_codebase\"",
        ] {
            assert!(js.contains(want), "codebase.js lacks {want}");
        }
        for bad in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            ".style",
            "cssText",
            "transition",
            "animat",
            "requestAnimationFrame",
            "scrollIntoView",
        ] {
            assert!(!js.contains(bad), "codebase.js uses {bad}");
        }
        let html = ui_file("index.html");
        assert!(
            html.contains(r#"<script type="module" src="codebase.js"></script>"#),
            "index.html does not load codebase.js as a module"
        );
        let (rules, moving) = crate::graph::motion_in_rules_naming("cb-");
        assert!(rules > 0, "style.css has no rule for the codebase view");
        assert!(moving.is_empty(), "codebase rules that move: {moving:?}");
    }

    #[test]
    fn a_refusal_becomes_its_reason() {
        // The risk: the no-.git refusal (#559) drawn as an empty graph, so
        // a folder that is not a repository looks like one with no modules.
        let refused = Response {
            command: Some("get_codebase".into()),
            success: Some(false),
            id: Some("c".into()),
            data: None,
            error: Some("get_codebase refused: /x has no .git entry".into()),
        };
        assert_eq!(
            event(&refused),
            json!({"error": "get_codebase refused: /x has no .git entry"})
        );
        let no_data = Response {
            success: Some(true),
            ..refused.clone()
        };
        assert!(event(&no_data)["error"].is_string());
        let ok = Response {
            success: Some(true),
            data: Some(answer(vec![module("a.b", &[], 0, 0)], 0)),
            ..refused
        };
        assert_eq!(event(&ok)["overview"]["regions"][0]["package"], "a");
    }
}
