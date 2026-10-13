// The web view's bridge (ADR 0050 section 5). When `trellis-desktop --web`
// serves this page there is no Tauri runtime, so this module, loaded before
// every other, gives them the two calls they make, `core.invoke` and
// `event.listen`, over the page's own origin: a POST to `invoke/<command>`
// and one server-sent event stream at `events`, both relative to the page's
// path, which carries the launch token. In the window Tauri has set
// window.__TAURI__ before any module runs, and this module does nothing.
//
// It is the one module that fetches, and only from its own path: every URL
// here is relative, and `src/web.rs` holds that. Nothing it receives is
// parsed as markup; a payload goes, as JSON, to the same listeners the
// window's events reach, and an invoke's refusal is the core's own text.

// Every event the core emits (`src/commands.rs`), heard from the start:
// an event the stream carries before a module has listened for it is kept,
// in order, and handed over when that module listens, as the window's
// on_page_load ordering gives it. The list is held against the Rust
// sources by a test in `src/web.rs`.
const EVENTS = [
  "child-started",
  "codebase",
  "no-child",
  "overview",
  "overview-error",
  "radius",
  "rpc-exited",
  "rpc-frame",
  "rpc-human",
  "rpc-unparsed",
  "view",
];
const KEPT_PER_EVENT = 64;

if (typeof window !== "undefined" && !window.__TAURI__) {
  const listeners = new Map();
  const kept = new Map();
  const stream = new EventSource("events");

  for (const name of EVENTS) {
    stream.addEventListener(name, (message) => {
      let payload;
      try {
        payload = JSON.parse(message.data);
      } catch {
        return;
      }
      const heard = listeners.get(name);
      if (heard && heard.size > 0) {
        for (const callback of heard) callback({ event: name, payload });
        return;
      }
      const queue = kept.get(name) ?? [];
      if (queue.length < KEPT_PER_EVENT) queue.push(payload);
      kept.set(name, queue);
    });
  }

  // Resolves once the callback is registered; the window's listen resolves
  // once Tauri has, and the modules await it before asking for anything.
  function listen(name, callback) {
    if (!listeners.has(name)) listeners.set(name, new Set());
    listeners.get(name).add(callback);
    const queue = kept.get(name);
    if (queue) {
      kept.delete(name);
      for (const payload of queue) callback({ event: name, payload });
    }
    return Promise.resolve(() => {
      const heard = listeners.get(name);
      if (heard) heard.delete(callback);
    });
  }

  // Rejects with the core's refusal as the window's invoke does: the text
  // itself, which the modules show through textContent.
  async function invoke(command, args) {
    const response = await fetch(`invoke/${command}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(args ?? {}),
    });
    let answer;
    try {
      answer = await response.json();
    } catch {
      throw `${command}: the web view answered ${response.status} with no JSON`;
    }
    if (!response.ok) {
      throw typeof answer.error === "string" ? answer.error : `${command}: ${response.status}`;
    }
    return answer.ok;
  }

  window.__TAURI__ = { core: { invoke }, event: { listen } };
}
