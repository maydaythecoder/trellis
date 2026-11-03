// The model pane (#564, #306; ADR 0048 sections 4 and 5 as amended, ADR 0050
// section 2).
//
// The desktop starts trellis with the user's environment unchanged, keys
// included (ADR 0048 section 3, amended), on the owner's condition that it
// asks which model to use after launch and keeps prompt input disabled until
// one is chosen. This pane is where it asks, and it is the model picker: the
// choices are settings.js's `modelChoices`, from the core's own models
// answer (the view's `available`) and the child's `list_models` answer (the
// view's `models`), with the user's configured spec preselected. Where trellis
// looked for local runtimes, and the user's own providers by the variable a
// key would be read from (never a value), are listed beside them.
//
// Choosing sends one `set_model` through the core's `rpc_send`, where the
// model guard (ADR 0048 section 4) refuses a hosted spec that is not the
// user's own; a refusal is shown as the core wrote it. With "save" ticked,
// trellis writes the spec to the user's own config, never the
// repository's. Whether a model has been chosen is the view's `model`, the
// core's record of the child's answer; the composer reads the same key, and
// the core refuses a prompt before it.
//
// With no trellis rpc child at all (none started, or it exited), nothing is
// asked and there is no session to choose a model for: the pane says that,
// with the core's reason, instead of a list it is still "asking" for, and
// does not open itself to ask for a choice nobody can take (#638).
//
// The pane is the status bar's last cell (#909): its summary is a
// design-system Status naming the model, and it opens as a popover over
// the stage; ESC, a click outside or the summary again closes it. Each
// runtime trellis probed is a Status, with trellis's own words for why it
// did not answer as the row's title rather than its text.
//
// Every spec, address, error and provider name is trellis- or
// configuration-supplied text, so it reaches the DOM through textContent
// only (ADR 0007, S10).

import { el } from "./dom.js";
import { dsStatus } from "./ds.js";
import { str } from "./fmt.js";
import { onEscape } from "./keys.js";
import { lineId, send } from "./rpc.js";
import { modelChoices } from "./settings.js";
import { exitText } from "./status.js";

// The two view keys the choices come from, as last sent.
let available = null;
let models = null;
// The choices drawn, by radio value.
let offered = new Map();
// Whether the child's session had a model when the view last said.
let chosen = false;
// The view's `model` as last sent, redrawn when the child's state changes.
let lastModel = null;
// Why no trellis rpc child is running, as the core last said, or null while
// one is (or none has been reported missing yet).
let stopped = null;

function parts(doc) {
  return {
    pane: doc.getElementById("model-pane"),
    summary: doc.getElementById("model-summary"),
    form: doc.getElementById("model"),
    current: doc.getElementById("model-current"),
    options: doc.getElementById("model-options"),
    typed: doc.getElementById("model-typed"),
    save: doc.getElementById("model-save"),
    refresh: doc.getElementById("model-refresh"),
    probed: doc.getElementById("model-probed"),
    providers: doc.getElementById("model-providers"),
    note: doc.getElementById("model-note"),
  };
}

// The selected radio's choice, or null with none selected.
function selected(p) {
  const r = p.options.querySelector('input[name="model-choice"]:checked');
  return r ? offered.get(r.value) ?? null : null;
}

// Saving applies to a spec the user names that is not already in a config
// file; trellis refuses to save the configured one again.
function drawSave(p) {
  const c = selected(p);
  p.save.disabled = c === null || !c.saves;
  if (p.save.disabled) p.save.checked = false;
}

// One radio and its label: the choice's label, and its spec as code. Its
// value is the choice's, which names the spec; its id is its place in this
// drawing, since a spec is a runtime's text and only labels the radio.
function option(doc, p, c, k, checked) {
  const li = el(doc, "li");
  const input = doc.createElement("input");
  input.type = "radio";
  input.name = "model-choice";
  input.value = c.value;
  input.id = `model-choice-${k}`;
  input.checked = checked;
  input.addEventListener("change", () => drawSave(p));
  const label = el(doc, "label", c.label);
  label.htmlFor = input.id;
  li.append(input, " ", label);
  if (c.spec !== null) li.append(" ", el(doc, "code", c.spec));
  return li;
}

// The choices, keeping the user's selection when it is still offered: by
// value, which names the spec, so a list that comes back reordered keeps
// the same spec selected.
function drawChoices(doc) {
  const p = parts(doc);
  const before = p.options.querySelector('input[name="model-choice"]:checked');
  const keep = before ? before.value : null;
  const choices = modelChoices(available, models);
  offered = new Map(choices.map((c) => [c.value, c]));
  const kept = keep !== null && offered.has(keep);
  p.options.replaceChildren(
    ...choices.map((c, k) => option(doc, p, c, k, kept ? c.value === keep : c.preselect)),
  );
  drawSave(p);
}

// Where trellis looked, and the user's providers, from `list_models`.
function drawFound(doc) {
  const p = parts(doc);
  const probed = [];
  const providers = [];
  if (stopped !== null) {
    p.note.textContent = "Not asked: trellis rpc is not running.";
  } else if (models === null || typeof models !== "object") {
    p.note.textContent = "Asking trellis which models there are.";
  } else if (str(models.error) !== null) {
    p.note.textContent = `trellis could not list models: ${models.error}`;
  } else {
    p.note.textContent = "";
    for (const t of Array.isArray(models.probed) ? models.probed : []) {
      const said = str(t && t.error);
      const li = el(doc, "li");
      li.append(
        said === null ? dsStatus(doc, "ok", "answered") : dsStatus(doc, "idle", "no answer", { title: said }),
        el(doc, "span", str(t && t.runtime) ?? "?"),
        el(doc, "code", str(t && t.address) ?? "?"),
      );
      if (said !== null) li.title = said;
      probed.push(li);
    }
    for (const v of Array.isArray(models.providers) ? models.providers : []) {
      const name = str(v && v.name);
      if (name === null) continue;
      const variable = str(v.key_variable);
      const li = el(doc, "li");
      li.append(
        el(doc, "code", `provider:${name}#<model>`),
        el(
          doc,
          "span",
          variable === null ? "a protocol trellis does not know" : `key from ${variable} in trellis's environment`,
          "muted",
        ),
      );
      providers.push(li);
    }
  }
  p.probed.replaceChildren(...probed);
  p.providers.replaceChildren(...providers);
}

// The summary: a Status in `tone` naming `word`, cut to the bar with the
// whole of it, and `title`, on hover.
function drawChip(doc, p, tone, word, title) {
  p.summary.replaceChildren(dsStatus(doc, tone, el(doc, "span", word, "cut"), { title }));
}

// The session's model as the child last confirmed it, and where it came
// from (`repository`, `user` or `session`, the view's `model.source`, null
// when the core cannot tell, and then said so, never guessed). The pane
// stays open until one is chosen, and closes on the first choice. With no
// child there is no session, which the pane says without opening.
function drawModel(doc, model) {
  const p = parts(doc);
  const spec = model && str(model.spec);
  const was = chosen;
  chosen = spec !== null && spec !== undefined;
  if (!chosen && stopped !== null) {
    drawChip(doc, p, "idle", "no model", `trellis rpc is not running: ${stopped}`);
    p.current.textContent = `trellis rpc is not running: ${stopped}`;
    return;
  }
  if (!chosen) {
    drawChip(doc, p, "signal", "no model", "No model is chosen for this window yet.");
    p.current.textContent = "Prompt input stays off until trellis takes a model.";
    p.pane.open = true;
    return;
  }
  const saved = str(model.saved_to);
  const source = str(model.source);
  drawChip(doc, p, "ok", spec, source === null ? `${spec} (source not known)` : `${spec} (from ${source})`);
  p.current.textContent = `Prompts run on ${spec}.` + (saved === null ? "" : ` Saved to ${saved}.`);
  if (!was) p.pane.open = false;
}

// Draws the model pane's keys of one `view` event; any other key is
// another module's.
export function drawModelPane(doc, view) {
  if (view === null || typeof view !== "object") return;
  if ("available" in view) available = view.available;
  if ("models" in view) {
    models = view.models;
    drawFound(doc);
  }
  if ("available" in view || "models" in view) drawChoices(doc);
  if ("model" in view) {
    lastModel = view.model;
    drawModel(doc, lastModel);
  }
}

// No trellis rpc child is running, for `why`: from `no-child`, from
// `rpc-exited`, or from main.js when a page that loaded late had the
// core refuse its `get_state` for want of one, having heard neither.
export function modelPaneStopped(doc, why) {
  stopped = why;
  drawFound(doc);
  drawModel(doc, lastModel);
}

// Sends one line through the core (rpc.js's send); the note says why when
// it is refused.
function forward(tauri, p, line) {
  return send(tauri, line, (why) => {
    p.note.textContent = why;
  });
}

// Asks the child what there is to choose from.
export function askModels(doc, tauri) {
  const p = parts(doc);
  return forward(tauri, p, { id: lineId("model", "list"), type: "list_models" });
}

// Wires the pane once, and listens for the child's events: `child-started`
// is a session with no model, and a repository whose configured spec may
// differ, and ends whatever `no-child` or `rpc-exited` said.
export async function wireModelPane(doc, tauri) {
  const p = parts(doc);
  await tauri.event.listen("child-started", () => {
    if (stopped !== null) {
      stopped = null;
      drawFound(doc);
      drawModel(doc, lastModel);
    }
    askModels(doc, tauri);
  });
  await tauri.event.listen("no-child", (event) => modelPaneStopped(doc, String(event.payload)));
  await tauri.event.listen("rpc-exited", (event) => {
    modelPaneStopped(doc, `it ${exitText(event.payload)}; start it again from the Overview`);
  });
  p.typed.addEventListener("input", () => {
    const typed = p.options.querySelector('input[name="model-choice"][value="typed"]');
    if (typed) typed.checked = true;
    drawSave(p);
  });
  p.refresh.addEventListener("click", () => askModels(doc, tauri));
  // ESC closes the chooser before it goes out a level (keys.js asks this
  // first).
  onEscape(() => {
    if (!p.pane.open) return false;
    p.pane.open = false;
    p.summary.focus();
    return true;
  });
  p.form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const c = selected(p);
    if (c === null) {
      p.note.textContent = "Choose one of the models listed, or type a spec.";
      return;
    }
    const line = c.line(p.save.checked, p.typed.value);
    if (line === null) {
      p.note.textContent = "Type a model spec first.";
      return;
    }
    if (await forward(tauri, p, line)) {
      p.note.textContent = "Sent; prompt input opens once trellis has taken it.";
    }
  });
  drawFound(doc);
  drawChoices(doc);
  drawModel(doc, null);
}
