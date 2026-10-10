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
// Every spec, address, error and provider name is trellis- or
// configuration-supplied text, so it reaches the DOM through textContent
// only (ADR 0007, S10).

import { modelChoices } from "./settings.js";

function el(doc, tag, text) {
  const e = doc.createElement(tag);
  if (text !== undefined) e.textContent = text;
  return e;
}

function str(v) {
  return typeof v === "string" ? v : null;
}

let nextId = 0;

function lineId(kind) {
  nextId += 1;
  return `desktop-model-${kind}-${nextId}`;
}

// The two view keys the choices come from, as last sent.
let available = null;
let models = null;
// The choices drawn, by radio value.
let offered = new Map();
// Whether the child's session had a model when the view last said.
let chosen = false;

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

// One radio and its label: the choice's label, and its spec as code.
function option(doc, p, c, checked) {
  const li = el(doc, "li");
  const input = doc.createElement("input");
  input.type = "radio";
  input.name = "model-choice";
  input.value = c.value;
  input.id = `model-choice-${c.value}`;
  input.checked = checked;
  input.addEventListener("change", () => drawSave(p));
  const label = el(doc, "label", c.label);
  label.htmlFor = input.id;
  li.append(input, " ", label);
  if (c.spec !== null) li.append(" ", el(doc, "code", c.spec));
  return li;
}

// The choices, keeping the user's selection when it is still offered.
function drawChoices(doc) {
  const p = parts(doc);
  const before = p.options.querySelector('input[name="model-choice"]:checked');
  const keep = before ? before.value : null;
  const choices = modelChoices(available, models);
  offered = new Map(choices.map((c) => [c.value, c]));
  const kept = keep !== null && offered.has(keep);
  p.options.replaceChildren(
    ...choices.map((c) => option(doc, p, c, kept ? c.value === keep : c.preselect)),
  );
  drawSave(p);
}

// Where trellis looked, and the user's providers, from `list_models`.
function drawFound(doc) {
  const p = parts(doc);
  const probed = [];
  const providers = [];
  if (models === null || typeof models !== "object") {
    p.note.textContent = "Asking trellis which models there are.";
  } else if (str(models.error) !== null) {
    p.note.textContent = `trellis could not list models: ${models.error}`;
  } else {
    p.note.textContent = "";
    for (const t of Array.isArray(models.probed) ? models.probed : []) {
      const said = str(t && t.error);
      const li = el(doc, "li");
      li.append(
        el(doc, "span", `${str(t && t.runtime) ?? "?"} `),
        el(doc, "code", str(t && t.address) ?? "?"),
        el(doc, "span", said === null ? ": answered" : `: ${said}`),
      );
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
          variable === null
            ? ": a protocol trellis does not know"
            : `: its key is read from ${variable} in trellis's environment`,
        ),
      );
      providers.push(li);
    }
  }
  p.probed.replaceChildren(...probed);
  p.providers.replaceChildren(...providers);
}

// The session's model as the child last confirmed it. The pane stays open
// until one is chosen, and closes on the first choice.
function drawModel(doc, model) {
  const p = parts(doc);
  const spec = model && str(model.spec);
  const was = chosen;
  chosen = spec !== null && spec !== undefined;
  if (!chosen) {
    p.summary.textContent = "Model: none chosen yet";
    p.current.textContent =
      "No model is chosen for this window yet, so prompt input stays disabled until you choose one.";
    p.pane.open = true;
    return;
  }
  const saved = str(model.saved_to);
  p.summary.textContent = `Model: ${spec}`;
  p.current.textContent =
    `Prompts in this window run on ${spec}.` + (saved === null ? "" : ` Saved to ${saved}.`);
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
  if ("model" in view) drawModel(doc, view.model);
}

async function send(tauri, p, line) {
  try {
    await tauri.core.invoke("rpc_send", { line });
    return true;
  } catch (err) {
    p.note.textContent = String(err);
    return false;
  }
}

// Asks the child what there is to choose from.
export function askModels(doc, tauri) {
  const p = parts(doc);
  return send(tauri, p, { id: lineId("list"), type: "list_models" });
}

// Wires the pane once, and listens for `child-started`: each new child is a
// session with no model, and a repository whose configured spec may differ.
export async function wireModelPane(doc, tauri) {
  const p = parts(doc);
  await tauri.event.listen("child-started", () => askModels(doc, tauri));
  p.typed.addEventListener("input", () => {
    const typed = p.options.querySelector('input[name="model-choice"][value="typed"]');
    if (typed) typed.checked = true;
    drawSave(p);
  });
  p.refresh.addEventListener("click", () => askModels(doc, tauri));
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
    if (await send(tauri, p, line)) {
      p.note.textContent = "Sent; prompt input opens once trellis has taken it.";
    }
  });
  drawFound(doc);
  drawChoices(doc);
  drawModel(doc, null);
}
