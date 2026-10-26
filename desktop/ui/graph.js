// Draws the run's action graph as SVG from the layout the core computes
// (#303, `desktop/src/graph.rs`).
//
// Where every node, edge and label sits, whether a node is filled or a
// ring, its counts and which target labels show are all decided in Rust,
// where `cargo test` holds them; this module only turns that layout into
// SVG elements. The view's `graph` key carries it, beside `actions`.
//
// The graph says what kind of work the run did and what it was aimed at,
// never whether it was correct: that is the checks pane's (ADR 0004).
//
// Every name and target is model-supplied text (ADR 0007, S10): each
// element is made with createElementNS and each label filled through
// textContent, so text that carries markup is drawn as characters.
//
// Nothing here moves.

// make(doc, tag, attrs) is dom.js's svg: an SVG element with its attributes.
import { svg as make } from "./dom.js";
import { isCount } from "./fmt.js";

// One string of the layout at its point, or nothing when the layout sent
// no text or no position for it.
function text(doc, t, cls) {
  if (t === null || typeof t !== "object" || typeof t.text !== "string" || t.text === "") {
    return null;
  }
  if (!isCount(t.x) || !isCount(t.y)) return null;
  const anchor = t.anchor === "end" ? "end" : "start";
  const e = make(doc, "text", { x: t.x, y: t.y, "text-anchor": anchor, class: cls });
  e.textContent = t.text;
  return e;
}

function point(p) {
  return Array.isArray(p) && p.length === 2 && isCount(p[0]) && isCount(p[1]);
}

// The layout, drawn whole into the run section's graph container. A view
// with no layout leaves one line saying so, never an empty picture that
// could be read as a run that called nothing.
export function drawGraph(doc, pane, g) {
  if (
    g === null ||
    typeof g !== "object" ||
    !isCount(g.width) ||
    !isCount(g.height) ||
    !Array.isArray(g.nodes)
  ) {
    const p = doc.createElement("p");
    p.className = "empty";
    p.textContent = "No action graph reported yet.";
    pane.replaceChildren(p);
    return;
  }

  const svg = make(doc, "svg", {
    viewBox: `0 0 ${g.width} ${g.height}`,
    class: "graph-svg",
    role: "img",
    "aria-label": typeof g.aria_label === "string" ? g.aria_label : "Action graph",
  });

  for (const e of Array.isArray(g.edges) ? g.edges : []) {
    if (!e || !Array.isArray(e.points) || !e.points.every(point)) continue;
    svg.append(
      make(doc, "polyline", {
        points: e.points.map((p) => `${p[0]},${p[1]}`).join(" "),
        class: e.mark === "fill" ? "graph-edge" : "graph-edge graph-edge-idle",
      }),
    );
  }

  for (const n of g.nodes) {
    if (!n || !isCount(n.x) || !isCount(n.y)) continue;
    const r = isCount(n.r) ? n.r : 5;
    // A tool that changed the tree is the one node whose colour carries a
    // fact rather than a status, as in the TUI.
    let cls = "graph-node graph-ring";
    if (n.mark === "fill") cls = n.wrote === true ? "graph-node graph-wrote" : "graph-node graph-fill";
    svg.append(make(doc, "circle", { cx: n.x, cy: n.y, r, class: cls }));
    for (const [t, c] of [
      [n.name, n.mark === "fill" ? "graph-name" : "graph-name graph-idle"],
      [n.counts, "graph-counts"],
    ]) {
      const e = text(doc, t, c);
      if (e) svg.append(e);
    }
  }

  for (const l of Array.isArray(g.labels) ? g.labels : []) {
    const e = text(doc, l, "graph-target");
    if (e) svg.append(e);
  }

  const f = g.focus;
  if (f && isCount(f.x) && isCount(f.y)) {
    svg.append(make(doc, "circle", { cx: f.x, cy: f.y, r: isCount(f.r) ? f.r : 7, class: "graph-focus" }));
  }
  for (const [t, c] of [
    [g.focus_label, "graph-focus-label"],
    [g.caption, "graph-caption"],
  ]) {
    const e = text(doc, t, c);
    if (e) svg.append(e);
  }

  pane.replaceChildren(svg);
}
