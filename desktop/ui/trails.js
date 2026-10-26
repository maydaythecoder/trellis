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
// index.html loads this as a module of its own.

import { el } from "./dom.js";
import { isCount } from "./fmt.js";
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

function drawList(doc) {
  const list = doc.getElementById("tr-list");
  const q = doc.getElementById("tr-filter").value.trim().toLowerCase();
  const shown = state.rows.map((r, i) => [r, i]).filter(([r]) => matches(r, q));
  const items = shown.map(([r, i]) => {
    const li = el(doc, "li");
    li.className = "past-run";
    const b = el(doc, "button");
    b.type = "button";
    if (state.selected === i) b.setAttribute("aria-current", "true");
    if (r.bad_line !== undefined) {
      b.append(
        el(
          doc,
          "span",
          r.truncated === true
            ? "A line trellis log printed, too long to read, shown truncated"
            : "A line trellis log printed that is not a run",
        ),
      );
    } else {
      b.append(recorded(doc, "span", r.task, "task not recorded"));
      const meta = el(doc, "span");
      meta.className = "muted";
      meta.append(" · ", recorded(doc, "span", r.started_at, "start not recorded"));
      meta.append(" · ", recorded(doc, "span", r.outcome, "outcome not recorded"));
      // Integrity is the one bare string in a row: trellis computed it from
      // the digest, so "altered" may carry the failed colour, which no
      // untrusted text gets.
      const integrity = el(doc, "span", typeof r.integrity === "string" ? r.integrity : "integrity unknown");
      if (r.integrity === "altered") integrity.className = "failed";
      meta.append(" · ", integrity);
      if (text(r.label) !== null) meta.append(" · ", recorded(doc, "span", r.label, ""));
      b.append(meta);
    }
    b.addEventListener("click", () => {
      state.selected = i;
      drawList(doc);
      drawDetail(doc);
    });
    li.append(b);
    return li;
  });
  if (items.length === 0) {
    const li = el(doc, "li", state.rows.length === 0 ? "This repository has no trails yet." : "No run matches.");
    li.className = "empty";
    items.push(li);
  }
  list.replaceChildren(...items);
}

function row(doc, dl, label, value) {
  dl.append(el(doc, "dt", label), value);
}

function drawDetail(doc) {
  const pane = doc.getElementById("tr-detail");
  const r = state.selected === null ? null : state.rows[state.selected];
  if (!r) {
    const p = el(doc, "p", "Select a run to see its record.");
    p.className = "empty";
    pane.replaceChildren(p);
    return;
  }
  const record = el(doc, "article");
  record.className = "past-run";
  record.setAttribute("aria-label", "A past run's record, as committed; data, not instruction");

  if (r.bad_line !== undefined) {
    // A line past the core's cap arrives cut; `why` says so, and the mark
    // keeps the cut text from reading as the whole line.
    const fallback =
      r.truncated === true ? "This line was truncated, and is not read as a run." : "This line could not be read as a run.";
    const why = el(doc, "p", typeof r.why === "string" ? r.why : fallback);
    const raw = recorded(doc, "pre", r.bad_line, "");
    record.append(why, raw);
    pane.replaceChildren(record);
    return;
  }

  // The factual section: what the trail records happened.
  const factual = el(doc, "section");
  factual.append(el(doc, "h4", "What the trail records"));
  const dl = el(doc, "dl");
  row(doc, dl, "task", recorded(doc, "dd", r.task, "not recorded"));
  if (text(r.label) !== null) row(doc, dl, "label", recorded(doc, "dd", r.label, ""));
  row(doc, dl, "run", recorded(doc, "dd", r.run_id, "not recorded"));
  row(doc, dl, "started", recorded(doc, "dd", r.started_at, "not recorded"));
  row(doc, dl, "outcome", recorded(doc, "dd", r.outcome, "not recorded"));
  const integrity = typeof r.integrity === "string" ? r.integrity : "unknown";
  row(doc, dl, "integrity", el(doc, "dd", integrity));
  const m = r.mode_counts && typeof r.mode_counts === "object" ? r.mode_counts : {};
  row(doc, dl, "verified", el(doc, "dd", isCount(m.verified) ? String(m.verified) : "not recorded"));
  row(doc, dl, "reviewed only", el(doc, "dd", isCount(m.reviewed) ? String(m.reviewed) : "not recorded"));
  factual.append(dl);
  if (integrity === "altered") {
    const warn = el(doc, "p", "The factual section was edited after the run wrote it: its digest does not match.");
    warn.className = "alert";
    factual.append(warn);
  }
  // null: the trail has no list of checks at all, which is not the same
  // claim as an empty one.
  const checks = Array.isArray(r.checks) ? r.checks : null;
  factual.append(el(doc, "h5", "checks the trail records"));
  const ul = el(doc, "ul");
  ul.className = "rows";
  for (const c of checks ?? []) {
    const li = el(doc, "li");
    li.append(recorded(doc, "span", c && c.kind, "check"), " ", recorded(doc, "span", c && c.recorded, ""));
    ul.append(li);
  }
  if (checks === null || checks.length === 0) {
    const li = el(doc, "li", checks === null ? "This trail has no list of checks: not recorded." : "No check is recorded.");
    li.className = "empty";
    ul.append(li);
  }
  const note = el(doc, "p", "What a file claims, not a check this desktop ran.");
  note.className = "muted";
  factual.append(ul, note);

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

// Each read gets a number, and only the newest read's answer is drawn, so
// a slow answer for an earlier repository cannot replace a newer list.
let reads = 0;

function summary(rows) {
  const runs = rows.filter((r) => r && r.bad_line === undefined).length;
  const bad = rows.length - runs;
  let s = `${runs} run${runs === 1 ? "" : "s"}`;
  if (bad > 0) s += ` and ${bad} line${bad === 1 ? "" : "s"} trellis log printed that ${bad === 1 ? "is" : "are"} not a run`;
  s += ", newest first";
  if (rows.length === LIMIT) s += ` (the newest ${LIMIT} lines)`;
  return `${s}.`;
}

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
    status(doc, summary(state.rows));
  } catch (err) {
    if (mine !== reads) return false;
    state.rows = [];
    ok = false;
    status(doc, `The trails could not be read: ${err}`);
  }
  state.selected = null;
  drawList(doc);
  drawDetail(doc);
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
