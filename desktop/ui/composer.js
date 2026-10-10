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
// The starters are the prompt templates the child's `get_commands` listed,
// and the undelivered list is what an `abort` answer handed back. Both are
// repository or user text, so they reach the DOM through textContent only
// (ADR 0007, S10).

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

// What the composer looks like for whether a run is in flight: the label
// says what Send will do, Stop shows only during a run, and the starters
// and the control, which only a new prompt reads, are disabled while one
// runs, as colibri's settings are during a generation.
function drawRunning(doc) {
  const p = parts(doc);
  p.label.textContent = running ? "Steer the run in flight" : "Task";
  p.stop.hidden = !running;
  p.control.disabled = running;
  for (const b of p.startersList.querySelectorAll("button")) b.disabled = running;
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
    b.disabled = running;
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
  if ("running" in view) {
    running = view.running === true;
    drawRunning(doc);
  }
  if ("undelivered" in view) drawUndelivered(doc, view.undelivered);
  if ("starters" in view) drawStarters(doc, view.starters, tauri);
}

// Asks the child for its commands, whose prompt templates become the
// starters.
function askStarters(tauri, p) {
  send(tauri, p, { id: "desktop-commands", type: "get_commands" });
}

// Wires the form once, and listens for `child-started`: each new child is
// a repository whose templates are asked for. Resolves once the listener is
// registered, so the caller can ask for nothing before it is. Enter sends
// and Shift+Enter is a newline, the textarea's own behaviour; while an
// input method is composing, Enter is its, not the form's.
export async function wireComposer(doc, tauri) {
  const p = parts(doc);
  await tauri.event.listen("child-started", () => askStarters(tauri, p));
  p.text.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      p.form.requestSubmit();
    }
  });
  p.form.addEventListener("submit", async (e) => {
    e.preventDefault();
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
