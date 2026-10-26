// What every module that talks to the trellis rpc child shares, one copy
// each (#816): sending a command, the id that tells its answer apart,
// starting a module, fetching when a section is shown, and knowing a run
// ended. The copies modules still keep move here ticket by ticket (#817 to
// #820).
//
// The page reaches trellis only through the core's commands and events
// (ADR 0048 section 5): `rpc_send` forwards one command, and the core
// refuses a type rpc does not list.

import { pageError } from "./dialogs.js";

// Sends one command to the child. A refusal (no child, a type rpc does not
// list, the model gate) is handed to `refused` as text and answers false,
// so the caller says it where the person is looking.
export async function send(tauri, line, refused) {
  try {
    await tauri.core.invoke("rpc_send", { line });
    return true;
  } catch (err) {
    if (refused) refused(String(err));
    return false;
  }
}

// The next id for a command `module` sends, `desktop-<module>-<kind>-<n>`:
// the core and the page match an answer to its question by it, and each
// module's prefix keeps one module's answers from being taken for
// another's.
const counters = new Map();
export function lineId(module, kind) {
  const n = (counters.get(module) ?? 0) + 1;
  counters.set(module, n);
  return `desktop-${module}-${kind}-${n}`;
}

// Starts a module: runs `wire` once the document is parsed and, when it
// fails, says so in the page's banner through dialogs.js. Every module used
// to end in `.catch(console.error)`, and a release window has no console,
// so a module that failed to start did so silently. The console still
// hears it, so a developer's devtools and the CI smoke run (ADR 0059) do
// too. A module script runs once the document is parsed, so the wait is
// only for a page that loaded one some other way.
export function start(name, wire) {
  const run = () =>
    Promise.resolve()
      .then(wire)
      .catch((err) => {
        pageError(`${name} did not start: ${err}`);
        console.error(`trellis desktop: ${name} did not start:`, err);
      });
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", run, { once: true });
  else run();
}

// Calls `show` each time the element with id `id` stops being hidden, so a
// section fetches what it shows when it is shown and not before. Answers a
// function that stops watching.
export function onShown(doc, id, show) {
  const section = doc.getElementById(id);
  if (!section) return () => {};
  let hidden = section.hidden;
  const watch = new MutationObserver(() => {
    if (hidden && !section.hidden) show();
    hidden = section.hidden;
  });
  watch.observe(section, { attributes: true, attributeFilter: ["hidden"] });
  return () => watch.disconnect();
}

// Whether a frame from the child is a run's end. The child's run_end is the
// one sign a run is over: the view's outcome changes then too, but a
// get_state reply sends the same outcome again.
export function runEnded(frame) {
  return frame !== null && typeof frame === "object" && frame.type === "run_end";
}
