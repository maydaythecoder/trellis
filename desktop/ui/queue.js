// The board (#311): the repository's queue, read-only, from the `view`
// event's `queue` key, which the desktop core types from trellis's
// `get_queue` answer (`desktop/src/queue.rs`), and PR and CI state from the
// overview's `board.pr_ci` (#558), the user's own `gh` read-only.
//
// The core counts the tasks: eight cells, one per queue status, in the order
// `counts` gives them, and no sum of any (an unreadable task is in none).
// This module computes no figure; it writes each count as the view gives it.
// Each cell is a button: pressed, the list below shows only the tasks whose
// `status` is the cell's, and "Show all" lifts that (#656). Presentation
// only: the rows are the view's tasks, and nothing is counted from them.
// Below the cells, each task with its text, priority, labels, waiting reason
// and chain (#559). PR and CI state is drawn by overview.js's drawPrCi, so
// the board and the overview never draw it differently.
//
// Task text, labels, waiting reasons, errors and everything `gh` reports are
// written by others (ADR 0007, S10): they reach the DOM only through
// textContent. Nothing here writes the queue: its writes are `trellis queue`
// and `trellis work` (ADR 0020 section 5), which rpc does not have.
//
// index.html loads this as a module of its own.

import { el, muted } from "./dom.js";
import { isCount, str } from "./fmt.js";
import { drawPrCi } from "./overview.js";
import { send, start } from "./rpc.js";

let asked = 0;
// The overview reads for PR and CI state: each runs `trellis overview` and
// two `gh` calls, bounded to 10 seconds each, so at most one is out at a
// time, and only the newest is drawn. A new child starts a read of its
// own: the one out was for the last repository, and is dropped when it
// comes (as trails.js drops a stale read).
let prciReads = 0;
let prciInFlight = false;
// The status whose cell is pressed, or null to show every task.
let pressed = null;
// The view's queue as last drawn, so a press redraws it.
let lastQueue = null;

function texts(v) {
  return Array.isArray(v) ? v.filter((x) => typeof x === "string") : [];
}

function status(doc, text) {
  doc.getElementById("qb-status").textContent = text;
}

// Presses the cell for `status`, or with null, or a second press of the
// pressed cell, shows every task again.
function press(doc, status) {
  pressed = status === pressed ? null : status;
  if (lastQueue !== null) {
    drawCounts(doc, lastQueue.counts);
    drawTasks(doc, lastQueue.tasks);
  }
}

// The eight cells, in the order the view lists them, each a button that
// filters the list by its status, then "Show all".
function drawCounts(doc, counts) {
  const pane = doc.getElementById("qb-counts");
  if (!Array.isArray(counts)) {
    pane.replaceChildren(muted(doc, "li", "No counts are shown."));
    return;
  }
  const cells = counts.map((c) => {
    const li = el(doc, "li");
    const s = str(c && c.status);
    const b = el(doc, "button");
    b.type = "button";
    b.className = "qb-cell";
    const n = el(doc, "span", isCount(c && c.count) ? String(c.count) : "not reported");
    n.className = "qb-count";
    b.append(n, el(doc, "span", s ?? "status not reported"));
    b.setAttribute("aria-pressed", s !== null && s === pressed ? "true" : "false");
    if (s === null) b.disabled = true;
    else b.addEventListener("click", () => press(doc, s));
    li.append(b);
    return li;
  });
  const all = el(doc, "li");
  const b = el(doc, "button", "Show all");
  b.type = "button";
  b.className = "qb-cell";
  b.disabled = pressed === null;
  b.addEventListener("click", () => press(doc, null));
  all.append(b);
  pane.replaceChildren(...cells, all);
}

function task(doc, t) {
  const li = el(doc, "li");
  const id = str(t && t.id) ?? "?";
  const state = str(t && t.state) ?? "?";
  if (str(t && t.error) !== null) {
    li.append(el(doc, "code", id), " ", el(doc, "span", state), " ");
    const e = el(doc, "span", `unreadable: ${t.error}`);
    e.className = "failed";
    li.append(e);
    return li;
  }
  li.append(
    el(doc, "code", id),
    " ",
    el(doc, "span", str(t.status) ?? "status not reported"),
    " ",
    muted(doc, "span", str(t.priority) ?? "priority not reported"),
    " ",
    el(doc, "span", str(t.task) ?? "task not reported"),
  );
  const meta = [];
  const labels = texts(t.labels);
  if (labels.length > 0) meta.push(`labels: ${labels.join(", ")}`);
  if (str(t.assignee) !== null) meta.push(`assignee: ${t.assignee}`);
  if (str(t.parent) !== null) meta.push(`part of ${t.parent}`);
  const blockers = texts(t.blockers);
  if (blockers.length > 0) meta.push(`after ${blockers.join(", ")}`);
  if (str(t.waiting) !== null) meta.push(`waiting: ${t.waiting}`);
  if (meta.length > 0) li.append(muted(doc, "div", meta.join(" · ")));
  return li;
}

function drawTasks(doc, tasks) {
  const pane = doc.getElementById("qb-tasks");
  if (!Array.isArray(tasks)) {
    pane.replaceChildren();
    return;
  }
  // With a cell pressed, only the tasks whose status is its; an
  // unreadable task has none, as it is in no cell.
  const items = tasks
    .filter((t) => pressed === null || str(t && t.status) === pressed)
    .map((t) => task(doc, t));
  if (items.length === 0) {
    const li = el(doc, "li", pressed === null ? "No tasks queued." : `No task is ${pressed}.`);
    li.className = "empty";
    items.push(li);
  }
  pane.replaceChildren(...items);
}

export function drawQueue(doc, q) {
  if (q === null || typeof q !== "object") return;
  lastQueue = q;
  if (str(q.error) !== null) status(doc, q.error);
  else if (Array.isArray(q.tasks)) {
    const n = q.tasks.length;
    status(doc, `${n} task${n === 1 ? "" : "s"} in the queue, read-only: trellis queue and trellis work change it.`);
  }
  drawCounts(doc, q.counts);
  drawTasks(doc, q.tasks);
}

async function askQueue(doc, tauri) {
  asked += 1;
  await send(tauri, { id: `desktop-queue-${asked}`, type: "get_queue" }, (why) =>
    status(doc, `Open a repository to see its queue. ${why}`),
  );
}

// PR and CI state comes with the overview, which runs `gh` with a bounded
// wait, so it is read when the board opens and on Refresh only, and not
// again while a read is out.
async function askPrCi(doc, tauri) {
  if (prciInFlight) return;
  prciInFlight = true;
  prciReads += 1;
  const mine = prciReads;
  const pane = doc.getElementById("qb-prci");
  pane.replaceChildren(muted(doc, "p", "Reading pull requests and CI from gh."));
  let drawn;
  try {
    const o = await tauri.core.invoke("get_overview");
    const b = o && typeof o === "object" && o.board && typeof o.board === "object" ? o.board : null;
    drawn =
      b === null ? [muted(doc, "p", "PR and CI state unavailable: no repository is open.")] : drawPrCi(doc, b.pr_ci);
  } catch (err) {
    drawn = [muted(doc, "p", `PR and CI state unavailable: ${err}`)];
  }
  // A read the last repository started: a newer one is out or done.
  if (mine !== prciReads) return;
  prciInFlight = false;
  pane.replaceChildren(...drawn);
}

export async function wireQueue(doc, tauri) {
  const shown = () => !doc.getElementById("board").hidden;
  const refresh = () => {
    askQueue(doc, tauri);
    askPrCi(doc, tauri);
  };
  doc.getElementById("qb-refresh").addEventListener("click", refresh);
  drawCounts(doc, null);
  await tauri.event.listen("view", (event) => {
    const p = event.payload;
    if (p !== null && typeof p === "object" && "queue" in p) drawQueue(doc, p.queue);
  });
  // A run may have claimed, finished or failed a task.
  await tauri.event.listen("rpc-frame", (event) => {
    const f = event.payload;
    if (f !== null && typeof f === "object" && f.type === "run_end") askQueue(doc, tauri);
  });
  // Another repository is another queue. A PR and CI read still out is the
  // last repository's: it is dropped when it comes, and this one may start.
  await tauri.event.listen("child-started", () => {
    prciReads += 1;
    prciInFlight = false;
    pressed = null;
    if (shown()) refresh();
  });
  const button = doc.querySelector('nav.rail button[aria-controls="board"]');
  if (button) button.addEventListener("click", refresh);
}

if (typeof window !== "undefined" && window.__TAURI__) {
  start("the board", () => wireQueue(document, window.__TAURI__));
}
