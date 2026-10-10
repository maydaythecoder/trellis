// The approval dialog, the alert banner and the trellis messages pane
// (#302; ADR 0018, ADR 0048 sections 2 and 5).
//
// In approve mode trellis asks before a gated call with an
// `approval_request` and waits for as long as nobody answers. The view's
// `approvals` are the questions the child asked and nobody has answered;
// the dialog shows the first of them with `showModal()`. Only Approve sends
// `approve` with `yes` true, as only `"yes": true` approves in rpc.rs;
// Reject, Escape and any other way the dialog closes send `yes` false. The
// core forwards an `approve` only for a question open in its own view
// (`rpc_send`), so this page cannot answer one that was never asked.
//
// The banner (`role="alert"`) shows the view's `alert`, what last failed as
// the core worded it, and any error this page raised and did not catch.
// The "trellis messages" pane lists the view's `stderr`: trellis's human
// text, shown as text and never read as data (ADR 0048 section 2, amended
// by ADR 0052).
//
// Every string here is trellis-, model- or repository-supplied, so it
// reaches the DOM through textContent only (ADR 0007, S10).

function el(doc, tag, text) {
  const e = doc.createElement(tag);
  if (text !== undefined) e.textContent = text;
  return e;
}

function str(v) {
  return typeof v === "string" ? v : null;
}

// The page errors kept for the banner: the newest few, since one fault
// in a loop would otherwise fill it.
const PAGE_ERRORS_KEPT = 5;
const pageErrors = [];
// The view's alert as last sent; `null` while nothing has failed.
let viewAlert = null;

// The question the dialog shows, by id, and the ids already answered from
// this page: a view sent before trellis's reply still lists them, and a
// question is never put to the user twice.
let shown = null;
const answered = new Set();
// A dialog's `close` event comes after `close()` returns, as a task of its
// own, so each one is matched to the question it was opened for: one id per
// `showModal()`, in order. A question the page closed itself, because it
// went away (its run ended), is in `withdrawn`: that close is no answer.
const opened = [];
const withdrawn = new Set();

function drawBanner(doc) {
  const banner = doc.getElementById("alert");
  const lines = [];
  if (viewAlert !== null) lines.push(viewAlert);
  for (const e of pageErrors) lines.push(`page error: ${e}`);
  doc.getElementById("alert-text").replaceChildren(...lines.map((l) => el(doc, "span", l)));
  banner.hidden = lines.length === 0;
}

function drawStderr(doc, lines) {
  const log = doc.getElementById("messages-log");
  const rows = (Array.isArray(lines) ? lines : []).map((l) => el(doc, "div", String(l)));
  if (rows.length === 0) {
    const e = el(doc, "div", "trellis has written nothing to stderr.");
    e.className = "empty";
    log.replaceChildren(e);
  } else {
    log.replaceChildren(...rows);
  }
  log.scrollTop = log.scrollHeight;
}

function drawApprovals(doc, list) {
  const dialog = doc.getElementById("approval");
  const open = (Array.isArray(list) ? list : []).filter(
    (a) => a && str(a.id) !== null && !answered.has(a.id),
  );
  if (shown !== null && !open.some((a) => a.id === shown)) {
    // Its run ended or it was answered elsewhere: nobody waits on it now.
    withdrawn.add(shown);
    shown = null;
    if (dialog.open) dialog.close();
  }
  if (shown !== null || open.length === 0) return;
  const q = open[0];
  shown = q.id;
  doc.getElementById("approval-question").textContent =
    str(q.question) ?? "trellis did not say what it asks";
  doc.getElementById("approval-detail").textContent = str(q.detail) ?? "";
  dialog.returnValue = "";
  opened.push(q.id);
  dialog.showModal();
  // Reject has the focus, so a stray Enter is a no.
  doc.getElementById("approval-reject").focus();
}

// Draws the dialog's, banner's and pane's keys of one `view` event; any
// other key is another module's.
export function drawDialogs(doc, view) {
  if (view === null || typeof view !== "object") return;
  if ("alert" in view) {
    viewAlert = str(view.alert);
    drawBanner(doc);
  }
  if ("stderr" in view) drawStderr(doc, view.stderr);
  if ("approvals" in view) drawApprovals(doc, view.approvals);
}

// Wires the dialog, the banner's Dismiss and the page's own error events,
// once, before anything is asked for.
export function wireDialogs(doc, win, tauri) {
  const dialog = doc.getElementById("approval");
  dialog.addEventListener("close", () => {
    const id = opened.shift();
    if (id === undefined || withdrawn.delete(id)) return;
    if (shown === id) shown = null;
    answered.add(id);
    // Only the Approve button sets this value; Escape, Reject and a close
    // by any other means leave something else, which is a no (ADR 0018).
    const yes = dialog.returnValue === "approve";
    tauri.core
      .invoke("rpc_send", { line: { id, type: "approve", yes } })
      .catch((err) => {
        viewAlert = String(err);
        drawBanner(doc);
      });
  });
  doc.getElementById("alert-dismiss").addEventListener("click", () => {
    pageErrors.length = 0;
    viewAlert = null;
    drawBanner(doc);
  });
  const keep = (what) => {
    pageErrors.push(what);
    while (pageErrors.length > PAGE_ERRORS_KEPT) pageErrors.shift();
    drawBanner(doc);
  };
  win.addEventListener("error", (e) => keep(String(e.message || e.error || "an uncaught error")));
  win.addEventListener("unhandledrejection", (e) => keep(`unhandled rejection: ${String(e.reason)}`));
  drawStderr(doc, []);
}
