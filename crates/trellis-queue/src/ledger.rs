//! The token ledger and budget enforcement (ADR 0026 §5).
//!
//! Spend is what trails recorded: completion tokens as the runtime adapter
//! reported them, per run, attributed to the profile the run executed as.
//! Prompt tokens are summed beside them and never into them: the two are
//! different quantities (a prompt is re-sent every turn), budgets limit
//! completion tokens only, and one total would hide which of the two moved.
//! This crate does not read trails — it takes [`SpendRecord`]s, so the
//! arithmetic and the thresholds are tested without a repository, and the
//! binary is the one place that turns trail files into records.
//!
//! No dollars: COST_MODEL.md does not price tokens. Dollar pricing is
//! deferred to v1.x, beside tokens and never replacing them (ADR 0026 §5,
//! amended for #67).

use crate::activity::{Actor, Event};
use crate::approval;
use crate::profile::{self, Overlay, Pause, PausedBy, Profile, Window};
use crate::{Queue, QueueError};

/// One run's recorded spend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendRecord {
    pub agent: String,
    /// `None` when the trail recorded no count: a scripted run, a process
    /// agent, or a trail written before ADR 0026.
    pub tokens: Option<u64>,
    /// Prompt tokens, read from the trail's own `prompt_tokens` and kept
    /// apart from `tokens`. `None` means the adapter reported no prompt
    /// figure, which is not the same as a prompt of zero tokens.
    pub prompt_tokens: Option<u64>,
    /// The trail's `started_at`: which month the run belongs to.
    pub started_at: String,
    /// The trail's factual digest did not match. Counted anyway — it is the
    /// only record there is — and reported, since the figure may have been
    /// edited.
    pub altered: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Spend {
    pub month: u64,
    pub lifetime: u64,
    pub runs: u32,
    /// Runs attributed to this profile whose trail recorded no token count.
    pub runs_without_count: u32,
    /// Prompt tokens this month and lifetime, from the runs that recorded a
    /// prompt figure. Deliberately no method adds these to `month` or
    /// `lifetime`: see the module comment.
    pub prompt_month: u64,
    pub prompt_lifetime: u64,
    /// Runs whose trail recorded no prompt figure. The prompt sums are a
    /// lower bound whenever this is non-zero, and a reader has to be told so
    /// rather than shown a smaller number as if it were complete.
    pub runs_without_prompt_count: u32,
    pub altered: u32,
}

/// Sums one profile's spend for `month` (`YYYY-MM`, UTC) and lifetime.
pub fn spend(records: &[SpendRecord], agent: &str, month: &str) -> Spend {
    let mut s = Spend::default();
    for r in records.iter().filter(|r| r.agent == agent) {
        s.runs += 1;
        if r.altered {
            s.altered += 1;
        }
        let this_month = crate::month_of(&r.started_at).as_deref() == Some(month);
        // Before the completion figure, whose absence ends this iteration:
        // the two counts are reported independently and one missing does
        // not make the other unknown.
        match r.prompt_tokens {
            Some(p) => {
                s.prompt_lifetime = s.prompt_lifetime.saturating_add(p);
                if this_month {
                    s.prompt_month = s.prompt_month.saturating_add(p);
                }
            }
            None => s.runs_without_prompt_count += 1,
        }
        let Some(t) = r.tokens else {
            s.runs_without_count += 1;
            continue;
        };
        s.lifetime = s.lifetime.saturating_add(t);
        if this_month {
            s.month = s.month.saturating_add(t);
        }
    }
    s
}

#[derive(Debug, Clone, PartialEq)]
pub enum Assessment {
    /// No budget, or under the warning line.
    Ok,
    Warn {
        window: Window,
        spent: u64,
        limit: u64,
    },
    Exceeded {
        window: Window,
        spent: u64,
        limit: u64,
    },
}

/// Where a profile stands against its limits after raises. An exceeded
/// window wins over a warning in the other; lifetime is checked first
/// because a raise to the month does not help a spent lifetime.
pub fn assess(p: &Profile, o: &Overlay, s: &Spend, month: &str) -> Assessment {
    let Some(b) = &p.file.budget else {
        return Assessment::Ok;
    };
    let windows = [
        (
            Window::Lifetime,
            o.limit(b.lifetime_tokens, Window::Lifetime, month),
            s.lifetime,
        ),
        (
            Window::Monthly,
            o.limit(b.monthly_tokens, Window::Monthly, month),
            s.month,
        ),
    ];
    let mut warn = Assessment::Ok;
    for (window, limit, spent) in windows {
        let Some(limit) = limit else { continue };
        if spent >= limit {
            return Assessment::Exceeded {
                window,
                spent,
                limit,
            };
        }
        if warn == Assessment::Ok && (spent as f64) >= p.warn_at() * limit as f64 {
            warn = Assessment::Warn {
                window,
                spent,
                limit,
            };
        }
    }
    warn
}

/// What [`enforce`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Enforced {
    /// A line to print: the warning, or the hard stop.
    pub message: Option<String>,
    /// The profile is paused by its budget as of now.
    pub paused: bool,
    /// A budget pause was lifted, and why: the monthly window rolled over,
    /// or the profile is no longer over its limit (its file raised it).
    pub resumed: Option<String>,
    /// The pending override request, if one was created or already open.
    pub request: Option<String>,
}

/// Applies the thresholds to one profile (ADR 0026 §5).
///
/// - Under the warning line: nothing.
/// - At `warn_at`: a message.
/// - On crossing 100%: the profile is paused by its budget and one
///   `budget_override` request is opened. One per pause, not per tick: a
///   profile that stays over its limit keeps its pause and opens nothing
///   more, so a rejected request stays rejected.
/// - A budget pause is lifted once the profile is under every limit again:
///   the month rolled over, or someone raised the limit in its file. A pause
///   by a person is never lifted here.
pub fn enforce(
    q: &Queue,
    p: &Profile,
    s: &Spend,
    month: &str,
    actor: Actor,
) -> Result<Enforced, QueueError> {
    let mut o = profile::overlay(q, &p.name)?;
    let mut done = Enforced::default();
    let entity = format!("agent:{}", p.name);
    let unpaused = Overlay {
        paused: None,
        ..o.clone()
    };
    let over = matches!(assess(p, &unpaused, s, month), Assessment::Exceeded { .. });

    if let Some(Pause {
        by: PausedBy::Budget,
        month: paused_in,
        ..
    }) = &o.paused
        && !over
    {
        let why = match paused_in {
            Some(m) if m != month => format!("monthly budget window rolled over to {month}"),
            _ => "no longer over its token limit".to_string(),
        };
        let lifted = profile::update_overlay(q, &p.name, |ov| {
            let still = matches!(&ov.paused, Some(x) if x.by == PausedBy::Budget);
            if still {
                ov.paused = None;
            }
            still
        })?;
        if lifted {
            q.log(Event::new(actor, "agent.resume", entity.clone()).detail(why.clone()))?;
            // The request this pause opened asks about a pause that no
            // longer exists; left pending, a later pause would have reused
            // it with this one's window, month and figures.
            approval::close_open_requests(
                q,
                &p.name,
                &format!("the pause was lifted: {why}"),
                actor,
            )?;
            done.resumed = Some(why);
            o = profile::overlay(q, &p.name)?;
        }
    }

    match assess(p, &o, s, month) {
        Assessment::Ok => {}
        Assessment::Warn {
            window,
            spent,
            limit,
        } => {
            done.message = Some(format!(
                "agent `{}` has used {spent} of its {} limit of {limit} completion tokens \
                 ({:.0}%, warning at {:.0}%)",
                p.name,
                window.as_str(),
                spent as f64 * 100.0 / limit as f64,
                p.warn_at() * 100.0
            ));
        }
        Assessment::Exceeded {
            window,
            spent,
            limit,
        } => match o.paused.as_ref().map(|x| x.by) {
            // The transition into a budget pause: pause, and ask once.
            None => {
                let paused_now = profile::update_overlay(q, &p.name, |ov| {
                    if ov.paused.is_some() {
                        return false;
                    }
                    ov.paused = Some(Pause {
                        by: PausedBy::Budget,
                        at: crate::iso_now(),
                        reason: Some(format!("{spent} of {limit} {} tokens", window.as_str())),
                        window: Some(window),
                        month: (window == Window::Monthly).then(|| month.to_string()),
                    });
                    true
                })?;
                done.paused = true;
                if paused_now {
                    q.log(
                        Event::new(actor, "agent.pause", entity.clone()).detail(format!(
                            "budget: {spent} of {limit} {} completion tokens",
                            window.as_str()
                        )),
                    )?;
                    let req = approval::open_budget_request(
                        q, &p.name, window, month, spent, limit, actor,
                    )?;
                    done.message = Some(format!(
                        "agent `{}` reached its {} limit ({spent} of {limit} completion \
                         tokens) and is paused; its tasks are skipped. `trellis queue \
                         approve {req} --raise <tokens>` resumes it, `trellis queue reject \
                         {req}` keeps it paused",
                        p.name,
                        window.as_str()
                    ));
                    done.request = Some(req);
                }
            }
            // Already paused by its budget: nothing new to ask.
            Some(PausedBy::Budget) => {
                done.paused = true;
                done.request = approval::list(q, "pending")?
                    .into_iter()
                    .find(|r| r.agent == p.name)
                    .map(|r| r.id);
                done.message = Some(match &done.request {
                    Some(req) => format!(
                        "agent `{}` is over its {} limit ({spent} of {limit}) and paused; \
                         `trellis queue approve {req} --raise <tokens>` resumes it",
                        p.name,
                        window.as_str()
                    ),
                    None => format!(
                        "agent `{}` is over its {} limit ({spent} of {limit}) and stays \
                         paused: its override was rejected. Raising [budget] in \
                         .trellis/agents/{}.toml lifts the pause",
                        p.name,
                        window.as_str(),
                        p.name
                    ),
                });
            }
            // A person paused it; their pause stands and nothing is asked.
            Some(PausedBy::Human) => {}
        },
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(agent: &str, tokens: Option<u64>, at: &str) -> SpendRecord {
        SpendRecord {
            agent: agent.into(),
            tokens,
            prompt_tokens: None,
            started_at: at.into(),
            altered: false,
        }
    }

    fn prompted(agent: &str, tokens: Option<u64>, prompt: Option<u64>, at: &str) -> SpendRecord {
        SpendRecord {
            prompt_tokens: prompt,
            ..rec(agent, tokens, at)
        }
    }

    #[test]
    fn prompt_tokens_sum_apart_from_completion_across_a_month_boundary() {
        // The risk: prompt tokens folded into the completion sums (which
        // budgets limit), or attributed to the wrong month. Each figure is
        // chosen so any cross-addition lands on a number asserted against.
        let r = [
            prompted("a", Some(10), Some(1_000), "2026-09-30T23:59:59Z"),
            prompted("a", Some(5), Some(200), "2026-10-01T00:00:00Z"),
            prompted("a", Some(7), Some(30), "2026-10-15T12:00:00Z"),
            prompted("b", Some(1), Some(90_000), "2026-10-02T00:00:00Z"),
        ];
        let oct = spend(&r, "a", "2026-10");
        assert_eq!((oct.prompt_month, oct.prompt_lifetime), (230, 1_230));
        assert_eq!((oct.month, oct.lifetime), (12, 22), "completion unchanged");
        assert_eq!(oct.runs_without_prompt_count, 0);
        let sep = spend(&r, "a", "2026-09");
        assert_eq!((sep.prompt_month, sep.prompt_lifetime), (1_000, 1_230));
        assert_eq!(sep.month, 10);
    }

    #[test]
    fn prompt_tokens_unknown_is_counted_not_summed_as_zero() {
        // The risk: a run with no prompt figure read as a prompt of 0, so the
        // sum looks complete while under-counting. It must add nothing and be
        // counted, and it must not disturb its completion figure (nor a
        // missing completion figure disturb its prompt figure).
        let r = [
            prompted("a", Some(10), None, "2026-10-01T00:00:00Z"),
            prompted("a", None, Some(400), "2026-10-02T00:00:00Z"),
            prompted("a", Some(3), Some(50), "2026-10-03T00:00:00Z"),
            prompted("a", None, None, "2026-10-04T00:00:00Z"),
        ];
        let s = spend(&r, "a", "2026-10");
        assert_eq!((s.prompt_month, s.prompt_lifetime), (450, 450));
        assert_eq!(s.runs_without_prompt_count, 2);
        assert_eq!((s.month, s.lifetime), (13, 13));
        assert_eq!((s.runs, s.runs_without_count), (4, 2));
    }

    #[test]
    fn prompt_tokens_do_not_reach_budget_enforcement() {
        // The risk: a budget of completion tokens tripped by prompt tokens.
        // `assess` reads `month`/`lifetime` only; a large prompt spend with
        // no completion spend is still under a small limit.
        let p = crate::profile::parse(
            "rev",
            "[budget]
monthly_tokens = 100
",
        )
        .unwrap();
        let r = [prompted(
            "rev",
            Some(1),
            Some(1_000_000),
            "2026-10-01T00:00:00Z",
        )];
        let s = spend(&r, "rev", "2026-10");
        assert_eq!(
            assess(&p, &Overlay::default(), &s, "2026-10"),
            Assessment::Ok
        );
    }

    #[test]
    fn the_month_resets_at_utc_midnight_on_the_first() {
        // A run at 23:59:59Z on the 30th belongs to September even where
        // the local clock already says October; the window is UTC.
        let r = [
            rec("a", Some(10), "2026-09-30T23:59:59Z"),
            rec("a", Some(5), "2026-10-01T00:00:00Z"),
            rec("a", None, "2026-10-02T00:00:00Z"),
            rec("b", Some(1000), "2026-10-02T00:00:00Z"),
        ];
        let s = spend(&r, "a", "2026-10");
        assert_eq!(
            (s.month, s.lifetime, s.runs, s.runs_without_count),
            (5, 15, 3, 1)
        );
        assert_eq!(spend(&r, "a", "2026-09").month, 10);
    }
}
