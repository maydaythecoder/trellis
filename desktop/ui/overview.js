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
import { dsStatus, figure, railRows, stats, unreported } from "./ds.js";
import { isCount, notReported, str } from "./fmt.js";

function list(v) {
  return Array.isArray(v) ? v : [];
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

// The tone a trail's outcome word is drawn in: an outcome is not a check,
// so a converged run is ok and the rest are not, each with its word.
const OUTCOME_TONES = { converged: "ok", not_converged: "down", interrupted: "warn", error: "down" };

// A trail's outcome as a Status, or unreported when it carries none.
function outcomeStatus(doc, t) {
  const word = str(t.outcome);
  return word === null ? unreported(doc, "the trail carries no outcome") : dsStatus(doc, OUTCOME_TONES[word] ?? "idle", word, { tag: true });
}

// last run: the open repository's newest trail, as Stats. The overview
// carries no budget and no approvals for it (#558's `last_run` has
// neither), so those figures are drawn unreported rather than as a figure
// trellis did not send.
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
  const head = el(doc, "p");
  // The run's outcome and its checks' results are two vocabularies, so
  // each keeps its own place.
  head.append(outcomeStatus(doc, t), " ", muted(doc, "span", `integrity ${str(t.integrity) ?? "not reported"}`));
  const checks = list(t.checks).map((c) => {
    const result = c && c.passed === true ? "passed" : c && c.passed === false ? "failed" : "unknown";
    return { label: str(c && c.kind) ?? "check", meta: el(doc, "strong", result, result), tone: result === "passed" ? "ok" : result === "failed" ? "down" : null };
  });
  const checkRail = el(doc, "ol", null, "cd-rail");
  checkRail.append(...railRows(doc, checks));
  pane.replaceChildren(
    head,
    el(doc, "p", str(t.task) ?? "task not reported"),
    stats(doc, [
      { label: "turns", value: figure(doc, t.turns, "the trail's turns were not measured") },
      { label: "edits", value: figure(doc, t.edits, "the trail did not report edits"), unit: "files" },
      // Two counts, never one number (CLAUDE.md rule 4).
      { label: "verified", value: figure(doc, t.verified, "the trail did not report it") },
      { label: "reviewed", value: figure(doc, t.reviewed, "the trail did not report it") },
      { label: "budget", value: unreported(doc, "the overview carries no budget") },
      { label: "approvals", value: unreported(doc, "the overview carries no approvals") },
    ]),
    checks.length > 0 ? checkRail : muted(doc, "p", "no checks ran"),
    muted(doc, "p", `trail ${str(t.run_id) ?? "not reported"} · started ${str(t.started_at) ?? "not recorded"} · tier ${str(t.tier) ?? "not recorded"}`),
  );
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
  const why = "the saved index did not carry it";
  pane.replaceChildren(
    stats(doc, [{ label: "modules", value: figure(doc, c.modules_indexed, why), unit: "indexed" }]),
    stats(doc, [
      { label: "bound", value: figure(doc, c.bound_call_sites, why), unit: "call sites" },
      { label: "ambiguous", value: figure(doc, c.ambiguous, why) },
      { label: "unresolved", value: figure(doc, c.unresolved, why) },
    ]),
    muted(doc, "p", "Three counts from different populations; no share of one is drawn."),
    muted(doc, "p", "From .trellis/index.bin, which trellis index and the Codebase section save."),
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

// The tone a CI run's conclusion is drawn in, as gh reported it: a run
// that succeeded is ok, one that failed is down, and any other (none yet,
// cancelled, skipped) is idle, its word saying which.
const CI_TONES = { success: "ok", failure: "down" };

// One CI run as a Rail row (#910): its workflow and branch, its state as
// a Status, and its id and start as the row's meta.
function ciRun(doc, r) {
  const label = el(doc, "span");
  const conclusion = str(r.conclusion);
  label.append(el(doc, "span", str(r.workflow) ?? "?"), " ");
  label.append(el(doc, "code", str(r.branch) ?? "?"), " ");
  label.append(dsStatus(doc, CI_TONES[conclusion] ?? "idle", `${str(r.status) ?? "?"}/${conclusion ?? "none"}`));
  const when = str(r.created_at) ?? "start not reported";
  return { label, meta: isCount(r.id) ? `run ${r.id} · ${when}` : when, tone: CI_TONES[conclusion] ?? null };
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
    // A sequence in time is a Rail, newest first as gh lists them.
    out.push(el(doc, "h4", "Recent CI runs (from gh), newest first"));
    if (runs.length === 0) out.push(el(doc, "p", "No CI runs.", "empty"));
    else {
      const rail = el(doc, "ol", null, "cd-rail");
      rail.append(...railRows(doc, runs));
      out.push(rail);
    }
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

// The level tiles' figures (#909): one live figure per tile from the parts
// of the object it already reads, or nothing where the overview carries
// none (the Atlas and the Vault tiles have no slot). Each is drawn small,
// under the tile's name, and says nothing the pane below does not.
function levelRun(doc, pane, o) {
  const t = o.last_run;
  if (!("root" in o) || t === null || t === undefined) pane.replaceChildren();
  else if (str(t.error) !== null) pane.replaceChildren(dsStatus(doc, "down", "trail unreadable", { title: t.error }));
  else pane.replaceChildren(outcomeStatus(doc, t));
}

function levelTrails(doc, pane, o) {
  const t = o.last_run;
  const at = t && typeof t === "object" ? str(t.started_at) : null;
  if (at === null) pane.replaceChildren();
  else pane.replaceChildren(muted(doc, "span", `newest ${at.replace("T", " ").replace(/:\d\dZ$/, "Z")}`));
}

function levelCodebase(doc, pane, o) {
  const c = o.codebase;
  if (!("root" in o)) pane.replaceChildren();
  else if (c === null || typeof c !== "object") pane.replaceChildren(unreported(doc, str(o.codebase_reason) ?? "no reason given"));
  else pane.replaceChildren(stats(doc, [{ label: "modules", value: figure(doc, c.modules_indexed, "the saved index did not carry it") }]));
}

function levelBoard(doc, pane, o) {
  const b = o.board && typeof o.board === "object" ? o.board : null;
  if (!("root" in o) || b === null) {
    pane.replaceChildren();
    return;
  }
  const pr = b.pr_ci && typeof b.pr_ci === "object" ? b.pr_ci : {};
  pane.replaceChildren(
    stats(doc, [
      { label: "tasks", value: Array.isArray(b.tasks) ? String(b.tasks.length) : unreported(doc, "the queue could not be read") },
      {
        label: "open prs",
        value: pr.available === true ? String(list(pr.pull_requests).length) : unreported(doc, str(pr.reason) ?? "gh is not available"),
      },
    ]),
  );
}

// element id -> drawing function, each given the whole object.
const PANES = {
  "ov-root": rootLine,
  "ov-recent": recent,
  "ov-last-run": lastRun,
  "ov-codebase": codebase,
  "ov-board": board,
  "lv-run": levelRun,
  "lv-trails": levelTrails,
  "lv-codebase": levelCodebase,
  "lv-board": levelBoard,
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
