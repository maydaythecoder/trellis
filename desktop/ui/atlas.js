// The Atlas (the owner's request of 2026-10-11: "the current view is nice in
// the trellis atlas, could you add that to the current view without removing
// the other ones"): a map of the repository's modules that stays put while
// the scrubber moves across its history.
//
// The desktop's core computes the map (desktop/src/atlas.rs): which
// snapshots to load, every region's and module's rectangle, each symbol's
// rectangle inside its module, and the figures per snapshot. It sends them
// as the view's `atlas` key. This module draws that on a canvas and owns
// only what is the page's: zoom and pan, the lens, which snapshot and
// baseline are shown, the selection, which labels fit at the zoom drawn
// (each kept only where no label of higher priority sits), and line widths
// summed for drawing at that zoom. Every figure it writes as text is one
// the core sent, or the length of a list it sent; bound, ambiguous and
// unresolved stay three counts and reach and doubt two blocks, and nothing
// here divides one by another (CLAUDE.md rule 3, ADR 0014).
//
// Module names, paths, symbol names, tag names and commit subjects are
// repository or git text (ADR 0007, S10): every element is made with
// createElement and every string written through textContent, or drawn on
// the canvas, which parses nothing. Colours are the stylesheet's custom
// properties, read at draw time and again when the system theme changes. A
// flight across the map is animated unless prefers-reduced-motion asks for
// none, when the map jumps.
//
// index.html loads this as a module of its own. It asks trellis for what it
// draws through `rpc_send`, one index at a time, when the section is first
// shown for a repository: the history, HEAD, HEAD's symbols, then each
// earlier snapshot the core planned, newest first.

import { el } from "./dom.js";
import { isCount, plural, str } from "./fmt.js";
import { send, start } from "./rpc.js";

const ID = "desktop-atlas-";
// get_codebase lists this many modules: all of any repository the layout
// draws (rpc's own default is 40).
const MODULES_ASKED = 100000;
// Every tag get_history gives, up to its cap: the core picks the snapshots
// from those dated in the two years before HEAD (atlas.rs).
const HISTORY_ASKED = { commits: 20, tags: 200 };
// The world the core's normalised layout is drawn in: its height, and its
// width by the layout's aspect.
const WORLD_H = 1000;
// A module opens to its symbols once it is this many pixels each way.
const OPEN_W = 190;
const OPEN_H = 130;
// A region's edges are drawn to its modules once its short side is this
// many pixels; until then they are summed onto the region.
const OPEN_REGION = 300;
// The most aggregated edges drawn with nothing selected.
const EDGE_CAP = 60;
// World units per square root of a call site, for the doubt lens's two
// squares: one scale for every module and snapshot.
const GLYPH = 2.3;
const FLIGHT_MS = 450;
const ZOOM_STEP = 1.5;
const PAN_PX = 70;
const DRAG_PX = 4;
const RESULTS_MAX = 40;
const PIN_KEY = "trellis-desktop.atlas.working-set.v1";

const SEL = 1;
const CALLER = 2;
const CALLEE = 4;
const REACH = 8;
const DOUBT = 16;
const COMMIT = 32;

const TOKENS = [
  "bg", "fg", "muted", "border", "pane-bg", "rail-bg", "accent", "region", "region-test",
  "region-head", "region-head-test", "cell", "cell-test", "cell-line", "edge", "caller", "callee",
  "reach", "doubt", "added", "removed", "churn-1", "churn-2", "churn-3", "churn-4", "churn-5",
];

const FONT = {
  top: "600 13px system-ui, sans-serif",
  dir: "600 12px system-ui, sans-serif",
  dirS: "600 11px system-ui, sans-serif",
  mod: "11px system-ui, sans-serif",
  modB: "600 12px system-ui, sans-serif",
  sub: "10.5px system-ui, sans-serif",
  badge: "600 11px system-ui, sans-serif",
  sym: "11px system-ui, sans-serif",
  symB: "600 11px system-ui, sans-serif",
  tip: "12px system-ui, sans-serif",
  tipB: "600 12px system-ui, sans-serif",
};

const CHURN_BINS = ["0", "1", "2", "3–4", "5–8", "9+"];
const WINDOWS = ["d30", "d90", "d365", "commits"];

// --- small helpers ---------------------------------------------------------
// el, isCount, plural and str are the page's shared ones (dom.js, fmt.js);
// these are the Atlas's own.

function list(v) {
  return Array.isArray(v) ? v : [];
}

function n(v) {
  return isCount(v) ? v.toLocaleString("en-US") : "not reported";
}

function day(s) {
  return typeof s === "string" ? s.slice(0, 10) : "date not reported";
}

function short(s) {
  return typeof s === "string" ? s.slice(0, 7) : "";
}

function byId(A, id) {
  return A.doc.getElementById(id);
}

// --- colours ---------------------------------------------------------------

// Every colour is one of style.css's custom properties, read here at draw
// time; atlas.js names none of its own.
function readPalette(A) {
  const cs = A.win.getComputedStyle(A.doc.documentElement);
  for (const t of TOKENS) A.P[t] = cs.getPropertyValue(`--${t}`).trim();
  A.hatches = new Map();
  A.inks = new Map();
}

function rgb(hex) {
  let h = String(hex).replace("#", "");
  if (h.length === 3) h = [...h].map((c) => c + c).join("");
  const v = Number.parseInt(h.slice(0, 6), 16);
  if (!Number.isFinite(v)) return [128, 128, 128];
  return [(v >> 16) & 255, (v >> 8) & 255, v & 255];
}

function rgba(hex, a) {
  const [r, g, b] = rgb(hex);
  return `rgba(${r},${g},${b},${a})`;
}

function lum(hex) {
  const c = rgb(hex).map((v) => {
    const x = v / 255;
    return x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
}

// Text on a fill takes whichever of the two text colours reads better on it.
function inkOn(A, fill) {
  let v = A.inks.get(fill);
  if (v) return v;
  const lf = lum(fill);
  const contrast = (a, b) => (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
  v = contrast(lf, lum(A.P.fg)) >= contrast(lf, lum(A.P.bg)) ? A.P.fg : A.P.bg;
  A.inks.set(fill, v);
  return v;
}

function hatch(A, color) {
  const had = A.hatches.get(color);
  if (had) return had;
  const s = Math.max(2, Math.round(6 * A.dpr));
  const c = A.doc.createElement("canvas");
  c.width = s;
  c.height = s;
  const x = c.getContext("2d");
  x.strokeStyle = color;
  x.lineWidth = 1.4 * A.dpr;
  x.lineCap = "square";
  x.beginPath();
  x.moveTo(0, s);
  x.lineTo(s, 0);
  x.moveTo(-s / 2, s / 2);
  x.lineTo(s / 2, -s / 2);
  x.moveTo(s / 2, s * 1.5);
  x.lineTo(s * 1.5, s / 2);
  x.stroke();
  const p = A.ctx.createPattern(c, "repeat");
  if (p && typeof p.setTransform === "function" && typeof A.win.DOMMatrix === "function") {
    p.setTransform(new A.win.DOMMatrix([1 / A.dpr, 0, 0, 1 / A.dpr, 0, 0]));
  }
  const fill = p || color;
  A.hatches.set(color, fill);
  return fill;
}

// --- the view, indexed -------------------------------------------------------

function rectOf(o, ww) {
  return { x0: o.x0 * ww, y0: o.y0 * WORLD_H, x1: o.x1 * ww, y1: o.y1 * WORLD_H };
}

// Lookups over what the core sent: each a table, nothing summed but the
// lists the core already gave.
function index(v) {
  const L = v && v.layout;
  if (!L || !Array.isArray(L.modules) || !Array.isArray(L.regions)) return null;
  const aspect = isCount(L.aspect) && L.aspect > 0 ? L.aspect : 1.4;
  const ww = aspect * WORLD_H;
  const regions = L.regions.map((r, i) => ({
    ...rectOf(r, ww),
    idx: i,
    path: str(r.path) ?? "",
    label: str(r.label) ?? "",
    depth: isCount(r.depth) ? r.depth : 0,
    parent: isCount(r.parent) ? r.parent : null,
    test: r.test === true,
    hdr: (isCount(r.header) ? r.header : 0) * WORLD_H,
    mods: [],
    kids: [],
  }));
  for (const r of regions) if (r.parent !== null && regions[r.parent]) regions[r.parent].kids.push(r.idx);
  const modules = L.modules.map((m, i) => ({
    ...rectOf(m, ww),
    idx: i,
    module: str(m.module) ?? "module",
    path: str(m.path) ?? "",
    label: str(m.label) ?? "",
    region: isCount(m.region) ? m.region : 0,
    test: m.test === true,
    weight: m.weight,
  }));
  const chain = modules.map((m) => {
    const c = [];
    for (let g = m.region; g !== null && regions[g]; g = regions[g].parent) {
      regions[g].mods.push(m.idx);
      if (regions[g].depth > 0) c.unshift(g);
    }
    return c;
  });
  const byPath = new Map(modules.map((m) => [m.path, m.idx]));
  const snaps = list(v.snapshots);
  const S = snaps.map((s) => {
    const rows = new Map();
    for (const r of list(s.rows)) if (Array.isArray(r) && isCount(r[0])) rows.set(r[0], r);
    const inn = new Map();
    const out = new Map();
    const keys = new Map();
    for (const e of list(s.edges)) {
      if (!Array.isArray(e) || !isCount(e[0]) || !isCount(e[1]) || !isCount(e[2])) continue;
      if (!out.has(e[0])) out.set(e[0], []);
      if (!inn.has(e[1])) inn.set(e[1], []);
      out.get(e[0]).push([e[1], e[2]]);
      inn.get(e[1]).push([e[0], e[2]]);
      keys.set(`${e[0]}:${e[1]}`, e);
    }
    return { s, rows, inn, out, keys, edges: list(s.edges) };
  });
  const head = snaps.length - 1;
  // The scrubber's points: every loaded snapshot once the layout is fixed,
  // HEAD alone before then, since the map may still move.
  const points = [];
  snaps.forEach((s, i) => {
    if (s.state === "loaded" && (L.fixed === true || i === head)) points.push(i);
  });
  const sy = v.symbols && Array.isArray(v.symbols.list) ? v.symbols.list : [];
  const symbols = sy.map((s, i) => ({
    ...rectOf(s, ww),
    idx: i,
    m: isCount(s.m) ? s.m : -1,
    name: str(s.name) ?? "",
    kind: str(s.kind) ?? "",
    line: s.line,
    end: s.end_line,
    cin: s.calls_in,
    camb: s.calls_in_ambiguous,
    cout: s.calls_out,
    parent: isCount(s.parent) ? s.parent : null,
    depth: isCount(s.depth) ? s.depth : 0,
    holds: s.holds === true,
    hdr: (isCount(s.header) ? s.header : 0) * WORLD_H,
  }));
  // A symbol's name inside its module, through the definitions that hold
  // it (`Solver.solve`): the core sends parents before what they hold.
  for (const s of symbols) {
    const p = s.parent !== null ? symbols[s.parent] : null;
    s.qual = p ? `${p.qual}.${s.name}` : s.name;
  }
  const symsOf = new Map();
  for (const s of symbols) {
    if (!symsOf.has(s.m)) symsOf.set(s.m, []);
    symsOf.get(s.m).push(s.idx);
  }
  const h = v.history && !str(v.history.error) ? v.history : null;
  const churn = new Map();
  for (const c of list(h && h.churn)) if (isCount(c.m)) churn.set(c.m, c);
  const commits = list(h && h.commits);
  const commitsOf = new Map();
  commits.forEach((c, ci) => {
    for (const m of list(c.modules)) {
      if (!commitsOf.has(m)) commitsOf.set(m, []);
      commitsOf.get(m).push(ci);
    }
  });
  return {
    ww,
    regions,
    modules,
    chain,
    byPath,
    S,
    head,
    points,
    symbols,
    symsOf,
    history: h,
    churn,
    commits,
    commitsOf,
    fixed: L.fixed === true,
  };
}

function snap(A, si) {
  return A.D && A.D.S[si] ? A.D.S[si] : null;
}

function rowAt(A, si, m) {
  const s = snap(A, si);
  return s ? s.rows.get(m) ?? null : null;
}

function present(A, si, m) {
  return rowAt(A, si, m) !== null;
}

function label(A, si) {
  const s = snap(A, si);
  return s ? str(s.s.label) ?? "snapshot" : "snapshot";
}

function isHead(A) {
  return A.D !== null && A.si === A.D.head;
}

// The query get_radius takes for a symbol: its module and its name.
function radiusQuery(A, i) {
  const s = A.D.symbols[i];
  const m = s && A.D.modules[s.m];
  return s && m ? `${m.module}.${s.name}` : null;
}

// --- asking trellis ----------------------------------------------------------

function shown(A) {
  const s = byId(A, "atlas");
  return s !== null && !s.hidden;
}

function status(A, text) {
  byId(A, "at-status").textContent = text;
}

// Sends one line through the core (rpc.js's send). A refusal drops the
// question in flight and says why in the Atlas's status line.
function forward(A, line) {
  return send(A.tauri, line, (why) => {
    A.inflight = null;
    status(A, `Open a repository to see its atlas. ${why}`);
  });
}

function nextId(A, kind) {
  A.asked += 1;
  return `${ID}${kind}-${A.asked}`;
}

// What to ask next: the history once, then one index at a time: HEAD, its
// symbols, then each snapshot the core planned, newest first. A question
// in flight, here or in the core's record, is waited for.
function pump(A) {
  if (!shown(A) || !A.childUp) return;
  const v = A.view;
  if (!A.askedHistory && (v === null || v.history === null)) {
    A.askedHistory = true;
    forward(A, { id: nextId(A, "history"), type: "get_history", ...HISTORY_ASKED });
  }
  if (A.inflight !== null) return;
  const snaps = v ? list(v.snapshots) : [];
  if (snaps.some((s) => s.state === "asked")) return;
  const head = snaps[snaps.length - 1];
  if (!head || head.state === "planned") {
    A.inflight = { kind: "snapshot", label: "HEAD" };
    status(A, progressText(A));
    forward(A, { id: nextId(A, "codebase"), type: "get_codebase", edges: "all", limit: MODULES_ASKED });
    return;
  }
  if (head.state === "loaded" && v.symbols === null) {
    if (A.askedSymbols) return;
    A.askedSymbols = true;
    A.inflight = { kind: "symbols" };
    status(A, progressText(A));
    forward(A, { id: nextId(A, "symbols"), type: "get_symbols" });
    return;
  }
  if (!v.progress || v.progress.planned !== true) return;
  for (let i = snaps.length - 2; i >= 0; i--) {
    const s = snaps[i];
    if (s.state === "planned" && str(s.ref) !== null) {
      A.inflight = { kind: "snapshot", label: s.label };
      status(A, progressText(A));
      forward(A, { id: nextId(A, "codebase"), type: "get_codebase", edges: "all", limit: MODULES_ASKED, ref: s.ref });
      return;
    }
  }
}

// The question in flight has been answered once the view says so.
function settle(A) {
  const f = A.inflight;
  if (f === null || A.view === null) return;
  if (f.kind === "symbols" && A.view.symbols !== null) A.inflight = null;
  if (f.kind === "snapshot") {
    const s = list(A.view.snapshots).find((x) => x.label === f.label);
    if (s && (s.state === "loaded" || s.state === "failed")) A.inflight = null;
  }
}

function progressText(A) {
  const v = A.view;
  if (!v) {
    return A.inflight || A.askedHistory
      ? "Reading the repository's history and HEAD: trellis indexes it."
      : "The atlas is read when this section is shown.";
  }
  const snaps = list(v.snapshots);
  const head = snaps[snaps.length - 1];
  const p = v.progress || {};
  const parts = [];
  if (head && head.state === "failed") parts.push(`HEAD could not be read: ${head.error}`);
  else if (!head || head.state !== "loaded") parts.push("Reading HEAD: trellis indexes the repository.");
  if (v.history === null) parts.push("Reading the history.");
  else if (str(v.history.error)) parts.push(`History not read, so HEAD alone is shown: ${v.history.error}`);
  const loading = list(p.loading).filter((l) => l !== "HEAD");
  const f = A.inflight;
  if (f && f.kind === "snapshot" && f.label !== "HEAD" && !loading.includes(f.label)) loading.push(f.label);
  if (f && f.kind === "symbols") parts.push("Reading HEAD's symbols.");
  if (loading.length > 0 && isCount(p.settled) && isCount(p.of)) {
    parts.push(`Loading ${loading.join(", ")} … ${p.settled + 1} of ${p.of}.`);
  } else if (isCount(p.of) && p.of > 0 && p.settled < p.of) {
    parts.push(`Snapshots loaded: ${p.settled} of ${p.of}.`);
  }
  for (const s of snaps) {
    if (s.state === "failed" && s.label !== "HEAD") parts.push(`${s.label} could not be read: ${s.error}`);
  }
  if (v.layout && v.layout.fixed !== true && head && head.state === "loaded") {
    parts.push("The map is HEAD's until every snapshot has settled, then laid out once over all of them.");
  }
  if (v.symbols && str(v.symbols.error)) parts.push(`Symbols not read: ${v.symbols.error}`);
  return parts.length > 0 ? parts.join(" ") : "Every snapshot is loaded. The map stays put as the snapshot moves.";
}

// --- zoom and pan ------------------------------------------------------------

function fitT(A) {
  const pad = 10;
  const ww = A.D ? A.D.ww : 1400;
  const k = Math.max(1e-6, Math.min((A.vw - 2 * pad) / ww, (A.vh - 2 * pad) / WORLD_H));
  return { k, x: (A.vw - ww * k) / 2, y: Math.max(pad, (A.vh - WORLD_H * k) / 2) };
}

function kRange(A) {
  const f = fitT(A).k;
  return [f * 0.5, Math.max(f * 160, 60)];
}

function clampT(A, t) {
  const [lo, hi] = kRange(A);
  const k = Math.min(hi, Math.max(lo, t.k));
  const ww = A.D ? A.D.ww : 1400;
  // The world point at the canvas's centre stays near the map.
  let cx = (A.vw / 2 - t.x) / t.k;
  let cy = (A.vh / 2 - t.y) / t.k;
  cx = Math.min(1.4 * ww, Math.max(-0.4 * ww, cx));
  cy = Math.min(1.4 * WORLD_H, Math.max(-0.4 * WORLD_H, cy));
  return { k, x: A.vw / 2 - cx * k, y: A.vh / 2 - cy * k };
}

function setT(A, t) {
  A.t = clampT(A, t);
  redraw(A);
  crumbs(A);
}

function toward(A, x0, y0, x1, y1, fill, minK) {
  let k = Math.min((A.vw * fill) / Math.max(1e-6, x1 - x0), (A.vh * fill) / Math.max(1e-6, y1 - y0));
  if (minK) k = Math.max(k, minK);
  const [lo, hi] = kRange(A);
  k = Math.max(lo, Math.min(hi, k));
  return { k, x: A.vw / 2 - ((x0 + x1) / 2) * k, y: A.vh / 2 - ((y0 + y1) / 2) * k };
}

// Moves the view to `t`: at once under prefers-reduced-motion, or in a
// short flight that keeps the point under the centre moving smoothly.
function go(A, t) {
  t = clampT(A, t);
  if (A.reduced.matches || typeof A.win.requestAnimationFrame !== "function") {
    A.flight = null;
    setT(A, t);
    return;
  }
  const from = A.t;
  const c0 = [(A.vw / 2 - from.x) / from.k, (A.vh / 2 - from.y) / from.k];
  const c1 = [(A.vw / 2 - t.x) / t.k, (A.vh / 2 - t.y) / t.k];
  const flight = { start: null };
  A.flight = flight;
  const step = (now) => {
    if (A.flight !== flight) return;
    if (flight.start === null) flight.start = now;
    const p = Math.min(1, (now - flight.start) / FLIGHT_MS);
    const e = p < 0.5 ? 2 * p * p : 1 - (-2 * p + 2) ** 2 / 2;
    const k = Math.exp(Math.log(from.k) + (Math.log(t.k) - Math.log(from.k)) * e);
    const cx = c0[0] + (c1[0] - c0[0]) * e;
    const cy = c0[1] + (c1[1] - c0[1]) * e;
    A.t = { k, x: A.vw / 2 - cx * k, y: A.vh / 2 - cy * k };
    redraw(A);
    if (p < 1) A.win.requestAnimationFrame(step);
    else {
      A.flight = null;
      crumbs(A);
    }
  };
  A.win.requestAnimationFrame(step);
}

function zoomAbout(A, f, px, py) {
  const t = A.t;
  const [lo, hi] = kRange(A);
  const k = Math.max(lo, Math.min(hi, t.k * f));
  const wx = (px - t.x) / t.k;
  const wy = (py - t.y) / t.k;
  A.flight = null;
  setT(A, { k, x: px - wx * k, y: py - wy * k });
}

function panBy(A, dx, dy) {
  A.flight = null;
  setT(A, { k: A.t.k, x: A.t.x + dx, y: A.t.y + dy });
}

function scr(A, r) {
  const t = A.t;
  return [r.x0 * t.k + t.x, r.y0 * t.k + t.y, r.x1 * t.k + t.x, r.y1 * t.k + t.y];
}

function onScreen(A, r) {
  return r[2] >= 0 && r[0] <= A.vw && r[3] >= 0 && r[1] <= A.vh;
}

function openK(m) {
  return Math.max(OPEN_W / (m.x1 - m.x0), OPEN_H / (m.y1 - m.y0)) * 1.05;
}

// --- what is selected, and what it marks -------------------------------------

function focus(A) {
  const s = A.sel;
  if (s === null || A.D === null) return null;
  const F = { role: new Map(), edges: [], selMod: -1, region: null, sym: -1, radius: null, commit: -1, reachSyms: null };
  const add = (m, f) => F.role.set(m, (F.role.get(m) || 0) | f);
  const S = snap(A, A.si);
  const r = A.view && A.view.radius;
  const answered = r && r.state === "answered" && r.answer;
  if (s.t === "symbol" && isHead(A) && answered && r.symbol === radiusQuery(A, s.i)) {
    F.sym = s.i;
    F.radius = r.answer;
    F.selMod = A.D.symbols[s.i].m;
    add(F.selMod, SEL);
    for (const m of list(r.answer.reached_modules)) add(m, REACH);
    for (const m of list(r.answer.touched_modules)) add(m, DOUBT);
    F.reachSyms = new Set(list(r.answer.reached).map((x) => x.s).filter(isCount));
  } else if (s.t === "module" || s.t === "symbol") {
    const m = s.t === "module" ? s.m : A.D.symbols[s.i].m;
    F.selMod = m;
    if (s.t === "symbol") F.sym = s.i;
    add(m, SEL);
    if (S) {
      for (const [a, w] of S.inn.get(m) || []) {
        add(a, CALLER);
        F.edges.push([a, m, w, CALLER]);
      }
      for (const [b, w] of S.out.get(m) || []) {
        add(b, CALLEE);
        F.edges.push([m, b, w, CALLEE]);
      }
    }
  } else if (s.t === "region") {
    F.region = s.r;
  } else if (s.t === "commit") {
    F.commit = s.c;
    const c = A.D.commits[s.c];
    for (const m of list(c && c.modules)) add(m, COMMIT);
  }
  return F;
}

function churnCount(A, m) {
  const c = A.D.churn.get(m);
  // get_history lists every path a commit changed in the last 365 days or
  // since the oldest tag it lists (its contract), so a path it does not
  // list had none in either; a count it sent as null stays unknown, as
  // does "since" with no tag listed.
  if (c === undefined) return A.span < 3 || (A.D.history && A.D.history.since) ? 0 : null;
  const v = c[WINDOWS[A.span]];
  return isCount(v) ? v : null;
}

function churnBin(v) {
  if (!isCount(v) || v <= 0) return 0;
  if (v === 1) return 1;
  if (v === 2) return 2;
  if (v <= 4) return 3;
  if (v <= 8) return 4;
  return 5;
}

// The diff between the shown snapshot and the baseline: modules and edges
// in one and not the other, and modules whose counts differ. Lists of what
// the core sent, compared.
function diff(A) {
  const t = A.si;
  const b = A.base;
  const key = `${t}:${b}`;
  if (A.diffCache && A.diffCache.key === key) return A.diffCache;
  const T = snap(A, t);
  const B = snap(A, b);
  const D = { key, t, b, added: new Set(), removed: new Set(), changed: [], addedE: [], removedE: [] };
  if (T && B) {
    for (const m of T.rows.keys()) if (!B.rows.has(m)) D.added.add(m);
    for (const m of B.rows.keys()) if (!T.rows.has(m)) D.removed.add(m);
    for (const [m, r] of T.rows) {
      const o = B.rows.get(m);
      if (o && (o[1] !== r[1] || o[2] !== r[2] || o[3] !== r[3])) D.changed.push(m);
    }
    for (const [k, e] of T.keys) if (!B.keys.has(k)) D.addedE.push(e);
    for (const [k, e] of B.keys) if (!T.keys.has(k)) D.removedE.push(e);
    D.addedE.sort((x, y) => y[2] - x[2]);
    D.removedE.sort((x, y) => y[2] - x[2]);
  }
  A.diffCache = D;
  return D;
}

// --- drawing -----------------------------------------------------------------

function measure(A, font, t) {
  const key = `${font}|${t}`;
  let v = A.widths.get(key);
  if (v === undefined) {
    A.ctx.font = font;
    v = A.ctx.measureText(t).width;
    if (A.widths.size > 30000) A.widths.clear();
    A.widths.set(key, v);
  }
  return v;
}

function ellipsize(A, font, t, maxW) {
  if (measure(A, font, t) <= maxW) return t;
  let lo = 0;
  let hi = t.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (measure(A, font, `${t.slice(0, mid)}…`) <= maxW) lo = mid;
    else hi = mid - 1;
  }
  return lo >= 3 ? `${t.slice(0, lo)}…` : null;
}

// Draws the labels queued this frame, highest priority first, each only
// where no label already drawn sits, so no two overlap.
function placeLabels(A) {
  const ctx = A.ctx;
  A.labels.sort((a, b) => a.pri - b.pri);
  const taken = [];
  A.drawn = [];
  ctx.textBaseline = "middle";
  for (const L of A.labels) {
    if (L.needs && !L.needs.placed) continue;
    if (L.rx !== undefined && L.x < 4) {
      L.maxW = Math.min(L.maxW === undefined ? 1e9 : L.maxW, L.rx - 8);
      L.x = 4;
    }
    if (L.ry !== undefined && L.y < L.size / 2 + 3 + (L.dy || 0)) {
      const y = L.size / 2 + 3 + (L.dy || 0);
      if (y + L.size / 2 + 2 > L.ry) continue;
      L.y = y;
    }
    let text = L.text;
    if (L.maxW !== undefined) {
      if (L.maxW < 14) continue;
      text = ellipsize(A, L.font, text, L.maxW);
      if (!text) continue;
    }
    const tw = measure(A, L.font, text);
    const px = L.pill ? 4 : 1;
    const x0 = L.align === "center" ? L.x - tw / 2 : L.x;
    const box = [x0 - px, L.y - L.size / 2 - 1, x0 + tw + px, L.y + L.size / 2 + 1];
    if (box[2] < 0 || box[0] > A.vw || box[3] < 0 || box[1] > A.vh) continue;
    if (taken.some((o) => box[0] < o[2] && o[0] < box[2] && box[1] < o[3] && o[1] < box[3])) continue;
    taken.push(box);
    L.placed = true;
    A.drawn.push({ text, box, needs: L.needs ? L.needs.text : null });
    ctx.globalAlpha = L.alpha === undefined ? 1 : L.alpha;
    ctx.font = L.font;
    if (L.pill) {
      ctx.fillStyle = A.P["pane-bg"];
      ctx.strokeStyle = L.border || A.P.border;
      ctx.lineWidth = 1.25;
      ctx.beginPath();
      ctx.rect(box[0], box[1], box[2] - box[0], box[3] - box[1]);
      ctx.fill();
      ctx.stroke();
    } else if (L.halo) {
      ctx.strokeStyle = L.halo;
      ctx.lineWidth = 3;
      ctx.lineJoin = "round";
      ctx.strokeText(text, x0, L.y);
    }
    ctx.fillStyle = L.color;
    ctx.fillText(text, x0, L.y);
  }
  ctx.globalAlpha = 1;
}

function redraw(A) {
  if (A.queued) return;
  A.queued = true;
  const run = () => {
    A.queued = false;
    draw(A);
  };
  if (typeof A.win.requestAnimationFrame === "function") A.win.requestAnimationFrame(run);
  else run();
}

function draw(A) {
  const ctx = A.ctx;
  if (!ctx || A.vw <= 1 || A.vh <= 1) return;
  ctx.setTransform(A.dpr, 0, 0, A.dpr, 0, 0);
  ctx.fillStyle = A.P.bg;
  ctx.fillRect(0, 0, A.vw, A.vh);
  A.labels = [];
  if (A.D === null) {
    A.drawn = [];
    drawMinimap(A);
    return;
  }
  const F = A.F;
  const D = A.lens === "diff" ? diff(A) : null;
  drawRegions(A, F);
  const glyphs = [];
  for (const m of A.D.modules) {
    const r = scr(A, m);
    if (!onScreen(A, r)) continue;
    const w = r[2] - r[0];
    const h = r[3] - r[1];
    if (w < 0.5 || h < 0.5) continue;
    drawModule(A, m, r, w, h, F, D, glyphs);
  }
  drawEdges(A, F, D);
  if (A.lens === "doubt") for (const g of glyphs) drawGlyphs(A, ...g);
  placeLabels(A);
  if (A.hover && A.tipAt) drawTip(A);
  drawMinimap(A);
  const note = byId(A, "at-lg-edges");
  if (note && note.textContent !== A.edgeNote) note.textContent = A.edgeNote;
}

function regionLabel(A, g) {
  const S = snap(A, A.si);
  const counts = S ? list(S.s.regions)[g.idx] : null;
  const held = Array.isArray(counts) && isCount(counts[0]) ? counts[0] : null;
  const kind = g.test ? "test code" : "source";
  return held === null ? `${g.label} · ${kind}` : `${g.label} · ${kind} · ${plural(held, "module", "modules")}`;
}

function drawRegions(A, F) {
  const ctx = A.ctx;
  const P = A.P;
  const k = A.t.k;
  for (const g of A.D.regions) {
    if (g.depth === 0) continue;
    const r = scr(A, g);
    if (!onScreen(A, r)) continue;
    const w = r[2] - r[0];
    const h = r[3] - r[1];
    if (w < 2 || h < 2) continue;
    ctx.fillStyle = g.test ? P["region-test"] : P.region;
    ctx.fillRect(r[0], r[1], w, h);
    const hh = g.hdr * k;
    if (hh >= 1.5) {
      ctx.fillStyle = g.test ? P["region-head-test"] : P["region-head"];
      ctx.fillRect(r[0], r[1], w, hh);
    }
    if (w > 5 && h > 5) {
      ctx.strokeStyle = P.border;
      ctx.lineWidth = 1;
      ctx.strokeRect(r[0] + 0.5, r[1] + 0.5, w - 1, h - 1);
    }
    if (F && F.region === g.idx) {
      ctx.strokeStyle = P.fg;
      ctx.lineWidth = 2.5;
      ctx.strokeRect(r[0] + 1, r[1] + 1, w - 2, h - 2);
    }
    if (A.hover && A.hover.t === "region" && A.hover.r === g.idx) {
      ctx.strokeStyle = P.fg;
      ctx.lineWidth = 1.5;
      ctx.strokeRect(r[0] + 0.75, r[1] + 0.75, w - 1.5, hh);
    }
    if (g.depth === 1) {
      A.labels.push({
        pri: 0,
        x: r[0] + 6,
        y: r[1] + Math.max(hh, 16) / 2,
        text: regionLabel(A, g),
        font: FONT.top,
        size: 13,
        color: P.fg,
        maxW: w - 12,
        rx: r[2],
        ry: r[3],
        halo: g.test ? P["region-head-test"] : P["region-head"],
      });
    } else if (hh >= 10) {
      const big = hh >= 15;
      A.labels.push({
        pri: 2 + g.depth * 0.1,
        x: r[0] + 4,
        y: r[1] + hh / 2,
        text: g.label,
        font: big ? FONT.dir : FONT.dirS,
        size: big ? 12 : 11,
        color: P.fg,
        maxW: w - 8,
        rx: r[2],
      });
    }
  }
}

function cellFill(A, m) {
  return m.test ? A.P["cell-test"] : A.P.cell;
}

function moduleFill(A, m) {
  if (A.lens === "churn") {
    const b = churnBin(churnCount(A, m.idx));
    return b > 0 ? A.P[`churn-${b}`] : cellFill(A, m);
  }
  return cellFill(A, m);
}

// The line under a module's name at middle zoom: what the lens shows.
function lensLine(A, m) {
  const r = rowAt(A, A.si, m.idx);
  if (!r) return null;
  if (A.lens === "churn") {
    const c = churnCount(A, m.idx);
    return isCount(c) ? plural(c, "commit", "commits") : "commits not reported";
  }
  if (A.lens === "doubt") return `${n(r[2])} amb · ${n(r[3])} unres`;
  if (A.lens === "diff") {
    const D = diff(A);
    if (D.added.has(m.idx)) return "added";
    const o = rowAt(A, A.base, m.idx);
    if (o && o[1] !== r[1]) return `defs ${n(o[1])} → ${n(r[1])}`;
  }
  return `${n(r[1])} defs`;
}

function ring(A, r, w, h, color, lw, inset) {
  const ctx = A.ctx;
  ctx.strokeStyle = color;
  ctx.lineWidth = lw;
  ctx.strokeRect(r[0] + inset, r[1] + inset, Math.max(0, w - 2 * inset), Math.max(0, h - 2 * inset));
}

function drawModule(A, m, r, w, h, F, D, glyphs) {
  const ctx = A.ctx;
  const P = A.P;
  const pr = present(A, A.si, m.idx);
  const role = F ? F.role.get(m.idx) || 0 : 0;
  const dim = F !== null && role === 0 && F.region === null;
  const fill = moduleFill(A, m);
  ctx.globalAlpha = dim ? 0.28 : 1;
  if (pr) {
    ctx.fillStyle = fill;
    ctx.fillRect(r[0], r[1], w, h);
    if (w >= 3 && h >= 3) {
      ctx.strokeStyle = P["cell-line"];
      ctx.lineWidth = 1;
      ctx.strokeRect(r[0] + 0.5, r[1] + 0.5, w - 1, h - 1);
    }
  } else if (w >= 2 && h >= 2) {
    // A ghost: in the layout, not in this snapshot.
    const gone = D !== null && D.removed.has(m.idx);
    ctx.globalAlpha = dim ? 0.25 : gone ? 1 : 0.6;
    ctx.setLineDash(gone ? [4, 2] : [3, 3]);
    ctx.strokeStyle = gone ? P.removed : P.muted;
    ctx.lineWidth = gone ? 1.5 : 1;
    ctx.strokeRect(r[0] + 0.75, r[1] + 0.75, w - 1.5, h - 1.5);
    ctx.setLineDash([]);
  }
  ctx.globalAlpha = 1;
  if (D !== null && D.added.has(m.idx) && pr) {
    ctx.fillStyle = rgba(P.added, 0.3);
    ctx.fillRect(r[0], r[1], w, h);
    ring(A, r, w, h, P.added, 1.5, 0.75);
  }
  const open = pr && isHead(A) && w >= OPEN_W && h >= OPEN_H;
  if (role & REACH) {
    ctx.fillStyle = rgba(P.reach, 0.3);
    ctx.fillRect(r[0], r[1], w, h);
  }
  if (role & DOUBT && !open) {
    ctx.fillStyle = hatch(A, P.doubt);
    ctx.fillRect(r[0], r[1], w, h);
  }
  if (role & COMMIT) {
    ctx.fillStyle = rgba(P.accent, 0.22);
    ctx.fillRect(r[0], r[1], w, h);
  }
  if (role & CALLER && A.lens === "structure") {
    ctx.fillStyle = rgba(P.caller, 0.16);
    ctx.fillRect(r[0], r[1], w, h);
  }
  if (role & CALLEE && A.lens === "structure") {
    ctx.fillStyle = rgba(P.callee, 0.16);
    ctx.fillRect(r[0], r[1], w, h);
  }
  if (open) drawSymbols(A, m, F);
  if (role & REACH) ring(A, r, w, h, P.reach, 1.5, 0.75);
  if (role & DOUBT) {
    if (open) {
      ctx.strokeStyle = hatch(A, P.doubt);
      ctx.lineWidth = 8;
      ctx.strokeRect(r[0] + 4, r[1] + 4, w - 8, h - 8);
    }
    ring(A, r, w, h, P.doubt, 1, 0.5);
  }
  if (role & CALLER) ring(A, r, w, h, P.caller, 2.25, 1.1);
  if (role & CALLEE) ring(A, r, w, h, P.callee, 2.25, role & CALLER ? 3.4 : 1.1);
  if (role & COMMIT) ring(A, r, w, h, P.accent, 2.5, 1.25);
  if (role & SEL) ring(A, r, w, h, P.fg, 2.75, 1.4);
  if (A.hover && A.hover.t === "module" && A.hover.m === m.idx) ring(A, r, w, h, P.fg, 1.5, 0.75);
  if (A.pins.has(m.path) && w >= 8 && h >= 8) {
    const s = Math.min(9, w * 0.3, h * 0.3);
    ctx.fillStyle = P.accent;
    ctx.beginPath();
    ctx.moveTo(r[2] - s, r[1]);
    ctx.lineTo(r[2], r[1]);
    ctx.lineTo(r[2], r[1] + s);
    ctx.closePath();
    ctx.fill();
  }
  if (D !== null && w >= 9 && h >= 9) {
    const mark = D.added.has(m.idx) ? "+" : D.removed.has(m.idx) ? "−" : null;
    if (mark) {
      A.labels.push({
        pri: 1.5,
        x: r[2] - 3 - measure(A, FONT.badge, mark),
        y: r[3] - 7,
        text: mark,
        font: FONT.badge,
        size: 11,
        color: mark === "+" ? P.added : P.removed,
      });
    }
  }
  if (A.lens === "doubt" && pr && !open) {
    const row = rowAt(A, A.si, m.idx);
    if ((isCount(row[2]) && row[2] > 0) || (isCount(row[3]) && row[3] > 0)) glyphs.push([r, row, dim]);
  }
  const a = dim ? 0.4 : 1;
  const ink = pr ? inkOn(A, fill) : P.muted;
  if (open) {
    const syms = A.D.symsOf.get(m.idx) || [];
    const hh = Math.min(m.y1 - m.y0, m.x1 - m.x0) * 0.1 * A.t.k;
    const name = {
      pri: role & SEL ? 1 : 3,
      x: r[0] + 5,
      y: r[1] + Math.min(hh, 22) / 2 + 1,
      text: m.module,
      font: FONT.modB,
      size: 12,
      color: P.fg,
      halo: fill,
      maxW: w - 10,
      alpha: a,
      rx: r[2],
      ry: r[3],
    };
    A.labels.push(name);
    if (syms.length === 0) {
      const why = A.view && A.view.symbols === null ? "symbols not read yet" : "no definitions";
      A.labels.push({ needs: name, pri: 6, x: r[0] + 5, y: r[1] + hh + 10, text: why, font: FONT.sub, size: 11, color: P.muted, maxW: w - 10 });
    }
    return;
  }
  if (w >= 24 && h >= 13) {
    const pri = role & SEL ? 1 : 4 + 1 / (1 + w * h);
    const name = {
      pri,
      x: r[0] + 3,
      y: r[1] + Math.min(8, h / 2),
      text: m.label,
      font: role & SEL ? FONT.modB : FONT.mod,
      size: 11,
      color: ink,
      halo: pr ? fill : undefined,
      maxW: w - 6,
      alpha: a,
      rx: r[2],
      ry: r[3],
    };
    A.labels.push(name);
    if (h >= 30 && w >= 46 && pr) {
      const t = lensLine(A, m);
      if (t) {
        // A count is drawn only under its module's name: where the name
        // does not fit, neither does the count.
        A.labels.push({
          needs: name,
          pri: pri + 1,
          x: r[0] + 3,
          y: r[1] + 21,
          text: t,
          font: FONT.sub,
          size: 11,
          color: ink,
          halo: fill,
          maxW: w - 6,
          alpha: a * 0.8,
          rx: r[2],
          ry: r[3],
          dy: 13,
        });
      }
    }
  }
}

function drawSymbols(A, m, F) {
  const ctx = A.ctx;
  const P = A.P;
  const k = A.t.k;
  const rq = A.view && A.view.radius && A.view.radius.state === "answered" ? A.view.radius.symbol : null;
  for (const i of A.D.symsOf.get(m.idx) || []) {
    const s = A.D.symbols[i];
    const r = scr(A, s);
    const w = r[2] - r[0];
    const h = r[3] - r[1];
    if (w < 1.2 || h < 1.2 || !onScreen(A, r)) continue;
    ctx.fillStyle = s.holds ? (m.test ? P["region-test"] : P.region) : cellFill(A, m);
    ctx.fillRect(r[0], r[1], w, h);
    if (s.holds) {
      ctx.fillStyle = m.test ? P["region-head-test"] : P["region-head"];
      ctx.fillRect(r[0], r[1], w, s.hdr * k);
    }
    if (w >= 3 && h >= 3) {
      ctx.strokeStyle = P["cell-line"];
      ctx.lineWidth = 1;
      ctx.strokeRect(r[0] + 0.5, r[1] + 0.5, w - 1, h - 1);
    }
    if (F && F.reachSyms && F.reachSyms.has(i)) {
      ctx.fillStyle = rgba(P.reach, 0.32);
      ctx.fillRect(r[0], r[1], w, s.holds ? s.hdr * k : h);
      ring(A, r, w, h, P.reach, 1.5, 0.75);
    }
    if (F && F.sym === i) ring(A, r, w, h, P.fg, 2.5, 1.25);
    if (A.hover && A.hover.t === "symbol" && A.hover.i === i) ring(A, r, w, h, P.fg, 1.5, 0.75);
    const hasRadius = rq !== null && rq === radiusQuery(A, i);
    if (hasRadius && w >= 14 && h >= 10) {
      ctx.fillStyle = P.accent;
      ctx.beginPath();
      ctx.arc(r[2] - 5, r[1] + 5, 2.6, 0, Math.PI * 2);
      ctx.fill();
    }
    const lh = s.holds ? s.hdr * k : h;
    if (w >= 22 && lh >= 11) {
      const fill = s.holds ? (m.test ? P["region-head-test"] : P["region-head"]) : cellFill(A, m);
      const pri = F && F.sym === i ? 1 : 5 + s.depth * 0.1 + 1 / (1 + w * h);
      // Named by its kind: a function that holds nested definitions is a
      // box on the map, and still a function.
      const name = {
        pri,
        x: r[0] + 3,
        y: r[1] + Math.min(lh, 16) / 2 + 0.5,
        text: s.kind === "class" ? `class ${s.name}` : `${s.name}()`,
        font: s.holds || (F && F.sym === i) ? FONT.symB : FONT.sym,
        size: 11,
        color: P.fg,
        halo: fill,
        maxW: w - (hasRadius ? 14 : 6),
      };
      A.labels.push(name);
      if (!s.holds && h >= 28 && w >= 64) {
        A.labels.push({
          needs: name,
          pri: pri + 1,
          x: r[0] + 3,
          y: r[1] + 22.5,
          text: `in ${n(s.cin)} · amb ${n(s.camb)} · out ${n(s.cout)}`,
          font: FONT.sub,
          size: 11,
          color: P.muted,
          halo: fill,
          maxW: w - 6,
        });
      }
    }
  }
}

// The doubt lens: two squares per module, ambiguous filled and unresolved
// hatched, each with its area by its own count. Two counts, never a share.
function drawGlyphs(A, r, row, dim) {
  const ctx = A.ctx;
  const k = A.t.k;
  const a = isCount(row[2]) ? row[2] : 0;
  const u = isCount(row[3]) ? row[3] : 0;
  const sa = a ? Math.max(1.5, Math.sqrt(a) * GLYPH * k) : 0;
  const su = u ? Math.max(1.5, Math.sqrt(u) * GLYPH * k) : 0;
  const pad = Math.min(3, (r[2] - r[0]) * 0.08);
  ctx.save();
  ctx.beginPath();
  ctx.rect(r[0], r[1], r[2] - r[0], r[3] - r[1]);
  ctx.clip();
  ctx.globalAlpha = dim ? 0.3 : 1;
  let x = r[2] - pad;
  const yb = r[3] - pad;
  if (su) {
    ctx.fillStyle = A.P["pane-bg"];
    ctx.fillRect(x - su, yb - su, su, su);
    ctx.fillStyle = hatch(A, A.P.doubt);
    ctx.fillRect(x - su, yb - su, su, su);
    ctx.strokeStyle = A.P.doubt;
    ctx.lineWidth = 1;
    ctx.strokeRect(x - su + 0.5, yb - su + 0.5, su - 1, su - 1);
    x -= su + 2;
  }
  if (sa) {
    ctx.fillStyle = A.P.doubt;
    ctx.fillRect(x - sa, yb - sa, sa, sa);
  }
  ctx.restore();
}

// Where the segment from `r`'s centre towards (tx, ty) leaves `r`.
function clipTo(cx, cy, tx, ty, r) {
  const dx = tx - cx;
  const dy = ty - cy;
  const hw = (r[2] - r[0]) / 2;
  const hh = (r[3] - r[1]) / 2;
  let t = 1;
  if (dx) t = Math.min(t, hw / Math.abs(dx));
  if (dy) t = Math.min(t, hh / Math.abs(dy));
  return [cx + dx * t, cy + dy * t];
}

function edge(A, ra, rb, width, color, alpha, dash) {
  const ctx = A.ctx;
  const ac = [(ra[0] + ra[2]) / 2, (ra[1] + ra[3]) / 2];
  const bc = [(rb[0] + rb[2]) / 2, (rb[1] + rb[3]) / 2];
  if (Math.max(ac[0], bc[0]) < -50 || Math.min(ac[0], bc[0]) > A.vw + 50) return null;
  if (Math.max(ac[1], bc[1]) < -50 || Math.min(ac[1], bc[1]) > A.vh + 50) return null;
  const s = clipTo(ac[0], ac[1], bc[0], bc[1], ra);
  const e = clipTo(bc[0], bc[1], ac[0], ac[1], rb);
  const dx = e[0] - s[0];
  const dy = e[1] - s[1];
  const len = Math.hypot(dx, dy);
  if (len < 3) return null;
  const off = Math.min(len * 0.16, 70);
  const cx = (s[0] + e[0]) / 2 - (dy / len) * off;
  const cy = (s[1] + e[1]) / 2 + (dx / len) * off;
  let ux = e[0] - cx;
  let uy = e[1] - cy;
  const ul = Math.hypot(ux, uy) || 1;
  ux /= ul;
  uy /= ul;
  const ah = len > 16 ? Math.min(4 + width * 1.3, 13) : 0;
  ctx.globalAlpha = alpha;
  ctx.strokeStyle = color;
  ctx.lineWidth = width;
  ctx.lineCap = "round";
  if (dash) ctx.setLineDash(dash);
  ctx.beginPath();
  ctx.moveTo(s[0], s[1]);
  ctx.quadraticCurveTo(cx, cy, e[0] - ux * ah * 0.7, e[1] - uy * ah * 0.7);
  ctx.stroke();
  if (dash) ctx.setLineDash([]);
  if (ah) {
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.moveTo(e[0], e[1]);
    ctx.lineTo(e[0] - ux * ah - uy * ah * 0.55, e[1] - uy * ah + ux * ah * 0.55);
    ctx.lineTo(e[0] - ux * ah + uy * ah * 0.55, e[1] - uy * ah - ux * ah * 0.55);
    ctx.closePath();
    ctx.fill();
  }
  ctx.globalAlpha = 1;
  return (t) => [
    (1 - t) * (1 - t) * s[0] + 2 * (1 - t) * t * cx + t * t * e[0],
    (1 - t) * (1 - t) * s[1] + 2 * (1 - t) * t * cy + t * t * e[1],
  ];
}

function ewidth(w, max) {
  return Math.min(max, 0.8 + 0.42 * Math.sqrt(w));
}

// What stands for module `m` at this zoom: the outermost region around it
// too small on screen to show its own modules' edges, or the module.
function repOf(A, m) {
  const k = A.t.k;
  for (const g of A.D.chain[m]) {
    const r = A.D.regions[g];
    if (Math.min(r.x1 - r.x0, r.y1 - r.y0) * k < OPEN_REGION) return r;
  }
  return A.D.modules[m];
}

// Edges summed onto what stands for their ends at this zoom, for drawing.
function aggregate(A, edges) {
  const agg = new Map();
  for (const [a, b, w] of edges) {
    const ra = repOf(A, a);
    const rb = repOf(A, b);
    if (ra === rb) continue;
    const key = `${ra.kids ? "r" : "m"}${ra.idx}:${rb.kids ? "r" : "m"}${rb.idx}`;
    const v = agg.get(key);
    if (v) v[2] += w;
    else agg.set(key, [ra, rb, w]);
  }
  return [...agg.values()].sort((x, y) => x[2] - y[2]);
}

function drawEdges(A, F, D) {
  const P = A.P;
  const here = label(A, A.si);
  if (F && (F.radius || F.commit >= 0)) {
    A.edgeNote = F.radius ? "Edges hidden while a radius is shown." : "Edges hidden while a commit is shown.";
    return;
  }
  if (F && F.selMod >= 0) {
    const selR = scr(A, A.D.modules[F.selMod]);
    const edges = F.edges.slice().sort((a, b) => a[2] - b[2]);
    for (const [a, b, w, kind] of edges) {
      const ra = a === F.selMod ? selR : scr(A, A.D.modules[a]);
      const rb = b === F.selMod ? selR : scr(A, A.D.modules[b]);
      const col = kind === CALLER ? P.caller : P.callee;
      const q = edge(A, ra, rb, ewidth(w, 9), col, 0.9, null);
      if (q) {
        const p = kind === CALLER ? q(0.18) : q(0.82);
        A.labels.push({
          pri: 1.2 + 0.3 / (1 + w),
          x: p[0],
          y: p[1],
          text: w.toLocaleString("en-US"),
          font: FONT.badge,
          size: 12,
          color: P.fg,
          pill: true,
          border: col,
          align: "center",
        });
      }
    }
    const callers = F.edges.filter((e) => e[3] === CALLER).length;
    A.edgeNote = `${callers} calling and ${F.edges.length - callers} called modules at ${here}; pills give call sites.`;
    return;
  }
  if (D !== null) {
    for (const [ra, rb, w] of aggregate(A, D.removedE)) edge(A, scr(A, ra), scr(A, rb), ewidth(w, 5), P.removed, 0.8, [5, 3]);
    for (const [ra, rb, w] of aggregate(A, D.addedE)) edge(A, scr(A, ra), scr(A, rb), ewidth(w, 5), P.added, 0.8, null);
    A.edgeNote = `${D.addedE.length} module edges added and ${D.removedE.length} removed, ${label(A, D.b)} to ${label(A, D.t)}, merged into the packages open at this zoom.`;
    return;
  }
  if (A.lens !== "structure") {
    A.edgeNote = "Edges are drawn in the Structure lens, or for a selection.";
    return;
  }
  const S = snap(A, A.si);
  if (!S) {
    A.edgeNote = "";
    return;
  }
  const all = aggregate(A, S.edges);
  const shownEdges = all.slice(Math.max(0, all.length - EDGE_CAP));
  for (const [ra, rb, w] of shownEdges) edge(A, scr(A, ra), scr(A, rb), ewidth(w, 7), P.edge, 0.42, null);
  A.edgeNote =
    all.length > EDGE_CAP
      ? `The ${EDGE_CAP} heaviest of ${all.length} edges between the packages and modules open at this zoom.`
      : `${all.length} edges between the packages and modules open at this zoom.`;
}

// The hover card, drawn on the canvas beside the pointer.
function tipLines(A, hv) {
  const out = [];
  if (hv.t === "module") {
    const m = A.D.modules[hv.m];
    const r = rowAt(A, A.si, hv.m);
    out.push([m.module, true], [m.path, false]);
    if (!r) out.push([`Not in ${label(A, A.si)}.`, false]);
    else {
      out.push([`${n(r[1])} definitions (trellis)`, false]);
      out.push([`call sites: ${n(r[7])} bound out (derived), ${n(r[2])} ambiguous, ${n(r[3])} unresolved (trellis)`, false]);
      out.push([`${n(r[5])} bound in from ${n(r[6])} modules (derived)`, false]);
      const c = A.D.churn.get(hv.m);
      if (c) out.push([`commits 30/90/365 d: ${n(c.d30)} / ${n(c.d90)} / ${n(c.d365)} (git)`, false]);
    }
  } else if (hv.t === "symbol") {
    const s = A.D.symbols[hv.i];
    out.push([`${s.kind} ${s.qual}`, true], [`lines ${n(s.line)}–${n(s.end)}`, false]);
    out.push([`call sites: ${n(s.cin)} bound in, ${n(s.camb)} ambiguous in, ${n(s.cout)} bound out (trellis)`, false]);
    out.push(["Click for its radius.", false]);
  } else if (hv.t === "region") {
    const g = A.D.regions[hv.r];
    out.push([g.path, true], [regionLabel(A, g), false]);
  }
  return out;
}

function drawTip(A) {
  const ctx = A.ctx;
  const lines = tipLines(A, A.hover).map(([t, b]) => [t.length > 90 ? `${t.slice(0, 89)}…` : t, b]);
  if (lines.length === 0) return;
  const w = Math.max(...lines.map(([t, b]) => measure(A, b ? FONT.tipB : FONT.tip, t))) + 16;
  const h = lines.length * 16 + 10;
  let [x, y] = [A.tipAt[0] + 14, A.tipAt[1] + 14];
  if (x + w > A.vw - 4) x = Math.max(4, A.tipAt[0] - w - 14);
  if (y + h > A.vh - 4) y = Math.max(4, A.tipAt[1] - h - 14);
  ctx.fillStyle = A.P["pane-bg"];
  ctx.strokeStyle = A.P.border;
  ctx.lineWidth = 1;
  ctx.fillRect(x, y, w, h);
  ctx.strokeRect(x + 0.5, y + 0.5, w - 1, h - 1);
  ctx.textBaseline = "middle";
  lines.forEach(([t, b], i) => {
    ctx.font = b ? FONT.tipB : FONT.tip;
    ctx.fillStyle = b ? A.P.fg : A.P.muted;
    ctx.fillText(t, x + 8, y + 13 + i * 16);
  });
}

function drawMinimap(A) {
  const mm = byId(A, "at-minimap");
  if (!mm) return;
  const far = A.D === null || A.t.k <= fitT(A).k * 1.35;
  if (mm.hidden !== far) mm.hidden = far;
  if (far) return;
  const W = 140;
  const H = 100;
  const d = A.dpr;
  if (mm.width !== W * d) {
    mm.width = W * d;
    mm.height = H * d;
  }
  const c = mm.getContext("2d");
  const K = Math.min(W / A.D.ww, H / WORLD_H);
  const ox = (W - A.D.ww * K) / 2;
  const oy = (H - WORLD_H * K) / 2;
  c.setTransform(d, 0, 0, d, 0, 0);
  c.fillStyle = A.P["pane-bg"];
  c.fillRect(0, 0, W, H);
  for (const g of A.D.regions) {
    if (g.depth === 0 || g.depth > 2) continue;
    c.fillStyle = g.depth === 1 ? (g.test ? A.P["region-test"] : A.P.region) : g.test ? A.P["region-head-test"] : A.P["region-head"];
    c.fillRect(ox + g.x0 * K, oy + g.y0 * K, (g.x1 - g.x0) * K, (g.y1 - g.y0) * K);
  }
  if (A.F) {
    for (const [m, f] of A.F.role) {
      const l = A.D.modules[m];
      if (!l) continue;
      c.fillStyle = f & SEL ? A.P.fg : f & CALLER ? A.P.caller : f & CALLEE ? A.P.callee : f & REACH ? A.P.reach : f & DOUBT ? A.P.doubt : A.P.accent;
      c.fillRect(ox + l.x0 * K, oy + l.y0 * K, Math.max(1.5, (l.x1 - l.x0) * K), Math.max(1.5, (l.y1 - l.y0) * K));
    }
  }
  const t = A.t;
  const a = [-t.x / t.k, -t.y / t.k];
  const b = [(A.vw - t.x) / t.k, (A.vh - t.y) / t.k];
  c.strokeStyle = A.P.fg;
  c.lineWidth = 1.5;
  c.strokeRect(ox + a[0] * K, oy + a[1] * K, (b[0] - a[0]) * K, (b[1] - a[1]) * K);
}

// --- pointing ----------------------------------------------------------------

function hitTest(A, px, py) {
  if (A.D === null) return null;
  const t = A.t;
  const wx = (px - t.x) / t.k;
  const wy = (py - t.y) / t.k;
  for (const m of A.D.modules) {
    if (wx < m.x0 || wx > m.x1 || wy < m.y0 || wy > m.y1) continue;
    const open = isHead(A) && present(A, A.si, m.idx) && (m.x1 - m.x0) * t.k >= OPEN_W && (m.y1 - m.y0) * t.k >= OPEN_H;
    if (open) {
      let best = null;
      for (const i of A.D.symsOf.get(m.idx) || []) {
        const s = A.D.symbols[i];
        if (wx >= s.x0 && wx <= s.x1 && wy >= s.y0 && wy <= s.y1) best = i;
      }
      if (best !== null) return { t: "symbol", i: best };
    }
    return { t: "module", m: m.idx };
  }
  for (let j = A.D.regions.length - 1; j >= 0; j--) {
    const g = A.D.regions[j];
    if (g.depth === 0) continue;
    if (wx >= g.x0 && wx <= g.x1 && wy >= g.y0 && wy <= g.y0 + Math.max(g.hdr, 12 / t.k)) return { t: "region", r: g.idx };
  }
  return null;
}

function selKey(s) {
  if (!s) return "";
  return `${s.t}:${s.m ?? s.i ?? s.r ?? s.c}`;
}

// --- selection and history ---------------------------------------------------

function select(A, s, o = {}) {
  A.sel = s || null;
  if (A.sel && o.push !== false) {
    const key = selKey(A.sel);
    if (!(A.hist.pos >= 0 && selKey(A.hist.list[A.hist.pos]) === key)) {
      A.hist.list.splice(A.hist.pos + 1);
      A.hist.list.push(A.sel);
      A.hist.pos = A.hist.list.length - 1;
    }
  }
  // A symbol at HEAD asks for its radius, unless it is the one shown. One
  // picked from a list or the search is framed with its reach and doubt
  // when the answer comes, as the prototype framed one it held.
  A.radiusFly = null;
  if (A.sel && A.sel.t === "symbol" && isHead(A)) {
    const q = radiusQuery(A, A.sel.i);
    const r = A.view && A.view.radius;
    if (o.fly && q !== null) A.radiusFly = q;
    if (q !== null && !(r && r.symbol === q && r.state !== "refused")) {
      forward(A, { id: nextId(A, "radius"), type: "get_radius", symbol: q });
    }
  }
  A.F = focus(A);
  A.tab = o.tab || "sel";
  if (o.fly && A.F && A.F.radius) {
    A.radiusFly = null;
    flyToRadius(A);
  } else if (o.fly) flyTo(A, A.sel, false);
  renderAll(A);
}

function flyToRadius(A) {
  const F = A.F;
  go(A, frame(A, [F.selMod, ...list(F.radius.reached_modules), ...list(F.radius.touched_modules)], 0.9));
}

function frame(A, ms, fill) {
  let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
  for (const i of ms) {
    const l = A.D.modules[i];
    if (!l) continue;
    x0 = Math.min(x0, l.x0);
    y0 = Math.min(y0, l.y0);
    x1 = Math.max(x1, l.x1);
    y1 = Math.max(y1, l.y1);
  }
  if (!Number.isFinite(x0)) return fitT(A);
  const t = toward(A, x0, y0, x1, y1, fill, 0);
  const f = fitT(A);
  return t.k < f.k * 1.2 ? f : t;
}

function flyTo(A, s, deep) {
  if (!s || A.D === null) return;
  if (s.t === "module") {
    const l = A.D.modules[s.m];
    if (deep) {
      go(A, toward(A, l.x0, l.y0, l.x1, l.y1, 0.75, isHead(A) ? openK(l) : 0));
      return;
    }
    // The module with its callers and callees, so the answer is on screen.
    const S = snap(A, A.si);
    const near = [s.m];
    if (S) {
      for (const [a] of S.inn.get(s.m) || []) near.push(a);
      for (const [b] of S.out.get(s.m) || []) near.push(b);
    }
    const t = frame(A, near, 0.86);
    const kmax = Math.min(openK(l) * 0.6, fitT(A).k * 6);
    go(A, t.k > kmax ? toward(A, l.x0, l.y0, l.x1, l.y1, 1, kmax) : t);
  } else if (s.t === "symbol") {
    // Its module open, and the symbol a third of the view across.
    const sym = A.D.symbols[s.i];
    const l = A.D.modules[sym.m];
    if (!l) return;
    go(A, toward(A, sym.x0, sym.y0, sym.x1, sym.y1, 0.3, openK(l)));
  } else if (s.t === "region") {
    const g = A.D.regions[s.r];
    go(A, toward(A, g.x0, g.y0, g.x1, g.y1, 0.92, 0));
  } else if (s.t === "commit") {
    const c = A.D.commits[s.c];
    const ms = list(c && c.modules);
    if (ms.length > 0) go(A, frame(A, ms, 0.8));
  }
}

function histGo(A, d) {
  const p = A.hist.pos + d;
  if (p < 0 || p >= A.hist.list.length) return;
  A.hist.pos = p;
  select(A, A.hist.list[p], { push: false, fly: true });
}

// --- the detail pane ---------------------------------------------------------

const SOURCES = {
  trellis: "From trellis: get_codebase, get_symbols or get_radius",
  derived: "Summed or listed by the desktop's core from trellis's answer",
  git: "From git, through trellis's get_history",
};

function src(A, s) {
  const e = el(A.doc, "span", s, "at-src");
  e.title = SOURCES[s] || s;
  return e;
}

function nv(A, v, s) {
  const e = el(A.doc, "span");
  e.append(el(A.doc, "span", n(v)));
  if (s) e.append(src(A, s));
  return e;
}

function dl(A, rows) {
  const d = el(A.doc, "dl");
  for (const r of rows) {
    if (!r) continue;
    const dd = el(A.doc, "dd");
    dd.append(r[1]);
    d.append(el(A.doc, "dt", r[0]), dd);
  }
  return d;
}

function h4(A, text, s, sw) {
  const h = el(A.doc, "h4");
  if (sw) h.append(swatch(A, sw));
  h.append(text);
  if (s) h.append(src(A, s));
  return h;
}

function note(A, text) {
  return el(A.doc, "p", text, "at-note");
}

function swatch(A, cls) {
  const s = el(A.doc, "span", null, `at-sw ${cls}`);
  s.setAttribute("aria-hidden", "true");
  return s;
}

function link(A, text, fn, current) {
  const b = el(A.doc, "button", text, "at-lnk");
  b.type = "button";
  if (current) b.setAttribute("aria-current", "true");
  b.addEventListener("click", fn);
  return b;
}

function modLink(A, m, text) {
  const mod = A.D.modules[m];
  const cur = A.sel && A.sel.t === "module" && A.sel.m === m;
  return link(A, text ?? (mod ? mod.module : "module"), () => select(A, { t: "module", m }, { fly: true }), cur);
}

function symLink(A, i) {
  const s = A.D.symbols[i];
  const b = link(A, "", () => select(A, { t: "symbol", i }, { fly: true }), false);
  b.append(el(A.doc, "span", s.kind, "at-kind"), s.qual);
  return b;
}

function commitLink(A, ci) {
  const c = A.D.commits[ci];
  const cur = A.sel && A.sel.t === "commit" && A.sel.c === ci;
  const b = link(A, "", () => select(A, { t: "commit", c: ci }, { fly: true }), cur);
  b.classList.add("at-commit");
  b.append(el(A.doc, "span", `${day(c.date)}  ${short(c.commit)}`, "at-d"), el(A.doc, "span", str(c.subject) ?? "subject not reported"));
  return b;
}

// A list of modules with how many call sites each, and a bar in eight steps
// by the square root of its count against the list's largest.
function weighted(A, items, cls) {
  const top = Math.max(1, ...items.map((x) => x[1]));
  const ul = el(A.doc, "ul", null, "at-rows");
  for (const [m, w] of items) {
    const li = el(A.doc, "li");
    const bar = el(A.doc, "span", null, `at-bar ${cls} w${Math.max(1, Math.min(8, Math.round(8 * Math.sqrt(w / top))))}`);
    bar.setAttribute("aria-hidden", "true");
    li.append(bar, modLink(A, m), el(A.doc, "span", n(w), "at-n"));
    ul.append(li);
  }
  return ul;
}

function presence(A, m) {
  const d = el(A.doc, "div", null, "at-pres");
  d.setAttribute("role", "list");
  d.setAttribute("aria-label", "Snapshots holding this module");
  for (const si of A.D.points) {
    const on = present(A, si, m);
    const c = el(A.doc, "span", label(A, si), `at-pchip${on ? " on" : ""}${si === A.si ? " cur" : ""}`);
    c.setAttribute("role", "listitem");
    c.title = `${label(A, si)}: ${on ? "present" : "absent"}`;
    d.append(c);
  }
  return d;
}

function overviewView(A) {
  const S = snap(A, A.si);
  if (!S) return [el(A.doc, "h3", "Atlas"), note(A, progressText(A))];
  const s = S.s;
  // get_symbols reads HEAD: its definitions and the sites bound to them by
  // a Resolved edge (each definition's calls_in, summed by the core). At an
  // earlier snapshot the definitions are its modules' defines, summed.
  const sy = isHead(A) && A.view.symbols && !str(A.view.symbols.error) ? A.view.symbols : null;
  const defs = sy ? nv(A, sy.definitions, "trellis") : nv(A, s.definitions, "derived");
  const resolved = sy
    ? nv(A, sy.resolved_call_sites, "derived")
    : el(A.doc, "span", isHead(A) ? "not read yet" : "read at HEAD only");
  const out = [
    el(A.doc, "h3", `Snapshot ${s.label}`),
    (() => {
      const p = el(A.doc, "p");
      p.append(el(A.doc, "code", short(s.commit)), ` ${day(s.date)} `, el(A.doc, "span", str(s.subject) ?? "", "muted"));
      return p;
    })(),
    dl(A, [
      ["modules indexed", nv(A, s.modules_indexed, "trellis")],
      ["definitions", defs],
      ["module edges", nv(A, s.module_edges, "trellis")],
    ]),
    h4(A, "First-party call sites"),
    dl(A, [
      ["resolved call sites", resolved],
      ["ambiguous", nv(A, s.ambiguous, "trellis")],
      ["unresolved", nv(A, s.unresolved, "trellis")],
    ]),
    note(A, "Each line counts its own sites. The panel draws no total and no share."),
    h4(A, "The map's edges"),
    dl(A, [["call sites bound across modules", nv(A, s.bound_call_sites, "trellis")]]),
    note(A, "Click a module for what calls it and what it calls, a package header for its figures, a symbol (zoom in on a module) for its radius. Press / to search."),
  ];
  if (isCount(s.modules_not_drawn) && s.modules_not_drawn > 0) {
    out.push(note(A, `${s.modules_not_drawn} modules past the limit were not listed by trellis, so they are not on the map.`));
  }
  if (list(s.not_placed).length > 0) {
    out.push(note(A, `${list(s.not_placed).length} modules in this snapshot have no place on the map, which was fixed before they appeared: ${list(s.not_placed).slice(0, 5).join(", ")}`));
  }
  return out;
}

function moduleView(A, m) {
  const mod = A.D.modules[m];
  const r = rowAt(A, A.si, m);
  const S = snap(A, A.si);
  const pinned = A.pins.has(mod.path);
  const here = label(A, A.si);
  const actions = el(A.doc, "div", null, "at-actions");
  const pin = el(A.doc, "button", pinned ? "Unpin from working set" : "Pin to working set", "at-btn");
  pin.type = "button";
  pin.setAttribute("aria-pressed", String(pinned));
  pin.addEventListener("click", () => togglePin(A, mod.path));
  const open = el(A.doc, "button", "Open symbols", "at-btn");
  open.type = "button";
  open.addEventListener("click", () => flyTo(A, { t: "module", m }, true));
  actions.append(pin, open);
  const out = [
    el(A.doc, "h3", "Module"),
    el(A.doc, "p", mod.module, "at-title"),
    (() => {
      const p = el(A.doc, "p");
      p.append(el(A.doc, "code", mod.path));
      return p;
    })(),
    actions,
    h4(A, "In snapshots", "trellis"),
    presence(A, m),
  ];
  if (!r) {
    out.push(note(A, `Not in ${here}. Move the snapshot to one that holds it.`));
  } else {
    const floor = isCount(r[4]) && r[4] > 0;
    out.push(
      h4(A, `At ${here}`),
      dl(A, [
        ["definitions", nv(A, r[1], "trellis")],
        ["call sites bound in, from other modules", nv(A, r[5], "derived")],
      ]),
      h4(A, "Call sites made here"),
      dl(A, [
        [floor ? "bound to another module, at least" : "bound to another module", nv(A, r[7], "derived")],
        ["ambiguous", nv(A, r[2], "trellis")],
        ["unresolved", nv(A, r[3], "trellis")],
      ]),
      note(A, "Sites bound inside the module are counted only in the repository's figure, not per module."),
    );
    if (A.lens === "diff") out.push(...moduleDiff(A, m));
  }
  out.push(h4(A, "Change", "git"));
  const c = A.D.churn.get(m);
  const since = A.D.history && A.D.history.since ? A.D.history.since.name : null;
  if (A.D.history === null) out.push(note(A, "The history was not read."));
  else if (c) {
    out.push(
      dl(A, [
        ["commits, last 30 / 90 / 365 days", el(A.doc, "span", `${n(c.d30)} / ${n(c.d90)} / ${n(c.d365)}`)],
        since === null ? null : [`commits since ${since}`, nv(A, c.commits)],
        ["last change", el(A.doc, "span", day(c.last_date))],
      ]),
    );
  } else {
    out.push(note(A, since === null ? "No commit changed it in the last 365 days." : `No commit changed it in the last 365 days, nor since ${since}.`));
  }
  const cs = A.D.commitsOf.get(m) || [];
  if (cs.length > 0) {
    const ul = el(A.doc, "ul", null, "at-rows");
    for (const ci of cs) {
      const li = el(A.doc, "li");
      li.append(commitLink(A, ci));
      ul.append(li);
    }
    out.push(ul);
  } else if (A.D.history !== null) {
    out.push(note(A, `None of the ${A.D.commits.length} newest commits touch it.`));
  }
  if (r && S) {
    const inn = (S.inn.get(m) || []).slice().sort((a, b) => b[1] - a[1]);
    const outE = (S.out.get(m) || []).slice().sort((a, b) => b[1] - a[1]);
    out.push(h4(A, ` Called from ${plural(inn.length, "module", "modules")}`, "trellis", "caller"));
    out.push(inn.length ? weighted(A, inn, "caller") : note(A, "No other module has a call site bound here."));
    out.push(h4(A, ` Calls into ${plural(outE.length, "module", "modules")}`, "trellis", "callee"));
    out.push(outE.length ? weighted(A, outE, "callee") : note(A, "No call site here is bound to another module."));
  }
  out.push(...symbolTable(A, m));
  return out;
}

function moduleDiff(A, m) {
  const b = A.base;
  const o = rowAt(A, b, m);
  const r = rowAt(A, A.si, m);
  const out = [h4(A, `Against baseline ${label(A, b)}`)];
  if (!o) {
    out.push(note(A, `Not in ${label(A, b)}.`));
    return out;
  }
  out.push(
    dl(A, [
      ["definitions", el(A.doc, "span", `${n(o[1])} → ${n(r[1])}`)],
      ["ambiguous call sites", el(A.doc, "span", `${n(o[2])} → ${n(r[2])}`)],
      ["unresolved call sites", el(A.doc, "span", `${n(o[3])} → ${n(r[3])}`)],
    ]),
  );
  const D = diff(A);
  const touch = (e) => e[0] === m || e[1] === m;
  const ae = D.addedE.filter(touch);
  const re = D.removedE.filter(touch);
  if (ae.length === 0 && re.length === 0) {
    out.push(note(A, "No edge to or from it was added or removed."));
    return out;
  }
  const ul = el(A.doc, "ul", null, "at-rows");
  for (const [e, tag, to, from] of [
    ...ae.map((e) => [e, "added", "calls", "called by"]),
    ...re.map((e) => [e, "removed", "called", "was called by"]),
  ]) {
    const li = el(A.doc, "li");
    li.append(el(A.doc, "span", tag, "at-tag"), e[0] === m ? to : from, modLink(A, e[0] === m ? e[1] : e[0]), el(A.doc, "span", n(e[2]), "at-n"));
    ul.append(li);
  }
  out.push(ul);
  return out;
}

const SYM_COLUMNS = [
  ["name", "Name"],
  ["line", "Line"],
  ["cin", "In"],
  ["camb", "Amb in"],
  ["cout", "Out"],
];

function symbolTable(A, m) {
  const ids = A.D.symsOf.get(m) || [];
  const out = [h4(A, `Symbols at HEAD (${ids.length})`, "trellis")];
  if (A.view.symbols === null) {
    out.push(note(A, "HEAD's symbols have not been read yet."));
    return out;
  }
  if (ids.length === 0) {
    out.push(note(A, present(A, A.D.head, m) ? "No definitions." : "Not in HEAD, so no symbols are read for it."));
    return out;
  }
  const { key, desc } = A.symSort;
  const val = (i) => A.D.symbols[i][key];
  const sorted = ids.slice().sort((a, b) => {
    const x = val(a);
    const y = val(b);
    const c = typeof x === "string" ? x.localeCompare(y) : (isCount(x) ? x : -1) - (isCount(y) ? y : -1);
    return (desc ? -c : c) || A.D.symbols[a].line - A.D.symbols[b].line;
  });
  out.push(note(A, "In: call sites bound to it. Amb in: sites naming it among several candidates. Out: sites in it bound to one definition. Three counts from trellis's get_symbols."));
  const table = el(A.doc, "table", null, "at-syms");
  const head = el(A.doc, "tr");
  for (const [k, lab] of SYM_COLUMNS) {
    const th = el(A.doc, "th");
    th.scope = "col";
    if (key === k) th.setAttribute("aria-sort", desc ? "descending" : "ascending");
    const b = el(A.doc, "button", lab);
    b.type = "button";
    b.addEventListener("click", () => {
      A.symSort = { key: k, desc: key === k ? !desc : k !== "name" };
      renderPanel(A);
    });
    th.append(b);
    head.append(th);
  }
  const thead = el(A.doc, "thead");
  thead.append(head);
  const tbody = el(A.doc, "tbody");
  for (const i of sorted) {
    const s = A.D.symbols[i];
    const tr = el(A.doc, "tr");
    const first = el(A.doc, "td");
    first.append(symLink(A, i));
    tr.append(first, el(A.doc, "td", n(s.line)), el(A.doc, "td", n(s.cin)), el(A.doc, "td", n(s.camb)), el(A.doc, "td", n(s.cout)));
    tbody.append(tr);
  }
  table.append(thead, tbody);
  const wrap = el(A.doc, "div", null, "at-tablewrap");
  wrap.append(table);
  out.push(wrap);
  return out;
}

function symbolView(A, i) {
  const s = A.D.symbols[i];
  const q = radiusQuery(A, i);
  const title = el(A.doc, "p", null, "at-title");
  title.append(el(A.doc, "span", s.kind, "at-kind"), s.qual);
  const where = el(A.doc, "p");
  where.append("in ", modLink(A, s.m), `, lines ${n(s.line)}–${n(s.end)}`, src(A, "trellis"));
  const out = [
    el(A.doc, "h3", "Symbol"),
    title,
    where,
    h4(A, "Call sites, three counts", "trellis"),
    dl(A, [
      ["bound to it (in)", nv(A, s.cin)],
      ["naming it among several (ambiguous in)", nv(A, s.camb)],
      ["bound from it (out)", nv(A, s.cout)],
    ]),
    (() => {
      const p = el(A.doc, "p", null, "at-note");
      p.append("trellis names it ", el(A.doc, "code", q ?? s.name), ".");
      return p;
    })(),
    h4(A, "Radius"),
  ];
  const r = A.view.radius;
  if (!isHead(A)) {
    out.push(note(A, "Symbols and radius are read at HEAD. Move the snapshot to HEAD to see the radius on the map."));
    return out;
  }
  if (!r || r.symbol !== q) {
    out.push(note(A, "Select it on the map, or from a list, to ask trellis for its radius."));
    return out;
  }
  if (r.state === "asked") {
    out.push(note(A, `Asking trellis for the radius of ${q}: it indexes the repository for it.`));
    return out;
  }
  if (r.state === "refused") {
    out.push(note(A, `trellis gave no radius: ${r.error}`));
    return out;
  }
  const a = r.answer;
  const reach = a.reach || {};
  const doubt = a.doubt || {};
  const blocks = el(A.doc, "div", null, "at-rd");
  const rb = el(A.doc, "section", null, "at-rd-block reach");
  rb.setAttribute("aria-label", "Reach");
  rb.append(
    h4(A, "Reach", null, "reach"),
    dl(A, [
      ["resolved call sites", nv(A, reach.resolved, "trellis")],
      ["ambiguous call sites", nv(A, reach.ambiguous, "trellis")],
      ["symbols reached", nv(A, reach.symbols_reached, "trellis")],
      ["modules reached", nv(A, list(a.reached_modules).length, "derived")],
    ]),
  );
  const db = el(A.doc, "section", null, "at-rd-block doubt");
  db.setAttribute("aria-label", "Doubt");
  db.append(
    h4(A, "Doubt", null, "doubt"),
    dl(A, [
      ["unbound call sites in touched files", nv(A, doubt.unbound_in_touched_files, "trellis")],
      ["touched files", nv(A, doubt.touched_files, "trellis")],
    ]),
  );
  blocks.append(rb, db);
  out.push(blocks, note(A, "Reach and doubt count different populations, so they are shown side by side and never divided. Reach is a floor, not a boundary."));
  const rm = list(a.reached_modules);
  out.push(h4(A, ` Modules reached (${rm.length})`, "derived", "reach"));
  if (rm.length > 0) {
    const ul = el(A.doc, "ul", null, "at-rows");
    for (const m of rm) {
      const li = el(A.doc, "li");
      li.append(modLink(A, m));
      ul.append(li);
    }
    out.push(ul);
  } else out.push(note(A, "None outside the symbol itself."));
  if (isCount(a.reached_not_listed) && a.reached_not_listed > 0) out.push(note(A, `${a.reached_not_listed} reached symbols past trellis's list are not marked.`));
  const files = list(doubt.files);
  out.push(h4(A, ` Touched files (${files.length})`, "trellis", "doubt"));
  out.push(note(A, "Unbound call sites per file."));
  const onMap = files.filter((f) => isCount(f.m)).map((f) => [f.m, isCount(f.call_sites) ? f.call_sites : 0]);
  if (onMap.length > 0) out.push(weighted(A, onMap, "doubt"));
  const reasons = list(doubt.reasons);
  if (reasons.length > 0) {
    out.push(h4(A, "Doubt by reason", "trellis"));
    out.push(dl(A, reasons.map((x) => [str(x.reason) ?? "reason", nv(A, x.call_sites)])));
  }
  return out;
}

function regionView(A, g) {
  const S = snap(A, A.si);
  const c = S ? list(S.s.regions)[g.idx] : null;
  const v = Array.isArray(c) ? c : [];
  return [
    el(A.doc, "h3", "Package"),
    (() => {
      const p = el(A.doc, "p", null, "at-title");
      p.append(el(A.doc, "code", g.path));
      return p;
    })(),
    dl(A, [
      [`modules at ${label(A, A.si)}`, nv(A, v[0], "trellis")],
      ["definitions", nv(A, v[1], "derived")],
    ]),
    h4(A, "Call sites made inside, two of the three counts", "derived"),
    dl(A, [
      ["ambiguous", nv(A, v[2])],
      ["unresolved", nv(A, v[3])],
    ]),
    note(A, "Each summed over the package's modules at this snapshot. Click a module for its callers and callees."),
  ];
}

function commitView(A, ci) {
  const c = A.D.commits[ci];
  const ms = list(c.modules);
  const others = list(c.other_paths);
  const head = el(A.doc, "h3", "Commit");
  head.append(src(A, "git"));
  const out = [
    head,
    el(A.doc, "p", str(c.subject) ?? "subject not reported", "at-title"),
    (() => {
      const p = el(A.doc, "p");
      p.append(el(A.doc, "code", str(c.commit) ?? ""), ` ${day(c.date)}`);
      return p;
    })(),
    h4(A, ` Python modules it touched (${ms.length})`, null, "commit"),
  ];
  if (ms.length > 0) {
    const ul = el(A.doc, "ul", null, "at-rows");
    for (const m of ms) {
      const li = el(A.doc, "li");
      li.append(modLink(A, m));
      ul.append(li);
    }
    out.push(ul);
  } else out.push(note(A, "None on the map."));
  if (others.length > 0) {
    out.push(h4(A, `Other files, not on the map (${others.length})`));
    const ul = el(A.doc, "ul", null, "at-rows");
    for (const p of others) {
      const li = el(A.doc, "li");
      li.append(el(A.doc, "code", p));
      ul.append(li);
    }
    out.push(ul);
  }
  return out;
}

function changesView(A) {
  const out = [];
  if (A.lens === "diff") {
    const D = diff(A);
    out.push(el(A.doc, "h3", `Diff ${label(A, D.b)} → ${label(A, D.t)}`));
    if (D.b === D.t) out.push(note(A, "Baseline and snapshot are the same. Pick another baseline or move the snapshot."));
    out.push(
      dl(A, [
        ["modules added", nv(A, D.added.size, "trellis")],
        ["modules removed", nv(A, D.removed.size, "trellis")],
        ["modules with changed counts", nv(A, D.changed.length, "trellis")],
        ["module edges added", nv(A, D.addedE.length, "trellis")],
        ["module edges removed", nv(A, D.removedE.length, "trellis")],
      ]),
    );
    const names = (set) => [...set].sort((a, b) => A.D.modules[a].module.localeCompare(A.D.modules[b].module));
    for (const [set, cls, word] of [
      [D.added, "added", "Added"],
      [D.removed, "removed", "Removed"],
    ]) {
      out.push(h4(A, ` ${word} (${set.size})`, null, cls));
      const ul = el(A.doc, "ul", null, "at-rows");
      for (const m of names(set)) {
        const li = el(A.doc, "li");
        li.append(modLink(A, m));
        ul.append(li);
      }
      out.push(ul);
    }
    for (const [edges, cls, word] of [
      [D.addedE, "line added", "Edges added"],
      [D.removedE, "line removed", "Edges removed"],
    ]) {
      out.push(h4(A, ` ${word} (${edges.length}), call sites`, null, cls));
      const ul = el(A.doc, "ul", null, "at-rows");
      for (const e of edges.slice(0, 60)) {
        const li = el(A.doc, "li");
        li.append(modLink(A, e[0]), "→", modLink(A, e[1]), el(A.doc, "span", n(e[2]), "at-n"));
        ul.append(li);
      }
      out.push(ul);
      if (edges.length > 60) out.push(note(A, `${edges.length - 60} lighter edges not listed.`));
    }
  } else {
    const p = el(A.doc, "p", "To compare two snapshots, ", "at-note");
    const b = el(A.doc, "button", "switch to the Diff lens", "at-btn");
    b.type = "button";
    b.addEventListener("click", () => setLens(A, "diff"));
    p.append(b, ".");
    out.push(p);
  }
  if (A.D.history === null) {
    out.push(note(A, "The history was not read, so no commits are listed."));
    return out;
  }
  out.push(h4(A, `Recent commits (${A.D.commits.length} newest)`, "git"));
  out.push(note(A, "Click one to mark the files it touched."));
  const ul = el(A.doc, "ul", null, "at-rows");
  A.D.commits.forEach((c, ci) => {
    const li = el(A.doc, "li");
    li.append(commitLink(A, ci), el(A.doc, "span", `${list(c.modules).length} on map`, "at-n"));
    ul.append(li);
  });
  out.push(ul);
  return out;
}

function pinsView(A) {
  const out = [el(A.doc, "h3", "Working set")];
  const s = A.sel;
  if (s && s.t === "module" && !A.pins.has(A.D.modules[s.m].path)) {
    const b = el(A.doc, "button", `Pin ${A.D.modules[s.m].module}`, "at-btn");
    b.type = "button";
    b.addEventListener("click", () => togglePin(A, A.D.modules[s.m].path));
    out.push(b);
  }
  const pinned = [...A.pins].map((p) => A.D.byPath.get(p)).filter(isCount);
  if (pinned.length === 0) {
    out.push(note(A, "Pin the modules you return to. Pins are kept on this computer only, and the Atlas works without them."));
    return out;
  }
  pinned.sort((a, b) => A.D.modules[a].module.localeCompare(A.D.modules[b].module));
  const ul = el(A.doc, "ul", null, "at-rows");
  for (const m of pinned) {
    const li = el(A.doc, "li");
    li.append(modLink(A, m));
    if (!present(A, A.si, m)) li.append(el(A.doc, "span", `not in ${label(A, A.si)}`, "at-tag"));
    const un = el(A.doc, "button", "Unpin", "at-btn");
    un.type = "button";
    un.setAttribute("aria-label", `Unpin ${A.D.modules[m].module}`);
    un.addEventListener("click", () => togglePin(A, A.D.modules[m].path));
    li.append(un);
    ul.append(li);
  }
  out.push(ul, note(A, "Pinned modules carry a corner mark on the map."));
  return out;
}

function renderPanel(A) {
  for (const t of ["sel", "chg", "pin"]) byId(A, `at-tab-${t}`).setAttribute("aria-selected", String(A.tab === t));
  byId(A, "at-tab-pin").textContent = A.pins.size > 0 ? `Working set (${A.pins.size})` : "Working set";
  const pane = byId(A, "at-detail");
  pane.setAttribute("aria-labelledby", `at-tab-${A.tab}`);
  let kids;
  if (A.D === null) kids = [el(A.doc, "h3", "Atlas"), note(A, progressText(A))];
  else if (A.tab === "chg") kids = changesView(A);
  else if (A.tab === "pin") kids = pinsView(A);
  else if (!A.sel) kids = overviewView(A);
  else if (A.sel.t === "module") kids = moduleView(A, A.sel.m);
  else if (A.sel.t === "symbol") kids = symbolView(A, A.sel.i);
  else if (A.sel.t === "region") kids = regionView(A, A.D.regions[A.sel.r]);
  else kids = commitView(A, A.sel.c);
  pane.replaceChildren(...kids.filter(Boolean));
  pane.scrollTop = 0;
}

// --- the legend, the toolbar and the breadcrumbs -----------------------------

function lgRow(A, cls, text, extra) {
  const r = el(A.doc, "span", null, "at-lg-row");
  if (cls) r.append(swatch(A, cls));
  const t = el(A.doc, "span", text);
  if (extra) t.append(el(A.doc, "span", ` ${extra}`, "at-lg-note"));
  r.append(t);
  return r;
}

function renderLegend(A) {
  const F = A.F;
  const rows = [];
  const notes = [];
  const here = label(A, A.si);
  if (F && F.radius) {
    rows.push(lgRow(A, "sel", "Selected symbol’s module"));
    rows.push(lgRow(A, "reach", `Reach: ${plural(list(F.radius.reached_modules).length, "module", "modules")} reached`, "(derived from trellis)"));
    rows.push(lgRow(A, "doubt", `Doubt: ${n((F.radius.doubt || {}).touched_files)} touched files`, "(trellis)"));
    notes.push("Reach and doubt are two counts from different populations; they are not divided.");
  } else if (F && F.commit >= 0) {
    rows.push(lgRow(A, "commit", "Files the commit touched", "(git)"));
  } else if (F && F.selMod >= 0) {
    const callers = F.edges.filter((e) => e[3] === CALLER).length;
    rows.push(lgRow(A, "sel", "Selected module"));
    rows.push(lgRow(A, "caller", `Calls it: ${plural(callers, "module", "modules")}`));
    rows.push(lgRow(A, "callee", `Called by it: ${plural(F.edges.length - callers, "module", "modules")}`));
  } else if (F && F.region !== null) {
    rows.push(lgRow(A, "sel", "Selected package"));
  }
  if (A.lens === "churn") {
    const since = A.D && A.D.history && A.D.history.since ? `since ${A.D.history.since.name}` : "since the oldest tag";
    const win = ["last 30 days", "last 90 days", "last 365 days", since][A.span];
    rows.push(lgRow(A, null, `Commits touching the file, ${win}`, "(git)"));
    const ramp = el(A.doc, "span", null, "at-ramp");
    CHURN_BINS.forEach((b, i) => {
      const s = el(A.doc, "span");
      s.append(swatch(A, i ? `c${i}` : ""), b);
      ramp.append(s);
    });
    rows.push(ramp);
  } else if (A.lens === "doubt") {
    rows.push(lgRow(A, "amb", "Ambiguous call sites", "(trellis)"));
    rows.push(lgRow(A, "unres", "Unresolved call sites", "(trellis)"));
    notes.push("Each square's area grows with its count, on one scale for every module. Two counts, never a share.");
  } else if (A.lens === "diff") {
    const D = diff(A);
    rows.push(el(A.doc, "span", `${label(A, D.b)} → ${label(A, D.t)}`, "at-lg-row at-lg-head"));
    rows.push(lgRow(A, "added", `Module added (${D.added.size})`));
    rows.push(lgRow(A, "removed", `Module removed (${D.removed.size})`));
    if (!F) {
      rows.push(lgRow(A, "line added", "Edge added"));
      rows.push(lgRow(A, "line removed", "Edge removed"));
    }
  }
  rows.push(lgRow(A, "region", "Source package"));
  rows.push(lgRow(A, "region-test", "Test package"));
  rows.push(lgRow(A, "ghost", `Not in ${here}`));
  const floor = A.view && A.view.layout && isCount(A.view.layout.floor) ? A.view.layout.floor : null;
  notes.push(
    `Area: definitions per module (trellis) at the newest snapshot holding it${floor === null ? "" : `, at least ${floor}`}. Positions never change between snapshots once every snapshot is loaded.`,
  );
  const keys = el(A.doc, "div", null, "at-lg-keys");
  keys.append(...rows);
  const lines = el(A.doc, "div", "Lines: call sites bound across modules, width by square root. ", "at-lg-note");
  const edges = el(A.doc, "span", A.edgeNote);
  edges.id = "at-lg-edges";
  lines.append(edges);
  byId(A, "at-legend").replaceChildren(keys, ...notes.map((t) => el(A.doc, "div", t, "at-lg-note")), lines);
}

function renderToolbar(A) {
  for (const b of A.doc.querySelectorAll("#at-lens button")) b.setAttribute("aria-pressed", String(b.dataset.lens === A.lens));
  for (const b of A.doc.querySelectorAll("#at-win button")) b.setAttribute("aria-pressed", String(Number(b.dataset.win) === A.span));
  byId(A, "at-win-field").hidden = A.lens !== "churn";
  byId(A, "at-base-field").hidden = A.lens !== "diff";
  const since = A.D && A.D.history && A.D.history.since ? `since ${A.D.history.since.name}` : "since the oldest tag";
  byId(A, "at-win-since").textContent = since;
  const points = A.D ? A.D.points : [];
  const range = byId(A, "at-snap");
  range.max = String(Math.max(0, points.length - 1));
  range.value = String(Math.max(0, points.indexOf(A.si)));
  range.disabled = points.length < 2;
  const ticks = points.map((si) => {
    const t = el(A.doc, "span", label(A, si));
    t.setAttribute("aria-current", String(si === A.si));
    return t;
  });
  byId(A, "at-ticks").replaceChildren(...ticks);
  const base = byId(A, "at-base");
  base.replaceChildren(
    ...points.map((si) => {
      const o = el(A.doc, "option", `${label(A, si)} (${day(snap(A, si).s.date)})`);
      o.value = String(si);
      return o;
    }),
  );
  base.value = String(A.base);
  const S = snap(A, A.si);
  const readout = byId(A, "at-readout");
  if (S && S.s.state === "loaded") {
    const s = S.s;
    readout.replaceChildren(
      el(A.doc, "b", s.label),
      ` ${short(s.commit)} · ${day(s.date)} · ${n(s.modules_indexed)} modules · ${n(s.module_edges)} module edges`,
      src(A, "trellis"),
    );
    range.setAttribute("aria-valuetext", `${s.label}, ${day(s.date)}`);
  } else readout.replaceChildren();
  const head = A.D ? snap(A, A.D.head) : null;
  const parts = [rootName(A), head ? `HEAD ${short(head.s.commit)}` : "", A.version].filter((x) => x);
  byId(A, "at-where").textContent = parts.join(" · ");
}

function crumbs(A) {
  const ol = byId(A, "at-crumbs");
  if (A.D === null || A.vw <= 1) {
    if (ol.childElementCount !== 0) ol.replaceChildren();
    A.lastCrumbs = "";
    return;
  }
  const t = A.t;
  const cx = (A.vw / 2 - t.x) / t.k;
  const cy = (A.vh / 2 - t.y) / t.k;
  const v = [-t.x / t.k, -t.y / t.k, (A.vw - t.x) / t.k, (A.vh - t.y) / t.k];
  const path = [A.D.regions[0]];
  let g = A.D.regions[0];
  while (g) {
    const c = g.kids.map((i) => A.D.regions[i]).find((k) => cx >= k.x0 && cx <= k.x1 && cy >= k.y0 && cy <= k.y1);
    if (!c) break;
    const ix = Math.max(0, Math.min(v[2], c.x1) - Math.max(v[0], c.x0));
    const iy = Math.max(0, Math.min(v[3], c.y1) - Math.max(v[1], c.y0));
    if (ix * iy < 0.6 * (v[2] - v[0]) * (v[3] - v[1])) break;
    path.push(c);
    g = c;
  }
  const key = path.map((p) => p.idx).join(",");
  if (key === A.lastCrumbs) return;
  A.lastCrumbs = key;
  const root = rootName(A) ?? "root";
  ol.replaceChildren(
    ...path.map((p) => {
      const li = el(A.doc, "li");
      const b = el(A.doc, "button", p.depth === 0 ? root : p.label);
      b.type = "button";
      b.addEventListener("click", () => (p.depth === 0 ? go(A, fitT(A)) : go(A, toward(A, p.x0, p.y0, p.x1, p.y1, 0.92, 0))));
      li.append(b);
      return li;
    }),
  );
}

function renderAll(A) {
  renderPanel(A);
  renderLegend(A);
  renderToolbar(A);
  byId(A, "at-back").disabled = A.hist.pos <= 0;
  byId(A, "at-fwd").disabled = A.hist.pos >= A.hist.list.length - 1;
  status(A, progressText(A));
  redraw(A);
  crumbs(A);
}

// The baseline a diff opens against, until one is chosen: the oldest
// snapshot loaded, or the newest when the oldest is the one shown.
function oldestBase(A) {
  const p = A.D.points;
  const oldest = A.D.points[0];
  return oldest !== A.si || p.length < 2 ? oldest ?? A.si : p[p.length - 1];
}

function setLens(A, l) {
  A.lens = l;
  A.diffCache = null;
  if (l === "diff" && A.D && !A.baseChosen) A.base = oldestBase(A);
  if (l === "diff" && (!A.sel || A.sel.t !== "module")) A.tab = "chg";
  A.F = focus(A);
  renderAll(A);
}

// The open repository's directory name, or null while none is known.
function rootName(A) {
  return A.root ? A.root.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || A.root : null;
}

// The root, from child-started or, for a page that loaded after its child
// started and so heard none, from the child's answer to get_state.
function setRoot(A, root) {
  if (typeof root !== "string" || root === "" || root === A.root) return;
  A.root = root;
  A.pins = loadPins(A);
  A.lastCrumbs = "";
  renderAll(A);
}

// --- the working set ---------------------------------------------------------

// Pins are paths, kept per repository in localStorage. A page where storage
// is refused still works: the set then lasts this visit.
function loadPins(A) {
  try {
    const all = JSON.parse(A.win.localStorage.getItem(PIN_KEY) || "{}");
    const mine = all && typeof all === "object" && Array.isArray(all[A.root]) ? all[A.root] : [];
    return new Set(mine.filter((p) => typeof p === "string"));
  } catch {
    return new Set();
  }
}

function savePins(A) {
  try {
    const all = JSON.parse(A.win.localStorage.getItem(PIN_KEY) || "{}");
    const next = all && typeof all === "object" && !Array.isArray(all) ? all : {};
    next[A.root] = [...A.pins];
    A.win.localStorage.setItem(PIN_KEY, JSON.stringify(next));
  } catch {
    // Storage refused: the set lasts this visit.
  }
}

function togglePin(A, path) {
  if (A.pins.has(path)) A.pins.delete(path);
  else A.pins.add(path);
  savePins(A);
  renderPanel(A);
  redraw(A);
}

// --- search ------------------------------------------------------------------

// The search's candidates, built when an answer brings modules or symbols
// and kept while they are the same, so opening the search waits on nothing.
function indexSearch(A) {
  if (A.D === null) {
    A.cands = null;
    A.candsKey = null;
    return;
  }
  const L = A.view.layout;
  const key = `${A.D.modules.length}|${A.D.fixed}|${list(L.over).join(",")}|${A.D.symbols.length}`;
  if (A.cands && A.candsKey === key) return;
  A.candsKey = key;
  const out = [];
  for (const m of A.D.modules) out.push({ t: "module", id: m.idx, text: m.module, low: m.module.toLowerCase(), cut: m.module.lastIndexOf(".") });
  for (const s of A.D.symbols) {
    const m = A.D.modules[s.m];
    if (!m) continue;
    const text = `${m.module}.${s.qual}`;
    out.push({ t: "symbol", id: s.idx, text, low: text.toLowerCase(), cut: m.module.length });
  }
  A.cands = out;
}

function candidates(A) {
  return A.cands ?? [];
}

const SEPS = "._/ ";

function fuzzy(q, c) {
  const s = c.low;
  const o = c.text;
  let best = null;
  let start = s.indexOf(q[0]);
  let tries = 0;
  const bonus = (j) => (j === 0 || SEPS.includes(s[j - 1]) ? 6 : 0) + (j > 0 && o[j] !== s[j] && o[j - 1] === s[j - 1] ? 4 : 0);
  while (start !== -1 && tries < 16) {
    const pos = [start];
    let score = bonus(start);
    let prev = start;
    let ok = true;
    for (let qi = 1; qi < q.length; qi++) {
      const j = s.indexOf(q[qi], prev + 1);
      if (j < 0) {
        ok = false;
        break;
      }
      score += (j === prev + 1 ? 5 : -Math.min(4, (j - prev - 1) * 0.2)) + bonus(j);
      pos.push(j);
      prev = j;
    }
    if (!ok) break;
    if (pos[0] > c.cut) score += 10;
    score -= s.length * 0.03;
    if (!best || score > best.score) best = { score, pos };
    start = s.indexOf(q[0], start + 1);
    tries += 1;
  }
  return best;
}

function runSearch(A, raw) {
  const terms = raw.toLowerCase().split(/\s+/).filter(Boolean);
  if (terms.length === 0 || A.D === null) return [];
  const res = [];
  for (const c of candidates(A)) {
    let score = 0;
    let pos = [];
    let ok = true;
    for (const t of terms) {
      const f = fuzzy(t, c);
      if (!f) {
        ok = false;
        break;
      }
      score += f.score;
      pos = pos.concat(f.pos);
    }
    if (!ok) continue;
    const segs = c.low.split(".");
    for (const t of terms) {
      if (segs.includes(t)) score += 6;
      if (segs[segs.length - 1] === t) score += 6;
    }
    score += c.t === "module" ? 3 : Math.min(3, Math.log2(1 + (isCount(A.D.symbols[c.id].cin) ? A.D.symbols[c.id].cin : 0)) * 0.5);
    res.push({ c, score, pos });
  }
  res.sort((a, b) => b.score - a.score);
  return res.slice(0, RESULTS_MAX);
}

function highlighted(A, text, pos) {
  const set = new Set(pos);
  const out = [];
  let buf = "";
  let on = false;
  for (let i = 0; i < text.length; i++) {
    const m = set.has(i);
    if (m !== on && buf) {
      out.push(on ? el(A.doc, "b", buf) : buf);
      buf = "";
    }
    on = m;
    buf += text[i];
  }
  if (buf) out.push(on ? el(A.doc, "b", buf) : buf);
  return out;
}

function renderResults(A) {
  const q = byId(A, "at-q").value;
  A.results = runSearch(A, q);
  A.active = 0;
  const hint = byId(A, "at-qhint");
  if (A.D === null) hint.textContent = "Nothing to search until the map is drawn.";
  else if (!q.trim()) hint.textContent = "Fuzzy match over module and symbol names. Arrow keys move, Enter goes there.";
  else hint.textContent = A.results.length ? `${A.results.length}${A.results.length === RESULTS_MAX ? "+" : ""} matches` : "No module or symbol matches.";
  byId(A, "at-qres").replaceChildren(
    ...A.results.map((r, i) => {
      const c = r.c;
      let meta;
      if (c.t === "module") {
        const row = rowAt(A, A.si, c.id);
        meta = row ? `${n(row[1])} defs` : `not in ${label(A, A.si)}`;
      } else {
        const s = A.D.symbols[c.id];
        meta = `line ${n(s.line)} · in ${n(s.cin)}`;
      }
      const li = el(A.doc, "li");
      li.id = `at-qr${i}`;
      li.setAttribute("role", "option");
      li.setAttribute("aria-selected", String(i === A.active));
      li.addEventListener("click", () => pick(A, i));
      li.addEventListener("mousemove", () => setActive(A, i));
      const x = el(A.doc, "span", null, "at-x");
      x.append(...highlighted(A, c.text, r.pos));
      li.append(el(A.doc, "span", c.t === "module" ? "module" : A.D.symbols[c.id].kind, "at-k"), x, el(A.doc, "span", meta, "at-m"));
      return li;
    }),
  );
  byId(A, "at-q").setAttribute("aria-activedescendant", A.results.length ? "at-qr0" : "");
}

function setActive(A, i) {
  if (i === A.active || A.results.length === 0) return;
  const a = byId(A, `at-qr${A.active}`);
  if (a) a.setAttribute("aria-selected", "false");
  A.active = i;
  const b = byId(A, `at-qr${i}`);
  if (b) b.setAttribute("aria-selected", "true");
  byId(A, "at-q").setAttribute("aria-activedescendant", `at-qr${i}`);
}

function openSearch(A) {
  A.lastFocus = A.doc.activeElement;
  byId(A, "at-search").hidden = false;
  const q = byId(A, "at-q");
  q.focus();
  q.select();
  renderResults(A);
}

function closeSearch(A) {
  byId(A, "at-search").hidden = true;
  if (A.lastFocus && typeof A.lastFocus.focus === "function") A.lastFocus.focus();
}

function pick(A, i) {
  const r = A.results[i];
  if (!r) return;
  closeSearch(A);
  if (r.c.t === "module") select(A, { t: "module", m: r.c.id }, { fly: true });
  else {
    // Symbols are HEAD's.
    if (A.D && A.si !== A.D.head) moveTo(A, A.D.head);
    select(A, { t: "symbol", i: r.c.id }, { fly: true });
  }
}

// --- taking a view -----------------------------------------------------------

function moveTo(A, si) {
  A.si = si;
  if (A.D && !A.baseChosen) A.base = oldestBase(A);
  A.diffCache = null;
  A.F = focus(A);
}

// Draws one `atlas` view from the core. The shown snapshot is kept by its
// label as snapshots arrive, and the selection only while it still names
// the same module or symbol.
export function takeView(A, v) {
  if (v === null || typeof v !== "object") return;
  const wasLabel = A.D ? label(A, A.si) : null;
  const baseLabel = A.D ? label(A, A.base) : null;
  const selPath = A.sel && A.D ? selectionKey(A) : null;
  A.view = v;
  A.D = index(v);
  indexSearch(A);
  A.diffCache = null;
  settle(A);
  if (A.D !== null) {
    const find = (lab, fallback) => {
      const i = list(v.snapshots).findIndex((s) => s.label === lab);
      return i >= 0 && A.D.points.includes(i) ? i : fallback;
    };
    A.si = find(wasLabel, A.D.head);
    A.base = A.baseChosen ? find(baseLabel, oldestBase(A)) : oldestBase(A);
    A.sel = selPath === null ? null : reselect(A, selPath);
    if (!A.fitted && A.vw > 1) {
      A.fitted = true;
      A.t = fitT(A);
    }
  } else {
    A.sel = null;
  }
  A.F = focus(A);
  renderAll(A);
  if (A.F && A.F.radius && A.radiusFly !== null && A.radiusFly === v.radius.symbol) {
    A.radiusFly = null;
    flyToRadius(A);
  }
  pump(A);
}

function selectionKey(A) {
  const s = A.sel;
  if (s.t === "module") return { t: "module", path: A.D.modules[s.m].path };
  if (s.t === "symbol") {
    const x = A.D.symbols[s.i];
    return { t: "symbol", path: A.D.modules[x.m].path, name: x.name, line: x.line };
  }
  if (s.t === "region") return { t: "region", path: A.D.regions[s.r].path };
  return { t: "commit", commit: A.D.commits[s.c] && A.D.commits[s.c].commit };
}

function reselect(A, k) {
  if (k.t === "module") {
    const m = A.D.byPath.get(k.path);
    return isCount(m) ? { t: "module", m } : null;
  }
  if (k.t === "symbol") {
    const m = A.D.byPath.get(k.path);
    const i = (A.D.symsOf.get(m) || []).find((j) => A.D.symbols[j].name === k.name && A.D.symbols[j].line === k.line);
    return isCount(i) ? { t: "symbol", i } : null;
  }
  if (k.t === "region") {
    const g = A.D.regions.findIndex((r) => r.path === k.path);
    return g >= 0 ? { t: "region", r: g } : null;
  }
  const c = A.D.commits.findIndex((x) => x.commit === k.commit);
  return c >= 0 ? { t: "commit", c } : null;
}

// Forgets the last repository's atlas: its map, selection, history and the
// questions in flight, which the new child will never answer.
export function reset(A, said) {
  A.view = null;
  A.D = null;
  A.sel = null;
  A.F = null;
  A.hover = null;
  A.hist = { list: [], pos: -1 };
  A.askedHistory = false;
  A.askedSymbols = false;
  A.inflight = null;
  A.fitted = false;
  A.cands = null;
  A.candsKey = null;
  A.baseChosen = false;
  A.diffCache = null;
  A.si = -1;
  A.base = -1;
  A.pins = loadPins(A);
  byId(A, "at-search").hidden = true;
  renderAll(A);
  status(A, said);
}

// --- wiring ------------------------------------------------------------------

function resize(A) {
  const box = byId(A, "at-map").getBoundingClientRect();
  const was = A.vw > 1 ? [(A.vw / 2 - A.t.x) / A.t.k, (A.vh / 2 - A.t.y) / A.t.k] : null;
  A.vw = Math.max(1, box.width);
  A.vh = Math.max(1, box.height);
  A.dpr = A.win.devicePixelRatio || 1;
  A.canvas.width = Math.round(A.vw * A.dpr);
  A.canvas.height = Math.round(A.vh * A.dpr);
  A.hatches = new Map();
  if (A.vw > 1 && A.D !== null && !A.fitted) {
    A.fitted = true;
    A.t = fitT(A);
  } else if (was && A.vw > 1) {
    A.t = clampT(A, { k: A.t.k, x: A.vw / 2 - was[0] * A.t.k, y: A.vh / 2 - was[1] * A.t.k });
  }
  redraw(A);
  crumbs(A);
}

function wirePointer(A) {
  const cv = A.canvas;
  let drag = null;
  const at = (ev) => {
    const r = cv.getBoundingClientRect();
    return [ev.clientX - r.left, ev.clientY - r.top];
  };
  cv.addEventListener("pointerdown", (ev) => {
    if (ev.button !== 0) return;
    // A click gives the map focus, and with it the wheel.
    A.canvas.focus({ preventScroll: true });
    A.dragged = false;
    drag = { id: ev.pointerId, x: ev.clientX, y: ev.clientY, t: { ...A.t } };
  });
  cv.addEventListener("pointermove", (ev) => {
    const [px, py] = at(ev);
    if (drag !== null && ev.pointerId === drag.id) {
      const dx = ev.clientX - drag.x;
      const dy = ev.clientY - drag.y;
      if (!A.dragged && Math.hypot(dx, dy) < DRAG_PX) return;
      if (!A.dragged) {
        A.dragged = true;
        if (typeof cv.setPointerCapture === "function") cv.setPointerCapture(ev.pointerId);
      }
      A.hover = null;
      A.flight = null;
      setT(A, { k: drag.t.k, x: drag.t.x + dx, y: drag.t.y + dy });
      return;
    }
    const hv = hitTest(A, px, py);
    if (hv) cv.classList.add("at-pointing");
    else cv.classList.remove("at-pointing");
    if (selKey(hv) !== selKey(A.hover) || (hv && ev.pointerType === "mouse")) {
      A.hover = hv;
      A.tipAt = hv && ev.pointerType === "mouse" ? [px, py] : null;
      redraw(A);
    }
  });
  const end = (ev) => {
    if (drag !== null && ev.pointerId === drag.id) drag = null;
  };
  cv.addEventListener("pointerup", end);
  cv.addEventListener("pointercancel", end);
  cv.addEventListener("pointerleave", () => {
    if (A.hover) {
      A.hover = null;
      A.tipAt = null;
      redraw(A);
    }
  });
  cv.addEventListener("click", (ev) => {
    if (A.dragged) return;
    const [px, py] = at(ev);
    const hv = hitTest(A, px, py);
    if (hv) select(A, hv);
  });
  cv.addEventListener("dblclick", (ev) => {
    const [px, py] = at(ev);
    const hv = hitTest(A, px, py);
    if (hv) {
      select(A, hv);
      flyTo(A, hv, true);
    }
  });
  cv.addEventListener("wheel", (ev) => onWheel(A, ev), { passive: false });
  cv.addEventListener("focus", () => {
    byId(A, "at-hint").hidden = true;
  });
  cv.addEventListener("keydown", (ev) => {
    if (A.D === null) return;
    switch (ev.key) {
      case "ArrowLeft":
        panBy(A, PAN_PX, 0);
        break;
      case "ArrowRight":
        panBy(A, -PAN_PX, 0);
        break;
      case "ArrowUp":
        panBy(A, 0, PAN_PX);
        break;
      case "ArrowDown":
        panBy(A, 0, -PAN_PX);
        break;
      case "+":
      case "=":
        zoomAbout(A, ZOOM_STEP, A.vw / 2, A.vh / 2);
        break;
      case "-":
      case "_":
        zoomAbout(A, 1 / ZOOM_STEP, A.vw / 2, A.vh / 2);
        break;
      case "0":
        go(A, fitT(A));
        break;
      default:
        return;
    }
    ev.preventDefault();
  });
  const mm = byId(A, "at-minimap");
  let mmDrag = false;
  const mmGo = (ev) => {
    if (A.D === null) return;
    const r = mm.getBoundingClientRect();
    const K = Math.min(140 / A.D.ww, 100 / WORLD_H);
    const wx = (ev.clientX - r.left - (140 - A.D.ww * K) / 2) / K;
    const wy = (ev.clientY - r.top - (100 - WORLD_H * K) / 2) / K;
    A.flight = null;
    setT(A, { k: A.t.k, x: A.vw / 2 - wx * A.t.k, y: A.vh / 2 - wy * A.t.k });
  };
  mm.addEventListener("pointerdown", (ev) => {
    mmDrag = true;
    if (typeof mm.setPointerCapture === "function") mm.setPointerCapture(ev.pointerId);
    mmGo(ev);
  });
  mm.addEventListener("pointermove", (ev) => {
    if (mmDrag) mmGo(ev);
  });
  mm.addEventListener("pointerup", () => {
    mmDrag = false;
  });
}

// Whether the wheel zooms the map: once it has focus, from a click or the
// keyboard, or with Ctrl or Meta held, which a trackpad's pinch sends.
// Otherwise the wheel scrolls the page, so the detail pane below a short
// window's map can be reached.
function wheelZooms(A, ev) {
  return A.doc.activeElement === A.canvas || ev.ctrlKey || ev.metaKey;
}

function onWheel(A, ev) {
  if (A.D === null) return;
  if (!wheelZooms(A, ev)) {
    hint(A);
    return;
  }
  ev.preventDefault();
  const r = A.canvas.getBoundingClientRect();
  const scale = ev.deltaMode === 1 ? 0.05 : ev.ctrlKey ? 0.01 : 0.002;
  zoomAbout(A, Math.exp(-ev.deltaY * scale), ev.clientX - r.left, ev.clientY - r.top);
}

// Says, for a moment, how to zoom with the wheel.
function hint(A) {
  const h = byId(A, "at-hint");
  h.hidden = false;
  if (A.hintTimer !== null) A.win.clearTimeout(A.hintTimer);
  A.hintTimer = A.win.setTimeout(() => {
    h.hidden = true;
    A.hintTimer = null;
  }, 2500);
}

function typing(t) {
  return t && ["INPUT", "SELECT", "TEXTAREA"].includes(t.tagName);
}

function wireControls(A) {
  byId(A, "at-zin").addEventListener("click", () => zoomAbout(A, 1.6, A.vw / 2, A.vh / 2));
  byId(A, "at-zout").addEventListener("click", () => zoomAbout(A, 1 / 1.6, A.vw / 2, A.vh / 2));
  byId(A, "at-zfit").addEventListener("click", () => go(A, fitT(A)));
  byId(A, "at-back").addEventListener("click", () => histGo(A, -1));
  byId(A, "at-fwd").addEventListener("click", () => histGo(A, 1));
  for (const b of A.doc.querySelectorAll("#at-lens button")) b.addEventListener("click", () => setLens(A, b.dataset.lens));
  for (const b of A.doc.querySelectorAll("#at-win button")) {
    b.addEventListener("click", () => {
      A.span = Number(b.dataset.win);
      renderAll(A);
    });
  }
  byId(A, "at-snap").addEventListener("input", (ev) => {
    if (A.D === null) return;
    const si = A.D.points[Number(ev.target.value)];
    if (!isCount(si)) return;
    moveTo(A, si);
    renderAll(A);
  });
  byId(A, "at-base").addEventListener("change", (ev) => {
    A.base = Number(ev.target.value);
    A.baseChosen = true;
    A.diffCache = null;
    renderAll(A);
  });
  for (const t of ["sel", "chg", "pin"]) {
    byId(A, `at-tab-${t}`).addEventListener("click", () => {
      A.tab = t;
      renderPanel(A);
    });
  }
  byId(A, "at-open-search").addEventListener("click", () => openSearch(A));
  byId(A, "at-qclose").addEventListener("click", () => closeSearch(A));
  const q = byId(A, "at-q");
  q.addEventListener("input", () => renderResults(A));
  q.addEventListener("keydown", (ev) => {
    if (ev.key === "ArrowDown") setActive(A, Math.min(A.results.length - 1, A.active + 1));
    else if (ev.key === "ArrowUp") setActive(A, Math.max(0, A.active - 1));
    else if (ev.key === "Enter") pick(A, A.active);
    else if (ev.key === "Escape") closeSearch(A);
    else return;
    ev.preventDefault();
  });
  // The page-wide keys, only while the Atlas is the section shown.
  A.doc.addEventListener("keydown", (ev) => {
    if (!shown(A)) return;
    const busy = typing(ev.target);
    if (ev.key === "/" && !busy) {
      ev.preventDefault();
      openSearch(A);
    } else if (ev.key === "Escape" && !byId(A, "at-search").hidden) closeSearch(A);
    else if (ev.key === "Escape" && !busy && A.sel) select(A, null);
    else if (ev.key === "[" && !busy) histGo(A, -1);
    else if (ev.key === "]" && !busy) histGo(A, 1);
  });
  A.doc.addEventListener("pointerdown", (ev) => {
    const s = byId(A, "at-search");
    if (!s.hidden && !s.contains(ev.target) && ev.target !== byId(A, "at-open-search")) closeSearch(A);
  });
}

export async function wireAtlas(doc, win, tauri) {
  const canvas = doc.getElementById("at-canvas");
  const A = {
    doc,
    win,
    tauri,
    canvas,
    ctx: canvas.getContext("2d"),
    P: {},
    hatches: new Map(),
    inks: new Map(),
    widths: new Map(),
    labels: [],
    drawn: [],
    view: null,
    D: null,
    root: null,
    version: null,
    childUp: false,
    asked: 0,
    askedHistory: false,
    askedSymbols: false,
    inflight: null,
    si: -1,
    base: -1,
    lens: "structure",
    span: 1,
    sel: null,
    F: null,
    hover: null,
    tipAt: null,
    hist: { list: [], pos: -1 },
    tab: "sel",
    symSort: { key: "cin", desc: true },
    t: { k: 1, x: 0, y: 0 },
    vw: 0,
    vh: 0,
    dpr: 1,
    fitted: false,
    flight: null,
    queued: false,
    edgeNote: "",
    lastCrumbs: "",
    dragged: false,
    results: [],
    active: 0,
    lastFocus: null,
    cands: null,
    diffCache: null,
    pins: new Set(),
    radiusFly: null,
    baseChosen: false,
    candsKey: null,
    hintTimer: null,
    reduced: win.matchMedia("(prefers-reduced-motion: reduce)"),
  };
  readPalette(A);
  wirePointer(A);
  wireControls(A);
  // The desktop has no theme toggle: the system's setting decides, and the
  // map is redrawn in its colours when it changes.
  win.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
    readPalette(A);
    redraw(A);
  });
  if (typeof win.ResizeObserver === "function") new win.ResizeObserver(() => resize(A)).observe(doc.getElementById("at-map"));
  renderAll(A);

  await tauri.event.listen("view", (event) => {
    const view = event.payload;
    if (view !== null && typeof view === "object" && "atlas" in view) takeView(A, view.atlas);
  });
  await tauri.event.listen("child-started", (event) => {
    const c = event.payload || {};
    A.root = str(c.root);
    A.version = str(c.version);
    A.childUp = true;
    reset(A, "Another repository is open: its atlas is read when this section is shown.");
    pump(A);
  });
  await tauri.event.listen("rpc-frame", (event) => {
    const f = event.payload;
    if (f !== null && typeof f === "object" && f.type === "response" && f.command === "get_state" && f.success === true && f.data) setRoot(A, f.data.root);
  });
  await tauri.event.listen("rpc-exited", () => {
    A.childUp = false;
    A.inflight = null;
  });
  await tauri.event.listen("no-child", () => {
    A.childUp = false;
    if (A.view === null) status(A, "No repository is open: open a repository to see its atlas.");
  });
  const button = doc.querySelector('nav.rail button[aria-controls="atlas"]');
  if (button) {
    button.addEventListener("click", () => {
      // The model pane, open above every section until a model is chosen,
      // would push the map below the fold; showing the Atlas folds it, and
      // its summary opens it again.
      const pane = doc.getElementById("model-pane");
      if (pane && pane.open) pane.open = false;
      // After every click listener, the rail's among them, has run, so the
      // section is shown when it is measured. A page loaded after its child
      // started saw no child-started; asking is what tells whether one is
      // there.
      Promise.resolve().then(() => {
        A.childUp = true;
        resize(A);
        pump(A);
      });
    });
  }
  return A;
}

if (typeof window !== "undefined" && window.__TAURI__) {
  start("the Atlas", () => wireAtlas(document, window, window.__TAURI__));
}
