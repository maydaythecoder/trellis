// The status bar (#306; ADR 0048 sections 2, 5 and 8).
//
// It shows what the core already knows, and nothing it does not:
//
// - the trellis binary's path and version, and the repository root, from
//   `child-started` (`version` is null when `trellis --version` could not
//   be read, and is said so, never guessed); each path as its last
//   component, with the whole path as the cell's title, so the bar stays
//   one line (#652); the root also names the window in `document.title`
//   (#645);
// - the session's model and where it came from (`repository`, `user` or
//   `session`, the view's `model.source`, null when the core cannot tell);
// - whether a run is in flight, the view's `running`;
// - what the run is on, the view's `phase` (#615): the model being called
//   for a turn, or the tool or checks the last turn ran, each with the
//   turn number trellis sent; null, said as none, before the first call and
//   once the run has ended. No elapsed time: that would be this page's
//   clock, not a trellis figure (ADR 0048 section 8);
// - the child's state: running since `child-started`, exited with the
//   status `rpc-exited` carried, or not started for the reason `no-child`
//   gave.
//
// Runtime reachability is not shown here: rpc's `list_models` probes the
// default local runtimes, and the model pane (model.js) lists what each
// address answered; the page itself never talks to a runtime. Every value
// is trellis-supplied text, written through textContent only (ADR 0007,
// S10).

function str(v) {
  return typeof v === "string" ? v : null;
}

function show(doc, id, text) {
  doc.getElementById(id).textContent = text;
}

// The last component of `path`, split on either separator, since trellis
// may run on Windows; a path that is all separators is itself.
export function baseName(path) {
  const parts = path.split(/[\\/]/).filter((p) => p !== "");
  return parts.length === 0 ? path : parts[parts.length - 1];
}

// Shows `path` in the cell `id` by its last component, with the whole path
// as the cell's title; or `missing`, with no title, when there is none.
function showPath(doc, id, path, missing, after = "") {
  const e = doc.getElementById(id);
  if (path === null) {
    e.textContent = missing + after;
    e.removeAttribute("title");
    return;
  }
  e.textContent = baseName(path) + after;
  e.setAttribute("title", path);
}

// The session's model as the status bar words it.
function modelText(m) {
  const spec = m && typeof m === "object" ? str(m.spec) : null;
  if (spec === null) return "none chosen yet";
  const source = str(m.source);
  return source === null ? `${spec} (source not known)` : `${spec} (from ${source})`;
}

// What `rpc-exited` carried, as text: the code, or why there is none.
export function exitText(status) {
  if (status === null || typeof status !== "object") return "exited";
  if (typeof status.code === "number") {
    return status.success === true ? "exited with status 0" : `exited with status ${status.code}`;
  }
  const why = str(status.error);
  return why === null ? "exited with no status (ended by a signal)" : `exited: ${why}`;
}

// What the run is on, as the status bar words the view's `phase`: the
// phase trellis's last event said, and its turn number, nothing else.
export function phaseText(p) {
  if (p === null || typeof p !== "object") return "none";
  const turn = typeof p.turn === "number" ? `, turn ${p.turn}` : "";
  switch (str(p.phase)) {
    case "model":
      return `calling the model${turn}`;
    case "tool":
      return `ran ${str(p.tool) ?? "a tool"}${turn}`;
    case "checks":
      return `ran checks${turn}`;
    default:
      return "none";
  }
}

// Draws the status bar's keys of one `view` event.
export function drawStatus(doc, view) {
  if (view === null || typeof view !== "object") return;
  if ("model" in view) show(doc, "status-model", modelText(view.model));
  if ("running" in view) {
    show(doc, "status-run", view.running === true ? "a run is in flight" : "no run in flight");
  }
  if ("phase" in view) show(doc, "status-phase", phaseText(view.phase));
}

// Listens for the core's child events. Resolves once every listener is
// registered, so the caller asks for nothing before one is.
export async function wireStatus(doc, tauri) {
  await tauri.event.listen("child-started", (event) => {
    const c = event.payload && typeof event.payload === "object" ? event.payload : {};
    const version = str(c.version) ?? "version not reported";
    showPath(doc, "status-trellis", str(c.trellis), "path not reported", ` · ${version}`);
    showPath(doc, "status-root", str(c.root), "not reported");
    show(doc, "status-child", "running");
    // The window title names the open repository, so someone with several
    // windows can tell them apart from the task bar (#645). The root is a
    // path trellis reported, written as text: `title` is plain text.
    doc.title = str(c.root) === null ? "trellis" : `trellis · ${c.root}`;
  });
  await tauri.event.listen("rpc-exited", (event) => {
    show(doc, "status-child", exitText(event.payload));
  });
  await tauri.event.listen("no-child", (event) => {
    show(doc, "status-child", `not started: ${String(event.payload)}`);
  });
}
