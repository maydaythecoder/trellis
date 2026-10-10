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

function el(doc, tag, text) {
  const e = doc.createElement(tag);
  if (text !== undefined) e.textContent = text;
  return e;
}

function isCount(v) {
  return typeof v === "number" && Number.isFinite(v);
}

function str(v) {
  return typeof v === "string" ? v : null;
}

// `pane` replaced by `children`, or by one muted line saying it is empty.
function fill(doc, pane, children, empty, emptyTag = "li") {
  if (children.length === 0) {
    const e = el(doc, emptyTag, empty);
    e.className = "empty";
    pane.replaceChildren(e);
  } else {
    pane.replaceChildren(...children);
  }
}

// The actions log: the transcript, as the TUI's, one line per element.
// Kept scrolled to its end, as a log is read.
function transcript(doc, pane, lines) {
  const rows = (Array.isArray(lines) ? lines : []).map((l) => el(doc, "div", String(l)));
  fill(doc, pane, rows, "Nothing has run yet.", "div");
  pane.scrollTop = pane.scrollHeight;
}

// One row per turn: the tool, its target, and its own figures, each with
// its unit. The prompt is in bytes and the reply in tokens (#312), never
// joined into one figure.
function actions(doc, pane, list) {
  const rows = (Array.isArray(list) ? list : []).map((a) => {
    const li = el(doc, "li");
    li.append(el(doc, "span", str(a.tool) ?? "?"));
    const target = str(a.target);
    if (target) li.append(" ", el(doc, "code", target));
    if (a.wrote === true) li.append(" ", el(doc, "span", "wrote"));
    const figures = [];
    if (isCount(a.prompt_bytes)) figures.push(`${a.prompt_bytes} B prompt`);
    if (isCount(a.completion_tokens)) figures.push(`${a.completion_tokens} tok out`);
    if (figures.length > 0) {
      const f = el(doc, "span", ` · ${figures.join(" · ")}`);
      f.className = "muted";
      li.append(f);
    }
    return li;
  });
  fill(doc, pane, rows, "No turns this run.");
}

function edits(doc, pane, list) {
  const rows = (Array.isArray(list) ? list : []).map((p) => {
    const li = el(doc, "li");
    li.append(el(doc, "code", String(p)));
    return li;
  });
  fill(doc, pane, rows, "No edits this run.");
}

// A check row is its kind and the word trellis's verdict maps to,
// `passed` or `failed` (DESK-08). The page decides nothing about it.
function checks(doc, pane, list) {
  const rows = (Array.isArray(list) ? list : []).map((c) => {
    const li = el(doc, "li");
    li.append(el(doc, "span", str(c.kind) ?? "check"), " ");
    const result = c.result === "passed" || c.result === "failed" ? c.result : "unknown";
    const r = el(doc, "strong", result);
    r.className = result;
    li.append(r);
    return li;
  });
  fill(doc, pane, rows, "No checks ran this run.");
}

// Three counts, never a ratio, a percentage or a sum (CLAUDE.md rule 3):
// the reach counts and the doubt count come from different populations.
function radius(doc, pane, r) {
  if (r === null || typeof r !== "object") {
    fill(doc, pane, [], "No blast radius this run.", "p");
    return;
  }
  const dl = el(doc, "dl");
  for (const [key, label] of [
    ["resolved", "resolved"],
    ["candidates", "candidates"],
    ["unresolved", "unresolved"],
  ]) {
    dl.append(el(doc, "dt", label), el(doc, "dd", isCount(r[key]) ? String(r[key]) : "not reported"));
  }
  const note = el(doc, "p", "a floor on reach, not a boundary");
  note.className = "muted";
  pane.replaceChildren(dl, note);
}

// The budget line: `iteration n/max · <bytes> B prompt · ...`. The view
// carries no elapsed time (DESK-08's budget has none), so the line says so
// rather than drawing one the core did not send; the per-turn usage
// figures follow #312's rule, and a null figure writes nothing.
export function budgetLine(b) {
  if (b === null || typeof b !== "object") return "No budget reported yet.";
  const parts = [];
  if (isCount(b.turn)) {
    parts.push(`iteration ${b.turn}/${isCount(b.max_turns) ? b.max_turns : "max not reported"}`);
  } else if (isCount(b.max_turns)) {
    // No turn reported is not turn 0.
    parts.push(`no iteration yet of ${b.max_turns}`);
  } else {
    parts.push("no iteration yet");
  }
  parts.push(isCount(b.prompt_bytes) ? `${b.prompt_bytes} B prompt` : "prompt size not reported");
  parts.push("elapsed not reported");
  const rate = str(b.tokens_per_second);
  if (isCount(b.completion_tokens) && rate !== null) {
    parts.push(`${b.completion_tokens} tok, ${rate} tok/s`);
  } else if (isCount(b.completion_tokens)) {
    parts.push(`${b.completion_tokens} tok`);
  }
  const queued = str(b.queued_seconds);
  if (queued !== null) parts.push(`queued ${queued} s`);
  return parts.join(" · ");
}

// The tier `get_state` last reported, as trellis labels it; a status bar
// writes `tier ` before the label (client_contract.json). Null means no run
// of this rpc process has decided one, and it is never guessed.
function tier(doc, pane, t) {
  pane.textContent =
    str(t) !== null ? `tier ${t}` : "Not reported: no run of this trellis rpc has decided a tier yet.";
}

function running(doc, pane, r) {
  pane.textContent = r === true ? "A run is in flight." : "No run in flight.";
}

// The last run's `run_end`, field by field as trellis sent it.
function outcome(doc, pane, o) {
  if (o === null || typeof o !== "object") {
    fill(doc, pane, [], "No run has ended yet.", "p");
    return;
  }
  const dl = el(doc, "dl");
  const row = (label, value) => dl.append(el(doc, "dt", label), el(doc, "dd", value));
  row("outcome", str(o.outcome) ?? "not reported");
  row("exit", isCount(o.exit) ? String(o.exit) : "not reported");
  if (str(o.run_id) !== null) row("trail", o.run_id);
  if (typeof o.restored === "boolean") row("restored", o.restored ? "yes" : "no");
  if (str(o.error) !== null) row("error", o.error);
  pane.replaceChildren(dl);
}

// view key -> [element id, drawing function]. `approvals` is DESK-09b's
// dialog and is not drawn here.
const PANES = {
  transcript: ["pane-transcript", transcript],
  actions: ["pane-actions", actions],
  edits: ["pane-edits", edits],
  checks: ["pane-checks", checks],
  radius: ["pane-radius", radius],
  budget: ["pane-budget", (doc, pane, b) => (pane.textContent = budgetLine(b))],
  tier: ["pane-tier", tier],
  running: ["pane-running", running],
  outcome: ["pane-outcome", outcome],
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
