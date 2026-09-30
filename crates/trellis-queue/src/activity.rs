//! The activity log: one JSON line per mutation (ADR 0026 §9).
//!
//! Appended, never published. Every other file in the git directory is
//! written whole and hard-linked into place so it is never seen half-written
//! and never overwritten; an append-only log meets the second half by
//! construction and the first by writing each line with one `write_all`
//! under `O_APPEND`. A reader skips a trailing line that does not parse
//! rather than failing on it.

use crate::QueueError;
use crate::files::io;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// Who made a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    /// A person, at a `trellis` command.
    Human,
    /// The heartbeat.
    Work,
    /// A routine firing.
    Routine,
    /// The model, through `ask_human`: the question a run ended on. Its
    /// own actor so a note written by a model is never read as a person's
    /// (ADR 0026 §1), and so the next run can find where the answers begin.
    Model,
}

impl Actor {
    pub fn as_str(self) -> &'static str {
        match self {
            Actor::Human => "human",
            Actor::Work => "work",
            Actor::Routine => "routine",
            Actor::Model => "model",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub at: String,
    pub actor: Actor,
    /// `task.add`, `task.finish`, `agent.pause`, `routine.fire`, …
    pub action: String,
    /// `task:<id>`, `agent:<name>`, `routine:<name>`, `approval:<id>`.
    pub entity: String,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Event {
    pub fn new(actor: Actor, action: &str, entity: impl Into<String>) -> Self {
        Event {
            at: crate::iso_now(),
            actor,
            action: action.to_string(),
            entity: entity.into(),
            run_id: None,
            detail: None,
        }
    }

    pub fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = Some(d.into());
        self
    }

    pub fn run(mut self, run_id: Option<String>) -> Self {
        self.run_id = run_id;
        self
    }
}

#[derive(Debug, Clone)]
pub struct ActivityLog {
    path: PathBuf,
}

impl ActivityLog {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        ActivityLog { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, e: &Event) -> Result<(), QueueError> {
        if let Some(p) = self.path.parent() {
            std::fs::create_dir_all(p).map_err(io(p))?;
        }
        let mut line = serde_json::to_string(e).map_err(|err| QueueError::Unreadable {
            path: self.path.clone(),
            error: err.to_string(),
        })?;
        line.push('\n');
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(io(&self.path))?;
        // One write per line: the whole record lands at the end or none of
        // it does, and two writers never interleave inside a line.
        f.write_all(line.as_bytes()).map_err(io(&self.path))
    }

    /// The newest `limit` events, oldest first. A line that will not parse
    /// is skipped: the log is a record for a person, and one torn line must
    /// not hide the rest.
    pub fn tail(&self, limit: usize) -> Result<Vec<Event>, QueueError> {
        let f = match std::fs::File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io(&self.path)(e)),
        };
        let mut all: Vec<Event> = std::io::BufReader::new(f)
            .lines()
            .map_while(Result::ok)
            .filter_map(|l| serde_json::from_str(&l).ok())
            .collect();
        let skip = all.len().saturating_sub(limit);
        Ok(all.split_off(skip))
    }
}
