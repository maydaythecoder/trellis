//! The one wrapper every run drives its model through, observing what the
//! run needs on the way past (#931).

use crate::rpc::SteeringPort;

/// A driver whose requests are reported to RPC's [`SteeringPort`], and
/// which remembers the `done` summary it handed the loop and every step,
/// for the run's replay recording (#620, ADR 0056).
///
/// The loop drains the steering channel immediately before each request,
/// so the steers a request carries are the transcript's delivered ones at
/// the moment it is made. Reading them here, rather than from the
/// transcript the run returns, is what makes delivery known on an aborted
/// run too. With no port nothing is reported.
///
/// `Outcome` does not carry the summary, and the session's `/copy` wants it.
/// The transcript does not carry a `done` step, and a model's step that a
/// gate refused is still the model's, so the steps are taken here, as the
/// driver handed them over, rather than from the transcript.
///
/// Every `trellis_loop::Model` method passes straight through, so the
/// budget, the context figures, the compactions, the step's usage and what
/// the model said are the driver's own. One type rather than two stacked
/// ones: the two had drifted, one forwarding `requested_compactions` and
/// the other answering the trait's default, and a method one of them missed
/// would drop a field from every `turn` event without failing anything.
pub(crate) struct Observed<'a, M> {
    model: &'a mut M,
    port: Option<&'a SteeringPort>,
    done: &'a mut Option<String>,
    steps: &'a mut Vec<trellis_loop::Step>,
}

impl<'a, M> Observed<'a, M> {
    pub(crate) fn new(
        model: &'a mut M,
        port: Option<&'a SteeringPort>,
        done: &'a mut Option<String>,
        steps: &'a mut Vec<trellis_loop::Step>,
    ) -> Self {
        Self {
            model,
            port,
            done,
            steps,
        }
    }
}

impl<M: trellis_loop::Model + Send> trellis_loop::Model for Observed<'_, M> {
    async fn next_step(
        &mut self,
        t: &trellis_loop::Transcript,
    ) -> Result<trellis_loop::Step, String> {
        if let Some(p) = self.port {
            p.observe(&t.steering.delivered);
        }
        let step = self.model.next_step(t).await;
        if let Ok(trellis_loop::Step::Done { summary }) = &step {
            *self.done = Some(summary.clone());
        }
        if let Ok(s) = &step {
            self.steps.push(s.clone());
        }
        step
    }

    fn tokens_used(&self) -> Option<u64> {
        self.model.tokens_used()
    }

    fn prompt_tokens(&self) -> Option<u64> {
        self.model.prompt_tokens()
    }

    fn prompt_bytes(&self) -> Option<usize> {
        self.model.prompt_bytes()
    }

    fn context_bytes(&self) -> Option<usize> {
        self.model.context_bytes()
    }

    fn compactions(&self) -> &[trellis_loop::Compacted] {
        self.model.compactions()
    }

    fn requested_compactions(&self) -> &[trellis_loop::RequestedCompaction] {
        self.model.requested_compactions()
    }

    fn step_usage(&self) -> trellis_loop::StepUsage {
        self.model.step_usage()
    }

    fn said(&self) -> Option<&str> {
        self.model.said()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- #248: survivors of the trellis-cli mutation run ----

    #[test]
    fn watched_passes_the_driver_s_figures_through() {
        use trellis_loop::Model;
        let mut inner = crate::testutil::figures();
        let (mut done, mut steps) = (None, Vec::new());
        let w = Observed::new(&mut inner, None, &mut done, &mut steps);
        assert_eq!(w.tokens_used(), Some(7));
        assert_eq!(w.prompt_bytes(), Some(4321));
        assert_eq!(w.context_bytes(), Some(9_000));
        assert_eq!(w.compactions().len(), 1);
        assert_eq!(w.compactions()[0].elided_bytes, 50);
        assert_eq!(w.requested_compactions().len(), 1);
        assert_eq!(w.requested_compactions()[0].elided_bytes, 60);
    }

    #[test]
    fn watched_forwards_step_usage() {
        // The risk (#256): every RPC run drives its model through
        // `Watched`, so a default answer here would drop the step's figures
        // from every `turn` event a client is sent.
        use trellis_loop::Model;
        let mut inner = crate::testutil::figures();
        let (mut done, mut steps) = (None, Vec::new());
        let w = Observed::new(&mut inner, None, &mut done, &mut steps);
        assert_eq!(
            w.step_usage(),
            trellis_loop::StepUsage {
                completion_tokens: Some(13),
                queue_wait_ms: Some(250),
            }
        );
    }

    #[test]
    fn the_wrappers_a_run_drives_pass_the_driver_s_figures_through() {
        // The risk: the budget counts tokens and the trail records prompt
        // bytes through `SeesDone` over `rpc::Watched`, the stack every
        // run drives; a wrapper answering a default would make a metered
        // run look free, its context unmeasured and its `run budget`
        // without a context ceiling.
        use trellis_loop::Model;
        let mut inner = crate::testutil::figures();
        let (mut done, mut steps) = (None, Vec::new());
        let m = Observed::new(&mut inner, None, &mut done, &mut steps);
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
        let (mut done, mut steps) = (None, Vec::new());
        let m = Observed::new(&mut inner, None, &mut done, &mut steps);
        assert_eq!(
            m.step_usage(),
            trellis_loop::StepUsage {
                completion_tokens: Some(13),
                queue_wait_ms: Some(250),
            }
        );
    }

    #[test]
    fn the_wrapper_forwards_said_and_prompt_tokens() {
        // The risk (#931): neither had a test through a wrapper, so a
        // missed forward would leave every `turn` event without the words
        // the model said beside its call (#953), and a prompt-token read
        // through the wrapper answering `None` as if no call reported one.
        use trellis_loop::Model;
        let mut inner = crate::testutil::figures();
        let (mut done, mut steps) = (None, Vec::new());
        let m = Observed::new(&mut inner, None, &mut done, &mut steps);
        assert_eq!(m.said(), Some("looking at the caller first"));
        assert_eq!(m.prompt_tokens(), Some(1_234));
    }
}
