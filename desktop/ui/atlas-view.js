// The Atlas's view of what the core sent (atlas.js's section "the view,
// indexed", split out of atlas.js by #955): the tables it looks figures up
// in, the colours it reads from the stylesheet, where the map is zoomed and
// panned to, and what a selection marks. Every figure here is one the core
// sent, or a list it sent, compared; nothing divides one count by another
// (CLAUDE.md rule 3, ADR 0014).

import { crumbs } from "./atlas-toolbar.js";
import { redraw } from "./atlas-draw.js";
import { isCount, str } from "./fmt.js";

// The world the core's normalised layout is drawn in: its height, and its
// width by the layout's aspect.
export const WORLD_H = 1000;
// A module opens to its symbols once it is this many pixels each way.
export const OPEN_W = 190;
export const OPEN_H = 130;
const FLIGHT_MS = 450;

export const SEL = 1;
export const CALLER = 2;
export const CALLEE = 4;
export const REACH = 8;
export const DOUBT = 16;
export const COMMIT = 32;

// A tint on the map is the -soft twin of its colour, an opaque fill the
// stylesheet names, never an alpha the Atlas computes over the base (#955).
const TOKENS = [
  "ground", "ink", "ink-muted", "line", "panel", "panel-inset", "signal", "signal-soft", "region",
  "region-test", "region-head", "region-head-test", "cell", "cell-test", "cell-line", "edge", "caller",
  "caller-soft", "callee", "callee-soft", "reach", "reach-soft", "doubt", "diff-add", "diff-add-soft",
  "diff-del", "churn-1", "churn-2", "churn-3", "churn-4", "churn-5",
];

const WINDOWS = ["d30", "d90", "d365", "commits"];

// --- small helpers ---------------------------------------------------------
// el, isCount, notReported, plural and str are the page's shared ones
// (dom.js, fmt.js); these are the Atlas's own.

export function list(v) {
  return Array.isArray(v) ? v : [];
}

export function day(s) {
  return typeof s === "string" ? s.slice(0, 10) : "date not reported";
}

export function short(s) {
  return typeof s === "string" ? s.slice(0, 7) : "";
}

export function byId(A, id) {
  return A.doc.getElementById(id);
}

// --- colours ---------------------------------------------------------------

// Every colour is one of style.css's custom properties, read here at draw
// time; the Atlas names none of its own.
export function readPalette(A) {
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

function lum(hex) {
  const c = rgb(hex).map((v) => {
    const x = v / 255;
    return x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
}

// Text on a fill takes whichever of the two text colours reads better on it.
export function inkOn(A, fill) {
  let v = A.inks.get(fill);
  if (v) return v;
  const lf = lum(fill);
  const contrast = (a, b) => (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
  v = contrast(lf, lum(A.P.ink)) >= contrast(lf, lum(A.P.ground)) ? A.P.ink : A.P.ground;
  A.inks.set(fill, v);
  return v;
}

export function hatch(A, color) {
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
export function index(v) {
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

export function snap(A, si) {
  return A.D && A.D.S[si] ? A.D.S[si] : null;
}

export function rowAt(A, si, m) {
  const s = snap(A, si);
  return s ? s.rows.get(m) ?? null : null;
}

export function present(A, si, m) {
  return rowAt(A, si, m) !== null;
}

export function label(A, si) {
  const s = snap(A, si);
  return s ? str(s.s.label) ?? "snapshot" : "snapshot";
}

export function isHead(A) {
  return A.D !== null && A.si === A.D.head;
}

// The query get_radius takes for a symbol: its module and its name.
export function radiusQuery(A, i) {
  const s = A.D.symbols[i];
  const m = s && A.D.modules[s.m];
  return s && m ? `${m.module}.${s.name}` : null;
}


// --- zoom and pan ------------------------------------------------------------

export function fitT(A) {
  const pad = 10;
  const ww = A.D ? A.D.ww : 1400;
  const k = Math.max(1e-6, Math.min((A.vw - 2 * pad) / ww, (A.vh - 2 * pad) / WORLD_H));
  return { k, x: (A.vw - ww * k) / 2, y: Math.max(pad, (A.vh - WORLD_H * k) / 2) };
}

function kRange(A) {
  const f = fitT(A).k;
  return [f * 0.5, Math.max(f * 160, 60)];
}

export function clampT(A, t) {
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

export function setT(A, t) {
  A.t = clampT(A, t);
  redraw(A);
  crumbs(A);
}

export function toward(A, x0, y0, x1, y1, fill, minK) {
  let k = Math.min((A.vw * fill) / Math.max(1e-6, x1 - x0), (A.vh * fill) / Math.max(1e-6, y1 - y0));
  if (minK) k = Math.max(k, minK);
  const [lo, hi] = kRange(A);
  k = Math.max(lo, Math.min(hi, k));
  return { k, x: A.vw / 2 - ((x0 + x1) / 2) * k, y: A.vh / 2 - ((y0 + y1) / 2) * k };
}

// Moves the view to `t`: at once under prefers-reduced-motion, or in a
// short flight that keeps the point under the centre moving smoothly.
export function go(A, t) {
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

export function zoomAbout(A, f, px, py) {
  const t = A.t;
  const [lo, hi] = kRange(A);
  const k = Math.max(lo, Math.min(hi, t.k * f));
  const wx = (px - t.x) / t.k;
  const wy = (py - t.y) / t.k;
  A.flight = null;
  setT(A, { k, x: px - wx * k, y: py - wy * k });
}

export function panBy(A, dx, dy) {
  A.flight = null;
  setT(A, { k: A.t.k, x: A.t.x + dx, y: A.t.y + dy });
}

export function scr(A, r) {
  const t = A.t;
  return [r.x0 * t.k + t.x, r.y0 * t.k + t.y, r.x1 * t.k + t.x, r.y1 * t.k + t.y];
}

export function onScreen(A, r) {
  return r[2] >= 0 && r[0] <= A.vw && r[3] >= 0 && r[1] <= A.vh;
}

export function openK(m) {
  return Math.max(OPEN_W / (m.x1 - m.x0), OPEN_H / (m.y1 - m.y0)) * 1.05;
}

// --- what is selected, and what it marks -------------------------------------

export function focus(A) {
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

export function churnCount(A, m) {
  const c = A.D.churn.get(m);
  // get_history lists every path a commit changed in the last 365 days or
  // since the oldest tag it lists (its contract), so a path it does not
  // list had none in either; a count it sent as null stays unknown, as
  // does "since" with no tag listed.
  if (c === undefined) return A.span < 3 || (A.D.history && A.D.history.since) ? 0 : null;
  const v = c[WINDOWS[A.span]];
  return isCount(v) ? v : null;
}

export function churnBin(v) {
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
export function diff(A) {
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

