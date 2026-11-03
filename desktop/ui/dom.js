// The page's DOM helpers, one copy each (#816). A module that draws imports
// them from here, and `page_helpers_live_in_one_module_each` in
// src/lockdown.rs fails on a second copy in any other module (#817 to #820
// moved the copies modules kept).
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

// An SVG element with `attrs` set through setAttribute, never a style
// (the CSP's style-src 'self' refuses an inline one).
export function svg(doc, tag, attrs = {}) {
  const e = doc.createElementNS(SVG, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
  return e;
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

// Draws a feed the core sends (#827; desktop/src/feed.rs) into the log
// `pane`: `feed.rows`, each with its `seq`, each drawn by `draw(row)` into
// an element this marks with its seq, its turn and its kind (the actions
// log's filters hide rows by data-kind in style.css, #949, so a filter
// drops none). With `feed.whole` the rows
// replace what the pane holds; otherwise a row whose seq the pane holds
// replaces that row in place (it changed, as a steer does once delivered)
// and the rest are appended, so a capped log appends instead of redrawing
// every row: replacing every child of a role="log" region may have a screen
// reader read the whole log again. `divider(row, before)`, when given, may
// answer an element to set ahead of an appended row, `before` being the
// last row drawn. The pane keeps `feed.keep` rows, dropping its oldest, and
// says `empty` while it has none. It keeps to its end only if the reader
// was there. Answers whether it followed, how many rows it appended, and
// whether rows are missing: a new row numbered past the one the pane
// expected next, which a page that fell behind its event stream sees, and
// which the caller mends by asking for the whole run again.
export function drawFeed(doc, pane, feed, { draw, divider, empty }) {
  const rows = feed && Array.isArray(feed.rows) ? feed.rows : [];
  const follow = atEnd(pane);
  const mark = (row) => {
    const e = draw(row);
    e.dataset.seq = String(row.seq);
    if (Number.isFinite(row.turn)) e.dataset.turn = String(row.turn);
    if (typeof row.kind === "string") e.dataset.kind = row.kind;
    return e;
  };
  let drawn = [...pane.children].filter((c) => c.dataset.seq !== undefined);
  let last = drawn.length > 0 ? drawn[drawn.length - 1] : null;
  let appended = 0;
  let missing = false;
  if (feed && feed.whole === true) {
    pane.replaceChildren();
    drawn = [];
    last = null;
  }
  for (const row of rows) {
    if (!Number.isFinite(row.seq)) continue;
    // A row at or before the last drawn changed since it was sent: it is
    // replaced where it stands, or skipped if the cap has dropped it.
    if (last !== null && row.seq <= Number(last.dataset.seq)) {
      const at = drawn.findIndex((c) => Number(c.dataset.seq) === row.seq);
      if (at >= 0) {
        const e = mark(row);
        drawn[at].replaceWith(e);
        drawn[at] = e;
        last = drawn[drawn.length - 1];
      }
      continue;
    }
    if (last !== null && row.seq !== Number(last.dataset.seq) + 1) missing = true;
    const before = divider ? divider(row, last) : null;
    if (before) pane.append(before);
    const e = mark(row);
    pane.append(e);
    drawn.push(e);
    last = e;
    appended += 1;
  }
  // The cap, oldest first, with whatever stood ahead of the oldest row.
  const keep = Number.isFinite(feed && feed.keep) ? feed.keep : Infinity;
  while (drawn.length > keep) {
    const first = pane.firstElementChild;
    first.remove();
    if (first === drawn[0]) drawn.shift();
  }
  for (const e of [...pane.children]) if (e.className === "empty") e.remove();
  if (drawn.length === 0) pane.replaceChildren(el(doc, "div", empty, "empty"));
  if (follow) pane.scrollTop = pane.scrollHeight;
  return { followed: follow, drawn: appended, missing };
}
