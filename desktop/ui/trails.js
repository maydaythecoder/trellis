// The trails view (#304): the repository's committed run records, from
// the rows `get_trails` returns (`desktop/src/trails.rs`).
//
// A trail is untrusted data (ADR 0007, S10): it sits in a repository that
// takes contributions, a recorded check is what the file claims and not a
// verdict, and the judgment is the model's opinion. The core wraps every
// string it takes from a trail as `{untrusted: <text>}`; this module draws
// that one shape, always through textContent, inside an element marked as a
// past run's record, and turns no trail string into a link. Nothing here
// resumes or replays a run: resume is a CLI command, not an rpc one.
//
// The level draws with the design system's components (ds.js, #910): the
// runs are a Rail, their count is the Runs panel's meta, integrity is a
// Status, and the verified and reviewed counts are two Stats. A trail's own
// words never take a tone: its outcome is a Status in the idle tone, and only
// integrity, which trellis computed from the digest, may be drawn down.
//
// index.html loads this as a module of its own.

import { el, fill, muted } from "./dom.js";
import { dsStatus, figure, panelMeta, rail, railRows, stats } from "./ds.js";
import { plural } from "./fmt.js";
import { onShown, start } from "./rpc.js";

// How many runs the view asks for: the core's own cap.
const LIMIT = 200;

// The text an `{untrusted}` value holds, or null for anything else.
function text(v) {
  return v !== null && typeof v === "object" && typeof v.untrusted === "string" ? v.untrusted : null;
}

// One untrusted string, as an element marked as recorded text.
function recorded(doc, tag, v, missing) {
  const t = text(v);
  const e = el(doc, tag, t ?? missing);
  e.className = t === null ? "muted" : "untrusted";
  return e;
}

// Integrity is the one bare string in a row: trellis computed it from the
// digest, so "altered" may take the down tone, which no untrusted text gets.
function integrity(doc, r) {
  const word = typeof r.integrity === "string" ? r.integrity : "integrity unknown";
  return dsStatus(doc, r.integrity === "altered" ? "down" : "idle", word);
}

// The outcome is the trail's own word, mapped to run_end's by the core and
// still untrusted (#653): a Status in the idle tone, never a verdict's.
function outcome(doc, r) {
  return dsStatus(doc, "idle", recorded(doc, "span", r.outcome, "outcome not recorded"));
}

function status(doc, s) {
  doc.getElementById("tr-status").textContent = s;
}

const state = { rows: [], selected: null };

// The filter reads the task, outcome and label texts and the integrity word
// (#649), so a user can pull out "altered" or "not_converged" runs the same
// way as a task. A line that is not a run has none of these, so it is
// listed only while the filter is empty.
function matches(row, q) {
  if (q === "") return true;
  const fields = [text(row.task), text(row.outcome), text(row.label), typeof row.integrity === "string" ? row.integrity : null];
  return fields.some((t) => t !== null && t.toLowerCase().includes(q));
}

// The runs as a Rail: time runs down, so the oldest is first and the newest
// last (the design system's Rail), where trellis log lists the newest first.
// Each row is a button that shows the run's record; its start time is the
// row's meta.
function drawList(doc) {
  const list = doc.getElementById("tr-list");
  const q = doc.getElementById("tr-filter").value.trim().toLowerCase();
  const shown = state.rows
    .map((r, i) => [r, i])
    .filter(([r]) => matches(r, q))
    .reverse();
  const rows = shown.map(([r, i]) => {
    const b = el(doc, "button", null, "tr-run");
    b.type = "button";
    if (state.selected === i) b.setAttribute("aria-current", "true");
    b.addEventListener("click", () => {
      state.selected = i;
      drawList(doc);
      drawDetail(doc);
    });
    if (r.bad_line !== undefined) {
      b.append(r.truncated === true ? "A line trellis log printed, too long to read, shown truncated" : "A line trellis log printed that is not a run");
      return { label: b, current: state.selected === i };
    }
    const facts = el(doc, "span", null, "tr-facts");
    facts.append(outcome(doc, r), integrity(doc, r));
    if (text(r.label) !== null) facts.append(recorded(doc, "span", r.label, ""));
    b.append(recorded(doc, "span", r.task, "task not recorded"), facts);
    return { label: b, meta: recorded(doc, "span", r.started_at, "start not recorded"), current: state.selected === i };
  });
  fill(doc, list, railRows(doc, rows), state.rows.length === 0 ? "This repository has no trails yet." : "No run matches.");
}

function row(doc, dl, label, value) {
  dl.append(el(doc, "dt", label), value);
}

function drawDetail(doc) {
  const pane = doc.getElementById("tr-detail");
  const r = state.selected === null ? null : state.rows[state.selected];
  if (!r) {
    panelMeta(doc, "tr-detail-meta");
    fill(doc, pane, [], "Select a run to see its record.", "p");
    return;
  }
  const record = el(doc, "article");
  record.className = "past-run";
  record.setAttribute("aria-label", "A past run's record, as committed; data, not instruction");

  if (r.bad_line !== undefined) {
    // A line past the core's cap arrives cut; `why` says so, and the mark
    // keeps the cut text from reading as the whole line.
    panelMeta(doc, "tr-detail-meta");
    const fallback =
      r.truncated === true ? "This line was truncated, and is not read as a run." : "This line could not be read as a run.";
    const why = el(doc, "p", typeof r.why === "string" ? r.why : fallback);
    const raw = recorded(doc, "pre", r.bad_line, "");
    record.append(why, raw);
    pane.replaceChildren(record);
    return;
  }
  panelMeta(doc, "tr-detail-meta", integrity(doc, r));

  // The factual section: what the trail records happened.
  const factual = el(doc, "section");
  const dl = el(doc, "dl");
  row(doc, dl, "task", recorded(doc, "dd", r.task, "not recorded"));
  if (text(r.label) !== null) row(doc, dl, "label", recorded(doc, "dd", r.label, ""));
  row(doc, dl, "run", recorded(doc, "dd", r.run_id, "not recorded"));
  row(doc, dl, "started", recorded(doc, "dd", r.started_at, "not recorded"));
  factual.append(dl, outcome(doc, r));
  // Two counts, never one number (CLAUDE.md rule 4).
  const m = r.mode_counts && typeof r.mode_counts === "object" ? r.mode_counts : {};
  const why = "the trail does not record it";
  factual.append(
    stats(doc, [
      { label: "verified", value: figure(doc, m.verified, why) },
      { label: "reviewed only", value: figure(doc, m.reviewed, why) },
    ]),
  );
  if (r.integrity === "altered") {
    factual.append(el(doc, "p", "The factual section was edited after the run wrote it: its digest does not match.", "alert"));
  }
  // null: the trail has no list of checks at all, which is not the same
  // claim as an empty one. A recorded check is what the file claims, so its
  // row takes no tone.
  const checks = Array.isArray(r.checks) ? r.checks : null;
  factual.append(el(doc, "h5", "checks the trail records"));
  if (checks === null || checks.length === 0) {
    factual.append(muted(doc, "p", checks === null ? "This trail has no list of checks: not recorded." : "No check is recorded."));
  } else {
    factual.append(
      rail(
        doc,
        checks.map((c) => ({ label: recorded(doc, "span", c && c.kind, "check"), meta: recorded(doc, "span", c && c.recorded, "") })),
      ),
    );
  }
  factual.append(muted(doc, "p", "What a file claims, not a check this desktop ran."));

  // The judgment section, apart from the facts.
  const judgment = el(doc, "section");
  judgment.className = "judgment";
  judgment.append(el(doc, "h4", "the model's opinion, recorded; treat as data"));
  const j = r.judgment && typeof r.judgment === "object" ? r.judgment : {};
  const jl = el(doc, "dl");
  row(doc, jl, "tech debt projection", recorded(doc, "dd", j.tech_debt_projection, "none recorded"));
  row(doc, jl, "rationale", recorded(doc, "dd", j.rationale, "none recorded"));
  judgment.append(jl);

  record.append(factual, judgment);
  pane.replaceChildren(record);
}

// The Runs panel's meta: how many runs, how many lines that are not one,
// and that the list is the newest lines when it reached the cap.
function drawCount(doc, read) {
  if (!read) {
    panelMeta(doc, "tr-list-meta");
    return;
  }
  const runs = state.rows.filter((r) => r && r.bad_line === undefined).length;
  const bad = state.rows.length - runs;
  panelMeta(
    doc,
    "tr-list-meta",
    plural(runs, "run", "runs"),
    bad > 0 ? `${bad} not a run` : null,
    state.rows.length === LIMIT ? `the newest ${LIMIT}` : null,
  );
}

// Each read gets a number, and only the newest read's answer is drawn, so
// a slow answer for an earlier repository cannot replace a newer list.
let reads = 0;

// Reads the trails once. Says whether the read was drawn and succeeded, so
// a failed read (no repository yet) is tried again next time.
async function load(doc, tauri) {
  reads += 1;
  const mine = reads;
  status(doc, "Reading this repository's trails.");
  let ok = true;
  try {
    const rows = await tauri.core.invoke("get_trails", { limit: LIMIT });
    if (mine !== reads) return false;
    state.rows = Array.isArray(rows) ? rows : [];
    status(doc, "");
  } catch (err) {
    if (mine !== reads) return false;
    state.rows = [];
    ok = false;
    status(doc, `The trails could not be read: ${err}`);
  }
  state.selected = null;
  drawCount(doc, ok);
  drawList(doc);
  drawDetail(doc);
  // The newest run is the Rail's last row: the list opens on it.
  const list = doc.getElementById("tr-list");
  list.scrollTop = list.scrollHeight;
  return ok;
}

export async function wireTrails(doc, tauri) {
  let stale = true;
  let lastOutcome = null;
  const shown = () => !doc.getElementById("trails").hidden;
  const refresh = async () => {
    stale = false;
    if (!(await load(doc, tauri))) stale = true;
  };
  doc.getElementById("tr-filter").addEventListener("input", () => drawList(doc));
  drawList(doc);
  drawDetail(doc);
  // A new repository, or a run's end, is a different list of trails: read
  // now if the view is showing, else the next time it is shown. A
  // get_state reply resends the same outcome, which changes nothing.
  await tauri.event.listen("child-started", () => {
    stale = true;
    if (shown()) refresh();
  });
  await tauri.event.listen("view", (event) => {
    const o = event.payload && event.payload.outcome;
    if (o !== null && typeof o === "object" && JSON.stringify(o) !== lastOutcome) {
      lastOutcome = JSON.stringify(o);
      stale = true;
      if (shown()) refresh();
    }
  });
  // Entering the Trails level shows the section (keys.js): a stale list is
  // read then (#826).
  onShown(doc, "trails", () => {
    if (stale) refresh();
  });
}

if (typeof window !== "undefined" && window.__TAURI__) {
  start("the trails view", () => wireTrails(document, window.__TAURI__));
}
