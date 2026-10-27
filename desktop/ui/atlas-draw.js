// The Atlas's map, drawn on its canvas (atlas.js's sections "drawing" and
// "pointing", split out of atlas.js by #955): regions, modules and the
// symbols inside an open module, the edges summed for the zoom drawn, the
// labels that fit, the hover card and the minimap, and what lies under the
// pointer. The canvas parses nothing, so repository text drawn here is
// drawn as text (ADR 0007, S10). Every colour is one the stylesheet names,
// read by atlas-view.js's readPalette.

import { byId, CALLEE, CALLER, churnBin, churnCount, COMMIT, diff, DOUBT, fitT, hatch, inkOn, isHead, label, list, onScreen, OPEN_H, OPEN_W, present, radiusQuery, REACH, rowAt, scr, SEL, snap, WORLD_H } from "./atlas-view.js";
import { isCount, notReported, plural } from "./fmt.js";

// A region's edges are drawn to its modules once its short side is this
// many pixels; until then they are summed onto the region.
const OPEN_REGION = 300;
// The most aggregated edges drawn with nothing selected.
const EDGE_CAP = 60;
// World units per square root of a call site, for the doubt lens's two
// squares: one scale for every module and snapshot.
const GLYPH = 2.3;

const FONT = {
  top: "600 13px system-ui, sans-serif",
  dir: "600 12px system-ui, sans-serif",
  dirS: "600 11px system-ui, sans-serif",
  mod: "11px system-ui, sans-serif",
  modB: "600 12px system-ui, sans-serif",
  sub: "10.5px system-ui, sans-serif",
  badge: "600 11px system-ui, sans-serif",
  sym: "11px system-ui, sans-serif",
  symB: "600 11px system-ui, sans-serif",
  tip: "12px system-ui, sans-serif",
  tipB: "600 12px system-ui, sans-serif",
};

// --- drawing -----------------------------------------------------------------

function measure(A, font, t) {
  const key = `${font}|${t}`;
  let v = A.widths.get(key);
  if (v === undefined) {
    A.ctx.font = font;
    v = A.ctx.measureText(t).width;
    if (A.widths.size > 30000) A.widths.clear();
    A.widths.set(key, v);
  }
  return v;
}

function ellipsize(A, font, t, maxW) {
  if (measure(A, font, t) <= maxW) return t;
  let lo = 0;
  let hi = t.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (measure(A, font, `${t.slice(0, mid)}…`) <= maxW) lo = mid;
    else hi = mid - 1;
  }
  return lo >= 3 ? `${t.slice(0, lo)}…` : null;
}

// Draws the labels queued this frame, highest priority first, each only
// where no label already drawn sits, so no two overlap.
function placeLabels(A) {
  const ctx = A.ctx;
  A.labels.sort((a, b) => a.pri - b.pri);
  const taken = [];
  A.drawn = [];
  ctx.textBaseline = "middle";
  for (const L of A.labels) {
    if (L.needs && !L.needs.placed) continue;
    if (L.rx !== undefined && L.x < 4) {
      L.maxW = Math.min(L.maxW === undefined ? 1e9 : L.maxW, L.rx - 8);
      L.x = 4;
    }
    if (L.ry !== undefined && L.y < L.size / 2 + 3 + (L.dy || 0)) {
      const y = L.size / 2 + 3 + (L.dy || 0);
      if (y + L.size / 2 + 2 > L.ry) continue;
      L.y = y;
    }
    let text = L.text;
    if (L.maxW !== undefined) {
      if (L.maxW < 14) continue;
      text = ellipsize(A, L.font, text, L.maxW);
      if (!text) continue;
    }
    const tw = measure(A, L.font, text);
    const px = L.pill ? 4 : 1;
    const x0 = L.align === "center" ? L.x - tw / 2 : L.x;
    const box = [x0 - px, L.y - L.size / 2 - 1, x0 + tw + px, L.y + L.size / 2 + 1];
    if (box[2] < 0 || box[0] > A.vw || box[3] < 0 || box[1] > A.vh) continue;
    if (taken.some((o) => box[0] < o[2] && o[0] < box[2] && box[1] < o[3] && o[1] < box[3])) continue;
    taken.push(box);
    L.placed = true;
    A.drawn.push({ text, box, needs: L.needs ? L.needs.text : null });
    ctx.globalAlpha = L.alpha === undefined ? 1 : L.alpha;
    ctx.font = L.font;
    if (L.pill) {
      ctx.fillStyle = A.P.panel;
      ctx.strokeStyle = L.border || A.P.line;
      ctx.lineWidth = 1.25;
      ctx.beginPath();
      ctx.rect(box[0], box[1], box[2] - box[0], box[3] - box[1]);
      ctx.fill();
      ctx.stroke();
    } else if (L.halo) {
      ctx.strokeStyle = L.halo;
      ctx.lineWidth = 3;
      ctx.lineJoin = "round";
      ctx.strokeText(text, x0, L.y);
    }
    ctx.fillStyle = L.color;
    ctx.fillText(text, x0, L.y);
  }
  ctx.globalAlpha = 1;
}

export function redraw(A) {
  if (A.queued) return;
  A.queued = true;
  const run = () => {
    A.queued = false;
    draw(A);
  };
  if (typeof A.win.requestAnimationFrame === "function") A.win.requestAnimationFrame(run);
  else run();
}

function draw(A) {
  const ctx = A.ctx;
  if (!ctx || A.vw <= 1 || A.vh <= 1) return;
  ctx.setTransform(A.dpr, 0, 0, A.dpr, 0, 0);
  ctx.fillStyle = A.P.ground;
  ctx.fillRect(0, 0, A.vw, A.vh);
  A.labels = [];
  if (A.D === null) {
    A.drawn = [];
    drawMinimap(A);
    return;
  }
  const F = A.F;
  const D = A.lens === "diff" ? diff(A) : null;
  drawRegions(A, F);
  const glyphs = [];
  for (const m of A.D.modules) {
    const r = scr(A, m);
    if (!onScreen(A, r)) continue;
    const w = r[2] - r[0];
    const h = r[3] - r[1];
    if (w < 0.5 || h < 0.5) continue;
    drawModule(A, m, r, w, h, F, D, glyphs);
  }
  drawEdges(A, F, D);
  if (A.lens === "doubt") for (const g of glyphs) drawGlyphs(A, ...g);
  placeLabels(A);
  if (A.hover && A.tipAt) drawTip(A);
  drawMinimap(A);
  const note = byId(A, "at-lg-edges");
  if (note && note.textContent !== A.edgeNote) note.textContent = A.edgeNote;
}

function regionLabel(A, g) {
  const S = snap(A, A.si);
  const counts = S ? list(S.s.regions)[g.idx] : null;
  const held = Array.isArray(counts) && isCount(counts[0]) ? counts[0] : null;
  const kind = g.test ? "test code" : "source";
  return held === null ? `${g.label} · ${kind}` : `${g.label} · ${kind} · ${plural(held, "module", "modules")}`;
}

function drawRegions(A, F) {
  const ctx = A.ctx;
  const P = A.P;
  const k = A.t.k;
  for (const g of A.D.regions) {
    if (g.depth === 0) continue;
    const r = scr(A, g);
    if (!onScreen(A, r)) continue;
    const w = r[2] - r[0];
    const h = r[3] - r[1];
    if (w < 2 || h < 2) continue;
    ctx.fillStyle = g.test ? P["region-test"] : P.region;
    ctx.fillRect(r[0], r[1], w, h);
    const hh = g.hdr * k;
    if (hh >= 1.5) {
      ctx.fillStyle = g.test ? P["region-head-test"] : P["region-head"];
      ctx.fillRect(r[0], r[1], w, hh);
    }
    if (w > 5 && h > 5) {
      ctx.strokeStyle = P.line;
      ctx.lineWidth = 1;
      ctx.strokeRect(r[0] + 0.5, r[1] + 0.5, w - 1, h - 1);
    }
    if (F && F.region === g.idx) {
      ctx.strokeStyle = P.ink;
      ctx.lineWidth = 2.5;
      ctx.strokeRect(r[0] + 1, r[1] + 1, w - 2, h - 2);
    }
    if (A.hover && A.hover.t === "region" && A.hover.r === g.idx) {
      ctx.strokeStyle = P.ink;
      ctx.lineWidth = 1.5;
      ctx.strokeRect(r[0] + 0.75, r[1] + 0.75, w - 1.5, hh);
    }
    if (g.depth === 1) {
      A.labels.push({
        pri: 0,
        x: r[0] + 6,
        y: r[1] + Math.max(hh, 16) / 2,
        text: regionLabel(A, g),
        font: FONT.top,
        size: 13,
        color: P.ink,
        maxW: w - 12,
        rx: r[2],
        ry: r[3],
        halo: g.test ? P["region-head-test"] : P["region-head"],
      });
    } else if (hh >= 10) {
      const big = hh >= 15;
      A.labels.push({
        pri: 2 + g.depth * 0.1,
        x: r[0] + 4,
        y: r[1] + hh / 2,
        text: g.label,
        font: big ? FONT.dir : FONT.dirS,
        size: big ? 12 : 11,
        color: P.ink,
        maxW: w - 8,
        rx: r[2],
      });
    }
  }
}

function cellFill(A, m) {
  return m.test ? A.P["cell-test"] : A.P.cell;
}

function moduleFill(A, m) {
  if (A.lens === "churn") {
    const b = churnBin(churnCount(A, m.idx));
    return b > 0 ? A.P[`churn-${b}`] : cellFill(A, m);
  }
  return cellFill(A, m);
}

// The line under a module's name at middle zoom: what the lens shows.
function lensLine(A, m) {
  const r = rowAt(A, A.si, m.idx);
  if (!r) return null;
  if (A.lens === "churn") {
    const c = churnCount(A, m.idx);
    return isCount(c) ? plural(c, "commit", "commits") : "commits not reported";
  }
  if (A.lens === "doubt") return `${notReported(r[2])} amb · ${notReported(r[3])} unres`;
  if (A.lens === "diff") {
    const D = diff(A);
    if (D.added.has(m.idx)) return "added";
    const o = rowAt(A, A.base, m.idx);
    if (o && o[1] !== r[1]) return `defs ${notReported(o[1])} → ${notReported(r[1])}`;
  }
  return `${notReported(r[1])} defs`;
}

function ring(A, r, w, h, color, lw, inset) {
  const ctx = A.ctx;
  ctx.strokeStyle = color;
  ctx.lineWidth = lw;
  ctx.strokeRect(r[0] + inset, r[1] + inset, Math.max(0, w - 2 * inset), Math.max(0, h - 2 * inset));
}

function drawModule(A, m, r, w, h, F, D, glyphs) {
  const ctx = A.ctx;
  const P = A.P;
  const pr = present(A, A.si, m.idx);
  const role = F ? F.role.get(m.idx) || 0 : 0;
  const dim = F !== null && role === 0 && F.region === null;
  const fill = moduleFill(A, m);
  ctx.globalAlpha = dim ? 0.28 : 1;
  if (pr) {
    ctx.fillStyle = fill;
    ctx.fillRect(r[0], r[1], w, h);
    if (w >= 3 && h >= 3) {
      ctx.strokeStyle = P["cell-line"];
      ctx.lineWidth = 1;
      ctx.strokeRect(r[0] + 0.5, r[1] + 0.5, w - 1, h - 1);
    }
  } else if (w >= 2 && h >= 2) {
    // A ghost: in the layout, not in this snapshot.
    const gone = D !== null && D.removed.has(m.idx);
    ctx.globalAlpha = dim ? 0.25 : gone ? 1 : 0.6;
    ctx.setLineDash(gone ? [4, 2] : [3, 3]);
    ctx.strokeStyle = gone ? P["diff-del"] : P["ink-muted"];
    ctx.lineWidth = gone ? 1.5 : 1;
    ctx.strokeRect(r[0] + 0.75, r[1] + 0.75, w - 1.5, h - 1.5);
    ctx.setLineDash([]);
  }
  ctx.globalAlpha = 1;
  if (D !== null && D.added.has(m.idx) && pr) {
    ctx.fillStyle = P["diff-add-soft"];
    ctx.fillRect(r[0], r[1], w, h);
    ring(A, r, w, h, P["diff-add"], 1.5, 0.75);
  }
  const open = pr && isHead(A) && w >= OPEN_W && h >= OPEN_H;
  if (role & REACH) {
    ctx.fillStyle = P["reach-soft"];
    ctx.fillRect(r[0], r[1], w, h);
  }
  if (role & DOUBT && !open) {
    ctx.fillStyle = hatch(A, P.doubt);
    ctx.fillRect(r[0], r[1], w, h);
  }
  if (role & COMMIT) {
    ctx.fillStyle = P["signal-soft"];
    ctx.fillRect(r[0], r[1], w, h);
  }
  if (role & CALLER && A.lens === "structure") {
    ctx.fillStyle = P["caller-soft"];
    ctx.fillRect(r[0], r[1], w, h);
  }
  if (role & CALLEE && A.lens === "structure") {
    ctx.fillStyle = P["callee-soft"];
    ctx.fillRect(r[0], r[1], w, h);
  }
  if (open) drawSymbols(A, m, F);
  if (role & REACH) ring(A, r, w, h, P.reach, 1.5, 0.75);
  if (role & DOUBT) {
    if (open) {
      ctx.strokeStyle = hatch(A, P.doubt);
      ctx.lineWidth = 8;
      ctx.strokeRect(r[0] + 4, r[1] + 4, w - 8, h - 8);
    }
    ring(A, r, w, h, P.doubt, 1, 0.5);
  }
  if (role & CALLER) ring(A, r, w, h, P.caller, 2.25, 1.1);
  if (role & CALLEE) ring(A, r, w, h, P.callee, 2.25, role & CALLER ? 3.4 : 1.1);
  if (role & COMMIT) ring(A, r, w, h, P.signal, 2.5, 1.25);
  if (role & SEL) ring(A, r, w, h, P.ink, 2.75, 1.4);
  if (A.hover && A.hover.t === "module" && A.hover.m === m.idx) ring(A, r, w, h, P.ink, 1.5, 0.75);
  if (A.pins.has(m.path) && w >= 8 && h >= 8) {
    const s = Math.min(9, w * 0.3, h * 0.3);
    ctx.fillStyle = P.signal;
    ctx.beginPath();
    ctx.moveTo(r[2] - s, r[1]);
    ctx.lineTo(r[2], r[1]);
    ctx.lineTo(r[2], r[1] + s);
    ctx.closePath();
    ctx.fill();
  }
  if (D !== null && w >= 9 && h >= 9) {
    const mark = D.added.has(m.idx) ? "+" : D.removed.has(m.idx) ? "−" : null;
    if (mark) {
      A.labels.push({
        pri: 1.5,
        x: r[2] - 3 - measure(A, FONT.badge, mark),
        y: r[3] - 7,
        text: mark,
        font: FONT.badge,
        size: 11,
        color: mark === "+" ? P["diff-add"] : P["diff-del"],
      });
    }
  }
  if (A.lens === "doubt" && pr && !open) {
    const row = rowAt(A, A.si, m.idx);
    if ((isCount(row[2]) && row[2] > 0) || (isCount(row[3]) && row[3] > 0)) glyphs.push([r, row, dim]);
  }
  const a = dim ? 0.4 : 1;
  const ink = pr ? inkOn(A, fill) : P["ink-muted"];
  if (open) {
    const syms = A.D.symsOf.get(m.idx) || [];
    const hh = Math.min(m.y1 - m.y0, m.x1 - m.x0) * 0.1 * A.t.k;
    const name = {
      pri: role & SEL ? 1 : 3,
      x: r[0] + 5,
      y: r[1] + Math.min(hh, 22) / 2 + 1,
      text: m.module,
      font: FONT.modB,
      size: 12,
      color: P.ink,
      halo: fill,
      maxW: w - 10,
      alpha: a,
      rx: r[2],
      ry: r[3],
    };
    A.labels.push(name);
    if (syms.length === 0) {
      const why = A.view && A.view.symbols === null ? "symbols not read yet" : "no definitions";
      A.labels.push({ needs: name, pri: 6, x: r[0] + 5, y: r[1] + hh + 10, text: why, font: FONT.sub, size: 11, color: P["ink-muted"], maxW: w - 10 });
    }
    return;
  }
  if (w >= 24 && h >= 13) {
    const pri = role & SEL ? 1 : 4 + 1 / (1 + w * h);
    const name = {
      pri,
      x: r[0] + 3,
      y: r[1] + Math.min(8, h / 2),
      text: m.label,
      font: role & SEL ? FONT.modB : FONT.mod,
      size: 11,
      color: ink,
      halo: pr ? fill : undefined,
      maxW: w - 6,
      alpha: a,
      rx: r[2],
      ry: r[3],
    };
    A.labels.push(name);
    if (h >= 30 && w >= 46 && pr) {
      const t = lensLine(A, m);
      if (t) {
        // A count is drawn only under its module's name: where the name
        // does not fit, neither does the count.
        A.labels.push({
          needs: name,
          pri: pri + 1,
          x: r[0] + 3,
          y: r[1] + 21,
          text: t,
          font: FONT.sub,
          size: 11,
          color: ink,
          halo: fill,
          maxW: w - 6,
          alpha: a * 0.8,
          rx: r[2],
          ry: r[3],
          dy: 13,
        });
      }
    }
  }
}

function drawSymbols(A, m, F) {
  const ctx = A.ctx;
  const P = A.P;
  const k = A.t.k;
  const rq = A.view && A.view.radius && A.view.radius.state === "answered" ? A.view.radius.symbol : null;
  for (const i of A.D.symsOf.get(m.idx) || []) {
    const s = A.D.symbols[i];
    const r = scr(A, s);
    const w = r[2] - r[0];
    const h = r[3] - r[1];
    if (w < 1.2 || h < 1.2 || !onScreen(A, r)) continue;
    ctx.fillStyle = s.holds ? (m.test ? P["region-test"] : P.region) : cellFill(A, m);
    ctx.fillRect(r[0], r[1], w, h);
    if (s.holds) {
      ctx.fillStyle = m.test ? P["region-head-test"] : P["region-head"];
      ctx.fillRect(r[0], r[1], w, s.hdr * k);
    }
    if (w >= 3 && h >= 3) {
      ctx.strokeStyle = P["cell-line"];
      ctx.lineWidth = 1;
      ctx.strokeRect(r[0] + 0.5, r[1] + 0.5, w - 1, h - 1);
    }
    if (F && F.reachSyms && F.reachSyms.has(i)) {
      ctx.fillStyle = P["reach-soft"];
      ctx.fillRect(r[0], r[1], w, s.holds ? s.hdr * k : h);
      ring(A, r, w, h, P.reach, 1.5, 0.75);
    }
    if (F && F.sym === i) ring(A, r, w, h, P.ink, 2.5, 1.25);
    if (A.hover && A.hover.t === "symbol" && A.hover.i === i) ring(A, r, w, h, P.ink, 1.5, 0.75);
    const hasRadius = rq !== null && rq === radiusQuery(A, i);
    if (hasRadius && w >= 14 && h >= 10) {
      ctx.fillStyle = P.signal;
      ctx.beginPath();
      ctx.arc(r[2] - 5, r[1] + 5, 2.6, 0, Math.PI * 2);
      ctx.fill();
    }
    const lh = s.holds ? s.hdr * k : h;
    if (w >= 22 && lh >= 11) {
      const fill = s.holds ? (m.test ? P["region-head-test"] : P["region-head"]) : cellFill(A, m);
      const pri = F && F.sym === i ? 1 : 5 + s.depth * 0.1 + 1 / (1 + w * h);
      // Named by its kind: a function that holds nested definitions is a
      // box on the map, and still a function.
      const name = {
        pri,
        x: r[0] + 3,
        y: r[1] + Math.min(lh, 16) / 2 + 0.5,
        text: s.kind === "class" ? `class ${s.name}` : `${s.name}()`,
        font: s.holds || (F && F.sym === i) ? FONT.symB : FONT.sym,
        size: 11,
        color: P.ink,
        halo: fill,
        maxW: w - (hasRadius ? 14 : 6),
      };
      A.labels.push(name);
      if (!s.holds && h >= 28 && w >= 64) {
        A.labels.push({
          needs: name,
          pri: pri + 1,
          x: r[0] + 3,
          y: r[1] + 22.5,
          text: `in ${notReported(s.cin)} · amb ${notReported(s.camb)} · out ${notReported(s.cout)}`,
          font: FONT.sub,
          size: 11,
          color: P["ink-muted"],
          halo: fill,
          maxW: w - 6,
        });
      }
    }
  }
}

// The doubt lens: two squares per module, ambiguous filled and unresolved
// hatched, each with its area by its own count. Two counts, never a share.
function drawGlyphs(A, r, row, dim) {
  const ctx = A.ctx;
  const k = A.t.k;
  const a = isCount(row[2]) ? row[2] : 0;
  const u = isCount(row[3]) ? row[3] : 0;
  const sa = a ? Math.max(1.5, Math.sqrt(a) * GLYPH * k) : 0;
  const su = u ? Math.max(1.5, Math.sqrt(u) * GLYPH * k) : 0;
  const pad = Math.min(3, (r[2] - r[0]) * 0.08);
  ctx.save();
  ctx.beginPath();
  ctx.rect(r[0], r[1], r[2] - r[0], r[3] - r[1]);
  ctx.clip();
  ctx.globalAlpha = dim ? 0.3 : 1;
  let x = r[2] - pad;
  const yb = r[3] - pad;
  if (su) {
    ctx.fillStyle = A.P.panel;
    ctx.fillRect(x - su, yb - su, su, su);
    ctx.fillStyle = hatch(A, A.P.doubt);
    ctx.fillRect(x - su, yb - su, su, su);
    ctx.strokeStyle = A.P.doubt;
    ctx.lineWidth = 1;
    ctx.strokeRect(x - su + 0.5, yb - su + 0.5, su - 1, su - 1);
    x -= su + 2;
  }
  if (sa) {
    ctx.fillStyle = A.P.doubt;
    ctx.fillRect(x - sa, yb - sa, sa, sa);
  }
  ctx.restore();
}

// Where the segment from `r`'s centre towards (tx, ty) leaves `r`.
function clipTo(cx, cy, tx, ty, r) {
  const dx = tx - cx;
  const dy = ty - cy;
  const hw = (r[2] - r[0]) / 2;
  const hh = (r[3] - r[1]) / 2;
  let t = 1;
  if (dx) t = Math.min(t, hw / Math.abs(dx));
  if (dy) t = Math.min(t, hh / Math.abs(dy));
  return [cx + dx * t, cy + dy * t];
}

function edge(A, ra, rb, width, color, alpha, dash) {
  const ctx = A.ctx;
  const ac = [(ra[0] + ra[2]) / 2, (ra[1] + ra[3]) / 2];
  const bc = [(rb[0] + rb[2]) / 2, (rb[1] + rb[3]) / 2];
  if (Math.max(ac[0], bc[0]) < -50 || Math.min(ac[0], bc[0]) > A.vw + 50) return null;
  if (Math.max(ac[1], bc[1]) < -50 || Math.min(ac[1], bc[1]) > A.vh + 50) return null;
  const s = clipTo(ac[0], ac[1], bc[0], bc[1], ra);
  const e = clipTo(bc[0], bc[1], ac[0], ac[1], rb);
  const dx = e[0] - s[0];
  const dy = e[1] - s[1];
  const len = Math.hypot(dx, dy);
  if (len < 3) return null;
  const off = Math.min(len * 0.16, 70);
  const cx = (s[0] + e[0]) / 2 - (dy / len) * off;
  const cy = (s[1] + e[1]) / 2 + (dx / len) * off;
  let ux = e[0] - cx;
  let uy = e[1] - cy;
  const ul = Math.hypot(ux, uy) || 1;
  ux /= ul;
  uy /= ul;
  const ah = len > 16 ? Math.min(4 + width * 1.3, 13) : 0;
  ctx.globalAlpha = alpha;
  ctx.strokeStyle = color;
  ctx.lineWidth = width;
  ctx.lineCap = "round";
  if (dash) ctx.setLineDash(dash);
  ctx.beginPath();
  ctx.moveTo(s[0], s[1]);
  ctx.quadraticCurveTo(cx, cy, e[0] - ux * ah * 0.7, e[1] - uy * ah * 0.7);
  ctx.stroke();
  if (dash) ctx.setLineDash([]);
  if (ah) {
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.moveTo(e[0], e[1]);
    ctx.lineTo(e[0] - ux * ah - uy * ah * 0.55, e[1] - uy * ah + ux * ah * 0.55);
    ctx.lineTo(e[0] - ux * ah + uy * ah * 0.55, e[1] - uy * ah - ux * ah * 0.55);
    ctx.closePath();
    ctx.fill();
  }
  ctx.globalAlpha = 1;
  return (t) => [
    (1 - t) * (1 - t) * s[0] + 2 * (1 - t) * t * cx + t * t * e[0],
    (1 - t) * (1 - t) * s[1] + 2 * (1 - t) * t * cy + t * t * e[1],
  ];
}

function ewidth(w, max) {
  return Math.min(max, 0.8 + 0.42 * Math.sqrt(w));
}

// What stands for module `m` at this zoom: the outermost region around it
// too small on screen to show its own modules' edges, or the module.
function repOf(A, m) {
  const k = A.t.k;
  for (const g of A.D.chain[m]) {
    const r = A.D.regions[g];
    if (Math.min(r.x1 - r.x0, r.y1 - r.y0) * k < OPEN_REGION) return r;
  }
  return A.D.modules[m];
}

// Edges summed onto what stands for their ends at this zoom, for drawing.
function aggregate(A, edges) {
  const agg = new Map();
  for (const [a, b, w] of edges) {
    const ra = repOf(A, a);
    const rb = repOf(A, b);
    if (ra === rb) continue;
    const key = `${ra.kids ? "r" : "m"}${ra.idx}:${rb.kids ? "r" : "m"}${rb.idx}`;
    const v = agg.get(key);
    if (v) v[2] += w;
    else agg.set(key, [ra, rb, w]);
  }
  return [...agg.values()].sort((x, y) => x[2] - y[2]);
}

function drawEdges(A, F, D) {
  const P = A.P;
  const here = label(A, A.si);
  if (F && (F.radius || F.commit >= 0)) {
    A.edgeNote = F.radius ? "Edges hidden while a radius is shown." : "Edges hidden while a commit is shown.";
    return;
  }
  if (F && F.selMod >= 0) {
    const selR = scr(A, A.D.modules[F.selMod]);
    const edges = F.edges.slice().sort((a, b) => a[2] - b[2]);
    for (const [a, b, w, kind] of edges) {
      const ra = a === F.selMod ? selR : scr(A, A.D.modules[a]);
      const rb = b === F.selMod ? selR : scr(A, A.D.modules[b]);
      const col = kind === CALLER ? P.caller : P.callee;
      const q = edge(A, ra, rb, ewidth(w, 9), col, 0.9, null);
      if (q) {
        const p = kind === CALLER ? q(0.18) : q(0.82);
        A.labels.push({
          pri: 1.2 + 0.3 / (1 + w),
          x: p[0],
          y: p[1],
          text: w.toLocaleString("en-US"),
          font: FONT.badge,
          size: 12,
          color: P.ink,
          pill: true,
          border: col,
          align: "center",
        });
      }
    }
    const callers = F.edges.filter((e) => e[3] === CALLER).length;
    A.edgeNote = `${callers} calling and ${F.edges.length - callers} called modules at ${here}; pills give call sites.`;
    return;
  }
  if (D !== null) {
    for (const [ra, rb, w] of aggregate(A, D.removedE)) edge(A, scr(A, ra), scr(A, rb), ewidth(w, 5), P["diff-del"], 0.8, [5, 3]);
    for (const [ra, rb, w] of aggregate(A, D.addedE)) edge(A, scr(A, ra), scr(A, rb), ewidth(w, 5), P["diff-add"], 0.8, null);
    A.edgeNote = `${D.addedE.length} module edges added and ${D.removedE.length} removed, ${label(A, D.b)} to ${label(A, D.t)}, merged into the packages open at this zoom.`;
    return;
  }
  if (A.lens !== "structure") {
    A.edgeNote = "Edges are drawn in the Structure lens, or for a selection.";
    return;
  }
  const S = snap(A, A.si);
  if (!S) {
    A.edgeNote = "";
    return;
  }
  const all = aggregate(A, S.edges);
  const shownEdges = all.slice(Math.max(0, all.length - EDGE_CAP));
  for (const [ra, rb, w] of shownEdges) edge(A, scr(A, ra), scr(A, rb), ewidth(w, 7), P.edge, 0.42, null);
  A.edgeNote =
    all.length > EDGE_CAP
      ? `The ${EDGE_CAP} heaviest of ${all.length} edges between the packages and modules open at this zoom.`
      : `${all.length} edges between the packages and modules open at this zoom.`;
}

// The hover card, drawn on the canvas beside the pointer.
function tipLines(A, hv) {
  const out = [];
  if (hv.t === "module") {
    const m = A.D.modules[hv.m];
    const r = rowAt(A, A.si, hv.m);
    out.push([m.module, true], [m.path, false]);
    if (!r) out.push([`Not in ${label(A, A.si)}.`, false]);
    else {
      out.push([`${notReported(r[1])} definitions (trellis)`, false]);
      out.push([`call sites: ${notReported(r[7])} bound out (derived), ${notReported(r[2])} ambiguous, ${notReported(r[3])} unresolved (trellis)`, false]);
      out.push([`${notReported(r[5])} bound in from ${notReported(r[6])} modules (derived)`, false]);
      const c = A.D.churn.get(hv.m);
      if (c) out.push([`commits 30/90/365 d: ${notReported(c.d30)} / ${notReported(c.d90)} / ${notReported(c.d365)} (git)`, false]);
    }
  } else if (hv.t === "symbol") {
    const s = A.D.symbols[hv.i];
    out.push([`${s.kind} ${s.qual}`, true], [`lines ${notReported(s.line)}–${notReported(s.end)}`, false]);
    out.push([`call sites: ${notReported(s.cin)} bound in, ${notReported(s.camb)} ambiguous in, ${notReported(s.cout)} bound out (trellis)`, false]);
    out.push(["Click for its radius.", false]);
  } else if (hv.t === "region") {
    const g = A.D.regions[hv.r];
    out.push([g.path, true], [regionLabel(A, g), false]);
  }
  return out;
}

function drawTip(A) {
  const ctx = A.ctx;
  const lines = tipLines(A, A.hover).map(([t, b]) => [t.length > 90 ? `${t.slice(0, 89)}…` : t, b]);
  if (lines.length === 0) return;
  const w = Math.max(...lines.map(([t, b]) => measure(A, b ? FONT.tipB : FONT.tip, t))) + 16;
  const h = lines.length * 16 + 10;
  let [x, y] = [A.tipAt[0] + 14, A.tipAt[1] + 14];
  if (x + w > A.vw - 4) x = Math.max(4, A.tipAt[0] - w - 14);
  if (y + h > A.vh - 4) y = Math.max(4, A.tipAt[1] - h - 14);
  ctx.fillStyle = A.P.panel;
  ctx.strokeStyle = A.P.line;
  ctx.lineWidth = 1;
  ctx.fillRect(x, y, w, h);
  ctx.strokeRect(x + 0.5, y + 0.5, w - 1, h - 1);
  ctx.textBaseline = "middle";
  lines.forEach(([t, b], i) => {
    ctx.font = b ? FONT.tipB : FONT.tip;
    ctx.fillStyle = b ? A.P.ink : A.P["ink-muted"];
    ctx.fillText(t, x + 8, y + 13 + i * 16);
  });
}

function drawMinimap(A) {
  const mm = byId(A, "at-minimap");
  if (!mm) return;
  const far = A.D === null || A.t.k <= fitT(A).k * 1.35;
  if (mm.hidden !== far) mm.hidden = far;
  if (far) return;
  const W = 140;
  const H = 100;
  const d = A.dpr;
  if (mm.width !== W * d) {
    mm.width = W * d;
    mm.height = H * d;
  }
  const c = mm.getContext("2d");
  const K = Math.min(W / A.D.ww, H / WORLD_H);
  const ox = (W - A.D.ww * K) / 2;
  const oy = (H - WORLD_H * K) / 2;
  c.setTransform(d, 0, 0, d, 0, 0);
  c.fillStyle = A.P.panel;
  c.fillRect(0, 0, W, H);
  for (const g of A.D.regions) {
    if (g.depth === 0 || g.depth > 2) continue;
    c.fillStyle = g.depth === 1 ? (g.test ? A.P["region-test"] : A.P.region) : g.test ? A.P["region-head-test"] : A.P["region-head"];
    c.fillRect(ox + g.x0 * K, oy + g.y0 * K, (g.x1 - g.x0) * K, (g.y1 - g.y0) * K);
  }
  if (A.F) {
    for (const [m, f] of A.F.role) {
      const l = A.D.modules[m];
      if (!l) continue;
      c.fillStyle = f & SEL ? A.P.ink : f & CALLER ? A.P.caller : f & CALLEE ? A.P.callee : f & REACH ? A.P.reach : f & DOUBT ? A.P.doubt : A.P.signal;
      c.fillRect(ox + l.x0 * K, oy + l.y0 * K, Math.max(1.5, (l.x1 - l.x0) * K), Math.max(1.5, (l.y1 - l.y0) * K));
    }
  }
  const t = A.t;
  const a = [-t.x / t.k, -t.y / t.k];
  const b = [(A.vw - t.x) / t.k, (A.vh - t.y) / t.k];
  c.strokeStyle = A.P.ink;
  c.lineWidth = 1.5;
  c.strokeRect(ox + a[0] * K, oy + a[1] * K, (b[0] - a[0]) * K, (b[1] - a[1]) * K);
}

// --- pointing ----------------------------------------------------------------

export function hitTest(A, px, py) {
  if (A.D === null) return null;
  const t = A.t;
  const wx = (px - t.x) / t.k;
  const wy = (py - t.y) / t.k;
  for (const m of A.D.modules) {
    if (wx < m.x0 || wx > m.x1 || wy < m.y0 || wy > m.y1) continue;
    const open = isHead(A) && present(A, A.si, m.idx) && (m.x1 - m.x0) * t.k >= OPEN_W && (m.y1 - m.y0) * t.k >= OPEN_H;
    if (open) {
      let best = null;
      for (const i of A.D.symsOf.get(m.idx) || []) {
        const s = A.D.symbols[i];
        if (wx >= s.x0 && wx <= s.x1 && wy >= s.y0 && wy <= s.y1) best = i;
      }
      if (best !== null) return { t: "symbol", i: best };
    }
    return { t: "module", m: m.idx };
  }
  for (let j = A.D.regions.length - 1; j >= 0; j--) {
    const g = A.D.regions[j];
    if (g.depth === 0) continue;
    if (wx >= g.x0 && wx <= g.x1 && wy >= g.y0 && wy <= g.y0 + Math.max(g.hdr, 12 / t.k)) return { t: "region", r: g.idx };
  }
  return null;
}

export function selKey(s) {
  if (!s) return "";
  return `${s.t}:${s.m ?? s.i ?? s.r ?? s.c}`;
}

