// The Atlas's search (atlas.js's section "search", split out of atlas.js
// by #955): a fuzzy match over every module's and symbol's name, opened
// with / or its button, over the map and not modal. Names are repository
// text and are written through textContent, the matched letters in <b>.

import { moveTo } from "./atlas.js";
import { select } from "./atlas-detail.js";
import { byId, label, list, rowAt } from "./atlas-view.js";
import { el } from "./dom.js";
import { isCount, notReported } from "./fmt.js";

const RESULTS_MAX = 40;

// --- search ------------------------------------------------------------------

// The search's candidates, built when an answer brings modules or symbols
// and kept while they are the same, so opening the search waits on nothing.
export function indexSearch(A) {
  if (A.D === null) {
    A.cands = null;
    A.candsKey = null;
    return;
  }
  const L = A.view.layout;
  const key = `${A.D.modules.length}|${A.D.fixed}|${list(L.over).join(",")}|${A.D.symbols.length}`;
  if (A.cands && A.candsKey === key) return;
  A.candsKey = key;
  const out = [];
  for (const m of A.D.modules) out.push({ t: "module", id: m.idx, text: m.module, low: m.module.toLowerCase(), cut: m.module.lastIndexOf(".") });
  for (const s of A.D.symbols) {
    const m = A.D.modules[s.m];
    if (!m) continue;
    const text = `${m.module}.${s.qual}`;
    out.push({ t: "symbol", id: s.idx, text, low: text.toLowerCase(), cut: m.module.length });
  }
  A.cands = out;
}

function candidates(A) {
  return A.cands ?? [];
}

const SEPS = "._/ ";

function fuzzy(q, c) {
  const s = c.low;
  const o = c.text;
  let best = null;
  let start = s.indexOf(q[0]);
  let tries = 0;
  const bonus = (j) => (j === 0 || SEPS.includes(s[j - 1]) ? 6 : 0) + (j > 0 && o[j] !== s[j] && o[j - 1] === s[j - 1] ? 4 : 0);
  while (start !== -1 && tries < 16) {
    const pos = [start];
    let score = bonus(start);
    let prev = start;
    let ok = true;
    for (let qi = 1; qi < q.length; qi++) {
      const j = s.indexOf(q[qi], prev + 1);
      if (j < 0) {
        ok = false;
        break;
      }
      score += (j === prev + 1 ? 5 : -Math.min(4, (j - prev - 1) * 0.2)) + bonus(j);
      pos.push(j);
      prev = j;
    }
    if (!ok) break;
    if (pos[0] > c.cut) score += 10;
    score -= s.length * 0.03;
    if (!best || score > best.score) best = { score, pos };
    start = s.indexOf(q[0], start + 1);
    tries += 1;
  }
  return best;
}

function runSearch(A, raw) {
  const terms = raw.toLowerCase().split(/\s+/).filter(Boolean);
  if (terms.length === 0 || A.D === null) return [];
  const res = [];
  for (const c of candidates(A)) {
    let score = 0;
    let pos = [];
    let ok = true;
    for (const t of terms) {
      const f = fuzzy(t, c);
      if (!f) {
        ok = false;
        break;
      }
      score += f.score;
      pos = pos.concat(f.pos);
    }
    if (!ok) continue;
    const segs = c.low.split(".");
    for (const t of terms) {
      if (segs.includes(t)) score += 6;
      if (segs[segs.length - 1] === t) score += 6;
    }
    score += c.t === "module" ? 3 : Math.min(3, Math.log2(1 + (isCount(A.D.symbols[c.id].cin) ? A.D.symbols[c.id].cin : 0)) * 0.5);
    res.push({ c, score, pos });
  }
  res.sort((a, b) => b.score - a.score);
  return res.slice(0, RESULTS_MAX);
}

function highlighted(A, text, pos) {
  const set = new Set(pos);
  const out = [];
  let buf = "";
  let on = false;
  for (let i = 0; i < text.length; i++) {
    const m = set.has(i);
    if (m !== on && buf) {
      out.push(on ? el(A.doc, "b", buf) : buf);
      buf = "";
    }
    on = m;
    buf += text[i];
  }
  if (buf) out.push(on ? el(A.doc, "b", buf) : buf);
  return out;
}

export function renderResults(A) {
  const q = byId(A, "at-q").value;
  A.results = runSearch(A, q);
  A.active = 0;
  const hint = byId(A, "at-qhint");
  if (A.D === null) hint.textContent = "Nothing to search until the map is drawn.";
  else if (!q.trim()) hint.textContent = "Fuzzy match over module and symbol names. Arrow keys move, Enter goes there.";
  else hint.textContent = A.results.length ? `${A.results.length}${A.results.length === RESULTS_MAX ? "+" : ""} matches` : "No module or symbol matches.";
  byId(A, "at-qres").replaceChildren(
    ...A.results.map((r, i) => {
      const c = r.c;
      let meta;
      if (c.t === "module") {
        const row = rowAt(A, A.si, c.id);
        meta = row ? `${notReported(row[1])} defs` : `not in ${label(A, A.si)}`;
      } else {
        const s = A.D.symbols[c.id];
        meta = `line ${notReported(s.line)} · in ${notReported(s.cin)}`;
      }
      const li = el(A.doc, "li");
      li.id = `at-qr${i}`;
      li.setAttribute("role", "option");
      li.setAttribute("aria-selected", String(i === A.active));
      li.addEventListener("click", () => pick(A, i));
      li.addEventListener("mousemove", () => setActive(A, i));
      const x = el(A.doc, "span", null, "at-x");
      x.append(...highlighted(A, c.text, r.pos));
      li.append(el(A.doc, "span", c.t === "module" ? "module" : A.D.symbols[c.id].kind, "at-k"), x, el(A.doc, "span", meta, "at-m"));
      return li;
    }),
  );
  byId(A, "at-q").setAttribute("aria-activedescendant", A.results.length ? "at-qr0" : "");
}

export function setActive(A, i) {
  if (i === A.active || A.results.length === 0) return;
  const a = byId(A, `at-qr${A.active}`);
  if (a) a.setAttribute("aria-selected", "false");
  A.active = i;
  const b = byId(A, `at-qr${i}`);
  if (b) b.setAttribute("aria-selected", "true");
  byId(A, "at-q").setAttribute("aria-activedescendant", `at-qr${i}`);
}

export function openSearch(A) {
  A.lastFocus = A.doc.activeElement;
  byId(A, "at-search").hidden = false;
  const q = byId(A, "at-q");
  q.focus();
  q.select();
  renderResults(A);
}

export function closeSearch(A) {
  byId(A, "at-search").hidden = true;
  if (A.lastFocus && typeof A.lastFocus.focus === "function") A.lastFocus.focus();
}

export function pick(A, i) {
  const r = A.results[i];
  if (!r) return;
  closeSearch(A);
  if (r.c.t === "module") select(A, { t: "module", m: r.c.id }, { fly: true });
  else {
    // Symbols are HEAD's.
    if (A.D && A.si !== A.D.head) moveTo(A, A.D.head);
    select(A, { t: "symbol", i: r.c.id }, { fly: true });
  }
}

