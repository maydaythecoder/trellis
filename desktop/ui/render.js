// Draws the run panes from the core's `view` event (ADR 0048 sections 5
// and 8).
//
// The view is DESK-08's reducer output (`desktop/src/state.rs`), where
// `cargo test` holds what each pane holds; this module only draws it. A
// `view` event carries the panes that changed, each under its key, so each
// key here draws one pane whole.
//
// Every value is trellis- or model-supplied text (ADR 0007, S10): it reaches
// the DOM only through `textContent` or `createElement`, so text that
// carries markup is shown as characters, never parsed. A figure trellis did
// not report is `null` in the view, and is said to be unreported here,
// never drawn as 0.
//
// Each function takes the document it draws into, so the module has no
// global state and can be driven by a test document.
//
// The actions log follows its end only while the reader is there (#643):
// a reader who scrolled back is not pulled down at each turn, and a button
// offers the end instead. main.js wires that button through `wireLog`.
//
// The DOM helpers and the value readers are the page's shared ones (dom.js
// and fmt.js, #817), and each pane is drawn with the design system's
// components (ds.js, #909): Stats for figures, Rails for what happened in
// order, Statuses for states. A figure trellis did not report is ds.js's
// unreported: a dash with the reason as its title, never 0.

import { appendRows, atEnd, el, fill, muted } from "./dom.js";
import { dsStatus, figure, railRows, stats, unreported } from "./ds.js";
import { isCount, str } from "./fmt.js";
import { drawGraph } from "./graph.js";

// The actions log: the transcript, as the TUI's, one line per element. It
// follows its end only while the reader is there (#643, as the TUI has
// since #244); otherwise their place is kept and the jump button under the
// log says new lines arrived. The view carries the whole transcript each
// turn; dom.js's appendRows appends only the lines beyond those drawn, since
// replacing every child of a role="log" region may have a screen reader
// read the whole log again.
function transcript(doc, pane, lines) {
  const { followed, drawn } = appendRows(doc, pane, lines, "Nothing has run yet.");
  const jump = doc.getElementById("log-jump");
  if (followed) jump.hidden = true;
  else if (drawn > 0) jump.hidden = false;
}

// Wires the actions log once: the jump button takes the reader to the end,
// and scrolling there by hand puts the button away, since what it offered
// is done.
export function wireLog(doc) {
  const pane = doc.getElementById("pane-transcript");
  const jump = doc.getElementById("log-jump");
  jump.addEventListener("click", () => {
    pane.scrollTop = pane.scrollHeight;
    jump.hidden = true;
    pane.focus();
  });
  pane.addEventListener("scroll", () => {
    if (atEnd(pane)) jump.hidden = true;
  });
}

// One Rail row per turn, in order: the tool and its target, signal when it
// wrote, and its own figures, each with its unit. The prompt is in bytes and
// the reply in tokens (#312), never joined into one figure.
function actions(doc, pane, list) {
  const rows = (Array.isArray(list) ? list : []).map((a, i) => {
    const label = el(doc, "span");
    label.append(el(doc, "b", str(a.tool) ?? "?"));
    const target = str(a.target);
    if (target) label.append(" ", el(doc, "code", target));
    const meta = [];
    if (a.wrote === true) meta.push("wrote");
    if (isCount(a.prompt_bytes)) meta.push(`${a.prompt_bytes} B prompt`);
    if (isCount(a.completion_tokens)) meta.push(`${a.completion_tokens} tok out`);
    meta.push(`t${i + 1}`);
    return { label, meta: meta.join(" · "), tone: a.wrote === true ? "signal" : "cool" };
  });
  fill(doc, pane, railRows(doc, rows), "none yet");
}

function edits(doc, pane, list) {
  const rows = (Array.isArray(list) ? list : []).map((p) => ({ label: el(doc, "code", String(p)), tone: "signal" }));
  fill(doc, pane, railRows(doc, rows), "none yet");
}

// A check row is its kind and the word trellis's verdict maps to, `passed`
// or `failed` (DESK-08), in the ok or down tone. The page decides nothing
// about it.
function checks(doc, pane, list) {
  const rows = (Array.isArray(list) ? list : []).map((c) => {
    const result = c.result === "passed" || c.result === "failed" ? c.result : "unknown";
    return {
      label: str(c.kind) ?? "check",
      meta: el(doc, "strong", result, result),
      tone: result === "passed" ? "ok" : result === "failed" ? "down" : null,
    };
  });
  fill(doc, pane, railRows(doc, rows), "none yet");
}

// Three counts side by side, never a ratio, a percentage or a sum (CLAUDE.md
// rule 3): the reach counts and the doubt count come from different
// populations, and the caption says what the reach is.
function radius(doc, pane, r) {
  if (r === null || typeof r !== "object") {
    fill(doc, pane, [], "none this run", "p");
    return;
  }
  const why = "this radius did not carry it";
  pane.replaceChildren(
    stats(doc, [
      { label: "resolved", value: figure(doc, r.resolved, why) },
      { label: "candidates", value: figure(doc, r.candidates, why) },
      { label: "unresolved", value: figure(doc, r.unresolved, why) },
    ]),
    muted(doc, "p", "a floor on reach, not a boundary"),
  );
}

// The budget as Stats: the turn against its ceiling, the last turn's prompt
// in bytes and its reply in tokens (#312), never joined. The view carries no
// elapsed time (DESK-08's budget has none) and the page draws no clock of
// its own, so elapsed is drawn unreported rather than as one the core did
// not send; a turn not reported is not turn 0.
function budget(doc, pane, b) {
  const v = b !== null && typeof b === "object" ? b : {};
  const list = [
    {
      label: "turn",
      value: figure(doc, v.turn, "no turn has started"),
      unit: isCount(v.max_turns) ? `/ ${v.max_turns}` : null,
    },
    { label: "prompt", value: figure(doc, v.prompt_bytes, "the last turn sent no prompt size"), unit: "B" },
    { label: "out", value: figure(doc, v.completion_tokens, "the last turn sent no token count"), unit: "tok" },
    { label: "elapsed", value: unreported(doc, "trellis sends no elapsed time yet, and the desktop keeps no clock of its own") },
  ];
  const rate = str(v.tokens_per_second);
  if (rate !== null) list.push({ label: "rate", value: rate, unit: "tok/s" });
  const queued = str(v.queued_seconds);
  if (queued !== null) list.push({ label: "queued", value: queued, unit: "s" });
  pane.replaceChildren(stats(doc, list));
}

// The session's prompt and completion tokens per model, as trellis's
// `get_session_stats` answered them (#313, PI-10): per model, its name and
// two Stats, never added together. The rows cover the runs since this
// repository was opened (one `trellis rpc` per repository), and a run's
// start leaves them as they are. The model name is trail text (ADR 0007),
// drawn as text, whole in its title where the pane cuts it.
function sessionTokens(doc, pane, t) {
  if (t === null || typeof t !== "object") {
    fill(doc, pane, [], "not asked yet", "p");
    return;
  }
  if (str(t.error) !== null) {
    fill(doc, pane, [], `get_session_stats refused: ${t.error}`, "p");
    return;
  }
  const why = "trellis's answer did not carry it";
  const blocks = (Array.isArray(t.models) ? t.models : []).flatMap((m) => {
    const model = str(m && m.model);
    if (model === null) return [];
    const name = el(doc, "code", model, "cut");
    name.title = model;
    return [
      name,
      stats(doc, [
        { label: "prompt", value: figure(doc, m.prompt_tokens, why), unit: "tok" },
        { label: "completion", value: figure(doc, m.completion_tokens, why), unit: "tok" },
      ]),
    ];
  });
  if (isCount(t.runs_not_counted) && t.runs_not_counted > 0) {
    blocks.push(el(doc, "p", `${t.runs_not_counted} run(s) not counted: their trail did not read back.`, "failed"));
  }
  fill(doc, pane, blocks, "no run has ended yet", "p");
}

// colibri's self-report for this run (`runtime identified`, #628): what
// it serves, its KV slots and its experts per tier, each as colibri sent
// it. A count it did not report is unreported, never 0, and the three tier
// counts are three numerals: nothing here adds them or takes a share (ADR
// 0048 §8). Only a run against colibri sends the event, so before one the
// pane says so.
function runtime(doc, pane, r) {
  if (r === null || typeof r !== "object") {
    pane.replaceChildren(unreported(doc, "no runtime has identified itself for this run"));
    return;
  }
  const why = "colibri did not report it";
  const head = el(doc, "p");
  head.append(dsStatus(doc, "cool", str(r.runtime) ?? "runtime not reported"), " ", el(doc, "code", str(r.served) ?? "not reported"));
  pane.replaceChildren(
    head,
    stats(doc, [
      { label: "kv slots", value: figure(doc, r.kv_slots, why) },
      { label: "vram", value: figure(doc, r.tiers_vram, why), unit: "experts" },
      { label: "ram", value: figure(doc, r.tiers_ram, why), unit: "experts" },
      { label: "disk", value: figure(doc, r.tiers_disk, why), unit: "experts" },
    ]),
  );
}

// The tier `get_state` last reported, as trellis labels it; a status bar
// writes `tier ` before the label (client_contract.json). Null means no run
// of this rpc process has decided one, and it is never guessed.
function tier(doc, pane, t) {
  pane.replaceChildren(
    str(t) !== null ? dsStatus(doc, "cool", `tier ${t}`) : unreported(doc, "no run of this trellis rpc has decided a tier yet"),
  );
}

function running(doc, pane, r) {
  pane.replaceChildren(r === true ? dsStatus(doc, "signal", "running", { live: true }) : dsStatus(doc, "idle", "no run in flight"));
}

// The tone each `run_end` outcome word is drawn in: an outcome is not a
// check, so a converged run is ok and the rest are not, each with its word.
const OUTCOME_TONES = { converged: "ok", "not converged": "down", interrupted: "warn", error: "down" };

// The last run's `run_end`, field by field as trellis sent it.
function outcome(doc, pane, o) {
  if (o === null || typeof o !== "object") {
    fill(doc, pane, [], "no run has ended yet", "p");
    return;
  }
  const word = str(o.outcome);
  const head = el(doc, "p");
  head.append(word === null ? unreported(doc, "run_end carried no outcome") : dsStatus(doc, OUTCOME_TONES[word] ?? "idle", word, { tag: true }));
  const out = [head, stats(doc, [{ label: "exit", value: figure(doc, o.exit, "run_end carried no exit code") }])];
  const meta = [];
  if (typeof o.restored === "boolean") meta.push(o.restored ? "tree restored" : "tree not restored");
  if (str(o.run_id) !== null) meta.push(`trail ${o.run_id}`);
  if (meta.length > 0) out.push(muted(doc, "p", meta.join(" · ")));
  if (str(o.error) !== null) out.push(el(doc, "p", o.error, "failed"));
  pane.replaceChildren(...out);
}

// view key -> [element id, drawing function]. The view's other keys are
// drawn by the modules that read them from the same event: `approvals`,
// `alert` and `stderr` by dialogs.js; `running`, `model`, `starters` and
// `undelivered` by composer.js; `model`, `models` and `available` by
// model.js; `available` by settings.js; `model`, `running` and `phase` by
// status.js; `vault` by vault.js; `queue` by queue.js; `atlas` by atlas.js.
// The view_renders_ tests in src/lockdown.rs fail when a key the reducer
// sends is read by none of them, or when a pane in the run section is one
// this table never names.
const PANES = {
  transcript: ["pane-transcript", transcript],
  actions: ["pane-actions", actions],
  edits: ["pane-edits", edits],
  checks: ["pane-checks", checks],
  radius: ["pane-radius", radius],
  budget: ["pane-budget", budget],
  tier: ["pane-tier", tier],
  runtime: ["pane-runtime", runtime],
  running: ["pane-running", running],
  outcome: ["pane-outcome", outcome],
  session_tokens: ["pane-session-tokens", sessionTokens],
  // The action graph's layout, computed by the core (`graph.rs`).
  graph: ["graph", drawGraph],
};

// Draws every pane the event carries; a key this page does not know is
// left alone.
export function drawView(doc, view) {
  if (view === null || typeof view !== "object") return;
  for (const [key, value] of Object.entries(view)) {
    const entry = PANES[key];
    if (!entry) continue;
    const pane = doc.getElementById(entry[0]);
    if (pane) entry[1](doc, pane, value);
  }
}
