// The TRELLIS desktop page's one module (ADR 0048 section 5).
//
// It renders what the desktop's Rust core forwards and computes nothing: at
// this stage, the reply to one get_state request. There is no bundler, so it
// reaches Tauri through the global window.__TAURI__ rather than an import of
// @tauri-apps/api.
//
// Every frame is trellis-supplied text, so it reaches the DOM only through
// textContent (ADR 0007, S10): a frame that carries markup is shown as
// characters, never parsed.

const REQUEST_ID = "desktop-1";

// No core code emits rpc-frame yet (DESK-01d adds it), so this accepts
// either shape it may choose: the parsed object, or the JSON line read from
// trellis's stdout. Anything that is neither is not a frame.
function asFrame(payload) {
  if (typeof payload === "string") {
    try {
      return JSON.parse(payload);
    } catch {
      return null;
    }
  }
  return payload;
}

async function main() {
  const tauri = window.__TAURI__;
  const out = document.getElementById("state");

  // The listener is registered, and awaited, before the request is sent, so
  // the reply cannot arrive before anything is listening for it.
  await tauri.event.listen("rpc-frame", (event) => {
    const frame = asFrame(event.payload);
    if (frame !== null && typeof frame === "object" && frame.id === REQUEST_ID) {
      out.textContent = JSON.stringify(frame, null, 2);
    }
  });

  await tauri.core.invoke("rpc_send", {
    line: { id: REQUEST_ID, type: "get_state" },
  });
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", () => {
    main().catch((err) => console.error("trellis desktop:", err));
  });
} else {
  main().catch((err) => console.error("trellis desktop:", err));
}
