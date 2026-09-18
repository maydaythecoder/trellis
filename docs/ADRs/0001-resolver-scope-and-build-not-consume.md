# ADR 0001 — Resolver scope, and building the index rather than consuming one

**Status:** Accepted
**Date:** 2026-09-18

## Context

TRELLIS needs a call graph of first-party Python code. Existing options range
from mature type-aware engines (Pyright) to graph services (Sourcegraph) to
lighter repo maps (Aider's tree-sitter map). PyCG, the academic state of the
art, reports roughly 99.2% precision at about 69.9% recall.

## Decision

Build the index in-process with tree-sitter, scoped to **first-party names
only**. Resolve imports against the filesystem; anything landing outside the
repository root — stdlib, site-packages — is discarded at Pass 2 and never
enters the graph.

The algorithm is two passes plus a resolve step, as specified in the brief 4.3.
`trellis-index` depends on `trellis-core` and nothing else in the workspace, and
**must never depend on `trellis-model`**.

## Alternatives considered

**Pyright over LSP.** Type-aware, mature, MIT. Rejected as the *primary*
resolver: a Node process in the critical path of a tool whose selling point is
local, dependency-light operation; multi-GB memory on large repos; no batch
call-graph dump, so a full index means driving it query by query. Retained as
the S1 **fallback** for unresolved sites only — see ADR 0002.

**GitHub stack-graphs.** Rust-native, file-incremental, production-proven at
GitHub since 2021. Rejected: the repository is **archived** (verified
2026-09-18, `isArchived: true`, last push 2025-09-09), its Python ruleset was
pinned to old grammars, and it has open cross-file resolution bugs. An archived
dependency in the correctness path is not a trade worth making.

**Sourcegraph / a code-graph service.** Rejected: networked, which contradicts
a v1 non-goal outright.

**Aider-style repo map.** Rejected: it produces a ranked context summary, not a
call graph with per-edge confidence. The three-valued confidence model is the
thing the product rests on and it is not retrofittable.

## Why

Scoping to first-party names is what makes the problem tractable. We are not
analyzing Python; we are analyzing *this repository's own names*. That is a
much smaller problem than the one PyCG solves, and it should therefore produce
a better recall number on the subset we care about — which is also why a figure
*below* PyCG's would indicate the resolver underperforming its own design
rather than hitting a language limit.

The three requirements that no off-the-shelf option satisfies cheaply:
per-edge confidence, file-incremental update, and in-process with no daemon.

This is not a differentiation argument. The index is commodity (S15) and
`MISSION.md` says so.

## What changes our mind

- Measured recall below 85% on the benchmark repo (ADR 0002 thresholds) →
  adopt a backstop, decorator unwrapping first, then Pyright.
- Measured recall below 60% → the blast-radius claim dies; see ADR 0002 and
  `MISSION.md`.
- A maintained, Rust-native, file-incremental engine with per-edge confidence
  appearing → reconsider consuming rather than building. stack-graphs was the
  only candidate and it is archived.
