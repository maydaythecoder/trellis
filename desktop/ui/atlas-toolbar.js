// The Atlas's legend, toolbar and breadcrumbs, and its working set
// (atlas.js's sections "the legend, the toolbar and the breadcrumbs" and
// "the working set", split out of atlas.js by #955): what the map's marks
// mean for the lens and selection shown, the lens, window, baseline and
// snapshot controls, the zoom path, and the modules a person pinned, kept
// in this computer's storage only.

import { progressText, status } from "./atlas-ask.js";
import { renderPanel, src, swatch } from "./atlas-detail.js";
import { redraw } from "./atlas-draw.js";
import { byId, CALLER, day, diff, fitT, focus, go, label, list, short, snap, toward } from "./atlas-view.js";
import { el } from "./dom.js";
import { isCount, notReported, plural } from "./fmt.js";

const CHURN_BINS = ["0", "1", "2", "3–4", "5–8", "9+"];
const PIN_KEY = "trellis-desktop.atlas.working-set.v1";

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
    rows.push(lgRow(A, "doubt", `Doubt: ${notReported((F.radius.doubt || {}).touched_files)} touched files`, "(trellis)"));
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
      ` ${short(s.commit)} · ${day(s.date)} · ${notReported(s.modules_indexed)} modules · ${notReported(s.module_edges)} module edges`,
      src(A, "trellis"),
    );
    range.setAttribute("aria-valuetext", `${s.label}, ${day(s.date)}`);
  } else readout.replaceChildren();
  const head = A.D ? snap(A, A.D.head) : null;
  const parts = [rootName(A), head ? `HEAD ${short(head.s.commit)}` : "", A.version].filter((x) => x);
  byId(A, "at-where").textContent = parts.join(" · ");
}

export function crumbs(A) {
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

export function renderAll(A) {
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
export function oldestBase(A) {
  const p = A.D.points;
  const oldest = A.D.points[0];
  return oldest !== A.si || p.length < 2 ? oldest ?? A.si : p[p.length - 1];
}

export function moveTo(A, si) {
  A.si = si;
  if (A.D && !A.baseChosen) A.base = oldestBase(A);
  A.diffCache = null;
  A.F = focus(A);
}

export function setLens(A, l) {
  A.lens = l;
  A.diffCache = null;
  if (l === "diff" && A.D && !A.baseChosen) A.base = oldestBase(A);
  A.F = focus(A);
  renderAll(A);
}

// The open repository's directory name, or null while none is known.
function rootName(A) {
  return A.root ? A.root.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || A.root : null;
}

// The root, from child-started or, for a page that loaded after its child
// started and so heard none, from the child's answer to get_state.
export function setRoot(A, root) {
  if (typeof root !== "string" || root === "" || root === A.root) return;
  A.root = root;
  A.pins = loadPins(A);
  A.lastCrumbs = "";
  renderAll(A);
}

// --- the working set ---------------------------------------------------------

// Pins are paths, kept per repository in localStorage. A page where storage
// is refused still works: the set then lasts this visit.
export function loadPins(A) {
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

export function togglePin(A, path) {
  if (A.pins.has(path)) A.pins.delete(path);
  else A.pins.add(path);
  savePins(A);
  renderPanel(A);
  redraw(A);
}

