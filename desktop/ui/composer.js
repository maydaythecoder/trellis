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

import { exitText } from "./status.js";

function el(doc, tag, text) {
  const e = doc.createElement(tag);
  if (text !== undefined) e.textContent = text;
  return e;
}

function str(v) {
  return typeof v === "string" ? v : null;
}

// Whether a run is in flight, as the last view that carried it said. No
// view yet is no run: the core refuses a prompt during one regardless.
let running = false;
// Whether the child's session has a model, as the last view that carried
// `model` said. None yet is none: prompt input stays shut (#564).
let modelChosen = false;
// Why no trellis rpc child is running, as the core last said, or null while
// one is (or none has been reported missing yet).
let stopped = null;
let nextId = 0;

function lineId(kind) {
  nextId += 1;
  return `desktop-composer-${kind}-${nextId}`;
}

// The composer's elements, looked up once.
function parts(doc) {
  return {
    form: doc.getElementById("composer"),
    label: doc.getElementById("composer-label"),
    text: doc.getElementById("composer-text"),
    control: doc.getElementById("composer-control"),
    send: doc.getElementById("composer-send"),
    stop: doc.getElementById("composer-stop"),
    note: doc.getElementById("composer-note"),
    undelivered: doc.getElementById("composer-undelivered"),
    undeliveredList: doc.getElementById("composer-undelivered-list"),
    starters: doc.getElementById("composer-starters"),
    startersList: doc.getElementById("composer-starters-list"),
  };
}

// The control's value, only ever one of the two rpc accepts.
function control(p) {
  return p.control.value === "auto" ? "auto" : "approve";
}

// Sends one line through the core, and says why when it is refused.
async function send(tauri, p, line) {
  try {
    await tauri.core.invoke("rpc_send", { line });
    p.note.textContent = "";
    return true;
  } catch (err) {
    p.note.textContent = String(err);
    return false;
  }
}

// What the composer looks like for whether a run is in flight and whether a
// model is chosen: the label says what Send will do, Stop shows only during
// a run, and the starters and the control, which only a new prompt reads,
// are disabled while one runs, as colibri's settings are during a
// generation. With no model chosen, the textarea and Send are disabled too;
// with no child, the label says so before it says anything about a model.
function drawRunning(doc) {
  const p = parts(doc);
  p.label.textContent = stopped !== null
    ? "Task (trellis rpc is not running)"
    : !modelChosen
      ? "Task (choose a model first)"
      : running
        ? "Steer the run in flight"
        : "Task";
  p.stop.hidden = !running;
  p.text.disabled = !modelChosen;
  p.send.disabled = !modelChosen;
  p.control.disabled = running || !modelChosen;
  for (const b of p.startersList.querySelectorAll("button")) {
    b.disabled = running || !modelChosen;
  }
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

// One button per template, its name as its text and its description as
// its title. `null` is no answer yet, or a refused one: nothing to offer.
function drawStarters(doc, list, tauri) {
  const p = parts(doc);
  const rows = (Array.isArray(list) ? list : []).flatMap((t) => {
    const name = str(t && t.name);
    if (name === null) return [];
    const li = el(doc, "li");
    const b = el(doc, "button", name);
    b.type = "button";
    const description = str(t.description);
    if (description !== null) b.title = description;
    b.setAttribute("aria-label", `Start with the ${name} template`);
    b.disabled = running || !modelChosen;
    b.addEventListener("click", () => {
      send(tauri, p, { id: lineId("starter"), type: "prompt", prompt: name, control: control(p) });
    });
    li.append(b);
    return [li];
  });
  p.startersList.replaceChildren(...rows);
  p.starters.hidden = rows.length === 0;
}

// Draws the composer's keys of one `view` event; any other key is
// render.js's.
export function drawComposer(doc, view, tauri) {
  if (view === null || typeof view !== "object") return;
  if ("running" in view || "model" in view) {
    if ("running" in view) running = view.running === true;
    if ("model" in view) {
      modelChosen = view.model !== null && typeof view.model === "object";
    }
    drawRunning(doc);
  }
  if ("undelivered" in view) drawUndelivered(doc, view.undelivered);
  if ("starters" in view) drawStarters(doc, view.starters, tauri);
}

// Asks the child for its commands, whose prompt templates become the
// starters: on each child-started, and from main.js when the page loaded
// after its child started and so saw none.
export function askStarters(doc, tauri) {
  send(tauri, parts(doc), { id: "desktop-commands", type: "get_commands" });
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
    if (stopped !== null) {
      p.note.textContent = `trellis rpc is not running: ${stopped}`;
      return;
    }
    if (!modelChosen) {
      p.note.textContent = "Choose a model first, in the Model pane above.";
      return;
    }
    const text = p.text.value;
    if (text.trim() === "") {
      p.note.textContent = running ? "Type the text to steer the run with." : "Type a task first.";
      return;
    }
    const line = running
      ? { id: lineId("steer"), type: "steer", message: text }
      : { id: lineId("prompt"), type: "prompt", task: text, control: control(p) };
    if (await send(tauri, p, line)) p.text.value = "";
  });
  p.stop.addEventListener("click", () => {
    send(tauri, p, { id: lineId("abort"), type: "abort" });
  });
  drawRunning(doc);
}
