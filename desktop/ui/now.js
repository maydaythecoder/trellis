// The Now strip and Where it is (#828; DESIGN.md §2.1 and §3.3, the
// design system's NowStrip card).
//
// The reducer (`desktop/src/state.rs`, the view's `now` and `where` keys)
// decides everything these draw: the state word, the clock, which turn and
// tool the run is on, and whether Stop applies; this module only draws
// what it was sent, through ds.js's components. The clock is trellis's
// elapsed_ms as the reducer last had it, never a clock of the page's own
// (ADR 0048 section 8), so nothing here ticks between events.
//
// Stop opens the same confirmation the composer's Stop opens, and only
// opens it: the dialog's close, in composer.js, is the one place an abort
// is sent, and only when its own Stop closed it (#637). Summary so far,
// or Summary once the run has ended, opens the summary level (#832) one
// burrow in from Run, and sends nothing.
//
// Every value is trellis- or model-supplied text, written through
// textContent only (ADR 0007, S10).

import { el } from "./dom.js";
import { asciiFace, dsButton, dsStatus, facts, nowStrip, unreported } from "./ds.js";
import { isCount, str } from "./fmt.js";
import { burrow } from "./keys.js";
import { currentRegister } from "./settings.js";

// The bundle's NowStrip state each of the reducer's states is coloured as.
// The word drawn is the reducer's, never the bundle's: an ended run reads
// run_end's own outcome, never `done` or `failed` (DESIGN.md §2.1).
const NOW_TONES = { idle: "idle", running: "running", needs_you: "waiting", stopping: "failed" };

// An ended run's tone: converged as `done`'s, every other outcome as
// `failed`'s, as the outcome pane tones them.
function endedTone(outcome) {
  return outcome === "converged" ? "done" : "failed";
}

// The tool the run is on and its target, the tool in bold as the bundle
// draws it; or the model call, in words.
function onNow(doc, n) {
  const tool = str(n.tool);
  if (tool !== null) {
    const w = el(doc, "span");
    w.append(el(doc, "b", tool));
    const target = str(n.target);
    if (target !== null) w.append(` → ${target}`);
    return w;
  }
  return str(n.phase) === "model" ? "calling the model" : null;
}

// The register dial's face and copy (#836), from the reducer's `register`
// for the position the dial is in: nothing else in this module reads the
// dial, so moving it changes no figure, state word or control. At most one
// face, since the reducer sends at most one per position.
function registerFace(doc, r) {
  const said = r !== null && typeof r === "object" ? r[currentRegister()] : null;
  if (said === null || typeof said !== "object" || str(said.face) === null) return [];
  return [asciiFace(doc, { face: said.face, caption: str(said.copy) })];
}

// The last `now` drawn, so the dial redraws the strip without an event;
// undefined until the first view.
let lastNow;

// Redraws the Now strip from the last `now`, for the register dial.
export function redrawNow(doc) {
  const pane = doc.getElementById("pane-now");
  if (pane && lastNow !== undefined) drawNow(doc, pane, lastNow);
}

// The Now strip, from the view's `now`.
export function drawNow(doc, pane, n) {
  lastNow = n;
  const v = n !== null && typeof n === "object" ? n : {};
  const state = str(v.state) ?? "idle";
  const word = str(v.word) ?? "idle";
  const clock = str(v.elapsed) ?? (str(v.elapsed_why) !== null ? unreported(doc, v.elapsed_why) : null);
  let turn = null;
  if (isCount(v.turn)) turn = isCount(v.max_turns) ? `turn ${v.turn} / ${v.max_turns}` : `turn ${v.turn}`;
  let wait = null;
  if (state === "needs_you") wait = str(v.reason);
  else if (state === "ended" && isCount(v.exit)) wait = `exit ${v.exit}`;
  let model = null;
  if (str(v.model) !== null) model = str(v.tier) !== null ? `${v.model} · tier ${v.tier}` : v.model;
  const controls = [];
  if (v.can_stop === true) {
    const stop = dsButton(doc, "Stop", { variant: "down", title: "Stop the run: trellis rolls back its edits and writes a trail" });
    stop.setAttribute("aria-haspopup", "dialog");
    stop.setAttribute("aria-controls", "stop-confirm");
    stop.addEventListener("click", () => {
      const dialog = doc.getElementById("stop-confirm");
      dialog.returnValue = "";
      dialog.showModal();
      doc.getElementById("stop-confirm-cancel").focus();
    });
    controls.push(stop);
  }
  if (state !== "idle") {
    const summary = dsButton(doc, state === "ended" ? "Summary" : "Summary so far", {
      variant: "ghost",
      title: state === "ended" ? "How the run ended, what it changed and its trail" : "What the run has done so far: it has not ended",
    });
    summary.addEventListener("click", () => burrow(doc, ["run", "summary"]));
    controls.push(summary);
  }
  pane.replaceChildren(
    nowStrip(doc, {
      tone: state === "ended" ? endedTone(str(v.outcome)) : NOW_TONES[state] ?? "idle",
      word,
      clock,
      turn,
      where: state === "ended" || state === "idle" ? null : onNow(doc, v),
      wait,
      model,
      controls,
    }),
    ...registerFace(doc, v.register),
  );
}

// `v` as text when trellis reported it, else unreported for `why`.
function fact(doc, v, why) {
  return str(v) ?? unreported(doc, why);
}

// Where it is, from the view's `where`: each fact as trellis reported it,
// and unreported, with why, where it did not.
export function drawWhere(doc, pane, w) {
  const v = w !== null && typeof w === "object" ? w : {};
  let tool = unreported(doc, "the run is on no tool now");
  if (str(v.tool) !== null) {
    tool = el(doc, "span");
    tool.append(el(doc, "b", v.tool));
    if (str(v.target) !== null) tool.append(" → ", el(doc, "code", v.target));
  }
  let turn = unreported(doc, "no turn has started");
  if (isCount(v.turn)) turn = isCount(v.max_turns) ? `${v.turn} of ${v.max_turns}` : String(v.turn);
  const call = [];
  if (isCount(v.model_ms)) call.push(`${v.model_ms} ms`);
  if (isCount(v.completion_tokens)) call.push(`${v.completion_tokens} tok out`);
  const r = v.runtime !== null && typeof v.runtime === "object" ? v.runtime : null;
  let runtime = unreported(doc, "no runtime has identified itself for this run");
  if (r !== null) {
    runtime = el(doc, "span");
    runtime.append(dsStatus(doc, "cool", str(r.runtime) ?? "runtime not reported"), " ", el(doc, "code", str(r.served) ?? "not reported"));
  }
  const root = str(v.root);
  let rootValue = unreported(doc, "trellis has not said which repository it serves yet");
  if (root !== null) {
    rootValue = el(doc, "code", root, "cut");
    rootValue.title = root;
  }
  pane.replaceChildren(
    facts(doc, [
      { label: "phase", value: fact(doc, v.phase, "trellis has said nothing of this run") },
      { label: "tool", value: tool },
      { label: "turn", value: turn },
      { label: "last call", value: call.length > 0 ? call.join(" · ") : unreported(doc, "no turn has reported a model call") },
      { label: "model", value: fact(doc, v.model, "no run or get_state has named one") },
      { label: "tier", value: fact(doc, v.tier, "no run of this trellis rpc has decided a tier yet") },
      { label: "runtime", value: runtime },
      // trellis sends no control mode to a client: a prompt that names
      // none runs in the repository's, which it does not report either.
      { label: "control", value: unreported(doc, "trellis reports no control mode to a client") },
      { label: "root", value: rootValue },
    ]),
  );
}
