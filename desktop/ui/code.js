// The Code level (#831): one burrow in from the Run level, the files the
// run wrote or asks to write; one more, a file's diff.
//
// The view's `code` key is the reducer's (`desktop/src/state.rs`, `code`),
// where cargo test holds what it holds: each file with its two line counts,
// its state word (`in the worktree`, `awaiting your approval`, `left in the
// tree`, `rolled back`, or `not reported` with why), the open approval's id
// while one waits on it, and its hunks, every line numbered from trellis's
// @@ header and, where it takes a comment, named `file:line`. This module
// only draws it, through ds.js's Rail, Status and Diff, and decides nothing.
//
// Approve and Reject show only while the view names an approval for the
// file, and answer through dialogs.js's answerApproval, the approval
// dialog's own path to `rpc_send`, where the core lets an `approve` through
// only for a question open in its view. The dialog's Review in Code leaves
// a question unanswered and opens its file here.
//
// A comment on a line is sent as a steer, through the same `rpc_send` the
// composer uses, its message starting with the line's `file:line` as the
// reducer named it. A line the reducer named none takes no comment, and
// says why as its title.
//
// The diff is repository text and model output (CLAUDE.md rule 10): it
// reaches the DOM through textContent only (ADR 0007, S10).
//
// The whole-file view (#838, ADR 0058 section 4) reads the chosen file
// through trellis's `read_file`, only when asked, as it is in the worktree
// then: a later edit by the run is not in it until it is read again. The
// view's `whole_file` key is the reducer's reading of the answer to the
// last read asked, and which paths may be read is trellis's rule (nothing
// git ignores, nothing in .git or outside the repository); a refusal is
// shown with trellis's reason. The text is repository text, drawn one
// line a text node through ds.js.

import { answerApproval, onReview } from "./dialogs.js";
import { el, fill } from "./dom.js";
import { diff, dsStatus, lineCounts, railRows, wholeFile } from "./ds.js";
import { isCount, str } from "./fmt.js";
import { burrow } from "./keys.js";
import { lineId, send } from "./rpc.js";

// The view's `code.files` as last sent.
let files = [];
// The path the file level shows; null until a file is chosen.
let chosen = null;
// The `file:line` the comment form is open on; null while it is closed.
let commenting = null;
// The view's `whole_file` as last sent: null until a file is read.
let whole = null;

// The levels a file is shown at, below the overview.
const FILE_LEVEL = ["run", "code", "code-file"];

// The Rail's tones for a file's state; warn and idle have none of their
// own on a Rail, so the row is left plain and the Status says it.
const RAIL_TONES = { signal: "signal", ok: "ok", down: "down" };

function file(path) {
  return files.find((f) => str(f && f.path) === path) ?? null;
}

function countsOf(doc, f) {
  return lineCounts(doc, f.added, f.removed, str(f.why_counts) ?? "trellis sent no line counts");
}

function stateOf(doc, f) {
  const why = str(f.why_state);
  return dsStatus(doc, str(f.tone) ?? "idle", str(f.word) ?? "not reported", why === null ? {} : { title: why });
}

// The files, one Rail row each: its path as the way in, its two counts and
// its state word.
function drawFiles(doc) {
  const rows = files.map((f) => {
    const path = str(f.path) ?? "";
    const pick = el(doc, "button", path, "code-pick");
    pick.type = "button";
    pick.dataset.burrow = "code-file";
    pick.dataset.path = path;
    if (path === chosen) pick.setAttribute("aria-current", "true");
    const meta = el(doc, "span", null, "code-meta");
    meta.append(countsOf(doc, f), stateOf(doc, f));
    return { label: pick, meta, tone: RAIL_TONES[str(f.tone)] ?? null };
  });
  fill(doc, doc.getElementById("code-files"), railRows(doc, rows), "This run has written no file yet.");
}

// A hunk's label: trellis's @@ header and the turn that applied it, or what
// stands in for a header the hunk has none of.
function hunkOf(h) {
  const header = str(h.header);
  const turn = isCount(h.turn) ? `turn ${h.turn}` : null;
  let label;
  if (turn === null) label = "awaiting your approval · its lines are numbered once a turn applies it";
  else if (header !== null) label = `${header} · ${turn}`;
  else label = `${turn} · ${str(h.why_no_diff) ?? "trellis sent no @@ header"}`;
  return { label, lines: Array.isArray(h.lines) ? h.lines : [] };
}

// What closes a diff line: a comment button on a line the reducer named,
// an empty cell on any other, so every row keeps its columns.
function commentControl(doc, l) {
  const at = str(l.at);
  if (at === null) return el(doc, "span");
  const b = el(doc, "button", "✉", "code-comment");
  b.type = "button";
  b.dataset.at = at;
  b.title = `Comment on ${at}: sent as a steer`;
  b.setAttribute("aria-label", `Comment on ${at}`);
  return b;
}

// The file level: the file's state, Approve and Reject while an approval
// waits on it, and its diff.
function drawFile(doc) {
  const f = chosen === null ? null : file(chosen);
  doc.getElementById("code-file-title").textContent = chosen ?? "File";
  const meta = doc.getElementById("code-file-meta");
  const bar = doc.getElementById("code-approve");
  const body = doc.getElementById("code-diff");
  if (f === null) {
    meta.replaceChildren();
    bar.hidden = true;
    bar.dataset.id = "";
    const said = chosen === null ? "Choose a file on the Code level." : "This run has no write to this file now: a new run starts with none.";
    body.replaceChildren(el(doc, "p", said, "empty"));
    closeComment(doc, null);
    return;
  }
  meta.replaceChildren(stateOf(doc, f));
  const approval = str(f.approval);
  bar.hidden = approval === null;
  bar.dataset.id = approval ?? "";
  const hunks = (Array.isArray(f.hunks) ? f.hunks : []).map(hunkOf);
  body.replaceChildren(diff(doc, { file: chosen, counts: countsOf(doc, f), hunks, lineEnd: (l) => commentControl(doc, l) }));
  // A form left open on a line that takes no comment now is closed, and
  // says why, rather than sending a steer about a line that has moved.
  if (commenting !== null && !hunks.some((h) => h.lines.some((l) => str(l.at) === commenting))) {
    closeComment(doc, `${commenting} takes no comment now: the run ended, or a later edit may have moved it.`);
  }
}

// The whole-file view of the chosen file: what the last read answered,
// when it was for this file; otherwise only the way to read it.
function drawWhole(doc) {
  const body = doc.getElementById("code-whole");
  const note = doc.getElementById("code-whole-note");
  const read = doc.getElementById("code-whole-read");
  const mine = whole !== null && chosen !== null && str(whole.path) === chosen;
  const state = mine ? str(whole.state) : null;
  read.disabled = chosen === null || state === "reading";
  read.textContent = state === "read" ? "Read it again" : "Read the whole file";
  if (state === "read" && typeof whole.text === "string") {
    const size = isCount(whole.bytes) ? `${whole.bytes} bytes on disk` : "size not reported";
    const meta = whole.cut === true ? `${size} · cut at 256 KiB, the rest not shown` : size;
    body.replaceChildren(wholeFile(doc, { file: chosen, meta, text: whole.text }));
    note.textContent = "As it was in the worktree when it was read; a later edit is not in it until it is read again.";
    return;
  }
  body.replaceChildren();
  if (state === "reading") note.textContent = `Reading ${chosen} through trellis…`;
  else if (state === "refused") note.textContent = `trellis did not read ${chosen}: ${str(whole.why) ?? "no reason given"}`;
  else note.textContent = chosen === null ? "" : "The file as it is in the worktree now, read through trellis when asked.";
}

// Draws the Code level's keys of one `view` event; any other key is
// another module's.
export function drawCode(doc, view) {
  if (view === null || typeof view !== "object") return;
  if ("whole_file" in view) {
    const w = view.whole_file;
    whole = w !== null && typeof w === "object" ? w : null;
    drawWhole(doc);
  }
  if (!("code" in view)) return;
  const c = view.code;
  files = c !== null && typeof c === "object" && Array.isArray(c.files) ? c.files : [];
  drawFiles(doc);
  drawFile(doc);
}

function note(doc, text) {
  doc.getElementById("code-comment-note").textContent = text ?? "";
}

function openComment(doc, at) {
  commenting = at;
  doc.getElementById("code-comment-label").textContent = `Comment on ${at}, sent as a steer`;
  doc.getElementById("code-comment").hidden = false;
  note(doc, "");
  doc.getElementById("code-comment-text").focus();
}

function closeComment(doc, said) {
  commenting = null;
  doc.getElementById("code-comment").hidden = true;
  if (said !== null) note(doc, said);
}

// Shows `path` at the file level.
function openFile(doc, path) {
  chosen = path;
  closeComment(doc, null);
  note(doc, "");
  drawFiles(doc);
  drawFile(doc);
  drawWhole(doc);
  burrow(doc, FILE_LEVEL);
}

// Asks trellis for the chosen file's whole text. The answer comes back as
// the view's `whole_file`; a line the core refuses is said in the note.
function readWhole(doc, tauri) {
  if (chosen === null) return;
  const line = { id: lineId("code", "read_file"), type: "read_file", path: chosen };
  send(tauri, line, (why) => {
    doc.getElementById("code-whole-note").textContent = why;
  });
}

// Sends the open comment as a steer naming its line.
async function sendComment(doc, tauri) {
  const input = doc.getElementById("code-comment-text");
  const text = input.value.trim();
  const at = commenting;
  if (at === null) return;
  if (text === "") {
    note(doc, "Type what should change at this line.");
    return;
  }
  const line = { id: lineId("code", "steer"), type: "steer", message: `${at}: ${text}` };
  if (await send(tauri, line, (why) => note(doc, why))) {
    input.value = "";
    closeComment(doc, `Sent as a steer naming ${at}. The Run level's log says when a model call carries it.`);
  }
}

// Wires the Code level once: the Run level's way in, the files' ways in,
// the comment buttons, Approve and Reject, and the comment form, each one
// listener on an element the page serves, so the rows drawn add none.
export function wireCode(doc, tauri) {
  doc.querySelector('button[data-burrow="code"]').addEventListener("click", () => burrow(doc, ["run", "code"]));
  doc.getElementById("code-files").addEventListener("click", (e) => {
    const pick = e.target.closest("button[data-burrow]");
    if (pick !== null) openFile(doc, pick.dataset.path);
  });
  doc.getElementById("code-diff").addEventListener("click", (e) => {
    const b = e.target.closest("button.code-comment");
    if (b !== null) openComment(doc, b.dataset.at);
  });
  const bar = doc.getElementById("code-approve");
  bar.addEventListener("click", (e) => {
    const b = e.target.closest("button[data-answer]");
    if (b === null || bar.dataset.id === "") return;
    answerApproval(doc, bar.dataset.id, b.dataset.answer === "approve");
    bar.hidden = true;
  });
  doc.getElementById("code-comment").addEventListener("submit", (e) => {
    e.preventDefault();
    sendComment(doc, tauri);
  });
  doc.getElementById("code-comment-cancel").addEventListener("click", () => closeComment(doc, ""));
  doc.getElementById("code-whole-read").addEventListener("click", () => readWhole(doc, tauri));
  onReview({
    pathOf: (id) => {
      const f = files.find((x) => x && str(x.approval) === id);
      return f ? str(f.path) : null;
    },
    open: (path) => openFile(doc, path),
  });
  drawFiles(doc);
  drawFile(doc);
  drawWhole(doc);
}
