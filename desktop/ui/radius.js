// The symbol radius view (#308): one symbol's blast radius, from a
// `get_radius` answer the desktop core has typed (`desktop/src/radius.rs`).
//
// trellis prints a radius as two blocks, reach and doubt, and so does this:
// the reach is call sites that reach the symbol; the doubt is every call
// site in the files the radius touches that bound to nothing, which is a
// different population and is never divided into the reach (CLAUDE.md rule
// 3, ADR 0014). The core collapses the doubt's reasons and files as the
// printed report does and refuses an answer carrying a ratio-shaped key, so
// this module computes no figure: it shows the counts the event carries,
// and "not reported" for one it lacks.
//
// The symbol, the paths, the reasons and trellis's messages are repository
// or trellis text (ADR 0007, S10): every value is written with textContent.
//
// index.html loads this as a module of its own.

let asked = 0;
// The id of the question asked last: only its answer is shown, so a slow
// answer to an earlier question cannot replace a newer one.
let waiting = null;

function el(doc, tag, text) {
  const e = doc.createElement(tag);
  if (text !== undefined) e.textContent = text;
  return e;
}

function isCount(v) {
  return typeof v === "number" && Number.isFinite(v);
}

function n(v) {
  return isCount(v) ? String(v) : "not reported";
}

function str(v) {
  return typeof v === "string" ? v : null;
}

function status(doc, text) {
  doc.getElementById("rd-status").textContent = text;
}

function block(doc, heading, rows) {
  const div = el(doc, "div");
  div.className = "rd-block";
  div.append(el(doc, "h4", heading));
  const dl = el(doc, "dl");
  for (const [label, value] of rows) dl.append(el(doc, "dt", label), el(doc, "dd", value));
  div.append(dl);
  return div;
}

// The doubt's reasons or files: each row's name and call sites, then the
// rows the core counted and did not list.
function counted(doc, list, rest, further, withExplain) {
  const ul = el(doc, "ul");
  ul.className = "rows";
  for (const c of Array.isArray(list) ? list : []) {
    if (!c || str(c.name) === null) continue;
    const li = el(doc, "li");
    li.append(el(doc, "code", c.name), ` ${n(c.call_sites)} call sites`);
    if (withExplain && str(c.explain) !== null) {
      const x = el(doc, "span", ` · ${c.explain}`);
      x.className = "muted";
      li.append(x);
    }
    ul.append(li);
  }
  if (rest && isCount(rest.rows)) {
    const li = el(doc, "li", `(other) ${n(rest.call_sites)} call sites across ${rest.rows} further ${further}`);
    li.className = "muted";
    ul.append(li);
  }
  return ul;
}

export function drawRadius(doc, payload) {
  const pane = doc.getElementById("rd-result");
  if (payload === null || typeof payload !== "object") return;
  if (waiting !== null && payload.id !== waiting) return;
  waiting = null;
  if (typeof payload.error === "string") {
    status(doc, payload.error);
    pane.replaceChildren();
    return;
  }
  const r = payload.reach && typeof payload.reach === "object" ? payload.reach : {};
  const d = payload.doubt && typeof payload.doubt === "object" ? payload.doubt : {};
  status(doc, `${str(payload.symbol) ?? "symbol not reported"} (${str(payload.file) ?? "file not reported"})`);

  const reach = block(doc, "reach — call sites that reach this symbol", [
    ["resolved", n(r.resolved)],
    ["ambiguous", n(r.ambiguous)],
    ["symbols reached", n(r.symbols_reached)],
  ]);
  const doubt = block(doc, "doubt — a different population, not a fraction of the reach", [
    ["unbound sites in touched files", n(d.unbound_in_touched_files)],
    ["touched files", n(d.touched_files)],
  ]);
  if (Array.isArray(d.reasons) && d.reasons.length > 0) {
    doubt.append(el(doc, "h5", "top reasons"), counted(doc, d.reasons, d.reasons_not_shown, "reasons", true));
  }
  if (Array.isArray(d.files) && d.files.length > 0) {
    doubt.append(el(doc, "h5", "files"), counted(doc, d.files, d.files_not_shown, "files", false));
  }
  const note = el(
    doc,
    "p",
    "The two blocks count different things and must not be divided. Reach is a floor, not a " +
      "boundary: roughly three call edges in ten are statically invisible in Python, and any " +
      "unbound site above may also reach this symbol.",
  );
  note.className = "muted";
  pane.replaceChildren(reach, doubt, note);
}

async function ask(doc, tauri) {
  const symbol = doc.getElementById("rd-symbol").value.trim();
  if (symbol === "") {
    status(doc, "Name a symbol, as trellis radius takes it: a.f, f, or a class's method.");
    return;
  }
  asked += 1;
  const id = `desktop-radius-${asked}`;
  waiting = id;
  status(doc, `Reading the radius of ${symbol}: trellis indexes the repository for it.`);
  doc.getElementById("rd-result").replaceChildren();
  try {
    await tauri.core.invoke("rpc_send", { line: { id, type: "get_radius", symbol } });
  } catch (err) {
    if (waiting === id) waiting = null;
    status(doc, `Open a repository to see a radius. ${err}`);
  }
}

export async function wireRadius(doc, tauri) {
  doc.getElementById("rd-form").addEventListener("submit", (e) => {
    e.preventDefault();
    ask(doc, tauri);
  });
  await tauri.event.listen("radius", (event) => drawRadius(doc, event.payload));
}

function start() {
  wireRadius(document, window.__TAURI__).catch((err) => console.error("trellis desktop radius:", err));
}

if (typeof window !== "undefined" && window.__TAURI__) {
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start);
  else start();
}
