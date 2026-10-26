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
// `size` "sm" is the 36px numeral.
export function stat(doc, { label, meta, value, unit, footnote, size, title }) {
  const root = el(doc, "div", null, cx("cd-stat", size === "sm" && "cd-stat-sm"));
  const head = el(doc, "div", null, "cd-stat-label");
  head.append(el(doc, "span", label));
  if (meta) head.append(el(doc, "span", meta, "cd-stat-unit"));
  const value_ = el(doc, "span", null, "cd-stat-value");
  value_.append(part(doc, value));
  if (unit && typeof value === "string") value_.append(el(doc, "span", unit, "cd-stat-unit"));
  const row = el(doc, "div", null, "cd-stat-value-row");
  row.append(value_);
  root.append(head, row);
  if (footnote) {
    const foot = el(doc, "div", null, "cd-stat-foot");
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

// The rule a transcript draws ahead of a turn's first row.
export function txTurn(doc, turn) {
  return el(doc, "div", `turn ${turn}`, "cd-tx-turn");
}
