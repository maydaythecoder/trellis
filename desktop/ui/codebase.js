// The codebase view (#307): trellis's module graph, as the desktop core lays
// it out from a `get_codebase` answer (`desktop/src/codebase.rs`).
//
// The core decides where every region, module, edge and label sits, and
// which six labels a package keeps; it sends the result as the `codebase`
// event, or `{error}` with trellis's reason when it refused (a folder with
// no .git entry among them). This module draws that layout as SVG and owns
// only what is the page's to own: which package is open, which module is
// selected, the search box, and pan and zoom, which change the SVG's
// viewBox at once. Nothing here moves on its own, so there is no motion to
// reduce.
//
// Module names and paths are repository text (ADR 0007, S10): every
// element is made with createElement or createElementNS, and every name is
// written through textContent, so text that carries markup is drawn as
// characters.
//
// index.html loads this as a module of its own. It asks trellis for the
// graph through `rpc_send` when the view is shown and what it holds is
// stale: no graph yet, a new child, or a run that has ended since.

const SVG = "http://www.w3.org/2000/svg";
const REQUEST_ID = "desktop-codebase";
// The search lists at most this many modules, and says how many more.
const RESULTS_MAX = 20;
// One + or - zooms by this factor; the view is never wider than the whole
// picture, nor narrower than this share of it.
const ZOOM = 1.25;
const ZOOM_MIN = 1 / 8;
// One arrow key pans by this share of what is in view.
const PAN = 0.1;
// A pointer that moves less than this many pixels between down and up is a
// click, not a drag.
const DRAG_PX = 3;

function isCount(v) {
  return typeof v === "number" && Number.isFinite(v);
}

function str(v) {
  return typeof v === "string" ? v : null;
}

function el(doc, tag, text) {
  const e = doc.createElement(tag);
  if (text !== undefined) e.textContent = text;
  return e;
}

function make(doc, tag, attrs) {
  const e = doc.createElementNS(SVG, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
  return e;
}

// One string of the layout at its point, or nothing when the layout sent
// no text or no position for it.
function label(doc, t, cls) {
  if (t === null || typeof t !== "object" || typeof t.text !== "string" || t.text === "") {
    return null;
  }
  if (!isCount(t.x) || !isCount(t.y)) return null;
  const anchor = ["start", "middle", "end"].includes(t.anchor) ? t.anchor : "start";
  const e = make(doc, "text", { x: t.x, y: t.y, "text-anchor": anchor, class: cls });
  e.textContent = t.text;
  return e;
}

// An SVG <title>, which is the element's accessible name and its tooltip.
function title(doc, text) {
  const t = make(doc, "title", {});
  t.textContent = text;
  return t;
}

function line(doc, e, cls) {
  const p = e && e.points;
  if (!Array.isArray(p) || p.length !== 2 || !p.every((q) => Array.isArray(q) && isCount(q[0]) && isCount(q[1]))) {
    return null;
  }
  return make(doc, "line", { x1: p[0][0], y1: p[0][1], x2: p[1][0], y2: p[1][1], class: cls });
}

function status(doc, text) {
  doc.getElementById("cb-status").textContent = text;
}

// What the whole answer says, as trellis counted it: modules, then call
// sites as three counts that are never divided (ADR 0014).
function summary(d) {
  const n = (v) => (isCount(v) ? String(v) : "not reported");
  const parts = [`${n(d.modules_indexed)} modules indexed, ${n(d.modules_listed)} listed`];
  if (isCount(d.modules_not_drawn) && d.modules_not_drawn > 0) {
    parts[0] += ` (${d.modules_not_drawn} past the limit, not drawn)`;
  }
  parts.push(
    `${n(d.bound_call_sites)} call sites bound across modules, ${n(d.ambiguous)} ambiguous, ` +
      `${n(d.unresolved)} unresolved: three counts, not a ratio`,
  );
  return parts.join(" · ");
}

// Whether a drag just ended: the click that follows it is not a click.
function activate(e, ctx, fn) {
  e.addEventListener("click", () => {
    if (!ctx.dragged) fn();
  });
  e.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter" || ev.key === " ") {
      ev.preventDefault();
      ev.stopPropagation();
      fn();
    }
  });
}

function packages(ctx) {
  return ctx.data && Array.isArray(ctx.data.packages) ? ctx.data.packages : [];
}

function openPackage(ctx) {
  return ctx.pkg === null ? null : packages(ctx)[ctx.pkg] ?? null;
}

function selectedModule(ctx) {
  if (ctx.selected === null) return null;
  const p = packages(ctx)[ctx.selected.pkg];
  return p && Array.isArray(p.modules) ? p.modules[ctx.selected.module] ?? null : null;
}

// --- the viewBox ---------------------------------------------------------

function applyView(ctx) {
  const v = ctx.view;
  if (ctx.svg && v) ctx.svg.setAttribute("viewBox", `${v.x} ${v.y} ${v.w} ${v.h}`);
}

function resetView(ctx) {
  if (ctx.base) ctx.view = { ...ctx.base };
  applyView(ctx);
}

// The view's centre stays over the picture, so it cannot be panned away.
function clampView(ctx) {
  const v = ctx.view;
  const b = ctx.base;
  const cx = Math.min(Math.max(v.x + v.w / 2, b.x), b.x + b.w);
  const cy = Math.min(Math.max(v.y + v.h / 2, b.y), b.y + b.h);
  v.x = cx - v.w / 2;
  v.y = cy - v.h / 2;
}

function pan(ctx, dx, dy) {
  if (!ctx.view) return;
  ctx.view.x += dx;
  ctx.view.y += dy;
  clampView(ctx);
  applyView(ctx);
}

// Zooms about the centre of what is in view.
function zoom(ctx, factor) {
  const v = ctx.view;
  const b = ctx.base;
  if (!v) return;
  const w = Math.min(Math.max(v.w / factor, b.w * ZOOM_MIN), b.w);
  const h = (w / v.w) * v.h;
  v.x += (v.w - w) / 2;
  v.y += (v.h - h) / 2;
  v.w = w;
  v.h = h;
  clampView(ctx);
  applyView(ctx);
}

// A fresh SVG for a picture of `width` by `height`, its view reset.
function canvas(doc, ctx, width, height, name) {
  const svg = make(doc, "svg", { class: "cb-svg", role: "group" });
  svg.append(title(doc, name));
  ctx.svg = svg;
  ctx.base = { x: 0, y: 0, w: width, h: height };
  resetView(ctx);
  return svg;
}

// --- drawing -------------------------------------------------------------

function drawRegions(doc, ctx) {
  const o = ctx.data.overview;
  const graph = doc.getElementById("cb-graph");
  if (!o || !isCount(o.width) || !isCount(o.height) || !Array.isArray(o.regions) || o.regions.length === 0) {
    const p = el(doc, "p", "trellis indexed no first-party module here.");
    p.className = "empty";
    graph.replaceChildren(p);
    ctx.svg = null;
    ctx.view = null;
    return;
  }
  const svg = canvas(doc, ctx, o.width, o.height, "Every top-level package, as a region");
  for (const e of Array.isArray(o.edges) ? o.edges : []) {
    const l = line(doc, e, "cb-edge");
    if (l) svg.append(l);
  }
  o.regions.forEach((r, i) => {
    if (!r || !isCount(r.x) || !isCount(r.y) || !isCount(r.r)) return;
    const g = make(doc, "g", { class: "cb-region", tabindex: 0, role: "button" });
    const count = r.count && typeof r.count.text === "string" ? r.count.text : "";
    g.append(
      title(doc, `${str(r.package) ?? "package"}: ${count}, open`),
      make(doc, "circle", { cx: r.x, cy: r.y, r: r.r }),
    );
    for (const [t, cls] of [
      [r.name, "cb-name"],
      [r.count, "cb-count"],
    ]) {
      const e = label(doc, t, cls);
      if (e) g.append(e);
    }
    activate(g, ctx, () => enter(doc, ctx, i, null));
    svg.append(g);
  });
  graph.replaceChildren(svg);
}

function drawPackage(doc, ctx) {
  const p = openPackage(ctx);
  const graph = doc.getElementById("cb-graph");
  if (!p || !isCount(p.width) || !isCount(p.height) || !Array.isArray(p.modules)) {
    ctx.pkg = null;
    drawRegions(doc, ctx);
    return;
  }
  const svg = canvas(doc, ctx, p.width, p.height, `The modules of ${str(p.package) ?? "this package"}`);
  for (const e of Array.isArray(p.edges) ? p.edges : []) {
    const l = line(doc, e, "cb-edge");
    if (l) svg.append(l);
  }
  ctx.nodes = [];
  p.modules.forEach((m, k) => {
    if (!m || !isCount(m.x) || !isCount(m.y)) return;
    const c = make(doc, "circle", {
      cx: m.x,
      cy: m.y,
      r: isCount(m.r) ? m.r : 5,
      class: "cb-module",
      tabindex: 0,
      role: "button",
    });
    const name = str(m.module) ?? "module";
    const path = str(m.path);
    c.append(title(doc, path === null ? name : `${name} (${path})`));
    activate(c, ctx, () => select(doc, ctx, ctx.pkg, k));
    ctx.nodes[k] = c;
    svg.append(c);
  });
  for (const t of Array.isArray(p.labels) ? p.labels : []) {
    const e = label(doc, t, "cb-label");
    if (e) svg.append(e);
  }
  graph.replaceChildren(svg);
  markSelected(ctx);
}

function markSelected(ctx) {
  for (const [k, c] of (ctx.nodes ?? []).entries()) {
    if (!c) continue;
    const on = ctx.selected !== null && ctx.selected.pkg === ctx.pkg && ctx.selected.module === k;
    c.setAttribute("class", on ? "cb-module cb-selected" : "cb-module");
    if (on) c.setAttribute("aria-pressed", "true");
    else c.removeAttribute("aria-pressed");
  }
}

function drawCrumbs(doc, ctx) {
  const items = [];
  const all = el(doc, "li");
  const p = openPackage(ctx);
  if (p === null) {
    const s = el(doc, "span", "All packages");
    s.setAttribute("aria-current", "page");
    all.append(s);
  } else {
    const b = el(doc, "button", "All packages");
    b.type = "button";
    b.addEventListener("click", () => leave(doc, ctx));
    all.append(b);
  }
  items.push(all);
  if (p !== null) {
    const li = el(doc, "li");
    const s = el(doc, "span", str(p.package) ?? "package");
    s.setAttribute("aria-current", "page");
    li.append(s);
    items.push(li);
  }
  doc.getElementById("cb-crumbs").replaceChildren(...items);
  doc.getElementById("cb-back").disabled = p === null;
}

// The selected module's three numbers: edges in and edges out, which count
// modules, and doubt, which counts call sites. Different populations, so
// they are shown side by side and never combined.
function drawDetails(doc, ctx) {
  const pane = doc.getElementById("cb-details");
  const m = selectedModule(ctx);
  if (m === null) {
    const p = el(doc, "p", "Select a module to see its edges and doubt.");
    p.className = "empty";
    pane.replaceChildren(p);
    return;
  }
  const head = el(doc, "h4", str(m.module) ?? "module");
  const path = el(doc, "p");
  path.append(el(doc, "code", str(m.path) ?? "path not reported"));
  const dl = el(doc, "dl");
  const row = (name, value) => dl.append(el(doc, "dt", name), el(doc, "dd", value));
  const complete = ctx.data.edges_in_complete === true;
  row(
    "edges in",
    isCount(m.edges_in) ? (complete ? String(m.edges_in) : `at least ${m.edges_in}`) : "not reported",
  );
  row("edges out", isCount(m.edges_out) ? String(m.edges_out) : "not reported");
  row("doubt", isCount(m.doubt) ? String(m.doubt) : "not reported");
  const note = el(
    doc,
    "p",
    "Edges count modules the resolver bound a call between; doubt counts this module's call " +
      "sites it bound to no single definition. They come from different populations and are " +
      "not divided. An edge's absence is the absence of a binding, not of a call.",
  );
  note.className = "muted";
  const parts = [head, path, dl, note];
  if (!complete) {
    const floor = el(
      doc,
      "p",
      "Edges in count only the modules this answer lists, and trellis left some out, so the " +
        "figure is a floor.",
    );
    floor.className = "muted";
    parts.push(floor);
  }
  pane.replaceChildren(...parts);
}

function drawResults(doc, ctx) {
  const out = doc.getElementById("cb-results");
  const q = doc.getElementById("cb-search").value.trim().toLowerCase();
  if (ctx.data === null || q === "") {
    out.replaceChildren();
    return;
  }
  const hits = [];
  packages(ctx).forEach((p, i) => {
    (Array.isArray(p.modules) ? p.modules : []).forEach((m, k) => {
      const name = str(m && m.module);
      if (name !== null && name.toLowerCase().includes(q)) hits.push([i, k, name]);
    });
  });
  const rows = hits.slice(0, RESULTS_MAX).map(([i, k, name]) => {
    const li = el(doc, "li");
    const b = el(doc, "button", name);
    b.type = "button";
    b.addEventListener("click", () => enter(doc, ctx, i, k));
    li.append(b);
    return li;
  });
  if (hits.length === 0) {
    const li = el(doc, "li", "No module name matches.");
    li.className = "empty";
    rows.push(li);
  } else if (hits.length > RESULTS_MAX) {
    const li = el(doc, "li", `${hits.length - RESULTS_MAX} more match, not listed`);
    li.className = "empty";
    rows.push(li);
  }
  out.replaceChildren(...rows);
}

function draw(doc, ctx) {
  if (ctx.pkg === null) drawRegions(doc, ctx);
  else drawPackage(doc, ctx);
  drawCrumbs(doc, ctx);
  drawDetails(doc, ctx);
}

// --- moving between views ------------------------------------------------

// Enters package `i`, and selects its module `k` when one is named. The
// viewBox changes at once.
function enter(doc, ctx, i, k) {
  if (!packages(ctx)[i]) return;
  ctx.pkg = i;
  if (k !== null) ctx.selected = { pkg: i, module: k };
  draw(doc, ctx);
  doc.getElementById("cb-graph").focus();
}

function leave(doc, ctx) {
  ctx.pkg = null;
  draw(doc, ctx);
  doc.getElementById("cb-graph").focus();
}

function select(doc, ctx, i, k) {
  ctx.selected = { pkg: i, module: k };
  markSelected(ctx);
  drawDetails(doc, ctx);
}

// Draws one `codebase` event: the layout, or why there is none.
export function drawCodebase(doc, ctx, payload) {
  ctx.pending = false;
  if (payload === null || typeof payload !== "object") {
    status(doc, "trellis sent no codebase graph.");
    return;
  }
  if (typeof payload.error === "string") {
    ctx.data = null;
    ctx.pkg = null;
    ctx.selected = null;
    status(doc, payload.error);
    doc.getElementById("cb-graph").replaceChildren();
    ctx.svg = null;
    ctx.view = null;
    drawCrumbs(doc, ctx);
    drawDetails(doc, ctx);
    drawResults(doc, ctx);
    return;
  }
  // A new answer: the open package is kept if it is still there, and the
  // selection only if it still names the same module.
  const old = ctx.data;
  const was = (c) => (c === null || old === null ? null : packages({ data: old })[c]);
  const openName = str(was(ctx.pkg) && was(ctx.pkg).package);
  const sel = selectedModule(ctx);
  const selName = str(sel && sel.module);
  ctx.data = payload;
  ctx.pkg = null;
  ctx.selected = null;
  packages(ctx).forEach((p, i) => {
    if (openName !== null && p.package === openName) ctx.pkg = i;
    (Array.isArray(p.modules) ? p.modules : []).forEach((m, k) => {
      if (selName !== null && m.module === selName) ctx.selected = { pkg: i, module: k };
    });
  });
  status(doc, summary(payload));
  draw(doc, ctx);
  drawResults(doc, ctx);
}

// --- wiring --------------------------------------------------------------

function wirePanZoom(doc, ctx) {
  const graph = doc.getElementById("cb-graph");
  graph.addEventListener("keydown", (e) => {
    if (!ctx.view) return;
    const v = ctx.view;
    switch (e.key) {
      case "ArrowLeft":
        pan(ctx, -v.w * PAN, 0);
        break;
      case "ArrowRight":
        pan(ctx, v.w * PAN, 0);
        break;
      case "ArrowUp":
        pan(ctx, 0, -v.h * PAN);
        break;
      case "ArrowDown":
        pan(ctx, 0, v.h * PAN);
        break;
      case "+":
      case "=":
        zoom(ctx, ZOOM);
        break;
      case "-":
      case "_":
        zoom(ctx, 1 / ZOOM);
        break;
      case "Escape":
        resetView(ctx);
        break;
      default:
        return;
    }
    e.preventDefault();
  });

  // A drag pans. The pointer is captured only once it has moved, so a
  // plain click still reaches the region or module under it.
  let drag = null;
  graph.addEventListener("pointerdown", (e) => {
    ctx.dragged = false;
    if (!ctx.view || e.button !== 0) return;
    drag = { id: e.pointerId, x: e.clientX, y: e.clientY, vx: ctx.view.x, vy: ctx.view.y };
  });
  graph.addEventListener("pointermove", (e) => {
    if (drag === null || e.pointerId !== drag.id || !ctx.svg) return;
    const dx = e.clientX - drag.x;
    const dy = e.clientY - drag.y;
    if (!ctx.dragged && Math.hypot(dx, dy) < DRAG_PX) return;
    if (!ctx.dragged) {
      ctx.dragged = true;
      graph.setPointerCapture(e.pointerId);
    }
    const box = ctx.svg.getBoundingClientRect();
    if (box.width === 0 || box.height === 0) return;
    // The viewBox's units per pixel, as `xMidYMid meet` scales it.
    const scale = Math.max(ctx.view.w / box.width, ctx.view.h / box.height);
    ctx.view.x = drag.vx - dx * scale;
    ctx.view.y = drag.vy - dy * scale;
    clampView(ctx);
    applyView(ctx);
  });
  const end = (e) => {
    if (drag !== null && e.pointerId === drag.id) drag = null;
  };
  graph.addEventListener("pointerup", end);
  graph.addEventListener("pointercancel", end);
}

// Asks trellis for the graph. A refusal is the core's reason there is no
// child, said with what to do about it.
async function ask(doc, tauri, ctx) {
  if (ctx.pending) return;
  ctx.pending = true;
  ctx.stale = false;
  status(doc, "Reading the module graph: trellis indexes the repository for it.");
  try {
    await tauri.core.invoke("rpc_send", { line: { id: REQUEST_ID, type: "get_codebase" } });
  } catch (err) {
    ctx.pending = false;
    ctx.stale = true;
    status(doc, `Open a repository to see its graph. ${err}`);
  }
}

function shown(doc) {
  return !doc.getElementById("codebase").hidden;
}

export async function wireCodebase(doc, tauri) {
  const ctx = {
    data: null,
    pkg: null,
    selected: null,
    svg: null,
    view: null,
    base: null,
    nodes: [],
    dragged: false,
    pending: false,
    stale: true,
    lastOutcome: null,
  };
  wirePanZoom(doc, ctx);
  doc.getElementById("cb-back").addEventListener("click", () => leave(doc, ctx));
  doc.getElementById("cb-search").addEventListener("input", () => drawResults(doc, ctx));
  drawCrumbs(doc, ctx);
  drawDetails(doc, ctx);

  await tauri.event.listen("codebase", (event) => drawCodebase(doc, ctx, event.payload));
  await tauri.event.listen("child-started", () => {
    // A question the last child had not answered never will be.
    ctx.pending = false;
    ctx.stale = true;
    if (shown(doc)) ask(doc, tauri, ctx);
  });
  await tauri.event.listen("no-child", () => {
    if (ctx.data === null) status(doc, "No repository is open: open a repository to see its graph.");
  });
  // A run's end may have changed the tree, so the graph is read again: now
  // if the view is showing, else the next time it is shown. A get_state
  // reply resends the same outcome, which changes nothing.
  await tauri.event.listen("view", (event) => {
    const o = event.payload && event.payload.outcome;
    if (o !== null && typeof o === "object" && JSON.stringify(o) !== ctx.lastOutcome) {
      ctx.lastOutcome = JSON.stringify(o);
      ctx.stale = true;
      if (shown(doc)) ask(doc, tauri, ctx);
    }
  });
  const button = doc.querySelector('nav.rail button[aria-controls="codebase"]');
  if (button) {
    button.addEventListener("click", () => {
      if (ctx.stale) ask(doc, tauri, ctx);
    });
  }
}

function start() {
  wireCodebase(document, window.__TAURI__).catch((err) => console.error("trellis desktop codebase:", err));
}

if (typeof window !== "undefined" && window.__TAURI__) {
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start);
  else start();
}
