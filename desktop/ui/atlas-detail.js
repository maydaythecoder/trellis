// The Atlas's selection and its detail column (atlas.js's sections
// "selection and history" and "the detail pane", split out of atlas.js by
// #955): what is selected and the way back to what was, and the three
// Panels beside the map, the selection, the changes and the working set,
// drawn together with no tabs between them (DESIGN.md: no tabs). Their
// frames and heads are index.html's, their figures ds.js's Stats with each
// source as the label's meta (#910). Repository and git text reaches the
// page through textContent alone (ADR 0007, S10).

import { forward, nextId, progressText } from "./atlas.js";
import { selKey } from "./atlas-draw.js";
import { renderAll, setLens, togglePin } from "./atlas-toolbar.js";
import { byId, day, diff, fitT, focus, go, isHead, label, list, openK, present, radiusQuery, rowAt, short, snap, toward } from "./atlas-view.js";
import { el } from "./dom.js";
import { panelMeta, stats, unreported } from "./ds.js";
import { isCount, notReported, plural, str } from "./fmt.js";

// --- selection and history ---------------------------------------------------

export function select(A, s, o = {}) {
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
  if (o.fly && A.F && A.F.radius) {
    A.radiusFly = null;
    flyToRadius(A);
  } else if (o.fly) flyTo(A, A.sel, false);
  renderAll(A);
}

export function flyToRadius(A) {
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

export function flyTo(A, s, deep) {
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

export function histGo(A, d) {
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

export function src(A, s) {
  const e = el(A.doc, "span", s, "at-src");
  e.title = SOURCES[s] || s;
  return e;
}

function nv(A, v, s) {
  const e = el(A.doc, "span");
  e.append(el(A.doc, "span", notReported(v)));
  if (s) e.append(src(A, s));
  return e;
}

// One figure as a Stat for ds.js's stats: `label` over the count, its
// source (`s`) as the label's meta with what that source is as the title,
// and the dash with `why` where the count was not reported, never 0. A
// `value` already worded (a before and after) is drawn as given.
function fig(A, label, v, s, why = "trellis's answer did not carry it") {
  return {
    label,
    meta: s || null,
    value: typeof v === "string" ? v : isCount(v) ? notReported(v) : unreported(A.doc, why),
    title: s ? SOURCES[s] : null,
  };
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

export function swatch(A, cls) {
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
    li.append(bar, modLink(A, m), el(A.doc, "span", notReported(w), "at-n"));
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

// Each view answers what the detail Panel shows: its head's title and meta,
// and its body.
function overviewView(A) {
  const S = snap(A, A.si);
  if (!S) return { title: "Atlas", body: [note(A, progressText(A))] };
  const s = S.s;
  // get_symbols reads HEAD: its definitions and the sites bound to them by
  // a Resolved edge (each definition's calls_in, summed by the core). At an
  // earlier snapshot the definitions are its modules' defines, summed.
  const sy = isHead(A) && A.view.symbols && !str(A.view.symbols.error) ? A.view.symbols : null;
  const defs = sy ? fig(A, "definitions", sy.definitions, "trellis") : fig(A, "definitions", s.definitions, "derived");
  const resolved = sy
    ? fig(A, "resolved call sites", sy.resolved_call_sites, "derived")
    : fig(A, "resolved call sites", null, null, isHead(A) ? "HEAD's symbols are not read yet" : "read at HEAD only");
  const subject = el(A.doc, "p");
  subject.append(el(A.doc, "code", short(s.commit)), ` ${day(s.date)} `, el(A.doc, "span", str(s.subject) ?? "", "muted"));
  const out = [
    subject,
    stats(A.doc, [fig(A, "modules indexed", s.modules_indexed, "trellis"), defs, fig(A, "module edges", s.module_edges, "trellis")]),
    h4(A, "First-party call sites"),
    stats(A.doc, [resolved, fig(A, "ambiguous", s.ambiguous, "trellis"), fig(A, "unresolved", s.unresolved, "trellis")]),
    note(A, "Each counts its own sites: no total and no share."),
    h4(A, "The map's edges"),
    stats(A.doc, [fig(A, "call sites bound across modules", s.bound_call_sites, "trellis")]),
    note(A, "Click a module for what calls it and what it calls, a package header for its figures, a symbol (zoom in on a module) for its radius. Press / to search."),
  ];
  if (isCount(s.modules_not_drawn) && s.modules_not_drawn > 0) {
    out.push(note(A, `${s.modules_not_drawn} modules past the limit were not listed by trellis, so they are not on the map.`));
  }
  if (list(s.not_placed).length > 0) {
    out.push(note(A, `${list(s.not_placed).length} modules in this snapshot have no place on the map, which was fixed before they appeared: ${list(s.not_placed).slice(0, 5).join(", ")}`));
  }
  return { title: `Snapshot ${s.label}`, body: out };
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
      stats(A.doc, [fig(A, "definitions", r[1], "trellis"), fig(A, "call sites bound in, from other modules", r[5], "derived")]),
      h4(A, "Call sites made here"),
      stats(A.doc, [
        fig(A, floor ? "bound to another module, at least" : "bound to another module", r[7], "derived"),
        fig(A, "ambiguous", r[2], "trellis"),
        fig(A, "unresolved", r[3], "trellis"),
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
    // Commits in each window, each its own count: the windows nest (the
    // last 30 days are inside the last 90), so they are never added.
    const windows = [fig(A, "30 days", c.d30), fig(A, "90 days", c.d90), fig(A, "365 days", c.d365)];
    if (since !== null) windows.push(fig(A, `since ${since}`, c.commits));
    out.push(stats(A.doc, windows), note(A, `Commits that touched it. Last change ${day(c.last_date)}.`));
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
  return { title: "Module", body: out };
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
    stats(A.doc, [
      fig(A, "definitions", `${notReported(o[1])} → ${notReported(r[1])}`, "trellis"),
      fig(A, "ambiguous call sites", `${notReported(o[2])} → ${notReported(r[2])}`, "trellis"),
      fig(A, "unresolved call sites", `${notReported(o[3])} → ${notReported(r[3])}`, "trellis"),
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
    li.append(el(A.doc, "span", tag, "at-tag"), e[0] === m ? to : from, modLink(A, e[0] === m ? e[1] : e[0]), el(A.doc, "span", notReported(e[2]), "at-n"));
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
    tr.append(first, el(A.doc, "td", notReported(s.line)), el(A.doc, "td", notReported(s.cin)), el(A.doc, "td", notReported(s.camb)), el(A.doc, "td", notReported(s.cout)));
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
  where.append("in ", modLink(A, s.m), `, lines ${notReported(s.line)}–${notReported(s.end)}`, src(A, "trellis"));
  const out = [
    title,
    where,
    h4(A, "Call sites, three counts", "trellis"),
    stats(A.doc, [
      fig(A, "bound to it (in)", s.cin),
      fig(A, "naming it among several (ambiguous in)", s.camb),
      fig(A, "bound from it (out)", s.cout),
    ]),
    (() => {
      const p = el(A.doc, "p", null, "at-note");
      p.append("trellis names it ", el(A.doc, "code", q ?? s.name), ".");
      return p;
    })(),
    h4(A, "Radius"),
  ];
  const r = A.view.radius;
  const view = { title: "Symbol", body: out };
  if (!isHead(A)) {
    out.push(note(A, "Symbols and radius are read at HEAD. Move the snapshot to HEAD to see the radius on the map."));
    return view;
  }
  if (!r || r.symbol !== q) {
    out.push(note(A, "Select it on the map, or from a list, to ask trellis for its radius."));
    return view;
  }
  if (r.state === "asked") {
    out.push(note(A, `Asking trellis for the radius of ${q}: it indexes the repository for it.`));
    return view;
  }
  if (r.state === "refused") {
    out.push(note(A, `trellis gave no radius: ${r.error}`));
    return view;
  }
  const a = r.answer;
  const reach = a.reach || {};
  const doubt = a.doubt || {};
  // Reach and doubt, each a sub-region of the panel with its map colour's
  // swatch, never a frame inside the frame.
  const rb = el(A.doc, "section", null, "at-rd-block");
  rb.setAttribute("aria-label", "Reach");
  rb.append(
    h4(A, "Reach", null, "reach"),
    stats(A.doc, [
      fig(A, "resolved call sites", reach.resolved, "trellis"),
      fig(A, "ambiguous call sites", reach.ambiguous, "trellis"),
      fig(A, "symbols reached", reach.symbols_reached, "trellis"),
      fig(A, "modules reached", list(a.reached_modules).length, "derived"),
    ]),
  );
  const db = el(A.doc, "section", null, "at-rd-block");
  db.setAttribute("aria-label", "Doubt");
  db.append(
    h4(A, "Doubt", null, "doubt"),
    stats(A.doc, [
      fig(A, "unbound call sites in touched files", doubt.unbound_in_touched_files, "trellis"),
      fig(A, "touched files", doubt.touched_files, "trellis"),
    ]),
  );
  out.push(rb, db, note(A, "Reach and doubt count different populations and are never divided; reach is a floor, not a boundary."));
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
  return view;
}

function regionView(A, g) {
  const S = snap(A, A.si);
  const c = S ? list(S.s.regions)[g.idx] : null;
  const v = Array.isArray(c) ? c : [];
  const path = el(A.doc, "p", null, "at-title");
  path.append(el(A.doc, "code", g.path));
  return {
    title: "Package",
    body: [
      path,
      stats(A.doc, [fig(A, `modules at ${label(A, A.si)}`, v[0], "trellis"), fig(A, "definitions", v[1], "derived")]),
      h4(A, "Call sites made inside, two of the three counts", "derived"),
      stats(A.doc, [fig(A, "ambiguous", v[2]), fig(A, "unresolved", v[3])]),
      note(A, "Each summed over the package's modules at this snapshot. Click a module for its callers and callees."),
    ],
  };
}

function commitView(A, ci) {
  const c = A.D.commits[ci];
  const ms = list(c.modules);
  const others = list(c.other_paths);
  const out = [
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
  return { title: "Commit", meta: [src(A, "git")], body: out };
}

function changesView(A) {
  const out = [];
  const view = { title: "Changes", body: out };
  if (A.lens === "diff") {
    const D = diff(A);
    view.title = `Diff ${label(A, D.b)} → ${label(A, D.t)}`;
    if (D.b === D.t) out.push(note(A, "Baseline and snapshot are the same. Pick another baseline or move the snapshot."));
    out.push(
      stats(A.doc, [
        fig(A, "modules added", D.added.size, "trellis"),
        fig(A, "modules removed", D.removed.size, "trellis"),
        fig(A, "modules with changed counts", D.changed.length, "trellis"),
      ]),
      stats(A.doc, [fig(A, "module edges added", D.addedE.length, "trellis"), fig(A, "module edges removed", D.removedE.length, "trellis")]),
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
        li.append(modLink(A, e[0]), "→", modLink(A, e[1]), el(A.doc, "span", notReported(e[2]), "at-n"));
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
    return view;
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
  return view;
}

function pinsView(A) {
  const out = [];
  const view = { title: "Working set", meta: A.pins.size > 0 ? [plural(A.pins.size, "module", "modules")] : [], body: out };
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
    return view;
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
  return view;
}

// Draws one of the detail column's Panels: the view's title and meta in
// the head index.html serves, its body below.
function fillPanel(A, [body, title, meta], view) {
  byId(A, title).textContent = view.title;
  panelMeta(A.doc, meta, ...(view.meta ?? []));
  byId(A, body).replaceChildren(...view.body.filter(Boolean));
}

// The detail column: the selection, the changes and the working set, three
// Panels one under another in the Atlas's level, where they were three tabs
// over one Panel (#955). The selection's Panel goes back to its top when it
// is drawn again, since it may now hold another module; the other two keep
// their scroll, so picking a commit from the changes leaves the list where
// it was.
export function renderPanel(A) {
  let view;
  if (A.D === null) view = { title: "Atlas", body: [note(A, progressText(A))] };
  else if (!A.sel) view = overviewView(A);
  else if (A.sel.t === "module") view = moduleView(A, A.sel.m);
  else if (A.sel.t === "symbol") view = symbolView(A, A.sel.i);
  else if (A.sel.t === "region") view = regionView(A, A.D.regions[A.sel.r]);
  else view = commitView(A, A.sel.c);
  fillPanel(A, ["at-detail", "at-detail-title", "at-detail-meta"], view);
  byId(A, "at-detail").scrollTop = 0;
  const later = (title) => ({ title, body: [note(A, "Listed once the map is drawn.")] });
  fillPanel(A, ["at-chg", "at-chg-title", "at-chg-meta"], A.D === null ? later("Changes") : changesView(A));
  fillPanel(A, ["at-pin", "at-pin-title", "at-pin-meta"], A.D === null ? later("Working set") : pinsView(A));
}
