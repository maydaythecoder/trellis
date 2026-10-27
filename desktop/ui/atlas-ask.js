// What the Atlas asks trellis, and when (atlas.js's section "asking
// trellis", split out of atlas.js by #955): the history once, then one
// index at a time, HEAD, its symbols and each earlier snapshot the core
// planned, newest first, each question carrying an Atlas id; and the
// status line that says how far it has got. It reaches the core only
// through rpc.js's send.

import { byId, list } from "./atlas-view.js";
import { isCount, str } from "./fmt.js";
import { send } from "./rpc.js";

const ID = "desktop-atlas-";
// get_codebase lists this many modules: all of any repository the layout
// draws (rpc's own default is 40).
const MODULES_ASKED = 100000;
// Every tag get_history gives, up to its cap: the core picks the snapshots
// from those dated in the two years before HEAD (atlas.rs).
const HISTORY_ASKED = { commits: 20, tags: 200 };

// --- asking trellis ----------------------------------------------------------

export function shown(A) {
  const s = byId(A, "atlas");
  return s !== null && !s.hidden;
}

export function status(A, text) {
  byId(A, "at-status").textContent = text;
}

// Sends one line through the core (rpc.js's send). A refusal drops the
// question in flight and says why in the Atlas's status line.
export function forward(A, line) {
  return send(A.tauri, line, (why) => {
    A.inflight = null;
    status(A, `Open a repository to see its atlas. ${why}`);
  });
}

export function nextId(A, kind) {
  A.asked += 1;
  return `${ID}${kind}-${A.asked}`;
}

// What to ask next: the history once, then one index at a time: HEAD, its
// symbols, then each snapshot the core planned, newest first. A question
// in flight, here or in the core's record, is waited for.
export function pump(A) {
  if (!shown(A) || !A.childUp) return;
  const v = A.view;
  if (!A.askedHistory && (v === null || v.history === null)) {
    A.askedHistory = true;
    forward(A, { id: nextId(A, "history"), type: "get_history", ...HISTORY_ASKED });
  }
  if (A.inflight !== null) return;
  const snaps = v ? list(v.snapshots) : [];
  if (snaps.some((s) => s.state === "asked")) return;
  const head = snaps[snaps.length - 1];
  if (!head || head.state === "planned") {
    A.inflight = { kind: "snapshot", label: "HEAD" };
    status(A, progressText(A));
    forward(A, { id: nextId(A, "codebase"), type: "get_codebase", edges: "all", limit: MODULES_ASKED });
    return;
  }
  if (head.state === "loaded" && v.symbols === null) {
    if (A.askedSymbols) return;
    A.askedSymbols = true;
    A.inflight = { kind: "symbols" };
    status(A, progressText(A));
    forward(A, { id: nextId(A, "symbols"), type: "get_symbols" });
    return;
  }
  if (!v.progress || v.progress.planned !== true) return;
  for (let i = snaps.length - 2; i >= 0; i--) {
    const s = snaps[i];
    if (s.state === "planned" && str(s.ref) !== null) {
      A.inflight = { kind: "snapshot", label: s.label };
      status(A, progressText(A));
      forward(A, { id: nextId(A, "codebase"), type: "get_codebase", edges: "all", limit: MODULES_ASKED, ref: s.ref });
      return;
    }
  }
}

// The question in flight has been answered once the view says so.
export function settle(A) {
  const f = A.inflight;
  if (f === null || A.view === null) return;
  if (f.kind === "symbols" && A.view.symbols !== null) A.inflight = null;
  if (f.kind === "snapshot") {
    const s = list(A.view.snapshots).find((x) => x.label === f.label);
    if (s && (s.state === "loaded" || s.state === "failed")) A.inflight = null;
  }
}

export function progressText(A) {
  const v = A.view;
  if (!v) {
    return A.inflight || A.askedHistory
      ? "Reading the repository's history and HEAD: trellis indexes it."
      : "The atlas is read when this section is shown.";
  }
  const snaps = list(v.snapshots);
  const head = snaps[snaps.length - 1];
  const p = v.progress || {};
  const parts = [];
  if (head && head.state === "failed") parts.push(`HEAD could not be read: ${head.error}`);
  else if (!head || head.state !== "loaded") parts.push("Reading HEAD: trellis indexes the repository.");
  if (v.history === null) parts.push("Reading the history.");
  else if (str(v.history.error)) parts.push(`History not read, so HEAD alone is shown: ${v.history.error}`);
  const loading = list(p.loading).filter((l) => l !== "HEAD");
  const f = A.inflight;
  if (f && f.kind === "snapshot" && f.label !== "HEAD" && !loading.includes(f.label)) loading.push(f.label);
  if (f && f.kind === "symbols") parts.push("Reading HEAD's symbols.");
  if (loading.length > 0 && isCount(p.settled) && isCount(p.of)) {
    parts.push(`Loading ${loading.join(", ")} … ${p.settled + 1} of ${p.of}.`);
  } else if (isCount(p.of) && p.of > 0 && p.settled < p.of) {
    parts.push(`Snapshots loaded: ${p.settled} of ${p.of}.`);
  }
  for (const s of snaps) {
    if (s.state === "failed" && s.label !== "HEAD") parts.push(`${s.label} could not be read: ${s.error}`);
  }
  if (v.layout && v.layout.fixed !== true && head && head.state === "loaded") {
    parts.push("The map is HEAD's until every snapshot has settled, then laid out once over all of them.");
  }
  if (v.symbols && str(v.symbols.error)) parts.push(`Symbols not read: ${v.symbols.error}`);
  return parts.length > 0 ? parts.join(" ") : "Every snapshot is loaded. The map stays put as the snapshot moves.";
}
