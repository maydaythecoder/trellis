// The model pane (#564; ADR 0048 section 5 as amended, ADR 0050 section 2).
//
// The desktop starts trellis with the user's environment unchanged, keys
// included (ADR 0048 section 3, amended), on the owner's condition that it
// asks which model to use after launch and keeps prompt input disabled until
// one is chosen. This pane is where it asks. It lists what the child's
// `list_models` answered: the configured spec, preselected; the specs the
// local runtimes on their default ports report, as `first_run` lists them;
// and a spec the user types. The user's own providers are named with the
// variable a key would be read from, never a value, since no key reaches
// trellis-desktop's page.
//
// Choosing sends `set_model` through the core: `{configured: true}` for the
// configured spec, which names no model, or `{model, save?}` for any other,
// which the core's model guard holds to ADR 0048 section 4 (a hosted spec
// only when it is the user's own). With "save" ticked, trellis writes it to
// the user's own config, never the repository's. Whether a model has been
// chosen is the view's `model`, the core's record of the child's answer; the
// composer reads the same key, and the core refuses a prompt before it.
//
// Every spec, address, error and provider name is trellis- or
// configuration-supplied text, so it reaches the DOM through textContent
// only (ADR 0007, S10).

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

// What each radio stands for, by its value, from the last list drawn.
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

// The selected radio's value, or null with none selected.
function selected(p) {
  const r = p.options.querySelector('input[name="model-choice"]:checked');
  return r ? r.value : null;
}

// Saving applies to a spec the user names; the configured one is already
// in a config file, and trellis refuses to save it again.
function drawSave(p) {
  const s = selected(p);
  p.save.disabled = s === null || s === "configured";
  if (p.save.disabled) p.save.checked = false;
}

// One radio and its label: `text`, and `spec` as code beside it.
function option(doc, p, value, text, spec, checked) {
  const li = el(doc, "li");
  const input = doc.createElement("input");
  input.type = "radio";
  input.name = "model-choice";
  input.value = value;
  input.id = `model-choice-${value}`;
  input.checked = checked;
  input.addEventListener("change", () => drawSave(p));
  const label = el(doc, "label", text);
  label.htmlFor = input.id;
  li.append(input, " ", label);
  if (spec !== null) li.append(" ", el(doc, "code", spec));
  return li;
}

// The list `list_models` answered, or why it could not be read. The typed
// spec is offered whatever the list says, so a runtime on another port, or a
// hosted spec of the user's own, can still be chosen.
function drawModels(doc, models) {
  const p = parts(doc);
  offered = new Map();
  const rows = [];
  const probed = [];
  const providers = [];
  if (models === null || typeof models !== "object") {
    p.note.textContent = "Asking trellis which models there are.";
  } else if (str(models.error) !== null) {
    p.note.textContent = `trellis could not list models: ${models.error}`;
  } else {
    p.note.textContent = "";
    const configured = str(models.configured);
    if (configured !== null) {
      offered.set("configured", { configured });
      rows.push(option(doc, p, "configured", "The configured spec:", configured, true));
    }
    const local = Array.isArray(models.local) ? models.local : [];
    local.forEach((spec, i) => {
      if (str(spec) === null) return;
      offered.set(`local-${i}`, { model: spec });
      rows.push(option(doc, p, `local-${i}`, "Running on this machine:", spec, false));
    });
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
  offered.set("typed", null);
  rows.push(option(doc, p, "typed", "A spec I type below", null, rows.length === 0));
  p.options.replaceChildren(...rows);
  p.probed.replaceChildren(...probed);
  p.providers.replaceChildren(...providers);
  drawSave(p);
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
  const how = model.configured === true ? " (the configured spec)" : "";
  const saved = str(model.saved_to);
  p.summary.textContent = `Model: ${spec}`;
  p.current.textContent =
    `Prompts in this window run on ${spec}${how}.` +
    (saved === null ? "" : ` Saved to ${saved}.`);
  if (!was) p.pane.open = false;
}

// Draws the model pane's keys of one `view` event; any other key is
// another module's.
export function drawModelPane(doc, view) {
  if (view === null || typeof view !== "object") return;
  if ("models" in view) drawModels(doc, view.models);
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

// The `set_model` line for the selected radio, or a note saying what is
// missing.
function choice(p) {
  const s = selected(p);
  if (s === null) return { note: "Choose one of the models listed, or type a spec." };
  if (s === "configured") {
    return { line: { id: lineId("set"), type: "set_model", configured: true } };
  }
  const spec = s === "typed" ? p.typed.value.trim() : str(offered.get(s) && offered.get(s).model);
  if (spec === null || spec === "") return { note: "Type a model spec first." };
  const line = { id: lineId("set"), type: "set_model", model: spec };
  if (p.save.checked) line.save = true;
  return { line };
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
    const c = choice(p);
    if (c.note !== undefined) {
      p.note.textContent = c.note;
      return;
    }
    if (await send(tauri, p, c.line)) {
      p.note.textContent = "Sent; prompt input opens once trellis has taken it.";
    }
  });
  drawModels(doc, null);
  drawModel(doc, null);
}
