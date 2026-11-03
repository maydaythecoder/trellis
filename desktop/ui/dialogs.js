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
// A question about a write the Code level shows (#831) also offers Review
// in Code: that close answers nothing and is not a no. The question is
// deferred, put to the person beside the file's diff, where Approve and
// Reject answer it through answerApproval, this module's one way to send an
// answer besides the dialog's close; the modal leaves it alone while it
// waits there.
//
// The banner (`role="alert"`) shows the view's `alert`, what last failed as
// the core worded it, and any error this page raised and did not catch.
// The "trellis messages" pane lists the view's `stderr`: trellis's human
// text, shown as text and never read as data (ADR 0048 section 2, amended
// by ADR 0052).
//
// Every string here is trellis-, model- or repository-supplied, so it
// reaches the DOM through textContent only (ADR 0007, S10).

import { drawFeed, el } from "./dom.js";
import { str } from "./fmt.js";

// The page errors kept for the banner: the newest few, since one fault
// in a loop would otherwise fill it.
const PAGE_ERRORS_KEPT = 5;
const pageErrors = [];
// The document the banner is drawn in, once wireDialogs has run; an error
// kept before then is drawn when it does.
let bannerDoc = null;

// Keeps a page error for the banner and draws it. The hook rpc.js's
// start() reports a module that failed to start through (#816), and the
// window's error and unhandledrejection events through too.
export function pageError(what) {
  pageErrors.push(String(what));
  while (pageErrors.length > PAGE_ERRORS_KEPT) pageErrors.shift();
  if (bannerDoc !== null) drawBanner(bannerDoc);
}
// The view's alert as last sent; `null` while nothing has failed.
let viewAlert = null;

// The question the dialog shows, by id, and the ids already answered from
// this page: a view sent before trellis's reply still lists them, and a
// question is never put to the user twice. Both are one child's: rpc
// numbers approvals per process from `approval-1`, so on `child-started`
// they are forgotten, or the next child's `approval-1` would be taken for
// one already answered and its run would wait for ever.
let shown = null;
const answered = new Set();
// Questions the person took to the Code level unanswered (#831), and the
// Code level's two hooks: the file a question asks to write, and opening it.
const deferred = new Set();
let review = { pathOf: () => null, open: () => {} };
// What sends an answer, once wireDialogs has run.
let sendAnswer = null;
// A dialog's `close` event comes after `close()` returns, as a task of its
// own, so each one is matched to the question it was opened for: one entry
// per `showModal()`, in order, each shifted off by its own close. A
// question the page closed itself, because it went away (its run ended, or
// its child was replaced), is marked `withdrawn`: that close is no answer.
// The mark is on the entry, not the id, since a new child reuses the ids.
const opened = [];

function drawBanner(doc) {
  const banner = doc.getElementById("alert");
  const lines = [];
  if (viewAlert !== null) lines.push(viewAlert);
  for (const e of pageErrors) lines.push(`page error: ${e}`);
  doc.getElementById("alert-text").replaceChildren(...lines.map((l) => el(doc, "span", l)));
  banner.hidden = lines.length === 0;
}

// The messages pane is a feed, as the actions log is (#827), drawn by
// dom.js's drawFeed: each line sent once, appended, and the pane follows
// its end only while the reader is there (#643), so a reader who scrolled
// back keeps their place. Answers whether lines are missing.
function drawStderr(doc, feed) {
  return drawFeed(doc, doc.getElementById("messages-log"), feed, {
    draw: (row) => el(doc, "div", str(row.text) ?? ""),
    empty: "trellis has written nothing to stderr.",
  }).missing;
}

// Closes the dialog on the question it shows, as no answer: every dialog
// still to send its close is marked withdrawn.
function withdraw(dialog) {
  for (const e of opened) e.withdrawn = true;
  shown = null;
  if (dialog.open) dialog.close();
}

// A new child: the questions the last one asked are nobody's now, and the
// ids it used will be used again. A close already queued for its dialog
// stays matched to its own entry, which is withdrawn, so it is never read as
// the answer to the new child's question of the same id.
function forgetChild(doc) {
  answered.clear();
  deferred.clear();
  withdraw(doc.getElementById("approval"));
}

function drawApprovals(doc, list) {
  const dialog = doc.getElementById("approval");
  const asked = Array.isArray(list) ? list : [];
  // A deferred question trellis no longer holds is nobody's to answer.
  for (const id of deferred) if (!asked.some((a) => a && a.id === id)) deferred.delete(id);
  const open = asked.filter((a) => a && str(a.id) !== null && !answered.has(a.id) && !deferred.has(a.id));
  if (shown !== null && !open.some((a) => a.id === shown)) {
    // Its run ended or it was answered elsewhere: nobody waits on it now.
    withdraw(dialog);
  }
  if (shown !== null || open.length === 0) return;
  const q = open[0];
  shown = q.id;
  doc.getElementById("approval-question").textContent =
    str(q.question) ?? "trellis did not say what it asks";
  // The prompt or follow-up whose run asks (#954), as the reducer named
  // it from request_id: the person's own words, written as text.
  doc.getElementById("approval-for").textContent = `for ${str(q.asked_for) ?? "a request trellis did not name"}`;
  doc.getElementById("approval-detail").textContent = str(q.detail) ?? "";
  const path = review.pathOf(q.id);
  doc.getElementById("approval-review").hidden = path === null;
  dialog.returnValue = "";
  opened.push({ id: q.id, withdrawn: false, path });
  dialog.showModal();
  // Reject has the focus, so a stray Enter is a no.
  doc.getElementById("approval-reject").focus();
}

// The Code level's hooks (#831): `pathOf(id)` answers the file a question
// asks to write, or null, and `open(path)` shows that file.
export function onReview(hooks) {
  review = hooks;
}

// Answers question `id` from the Code level (#831): once, and never again
// from the dialog, which is closed without an answer if it shows it.
export function answerApproval(doc, id, yes) {
  if (answered.has(id) || sendAnswer === null) return;
  answered.add(id);
  deferred.delete(id);
  if (shown === id) withdraw(doc.getElementById("approval"));
  sendAnswer(id, yes);
}

// Draws the dialog's, banner's and pane's keys of one `view` event; any
// other key is another module's. `onMissing` is called when the messages
// pane found lines missing, as drawView calls it for the transcript.
export function drawDialogs(doc, view, onMissing) {
  if (view === null || typeof view !== "object") return;
  if ("alert" in view) {
    viewAlert = str(view.alert);
    drawBanner(doc);
  }
  if ("stderr" in view && drawStderr(doc, view.stderr)) onMissing();
  if ("approvals" in view) drawApprovals(doc, view.approvals);
}

// Wires the dialog, the banner's Dismiss and the page's own error events,
// once, and listens for `child-started`. Resolves once the listener is
// registered, so the caller can ask for nothing before it is.
export async function wireDialogs(doc, win, tauri) {
  const dialog = doc.getElementById("approval");
  sendAnswer = (id, yes) =>
    tauri.core
      .invoke("rpc_send", { line: { id, type: "approve", yes } })
      .catch((err) => {
        viewAlert = String(err);
        drawBanner(doc);
      });
  dialog.addEventListener("close", () => {
    const entry = opened.shift();
    if (entry === undefined || entry.withdrawn) return;
    const id = entry.id;
    if (shown === id) shown = null;
    // Review in Code sets this value, offered only for a question the Code
    // level shows: it answers nothing, and the file's Approve or Reject
    // will (#831).
    if (dialog.returnValue === "review" && entry.path !== null) {
      deferred.add(id);
      review.open(entry.path);
      return;
    }
    answered.add(id);
    // Only the Approve button sets this value; Escape, Reject and a close
    // by any other means leave something else, which is a no (ADR 0018).
    const yes = dialog.returnValue === "approve";
    sendAnswer(id, yes);
  });
  doc.getElementById("alert-dismiss").addEventListener("click", () => {
    pageErrors.length = 0;
    viewAlert = null;
    drawBanner(doc);
  });
  bannerDoc = doc;
  win.addEventListener("error", (e) => pageError(String(e.message || e.error || "an uncaught error")));
  win.addEventListener("unhandledrejection", (e) => pageError(`unhandled rejection: ${String(e.reason)}`));
  if (pageErrors.length > 0) drawBanner(doc);
  drawStderr(doc, []);
  await tauri.event.listen("child-started", () => forgetChild(doc));
}
