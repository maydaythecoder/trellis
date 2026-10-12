// The vault notes view (#309): the repository's notes, read-only, from the
// `view` event's `vault` key, which the desktop core types from trellis's
// `vault_search`, `vault_graph` and `vault_check` answers
// (`desktop/src/state/vault.rs`).
//
// The core writes every figure as `trellis vault search` and `trellis vault
// graph` print it, so this module computes nothing: the scores, the reduced
// search line, the depth and the unresolved parent are drawn as the view
// gives them. Each relation is its own list, as `graph` prints them.
//
// A note's title, path and issues are the vault's own text, which a run
// shows the model as data (ADR 0017), so they reach the DOM only through
// textContent (ADR 0007, S10), and none becomes a link. A hit, and each
// lineage, child and reference entry with a path, is a button that asks
// for that note's graph as a hit does (#630); an entry with no path has
// nothing to ask for, so its button is disabled. Nothing here writes
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

// A button holding `content` that opens the note at `path` with a
// `vault_graph`, as a hit does; with no path it is disabled, since there
// is no note to ask for.
function opener(doc, tauri, path, ...content) {
  const b = el(doc, "button");
  b.type = "button";
  b.append(...content);
  if (path === null) b.disabled = true;
  else b.addEventListener("click", () => askNote(doc, tauri, path));
  return b;
}

// A note as `trellis vault graph` names one: its title, then its path.
function named(doc, tauri, n) {
  const li = el(doc, "li");
  const path = str(n && n.path);
  li.append(
    opener(
      doc,
      tauri,
      path,
      el(doc, "span", str(n && n.title) ?? "title not reported"),
      " ",
      el(doc, "code", path ?? "path not reported"),
    ),
  );
  return li;
}

// A reference out or in, with how many times it is made.
function reference(doc, tauri, r) {
  const li = named(doc, tauri, r);
  const w = el(doc, "span", ` ×${isCount(r && r.weight) ? r.weight : "not reported"}`);
  w.className = "muted";
  li.append(w);
  return li;
}

// Says whether the line was sent: a refusal is drawn as the status.
async function send(doc, tauri, line, waiting) {
  status(doc, waiting);
  try {
    await tauri.core.invoke("rpc_send", { line });
    return true;
  } catch (err) {
    status(doc, `Open a repository to read its vault. ${err}`);
    return false;
  }
}

function askNote(doc, tauri, path) {
  asked += 1;
  send(doc, tauri, { id: `desktop-vault-graph-${asked}`, type: "vault_graph", note: path }, `Reading ${path}.`);
}

// Asks for what `trellis vault check` prints (#632), never `--fix`: rpc
// has no fix field and refuses one by name. A refusal is drawn in the
// check pane itself, so the search's status line is left alone.
async function askCheck(doc, tauri) {
  asked += 1;
  const pane = doc.getElementById("vt-check");
  try {
    await tauri.core.invoke("rpc_send", { line: { id: `desktop-vault-check-${asked}`, type: "vault_check" } });
  } catch (err) {
    const p = el(doc, "p", `Open a repository to check its vault. ${err}`);
    p.className = "empty";
    pane.replaceChildren(p);
  }
}

// The report `trellis vault check` prints, line for line as the core gives
// it: the report line, the issues, the warnings and the run gate's answer.
// The gate answer is vault validation, not a verdict (ADR 0004): it is
// drawn here as text and never in the run section's checks pane.
function drawCheck(doc, c) {
  const pane = doc.getElementById("vt-check");
  if (c === null || typeof c !== "object") {
    const p = el(doc, "p", "Not checked yet: the report comes from trellis vault check, read-only.");
    p.className = "empty";
    pane.replaceChildren(p);
    return;
  }
  if (str(c.error) !== null) {
    const p = el(doc, "p", `vault_check refused: ${c.error}`);
    p.className = "empty";
    pane.replaceChildren(p);
    return;
  }
  const lines = (xs, what) => (Array.isArray(xs) ? xs : []).map((x) => el(doc, "li", str(x) ?? `${what} not reported`));
  pane.replaceChildren(
    el(doc, "p", str(c.line) ?? "report line not reported"),
    section(doc, "issues", rows(doc, lines(c.issues, "issue"), "none")),
    section(doc, "warnings", rows(doc, lines(c.warnings, "warning"), "none")),
    section(doc, "run gate", rows(doc, lines(c.violations, "violation"), str(c.gate) ?? "gate answer not reported")),
  );
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

function drawNote(doc, tauri, g) {
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
    li.append(opener(doc, tauri, str(p), el(doc, "code", str(p) ?? "path not reported")));
    lineage.append(li);
  }
  const list = (xs, each) => (Array.isArray(xs) ? xs : []).map((x) => each(doc, tauri, x));
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

// The Search button is disabled from the submit until a search's hits or a
// refusal is drawn (#657), so a second click does not start a second
// search on the child while the first is out. A child's exit answers
// nothing, so `wireVault` enables it again when one starts or exits.
function searching(doc, on) {
  doc.querySelector("#vt-form button[type=submit]").disabled = on;
}

export function drawVault(doc, tauri, v) {
  if (v === null || typeof v !== "object") return;
  if (str(v.error) !== null || Array.isArray(v.hits)) searching(doc, false);
  if (str(v.error) !== null) status(doc, v.error);
  else if (Array.isArray(v.hits)) {
    const n = v.hits.length;
    status(doc, `${n} note${n === 1 ? "" : "s"} matched, in the order trellis vault search ranks them.`);
  } else status(doc, "Read-only: a note is changed with trellis vault or an editor, never from here.");
  drawHits(doc, tauri, v);
  drawNote(doc, tauri, v.graph);
  drawCheck(doc, v.check);
}

export async function wireVault(doc, tauri) {
  doc.getElementById("vt-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const query = doc.getElementById("vt-query").value.trim();
    if (query === "") {
      status(doc, "Type a query: trellis vault search takes at least one word.");
      return;
    }
    asked += 1;
    searching(doc, true);
    const sent = await send(doc, tauri, { id: `desktop-vault-search-${asked}`, type: "vault_search", query }, `Searching the vault for ${query}.`);
    if (!sent) searching(doc, false);
  });
  doc.getElementById("vt-check-ask").addEventListener("click", () => askCheck(doc, tauri));
  drawVault(doc, tauri, { error: null, reduced: null, hits: null, graph: null, check: null });
  await tauri.event.listen("view", (event) => {
    const p = event.payload;
    if (p !== null && typeof p === "object" && "vault" in p) drawVault(doc, tauri, p.vault);
  });
  // A question the last child had not answered never will be; the new
  // child's vault is checked as it starts, so the report is always drawn.
  await tauri.event.listen("child-started", () => {
    searching(doc, false);
    askCheck(doc, tauri);
  });
  await tauri.event.listen("rpc-exited", () => searching(doc, false));
}

function start() {
  wireVault(document, window.__TAURI__).catch((err) => console.error("trellis desktop vault:", err));
}

if (typeof window !== "undefined" && window.__TAURI__) {
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start);
  else start();
}
