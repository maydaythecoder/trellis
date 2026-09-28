//! The token ledger and budget enforcement (ADR 0026 §5).
//!
//! Spend is what trails recorded: completion tokens as the runtime adapter
//! reported them, per run, attributed to the profile the run executed as.
//! This crate does not read trails — it takes [`SpendRecord`]s, so the
//! arithmetic and the thresholds are tested without a repository, and the
//! binary is the one place that turns trail files into records.
//!
//! No dollars: COST_MODEL.md does not price tokens, and ADR 0021 excludes a
//! price until it does.

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
        let Some(t) = r.tokens else {
            s.runs_without_count += 1;
            continue;
        };
        s.lifetime = s.lifetime.saturating_add(t);
        if crate::month_of(&r.started_at).as_deref() == Some(month) {
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
    /// A budget pause was lifted: the month rolled over or the limit rose.
    pub resumed: bool,
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
            q.log(Event::new(actor, "agent.resume", entity.clone()).detail(why))?;
            done.resumed = true;
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
            started_at: at.into(),
            altered: false,
        }
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
