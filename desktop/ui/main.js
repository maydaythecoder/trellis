// The TRELLIS desktop page's entry module (ADR 0048 section 5).
//
// It switches the rail's sections and hands what the desktop's Rust core
// emits to render.js, overview.js, composer.js, model.js, settings.js,
// status.js and dialogs.js, and computes
// nothing: the run panes the core's reducer sends as `view` events, which
// child started or why none did, and the overview object trellis printed.
// There is no bundler, so it reaches Tauri through the global
// window.__TAURI__ rather than an import of @tauri-apps/api.
//
// Every value is trellis- or model-supplied text, so it reaches the DOM only
// through textContent (ADR 0007, S10): text that carries markup is shown as
// characters, never parsed.

import { drawView, wireLog } from "./render.js";
import { runEnded, send, start } from "./rpc.js";
import { drawOpenRefusal, drawOverview, drawOverviewChild } from "./overview.js";
import { askStarters, composerStopped, drawComposer, wireComposer } from "./composer.js";
import { drawDialogs, wireDialogs } from "./dialogs.js";
import { askModels, drawModelPane, modelPaneStopped, wireModelPane } from "./model.js";
import { drawSettings, wireRepositorySwitch } from "./settings.js";
import { drawStatus, exitText, wireStatus } from "./status.js";

function show(id, text) {
  document.getElementById(id).textContent = text;
}

function json(value) {
  return JSON.stringify(value);
}

// The rail: each button names its section with aria-controls, and showing
// one hides the rest through the hidden attribute, never a style. A press
// moves focus to the shown section's heading, the one its aria-labelledby
// names, so Tab goes on from inside the section rather than from the rail
// (#646).
function wireRail() {
  const buttons = [...document.querySelectorAll("nav.rail button[aria-controls]")];
  const select = (chosen) => {
    for (const b of buttons) {
      const section = document.getElementById(b.getAttribute("aria-controls"));
      const on = b === chosen;
      if (section) section.hidden = !on;
      if (on) b.setAttribute("aria-current", "page");
      else b.removeAttribute("aria-current");
    }
  };
  for (const b of buttons) {
    b.addEventListener("click", () => {
      select(b);
      const section = document.getElementById(b.getAttribute("aria-controls"));
      const heading = section && document.getElementById(section.getAttribute("aria-labelledby"));
      if (heading) heading.focus();
    });
  }
}

// How many `child-started` events this page has heard, so a refusal that
// crossed a newer child's start is not taken for that child's state.
let childrenStarted = 0;

// How the core's `rpc_send` begins a refusal for want of a child
// (`commands.rs`); the rest is its reason.
const NO_CHILD = "rpc_send: no trellis rpc child is running: ";

// Asks the child for its state. The core answers a forwarded get_state by
// sending every pane in the next view event, so a page that loaded late or
// reloaded gets the run back. A refusal is the core's reason there is no
// child: still starting, no repository found, or the child exited. A page
// that loaded late heard no `no-child` or `rpc-exited` for that, so the
// refusal is passed on as the not-running state (#638). Says whether the
// line was forwarded, so a child is known to be running.
function askState(tauri) {
  const heard = childrenStarted;
  return send(tauri, { id: "desktop-state", type: "get_state" }, (said) => {
    show("status", said);
    if (said.startsWith(NO_CHILD) && childrenStarted === heard) {
      const why = said.slice(NO_CHILD.length);
      composerStopped(document, why);
      modelPaneStopped(document, why);
      drawOverviewChild(document, false);
    }
  });
}

// The overview's actions: "Open" on a recent repository. Success needs no
// drawing here: the new child's `child-started` reads the overview again.
// A refusal is drawn in the Recent repositories pane, where the click was.
function overviewActions(tauri) {
  return {
    open: (path) => {
      tauri.core.invoke("open_repository", { path }).catch((err) => drawOpenRefusal(document, String(err)));
    },
  };
}

// Reads the overview for the open repository and draws it. Each read runs
// trellis once, and `gh` within it, so it is asked for only when what it
// shows has changed: a child started, a run ended, or a Codebase answer
// saved the index (#654).
async function readOverview(tauri) {
  try {
    drawOverview(document, await tauri.core.invoke("get_overview"), overviewActions(tauri));
  } catch (err) {
    show("ov-root", `The overview could not be read: ${err}`);
  }
}

// The last run outcome a view carried, as JSON, so only a new one rereads
// the overview.
let lastOutcome = null;

// Asks the child for the session's prompt and completion tokens per model
// (#313). The answer comes back as the view's `session_tokens`, which the
// core keeps from trellis's own answer: the page adds up no turn itself.
// A refusal (no child yet) is the status line's to say, not this pane's.
function askSessionTokens(tauri) {
  send(tauri, { id: "desktop-session-stats", type: "get_session_stats" }, (why) => show("status", why));
}

async function main() {
  wireRail();
  wireLog(document);
  const tauri = window.__TAURI__;

  // Every listener is registered, and awaited, before anything is asked
  // for, so no answer arrives before something listens for it.
  await wireDialogs(document, window, tauri);
  await wireComposer(document, tauri);
  await wireModelPane(document, tauri);
  await wireStatus(document, tauri);
  wireRepositorySwitch(document, tauri);
  await tauri.event.listen("view", (event) => {
    drawView(document, event.payload);
    drawComposer(document, event.payload);
    drawModelPane(document, event.payload);
    drawSettings(document, event.payload);
    drawStatus(document, event.payload);
    drawDialogs(document, event.payload);
    // A run's end is a new newest trail, so the last run pane is stale. A
    // get_state reply resends the same outcome, which changes nothing.
    const o = event.payload && event.payload.outcome;
    if (o !== null && typeof o === "object" && json(o) !== lastOutcome) {
      lastOutcome = json(o);
      readOverview(tauri);
    }
  });
  await tauri.event.listen("child-started", (event) => {
    childrenStarted += 1;
    drawOverviewChild(document, true);
    const c = event.payload;
    show("status", `${c.version ?? "trellis (version unknown)"} rpc on ${c.root}`);
    askState(tauri);
    readOverview(tauri);
    askSessionTokens(tauri);
  });
  // Each run_end frame is a run the session's totals now count. Only its
  // type is read: the frame is the child's, and the figures come from
  // trellis's own answer.
  await tauri.event.listen("rpc-frame", (event) => {
    if (runEnded(event.payload)) askSessionTokens(tauri);
  });
  // A Codebase answer that saved .trellis/index.bin (#654) changes what the
  // overview's codebase pane reads, which is only that cache, so the
  // overview is read again; otherwise it would keep saying "not indexed"
  // beside the graph until the next run ended.
  await tauri.event.listen("codebase", (event) => {
    const c = event.payload;
    if (c !== null && typeof c === "object" && c.index_saved === true) readOverview(tauri);
  });
  await tauri.event.listen("no-child", (event) => {
    show("status", String(event.payload));
    drawOverviewChild(document, false);
  });
  await tauri.event.listen("rpc-exited", (event) => {
    show("status", `trellis rpc ${exitText(event.payload)}`);
    drawOverviewChild(document, false);
  });
  // With no root the core emits the overview once at startup.
  await tauri.event.listen("overview", (event) => {
    drawOverview(document, event.payload, overviewActions(tauri));
  });
  await tauri.event.listen("overview-error", (event) => {
    show("ov-root", `The overview could not be read: ${event.payload}`);
  });

  // A page loaded after its child started (a reload) sees no
  // child-started, so it reads the overview itself; before then the core
  // has no root yet, and the overview it read would say none is open.
  // The model list and the starters are asked for too: the core's view
  // keeps the model the child took and the starters a get_commands answer
  // listed, which the get_state resend draws, but a child that started
  // before this page listened was never asked for either, and its
  // child-started, which asks, came before anything heard it.
  if (await askState(tauri)) {
    readOverview(tauri);
    askModels(document, tauri);
    askStarters(document, tauri);
  }
}

start("trellis desktop", main);
