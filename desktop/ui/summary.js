// The summary level (#832): one burrow in from the Run level, through the
// Last run pane's way in or the Now strip's Summary so far, and reachable
// while the run is in flight.
//
// The view's `recap` key is the reducer's (`desktop/src/state.rs`,
// `summary`; not named `summary`, which holds `sum`, a word no view key
// may), where cargo test holds what it holds: the stage (`so_far`
// while the run is in flight, `final` once run_end has come), the outcome
// and exit code only after run_end, the checks by kind with each turn's
// time, the trail's Verified and Reviewed counts as two figures, the radius
// as three counts, the Code level's files, the budget's rows, this run's
// steers with their delivery, the path by turn, the trail's id and
// integrity, and the model's closing words. This module only draws it,
// through ds.js's components, and decides nothing.
//
// Every string is trellis-, trail- or model-supplied and reaches the DOM
// through textContent only (ADR 0007, S10). The closing words are model
// output, fenced as `{untrusted}` by the reducer and drawn under a label
// saying they are the model's words, not a verdict (CLAUDE.md rule 10).

import { el, fill } from "./dom.js";
import { budgetRows, dsStatus, figure, lineCounts, modelWords, railRows, stats, unreported } from "./ds.js";
import { isCount, str } from "./fmt.js";
import { burrow } from "./keys.js";
import { OUTCOME_TONES } from "./render.js";

// The levels the summary is shown at, below the overview.
const SUMMARY_LEVEL = ["run", "summary"];

// Integrity is trellis's word from the trail's digest, the one trail
// string drawn with a tone, as the Trails level draws it.
const INTEGRITY_TONES = { intact: "ok", altered: "down" };

// The Rail's tones for a file's state; warn and idle have none of their
// own on a Rail, so the row is left plain and the Status says it.
const RAIL_TONES = { signal: "signal", ok: "ok", down: "down" };

function object(v) {
  return v !== null && typeof v === "object" ? v : {};
}

function list(v) {
  return Array.isArray(v) ? v : [];
}

// The text an `{untrusted}` value holds, or null for anything else.
function untrusted(v) {
  return v !== null && typeof v === "object" && typeof v.untrusted === "string" ? v.untrusted : null;
}

// The stage and the outcome: so far, with no outcome, while the run is in
// flight; once run_end has come, its outcome word and exit code.
function drawEnd(doc, s) {
  const stage = str(s.stage) ?? "none";
  doc.getElementById("summary-end-title").textContent = stage === "final" ? "How it ended" : "So far";
  doc.getElementById("summary-stage").textContent = str(s.word) ?? "";
  const pane = doc.getElementById("summary-end");
  const o = s.outcome !== null && typeof s.outcome === "object" ? s.outcome : null;
  if (o === null) {
    const head = el(doc, "p");
    head.append(dsStatus(doc, stage === "so_far" ? "signal" : "idle", stage === "so_far" ? "so far · not ended" : "not ended"));
    pane.replaceChildren(
      head,
      stats(doc, [
        { label: "outcome", value: unreported(doc, str(s.why_outcome) ?? "run_end has not come") },
        { label: "exit", value: unreported(doc, str(s.why_outcome) ?? "run_end has not come") },
      ]),
    );
    return;
  }
  const word = str(o.outcome);
  const head = el(doc, "p");
  head.append(word === null ? unreported(doc, "run_end carried no outcome") : dsStatus(doc, OUTCOME_TONES[word] ?? "idle", word, { tag: true }));
  const out = [head, stats(doc, [{ label: "exit", value: figure(doc, o.exit, "run_end carried no exit code") }])];
  if (typeof o.restored === "boolean") out.push(el(doc, "p", o.restored ? "tree restored" : "tree not restored", "muted"));
  if (str(o.error) !== null) out.push(el(doc, "p", o.error, "failed"));
  pane.replaceChildren(...out);
}

// The checks by kind, one Rail row each: every run of that kind with
// trellis-checks' verdict and the turn's time. The tone is the last run's
// verdict as trellis reported it; the page decides nothing.
function drawChecks(doc, s) {
  const rows = list(s.checks).map((k) => {
    const runs = list(k.runs);
    const meta = el(doc, "span", null, "summary-runs");
    for (const r of runs) {
      const result = r.result === "passed" || r.result === "failed" ? r.result : "unknown";
      const turn = isCount(r.turn) ? `turn ${r.turn} ` : "";
      meta.append(dsStatus(doc, result === "passed" ? "ok" : result === "failed" ? "down" : "idle", `${turn}${result}`), " ");
      meta.append(isCount(r.took_ms) ? `${r.took_ms} ms` : unreported(doc, str(r.why_took) ?? "not reported"), " ");
    }
    const last = runs.length > 0 ? runs[runs.length - 1].result : null;
    return { label: str(k.kind) ?? "check", meta, tone: last === "passed" ? "ok" : last === "failed" ? "down" : null };
  });
  fill(doc, doc.getElementById("summary-checks"), railRows(doc, rows), "No check has run.");
}

// Verified and Reviewed: two rows, one Stat each, never added together and
// never one share of the other (CLAUDE.md rule 4).
function drawModes(doc, s) {
  const m = object(s.modes);
  doc.getElementById("summary-modes").replaceChildren(
    stats(doc, [{ label: "Verified", value: figure(doc, m.verified, str(m.why_verified) ?? "not reported") }]),
    stats(doc, [{ label: "Reviewed", value: figure(doc, m.reviewed, str(m.why_reviewed) ?? "not reported") }]),
  );
}

// The radius as three counts, never a ratio (CLAUDE.md rule 3).
function drawRadius(doc, s) {
  const pane = doc.getElementById("summary-radius");
  const r = s.radius !== null && typeof s.radius === "object" ? s.radius : null;
  if (r === null) {
    pane.replaceChildren(unreported(doc, str(s.why_radius) ?? "no radius this run"));
    return;
  }
  const why = "this radius did not carry it";
  pane.replaceChildren(
    stats(doc, [
      { label: "resolved", value: figure(doc, r.resolved, why) },
      { label: "candidates", value: figure(doc, r.candidates, why) },
      { label: "unresolved", value: figure(doc, r.unresolved, why) },
    ]),
    el(doc, "p", "a floor on reach, not a boundary", "muted"),
  );
}

// The files, as the Code level lists them: path, two counts, state word.
function drawFiles(doc, s) {
  const rows = list(s.files).map((f) => {
    const meta = el(doc, "span", null, "code-meta");
    const why = str(f.why_state);
    meta.append(
      lineCounts(doc, f.added, f.removed, str(f.why_counts) ?? "trellis sent no line counts"),
      dsStatus(doc, str(f.tone) ?? "idle", str(f.word) ?? "not reported", why === null ? {} : { title: why }),
    );
    return { label: el(doc, "code", str(f.path) ?? ""), meta, tone: RAIL_TONES[str(f.tone)] ?? null };
  });
  fill(doc, doc.getElementById("summary-files"), railRows(doc, rows), "This run has written no file.");
}

// A steer's delivery, as its transcript row says it.
function delivered(doc, d) {
  const state = d && str(d.state);
  if (state === "delivered" && isCount(d.after_step)) return dsStatus(doc, "cool", `delivered after turn ${d.after_step}`);
  if (state === "not_delivered") return dsStatus(doc, "down", "not delivered", { title: str(d.why) ?? "trellis gave no reason" });
  return dsStatus(doc, "warn", "pending · next model call");
}

function drawSteers(doc, s) {
  const rows = list(s.steers).map((t) => ({ label: str(t.text) ?? "", meta: delivered(doc, t.delivery), tone: "signal" }));
  fill(doc, doc.getElementById("summary-steers"), railRows(doc, rows), "No steer was sent this run.");
}

// The path, one Rail row per turn: the tool and what it was pointed at.
function drawPath(doc, s) {
  const rows = list(s.path).map((p) => {
    const label = el(doc, "span");
    label.append(el(doc, "b", str(p.tool) || "no tool"));
    if (str(p.target)) label.append(" → ", el(doc, "code", p.target));
    return { label, meta: [isCount(p.turn) ? `turn ${p.turn}` : null, p.wrote === true ? "wrote" : null].filter(Boolean).join(" · "), tone: p.wrote === true ? "signal" : "cool" };
  });
  fill(doc, doc.getElementById("summary-path"), railRows(doc, rows), "No turn yet.");
}

// The trail run_end named: its id and trellis's integrity word.
function drawTrail(doc, s) {
  const t = object(s.trail);
  const pane = doc.getElementById("summary-trail");
  const integrity = str(t.integrity);
  const row = el(doc, "p");
  if (str(t.run_id) !== null) row.append(el(doc, "code", t.run_id), " ");
  row.append(integrity !== null ? dsStatus(doc, INTEGRITY_TONES[integrity] ?? "idle", integrity) : unreported(doc, str(t.why) ?? "no trail"));
  pane.replaceChildren(row);
}

// The model's closing words, fenced: its own text under its label, or why
// there are none.
function drawClosing(doc, s) {
  const c = object(s.closing);
  const words = untrusted(c.words);
  const pane = doc.getElementById("summary-closing");
  if (words === null) {
    pane.replaceChildren(unreported(doc, str(c.why) ?? "trellis sent none"));
    return;
  }
  pane.replaceChildren(modelWords(doc, str(c.label) ?? "the model's words, not a verdict", words));
}

// Draws the summary level's key of one `view` event; any other key is
// another module's.
export function drawSummary(doc, view) {
  if (view === null || typeof view !== "object" || !("recap" in view)) return;
  const s = object(view.recap);
  drawEnd(doc, s);
  drawChecks(doc, s);
  drawModes(doc, s);
  drawRadius(doc, s);
  drawFiles(doc, s);
  doc.getElementById("summary-budget").replaceChildren(budgetRows(doc, list(s.budget)));
  drawSteers(doc, s);
  drawPath(doc, s);
  drawTrail(doc, s);
  drawClosing(doc, s);
}

// Wires the Run level's way in once.
export function wireSummary(doc) {
  doc.querySelector('button[data-burrow="summary"]').addEventListener("click", () => burrow(doc, SUMMARY_LEVEL));
}
