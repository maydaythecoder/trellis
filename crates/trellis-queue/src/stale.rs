//! Releasing a claim whose holder is gone (ADR 0020 §5, amended for #59).
//!
//! Off by default. Under `[work] stale_claims = "list"` the heartbeat names
//! a task left in `claimed/` and does nothing else, as it always has. Under
//! `"release"` it moves such a claim back to `pending/`, but only when it
//! can tell the holder is dead: the claim was made on this machine (its
//! recorded hostname and OS are this one's) and its pid is not running.
//! Anything short of that — another host, no host recorded, a probe that
//! could not answer, a running pid — leaves the claim where it is, because
//! releasing a live claim runs one task twice.
//!
//! Nothing here fails a task. A claim is released at most once an hour, so a
//! task whose runs keep dying is not fed back to the heartbeat in a loop,
//! and at most [`MAX_STALE_RELEASES`] times (#352): the next dead holder
//! blocks the task in `pending/` instead, where it waits for a person.
//!
//! The probe and the clock are the caller's. This crate spawns nothing but
//! `git` through `trellis_vcs`; the binary, which already depends on
//! `trellis_core::secrets`, supplies the platform probe.

use crate::files::{move_to, present, read_json, remove};
use crate::{
    Actor, Blocked, Event, Queue, QueueError, State, Status, Task, parse_iso, secs_to_iso,
};

/// How long after one release the same task may be released again.
pub const RELEASE_THROTTLE_SECS: i64 = 3600;

/// How many times one task's stale claim is released. After that a dead
/// holder blocks it instead (ADR 0020 §5, amended for #352): a task that
/// kills every run it gets, by exhausting memory say, stops being run.
pub const MAX_STALE_RELEASES: u32 = 3;

/// `[work] stale_claims`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StaleClaims {
    /// Name stale claims; release nothing. The behaviour before #59.
    #[default]
    List,
    /// Release a claim whose holder is provably gone.
    Release,
}

impl StaleClaims {
    pub fn parse(s: &str) -> Option<StaleClaims> {
        match s {
            "list" => Some(StaleClaims::List),
            "release" => Some(StaleClaims::Release),
            _ => None,
        }
    }
}

/// Who is claiming: recorded on each claim made through a queue that has
/// one ([`Queue::with_claimant`]), so a later heartbeat can tell whether
/// the claim was made here and whether its holder still runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claimant {
    pub host: String,
    /// `std::env::consts::OS`. With the hostname, because WSL reports the
    /// Windows computer name by default and can see a Windows checkout: a
    /// Windows pid probed from Linux is always "not running".
    pub os: String,
    /// This process's start time as [`Liveness::probe`] reports it, when it
    /// could say. Compared against a running pid to tell the holder from a
    /// stranger that was given the same pid.
    pub started: Option<String>,
}

/// What a probe says about one pid on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// No process has this pid (or only a zombie does).
    Gone,
    /// A process has this pid. `started` is its start time in the probe's
    /// own format, when the probe could read it.
    Running { started: Option<String> },
    /// The probe could not answer. Treated as running.
    Unknown(String),
}

/// The platform probe. Implemented by the binary; faked in tests.
pub trait Liveness {
    /// This machine's hostname, or `None` when it cannot be read — in which
    /// case nothing is released.
    fn host(&self) -> Option<String>;
    /// This machine's OS, as recorded in [`Claimant::os`].
    fn os(&self) -> String {
        std::env::consts::OS.to_string()
    }
    fn probe(&self, pid: u32) -> Probe;
}

/// What a sweep did with one claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Swept {
    /// Moved back to `pending/`, and logged. The detail is what the log says.
    Released(String),
    /// Moved back to `pending/` as `blocked`, because it had already been
    /// released [`MAX_STALE_RELEASES`] times, and logged. The detail is what
    /// the log says.
    Blocked(String),
    /// Left in `claimed/`, with why.
    Kept(String),
}

impl Queue {
    /// Records `c` on every claim this queue makes from now on.
    pub fn with_claimant(mut self, c: Claimant) -> Queue {
        self.claimant = Some(c);
        self
    }

    pub(crate) fn claimant(&self) -> Option<&Claimant> {
        self.claimant.as_ref()
    }

    /// Looks at every task in `claimed/` and releases each one whose holder
    /// is provably gone. `now` is Unix seconds.
    pub fn release_stale_claims(
        &self,
        live: &dyn Liveness,
        now: i64,
    ) -> Result<Vec<(String, Swept)>, QueueError> {
        let mut out = Vec::new();
        for id in self.ids(State::Claimed)? {
            let swept = match read_json::<Task>(&self.path(State::Claimed, &id)) {
                Ok(t) => self.sweep_one(&id, &t, live, now)?,
                // Gone between the listing and the read: its holder finished.
                Err(QueueError::Io { source, .. })
                    if source.kind() == std::io::ErrorKind::NotFound =>
                {
                    continue;
                }
                Err(e) => Swept::Kept(format!("not readable: {e}")),
            };
            out.push((id, swept));
        }
        Ok(out)
    }

    fn sweep_one(
        &self,
        id: &str,
        t: &Task,
        live: &dyn Liveness,
        now: i64,
    ) -> Result<Swept, QueueError> {
        let Some(pid) = t.claimed_by_pid else {
            return Ok(Swept::Kept("no pid recorded".into()));
        };
        // A claim made before #59, or under "list", names no host, and a pid
        // means nothing without knowing whose machine it was counted on.
        let (Some(host), Some(os)) = (&t.claimed_host, &t.claimed_os) else {
            return Ok(Swept::Kept(
                "no host recorded with the claim, so its pid cannot be checked".into(),
            ));
        };
        let Some(here) = live.host() else {
            return Ok(Swept::Kept(
                "this machine's hostname could not be read".into(),
            ));
        };
        if *host != here || *os != live.os() {
            return Ok(Swept::Kept(format!(
                "claimed on {} ({}), not this machine",
                crate::plain(host),
                crate::plain(os)
            )));
        }
        match live.probe(pid) {
            Probe::Gone => {}
            // A reused pid can only make a dead holder look alive, never a
            // live one look dead, so a running pid keeps the claim whatever
            // it is. The start time only lets the message say which case
            // this is; releasing on a mismatch is a separate decision.
            Probe::Running { started } => {
                let reused = matches!(
                    (&started, &t.claimed_pid_started),
                    (Some(now_started), Some(then)) if now_started != then
                );
                return Ok(Swept::Kept(if reused {
                    format!(
                        "pid {pid} is running but started at a different time than the \
                         claimant, so it is probably another process; not released \
                         automatically"
                    )
                } else {
                    format!("pid {pid} is running")
                }));
            }
            Probe::Unknown(why) => {
                return Ok(Swept::Kept(format!(
                    "could not tell whether pid {pid} is running: {}",
                    crate::plain(&why)
                )));
            }
        }
        // Before the hourly limit, which holds back releases only: a block
        // runs nothing, so there is no loop for it to slow.
        if t.stale_releases >= MAX_STALE_RELEASES {
            return self.release_dead(id, t, pid, &here, now, Dead::Block);
        }
        if let Some(last) = t.stale_released_at.as_deref().and_then(parse_iso)
            && now - last < RELEASE_THROTTLE_SECS
        {
            return Ok(Swept::Kept(format!(
                "pid {pid} is not running, but this task was already released at {}; \
                 released at most once an hour",
                secs_to_iso(last)
            )));
        }
        self.release_dead(id, t, pid, &here, now, Dead::Release)
    }

    /// Moves a claim whose holder is dead back to `pending/`, under the
    /// task lock `requeue` takes, keeping `attempts`: as `todo` to run
    /// again, or as `blocked` once it has been released too often.
    fn release_dead(
        &self,
        id: &str,
        seen: &Task,
        pid: u32,
        host: &str,
        now: i64,
        how: Dead,
    ) -> Result<Swept, QueueError> {
        let _lock = self.lock_task(id)?;
        let from = self.path(State::Claimed, id);
        // Read again under the lock: a `requeue` or another heartbeat may
        // have released it, and someone may have claimed it since.
        let mut t: Task = match read_json(&from) {
            Ok(t) => t,
            Err(QueueError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Swept::Kept("left claimed/ while being looked at".into()));
            }
            Err(e) => return Err(e),
        };
        if t.claimed_by_pid != seen.claimed_by_pid || t.claimed_at != seen.claimed_at {
            return Ok(Swept::Kept("claimed again while being looked at".into()));
        }
        let age = t
            .claimed_at
            .as_deref()
            .and_then(parse_iso)
            .map_or_else(|| "unknown".to_string(), |at| format!("{}s", now - at));
        crate::clear_claim(&mut t);
        match how {
            Dead::Release => {
                t.status = Some(Status::Todo);
                t.stale_released_at = Some(secs_to_iso(now));
                t.stale_releases = t.stale_releases.saturating_add(1);
            }
            // Not `failed`: nothing here fails a task. Only the status and
            // its reason change, so the count and the last release stay for
            // the person who reads it.
            Dead::Block => {
                t.status = Some(Status::Blocked);
                t.blocked = Some(Blocked::Manual {
                    detail: format!(
                        "its claim's holder died after {MAX_STALE_RELEASES} stale-claim \
                         releases, so it was not released again; \
                         `trellis queue set {id} --status todo` gives it another run"
                    ),
                });
            }
        }
        let to = self.path(State::Pending, id);
        move_to(&from, &to, &t)?;
        // A holder the probe wrongly called dead may have finished between
        // the read and the move. Its record is the one that happened; the
        // pending copy would run the task a second time.
        if [State::Done, State::Failed, State::Cancelled]
            .into_iter()
            .any(|s| present(&self.path(s, id)))
        {
            remove(&to)?;
            return Ok(Swept::Kept(match how {
                Dead::Release => "its holder finished it while it was being released".into(),
                Dead::Block => "its holder finished it while it was being blocked".into(),
            }));
        }
        let detail = format!("pid {pid}, age {age}, host {}", crate::plain(host));
        let action = match how {
            Dead::Release => "stale_claim_released",
            Dead::Block => "stale_claim_blocked",
        };
        self.log(Event::new(Actor::Work, action, format!("task:{id}")).detail(&detail))?;
        Ok(match how {
            Dead::Release => Swept::Released(detail),
            Dead::Block => Swept::Blocked(detail),
        })
    }
}

/// What [`Queue::release_dead`] does with a claim whose holder is dead.
#[derive(Clone, Copy)]
enum Dead {
    Release,
    Block,
}
