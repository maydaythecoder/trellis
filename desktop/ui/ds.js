// The design system's components, as the page draws them (#909).
//
// The owner's design system (docs/design/system/) ships its components as
// React on window.Trellis with their styles in components/bundle.css. The
// page loads that stylesheet itself, as ds.css, a byte-for-byte copy held
// by ds_css_is_the_design_systems_bundle in src/lockdown.rs, and this
// module builds each component's markup the way the bundle's code does,
// with the same class names, so the system's styles apply unchanged. The
// page cannot load React (ADR 0048 section 5: no npm package, no build
// step), so each component is a function that answers an element.
//
// Where the bundle sets a `style` this module sets a class: the CSP
// refuses a style attribute (page_has_no_style_attribute). A component
// joins this module when a level first draws it, so it holds only what the
// page uses.
//
// Every value reaches the DOM as text, through dom.js's el or append with
// a string, never parsed as markup (ADR 0007, S10).
//
// A component used by one module only is still built here, so a level that
// draws a Stat, a Status or a Rail never draws its own copy of one
// (ds_components_draw_the_bundles_markup, page_helpers_live_in_one_module_each).

import { el } from "./dom.js";
import { isCount, str } from "./fmt.js";

// The class list from the names given, without the falsy ones.
function cx(...names) {
  return names.filter(Boolean).join(" ");
}

// `node` as is when it is an element, otherwise as text.
function part(doc, node) {
  return node !== null && typeof node === "object" ? node : doc.createTextNode(String(node ?? ""));
}

// Status (the design system's): a dot and its word, never a dot alone.
// Tones: ok, down, warn, signal, cool, idle. Named dsStatus because several
// levels have a status line of their own called status.
export function dsStatus(doc, tone, word, { tag, live, title } = {}) {
  const s = el(doc, "span", null, cx("cd-status", `cd-status-${tone || "idle"}`, tag && "cd-status-tag", live && "cd-status-live"));
  const dot = el(doc, "span", null, "cd-status-dot");
  dot.setAttribute("aria-hidden", "true");
  s.append(dot, part(doc, word));
  if (title) s.title = title;
  return s;
}

// A figure trellis did not report: a dash, never 0 (DESIGN.md rule 2),
// with why as the element's title and as text a screen reader reads.
export function unreported(doc, why) {
  const s = el(doc, "span", null, "unreported");
  s.title = `not reported: ${why}`;
  const dash = el(doc, "span", "—");
  dash.setAttribute("aria-hidden", "true");
  s.append(dash, el(doc, "span", `not reported: ${why}`, "visually-hidden"));
  return s;
}

// `v` as text when trellis reported it, else unreported for `why`.
export function figure(doc, v, why) {
  return isCount(v) ? String(v) : unreported(doc, why);
}

// Stat: an uppercase label, a numeral and its unit. `value` is text or an
// element (unreported's dash); a unit follows a figure, never a dash.
// `meta` is text or an element (where the figure came from). `size` "sm"
// is the 36px numeral. `inline` builds it from spans, for a Stat inside a
// button, which holds phrasing content only; ds.css sets each part's
// display, so it draws the same.
export function stat(doc, { label, meta, value, unit, footnote, size, title, inline }) {
  const box = inline ? "span" : "div";
  const root = el(doc, box, null, cx("cd-stat", size === "sm" && "cd-stat-sm"));
  const head = el(doc, box, null, "cd-stat-label");
  head.append(el(doc, "span", label));
  if (meta) {
    const m = el(doc, "span", null, "cd-stat-unit");
    m.append(part(doc, meta));
    head.append(m);
  }
  const value_ = el(doc, "span", null, "cd-stat-value");
  value_.append(part(doc, value));
  if (unit && typeof value === "string") value_.append(el(doc, "span", unit, "cd-stat-unit"));
  const row = el(doc, box, null, "cd-stat-value-row");
  row.append(value_);
  root.append(head, row);
  if (footnote) {
    const foot = el(doc, box, null, "cd-stat-foot");
    foot.append(el(doc, "span"), el(doc, "span", footnote));
    root.append(foot);
  }
  if (title) root.title = title;
  return root;
}

// Stats side by side, as the bundle's previews set them in a cd-row.
export function stats(doc, list) {
  const row = el(doc, "div", null, "cd-row stat-row");
  row.append(...list.map((s) => stat(doc, { size: "sm", ...s })));
  return row;
}

// Rail rows: a timeline's entries, one row each, `{label, meta, tone,
// current, title}`; a label or a meta is text or an element. Tones: ok,
// down, signal, cool. A list element that is itself a Rail (`ol.cd-rail`)
// takes these rows directly.
export function railRows(doc, rows) {
  return rows.map((r) => {
    const li = el(doc, "li", null, cx("cd-rail-row", r.tone && `cd-rail-${r.tone}`, r.current && "cd-rail-current"));
    const label = el(doc, "span");
    label.append(part(doc, r.label));
    li.append(label);
    if (r.meta) {
      const meta = el(doc, "span", null, "cd-rail-meta");
      meta.append(part(doc, r.meta));
      li.append(meta);
    }
    if (r.title) li.title = r.title;
    return li;
  });
}

// A glyph that marks a row's kind: drawn, and not read, since the row's
// words say what it is.
function glyph(doc, g) {
  const e = el(doc, "span", g, "cd-tx-g");
  e.setAttribute("aria-hidden", "true");
  return e;
}

// Where a steer stands, as its row's meta says it: pending until a model
// request carries it, then the step it followed, or that none did.
function delivery(doc, d) {
  const state = d && str(d.state);
  if (state === "delivered" && isCount(d.after_step)) return el(doc, "span", `delivered after turn ${d.after_step}`, "cd-tx-meta");
  if (state === "not_delivered") {
    const e = el(doc, "span", "not delivered", "cd-tx-meta");
    e.title = str(d.why) ?? "trellis gave no reason";
    return e;
  }
  return el(doc, "span", "pending · next model call", "cd-tx-meta");
}

// Transcript (the design system's): one entry of a run's log, typed by what
// it came from (#827). The person's task and steers are signal blocks, a
// steer saying where it stands; a tool call is one line, its raw call and
// observation shown on demand (render.js's wireLog opens it, on one
// listener for the whole log); a check row carries trellis-checks' verdict
// and nothing else; a note and a refusal are trellis's words.
export function txEntry(doc, row) {
  const kind = str(row.kind);
  const text = str(row.text) ?? "";
  const turn = isCount(row.turn) ? `t${row.turn}` : "";
  if (kind === "task" || kind === "steer") {
    const pending = kind === "steer" && !(row.delivery && str(row.delivery.state) !== "pending");
    const e = el(doc, "div", null, cx("cd-tx-e", `cd-tx-${kind}`, pending && "cd-tx-steer-pending"));
    const words = el(doc, "div");
    words.append(el(doc, "span", kind === "task" ? "you · task" : "you · steer", "cd-tx-label"), el(doc, "div", text, "cd-tx-body"));
    e.append(glyph(doc, "◆"), words, kind === "steer" ? delivery(doc, row.delivery) : el(doc, "span", "", "cd-tx-meta"));
    return e;
  }
  if (kind === "tool") {
    const e = el(doc, "div", null, "cd-tx-e cd-tx-tool");
    e.tabIndex = 0;
    e.setAttribute("aria-expanded", "false");
    const space = text.indexOf(" ");
    const sent = el(doc, "span", null, "cd-tx-sent");
    if (space < 0) sent.append(el(doc, "b", text));
    else sent.append(el(doc, "b", text.slice(0, space)), text.slice(space));
    const raw = el(doc, "pre", null, "cd-tx-raw");
    raw.hidden = true;
    raw.append(el(doc, "span", "$ ", "k"), text);
    const detail = str(row.detail);
    if (detail !== null) raw.append(`\n${detail}`);
    e.append(glyph(doc, "→"), sent, el(doc, "span", turn, "cd-tx-meta"), raw);
    return e;
  }
  if (kind === "check" && typeof row.passed === "boolean") {
    const passed = row.passed;
    const e = el(doc, "div", null, cx("cd-tx-e", passed ? "cd-tx-check-ok" : "cd-tx-check-down"));
    e.append(glyph(doc, passed ? "✓" : "✗"), el(doc, "span", text, "cd-tx-sent"), el(doc, "span", turn, "cd-tx-meta"));
    return e;
  }
  const e = el(doc, "div", null, cx("cd-tx-e", kind === "refusal" ? "cd-tx-refusal" : "cd-tx-system"));
  e.append(glyph(doc, kind === "refusal" ? "×" : "!"), el(doc, "span", text, "cd-tx-sent"), el(doc, "span", turn, "cd-tx-meta"));
  return e;
}

// The model's words (the Transcript card's `model` entry, #832): the
// agent's own text under a label saying whose it is, plain sans body,
// fenced apart from every figure and check the page draws. The text is
// model output (CLAUDE.md rule 10) and reaches the DOM as text only; the
// entry carries no tone, so it never reads as a check's result.
export function modelWords(doc, label, text) {
  const e = el(doc, "div", null, "cd-tx-e cd-tx-model");
  const words = el(doc, "div");
  words.append(el(doc, "span", label, "cd-tx-label"), el(doc, "div", text, "cd-tx-body"));
  e.append(glyph(doc, "◇"), words, el(doc, "span", "", "cd-tx-meta"));
  return e;
}

// The rule a transcript draws ahead of a turn's first row.
export function txTurn(doc, turn) {
  return el(doc, "div", `turn ${turn}`, "cd-tx-turn");
}

// Button (the design system's): a button with its word, `variant` one of
// the bundle's (signal, ghost, down; none is the outline one).
export function dsButton(doc, word, { variant, title } = {}) {
  const b = el(doc, "button", word, cx("cd-btn", variant && `cd-btn-${variant}`));
  b.type = "button";
  if (title) b.title = title;
  return b;
}

// NowStrip (the design system's, #828): the run's state as a dot and its
// word, the clock, the turn, where the run is, and on the right what waits
// on the person, the model and the controls that apply. `tone` is one of
// the bundle's NowStrip states (idle, running, waiting, paused, failed,
// done), which colour the word; the word itself is the caller's. Each of
// `clock`, `turn`, `where`, `wait` and `model` is text or an element, and
// left out when null. The bundle sets role="status" on the strip; the page
// sets it on the pane that holds the strip, which stays while the strip
// is drawn afresh, since a live region added anew is not announced.
export function nowStrip(doc, { tone, word, clock, turn, where, wait, model, controls = [] }) {
  const root = el(doc, "div", null, "cd-now");
  const state = el(doc, "span", null, cx("cd-now-state", `cd-now-${tone || "idle"}`));
  const dot = el(doc, "span", null, "cd-now-dot");
  dot.setAttribute("aria-hidden", "true");
  state.append(dot, part(doc, word));
  root.append(state);
  const slot = (cls, v) => {
    const s = el(doc, "span", null, cls);
    s.append(part(doc, v));
    return s;
  };
  const sep = () => {
    const s = el(doc, "span", null, "cd-now-sep");
    s.setAttribute("aria-hidden", "true");
    return s;
  };
  if (clock != null) root.append(slot("cd-now-clock", clock));
  if (turn != null) root.append(sep(), slot(null, turn));
  if (where != null) root.append(sep(), slot("cd-now-where", where));
  root.append(el(doc, "span", null, "cd-now-spacer"));
  if (wait != null) root.append(slot("cd-now-wait", wait));
  if (model != null) root.append(slot("cd-now-model", model));
  root.append(...controls);
  return root;
}

// The Meter tones ds.css colours; signal is the bundle's default fill, so
// it takes no class of its own.
const METER_TONES = { cool: "cd-meter-cool", down: "cd-meter-down" };

// Meter (the design system's, #829): `filled` of `cells` block characters,
// █ then ░, as text in classed spans, so no width is ever a style. The
// reducer chooses the count and the tone; the cell count is fixed, since █
// and ░ have different East Asian width classes and a meter as long as
// its figures would not line up with the next. role="meter" with
// aria-valuetext saying the two figures (`used of ceiling`), because a
// screen reader reads aria-valuenow as a percentage, and a budget has no
// percentage (DESIGN.md rule 4).
export function meter(doc, { label, filled, cells, tone, now, max, text }) {
  const m = el(doc, "span", null, cx("cd-meter", METER_TONES[tone]));
  m.setAttribute("role", "meter");
  m.setAttribute("aria-label", label);
  m.setAttribute("aria-valuemin", "0");
  m.setAttribute("aria-valuemax", String(max));
  m.setAttribute("aria-valuenow", String(Math.min(now, max)));
  m.setAttribute("aria-valuetext", text);
  const bar = el(doc, "span", null, "cd-meter-bar");
  bar.setAttribute("aria-hidden", "true");
  bar.append(el(doc, "span", "█".repeat(filled), "cd-meter-on"), el(doc, "span", "░".repeat(cells - filled), "cd-meter-off"));
  m.append(bar);
  return m;
}

// Budget (the design system's, #829): one row per limit, its label, a
// Meter, and `used / ceiling` as two figures in the unit measured. A side
// trellis did not report is unreported's dash with its reason, and with
// either side unreported no Meter is drawn: cells with no share to show
// would read as 0. Rows are the reducer's `budget.limits`; no currency and
// no raise control (#837) here.
export function budgetRows(doc, rows) {
  const box = el(doc, "div", null, "cd-bud");
  for (const r of rows) {
    const row = el(doc, "div", null, "cd-bud-row");
    const unit = str(r.unit);
    const used = str(r.used_text);
    const ceiling = str(r.ceiling_text);
    const drawn = isCount(r.used) && isCount(r.ceiling) && isCount(r.filled) && isCount(r.cells) && r.filled <= r.cells;
    const label = str(r.limit) ?? "limit";
    const said = (t) => (unit ? `${t} ${unit}` : t);
    const slot = el(doc, "span");
    if (drawn && used !== null && ceiling !== null) {
      slot.append(meter(doc, { label, filled: r.filled, cells: r.cells, tone: str(r.tone), now: r.used, max: r.ceiling, text: `${said(used)} of ${said(ceiling)}` }));
    }
    // The unit follows the last figure drawn, never a dash.
    const readout = el(doc, "span", null, "cd-meter-value");
    readout.append(
      used === null ? unreported(doc, str(r.why_used) ?? "trellis did not report it") : ceiling === null ? said(used) : used,
      " / ",
      ceiling === null ? unreported(doc, str(r.why_ceiling) ?? "trellis did not report it") : said(ceiling),
    );
    row.append(el(doc, "span", label, "cd-bud-label"), slot, readout);
    box.append(row);
  }
  return box;
}

// Facts (#828): a label and its value per row, as a description list, for
// a region that states where something is rather than counting it (Where
// it is). Each value is text or an element (unreported's dash).
export function facts(doc, rows) {
  const dl = el(doc, "dl", null, "facts");
  for (const r of rows) {
    const row = el(doc, "div");
    const dd = el(doc, "dd");
    dd.append(part(doc, r.value));
    row.append(el(doc, "dt", r.label), dd);
    dl.append(row);
  }
  return dl;
}

// Queue (the design system's, #830): one of the composer's verbs while a
// run is in flight, as the card's mode option, pressed when it is what
// Send will do. Stop and send takes no tone of its own here: the bundle's
// options have signal and cool only, and its danger is said by the Stop
// dialog it opens.
export function queueVerb(doc, word, { on, cool, title } = {}) {
  const b = el(doc, "button", word, cx("cd-q-opt", cool && "cd-q-opt-cool", on && "cd-q-opt-on"));
  b.type = "button";
  b.setAttribute("aria-pressed", on ? "true" : "false");
  if (title) b.title = title;
  return b;
}

// A change's two line counts, as the Diff card's head draws them: added in
// diff-add, removed in diff-del, never one figure. Either one trellis did
// not report draws unreported's dash with `why`, never 0.
export function lineCounts(doc, added, removed, why) {
  const s = el(doc, "span", null, "cd-diff-counts");
  if (!isCount(added) || !isCount(removed)) {
    s.append(unreported(doc, why));
    return s;
  }
  s.append(el(doc, "span", `+${added}`, "a"), el(doc, "span", `−${removed}`, "d"));
  return s;
}

// The class and the sign a diff line is drawn with, by the reducer's sign.
const DIFF_SIGNS = { added: ["cd-diff-add", "+"], removed: ["cd-diff-del", "-"], context: [null, ""], note: [null, ""] };

// Diff (the design system's, #831): one file, its name and two counts in
// the head, then each hunk under its label in diff-hunk, each line in a
// row: the old and the new line number in two gutters, the sign, and the
// text, added lines in diff-add and removed in diff-del. Every string is
// the diff's own, repository text and model output, shown as text. A line
// the reducer gave a reason it takes no comment carries it as its title;
// `lineEnd(line)`, when given, answers what closes the row.
export function diff(doc, { file, counts, hunks, lineEnd }) {
  const root = el(doc, "div", null, "cd-diff");
  const head = el(doc, "div", null, "cd-diff-file");
  head.append(el(doc, "span", file), counts);
  root.append(head);
  for (const h of hunks) {
    const block = el(doc, "div");
    block.append(el(doc, "div", h.label, "cd-diff-hunk"));
    for (const l of h.lines) {
      const [cls, sign] = DIFF_SIGNS[str(l.sign)] ?? DIFF_SIGNS.note;
      const row = el(doc, "div", null, cx("cd-diff-line", cls));
      row.append(
        el(doc, "span", isCount(l.old) ? String(l.old) : "", "cd-diff-ln"),
        el(doc, "span", isCount(l.new) ? String(l.new) : "", "cd-diff-ln"),
        el(doc, "span", sign, "cd-diff-sign"),
        el(doc, "span", str(l.text) ?? "", "cd-diff-text"),
      );
      if (lineEnd) row.append(lineEnd(l));
      const why = str(l.why);
      if (why !== null) row.title = why;
      block.append(row);
    }
    root.append(block);
  }
  return root;
}

// What each kind of queued item is called, and the kind class the card
// colours it with (a held prompt has none: it is neither).
const QUEUE_KINDS = {
  steer: ["steer", "cd-q-kind-steer"],
  follow_up: ["follow-up", "cd-q-kind-follow"],
  prompt: ["stop + run", null],
};

// When a queued item fires, as its state line says while it waits.
const QUEUE_WAITS = {
  steer: "queued · next model call",
  follow_up: "queued · after this run",
  prompt: "queued · once this run has ended",
};

// A queued item's state (the reducer's `pending` rows) as a Status: queued
// waits, a delivered steer names the turn whose next request carried it, a
// delivered follow-up or prompt is a run of its own, and one not delivered
// says why in its title. Delivered is cool, the engine's activity, not ok,
// which is a passed check's (DESIGN.md §3.1).
function queueState(doc, q) {
  const kind = str(q.kind);
  const state = str(q.state);
  if (state === "delivered") {
    return dsStatus(doc, "cool", isCount(q.after_turn) ? `delivered after turn ${q.after_turn}` : "delivered · its own run");
  }
  if (state === "not_delivered") {
    return dsStatus(doc, "down", "not delivered", { title: str(q.why) ?? "trellis gave no reason" });
  }
  return dsStatus(doc, "warn", QUEUE_WAITS[kind] ?? "queued");
}

// Queue (the design system's, #830): the items waiting on the run in
// flight, numbered by the card's own counter, each its kind, its text as
// sent (the person's words, as text) and its state. No ↑ ↓ ✎ ×: take
// back is not built (#838), and the page marks that beside the list.
export function queueItems(doc, items) {
  return items.map((q) => {
    const li = el(doc, "li", null, "cd-q-item");
    const body = el(doc, "div");
    const [word, cls] = QUEUE_KINDS[str(q.kind)] ?? ["queued", null];
    const when = el(doc, "span", null, "cd-q-when");
    when.append(queueState(doc, q));
    body.append(el(doc, "span", word, cx("cd-q-kind", cls)), el(doc, "span", str(q.text) ?? ""), when);
    li.append(body);
    return li;
  });
}
