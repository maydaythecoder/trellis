// The Plan panel (#838; the Plan card, docs/design/system/components/
// Plan.md; ADR 0058 section 5): the model's latest plan in the run, from
// the view's `plan` key, which the reducer keeps from the run's latest
// `plan` event. render.js draws it through drawPlan. trellis sends a plan
// only when the repository enables the opt-in `plan` tool, so a run
// without one says that.
//
// The steps are the model's words (CLAUDE.md rule 10): drawn as text
// through ds.js, never counted, and a plan whose steps the reducer could
// not read is said to be unreadable with its reason, never drawn in part.
//
// A person's edit goes back as an ordinary steer, through rpc.js's send and
// the core's steer path, as the composer's Steer does: the engine never
// edits the plan, and the model changes it only by calling plan again
// (ADR 0058 section 5). The steer's text is steerHead's words and then the
// edited steps, one a line in the terminal's `[x] [>] [ ]` form, so the
// model reads both what the person changed and that it was the person.
// Edit is offered only while the reducer says a steer can reach the run
// (`can_steer`: a run in flight and not stopping); the core refuses a
// steer with no run whatever the page sends.

import { dsButton, planEditor, planNote, planSteps } from "./ds.js";
import { el } from "./dom.js";
import { isCount, plural, str } from "./fmt.js";
import { lineId, send } from "./rpc.js";

// The view's `plan` as last drawn: null with no plan event in this run.
let plan = null;
// Whether the edit box is open, and what is typed in it, kept across the
// pane's redraws (a new plan event, the run's state changing).
let editing = false;
let draft = "";
// The page's last word on an edit, drawn under the plan; null for none.
let said = null;

// How a step's state is written in the edit box.
const MARKS = { done: "[x]", current: "[>]", todo: "[ ]" };

// The words ahead of the edited steps in the steer the model reads.
function steerHead(turn) {
  const from = isCount(turn) ? `your plan from turn ${turn}` : "your plan";
  return `I edited ${from}. Work from this plan now, one step a line ([x] done, [>] current, [ ] to do):`;
}

// The steps the view holds, or null when there is no plan or it could not
// be read.
function readSteps() {
  return plan !== null && Array.isArray(plan.steps) ? plan.steps : null;
}

// The steps as the edit box shows them, one a line.
function asLines(steps) {
  return steps.map((s) => `${MARKS[str(s && s.state)] ?? "[ ]"} ${str(s && s.text) ?? ""}`).join("\n");
}

// What the plan says: its steps with what trellis cut, the unreadable
// plan's reason, or that there is none.
function planBody(doc, steps) {
  if (plan === null) {
    return [el(doc, "p", "No plan in this run. The model sends one only when the repository enables the plan tool ([tools] enable = [\"plan\"]).", "empty")];
  }
  const turn = isCount(plan.turn) ? ` from turn ${plan.turn}` : "";
  if (steps === null) {
    return [planNote(doc, `The model's plan${turn} could not be read: ${str(plan.why) ?? "no reason given"}`)];
  }
  const out = [steps.length === 0 ? el(doc, "p", "The model's plan has no steps.", "empty") : planSteps(doc, steps)];
  out.push(planNote(doc, `The model's own account of its plan${turn}, not a check's.`));
  if (isCount(plan.steps_cut) && plan.steps_cut > 0) {
    out.push(planNote(doc, `${plural(plan.steps_cut, "step", "steps")} past the eighth cut by trellis: a plan that long is a sign the task should have been split.`));
  }
  if (isCount(plan.texts_cut) && plan.texts_cut > 0) {
    out.push(planNote(doc, `${plural(plan.texts_cut, "step's text", "steps' texts")} shortened by trellis to 200 characters.`));
  }
  return out;
}

// Draws the view's `plan` into `pane` (render.js's PANES): the plan, then
// Edit or the open edit box, then the page's last word on an edit.
export function drawPlan(doc, pane, value) {
  const box = pane.querySelector("textarea[data-plan]");
  if (box !== null) draft = box.value;
  const focused = box !== null && doc.activeElement === box;
  plan = value !== null && typeof value === "object" ? value : null;
  const steps = readSteps();
  const editable = steps !== null && steps.length > 0 && plan.can_steer === true;
  if (!editable) editing = false;
  const out = planBody(doc, steps);
  if (editing) {
    out.push(planEditor(doc, draft));
  } else if (editable) {
    const edit = dsButton(doc, "Edit the plan", { variant: "ghost", title: "Your version goes to the model as a steer" });
    edit.dataset.plan = "open";
    out.push(edit);
  }
  if (said !== null) {
    const line = el(doc, "p", said, "muted");
    line.setAttribute("role", "status");
    out.push(line);
  }
  pane.replaceChildren(...out);
  if (focused) pane.querySelector("textarea[data-plan]")?.focus();
}

// Sends the edit box's plan as a steer.
async function sendEdit(doc, tauri, pane) {
  const steps = readSteps();
  const edited = draft.trim();
  if (steps === null || edited === "") {
    said = "Type the plan to send, one step a line.";
  } else if (edited === asLines(steps).trim()) {
    said = "Change a step first: an unchanged plan is not sent.";
  } else {
    const line = { id: lineId("plan", "steer"), type: "steer", message: `${steerHead(plan.turn)}\n${edited}` };
    let refused = null;
    if (await send(tauri, line, (why) => (refused = why))) {
      editing = false;
      draft = "";
      said = "Sent as a steer. The log says when a model call carries it; this plan changes only when the model sends a new one.";
    } else {
      said = refused;
    }
  }
  drawPlan(doc, pane, plan);
}

// Wires Edit, Cancel and Send once, one listener each on the pane, which
// drawPlan draws afresh.
export function wirePlan(doc, tauri) {
  const pane = doc.getElementById("pane-plan");
  pane.addEventListener("input", (e) => {
    if (e.target.matches("textarea[data-plan]")) draft = e.target.value;
  });
  pane.addEventListener("click", (e) => {
    const b = e.target.closest("button[data-plan]");
    if (b === null) return;
    const steps = readSteps();
    if (b.dataset.plan === "open" && steps !== null) {
      editing = true;
      draft = asLines(steps);
      said = null;
      drawPlan(doc, pane, plan);
      pane.querySelector("textarea[data-plan]")?.focus();
    } else if (b.dataset.plan === "cancel") {
      editing = false;
      draft = "";
      drawPlan(doc, pane, plan);
    }
  });
  pane.addEventListener("submit", (e) => {
    e.preventDefault();
    sendEdit(doc, tauri, pane);
  });
  drawPlan(doc, pane, plan);
}
