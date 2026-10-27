//! The model driver a run's loop is generic over, and the wrapper every run
//! drives it through. Used by `run_cmd::run_body` and by
//! `run_partitioned::run_partitioned`, each of which builds its own.

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

/// A driver that remembers the `done` summary it handed the loop, and
/// every step, for the run's replay recording (#620, ADR 0056).
///
/// `Outcome` does not carry the summary, and the session's `/copy` wants it.
/// The transcript does not carry a `done` step, and a model's step that a
/// gate refused is still the model's, so the steps are taken here, as the
/// driver handed them over, rather than from the transcript.
/// Observing it on the way past keeps that inside the binary: the loop's
/// types stay as they are, and every method the loop reads passes straight
/// through, so the budget, the context figures, the compactions and the
/// step's usage are the driver's own.
///
/// `prompt_tokens` and `requested_compactions` are not forwarded, and need
/// not be (#487): the loop never calls them, and the trail reads them, with
/// the compactions, from the concrete `Driver` after the run, around this
/// wrapper (`prompt_tokens` and `driver.requested_compactions()` in
/// `cmd_run`). A compaction asked for over RPC is acted on by the driver,
/// through `Driver::compact_on_request`, not through this trait. A new read
/// through the wrapper needs the method forwarded here and in
/// `rpc::Watched`.
pub(crate) struct SeesDone<'a, M> {
    pub(crate) inner: &'a mut M,
    pub(crate) done: &'a mut Option<String>,
    pub(crate) steps: &'a mut Vec<trellis_loop::Step>,
}

impl<M: trellis_loop::Model + Send> trellis_loop::Model for SeesDone<'_, M> {
    async fn next_step(
        &mut self,
        t: &trellis_loop::Transcript,
    ) -> Result<trellis_loop::Step, String> {
        let step = self.inner.next_step(t).await;
        if let Ok(trellis_loop::Step::Done { summary }) = &step {
            *self.done = Some(summary.clone());
        }
        if let Ok(s) = &step {
            self.steps.push(s.clone());
        }
        step
    }

    fn tokens_used(&self) -> Option<u64> {
        self.inner.tokens_used()
    }

    fn prompt_bytes(&self) -> Option<usize> {
        self.inner.prompt_bytes()
    }

    fn context_bytes(&self) -> Option<usize> {
        self.inner.context_bytes()
    }

    fn compactions(&self) -> &[trellis_loop::Compacted] {
        self.inner.compactions()
    }

    fn step_usage(&self) -> trellis_loop::StepUsage {
        self.inner.step_usage()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wrappers_a_run_drives_pass_the_driver_s_figures_through() {
        // The risk: the budget counts tokens and the trail records prompt
        // bytes through `SeesDone` over `rpc::Watched`, the stack every
        // run drives; a wrapper answering a default would make a metered
        // run look free, its context unmeasured and its `run budget`
        // without a context ceiling.
        use trellis_loop::Model;
        let mut inner = crate::testutil::figures();
        let mut watched = crate::rpc::Watched::new(&mut inner, None);
        let (mut done, mut steps) = (None, Vec::new());
        let m = SeesDone {
            inner: &mut watched,
            done: &mut done,
            steps: &mut steps,
        };
        assert_eq!(m.tokens_used(), Some(7));
        assert_eq!(m.prompt_bytes(), Some(4321));
        assert_eq!(m.context_bytes(), Some(9_000));
        assert_eq!(m.compactions().len(), 1);
        assert_eq!(m.compactions()[0].elided_bytes, 50);
    }

    #[test]
    fn sees_done_over_watched_forwards_step_usage() {
        // The risk (#256): a wrapper answering the trait's default would
        // leave every run's `turn` event without output tokens or queue
        // wait, though the driver measured both.
        use trellis_loop::Model;
        let mut inner = crate::testutil::figures();
        let mut watched = crate::rpc::Watched::new(&mut inner, None);
        let (mut done, mut steps) = (None, Vec::new());
        let m = SeesDone {
            inner: &mut watched,
            done: &mut done,
            steps: &mut steps,
        };
        assert_eq!(
            m.step_usage(),
            trellis_loop::StepUsage {
                completion_tokens: Some(13),
                queue_wait_ms: Some(250),
            }
        );
    }
}
