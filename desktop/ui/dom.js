// The page's DOM helpers, one copy each (#816). A module that draws imports
// them from here; the copies modules still keep move here ticket by ticket
// (#817 to #820), and `page_helpers_live_in_one_module_each` in
// src/lockdown.rs names each helper's home and the copies not yet moved.
//
// Every value reaches the DOM through textContent, createElement or
// createElementNS: text that carries markup is shown as characters, never
// parsed (ADR 0007, S10; desktop/CLAUDE.md).

const SVG = "http://www.w3.org/2000/svg";

// An element with `text` as its text and `cls` as its class. `text` left
// undefined or null sets no text, so an empty string still empties it.
export function el(doc, tag, text, cls) {
  const e = doc.createElement(tag);
  if (text !== undefined && text !== null) e.textContent = text;
  if (cls) e.className = cls;
  return e;
}

// An element in the muted style: what the page says about a value, beside
// the value itself.
export function muted(doc, tag, text) {
  return el(doc, tag, text, "muted");
}

// An SVG element; the caller sets its attributes with setAttribute, never
// a style (the CSP's style-src 'self' refuses an inline one).
export function svg(doc, tag) {
  return doc.createElementNS(SVG, tag);
}

// `pane` replaced by `children`, or by one muted line saying it is empty,
// so an empty pane says why rather than showing nothing.
export function fill(doc, pane, children, empty, emptyTag = "li") {
  if (children.length === 0) pane.replaceChildren(el(doc, emptyTag, empty, "empty"));
  else pane.replaceChildren(...children);
}

// How far from a log's end still counts as at it, in px: a fractional
// scroll position or a zoom level can leave the end a pixel short. A pane
// that is not laid out (its section hidden) measures 0 and 0, and is at it.
const AT_END_SLACK = 2;

// Whether `pane` is scrolled to its end, read before it is redrawn so a
// reader who scrolled back keeps their place (#643).
export function atEnd(pane) {
  return pane.scrollHeight - pane.clientHeight - pane.scrollTop <= AT_END_SLACK;
}

// Draws `lines` into the log `pane`, one div each, and keeps it at its end
// only if the reader was there. When `lines` extends what is drawn, only
// the new lines are appended: replacing every child of a role="log" region
// may have a screen reader read the whole log again. A list that does not
// extend it (a cap dropped its oldest lines, or it was cleared) is drawn
// afresh, with `empty` when it has no line. Answers whether the pane
// followed its end and how many rows were drawn, so a caller can offer a
// jump to the end when the reader is elsewhere and lines arrived.
export function appendRows(doc, pane, lines, empty) {
  const list = (Array.isArray(lines) ? lines : []).map(String);
  const follow = atEnd(pane);
  const drawn = [...pane.children].filter((c) => c.className !== "empty");
  const grows = list.length >= drawn.length && drawn.every((c, i) => c.textContent === list[i]);
  const rows = (grows ? list.slice(drawn.length) : list).map((l) => el(doc, "div", l));
  if (grows && drawn.length > 0) pane.append(...rows);
  else fill(doc, pane, rows, empty, "div");
  if (follow) pane.scrollTop = pane.scrollHeight;
  return { followed: follow, drawn: rows.length };
}
