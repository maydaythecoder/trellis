// The TRELLIS desktop page's entry module (ADR 0048 section 5).
//
// It switches the rail's sections and hands what the desktop's Rust core
// emits to render.js, and computes nothing: the run panes the core's
// reducer sends as `view` events, which child started or why none did.
// There is no bundler, so it reaches Tauri through the global
// window.__TAURI__ rather than an import of @tauri-apps/api.
//
// Every value is trellis- or model-supplied text, so it reaches the DOM only
// through textContent (ADR 0007, S10): text that carries markup is shown as
// characters, never parsed.

import { drawView } from "./render.js";

function show(id, text) {
  document.getElementById(id).textContent = text;
}

function json(value) {
  return JSON.stringify(value);
}

// The rail: each button names its section with aria-controls, and showing
// one hides the rest through the hidden attribute, never a style.
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
  for (const b of buttons) b.addEventListener("click", () => select(b));
}

// Asks the child for its state. The core answers a forwarded get_state by
// sending every pane in the next view event, so a page that loaded late or
// reloaded gets the run back. A refusal is the core's reason there is no
// child: still starting, no repository found, or the child exited.
async function askState(tauri) {
  try {
    await tauri.core.invoke("rpc_send", {
      line: { id: "desktop-state", type: "get_state" },
    });
  } catch (err) {
    show("status", String(err));
  }
}

async function main() {
  wireRail();
  const tauri = window.__TAURI__;

  // Every listener is registered, and awaited, before anything is asked
  // for, so no answer arrives before something listens for it.
  await tauri.event.listen("view", (event) => {
    drawView(document, event.payload);
  });
  await tauri.event.listen("child-started", (event) => {
    const c = event.payload;
    show("status", `${c.version ?? "trellis (version unknown)"} rpc on ${c.root}`);
    askState(tauri);
  });
  await tauri.event.listen("no-child", (event) => {
    show("status", String(event.payload));
  });
  await tauri.event.listen("rpc-exited", (event) => {
    show("status", `trellis rpc exited: ${json(event.payload)}`);
  });
  // The overview's own drawing is #562's; until then only a failure to
  // read it is said, in the status line.
  await tauri.event.listen("overview-error", (event) => {
    show("status", `The overview could not be read: ${event.payload}`);
  });

  await askState(tauri);
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", () => {
    main().catch((err) => console.error("trellis desktop:", err));
  });
} else {
  main().catch((err) => console.error("trellis desktop:", err));
}
