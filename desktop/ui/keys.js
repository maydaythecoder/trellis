// The page's levels and the keys that move between them (#826; ADR 0048
// section 5 as amended 2026-10-26). The Trail and the rail are the design
// system's (ds.css's .cd-trail and .cd-shell-rail, #909).
//
// The page is one overview a person burrows into, with no tabs. The Trail
// along the top says where they are ("Overview ⟩ Run", "L2 / 4"), and each
// earlier segment is a way back. Below the overview, the sibling rail on the
// left holds the overview's other panels, each still a way in; the status
// bar along the bottom names the keys. ⏎ on one of the overview's panels
// (each is a button) or a click on it goes in; ESC goes out one level, back
// to the panel it came from, and each level keeps the scroll it had. A
// section is a level's stage: entering shows it, and the section's module
// fetches what it shows then (rpc.js's onShown), as it did when a tab
// showed it.
//
// This module owns ESC for the whole page. Typing in a field is never taken
// as navigation (ESC there leaves the field), an open dialog keeps its own
// ESC, and a module that uses ESC for itself (the Atlas's search and
// selection) registers through onEscape and is asked first; a key handler
// on an element that uses ESC calls preventDefault, and the page's ESC
// leaves it alone.
//
// What a level draws here (the Trail's way back and the sibling rail) it
// draws under the level's scope: each listener takes the scope's
// AbortController signal, and leaving the level aborts it, calls every
// unlisten the level registered and empties what it drew, so nothing a
// level added outlives it (leaving_a_level_removes_every_listener_it_added
// in src/lockdown.rs; the CI smoke run counts the page's listeners after
// walking in and out of every level).

import { el } from "./dom.js";

// The depth the Trail's gauge counts to: the overview, a section, and the
// two levels below a section the run view adds (#831, #832).
export const LEVELS = 4;

// The levels shown, the overview first, as section ids.
const path = ["overview"];
// The stage's scroll at each level, kept while another is shown.
const scrolls = new Map();
// Handlers a module registers to take ESC before the page goes out a level.
const escapes = new Set();

function newScope() {
  return { controller: new AbortController(), unlisten: [] };
}

// What the level shown registered; ended when it is left.
let scope = newScope();

// The scope of the level shown, for anything added while a level is shown:
// pass `signal` to addEventListener and `add` each Tauri unlisten function,
// and both end when the person leaves the level.
export function levelScope() {
  return { signal: scope.controller.signal, add: (unlisten) => scope.unlisten.push(unlisten) };
}

// Ends what the level shown registered, and starts the next level's scope.
function endScope() {
  scope.controller.abort();
  for (const unlisten of scope.unlisten.splice(0)) unlisten();
  scope = newScope();
}

// Registers a handler asked first when ESC is pressed outside a field and a
// dialog: it answers true when it used the key. Answers a function that
// takes the handler back.
export function onEscape(handler) {
  escapes.add(handler);
  return () => escapes.delete(handler);
}

// Whether `t` takes typing: a key pressed there is text, not navigation.
export function typing(t) {
  return t !== null && typeof t === "object" && (["INPUT", "SELECT", "TEXTAREA"].includes(t.tagName) || t.isContentEditable === true);
}

// The sections the overview's panels enter, in the order they list them.
function sections(doc) {
  return [...doc.querySelectorAll("button[data-enter]")].map((b) => b.dataset.enter);
}

// A section's name, as its heading says it.
function title(doc, id) {
  const section = doc.getElementById(id);
  const heading = section && doc.getElementById(section.getAttribute("aria-labelledby"));
  return heading ? heading.textContent.trim() : id;
}

// Shows the level at the end of the path, and draws the Trail and the
// sibling rail for it.
function show(doc) {
  const here = path[path.length - 1];
  for (const s of doc.querySelectorAll("#stage > section")) s.hidden = s.id !== here;
  doc.getElementById("stage").scrollTop = scrolls.get(here) ?? 0;
  drawTrail(doc);
  drawSiblings(doc);
}

function drawTrail(doc) {
  const { signal } = levelScope();
  const items = path.map((id, i) => {
    const li = el(doc, "li");
    // The separator is drawn, not read: the list already says the order.
    if (i > 0) {
      const sep = el(doc, "span", "⟩", "cd-trail-sep");
      sep.setAttribute("aria-hidden", "true");
      li.append(sep);
    }
    if (i === path.length - 1) {
      const current = el(doc, "span", title(doc, id), "cd-trail-current");
      current.setAttribute("aria-current", "location");
      li.append(current);
    } else {
      const back = el(doc, "button", title(doc, id), "cd-trail-seg");
      back.type = "button";
      back.addEventListener("click", () => go(doc, i), { signal });
      li.append(back);
    }
    return li;
  });
  doc.getElementById("trail-path").replaceChildren(...items);
  [...doc.getElementById("trail-ticks").children].forEach((t, i) => t.classList.toggle("cd-trail-tick-on", i < path.length));
  doc.getElementById("trail-level").textContent = `L${path.length} / ${LEVELS}`;
}

// The overview's panels, collapsed down the left, below the overview only.
// Below a section (#831) the section the person came through stays marked.
function drawSiblings(doc) {
  const rail = doc.getElementById("siblings");
  if (path.length < 2) {
    rail.replaceChildren();
    rail.hidden = true;
    return;
  }
  const { signal } = levelScope();
  const here = path[1];
  const items = sections(doc).map((id) => {
    const b = el(doc, "button", title(doc, id));
    b.type = "button";
    if (id === here) b.setAttribute("aria-current", "location");
    b.addEventListener("click", () => enter(doc, id), { signal });
    return b;
  });
  rail.replaceChildren(...items);
  rail.hidden = false;
}

// Keeps the stage's scroll for the level shown and ends what it registered.
function leave(doc) {
  scrolls.set(path[path.length - 1], doc.getElementById("stage").scrollTop);
  endScope();
}

// Goes back to level `i` of the path (the overview is 0), and puts the focus
// on the panel the person went in through.
function go(doc, i) {
  if (i >= path.length - 1) return;
  leave(doc);
  const left = path.splice(i + 1);
  show(doc);
  // A level below a section (#831) was entered by a data-burrow button, the
  // one for the file shown marked current; a section, by its panel.
  const panel =
    doc.querySelector(`button[data-enter="${left[0]}"]`) ??
    doc.querySelector(`button[data-burrow="${left[0]}"][aria-current="true"]`) ??
    doc.querySelector(`button[data-burrow="${left[0]}"]`);
  if (panel) panel.focus();
}

// Enters section `id`: from the overview, a level down; from a section, its
// sibling at the same level. The section's heading takes the focus, so a
// screen reader says where the person now is.
export function enter(doc, id) {
  const section = doc.getElementById(id);
  if (section === null || id === "overview") return;
  leave(doc);
  path.splice(1, path.length - 1, id);
  show(doc);
  const heading = doc.getElementById(section.getAttribute("aria-labelledby"));
  if (heading) heading.focus();
}

// Shows the levels `below` the overview, a section and what the person
// burrows into under it (#831: the Code level below Run, a file below
// Code), whatever level is shown now. The last one's heading takes the
// focus, as entering a section's does.
export function burrow(doc, below) {
  const section = doc.getElementById(below[below.length - 1]);
  if (section === null || below.length + 1 > LEVELS) return;
  leave(doc);
  path.splice(1, path.length - 1, ...below);
  show(doc);
  const heading = doc.getElementById(section.getAttribute("aria-labelledby"));
  if (heading) heading.focus();
}

// Goes out one level; false at the overview, which has none above it.
export function out(doc) {
  if (path.length < 2) return false;
  go(doc, path.length - 2);
  return true;
}

// Wires the overview's panels and the page's ESC, once, and draws the
// overview's level.
export function wireKeys(doc) {
  for (const b of doc.querySelectorAll("button[data-enter]")) {
    b.addEventListener("click", () => enter(doc, b.dataset.enter));
  }
  doc.addEventListener("keydown", (e) => {
    if (e.key !== "Escape" || e.defaultPrevented) return;
    // A dialog closes on its own ESC; the level stays.
    if (doc.querySelector("dialog[open]")) return;
    if (typing(e.target)) {
      e.target.blur();
      return;
    }
    for (const handler of [...escapes].reverse()) {
      if (handler(e)) {
        e.preventDefault();
        return;
      }
    }
    if (out(doc)) e.preventDefault();
  });
  show(doc);
}
