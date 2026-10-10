// The vault notes view (#309): the repository's notes, read-only, from the
// `view` event's `vault` key, which the desktop core types from trellis's
// `vault_search` and `vault_graph` answers (`desktop/src/state/vault.rs`).
//
// The core writes every figure as `trellis vault search` and `trellis vault
// graph` print it, so this module computes nothing: the scores, the reduced
// search line, the depth and the unresolved parent are drawn as the view
// gives them. Each relation is its own list, as `graph` prints them.
//
// A note's title, path and issues are the vault's own text, which a run
// shows the model as data (ADR 0017), so they reach the DOM only through
// textContent (ADR 0007, S10), and none becomes a link. Nothing here writes
// the vault: rpc has no vault write, and every write stays a `trellis vault`
// command the person runs (#70, ADR 0049).
//
// index.html loads this as a module of its own.

let asked = 0;

function el(doc, tag, text) {
  const e = doc.createElement(tag);
  if (text !== undefined) e.textContent = text;
  return e;
}

function str(v) {
  return typeof v === "string" ? v : null;
}

function isCount(v) {
  return typeof v === "number" && Number.isFinite(v);
}

function status(doc, text) {
  doc.getElementById("vt-status").textContent = text;
}

// `items` as list rows, or one muted row saying the list is empty.
function rows(doc, items, empty) {
  const ul = el(doc, "ul");
  ul.className = "rows";
  for (const item of items) ul.append(item);
  if (items.length === 0) {
    const li = el(doc, "li", empty);
    li.className = "empty";
    ul.append(li);
  }
  return ul;
}

// A note as `trellis vault graph` names one: its title, then its path.
function named(doc, n) {
  const li = el(doc, "li");
  li.append(el(doc, "span", str(n && n.title) ?? "title not reported"), " ");
  const path = el(doc, "code", str(n && n.path) ?? "path not reported");
  li.append(path);
  return li;
}

// A reference out or in, with how many times it is made.
function reference(doc, r) {
  const li = named(doc, r);
  const w = el(doc, "span", ` ×${isCount(r && r.weight) ? r.weight : "not reported"}`);
  w.className = "muted";
  li.append(w);
  return li;
}

async function send(doc, tauri, line, waiting) {
  status(doc, waiting);
  try {
    await tauri.core.invoke("rpc_send", { line });
  } catch (err) {
    status(doc, `Open a repository to read its vault. ${err}`);
  }
}

function askNote(doc, tauri, path) {
  asked += 1;
  send(doc, tauri, { id: `desktop-vault-graph-${asked}`, type: "vault_graph", note: path }, `Reading ${path}.`);
}

function drawHits(doc, tauri, v) {
  const pane = doc.getElementById("vt-hits");
  const out = [];
  if (str(v.reduced) !== null) {
    const p = el(doc, "p", v.reduced);
    p.className = "muted";
    out.push(p);
  }
  if (!Array.isArray(v.hits)) {
    const p = el(doc, "p", "Search the vault's notes by title, path and body, as trellis vault search does.");
    p.className = "empty";
    out.push(p);
    pane.replaceChildren(...out);
    return;
  }
  const selected = str(v.graph && v.graph.note && v.graph.note.path);
  const items = v.hits.map((h) => {
    const li = el(doc, "li");
    const b = el(doc, "button");
    b.type = "button";
    const path = str(h && h.path);
    b.append(el(doc, "span", str(h && h.title) ?? "title not reported"), " ");
    b.append(el(doc, "code", path ?? "path not reported"), " ");
    const score = el(doc, "span", str(h && h.score) ?? "score not reported");
    score.className = "muted";
    b.append(score);
    if (path !== null && path === selected) b.setAttribute("aria-current", "true");
    if (path === null) b.disabled = true;
    else b.addEventListener("click", () => askNote(doc, tauri, path));
    li.append(b);
    return li;
  });
  out.push(rows(doc, items, "No note matches."));
  pane.replaceChildren(...out);
}

function section(doc, heading, body) {
  const s = el(doc, "section");
  s.append(el(doc, "h5", heading), body);
  return s;
}

function drawNote(doc, g) {
  const pane = doc.getElementById("vt-note");
  if (g === null || typeof g !== "object") {
    const p = el(doc, "p", "Select a hit to see the note's lineage, children and links.");
    p.className = "empty";
    pane.replaceChildren(p);
    return;
  }
  const note = g.note && typeof g.note === "object" ? g.note : {};
  const head = el(doc, "h4", str(note.title) ?? "title not reported");
  const path = el(doc, "p");
  path.append(el(doc, "code", str(note.path) ?? "path not reported"));

  const lineage = el(doc, "ol");
  lineage.className = "rows";
  for (const p of Array.isArray(g.lineage) ? g.lineage : []) {
    const li = el(doc, "li");
    li.append(el(doc, "code", str(p) ?? "path not reported"));
    lineage.append(li);
  }
  const list = (xs, each) => (Array.isArray(xs) ? xs : []).map((x) => each(doc, x));
  const issues = (Array.isArray(g.issues) ? g.issues : []).map((i) => el(doc, "li", str(i) ?? "issue not reported"));

  pane.replaceChildren(
    head,
    path,
    section(doc, "depth", el(doc, "p", str(g.depth) ?? "not reported")),
    section(doc, "lineage", lineage),
    section(doc, "unresolved parent", el(doc, "p", str(g.unresolved_parent) ?? "none")),
    section(doc, "children", rows(doc, list(g.children, named), "none")),
    section(doc, "references out", rows(doc, list(g.refs_out, reference), "none")),
    section(doc, "references in", rows(doc, list(g.refs_in, reference), "none")),
    section(doc, "issues", rows(doc, issues, "none")),
  );
}

export function drawVault(doc, tauri, v) {
  if (v === null || typeof v !== "object") return;
  if (str(v.error) !== null) status(doc, v.error);
  else if (Array.isArray(v.hits)) {
    const n = v.hits.length;
    status(doc, `${n} note${n === 1 ? "" : "s"} matched, in the order trellis vault search ranks them.`);
  } else status(doc, "Read-only: a note is changed with trellis vault or an editor, never from here.");
  drawHits(doc, tauri, v);
  drawNote(doc, v.graph);
}

export async function wireVault(doc, tauri) {
  doc.getElementById("vt-form").addEventListener("submit", (e) => {
    e.preventDefault();
    const query = doc.getElementById("vt-query").value.trim();
    if (query === "") {
      status(doc, "Type a query: trellis vault search takes at least one word.");
      return;
    }
    asked += 1;
    send(doc, tauri, { id: `desktop-vault-search-${asked}`, type: "vault_search", query }, `Searching the vault for ${query}.`);
  });
  drawVault(doc, tauri, { error: null, reduced: null, hits: null, graph: null });
  await tauri.event.listen("view", (event) => {
    const p = event.payload;
    if (p !== null && typeof p === "object" && "vault" in p) drawVault(doc, tauri, p.vault);
  });
}

function start() {
  wireVault(document, window.__TAURI__).catch((err) => console.error("trellis desktop vault:", err));
}

if (typeof window !== "undefined" && window.__TAURI__) {
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start);
  else start();
}
