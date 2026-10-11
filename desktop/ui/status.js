// The status bar (#306; ADR 0048 sections 2, 5 and 8).
//
// It shows what the core already knows, and nothing it does not:
//
// - the trellis binary's path and version, and the repository root, from
//   `child-started` (`version` is null when `trellis --version` could not
//   be read, and is said so, never guessed);
// - the session's model and where it came from (`repository`, `user` or
//   `session`, the view's `model.source`, null when the core cannot tell);
// - whether a run is in flight, the view's `running`;
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

// Draws the status bar's keys of one `view` event.
export function drawStatus(doc, view) {
  if (view === null || typeof view !== "object") return;
  if ("model" in view) show(doc, "status-model", modelText(view.model));
  if ("running" in view) {
    show(doc, "status-run", view.running === true ? "a run is in flight" : "no run in flight");
  }
}

// Listens for the core's child events. Resolves once every listener is
// registered, so the caller asks for nothing before one is.
export async function wireStatus(doc, tauri) {
  await tauri.event.listen("child-started", (event) => {
    const c = event.payload && typeof event.payload === "object" ? event.payload : {};
    const version = str(c.version) ?? "version not reported";
    show(doc, "status-trellis", `${str(c.trellis) ?? "path not reported"} · ${version}`);
    show(doc, "status-root", str(c.root) ?? "not reported");
    show(doc, "status-child", "running");
  });
  await tauri.event.listen("rpc-exited", (event) => {
    show(doc, "status-child", exitText(event.payload));
  });
  await tauri.event.listen("no-child", (event) => {
    show(doc, "status-child", `not started: ${String(event.payload)}`);
  });
}
