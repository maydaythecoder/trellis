// The composer (#301): a task while no run is in flight, a steer while one
// is, and Stop (ADR 0048 sections 5 and 10).
//
// Whether a run is in flight is the view's `running`, the core's own record
// from `run_start` to `run_end` or the child's exit; this module keeps the
// last value it was sent and decides nothing else. The core refuses a
// `prompt` during a run and a `steer` or `abort` outside one (`rpc_send`),
// so a page that drew the wrong state still cannot send the wrong line; the
// refusal is shown as the core wrote it.
//
// Every write goes through `prompt`, and so through trellis's orchestrator
// (ADR 0048 section 10). Each `prompt` carries `control`, "auto" or
// "approve", from the composer's control, so a user can ask to approve
// each listed call without editing the repository's config.
//
// Prompt input (the textarea, Send, the control and the starters) stays
// disabled until the view's `model` says the child's session has a model
// (#564): the page is served with them `disabled`, and only that key opens
// them. The core refuses a `prompt` before then whatever the page sends.
//
// With no trellis rpc child at all (none started, or it exited), the label
// and the note say that, with the core's reason, rather than asking for a
// model: there is no session to choose one for, and "choose a model first"
// names the wrong cause (#638).
//
// The starters are the prompt templates the child's `get_commands` listed,
// and the undelivered list is what an `abort` answer handed back. Both are
// repository or user text, so they reach the DOM through textContent only
// (ADR 0007, S10).
//
// Enter starts a run (a snapshot, model calls, checks and a trail), so the
// page says what it does under the textarea; a starter only fills the
// template the next Send runs, with its description shown, never a run on
// its own click; and a sent task is acknowledged, with a button to the Run
// section, rather than the text simply vanishing (#644).
//
// Stop is one slip from Send, and what it does (abort, roll back, write a
// trail) cannot be undone, so it is drawn apart in the danger colour and a
// native dialog confirms it before any abort is sent (#637).
//
// While a run is in flight Send does one of three verbs, named on its
// button (#830, the Queue card): Steer, before the next model call; Queue,
// a `follow_up` that runs after this run; and Stop and send, which goes
// through the same dialog as Stop and sends one `abort` carrying the
// prompt as `then`. The core holds that prompt and sends it on the run's
// `run_end` and the abort's answer, never on a timer and never when the
// abort is refused (DESIGN.md §2.1); the page sequences nothing. The verb
// goes back to Steer whenever a run starts or ends, so Stop and send is
// never what Send does by default. The queue is the view's `pending`, the
// reducer's reading of trellis's own list, drawn through ds.js.

import { queueItems, queueVerb } from "./ds.js";
import { el } from "./dom.js";
import { str } from "./fmt.js";
import { enter } from "./keys.js";
import { lineId, send } from "./rpc.js";
import { exitText } from "./status.js";

// Whether a run is in flight, as the last view that carried it said. No
// view yet is no run: the core refuses a prompt during one regardless.
let running = false;
// Whether the child's session has a model, as the last view that carried
// `model` said. None yet is none: prompt input stays shut (#564).
let modelChosen = false;
// Why no trellis rpc child is running, as the core last said, or null while
// one is (or none has been reported missing yet).
let stopped = null;
// The starter chosen for the next task, `{ name, description }`, or null.
let template = null;
// What Send does while a run is in flight: "steer", "follow_up" or "stop".
let verb = "steer";
// The prompt a Stop and send asked the dialog about, while it is open; the
// dialog's close takes it, whatever closed it.
let afterStop = null;

// The verbs, each `[verb, word on the button, what it does, cool]`.
const VERBS = [
  ["steer", "Steer", "delivered before the next model call", false],
  ["follow_up", "Queue", "runs after this run ends, as a new prompt", true],
  ["stop", "Stop and send", "stops this run (rollback), then runs this once it has ended", false],
];

// The label over the textarea for each verb.
const VERB_LABELS = {
  steer: "Steer the run in flight",
  follow_up: "Queue a task for after this run",
  stop: "Stop the run, then run this task",
};

// The composer's elements, looked up once.
function parts(doc) {
  return {
    form: doc.getElementById("composer"),
    label: doc.getElementById("composer-label"),
    text: doc.getElementById("composer-text"),
    control: doc.getElementById("composer-control"),
    send: doc.getElementById("composer-send"),
    sendWord: doc.getElementById("composer-send-word"),
    verbs: doc.getElementById("composer-verbs"),
    queue: doc.getElementById("composer-queue"),
    queueList: doc.getElementById("composer-queue-list"),
    stop: doc.getElementById("composer-stop"),
    note: doc.getElementById("composer-note"),
    undelivered: doc.getElementById("composer-undelivered"),
    undeliveredList: doc.getElementById("composer-undelivered-list"),
    starters: doc.getElementById("composer-starters"),
    startersList: doc.getElementById("composer-starters-list"),
    template: doc.getElementById("composer-template"),
    templateText: doc.getElementById("composer-template-text"),
    templateClear: doc.getElementById("composer-template-clear"),
    follow: doc.getElementById("composer-follow"),
    stopConfirm: doc.getElementById("stop-confirm"),
    stopCancel: doc.getElementById("stop-confirm-cancel"),
    stopThen: doc.getElementById("stop-confirm-then"),
  };
}

// The control's value, only ever one of the two rpc accepts.
function control(p) {
  return p.control.value === "auto" ? "auto" : "approve";
}

// Sends one line through the core (rpc.js's send): the note says why when
// it is refused, and is cleared when it is not.
async function forward(tauri, p, line) {
  const sent = await send(tauri, line, (why) => {
    p.note.textContent = why;
  });
  if (sent) p.note.textContent = "";
  return sent;
}

// Whether the starters and the control, which only a new prompt reads,
// apply: with no run in flight, or for the prompt a Stop and send runs.
function promptInput() {
  return modelChosen && (!running || verb === "stop");
}

// What the composer looks like for whether a run is in flight and whether a
// model is chosen: the label and Send's word say what Send will do, Stop
// and the verbs show only during a run, and the starters and the control
// are disabled while one runs unless Stop and send will start a new one,
// as colibri's settings are during a generation. With no model chosen, the
// textarea and Send are disabled too; with no child, the label says so
// before it says anything about a model.
function drawRunning(doc) {
  const p = parts(doc);
  p.label.textContent = stopped !== null
    ? "Task (trellis rpc is not running)"
    : !modelChosen
      ? "Task (choose a model first)"
      : running
        ? VERB_LABELS[verb]
        : "Task";
  p.stop.hidden = !running;
  p.text.disabled = !modelChosen;
  p.send.disabled = !modelChosen;
  p.sendWord.textContent = running ? VERBS.find((v) => v[0] === verb)[1] : "Send";
  p.control.disabled = !promptInput();
  for (const b of p.startersList.querySelectorAll("button")) {
    b.disabled = !promptInput();
  }
  p.verbs.hidden = !running;
  p.verbs.replaceChildren(
    ...VERBS.map(([v, word, does, cool]) => {
      const b = queueVerb(doc, word, { on: v === verb, cool, title: does });
      b.dataset.verb = v;
      return b;
    }),
  );
}

// The queue: each item the view's `pending` holds, numbered, with its
// state. Shown whenever it has an item (the Queue card: never hidden then).
function drawPending(doc, list) {
  const p = parts(doc);
  const items = (Array.isArray(list) ? list : []).filter((q) => q !== null && typeof q === "object");
  p.queueList.replaceChildren(...queueItems(doc, items));
  p.queue.hidden = items.length === 0;
}

function drawUndelivered(doc, list) {
  const p = parts(doc);
  const rows = (Array.isArray(list) ? list : []).map((u) => {
    const kind = str(u && u.type) === "follow_up" ? "follow-up" : "steer";
    const li = el(doc, "li");
    li.append(el(doc, "span", `${kind}: `), el(doc, "span", str(u && u.message) ?? ""));
    return li;
  });
  p.undeliveredList.replaceChildren(...rows);
  p.undelivered.hidden = rows.length === 0;
}

// The chosen template, shown beside the textarea with its description, or
// nothing with none chosen.
function drawTemplate(doc) {
  const p = parts(doc);
  p.template.hidden = template === null;
  if (template === null) p.templateText.textContent = "";
  else if (template.description === null) p.templateText.textContent = `Template: ${template.name}`;
  else p.templateText.textContent = `Template: ${template.name} — ${template.description}`;
}

// One button per template, its name as its text and its description as
// its title. `null` is no answer yet, or a refused one: nothing to offer.
// A click chooses the template for the next Send and sends nothing; a
// chosen one the new list lacks (another repository's) is dropped.
function drawStarters(doc, list) {
  const p = parts(doc);
  const names = [];
  const rows = (Array.isArray(list) ? list : []).flatMap((t) => {
    const name = str(t && t.name);
    if (name === null) return [];
    const li = el(doc, "li");
    const b = el(doc, "button", name);
    b.type = "button";
    const description = str(t.description);
    if (description !== null) b.title = description;
    b.setAttribute("aria-label", `Use the ${name} template for the next task`);
    b.disabled = !promptInput();
    b.addEventListener("click", () => {
      template = { name, description };
      drawTemplate(doc);
      p.text.focus();
    });
    names.push(name);
    li.append(b);
    return [li];
  });
  p.startersList.replaceChildren(...rows);
  p.starters.hidden = rows.length === 0;
  if (template !== null && !names.includes(template.name)) {
    template = null;
    drawTemplate(doc);
  }
}

// Draws the composer's keys of one `view` event; any other key is
// render.js's.
export function drawComposer(doc, view) {
  if (view === null || typeof view !== "object") return;
  if ("running" in view || "model" in view) {
    if ("running" in view) {
      const now = view.running === true;
      if (now !== running) verb = "steer";
      running = now;
    }
    if ("model" in view) {
      modelChosen = view.model !== null && typeof view.model === "object";
    }
    drawRunning(doc);
  }
  if ("undelivered" in view) drawUndelivered(doc, view.undelivered);
  if ("pending" in view) drawPending(doc, view.pending);
  if ("starters" in view) drawStarters(doc, view.starters);
}

// Asks the child for its commands, whose prompt templates become the
// starters: on each child-started, and from main.js when the page loaded
// after its child started and so saw none.
export function askStarters(doc, tauri) {
  forward(tauri, parts(doc), { id: "desktop-commands", type: "get_commands" });
}

// No trellis rpc child is running, for `why`: from `no-child`, from
// `rpc-exited`, or from main.js when a page that loaded late had the
// core refuse its `get_state` for want of one, having heard neither.
export function composerStopped(doc, why) {
  stopped = why;
  parts(doc).note.textContent = `trellis rpc is not running: ${why}`;
  drawRunning(doc);
}

// Wires the form once, and listens for the child's events: `child-started`
// is a repository whose templates are asked for, and ends whatever
// `no-child` or `rpc-exited` said. Resolves once the listeners are
// registered, so the caller can ask for nothing before they are. Enter
// sends and Shift+Enter is a newline, the textarea's own behaviour; while
// an input method is composing, Enter is its, not the form's.
export async function wireComposer(doc, tauri) {
  const p = parts(doc);
  await tauri.event.listen("child-started", () => {
    if (stopped !== null) {
      stopped = null;
      p.note.textContent = "";
      drawRunning(doc);
    }
    askStarters(doc, tauri);
  });
  await tauri.event.listen("no-child", (event) => composerStopped(doc, String(event.payload)));
  await tauri.event.listen("rpc-exited", (event) => {
    composerStopped(doc, `it ${exitText(event.payload)}; start it again from the Overview`);
  });
  p.text.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      p.form.requestSubmit();
    }
  });
  p.form.addEventListener("submit", async (e) => {
    e.preventDefault();
    p.follow.hidden = true;
    if (stopped !== null) {
      p.note.textContent = `trellis rpc is not running: ${stopped}`;
      return;
    }
    if (!modelChosen) {
      p.note.textContent = "Choose a model first, in the Model pane above.";
      return;
    }
    const text = p.text.value;
    if (running && verb === "stop") {
      if (text.trim() === "" && template === null) {
        p.note.textContent = "Type the task to run once this run has stopped, or choose a starter.";
        return;
      }
      // Asked as Stop is: the dialog's close sends the one abort, with
      // this prompt, or nothing.
      afterStop = { id: lineId("composer", "prompt"), type: "prompt", control: control(p) };
      if (text.trim() !== "") afterStop.task = text;
      if (template !== null) afterStop.prompt = template.name;
      p.stopThen.hidden = false;
      p.stopConfirm.returnValue = "";
      p.stopConfirm.showModal();
      p.stopCancel.focus();
      return;
    }
    if (running) {
      if (text.trim() === "") {
        p.note.textContent = verb === "follow_up" ? "Type the task to run after this one." : "Type the text to steer the run with.";
        return;
      }
      if (verb === "follow_up") {
        if (await forward(tauri, p, { id: lineId("composer", "follow_up"), type: "follow_up", message: text })) {
          p.text.value = "";
          p.note.textContent = "Queued: trellis runs it after this run ends.";
        }
        return;
      }
      if (await forward(tauri, p, { id: lineId("composer", "steer"), type: "steer", message: text })) p.text.value = "";
      return;
    }
    if (text.trim() === "" && template === null) {
      p.note.textContent = "Type a task first, or choose a starter.";
      return;
    }
    // A template with no text is a whole task; with text, rpc runs the
    // template with the text after it.
    const line = { id: lineId("composer", "prompt"), type: "prompt", control: control(p) };
    if (text.trim() !== "") line.task = text;
    if (template !== null) line.prompt = template.name;
    if (await forward(tauri, p, line)) {
      p.text.value = "";
      template = null;
      drawTemplate(doc);
      p.note.textContent = "Sent to trellis; follow it in Run.";
      p.follow.hidden = false;
    }
  });
  p.templateClear.addEventListener("click", () => {
    template = null;
    drawTemplate(doc);
    p.text.focus();
  });
  // Entering Run as the overview's Run panel does (keys.js), so the Trail,
  // the sibling rail and the way back are the same.
  p.follow.addEventListener("click", () => enter(doc, "run"));
  // Stop aborts the run and rolls its edits back, which cannot be undone,
  // so a click only asks (#637). The dialog's close is the one place an
  // abort is sent, and only when its Stop button closed it: Escape, Cancel
  // and any other close send nothing. Cancel takes the focus, so a stray
  // Enter stops nothing.
  p.stop.addEventListener("click", () => {
    afterStop = null;
    p.stopThen.hidden = true;
    p.stopConfirm.returnValue = "";
    p.stopConfirm.showModal();
    p.stopCancel.focus();
  });
  // A Stop and send's prompt rides on the same abort as `then` (#830).
  p.stopConfirm.addEventListener("close", () => {
    const then = afterStop;
    afterStop = null;
    p.stopThen.hidden = true;
    if (p.stopConfirm.returnValue !== "stop") return;
    const line = { id: lineId("composer", "abort"), type: "abort" };
    if (then !== null) line.then = then;
    forward(tauri, p, line).then((sent) => {
      if (!sent || then === null) return;
      p.text.value = "";
      template = null;
      drawTemplate(doc);
      p.note.textContent = "Stopping; your task runs once trellis has ended this run.";
    });
  });
  // One listener for the verbs, which drawRunning draws afresh.
  p.verbs.addEventListener("click", (e) => {
    const b = e.target.closest("button[data-verb]");
    if (b === null || !VERBS.some((v) => v[0] === b.dataset.verb)) return;
    verb = b.dataset.verb;
    drawRunning(doc);
  });
  drawRunning(doc);
}
