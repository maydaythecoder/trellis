// The desktop's settings (#306; ADR 0048 sections 4 and 5, ADR 0050 section
// 2): what the model picker offers, the model catalogue, and the control that
// opens another repository.
//
// The picker is the model pane (#564, model.js), where a model is chosen
// after launch; this module says what it offers, in order:
//
// - "repository default": `set_model {configured: true}`, which names no
//   model. trellis pins whatever its configuration resolves to (the
//   repository's `[model] spec` over the user's), and counts that as the
//   configuration's choice, so a key goes only where a run with no `model`
//   would send one. The core's model guard has nothing to judge in it.
// - "your configured spec": the `user` spec from the core's own
//   `get_available_models` answer (the view's `available`), the one hosted
//   spec the guard forwards, sent byte for byte as the guard compares it.
// - the specs local runtimes on their default ports report (`list_models`),
//   which name a runtime and carry no key.
// - a spec the user types. The guard judges it like any other, and a
//   refusal is shown as the core wrote it.
//
// The repository's own spec is never offered as a `model`: the guard
// refuses it there, since rpc would count it as the user's choice (ADR 0024
// §3). The catalogue is shown, never sent: its names are weights `trellis
// model pull` fetches, run under a runtime the user starts. The page has no
// key field and no endpoint field (`page_has_no_key_or_endpoint_field`).
//
// Opening another repository is the core's `open_repository`, which refuses
// a path with no `.git` entry and refuses while a run is in flight; its
// error text is shown as it came.
//
// Every spec, name, licence and error is trellis- or configuration-supplied
// text, so it reaches the DOM through textContent only (ADR 0007, S10).

function el(doc, tag, text) {
  const e = doc.createElement(tag);
  if (text !== undefined) e.textContent = text;
  return e;
}

function str(v) {
  return typeof v === "string" ? v : null;
}

function isCount(v) {
  return typeof v === "number" && Number.isFinite(v);
}

let nextId = 0;

function lineId() {
  nextId += 1;
  return `desktop-settings-set-${nextId}`;
}

// What the picker calls the configured spec trellis pins when asked to.
export const REPOSITORY_DEFAULT = "repository default";

// The picker's choices, in order. Each is `{value, label, spec, preselect,
// saves, line}`: `spec` the text shown beside the label (null for none),
// `saves` whether "save to my user config" applies, and `line(save, typed)`
// the `set_model` it sends, or null when there is nothing to send. From the
// view's `available` (the core's own models answer) and `models` (the last
// `list_models` answer); either may be null while unanswered.
//
// Preselected, as #564 asks: the configured spec, which is what "repository
// default" pins; with nothing configured, the user's own spec; with neither,
// the typed spec.
export function modelChoices(available, models) {
  const configured = available && typeof available === "object" ? available.configured : null;
  const user = configured && typeof configured === "object" ? str(configured.user) : null;
  const listed = models && typeof models === "object" && str(models.error) === null ? models : null;
  const resolved = listed ? str(listed.configured) : null;
  const choices = [];
  choices.push({
    value: "default",
    label: `${REPOSITORY_DEFAULT}:`,
    spec: resolved ?? (listed ? "nothing configured" : "not reported yet"),
    preselect: resolved !== null,
    saves: false,
    line: () => ({ id: lineId(), type: "set_model", configured: true }),
  });
  if (user !== null) {
    choices.push({
      value: "user",
      label: "your configured spec:",
      spec: user,
      preselect: resolved === null,
      saves: false,
      line: () => ({ id: lineId(), type: "set_model", model: user }),
    });
  }
  const local = listed && Array.isArray(listed.local) ? listed.local : [];
  local.forEach((spec, i) => {
    if (str(spec) === null || spec === user) return;
    choices.push({
      value: `local-${i}`,
      label: "running on this machine:",
      spec,
      preselect: false,
      saves: true,
      line: (save) => withSave({ id: lineId(), type: "set_model", model: spec }, save),
    });
  });
  choices.push({
    value: "typed",
    label: "a spec I type below",
    spec: null,
    preselect: choices.every((c) => !c.preselect),
    saves: true,
    line: (save, typed) => {
      const spec = str(typed) === null ? "" : typed.trim();
      if (spec === "") return null;
      return withSave({ id: lineId(), type: "set_model", model: spec }, save);
    },
  });
  return choices;
}

function withSave(line, save) {
  if (save) line.save = true;
  return line;
}

// The catalogue from the view's `available`: each entry's name, licence and
// download size as trellis rounded it, and which one is the default. Drawn
// under a heading that says what these are: weights `trellis model pull`
// fetches, not models this window can run by name.
export function drawCatalog(doc, available) {
  const pane = doc.getElementById("model-catalog");
  const list = available && typeof available === "object" && Array.isArray(available.catalog)
    ? available.catalog
    : [];
  const rows = list.flatMap((e) => {
    const name = str(e && e.name);
    if (name === null) return [];
    const li = el(doc, "li");
    li.append(el(doc, "code", name));
    if (e.default === true) li.append(el(doc, "span", " (default)"));
    const gib = isCount(e.download_gib) ? `${e.download_gib} GiB download` : "size not reported";
    li.append(el(doc, "span", ` · ${str(e.license) ?? "licence not reported"} · ${gib}`));
    return [li];
  });
  if (rows.length === 0) {
    const li = el(doc, "li", "No catalogue reported yet.");
    li.className = "empty";
    rows.push(li);
  }
  pane.replaceChildren(...rows);
}

// Draws the settings keys of one `view` event; the picker's radios are
// model.js's.
export function drawSettings(doc, view) {
  if (view === null || typeof view !== "object") return;
  if ("available" in view) drawCatalog(doc, view.available);
}

// The open-repository control: a path, sent to the core's
// `open_repository`. On success the core starts a new child, whose
// `child-started` redraws the status bar and the picker; on refusal its
// text is shown as it came.
export function wireRepositorySwitch(doc, tauri) {
  const form = doc.getElementById("repo-form");
  const path = doc.getElementById("repo-path");
  const note = doc.getElementById("repo-note");
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const p = path.value.trim();
    if (p === "") {
      note.textContent = "Type the path of a repository first.";
      return;
    }
    note.textContent = `Opening ${p}.`;
    try {
      await tauri.core.invoke("open_repository", { path: p });
      note.textContent = "";
      path.value = "";
    } catch (err) {
      note.textContent = String(err);
    }
  });
  drawCatalog(doc, null);
}
