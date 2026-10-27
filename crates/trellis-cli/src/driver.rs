//! The model driver a run's loop is generic over. Used by
//! `run_cmd::run_body` and by `run_partitioned::run_partitioned`, each of
//! which builds its own and drives it through
//! [`crate::observed_model::Observed`].

use crate::{config, hosted};
use anyhow::{Context, Result};

/// What drives the loop this run.
///
/// Not a trait object: `Model::next_step` is an `async fn` in a trait, which
/// is not dyn-compatible, and the loop is generic over it.
pub(crate) enum Driver {
    Scripted(trellis_loop::ScriptedModel),
    Runtime(trellis_loop::RuntimeModel),
    /// Tier 1: a hosted provider's own tool calling (ADR 0024).
    ///
    /// Boxed: it carries the conversation's replayed provider messages, and
    /// the other variants should not pay for its size.
    Hosted(Box<trellis_loop::HostedModel>),
}

impl Driver {
    /// Builds the driver a parsed spec names.
    ///
    /// Takes a [`ModelSpec`] rather than a string: the string form was parsed
    /// here, in `cmd_doctor` and in the session, each deciding for itself what
    /// was valid. Construction is now the only thing this does.
    ///
    /// `targets` is `(every target of the run, this agent's)` for one agent
    /// of a multi-agent run, which a script answers from its `targets`
    /// section (#466); `None` for every other run.
    pub(crate) fn build(
        spec: &trellis_model::ModelSpec,
        unconstrained: bool,
        tools: &trellis_loop::ToolSet,
        cfg: &config::RepoConfig,
        from_user: bool,
        source: hosted::SpecSource,
        targets: Option<(&[String], &[String])>,
    ) -> Result<Self> {
        if let Some(r) = hosted::resolve_from(spec, cfg, from_user, source)? {
            if unconstrained {
                anyhow::bail!(
                    "--unconstrained is meaningless for a hosted model: it calls tools \
                     through the provider's own API (tier 1)"
                );
            }
            // The profile is off until `preflight` identifies colibri, which
            // is after this; `cmd_run` rebuilds the driver then.
            return Ok(Driver::Hosted(Box::new(hosted::build(&r, cfg, false)?)));
        }
        match spec {
            trellis_model::ModelSpec::Scripted(path) => {
                if unconstrained {
                    anyhow::bail!("--unconstrained is meaningless for a scripted model");
                }
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("cannot read script {}", path.display()))?;
                // Against the run's own tool set: a step calling a tool this
                // run does not offer fails here, before the snapshot.
                let m = match targets {
                    None => trellis_loop::ScriptedModel::from_json_with(&text, tools),
                    Some((all, mine)) => {
                        trellis_loop::ScriptedModel::from_json_for(&text, tools, all, mine)
                    }
                }
                .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
                Ok(Driver::Scripted(m))
            }
            trellis_model::ModelSpec::Runtime {
                dialect,
                base,
                model,
            } => {
                // `[model] timeout_s` bounds each request (ADR 0047 decision
                // 9); the wall-clock budget still bounds the run (ADR 0008).
                let client = trellis_model::RuntimeClient::new(base, *dialect, model.clone())?
                    .with_timeout(cfg.model_timeout());
                // Named rather than `new(client, !unconstrained)`: the flag is
                // called --unconstrained, so a negated bool at the call site
                // meant reading two negations to learn which tier runs.
                Ok(Driver::Runtime(
                    if unconstrained {
                        trellis_loop::RuntimeModel::unconstrained(client)
                    } else {
                        trellis_loop::RuntimeModel::constrained(client)
                    }
                    .with_context_bytes(Some(hosted::runtime_context_bytes(cfg))),
                ))
            }
            trellis_model::ModelSpec::Hosted { .. } | trellis_model::ModelSpec::Provider { .. } => {
                unreachable!("resolved above")
            }
        }
    }

    /// Every compaction the driver made (ADR 0024 §5).
    pub(crate) fn compactions(&self) -> Vec<trellis_loop::Compacted> {
        use trellis_loop::Model;
        match self {
            Driver::Scripted(m) => m.compactions().to_vec(),
            Driver::Runtime(m) => m.compactions().to_vec(),
            Driver::Hosted(m) => m.compactions().to_vec(),
        }
    }

    /// Every compaction a client asked for over RPC (ADR 0025 §5).
    pub(crate) fn requested_compactions(&self) -> Vec<trellis_loop::RequestedCompaction> {
        use trellis_loop::Model;
        match self {
            Driver::Scripted(m) => m.requested_compactions().to_vec(),
            Driver::Runtime(m) => m.requested_compactions().to_vec(),
            Driver::Hosted(m) => m.requested_compactions().to_vec(),
        }
    }

    /// Hands RPC's `compact` requests to the driver that will answer them.
    pub(crate) fn compact_on_request(
        &mut self,
        requests: std::sync::Arc<trellis_loop::CompactRequests>,
    ) {
        match self {
            Driver::Scripted(_) => requests.close(
                "this run's model is scripted: it is sent no prompt, so there is nothing \
                 to compact",
            ),
            Driver::Runtime(m) => m.compact_on_request(requests),
            Driver::Hosted(m) => m.compact_on_request(requests),
        }
    }

    /// Completion tokens as the adapter counted them, for the trail (ADR
    /// 0026 §8); `None` for a scripted driver, and for a hosted one once a
    /// reply went unreported.
    pub(crate) fn tokens(&self) -> Option<u64> {
        match self {
            Driver::Scripted(_) => None,
            Driver::Runtime(m) => trellis_loop::Model::tokens_used(m),
            // Hosted runs count against a profile's budget too; the provider
            // reports output tokens and the adapter sums them (ADR 0024).
            // Not `tokens_used`, which charges an unreported reply its upper
            // bound for the in-run ceiling: the trail takes only a measured
            // count, and `None` once any reply went unreported.
            Driver::Hosted(m) => m.tokens_measured(),
        }
    }

    /// Prompt tokens, `None` once any call went unreported (ADR 0027).
    pub(crate) fn prompt_tokens(&self) -> Option<u64> {
        match self {
            Driver::Scripted(_) => None,
            Driver::Runtime(m) => trellis_loop::Model::prompt_tokens(m),
            Driver::Hosted(m) => trellis_loop::Model::prompt_tokens(m.as_ref()),
        }
    }

    pub(crate) fn describe(&self) -> String {
        match self {
            Driver::Scripted(m) => format!(
                "scripted, {} steps — real loop, real checks, real rollback; judgement stubbed",
                m.len()
            ),
            // `degradation()` answers "what do you lose at this tier", and
            // printed bare it read as `LlamaCpp via RuntimeConstrained —
            // none for correctness; grammar compilation happens in the
            // serving runtime`, which says nothing to a reader who has not
            // memorised the tier table. Number the tier and label the string.
            Driver::Runtime(m) => format!(
                "{:?}, tier {} — degradation: {}",
                m.dialect(),
                match m.tier() {
                    trellis_model::AdapterTier::Native => "1, native tool calls",
                    trellis_model::AdapterTier::RuntimeConstrained =>
                        "2, schema enforced by the runtime",
                    trellis_model::AdapterTier::HarnessConstrained => "3",
                    trellis_model::AdapterTier::ValidateAndRetry =>
                        "4, no schema; the reply is validated after",
                },
                m.tier().degradation()
            ),
            Driver::Hosted(m) => hosted::describe(m),
        }
    }
}
