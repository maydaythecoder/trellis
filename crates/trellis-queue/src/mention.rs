//! `@name` in a person's note wakes that agent profile's task (#57,
//! amending ADR 0026 §1).
//!
//! Only a note a person typed with `trellis queue note` counts: one whose
//! actor is [`Actor::Human`] *and* that carries an id, which only
//! [`Task::note_from_person`] gives. The actor alone is not enough: the
//! queue also writes `Human` notes on a person's behalf (a cancel reason,
//! an approval, a manual routine fire), and those are records, not
//! messages to an agent. Notes by any other actor — the heartbeat, a
//! routine, or a model — never wake anything, so one agent's output cannot
//! set another agent running (ADR 0005: agents parallelize work, and a
//! chain of wakes is one agent directing another).
//!
//! Each note is read once. Its `mentions_read` flag is set in the same
//! edit that wakes the task, so a second heartbeat, or the next tick, finds
//! nothing left to do; a mention that names no profile, or a paused one, is
//! logged and marked read too, rather than logged again every tick.

use crate::profile::{self, Profile};
use crate::{Actor, Event, Queue, QueueError, State, Status, Task, plain};
use std::collections::BTreeMap;
use std::io::Write;

/// What one mention in one note came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    pub task: String,
    /// The note's id (`Note::id`).
    pub note: String,
    /// The name after `@`.
    pub name: String,
    pub outcome: MentionOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MentionOutcome {
    /// The task was assigned to `name` and moved to `todo` in `pending/`.
    Woke,
    /// Nothing changed, for this reason.
    Ignored(String),
}

/// The `@name` tokens in `text`, in order, each once.
///
/// A token is a whitespace-separated word that starts with `@` (after any
/// opening bracket or quote) and whose remainder, less trailing
/// punctuation, is a valid profile name. So `@reviewer,` is a mention and
/// `ops@example.com` is not.
pub fn mentions(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let word = word.trim_start_matches(['(', '[', '"', '\'', '`']);
        let Some(rest) = word.strip_prefix('@') else {
            continue;
        };
        let name = rest.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '"', '\'', '`']);
        if profile::valid_name(name) && !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    }
    names
}

/// Whether a note is a person's, unread, and so may wake its task.
fn unread_from_person(n: &crate::Note) -> bool {
    n.actor == Actor::Human && n.id.is_some() && !n.mentions_read
}

impl Queue {
    /// Reads every unread person's note on every task not in `claimed/`,
    /// and for each `@name` in it: wakes the task for `name` if that names
    /// a profile in `profiles` that is not paused, and otherwise logs why
    /// not. A note that mentions several runnable profiles wakes the task
    /// for the first; the rest are logged as ignored. Returns every
    /// mention, each already logged.
    ///
    /// A task someone else holds right now is left, its notes unread, for
    /// the next tick, as [`Queue::reconcile`] leaves it.
    pub fn wake_mentions(
        &self,
        profiles: &BTreeMap<String, Profile>,
        actor: Actor,
    ) -> Result<Vec<Mention>, QueueError> {
        let mut all = Vec::new();
        for e in self.list()? {
            if e.state == State::Claimed {
                continue;
            }
            let Ok(t) = &e.task else { continue };
            for n in t.notes.iter().filter(|n| unread_from_person(n)) {
                let Some(note) = n.id.clone() else { continue };
                let names = mentions(&n.text);
                if names.is_empty() {
                    // Nothing to act on, so nothing to write: the file is
                    // left alone rather than rewritten every tick.
                    continue;
                }
                let decided = self.decide(&names, profiles)?;
                all.extend(self.apply(&e.id, &note, decided, actor)?);
            }
        }
        Ok(all)
    }

    /// What each name in one note would do, before anything is written.
    fn decide(
        &self,
        names: &[String],
        profiles: &BTreeMap<String, Profile>,
    ) -> Result<Vec<(String, MentionOutcome)>, QueueError> {
        let mut woke: Option<&str> = None;
        let mut out = Vec::new();
        for name in names {
            let outcome = match profiles.get(name) {
                None => MentionOutcome::Ignored(format!("no agent profile `{name}`")),
                Some(p) => match profile::paused_reason(p, &profile::overlay(self, name)?) {
                    Some(why) => MentionOutcome::Ignored(why),
                    None => match woke {
                        Some(first) => MentionOutcome::Ignored(format!(
                            "the same note already woke the task for `{first}`"
                        )),
                        None => {
                            woke = Some(name);
                            MentionOutcome::Woke
                        }
                    },
                },
            };
            out.push((name.clone(), outcome));
        }
        Ok(out)
    }

    /// Marks one note read and applies its wake, if any, in one edit; then
    /// logs each mention. Nothing is logged when the note turned out to be
    /// read already (another heartbeat got there first) or the task is held.
    fn apply(
        &self,
        id: &str,
        note: &str,
        decided: Vec<(String, MentionOutcome)>,
        actor: Actor,
    ) -> Result<Vec<Mention>, QueueError> {
        let wake = decided
            .iter()
            .find(|(_, o)| *o == MentionOutcome::Woke)
            .map(|(n, _)| n.clone());
        let mut applied = false;
        let r = self.edit(id, |t: &mut Task, _| {
            let Some(n) = t
                .notes
                .iter_mut()
                .find(|n| n.id.as_deref() == Some(note) && unread_from_person(n))
            else {
                return Ok(None);
            };
            n.mentions_read = true;
            applied = true;
            let Some(name) = &wake else { return Ok(None) };
            t.assignee = Some(name.clone());
            t.status = Some(Status::Todo);
            t.blocked = None;
            // A pending task has not ended; an old outcome on it would be
            // read as the next attempt's, as `requeue` says.
            t.outcome = None;
            t.note(actor, format!("woken: @{name} in note {note}"));
            Ok(Some(State::Pending))
        });
        match r {
            Ok(_) => {}
            Err(QueueError::Claimed { .. } | QueueError::Locked(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        }
        if !applied {
            return Ok(Vec::new());
        }
        let entity = format!("task:{id}");
        let mut out = Vec::new();
        for (name, outcome) in decided {
            let e = match &outcome {
                MentionOutcome::Woke => Event::new(actor, "mention_wake", entity.clone())
                    .detail(format!("note {note}: @{name}")),
                MentionOutcome::Ignored(why) => {
                    Event::new(actor, "mention_ignored", entity.clone())
                        .detail(format!("note {note}: @{name}: {why}"))
                }
            };
            self.log(e)?;
            out.push(Mention {
                task: id.to_string(),
                note: note.to_string(),
                name,
                outcome,
            });
        }
        Ok(out)
    }
}

/// Wakes, logged and printed. A scan that fails is reported and not fatal,
/// as a reconcile is: its notes stay unread and the next tick reads them.
pub(crate) fn wake_and_say(q: &Queue, profiles: &BTreeMap<String, Profile>, out: &mut dyn Write) {
    match q.wake_mentions(profiles, Actor::Work) {
        Ok(ms) => {
            for m in ms {
                let _ = match &m.outcome {
                    MentionOutcome::Woke => writeln!(
                        out,
                        "work:     {} -> todo for `{}` (@{} in note {})",
                        m.task, m.name, m.name, m.note
                    ),
                    MentionOutcome::Ignored(why) => writeln!(
                        out,
                        "work:     {}: @{} in note {} ignored: {}",
                        m.task,
                        m.name,
                        m.note,
                        plain(why)
                    ),
                };
            }
        }
        Err(e) => {
            let _ = writeln!(out, "work:     could not read @mentions in notes: {e}");
        }
    }
}
