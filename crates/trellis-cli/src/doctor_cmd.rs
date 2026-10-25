//! `trellis doctor`, `check` and `init`: tier, batching and tooling
//! probes, and the checks run on their own. What doctor reports about a
//! run's inputs (user config, context files, trust, profiles, routines,
//! the vault) is in `doctor_extra.rs`.

use crate::floor::floor_line;
use crate::plain;
use crate::{config, doctor_extra, doctor_supply, hosted};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use trellis_model::RuntimeError;
use trellis_model::colibri::{self, Health, Identity};

/// What a constraint probe means for the run about to start.
///
/// Returns whether to rebuild the driver unconstrained, and the line to print.
/// Separated from `cmd_run` so the three outcomes are testable without a
/// runtime — which matters, because the failure this exists to prevent is a
/// confident sentence rather than a crash.
pub(crate) fn schema_report(probed: &Result<bool, String>) -> (bool, String) {
    match probed {
        Ok(true) => (false, "schema:   honoured by this runtime (probed)".into()),
        Ok(false) => (
            true,
            "schema:   sent and ignored by this runtime, so it is not enforced. Running\n\
             \x20         unconstrained; malformed calls are retried rather than prevented."
                .into(),
        ),
        // A probe that could not run leaves the tier assumed. Saying so is the
        // difference between an unverified claim and a false one — and it must
        // not silently change the tier, because a transient network error is
        // not evidence about the runtime's capabilities.
        Err(e) => (
            false,
            format!("schema:   enforcement not verified ({e}); the tier above is assumed"),
        ),
    }
}

/// The status bar's tier label for a runtime spec, from the same schema
/// probe [`schema_report`] reads, so the TUI's label, the run's and rpc's
/// `get_state` cannot disagree about it. A probe that could not run is
/// `unknown`: nothing is known, which is not the same as tier 2.
pub(crate) fn runtime_tier_label(probed: &Result<bool, String>) -> &'static str {
    match probed {
        Ok(true) => "2",
        Ok(false) => "4, no schema",
        Err(_) => "unknown",
    }
}

/// The module graph `codebase_json` draws: built in memory, never reading
/// or writing `.trellis/index.bin`. The index is built rather than read from
/// the cache because an interface asks for this while a repository is being
/// edited, and a cached graph would describe the tree as it stood before the
/// run the user has just watched.
fn codebase_graph(root: &Path) -> trellis_index::ModuleGraph {
    let sources = trellis_index::read_sources(root);
    let index = trellis_index::build_from_sources(&sources);
    trellis_index::module_graph(&index)
}

/// How many of a module's edges [`graph_json`] lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Edges {
    /// The heaviest [`MAX_EDGES_DRAWN`](crate::index_cmd::MAX_EDGES_DRAWN),
    /// as the drawing shows them.
    Drawn,
    /// Every one, for a client that draws the whole graph itself (the
    /// desktop's Atlas, owner request of 2026-10-11).
    All,
}

/// The codebase graph for `root` as data, for rpc's `get_codebase` at a
/// commit, whose tree is copied out and never cached. The worktree's answer
/// builds its index itself, since it saves it (`atlas::codebase`, #654).
///
/// What `graph_lines` draws for the same `limit`, field for field: the
/// same modules, busiest first, and the same edges per module, with what was
/// left out counted rather than dropped. Bound, ambiguous and unresolved stay
/// separate counts with no denominator beside them (CLAUDE.md rule 3).
/// Module names are whole: cutting them to a column is the drawing's.
pub(crate) fn codebase_json(root: &Path, limit: usize, edges: Edges) -> serde_json::Value {
    graph_json(&codebase_graph(root), limit, edges)
}

/// `codebase_json`'s shape for a graph already in hand: the overview's,
/// from a cached index it must not rebuild (#558).
pub(crate) fn graph_json(
    g: &trellis_index::ModuleGraph,
    limit: usize,
    edges: Edges,
) -> serde_json::Value {
    let shown = match edges {
        Edges::Drawn => crate::index_cmd::MAX_EDGES_DRAWN,
        Edges::All => usize::MAX,
    };
    let modules: Vec<serde_json::Value> = g
        .nodes
        .iter()
        .take(limit)
        .map(|n| {
            let calls: Vec<serde_json::Value> = n
                .calls
                .iter()
                .take(shown)
                .map(|(m, c)| serde_json::json!({"module": m, "call_sites": c}))
                .collect();
            serde_json::json!({
                "module": n.module,
                // Repository-relative and `/`-separated, so a client opens
                // the file without guessing it from the dotted name, which
                // a package's `__init__.py` does not spell (#559).
                "path": n.path,
                "defines": n.defines,
                "calls": calls,
                "calls_not_drawn": n.calls.len().saturating_sub(shown),
                "ambiguous": n.ambiguous,
                "unresolved": n.unresolved,
            })
        })
        .collect();
    serde_json::json!({
        "modules_indexed": g.nodes.len(),
        "bound_call_sites": g.bound_edges(),
        "ambiguous": g.ambiguous(),
        "unresolved": g.unresolved(),
        "modules": modules,
        "modules_not_drawn": g.nodes.len().saturating_sub(limit),
    })
}

/// Cuts a module path to the column, keeping the end.
///
/// The tail is what distinguishes `poetry.console.commands.add` from
/// `poetry.console.commands.remove`; the head is shared by hundreds of them.
pub(crate) fn truncate_module(m: &str, width: usize) -> String {
    let n = m.chars().count();
    if n <= width {
        return m.to_string();
    }
    let keep = width.saturating_sub(1);
    std::iter::once('…')
        .chain(m.chars().skip(n - keep))
        .collect()
}

/// What the backend `spec` names says about running requests at once (S18).
///
/// One probe for `doctor`, which reports it, and `trellis run --agents
/// auto`, which takes its width from it: two copies would let the width a
/// run picks drift from the capability `doctor` shows. A hosted provider
/// batches by design and reports no number; a scripted model has no backend
/// at all, so its capability is unknown. An `Err` is a slot count that could
/// not be read, which says nothing about the runtime, so callers treat it as
/// `Unknown` and say why.
///
/// A hosted spec on this machine may be colibri (ADR 0047 decision 3), whose
/// `/health` reports its KV slots; that count is the capability here, as in
/// `doctor`, through [`hosted_batching`].
/// How long each request `doctor` sends a serving runtime may take: the
/// constraint probe and the slot read. One constant, so the line said
/// before the wait and the wait agree.
const RUNTIME_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Says on stderr what `doctor` is about to wait on, and for how long at
/// most (#650).
///
/// Every probe runs before the report prints, so a runtime that does not
/// answer was up to half a minute of blank terminal, at the moment a person
/// is diagnosing it. Stderr, so stdout stays the report. Said whether or not
/// stderr is a terminal: a captured log of a slow `doctor` should say what
/// it waited on too.
fn say_waiting(what: &str, bound: std::time::Duration) {
    eprintln!("{what} (up to {} s)", bound.as_secs());
}

pub(crate) fn probe_batching(
    spec: &trellis_model::ModelSpec,
    cfg: &config::RepoConfig,
    from_user: bool,
) -> Result<trellis_model::BatchingCapability> {
    use trellis_model::BatchingCapability;

    if let Some(r) = hosted::resolve(spec, cfg, from_user)? {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        return hosted_batching(&rt.block_on(read_colibri(&r))).map_err(anyhow::Error::msg);
    }
    let trellis_model::ModelSpec::Runtime {
        dialect,
        base,
        model,
    } = spec
    else {
        return Ok(BatchingCapability::Unknown);
    };
    let client = trellis_model::RuntimeClient::new(base, *dialect, model.clone())?
        .with_timeout(RUNTIME_PROBE_TIMEOUT);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    Ok(match rt.block_on(client.probe_slots())? {
        Some(n) if n > 1 => BatchingCapability::Continuous {
            measured_concurrency: n,
        },
        Some(_) => BatchingCapability::SingleStream,
        // Continuous batching is inherent here, but the concurrency its
        // scheduler will actually grant is not a number to invent.
        None => BatchingCapability::ContinuousUnmeasured,
    })
}

/// What a hosted endpoint said when asked whether it is colibri (ADR 0047
/// decision 3), and colibri's `/health` when it is.
struct ColibriSeen {
    identity: Identity,
    health: Option<Result<Health, RuntimeError>>,
}

/// Reads `r`'s `/v1/models`, then `/health` when the server is colibri,
/// through `trellis_model::colibri` (each bounded by its `READ_TIMEOUT`).
///
/// Both carry the key `hosted::resolve` gave `r`, read the way the tier-1
/// client reads it, and so none when its variable is unset: identifying
/// colibri never changes where a key goes (ADR 0024 §3). A base that is not
/// loopback is answered `NotLoopback` without a connection.
async fn read_colibri(r: &hosted::Resolved) -> Result<ColibriSeen, RuntimeError> {
    let key = hosted::colibri_key(r);
    let identity = colibri::identify(&r.base, key.as_ref()).await?;
    let health = match identity {
        Identity::Identified { .. } => Some(colibri::health(&r.base, key.as_ref()).await),
        _ => None,
    };
    Ok(ColibriSeen { identity, health })
}

/// A hosted endpoint's batching from [`read_colibri`], for `doctor` and for
/// `probe_batching` alike, so the line `doctor` prints and the width `trellis
/// run --agents` takes agree. A provider that is not colibri batches by
/// design and reports no number. An `Err` is a read that failed, already
/// passed through [`plain`]: the text is the server's or about it.
fn hosted_batching(
    seen: &Result<ColibriSeen, RuntimeError>,
) -> Result<trellis_model::BatchingCapability, String> {
    use trellis_model::BatchingCapability;
    match seen {
        Ok(ColibriSeen {
            health: Some(Ok(h)),
            ..
        }) => Ok(colibri_batching(h)),
        Ok(ColibriSeen { health: None, .. }) => Ok(BatchingCapability::ContinuousUnmeasured),
        Ok(ColibriSeen {
            health: Some(Err(e)),
            ..
        })
        | Err(e) => Err(plain(&e.to_string())),
    }
}

/// colibri's batching from its `kv_slots` (ADR 0047 decision 7): continuous
/// with that count above one, single-stream at one. The count is colibri's
/// configured slots, not a measurement of what they sustain; `doctor` says
/// so. No `kv_slots` (a liveness-only body) is not knowledge of one slot.
fn colibri_batching(h: &Health) -> trellis_model::BatchingCapability {
    use trellis_model::BatchingCapability;
    match h.kv_slots {
        Some(n) if n > 1 => BatchingCapability::Continuous {
            measured_concurrency: n,
        },
        Some(_) => BatchingCapability::SingleStream,
        None => BatchingCapability::Unknown,
    }
}

/// `doctor`'s runtime line for an identified colibri: the served id, and
/// whether the spec names it. colibri answers 404 `model_not_found` to any
/// request naming another id (ADR 0047 finding 3).
fn colibri_runtime(base: &str, spec_model: &str, served: &str) -> String {
    // Compared as sent, escaped only for display: an escaped id must not
    // match one holding the byte it spells.
    let matches = served == spec_model;
    let (served, spec_model) = (plain(served), plain(spec_model));
    // A `[[providers]] base_url` is the repository's text too (#499).
    let base = plain(base);
    if matches {
        format!("colibri at {base}, serving `{served}`; the spec's model matches it")
    } else {
        format!(
            "colibri at {base}, serving `{served}`; the spec's `{spec_model}` does NOT match \
             it, and colibri answers 404 to a request naming any other id"
        )
    }
}

/// The `context:` line for an identified colibri: the budget a run holds
/// its prompt to (ADR 0047 decision 6), since colibri does not say how
/// large its window is and the user is the one who knows its `--ctx`.
fn colibri_context_line(budget: usize) -> String {
    let source = if budget == hosted::COLIBRI_CONTEXT_BYTES {
        format!(
            "{},{:03} bytes, which fits a window of 8,192 tokens with 1,024 left for the reply \
             at 3.5 bytes per token",
            budget / 1000,
            budget % 1000
        )
    } else {
        format!("{budget} bytes, from [model] context_bytes or the model's input_bytes")
    };
    format!(
        "context:    colibri does not report its window; runs use {source}. \
         Set [model] context_bytes to fit the --ctx colibri was started with"
    )
}

/// `doctor`'s lines for the model card colibri 2.0.0 puts on `/v1/models`
/// (#626): what the run's preflight refuses on. None for a body without
/// the lists, which decides nothing. Every entry is the server's, cut and
/// passed through [`plain`].
fn colibri_card_lines(card: &trellis_model::colibri::Card) -> Vec<String> {
    let show = |list: &[String]| -> String {
        list.iter()
            .take(8)
            .map(|s| plain(&s.chars().take(40).collect::<String>()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut out = Vec::new();
    if let Some(c) = &card.capabilities {
        out.push(format!(
            "card:       capabilities {}; {}; as reported by colibri",
            show(c),
            if card.chats() == Some(true) {
                "a run can chat with it"
            } else {
                "it does not chat, so `trellis run` refuses it before the snapshot"
            }
        ));
    }
    if let Some(m) = &card.input_modalities {
        out.push(format!(
            "card:       input modalities {}; {}; as reported by colibri",
            show(m),
            if card.takes_images() == Some(true) {
                "--image is accepted"
            } else {
                "--image is refused before the snapshot"
            }
        ));
    }
    out
}

/// `doctor`'s lines for what colibri's `/health` reported, each marked as
/// colibri's own report (ADR 0047 decision 7). Every string is the
/// server's, so each goes through [`plain`].
fn colibri_health_lines(h: &Health) -> Vec<String> {
    if h.liveness_only() {
        return vec![
            "health:     liveness only. colibri shows its KV slots, memory tiers and host \
             only to an authenticated request."
                .to_string(),
        ];
    }
    let mut out = Vec::new();
    out.push(match &h.tiers {
        Some(t) => format!(
            "experts:    {} in VRAM ({:.1} GB resident), {} in RAM ({:.1} GB resident), {} on \
             disk; as reported by colibri",
            t.vram, t.vram_gb, t.ram, t.ram_gb, t.disk
        ),
        None => "experts:    colibri has not reported its memory tiers yet".to_string(),
    });
    out.push(match &h.hwinfo {
        Some(hw) => format!(
            "host:       {} cores, {:.1} GB RAM ({:.1} GB available), CPU {}; {} GPU(s), {}, \
             {:.1} GB VRAM; as reported by colibri",
            hw.cores,
            hw.ram_total_gb,
            hw.ram_avail_gb,
            plain(&hw.cpu),
            hw.gpus,
            plain(&hw.gpu),
            hw.vram_total_gb
        ),
        None => "host:       colibri has not reported its host yet".to_string(),
    });
    out
}

/// `doctor`'s `placement:` lines from Ollama's `/api/ps` body (#633), the
/// Ollama counterpart of ADR 0047 decision 7: what Ollama reports, labelled
/// as its report. `None` when the body is not a `/api/ps` answer, so a
/// server that is not Ollama adds no line.
///
/// A model split between VRAM and RAM runs several times slower than one
/// resident in VRAM, and nothing else in doctor says which it is. The two
/// byte counts are printed as Ollama gives them, and the one comparison made
/// is between them; no fit score, since that would be a figure without a
/// method. With `model` named, only that model's entry is reported; an
/// entry's strings come off the network and go through [`plain`].
fn ollama_placement_lines(body: &str, model: Option<&str>) -> Option<Vec<String>> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let models = v.get("models")?.as_array()?;
    if models.is_empty() {
        return Some(vec![
            "placement:  not loaded; placement unknown until a run".to_string(),
        ]);
    }
    let name_of = |m: &serde_json::Value| -> String {
        m.get("name")
            .or_else(|| m.get("model"))
            .and_then(|n| n.as_str())
            .map_or_else(
                || "(unnamed)".to_string(),
                |n| plain(&n.chars().take(256).collect::<String>()),
            )
    };
    let chosen: Vec<&serde_json::Value> = match model {
        Some(wanted) => models
            .iter()
            .filter(|m| {
                ["name", "model"]
                    .iter()
                    .any(|f| m.get(f).and_then(|n| n.as_str()) == Some(wanted))
            })
            .collect(),
        None => models.iter().collect(),
    };
    if chosen.is_empty() {
        let loaded: Vec<String> = models.iter().map(name_of).collect();
        return Some(vec![format!(
            "placement:  {} is not loaded (ollama has {} loaded); placement unknown until a run",
            plain(model.unwrap_or("")),
            loaded.join(", ")
        )]);
    }
    Some(
        chosen
            .into_iter()
            .map(|m| {
                let quant = m
                    .get("details")
                    .and_then(|d| d.get("quantization_level"))
                    .and_then(|q| q.as_str())
                    .map_or_else(
                        || "quantization not reported".to_string(),
                        |q| plain(&q.chars().take(64).collect::<String>()),
                    );
                let size = m.get("size").and_then(|s| s.as_u64());
                let vram = m.get("size_vram").and_then(|s| s.as_u64());
                let bytes = match (size, vram) {
                    (Some(s), Some(v)) => {
                        let where_ = if v >= s {
                            "all of it in VRAM"
                        } else if v == 0 {
                            "none of it in VRAM"
                        } else {
                            "split between VRAM and RAM"
                        };
                        format!("{s} bytes, {v} bytes in VRAM ({where_})")
                    }
                    (Some(s), None) => format!("{s} bytes, bytes in VRAM not reported"),
                    (None, Some(v)) => format!("size not reported, {v} bytes in VRAM"),
                    (None, None) => "size and bytes in VRAM not reported".to_string(),
                };
                format!(
                    "placement:  {}: {quant}, {bytes}; as reported by ollama",
                    name_of(m)
                )
            })
            .collect(),
    )
}

/// `doctor`'s `tier:` line for an identified colibri, from the tools probe
/// the run and the TUI's status bar read too (ADR 0047 decisions 4 and 7),
/// and a note when tier 1 is unavailable. Every string the server supplied
/// is passed through [`plain`].
fn colibri_tier_report(tier: &Result<hosted::ColibriTier>) -> (String, Option<String>) {
    use trellis_model::AdapterTier;
    let native = AdapterTier::Native.degradation();
    match tier {
        Ok(hosted::ColibriTier::Probed { .. }) => {
            (format!("tier:       1, Native (probed) — {native}"), None)
        }
        Ok(hosted::ColibriTier::Assumed { reason, .. }) => (
            format!(
                "tier:       1, Native (assumed: {}) — {native}",
                plain(reason)
            ),
            None,
        ),
        Ok(hosted::ColibriTier::Refused {
            id,
            origin,
            fallback_named,
        }) => {
            let (id, origin) = (plain(id), plain(origin));
            (
                format!(
                    "tier:       1, unavailable: colibri refuses tools for `{id}` (HTTP 400, \
                     param `tools`); tier 4 is `vllm:{origin}#{id}`"
                ),
                Some(format!(
                    "colibri refused the tools probe for `{id}`: this model's chat template \
                     cannot render tools, so a tier-1 run against it is refused before its \
                     snapshot (ADR 0047 decision 4).{} Tier 4 runs without tools or a chat \
                     template, the reply validated after: --model vllm:{origin}#{id}",
                    hosted::fallback_sentence(*fallback_named)
                )),
            )
        }
        // Called for an identified server only, so not reached; the
        // protocol's own line if it were.
        Ok(hosted::ColibriTier::NotColibri) => (format!("tier:       1, Native — {native}"), None),
        // A probe that could not be sent leaves tier 1 assumed, as its other
        // non-answers do.
        Err(e) => (
            format!(
                "tier:       1, Native (assumed: {}) — {native}",
                plain(&e.to_string())
            ),
            None,
        ),
    }
}

/// `doctor`'s `tier:` line when `--model` was given and the constraint probe
/// got no answer. `Probe::report`'s unprobed line says "pass --model", which
/// sends a person who already did back to that step (#526). An unreachable
/// runtime is named with its endpoint, passed through [`plain`] because it
/// can come from repository config, and the likely fix; any other failure
/// points at the note that carries it.
fn unprobed_tier_line(e: &RuntimeError) -> String {
    match e {
        RuntimeError::Connect { base, .. } => format!(
            "tier:       unknown, the probe could not reach the runtime at {}; start it, or \
             check the port",
            plain(base)
        ),
        _ => "tier:       unknown, the probe did not complete; see the note below".to_string(),
    }
}

/// The note for a `vllm:` spec on a server identified as colibri.
fn colibri_vllm_note(base: &str, served: &str) -> String {
    format!(
        concat!(
            "This server is colibri, serving `{served}`. colibri's grammar never constrains ",
            "sampling, so the schema is not enforced and the tier is ValidateAndRetry without ",
            "a probe (ADR 0047). Its `/v1/completions`, which this spec uses, receives the ",
            "prompt without a chat template. The tier-1 spec for this server is ",
            "`openai-chat:{base}/v1#{served}`."
        ),
        served = plain(served),
        base = base.trim_end_matches('/')
    )
}

/// What `doctor` adds about a serving runtime's batching capability.
fn batching_note(b: trellis_model::BatchingCapability) -> Option<&'static str> {
    use trellis_model::BatchingCapability;
    match b {
        BatchingCapability::SingleStream => Some(concat!(
            "llama.cpp reports a single slot. Multi-agent mode would serialize; ",
            "start it with --parallel N for real concurrency (S18). "
        )),
        BatchingCapability::ContinuousUnmeasured => Some(concat!(
            "This runtime batches continuously, so multi-agent mode is supported ",
            "in principle. The concurrency it will grant is its scheduler's ",
            "decision and is not reported, so `--agents` runs one agent, ",
            "whatever N it is given. "
        )),
        BatchingCapability::Continuous { .. } | BatchingCapability::Unknown => None,
    }
}

pub(crate) fn cmd_doctor(root: &Path, model_spec: Option<&str>, from_user: bool) -> Result<()> {
    use trellis_model::{AdapterTier, BatchingCapability, Probe};

    let repo_cfg = config::RepoConfig::load(root)?;
    let cfg = repo_cfg.checks();

    // Unknown until measured. Reporting an optimistic guess is the failure
    // S17 and S18 both describe: the user cannot tell tier 2 from tier 4, or
    // a batching runtime from one that will serialize their agents.
    let mut model_id = "not configured".to_string();
    let mut runtime = "not configured".to_string();
    let mut tier: Option<AdapterTier> = None;
    let mut batching = BatchingCapability::Unknown;
    let mut notes: Vec<String> = Vec::new();
    // What colibri reported, printed after the probe's own lines.
    let mut colibri_lines: Vec<String> = Vec::new();
    // What Ollama reported about where the model sits (#633), printed
    // there too; empty when the endpoint did not answer `/api/ps`.
    let mut placement_lines: Vec<String> = Vec::new();
    // A hosted endpoint's batching, from the same read as its colibri lines.
    let mut hosted_seen: Option<Result<BatchingCapability, String>> = None;
    // A `tier:` line in place of the one `Probe::report` would give: colibri's
    // tier, from the tools probe, rather than the protocol's alone (ADR 0047
    // decision 4); or why a probe that `--model` asked for got no answer,
    // where the report's own line would tell the person to pass `--model`
    // (#526).
    let mut tier_line: Option<String> = None;

    if let Some(spec) = model_spec {
        let spec: trellis_model::ModelSpec = spec.parse()?;
        // Tier 1 is the protocol, not a probe finding (ADR 0024 §2).
        let resolved = hosted::resolve(&spec, &repo_cfg, from_user)?;
        if let Some(h) = resolved {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let seen = rt.block_on(read_colibri(&h));
            let read = !matches!(
                seen,
                Ok(ColibriSeen {
                    identity: Identity::NotLoopback,
                    ..
                })
            );
            let (m, r, n) = hosted::doctor(&h, read);
            model_id = m;
            runtime = r;
            notes.extend(n);
            tier = Some(AdapterTier::Native);
            match &seen {
                Ok(ColibriSeen {
                    identity: Identity::Identified { id, card },
                    health,
                }) => {
                    runtime = colibri_runtime(&h.base, &h.model, id);
                    colibri_lines.push("version:    not reported over HTTP".to_string());
                    colibri_lines.extend(colibri_card_lines(card));
                    colibri_lines.push(colibri_context_line(hosted::hosted_context_bytes(
                        &h, &repo_cfg, true,
                    )?));
                    match health {
                        Some(Ok(hl)) => {
                            colibri_lines.extend(colibri_health_lines(hl));
                            if let Some(n) = hl.kv_slots.filter(|&n| n > 1) {
                                notes.push(format!(
                                    "batching: the {n} above is colibri's configured KV slot \
                                     count (`kv_slots` on its /health), as colibri reports it; \
                                     it was not measured under load."
                                ));
                            }
                        }
                        Some(Err(e)) => notes.push(format!(
                            "colibri's /health could not be read: {}",
                            plain(&e.to_string())
                        )),
                        None => {}
                    }
                    // The served id, which is what colibri answers for; a
                    // spec naming another is reported on the runtime line.
                    let (line, note) =
                        colibri_tier_report(&rt.block_on(hosted::probed_tier(&h, id.clone())));
                    tier_line = Some(line);
                    notes.extend(note);
                }
                Ok(ColibriSeen {
                    identity: Identity::NotColibri { owned_by },
                    ..
                }) => notes.push(format!(
                    "This endpoint's /v1/models is owned by `{}`, so it is not colibri.",
                    plain(owned_by)
                )),
                Ok(ColibriSeen {
                    identity: Identity::Unauthorized { status },
                    ..
                }) => notes.push(format!(
                    "This endpoint's /v1/models answered HTTP {status} to {}, so whether it is \
                     colibri is not known: colibri started with a key lists its model only to \
                     an authenticated request.",
                    if hosted::colibri_key(&h).is_some() {
                        format!("a request sent with the key from {}", h.key_env)
                    } else {
                        format!("a request without a key ({} is not set)", h.key_env)
                    }
                )),
                Ok(ColibriSeen {
                    identity: Identity::Absent { status },
                    ..
                }) => notes.push(format!(
                    "This endpoint has no /v1/models ({status}), so it is not colibri."
                )),
                Ok(ColibriSeen {
                    identity: Identity::NotLoopback,
                    ..
                }) => {}
                Err(e) => notes.push(format!(
                    "Could not ask this endpoint whether it is colibri: {}",
                    plain(&e.to_string())
                )),
            }
            hosted_seen = Some(hosted_batching(&seen));
        } else {
            let trellis_model::ModelSpec::Runtime {
                dialect,
                base,
                model,
            } = &spec
            else {
                anyhow::bail!(
                    concat!(
                        "`{}` is a scripted model, which has no runtime to probe. ",
                        "Pass a serving runtime, e.g. llamacpp:http://127.0.0.1:8080."
                    ),
                    spec
                );
            };
            let client = trellis_model::RuntimeClient::new(base, *dialect, model.clone())?
                .with_timeout(RUNTIME_PROBE_TIMEOUT);
            runtime = format!("{dialect:?} at {base}");
            // The spec can come from repository config, so it is shown plain.
            let probed = plain(&spec.to_string());
            // True of llama.cpp, which serves what it was started with and is
            // never told. Not true once the spec names a weight — saying it
            // anyway would report ignorance the harness does not have, on the
            // one line a reader checks to confirm they are about to drive what
            // they think they are.
            model_id = match model {
                Some(m) => format!("{m} (named in the spec)"),
                None => "whatever the runtime has loaded".into(),
            };

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            rt.block_on(async {
                // colibri ignores the schema by construction (ADR 0047
                // finding 1), so a probe that a capable model passed
                // unprompted would claim a tier it does not have. A `vllm:`
                // spec carries no key (it is not hosted), and so neither
                // does this read.
                if *dialect == trellis_model::Dialect::VllmOpenAi {
                    say_waiting(
                        &format!("asking {probed} whether it is colibri"),
                        colibri::READ_TIMEOUT,
                    );
                    if let Ok(Identity::Identified { id, .. }) = colibri::identify(base, None).await
                    {
                        tier = Some(AdapterTier::ValidateAndRetry);
                        notes.push(colibri_vllm_note(base, &id));
                        return;
                    }
                }
                say_waiting(&format!("probing {probed}"), RUNTIME_PROBE_TIMEOUT);
                match client.probe_constraint().await {
                    Ok(true) => {
                        tier = Some(AdapterTier::RuntimeConstrained);
                        notes.push(
                            concat!(
                                "The runtime returned the key the probe schema demanded, so it ",
                                "appears to honour guided decoding. This is evidence, not proof: ",
                                "a capable model can produce conforming JSON unprompted."
                            )
                            .into(),
                        );
                    }
                    Ok(false) => {
                        tier = Some(AdapterTier::ValidateAndRetry);
                        notes.push(
                        concat!(
                            "The runtime ignored the schema, so output is NOT constrained. Runs ",
                            "will retry on malformed tool calls and can fail on format alone. ",
                            "Pass --unconstrained to `trellis run` to make that explicit. "
                        )
                        .into(),
                    );
                    }
                    Err(e) => {
                        tier_line = Some(unprobed_tier_line(&e));
                        // `e` names the endpoint, which can come from
                        // repository config.
                        notes.push(format!(
                            "Could not probe the runtime: {}",
                            plain(&e.to_string())
                        ));
                    }
                }
            });
            // Ollama is reached as a `vllm:` spec (the picker writes one, ADR
            // 0050 §2), so that dialect on this machine is asked `/api/ps`.
            // No answer, or an answer in another shape, is another server
            // and adds nothing; the lines are Ollama's own (#633).
            if *dialect == trellis_model::Dialect::VllmOpenAi
                && let Ok(body) = crate::first_run::ollama_ps(base, crate::first_run::BOUND)
                && let Some(lines) = ollama_placement_lines(&body, model.as_deref())
            {
                placement_lines = lines;
            }
        }

        // After the constraint probe, so its note still reads first. A hosted
        // provider batches by design and says nothing more, so its
        // `ContinuousUnmeasured` carries no note: there is no runtime to
        // restart differently. A hosted endpoint's batching comes from the
        // read above, through the mapping `probe_batching` uses too, so the
        // endpoint is not read twice; a failed read is already a note.
        // Only llama.cpp is asked: vLLM and SGLang report no slot count.
        if hosted_seen.is_none()
            && let trellis_model::ModelSpec::Runtime {
                dialect: trellis_model::Dialect::LlamaCpp,
                base,
                ..
            } = &spec
        {
            say_waiting(
                &format!("reading the slot count from {}", plain(base)),
                RUNTIME_PROBE_TIMEOUT,
            );
        }
        match hosted_seen {
            Some(b) => batching = b.unwrap_or(BatchingCapability::Unknown),
            None => match probe_batching(&spec, &repo_cfg, from_user) {
                Ok(b) => {
                    batching = b;
                    notes.extend(batching_note(b).map(str::to_string));
                }
                Err(e) => notes.push(format!("Could not read slot count: {e}")),
            },
        }
    }

    let probe = Probe {
        model_id,
        runtime,
        tier,
        batching,
        typecheck: version_of(&cfg.typecheck),
        tests: version_of(&cfg.tests),
        coverage_available: coverage_available(&cfg.tests),
    };

    // colibri's lines go under the model's, before the checkers'.
    let mut lines = probe.report();
    let at = lines
        .iter()
        .position(|l| l.starts_with("batching:"))
        .map_or(lines.len(), |i| i + 1);
    lines.splice(at..at, colibri_lines.into_iter().chain(placement_lines));
    if let Some(t) = tier_line
        && let Some(l) = lines.iter_mut().find(|l| l.starts_with("tier:"))
    {
        *l = t;
    }
    // `Probe::report` can only say "no coverage plugin", which would claim
    // an answer to a question that was never asked (#572).
    if !is_pytest(&cfg.tests)
        && let Some(v) = &probe.tests
        && let Some(l) = lines.iter_mut().find(|l| l.starts_with("tests:"))
    {
        *l = format!("tests:      {v}; coverage: not probed (the tests command is not pytest)");
    }
    for line in lines {
        println!("{line}");
    }
    println!("{}", floor_line(model_spec.is_some()));
    for line in repo_lines(root, &repo_cfg) {
        println!("{line}");
    }
    // Which binary confines the clone's command, or why none does (ADR
    // 0055): a sandbox that is missing or fails is a warning, so this is
    // where a person finds out before a run does.
    println!("{}", trellis_loop::OsSandbox::detected().doctor_line());
    println!("root:       {}", root.display());

    if model_spec.is_none() {
        println!();
        println!(concat!(
            "No --model given, so tier and batching are reported as unknown ",
            "rather than guessed.
",
            "Pass e.g. `--model llamacpp:http://127.0.0.1:8080` to probe."
        ));
    }
    for n in &notes {
        println!();
        println!("{n}");
    }
    Ok(())
}

/// Scaffolds `.trellis/` and reports what is missing before it bites.
///
/// `04-v1.0-harness.md`, task item 7. Two jobs in one command, and the second
/// is the one that matters: writing three files is easy, and a user who
/// discovers on their first run that no type checker is installed has already
/// formed an opinion about the tool.
///
/// **Nothing here is overwritten.** These files live in the user's
/// repository, and a scaffolder that clobbers a config someone tuned is worse
/// than one that refuses to run at all.
pub(crate) fn cmd_init(root: &Path) -> Result<()> {
    if !root.join(".git").exists() {
        anyhow::bail!(concat!(
            "this is not a git repository, and TRELLIS rolls back with git.\n",
            "  git init && git add -A && git commit -m 'initial'\n",
            "then run `trellis init` again."
        ));
    }

    let dir = root.join(".trellis");
    let files = [
        ("config.toml", CONFIG_TEMPLATE),
        ("context.md", CONTEXT_TEMPLATE),
        ("governance/README.md", GOVERNANCE_TEMPLATE),
    ];
    // A repository can commit `.trellis`, `.trellis/governance` or any of
    // these files as a link, and a dangling one reads as absent to
    // `exists()`, so the template was written at the link's target outside
    // the repository (#723). Every entry is checked before anything is
    // created, so a refusal leaves the disk as it was.
    for (rel, _) in &files {
        trellis_core::links::refuse_links_below(
            root,
            &Path::new(".trellis").join(rel),
            "`trellis init`'s scaffold",
        )?;
    }
    let mut wrote = Vec::new();
    let mut kept = Vec::new();
    for (rel, body) in files {
        let path = dir.join(rel);
        // `symlink_metadata`, not `exists()`, for the reason above.
        if std::fs::symlink_metadata(&path).is_ok() {
            kept.push(rel);
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        // `create_new`: a link planted after the check above is not
        // followed, and an existing entry is never overwritten.
        trellis_core::links::create_new(&path, body.as_bytes())
            .with_context(|| format!("writing {}", path.display()))?;
        wrote.push(rel);
    }

    for f in &wrote {
        println!("created  .trellis/{f}");
    }
    for f in &kept {
        println!("kept     .trellis/{f} (already present, not overwritten)");
    }
    if wrote.is_empty() {
        println!("nothing to create; `.trellis/` is already set up.");
    }

    println!();
    println!("Prerequisites");
    let cfg = config::RepoConfig::load(root)?.checks();
    let mut blocking = 0;

    match version_of(&cfg.typecheck) {
        Some(v) => println!("  typecheck  {v}"),
        None => {
            blocking += 1;
            println!("  typecheck  NOT FOUND: {}", cfg.typecheck.join(" "));
            println!("             No typecheck verdict can be produced, so no run can");
            println!("             converge. Install it, or set [checks] typecheck in");
            println!("             .trellis/config.toml to the command you already use.");
        }
    }
    match version_of(&cfg.tests) {
        Some(v) => println!("  tests      {v}"),
        None => {
            blocking += 1;
            println!("  tests      NOT FOUND: {}", cfg.tests.join(" "));
            println!("             The grounding contract has nothing to ground on. A");
            println!("             repository without a test suite is a stated v1.0");
            println!("             non-goal, not an oversight.");
        }
    }

    match trellis_vcs::pending(root) {
        Ok(p) if p.is_empty() => println!("  worktree   clean"),
        Ok(p) => {
            // Not counted as blocking: `init` itself just wrote files, so a
            // dirty tree here is usually this command's own doing and saying
            // "blocked" would be alarming and wrong.
            println!("  worktree   {} uncommitted change(s)", p.len());
            println!("             A run needs a clean tree so rollback is exact. Commit");
            println!("             what you want to keep — including .trellis/ — first.");
        }
        Err(e) => println!("  worktree   cannot be read: {e}"),
    }

    println!();
    if blocking > 0 {
        println!("{blocking} prerequisite(s) missing. `trellis doctor` re-checks at any time.");
    } else {
        println!("Ready. `trellis doctor` re-checks at any time, and `trellis` opens a session.");
    }
    Ok(())
}

/// The scaffolded config.
///
/// Every value is commented out and shows the default it would replace, so
/// the file documents what TRELLIS will do rather than changing it. A
/// scaffold that writes active settings makes the tool's behaviour depend on
/// a file the user never read.
const CONFIG_TEMPLATE: &str = r#"# TRELLIS configuration. Every setting below is commented out and shows the
# default it would override, so this file starts by changing nothing.
#
# This file belongs in version control. `.trellis/index.bin` does not, and
# TRELLIS writes a .gitignore next to it saying so.

# [checks]
# What TRELLIS runs to decide whether a change is correct. These are the only
# source of a passing verdict: the model never issues one.
#
# `--no-incremental` on mypy is a correctness flag, not a preference. mypy
# keys its cache on file size and mtime, and a rollback restores a previous
# version of the same byte length — so without it mypy can report a cached
# result for code it never read.
# typecheck = ["mypy", "--no-incremental"]
# tests     = ["pytest", "-q"]
# python    = "python"

# [budget]
# A run stops when any of these is reached, and rolls back.
# max_iterations   = 40
# max_wall_clock_s = 1200
# max_tokens       = 2000000
# regime           = "local"   # or "metered", when tokens cost money

# [model]
# Saves retyping a runtime address every session. Deliberately not a place for
# a key: this is a host and port, or a path to a script. Your API key never
# goes in a file TRELLIS reads — configure the runtime with it directly.
# spec          = "llamacpp:http://127.0.0.1:8080"
# unconstrained = false

# [governance]
# Paths no run may edit, enforced at the orchestrator's write path rather than
# asked of the model. A rule that lives only in a prompt is a suggestion.
# deny_write = ["vendor/", "*.lock", "migrations/"]

# [context]
# A note vault in Apex format (ADR 0017). One focus note, its parent chain,
# its children's titles and the notes it links, compiled in memory when a run
# starts and shown to the model. It informs; it does not bind. Switch note
# per run with `trellis run --focus <stem>` rather than by editing this file,
# which would dirty the worktree a run needs clean.
# vault = "notes"
# focus = "some-note"
"#;

/// The starter context file.
///
/// Written as questions rather than as a template to fill in. An empty
/// headed outline gets committed empty; a question either gets answered or
/// gets deleted, and both are better than a heading with nothing under it.
const CONTEXT_TEMPLATE: &str = r#"# Project context

Notes for whatever is driving the loop. This file is read verbatim and put in
front of the model at the start of every run, so it is worth keeping short:
everything here costs context window on every single turn.

It **informs**; it does not bind. Nothing written here can relax the grounding
contract — a note saying "the tests are slow, just declare success" changes
nothing, because convergence reads an executed check. Rules that must bind go
in `[governance] deny_write` or `.trellis/governance/`, which the orchestrator
enforces at the write path.

Worth answering, if the answer is not obvious from the code:

- What is this project for, in two sentences?
- Which directories are the important ones, and which are generated?
- Any convention a newcomer gets wrong on their first change?
- Anything that looks like a bug and is deliberate?

Delete the questions as you answer them, or delete the file: an empty context
file is treated as no context rather than as an instruction to say nothing.
"#;

/// What governance rules are, next to where they go.
///
/// A `README.md` rather than an example `.toml`, deliberately: `governance/`
/// loads every `*.toml` in it and a malformed rule is a hard error, so an
/// example file shipped here would either load as a real rule or break the
/// repository on first run.
const GOVERNANCE_TEMPLATE: &str = r#"# Governance rules

One `.toml` file per rule in this directory. Every `*.toml` here is loaded,
and a malformed one is an error rather than a skip — a rule that quietly
failed to load would leave you believing it was in force.

That is also why this file is Markdown: an example rule saved as `.toml` would
be loaded as a real one.

A rule looks like this:

    name = "No direct DB access from handlers"
    rank = "non-negotiable"          # or "nice-to-have"
    description = "Handlers go through the repository layer."
    forbid = { pattern = "psycopg2.connect" }
    scope = ["app/handlers/"]        # omit for the whole repository

Two mechanisms, with different consequences:

- `forbid` fails an audit after the fact. A `non-negotiable` violation stops
  the run from converging and the tree is rolled back; a `nice-to-have` is
  reported and does not.
- `deny-write = { paths = [...] }` refuses an edit before it reaches disk.
  Enforced at the orchestrator's only write path, so it binds whatever the
  model intended.

Matching is substring only, and that is deliberate. A governance check that
exercised judgment would be a model deciding whether it had followed the
rules, which is the thing this tool exists not to do.
"#;

/// Runs `<program> --version` and returns the first line, trimmed.
///
/// A version rather than a yes/no. "found" cannot tell the checker the user
/// configured from a different one earlier on `PATH`, and a verdict is only
/// as good as the process that produced it.
fn version_of(cmd: &[String]) -> Option<String> {
    let (program, pre) = cmd.split_first()?;
    // The configured command may be `python -m mypy`, where `--version` has
    // to follow the module, not the interpreter.
    let mut c = std::process::Command::new(program);
    c.args(pre).arg("--version");
    trellis_core::secrets::scrub(&mut c);
    let out = c.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = if out.stdout.is_empty() {
        String::from_utf8_lossy(&out.stderr).into_owned()
    } else {
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_string())
}

/// Whether the test runner has a coverage plugin available.
///
/// Reported because `Verified` versus `Reviewed` (S3) is the distinction the
/// product turns on, and with no coverage data nothing can populate
/// `Verified` — a user reading `0 verified` should know whether that is a
/// result or a missing tool.
///
/// Only pytest is asked (#572). The probe appends `--help`, which is not a
/// read-only question to an arbitrary program: `git --version --help` opens
/// Git's HTML help in a browser on Windows. Any other command answers false
/// without being run, and `doctor` says it was not probed.
fn coverage_available(tests: &[String]) -> bool {
    is_pytest(tests) && help_mentions_cov(tests)
}

/// Whether `tests` runs pytest: a program named `pytest`, `py.test` or
/// `pytest-3`, or a Python interpreter (`python`, `python3`, `py`) given
/// `-m pytest`, after any interpreter flags such as `py -3`.
fn is_pytest(tests: &[String]) -> bool {
    let Some((program, rest)) = tests.split_first() else {
        return false;
    };
    let Some(name) = Path::new(program).file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    match name {
        "pytest" | "py.test" | "pytest-3" => true,
        "python" | "python3" | "py" => {
            let mut args = rest.iter().skip_while(|a| a.starts_with('-') && *a != "-m");
            args.next().is_some_and(|a| a == "-m") && args.next().is_some_and(|a| a == "pytest")
        }
        _ => false,
    }
}

/// Runs `tests --help` and reads whether it offers `--cov`.
fn help_mentions_cov(tests: &[String]) -> bool {
    let Some((program, pre)) = tests.split_first() else {
        return false;
    };
    let mut c = std::process::Command::new(program);
    c.args(pre).arg("--help");
    trellis_core::secrets::scrub(&mut c);
    c.output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            let text = String::from_utf8_lossy(&o.stdout).to_lowercase();
            text.contains("--cov")
        })
        .unwrap_or(false)
}

/// The repository-side half of `doctor`: index, worktree, budget, floor.
///
/// `04-v1.0-harness.md` asks for these four alongside the model probe, and
/// every line that can be wrong says what to do about it.
fn repo_lines(root: &Path, cfg: &config::RepoConfig) -> Vec<String> {
    let mut out = Vec::new();

    // Index. Deliberately NOT the milestone document's "91.3% resolved".
    //
    // CLAUDE.md: what the resolver bound is a lower bound on recall, not
    // recall, and it must never be compared to the thresholds. Printing it
    // bare next to a tick would make `doctor` the place that claim finally
    // escapes — the same failure ADR 0014 fixed in `radius`, which had
    // printed "3.7% resolved" for a resolver audited at 72%.
    let started = std::time::Instant::now();
    let index = trellis_index::build_cached(root);
    let built = started.elapsed();
    let c = &index.coverage;
    if index.files.is_empty() {
        out.push(
            concat!(
                "index:      EMPTY — no Python sources found under this root.\n",
                "            Check --root, or whether the tree is one `discover` skips ",
                "(.venv,\n            site-packages, build, dist, __pycache__)."
            )
            .to_string(),
        );
    } else {
        out.push(format!(
            "index:      {} definitions across {} files · {} first-party call sites bound of \
             {} non-excluded call sites\n            ({:.1}% — a lower bound on recall, NOT recall; only the \
             hand audit is\n            comparable to the gate. See CLAUDE.md) · {} ms",
            index.symbols.len(),
            index.files.len(),
            c.resolved,
            c.total(),
            c.resolved_fraction().unwrap_or(0.0) * 100.0,
            built.as_millis()
        ));
    }

    // Worktree. The first wall users hit, so it says how to get past it.
    match trellis_vcs::pending(root) {
        Ok(p) if p.is_empty() => out.push("worktree:   clean".to_string()),
        Ok(p) => out.push(format!(
            "worktree:   {} uncommitted change(s) — `trellis run` will refuse to start.\n\
             {:12}Commit them, stash them, or `/revert` in a session. A run needs a clean\n\
             {:12}tree so rollback is exact (ADR 0009).",
            p.len(),
            "",
            ""
        )),
        Err(e) => out.push(format!(
            "worktree:   cannot read git status: {e}\n\
             {:12}TRELLIS needs a git repository; `git init` if this is not one.",
            ""
        )),
    }

    // Lockfile (ADR 0057): advisories and yanked crates in a `Cargo.lock` at
    // the root, so a lock that aged against the RustSec database is read
    // here before CI reads it. No line when there is no lock.
    out.extend(doctor_supply::line(root));

    // Skills (ADR 0019): what was found, what a run would list and leave out,
    // and every directory skipped with the reason. A skill that is silently
    // not loaded is one its author believes the model can read.
    let skills = trellis_loop::SkillCatalog::discover(root);
    let found = skills.valid.len() + skills.invalid.len();
    if found == 0 {
        out.push(format!(
            "skills:     none under {}",
            trellis_loop::SKILLS_DIR
        ));
    } else {
        let d = skills.disclosed();
        out.push(format!(
            "skills:     {found} found · {} listed · {} omitted (cap {} B) · {} invalid",
            d.listed.len(),
            d.omitted,
            trellis_loop::DISCLOSURE_CAP,
            skills.invalid.len()
        ));
        for i in &skills.invalid {
            out.push(format!(
                "            skipped {}: {}",
                plain(&i.dir),
                plain(&i.reason)
            ));
        }
    }

    // The control dial (ADR 0018). Said here because approve mode refuses to
    // start without a terminal, and that is better learned from `doctor`
    // than from a CI job.
    out.push(match cfg.control(None) {
        Ok(c) if c.mode == config::ControlMode::Approve => format!(
            "control:    {} — a run asks before each; needs a terminal, or `--auto`",
            c.describe()
        ),
        Ok(c) => format!("control:    {}", c.describe()),
        Err(e) => format!("control:    INVALID — {e}"),
    });

    // Budget, in the units it is actually enforced in.
    let b = cfg.budget();
    out.push(format!(
        "budget:     {:?} — {} iterations / {} s / {} tokens",
        b.regime,
        b.max_iterations,
        b.max_wall_clock.as_secs(),
        b.max_tokens
    ));

    // Integrated (ADR 0021 wave I): the user config, context files, user
    // skills, trust, profiles and routines, and the vault.
    out.extend(doctor_extra::input_report_lines(root, cfg));

    out
}

pub(crate) fn cmd_check(root: &Path, paths: Vec<PathBuf>) -> Result<()> {
    // `for_run`: two checks, one walk of `.trellis/` between them.
    let cfg = config::RepoConfig::load(root)?.checks().for_run(root);
    // An empty path list would invoke the checker with no arguments, which
    // most tools treat as an error rather than as "everything". Make the
    // documented default real.
    let paths = if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let verdicts = rt.block_on(async {
        let tc = trellis_checks::typecheck(root, &cfg, &paths).await;
        let tests = trellis_checks::run_tests(root, &cfg, &paths).await;
        [tc, tests]
    });

    let mut all_passed = true;
    for v in &verdicts {
        let d = v.detail();
        println!(
            "{:?}: {} ({} ms)",
            d.kind,
            if v.passed() { "passed" } else { "FAILED" },
            d.duration_ms
        );
        println!("  command: {}", d.command);
        if !v.passed() {
            all_passed = false;
            let tail: String = d
                .stdout
                .lines()
                .rev()
                .take(20)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            if !tail.trim().is_empty() {
                println!("{tail}");
            }
            if !d.stderr.trim().is_empty() {
                println!("{}", d.stderr.trim());
            }
        }
    }

    if !all_passed {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod doctor_tests {
    use super::*;

    fn repo(name: &str) -> PathBuf {
        let dir = crate::testutil::scratch(&format!("doctor-{name}"));
        std::fs::create_dir_all(dir.join("app")).unwrap();
        crate::testutil::git_init(&dir);
        std::fs::write(
            dir.join("app/util.py"),
            "def helper():
    return 1
",
        )
        .unwrap();
        crate::testutil::git_commit_all(&dir, "init");
        dir
    }

    /// The milestone mockup prints `Index  48,102 symbols · 91.3% resolved ✓`.
    /// CLAUDE.md forbids exactly that: what the resolver bound is a lower
    #[test]
    fn doctor_placement_names_a_model_that_is_not_the_loaded_one_and_another_server_adds_nothing() {
        // #633: the spec names qwen3:8b and Ollama has something else
        // loaded; reporting the other model's placement under this name
        // would describe the wrong weights. And a body in another shape
        // (a server that is not Ollama answering 200) adds no line.
        let body = r#"{"models":[{"name":"llama3:8b","size":10,"size_vram":10,
            "details":{"quantization_level":"Q8_0"}}]}"#;
        let lines = ollama_placement_lines(body, Some("qwen3:8b")).unwrap();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("qwen3:8b is not loaded (ollama has llama3:8b loaded)"),
            "{lines:?}"
        );
        assert!(!lines[0].contains("Q8_0"), "{lines:?}");
        // With no model named, every loaded entry is reported, and an ESC
        // byte in a name cannot repaint the terminal.
        let lines = ollama_placement_lines(
            r#"{"models":[{"name":"a\u001b[2Jb","size":10,"size_vram":0}]}"#,
            None,
        )
        .unwrap();
        assert!(lines[0].contains("a\\u{1b}[2Jb"), "{lines:?}");
        assert!(lines[0].contains("none of it in VRAM"), "{lines:?}");
        assert!(lines[0].contains("quantization not reported"), "{lines:?}");
        assert_eq!(
            ollama_placement_lines(r#"{"choices":[{"text":""}]}"#, Some("x")),
            None
        );
        assert_eq!(ollama_placement_lines("not json", None), None);
    }

    /// bound on recall, and comparing it to the gate thresholds is the error
    /// ADR 0014 already fixed once in `radius`, which printed "3.7% resolved"
    /// for a resolver audited at 72%.
    #[test]
    fn the_index_line_refuses_to_present_the_bound_as_recall() {
        let dir = repo("index-line");
        let cfg = config::RepoConfig::load(&dir).unwrap();
        let lines = repo_lines(&dir, &cfg).join(
            "
",
        );

        assert!(lines.contains("index:"), "{lines}");
        assert!(
            lines.contains("NOT recall"),
            "the caveat is the point of the line: {lines}"
        );
        assert!(
            lines.contains("hand audit"),
            "and it must name what IS comparable: {lines}"
        );
        assert!(
            lines.contains("non-excluded call sites"),
            "the denominator is named as `trellis index` names it (A-30): {lines}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// "Every line that can be wrong should say what to do about it."
    #[test]
    fn a_dirty_worktree_says_how_to_get_past_it() {
        let dir = repo("dirty");
        std::fs::write(
            dir.join("app/util.py"),
            "def helper():
    return 2
",
        )
        .unwrap();
        let cfg = config::RepoConfig::load(&dir).unwrap();
        let lines = repo_lines(&dir, &cfg).join(
            "
",
        );

        assert!(lines.contains("uncommitted change"), "{lines}");
        assert!(
            lines.contains("Commit them") || lines.contains("stash"),
            "the first wall users hit must name the way through: {lines}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clean_worktree_says_so_plainly() {
        let dir = repo("clean");
        let cfg = config::RepoConfig::load(&dir).unwrap();
        let lines = repo_lines(&dir, &cfg).join(
            "
",
        );
        assert!(lines.contains("worktree:   clean"), "{lines}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The mockup's `Floor  above measured minimum ✓` is a claim TRELLIS
    /// cannot make. A floor is located by a configuration that *passes*; of
    /// four measured, only one did, at tier 4 and not under constrained
    /// decoding, so there is no minimum this model is known to be above.
    /// Printing the tick would be the exact failure S7 exists to prevent.
    #[test]
    fn the_floor_line_never_claims_a_minimum_that_was_never_located() {
        for configured in [true, false] {
            let line = floor_line(configured);
            // The wrap differs between the two branches; compare the words.
            let words = line.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                words.contains("located only at tier 4")
                    && words.contains("not under constrained decoding"),
                "the floor is located only at tier 4, not under constraint: {line}"
            );
            assert!(
                !line.contains("above measured minimum"),
                "TRELLIS cannot know this and must not print it: {line}"
            );
            assert!(
                line.contains("BENCHMARK_AND_EVAL"),
                "and it must point at the evidence: {line}"
            );
        }
    }

    #[test]
    fn an_empty_index_says_what_to_check_rather_than_reporting_zero() {
        // Zero definitions reads as a repository with no code. The likelier
        // cause is a wrong --root or a tree `discover` skips, and the line
        // says so instead of leaving a user staring at 0.
        let dir = std::env::temp_dir().join(format!("trellis-doctor-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::testutil::git_ok(&dir, &["init", "-q"]);
        let cfg = config::RepoConfig::load(&dir).unwrap();
        let lines = repo_lines(&dir, &cfg).join(
            "
",
        );

        assert!(lines.contains("index:      EMPTY"), "{lines}");
        assert!(lines.contains("--root"), "{lines}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn coverage_is_available_only_when_the_test_runner_offers_cov() {
        // #248. The risk: `doctor` reporting a coverage plugin it never
        // asked for, or missing one that is there. The runner's `--help` is
        // what is read; `echo` stands in for a runner and prints its text.
        let runner = |text: &str| -> Vec<String> {
            if cfg!(windows) {
                vec!["cmd".into(), "/C".into(), format!("echo {text}")]
            } else {
                vec!["sh".into(), "-c".into(), format!("echo {text}")]
            }
        };
        assert!(help_mentions_cov(&runner("--cov=PATH")));
        assert!(!help_mentions_cov(&runner("no plugin here")));
    }

    #[test]
    fn doctor_coverage_probe_asks_only_pytest() {
        // #572. The risk: appending `--help` to a program that is not pytest,
        // which for `git --version` opens a browser on Windows. Each spelling
        // of pytest is still probed; anything else is not.
        let cmd = |s: &str| -> Vec<String> { s.split(' ').map(String::from).collect() };
        for yes in [
            "pytest",
            "py.test",
            "pytest-3",
            "/usr/bin/pytest",
            "pytest.exe",
            "python -m pytest",
            "python3 -m pytest -q",
            "py -3 -m pytest",
            "/opt/venv/bin/python -m pytest",
        ] {
            assert!(is_pytest(&cmd(yes)), "{yes} is pytest");
        }
        for no in [
            "git --version",
            "python",
            "python -m mypy",
            "python script.py -m pytest",
            "py -3",
            "pytest-runner",
            "sh -c pytest",
        ] {
            assert!(!is_pytest(&cmd(no)), "{no} is not pytest");
        }
        assert!(!is_pytest(&[]));
        // Answered without a spawn: `git --version --help` is never run.
        assert!(!coverage_available(&cmd("git --version")));
    }
}

#[cfg(test)]
mod init_tests {
    use super::*;
    use crate::index_cmd::{MAX_EDGES_DRAWN, graph_lines};
    use crate::run_cmd::{action_from, summarise};

    fn bare(name: &str) -> PathBuf {
        crate::testutil::scratch(&format!("init-{name}"))
    }

    fn repo(name: &str) -> PathBuf {
        let d = bare(name);
        crate::testutil::git_init(&d);
        d
    }

    #[test]
    fn init_refuses_outside_a_repository_and_says_how_to_make_one() {
        // The command for getting started must work before you have started,
        // and must name the missing step rather than the missing root.
        let d = bare("no-git");
        let e = cmd_init(&d).unwrap_err().to_string();
        assert!(e.contains("not a git repository"), "{e}");
        assert!(e.contains("git init"), "it must name the fix: {e}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn init_scaffolds_and_never_overwrites() {
        let d = repo("scaffold");
        cmd_init(&d).unwrap();
        assert!(d.join(".trellis/config.toml").exists());
        assert!(d.join(".trellis/context.md").exists());
        assert!(d.join(".trellis/governance/README.md").exists());

        // These files live in the user's repository. A scaffolder that
        // clobbers a config someone tuned is worse than one that refuses.
        std::fs::write(
            d.join(".trellis/config.toml"),
            "# mine
",
        )
        .unwrap();
        cmd_init(&d).unwrap();
        assert_eq!(
            std::fs::read_to_string(d.join(".trellis/config.toml")).unwrap(),
            "# mine
"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_scaffolded_config_parses_and_changes_nothing() {
        // Every value is commented out on purpose: a scaffold that writes
        // active settings makes the tool's behaviour depend on a file the
        // user has not read.
        let d = repo("config-inert");
        cmd_init(&d).unwrap();
        let cfg = config::RepoConfig::load(&d).expect("the scaffold must parse");
        assert_eq!(
            cfg.checks().typecheck,
            trellis_checks::CheckConfig::default().typecheck,
            "the scaffold must not change what runs"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_governance_example_is_markdown_so_it_is_never_loaded_as_a_rule() {
        // `governance/` loads every *.toml and a malformed one is a hard
        // error, so an example rule shipped as TOML would either take effect
        // or break the repository on first run.
        let d = repo("governance-example");
        cmd_init(&d).unwrap();
        let rules = trellis_checks::governance::load(&d).expect("the scaffold must not break it");
        assert!(rules.is_empty(), "a scaffold must not install live rules");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A turn, for building action-log cases without a model.
    fn turn(call: trellis_loop::ToolCall, observation: &str) -> trellis_loop::Turn {
        trellis_loop::Turn {
            call,
            observation: observation.to_string(),
            verdict_passed: None,
            check_kind: None,
            prompt_bytes: Some(1_000),
            radius_counts: None,
            radius_summarised: None,
            human: None,
            may_have_changed_tree: false,
            changed: Vec::new(),
            duration_ms: 0,
        }
    }

    #[test]
    fn an_escape_sequence_from_a_trail_cannot_repaint_the_terminal() {
        // A trail is untrusted on read-back (S10) and `trellis log` prints it
        // to a terminal, which interprets what it is given. A run that did not
        // converge could otherwise be made to look like one that did, in the
        // command a user reads to find out which.
        let forged = "fix the parser\x1b[2K\x1b[Aconverged, 2 checks passed";
        let out = plain(forged);
        assert!(!out.contains('\x1b'), "{out:?}");
        assert!(
            out.contains("fix the parser"),
            "the real text survives: {out:?}"
        );
        assert!(
            out.contains("\\u{1b}") || out.contains("\\x1b") || out.contains("\\e"),
            "and the escape is shown rather than dropped: {out:?}"
        );
    }

    #[test]
    fn ordinary_text_passes_through_a_sanitiser_unchanged() {
        // Including the em dashes and arrows this project's own messages use.
        for s in [
            "the describe function is annotated to return int",
            "app/calc.py",
            "2 rule(s) checked over 1 file(s); none broken — nothing to report",
        ] {
            assert_eq!(plain(s), s);
        }
    }

    #[test]
    fn a_hub_module_does_not_draw_an_unbounded_number_of_edges() {
        use trellis_index::{ModuleGraph, ModuleNode};
        // `--limit` caps modules and used to cap nothing else, so one module
        // every other one imports could emit more rows than the whole rest of
        // the listing — including into the interface's transcript.
        let g = ModuleGraph {
            nodes: vec![ModuleNode {
                module: "app.cli".into(),
                path: "app/cli.py".into(),
                defines: 1,
                calls: (0..40).map(|i| (format!("app.m{i}"), 40 - i)).collect(),
                ambiguous: 0,
                unresolved: 0,
            }],
        };
        let lines = graph_lines(&g, 40);
        let edges = lines.iter().filter(|l| l.contains("─▶")).count();
        assert_eq!(edges, MAX_EDGES_DRAWN, "{lines:#?}");
        assert!(
            lines.iter().any(|l| l.contains("30 more module(s) called")),
            "and the remainder is stated, not dropped: {lines:#?}"
        );
        // The heaviest edges are the ones kept — they are sorted by count.
        assert!(lines.iter().any(|l| l.contains("app.m0")), "{lines:#?}");
        assert!(!lines.iter().any(|l| l.contains("app.m39")), "{lines:#?}");
    }

    #[test]
    fn an_ignored_schema_downgrades_the_tier_rather_than_the_sentence() {
        // The defect: every run against ollama printed "tier 2, schema
        // enforced by the runtime" for a runtime that accepts `guided_json`
        // and ignores it. Rewording would have swapped a false claim for a
        // vaguer one; the tier itself has to change.
        let (unconstrain, note) = schema_report(&Ok(false));
        assert!(
            unconstrain,
            "the run must not claim a tier it does not have"
        );
        assert!(note.contains("ignored"), "{note}");
        assert!(
            note.contains("retried rather than prevented"),
            "and say what that costs: {note}"
        );
    }

    #[test]
    fn an_honoured_schema_keeps_the_tier_and_says_it_was_probed() {
        let (unconstrain, note) = schema_report(&Ok(true));
        assert!(!unconstrain);
        assert!(
            note.contains("probed"),
            "the claim names its evidence: {note}"
        );
    }

    #[test]
    fn a_probe_that_could_not_run_says_the_tier_is_assumed() {
        // A transient network error is not evidence about the runtime's
        // capabilities, so it must not silently change the tier — but it must
        // also not leave the banner asserting something nothing checked.
        let (unconstrain, note) = schema_report(&Err("connection reset".into()));
        assert!(
            !unconstrain,
            "a failed probe is not a measurement of an ignored schema"
        );
        assert!(note.contains("not verified"), "{note}");
        assert!(note.contains("connection reset"), "with the reason: {note}");
        assert!(note.contains("assumed"), "{note}");
    }

    #[test]
    fn an_applied_edit_is_the_only_action_that_records_a_write() {
        use trellis_loop::ToolCall;
        // `proposed` names the files a run ended up touching, not which action
        // did the touching. A run whose every edit was refused has an empty
        // `proposed` and a full list of actions.
        let refused = action_from(
            0,
            &turn(
                ToolCall::Edit {
                    path: "app/calc.py".into(),
                    old: "a".into(),
                    new: "b".into(),
                },
                "error: edit anchor not found in app/calc.py",
            ),
        );
        assert_eq!(refused.turn, 1, "turns are 1-based, as a run prints them");
        assert!(!refused.wrote, "a refused edit wrote nothing");

        let applied = action_from(
            1,
            &turn(
                ToolCall::Edit {
                    path: "app/calc.py".into(),
                    old: "a".into(),
                    new: "b".into(),
                },
                "edited app/calc.py (applied: true, bytes 59..87)",
            ),
        );
        assert!(applied.wrote, "an applied edit did");
        assert_eq!(applied.target, "app/calc.py");
    }

    #[test]
    fn reading_a_file_never_counts_as_a_write() {
        use trellis_loop::ToolCall;
        // The `applied: true` test is a substring match, so a read whose
        // content happened to contain that phrase must still not count.
        let a = action_from(
            0,
            &turn(
                ToolCall::ReadFile {
                    path: "app/calc.py".into(),
                },
                "applied: true",
            ),
        );
        assert!(!a.wrote, "only an edit can write");
        assert_eq!(a.tool, "read_file");
    }

    #[test]
    fn reading_a_skill_is_recorded_by_name_and_writes_nothing() {
        // ADR 0019 adds one closed-enum variant; the trail has to name it
        // rather than fold it into `read_file`, or a reader cannot tell which
        // procedure the model consulted.
        let a = action_from(
            2,
            &turn(
                trellis_loop::ToolCall::ReadSkill {
                    name: "rust-unsafe".into(),
                },
                "skill `rust-unsafe`, from .trellis/skills. applied: true",
            ),
        );
        assert_eq!(
            (a.turn, a.tool.as_str(), a.target.as_str(), a.wrote),
            (3, "read_skill", "rust-unsafe", false)
        );
    }

    #[test]
    fn checks_against_no_paths_say_so_rather_than_rendering_blank() {
        use trellis_checks::CheckKind;
        use trellis_loop::ToolCall;
        let a = action_from(
            0,
            &turn(
                ToolCall::RunChecks {
                    paths: Vec::new(),
                    kinds: vec![CheckKind::Tests],
                },
                "Tests: passed",
            ),
        );
        assert_eq!(a.target, "Tests on whole repository", "{a:?}");
    }

    #[test]
    fn changed_paths_reach_the_trail_action_cut_to_fifty_with_the_true_count() {
        // The risk: the paths a bash call changed reaching the turn and
        // stopping there, or reaching the trail whole. And `wrote` keeping
        // the meaning older trails gave it: set by `edit` and `write_file`.
        let mut t = turn(
            trellis_loop::ToolCall::Bash {
                command: "python gen.py".into(),
            },
            "exit 0
carried back from the clone: app/util.py",
        );
        t.changed = vec!["app/util.py".into()];
        let a = action_from(0, &t);
        assert_eq!(a.changed, ["app/util.py"]);
        assert_eq!(a.changed_count, 1);
        assert!(!a.wrote);

        t.changed = (0..60).map(|i| format!("pkg/f{i:02}.py")).collect();
        let a = action_from(0, &t);
        assert_eq!(a.changed.len(), 50);
        assert_eq!(a.changed[..], t.changed[..50]);
        assert_eq!(a.changed_count, 60);

        let read = action_from(
            1,
            &turn(
                trellis_loop::ToolCall::ReadFile {
                    path: "app/util.py".into(),
                },
                "def helper(): ...",
            ),
        );
        assert!(read.changed.is_empty() && read.changed_count == 0);
    }

    #[test]
    fn an_outcome_is_one_line_and_says_when_it_was_cut() {
        // A check's output runs to thousands of lines. A trail that stored it
        // would be mostly type-checker output, and a silently truncated line
        // reads as the whole outcome.
        assert_eq!(summarise("first\nsecond\nthird"), "first");
        assert_eq!(summarise("\n\n  indented  \nnext"), "indented");
        assert_eq!(summarise(""), "");
        let long = "x".repeat(400);
        let cut = summarise(&long);
        assert_eq!(cut.chars().count(), 120);
        assert!(cut.ends_with('…'), "a cut line says it was cut: {cut}");
    }

    #[test]
    fn a_multibyte_outcome_is_cut_on_a_character_boundary() {
        // `…` and an em dash both appear in this repository's own messages,
        // and slicing by bytes would panic on one.
        let long = "é".repeat(400);
        assert_eq!(summarise(&long).chars().count(), 120);
    }

    // ---- #248: survivors of the trellis-cli mutation run ----

    #[test]
    fn a_module_path_too_wide_for_the_column_keeps_its_tail() {
        // The tail is what tells two siblings apart; a cut that kept the
        // head, or the wrong number of characters, prints a column of
        // identical names.
        assert_eq!(truncate_module("app.cli", 7), "app.cli");
        assert_eq!(truncate_module("app.commands.add", 6), "…s.add");
        assert_eq!(truncate_module("app.commands.add", 6).chars().count(), 6);
    }

    fn node(module: &str, calls: usize) -> trellis_index::ModuleNode {
        trellis_index::ModuleNode {
            module: module.into(),
            path: format!("{module}.py"),
            defines: 1,
            calls: (0..calls).map(|i| (format!("{module}.c{i}"), 1)).collect(),
            ambiguous: 0,
            unresolved: 0,
        }
    }

    #[test]
    fn only_a_module_s_last_drawn_edge_closes_its_branch() {
        // The risk: `└` on the wrong row, or "and 0 more" under a module
        // whose edges were all drawn, so the listing claims a remainder that
        // does not exist.
        let g = trellis_index::ModuleGraph {
            nodes: vec![node("a", 2), node("b", MAX_EDGES_DRAWN)],
        };
        let lines = graph_lines(&g, 40);
        let edges: Vec<&String> = lines.iter().filter(|l| l.contains("─▶")).collect();
        assert_eq!(edges.len(), 2 + MAX_EDGES_DRAWN, "{lines:#?}");
        assert!(edges[0].contains("├─▶ a.c0"), "{lines:#?}");
        assert!(edges[1].contains("└─▶ a.c1"), "{lines:#?}");
        let last = MAX_EDGES_DRAWN - 1;
        assert!(
            edges[2 + last].contains(&format!("└─▶ b.c{last}")),
            "{lines:#?}"
        );
        let closed = edges.iter().filter(|l| l.contains('└')).count();
        assert_eq!(closed, 2, "{lines:#?}");
        assert!(
            !lines.iter().any(|l| l.contains("called from here")),
            "{lines:#?}"
        );
    }

    #[test]
    fn modules_past_the_limit_are_counted_and_none_past_it_are_not() {
        let g = trellis_index::ModuleGraph {
            nodes: vec![node("a", 0), node("b", 0)],
        };
        let more = |limit| {
            graph_lines(&g, limit)
                .into_iter()
                .find(|l| l.contains("not drawn"))
        };
        assert_eq!(more(2), None);
        assert_eq!(
            more(1).as_deref(),
            Some("  ... 1 more module(s), not drawn.")
        );
    }

    #[test]
    fn codebase_graph_draws_the_repository_s_own_modules() {
        let d = repo("codebase-lines");
        std::fs::create_dir_all(d.join("app")).unwrap();
        std::fs::write(d.join("app/util.py"), "def helper():\n    return 1\n").unwrap();
        let lines = graph_lines(&codebase_graph(&d), 40);
        assert_eq!(lines[0], "codebase graph — 1 module(s)", "{lines:#?}");
        assert!(lines.iter().any(|l| l.contains("app.util")), "{lines:#?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn doctor_warns_that_approve_mode_needs_a_terminal_and_only_then() {
        // Approve mode refuses to start without a terminal; doctor is where
        // a CI author learns that before the job fails (ADR 0018).
        let line = |mode: &str| {
            let d = repo(&format!("control-{mode}"));
            std::fs::create_dir_all(d.join(".trellis")).unwrap();
            std::fs::write(
                d.join(".trellis/config.toml"),
                format!("[control]\nmode = \"{mode}\"\n"),
            )
            .unwrap();
            let cfg = config::RepoConfig::load(&d).unwrap();
            let got = repo_lines(&d, &cfg)
                .into_iter()
                .find(|l| l.starts_with("control:"))
                .unwrap();
            let _ = std::fs::remove_dir_all(&d);
            got
        };
        assert!(line("approve").contains("needs a terminal"));
        assert!(!line("auto").contains("needs a terminal"));
    }
}
