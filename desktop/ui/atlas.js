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
//
// The map and the detail column's three Panels are the design system's
// Panels as index.html serves them, and the detail figures are its Stats,
// each with its source as the label's meta (ds.js, #910). The map's drawing
// is the Atlas's own.
//
// This module asks trellis, takes the core's view and wires the page's
// events; the rest is split along atlas.js's old sections (#955):
// atlas-view.js indexes the view, holds zoom and pan and what a selection
// marks; atlas-draw.js draws the map; atlas-detail.js the selection and the
// detail column; atlas-toolbar.js the legend, toolbar, breadcrumbs and
// working set; atlas-search.js the search.

import { flyTo, flyToRadius, histGo, select } from "./atlas-detail.js";
import { hitTest, redraw, selKey } from "./atlas-draw.js";
import { closeSearch, indexSearch, openSearch, pick, renderResults, setActive } from "./atlas-search.js";
import { crumbs, loadPins, oldestBase, renderAll, setLens, setRoot } from "./atlas-toolbar.js";
import { byId, clampT, fitT, focus, go, index, label, list, panBy, readPalette, setT, WORLD_H, zoomAbout } from "./atlas-view.js";
import { isCount, str } from "./fmt.js";
import { onEscape, typing } from "./keys.js";
import { onShown, send, start } from "./rpc.js";

const ID = "desktop-atlas-";
// get_codebase lists this many modules: all of any repository the layout
// draws (rpc's own default is 40).
const MODULES_ASKED = 100000;
// Every tag get_history gives, up to its cap: the core picks the snapshots
// from those dated in the two years before HEAD (atlas.rs).
const HISTORY_ASKED = { commits: 20, tags: 200 };
const ZOOM_STEP = 1.5;
const PAN_PX = 70;
const DRAG_PX = 4;

// --- asking trellis ----------------------------------------------------------

function shown(A) {
  const s = byId(A, "atlas");
  return s !== null && !s.hidden;
}

export function status(A, text) {
  byId(A, "at-status").textContent = text;
}

// Sends one line through the core (rpc.js's send). A refusal drops the
// question in flight and says why in the Atlas's status line.
export function forward(A, line) {
  return send(A.tauri, line, (why) => {
    A.inflight = null;
    status(A, `Open a repository to see its atlas. ${why}`);
  });
}

export function nextId(A, kind) {
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

export function progressText(A) {
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

// --- taking a view -----------------------------------------------------------

export function moveTo(A, si) {
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
    } else if (ev.key === "[" && !busy) histGo(A, -1);
    else if (ev.key === "]" && !busy) histGo(A, 1);
  });
  // ESC is keys.js's for the whole page (#826): the Atlas is asked first,
  // and closes its search, then clears its selection, before ESC goes out
  // a level.
  onEscape((ev) => {
    if (!shown(A)) return false;
    if (!byId(A, "at-search").hidden) {
      closeSearch(A);
      return true;
    }
    if (!typing(ev.target) && A.sel) {
      select(A, null);
      return true;
    }
    return false;
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
  // Entering the Atlas's level shows its section (keys.js), and the Atlas
  // measures and asks then (#826).
  onShown(doc, "atlas", () => {
    // The model pane, open above every section until a model is chosen,
    // would push the map below the fold; showing the Atlas folds it, and
    // its summary opens it again.
    const pane = doc.getElementById("model-pane");
    if (pane && pane.open) pane.open = false;
    // The section is shown by now, so it can be measured. A page loaded
    // after its child started saw no child-started; asking is what tells
    // whether one is there.
    A.childUp = true;
    resize(A);
    pump(A);
  });
  return A;
}

if (typeof window !== "undefined" && window.__TAURI__) {
  start("the Atlas", () => wireAtlas(document, window, window.__TAURI__));
}
