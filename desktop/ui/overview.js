// Draws the overview section, the eagle eye, from `trellis --json overview`
// (ADR 0050 section 3 and ADR 0048 section 2, both amended 2026-10-07;
// #558, #562).
//
// The object comes whole, from the core's `overview` event (no root, once
// at startup) or the page's `get_overview` command. Its shape is
// `client_contract.json`'s `overview` entry: `recent` always; `root`,
// `last_run`, `codebase`, `codebase_reason` and `board` only with a
// repository open. Four panes, each drawn whole from its part.
//
// Every string in it is written by someone else: trail tasks, queue tasks,
// PR titles and branch names from `gh` (ADR 0007, S10). It reaches the DOM
// only through `textContent` or `createElement`, so text that carries markup
// is shown as characters. A figure the object does not carry is said to be
// unreported, never drawn as 0, and no two counts are joined into a ratio
// (CLAUDE.md rules 3 and 4).

import { el, muted } from "./dom.js";
import { isCount, notReported, str } from "./fmt.js";

function list(v) {
  return Array.isArray(v) ? v : [];
}

// A definition list from [label, value] pairs, each value already text.
function pairs(doc, rows) {
  const dl = el(doc, "dl");
  for (const [label, value] of rows) dl.append(el(doc, "dt", label), el(doc, "dd", value));
  return dl;
}

// One muted line in place of a pane's contents.
function only(doc, pane, text, tag = "p") {
  const e = el(doc, tag, text);
  e.className = "empty";
  pane.replaceChildren(e);
}

// With no root only `recent` carries data (#558): the overview line says
// how to open a repository, and each other pane only that none is open.
const NO_ROOT = "No repository is open. Open one under Recent repositories, or start " +
  "trellis-desktop inside a repository or with --root <repo>.";
const NO_ROOT_PANE = "No repository is open.";

// A trail summary's first line: how the run ended, as trellis words it.
function trailLine(t) {
  const parts = [`outcome: ${str(t.outcome) ?? "not reported"}`];
  parts.push(`integrity: ${str(t.integrity) ?? "not reported"}`);
  return parts.join(" · ");
}

// Whether the open repository's trellis rpc child is running, as main.js
// last said: from `child-started`, `no-child`, `rpc-exited`, or a late
// page's refused `get_state`. Opening the root again starts a new child,
// which while one runs would only restart it and after an exit is the way
// to start it again (#638).
let childRuns = true;
// The overview and actions last drawn, so a change in `childRuns` redraws
// them.
let lastOverview = null;
let lastActions = null;

// recent: each listed repository with its newest trail, and "Open". The
// one already open is marked and its button disabled while its child runs.
function recent(doc, pane, o, actions) {
  const why = str(o.recent_error);
  if (why !== null) {
    only(doc, pane, `The recent list could not be read: ${why}`, "li");
    pane.append(refusalLine(doc));
    return;
  }
  const open = str(o.root);
  const rows = list(o.recent).map((r) => {
    const li = el(doc, "li");
    const path = str(r && r.path);
    const head = el(doc, "div");
    head.append(el(doc, "code", path ?? "path not reported"));
    if (path !== null) {
      const live = path === open && childRuns;
      const b = el(doc, "button", live ? "Open now" : "Open");
      b.type = "button";
      b.setAttribute("aria-label", `${live ? "Open now" : "Open"}: ${path}`);
      b.disabled = live;
      b.addEventListener("click", () => actions.open(path));
      head.append(" ", b);
    }
    li.append(head);
    const last = r ? r.last_run : null;
    if (last === null || last === undefined) {
      li.append(muted(doc, "div", "no runs yet"));
    } else if (str(last.error) !== null) {
      li.append(muted(doc, "div", `newest trail unreadable: ${last.error}`));
    } else {
      li.append(muted(doc, "div", trailLine(last)));
      const task = str(last.task);
      if (task !== null && task !== "") li.append(el(doc, "div", task));
    }
    return li;
  });
  if (rows.length === 0) only(doc, pane, "No repositories recorded yet.", "li");
  else pane.replaceChildren(...rows);
  pane.append(refusalLine(doc));
}

// The line an Open refusal is written into (#659), beside the buttons that
// asked, and announced; empty until one. The page serves it, and each
// drawing of the list makes it anew, so a refusal does not outlive the list
// it was about.
function refusalLine(doc) {
  const line = el(doc, "li");
  line.id = "ov-recent-status";
  line.className = "muted";
  line.setAttribute("role", "status");
  return line;
}

// The core's refusal of an "Open" above: a run in flight, or no repository
// at the path. It goes next to the button that asked, not the page's top.
export function drawOpenRefusal(doc, text) {
  doc.getElementById("ov-recent-status").textContent = text;
}

// last run: the open repository's newest trail. The overview carries no
// budget and no approvals for it (#558's `last_run` has neither), so those
// rows say so rather than drawing a figure trellis did not send.
function lastRun(doc, pane, o) {
  if (!("root" in o)) {
    only(doc, pane, NO_ROOT_PANE);
    return;
  }
  const t = o.last_run;
  if (t === null || t === undefined) {
    only(doc, pane, "No runs yet in this repository.");
    return;
  }
  if (str(t.error) !== null) {
    only(doc, pane, `The newest trail could not be read: ${t.error}`);
    return;
  }
  const checks = list(t.checks).map((c) => {
    const result = c && c.passed === true ? "passed" : c && c.passed === false ? "failed" : "unknown";
    return `${str(c && c.kind) ?? "check"} ${result}`;
  });
  const rows = [
    ["trail", str(t.run_id) ?? "not reported"],
    ["task", str(t.task) ?? "not reported"],
    ["started", str(t.started_at) ?? "not recorded"],
    // The run's outcome and its checks' results are two vocabularies, so
    // each keeps its own row.
    ["outcome", str(t.outcome) ?? "not reported"],
    ["integrity", str(t.integrity) ?? "not reported"],
    ["turns", isCount(t.turns) ? String(t.turns) : "not measured"],
    ["edits", isCount(t.edits) ? `${t.edits} file(s)` : "not reported"],
    ["checks", checks.length > 0 ? checks.join(", ") : "none ran"],
    // Two counts, never one number (CLAUDE.md rule 4).
    ["verified", notReported(t.verified)],
    ["reviewed", notReported(t.reviewed)],
    ["budget", "not reported in the overview"],
    ["tier", str(t.tier) ?? "not recorded"],
    ["approvals", "not reported in the overview"],
  ];
  pane.replaceChildren(pairs(doc, rows));
}

// codebase: the cached index's module count, and its call sites as three
// counts with no ratio and no total (CLAUDE.md rule 3). With no current
// cache, trellis's reason; the overview never indexes. The cache is what
// `trellis index` and the Codebase section save (#654), and the pane says
// so, so a reader knows where the counts came from.
function codebase(doc, pane, o) {
  if (!("root" in o)) {
    only(doc, pane, NO_ROOT_PANE);
    return;
  }
  const c = o.codebase;
  if (c === null || typeof c !== "object") {
    only(doc, pane, `Not shown: ${str(o.codebase_reason) ?? "no reason given"}.`);
    return;
  }
  pane.replaceChildren(
    pairs(doc, [
      ["modules indexed", notReported(c.modules_indexed)],
      ["bound call sites", notReported(c.bound_call_sites)],
      ["ambiguous", notReported(c.ambiguous)],
      ["unresolved", notReported(c.unresolved)],
    ]),
    muted(doc, "p", "Three counts from different populations; no share of one is drawn."),
    muted(doc, "p", "From the saved index, .trellis/index.bin, which trellis index and the Codebase section save."),
  );
}

// One queue task: its status, id and priority, its words, and its chain:
// the task it is part of and the ones it waits on. The status is the word
// the board's cells and rows use, so a task reads the same in both views
// (#656).
function task(doc, t) {
  const li = el(doc, "li");
  const id = str(t && t.id) ?? "?";
  if (str(t && t.error) !== null) {
    li.append(el(doc, "code", id), " ", muted(doc, "span", `unreadable: ${t.error}`));
    return li;
  }
  li.append(
    el(doc, "span", str(t.status) ?? "?"),
    " ",
    el(doc, "code", id),
    " ",
    muted(doc, "span", str(t.priority) ?? "?"),
    " ",
    el(doc, "span", str(t.task) ?? ""),
  );
  const chain = [];
  if (str(t.parent) !== null) chain.push(`part of ${t.parent}`);
  const blockers = list(t.blockers).filter((b) => typeof b === "string");
  if (blockers.length > 0) chain.push(`after ${blockers.join(", ")}`);
  if (str(t.waiting) !== null) chain.push(`waiting: ${t.waiting}`);
  if (chain.length > 0) li.append(muted(doc, "div", chain.join(" · ")));
  return li;
}

function pullRequest(doc, p) {
  const li = el(doc, "li");
  li.append(el(doc, "span", isCount(p.number) ? `#${p.number}` : "#?"), " ");
  li.append(el(doc, "span", str(p.title) ?? ""), " ");
  li.append(el(doc, "code", str(p.head) ?? "?"));
  if (p.draft === true) li.append(" ", muted(doc, "span", "draft"));
  const c = p.checks && typeof p.checks === "object" ? p.checks : {};
  li.append(
    muted(
      doc,
      "div",
      `checks: ${notReported(c.passed)} passed, ${notReported(c.failed)} failed, ` +
        `${notReported(c.pending)} pending, ${notReported(c.other)} other`,
    ),
  );
  return li;
}

function ciRun(doc, r) {
  const li = el(doc, "li");
  li.append(el(doc, "span", str(r.workflow) ?? "?"), " ");
  li.append(el(doc, "code", str(r.branch) ?? "?"), " ");
  li.append(el(doc, "span", `${str(r.status) ?? "?"}/${str(r.conclusion) ?? "none"}`));
  const when = str(r.created_at) ?? "start not reported";
  li.append(muted(doc, "div", isCount(r.id) ? `run ${r.id} · ${when}` : when));
  return li;
}

// A headed list, or one muted line when it is empty.
function block(doc, title, items, empty) {
  const ul = el(doc, "ul");
  ul.className = "rows";
  if (items.length === 0) {
    const e = el(doc, "li", empty);
    e.className = "empty";
    ul.append(e);
  } else {
    ul.append(...items);
  }
  return [el(doc, "h4", title), ul];
}

// board: the queue's chains, then PR and CI state from the user's own `gh`,
// or why it is not shown (`pr_ci.available` false and its `reason`).
function board(doc, pane, o) {
  if (!("root" in o)) {
    only(doc, pane, NO_ROOT_PANE);
    return;
  }
  const b = o.board && typeof o.board === "object" ? o.board : {};
  const out = [];
  if (Array.isArray(b.tasks)) {
    out.push(...block(doc, "Queue", b.tasks.map((t) => task(doc, t)), "No tasks queued."));
  } else {
    out.push(el(doc, "h4", "Queue"));
    out.push(muted(doc, "p", `The queue could not be read: ${str(b.tasks_error) ?? "no reason given"}`));
  }
  out.push(...drawPrCi(doc, b.pr_ci));
  pane.replaceChildren(...out);
}

// PR and CI state from the user's own `gh` (`board.pr_ci`), as nodes: the
// open pull requests and recent runs when `available` is true, or the
// reason it is not shown. Shared with the board section (queue.js, #311),
// so the two never draw it differently.
export function drawPrCi(doc, value) {
  const pr = value && typeof value === "object" ? value : {};
  const out = [];
  if (pr.available === true) {
    const prs = list(pr.pull_requests).map((p) => pullRequest(doc, p || {}));
    const runs = list(pr.runs).map((r) => ciRun(doc, r || {}));
    out.push(...block(doc, "Open pull requests (from gh)", prs, "No open pull requests."));
    out.push(...block(doc, "Recent CI runs (from gh), newest first", runs, "No CI runs."));
  } else {
    out.push(el(doc, "h4", "Pull requests and CI"));
    out.push(muted(doc, "p", `PR and CI state unavailable: ${str(pr.reason) ?? "no reason given"}`));
  }
  return out;
}

// The overview's own line: which repository the panes below describe, and,
// while its trellis rpc is not running, "Start again" on it. The line
// carries the button because the root may be missing from `recent`, which
// lists only the repositories an interface was started in.
function rootLine(doc, pane, o, actions) {
  if (!("root" in o)) {
    pane.textContent = NO_ROOT;
    return;
  }
  const root = str(o.root);
  pane.textContent = `Repository: ${root ?? "path not reported"}`;
  if (root !== null && !childRuns) {
    const b = el(doc, "button", "Start again");
    b.type = "button";
    b.setAttribute("aria-label", `Start trellis rpc again on ${root}`);
    b.addEventListener("click", () => actions.open(root));
    pane.append(" ", b);
  }
}

// element id -> drawing function, each given the whole object.
const PANES = {
  "ov-root": rootLine,
  "ov-recent": recent,
  "ov-last-run": lastRun,
  "ov-codebase": codebase,
  "ov-board": board,
};

// Draws every pane from one overview object. `actions.open(path)` is what
// a recent repository's "Open" does.
export function drawOverview(doc, o, actions) {
  if (o === null || typeof o !== "object") return;
  lastOverview = o;
  lastActions = actions;
  for (const [id, draw] of Object.entries(PANES)) {
    const pane = doc.getElementById(id);
    if (pane) draw(doc, pane, o, actions);
  }
}

// Whether the open repository's trellis rpc child runs, from main.js's
// child events; the last overview is redrawn so its root's controls follow.
export function drawOverviewChild(doc, runs) {
  childRuns = runs;
  if (lastOverview !== null) drawOverview(doc, lastOverview, lastActions);
}
