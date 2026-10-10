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

function el(doc, tag, text) {
  const e = doc.createElement(tag);
  if (text !== undefined) e.textContent = text;
  return e;
}

function muted(doc, tag, text) {
  const e = el(doc, tag, text);
  e.className = "muted";
  return e;
}

function isCount(v) {
  return typeof v === "number" && Number.isFinite(v);
}

function str(v) {
  return typeof v === "string" ? v : null;
}

function count(v) {
  return isCount(v) ? String(v) : "not reported";
}

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

// recent: each listed repository with its newest trail, and "Open". The
// one already open is marked and its button disabled: opening it again
// would only restart its child.
function recent(doc, pane, o, actions) {
  const why = str(o.recent_error);
  if (why !== null) {
    only(doc, pane, `The recent list could not be read: ${why}`, "li");
    return;
  }
  const open = str(o.root);
  const rows = list(o.recent).map((r) => {
    const li = el(doc, "li");
    const path = str(r && r.path);
    const head = el(doc, "div");
    head.append(el(doc, "code", path ?? "path not reported"));
    if (path !== null) {
      const b = el(doc, "button", path === open ? "Open now" : "Open");
      b.type = "button";
      b.setAttribute("aria-label", `${path === open ? "Open now" : "Open"}: ${path}`);
      b.disabled = path === open;
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
    ["verified", count(t.verified)],
    ["reviewed", count(t.reviewed)],
    ["budget", "not reported in the overview"],
    ["tier", str(t.tier) ?? "not recorded"],
    ["approvals", "not reported in the overview"],
  ];
  pane.replaceChildren(pairs(doc, rows));
}

// codebase: the cached index's module count, and its call sites as three
// counts with no ratio and no total (CLAUDE.md rule 3). With no current
// cache, trellis's reason; the overview never indexes.
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
      ["modules indexed", count(c.modules_indexed)],
      ["bound call sites", count(c.bound_call_sites)],
      ["ambiguous", count(c.ambiguous)],
      ["unresolved", count(c.unresolved)],
    ]),
    muted(doc, "p", "Three counts from different populations; no share of one is drawn."),
  );
}

// One queue task: its state, id and priority, its words, and its chain:
// the task it is part of and the ones it waits on.
function task(doc, t) {
  const li = el(doc, "li");
  const id = str(t && t.id) ?? "?";
  if (str(t && t.error) !== null) {
    li.append(el(doc, "code", id), " ", muted(doc, "span", `unreadable: ${t.error}`));
    return li;
  }
  li.append(
    el(doc, "span", str(t.state) ?? "?"),
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
      `checks: ${count(c.passed)} passed, ${count(c.failed)} failed, ` +
        `${count(c.pending)} pending, ${count(c.other)} other`,
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
  const pr = b.pr_ci && typeof b.pr_ci === "object" ? b.pr_ci : {};
  if (pr.available === true) {
    const prs = list(pr.pull_requests).map((p) => pullRequest(doc, p || {}));
    const runs = list(pr.runs).map((r) => ciRun(doc, r || {}));
    out.push(...block(doc, "Open pull requests (from gh)", prs, "No open pull requests."));
    out.push(...block(doc, "Recent CI runs (from gh), newest first", runs, "No CI runs."));
  } else {
    out.push(el(doc, "h4", "Pull requests and CI"));
    out.push(muted(doc, "p", `PR and CI state unavailable: ${str(pr.reason) ?? "no reason given"}`));
  }
  pane.replaceChildren(...out);
}

// The overview's own line: which repository the panes below describe.
function rootLine(doc, pane, o) {
  pane.textContent = "root" in o ? `Repository: ${str(o.root) ?? "path not reported"}` : NO_ROOT;
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
  for (const [id, draw] of Object.entries(PANES)) {
    const pane = doc.getElementById(id);
    if (pane) draw(doc, pane, o, actions);
  }
}
