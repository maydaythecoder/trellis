//! The only ways this crate writes a file in the git directory.
//!
//! Every whole-file write is [`publish`]: written under a unique dot-name,
//! then hard-linked to its final name, which fails when that name exists. A
//! reader never sees half a file and nothing is ever overwritten. There is
//! deliberately no `rename` anywhere (ADR 0020 §3: on Windows a rename let two
//! claimers both win, and renaming over an existing file was refused with
//! "Access is denied" in about one run in five).
//!
//! A file that must change in place — a finished task gaining a note, a
//! profile's pause overlay, a routine's last-fired time — is replaced under
//! an exclusive [`Lock`], itself a published file (ADR 0026 §2).

use crate::QueueError;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(crate) fn io(path: &Path) -> impl FnOnce(std::io::Error) -> QueueError + '_ {
    move |source| QueueError::Io {
        path: path.to_path_buf(),
        source,
    }
}

pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, QueueError> {
    let text = std::fs::read_to_string(path).map_err(io(path))?;
    serde_json::from_str(&text).map_err(|e| QueueError::Unreadable {
        path: path.to_path_buf(),
        error: e.to_string(),
    })
}

/// `Ok(None)` when the file does not exist; any other failure is an error.
pub(crate) fn read_json_opt<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, QueueError> {
    match read_json(path) {
        Ok(t) => Ok(Some(t)),
        Err(QueueError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

fn write_json<T: Serialize>(path: &Path, t: &T) -> Result<(), QueueError> {
    let mut json = serde_json::to_string_pretty(t).map_err(|e| QueueError::Unreadable {
        path: path.to_path_buf(),
        error: e.to_string(),
    })?;
    json.push('\n');
    std::fs::write(path, json).map_err(io(path))
}

/// Makes `dest` appear with `t` as its complete contents, or returns false
/// if `dest` already exists. Never overwrites.
pub(crate) fn publish<T: Serialize>(dest: &Path, t: &T) -> Result<bool, QueueError> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    // Unique per writer: two claimers racing for one id must not share a
    // temp file.
    let tmp = dest.with_file_name(format!(
        ".{name}.{}-{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    write_json(&tmp, t)?;
    let linked = patiently(|| std::fs::hard_link(&tmp, dest));
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(io(dest)(e)),
    }
}

/// Removes a file; one already gone is not an error.
pub(crate) fn remove(path: &Path) -> Result<(), QueueError> {
    match patiently(|| std::fs::remove_file(path)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io(path)(e)),
    }
}

/// Moves a file this process holds: publish at `to`, then remove `from`.
///
/// A crash between the two leaves the content complete in both places, which
/// `queue list` shows; it never leaves it in neither.
pub(crate) fn move_to<T: Serialize>(from: &Path, to: &Path, t: &T) -> Result<(), QueueError> {
    if !publish(to, t)? {
        return Err(io(to)(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "a file with this name is already there",
        )));
    }
    remove(from)
}

/// A file operation, retried for about five seconds on PermissionDenied.
///
/// On Windows a file another process has open without delete sharing cannot
/// be linked to, moved or removed for as long as it is open, and "open"
/// includes a virus scanner inspecting a file just written. Transient, so it
/// is waited out; anything longer is a real error and reported as one.
pub(crate) fn patiently<T>(mut op: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut waited = Duration::ZERO;
    let mut step = Duration::from_millis(10);
    loop {
        match op() {
            Err(e)
                if e.kind() == std::io::ErrorKind::PermissionDenied
                    && waited < Duration::from_secs(5) =>
            {
                std::thread::sleep(step);
                waited += step;
                step = (step * 2).min(Duration::from_millis(200));
            }
            r => return r,
        }
    }
}

/// An exclusive hold on one name, released on drop.
///
/// Taken by publishing `<locks>/<name>.lock`, so exactly one holder creates
/// it. A process that dies holding one leaves it behind; the next writer is
/// told which file to remove rather than guessing whether the holder lives —
/// the same trade ADR 0020 makes for stale claims.
#[derive(Debug)]
pub(crate) struct Lock {
    path: PathBuf,
}

#[derive(Serialize)]
struct LockBody {
    pid: u32,
    at: String,
}

impl Lock {
    pub(crate) fn take(locks: &Path, name: &str) -> Result<Lock, QueueError> {
        let path = locks.join(format!("{name}.lock"));
        // A second writer in another terminal usually finishes within
        // milliseconds; waiting briefly turns a collision into a delay
        // rather than an error. A lock older than that is reported.
        for _ in 0..50 {
            let body = LockBody {
                pid: std::process::id(),
                at: crate::iso_now(),
            };
            if publish(&path, &body)? {
                return Ok(Lock { path });
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Err(QueueError::Locked(path))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = remove(&self.path);
    }
}

/// Replaces the JSON at `path` with `f(current)` under the lock for `name`.
///
/// Remove-then-publish, because publish never overwrites. Under the lock no
/// other writer can interleave; a reader between the two steps sees no file,
/// which every reader of these files treats as "not written yet".
pub(crate) fn replace_locked<T, R>(
    locks: &Path,
    name: &str,
    path: &Path,
    f: impl FnOnce(Option<T>) -> Result<(T, R), QueueError>,
) -> Result<R, QueueError>
where
    T: Serialize + DeserializeOwned,
{
    let _lock = Lock::take(locks, name)?;
    let current = read_json_opt::<T>(path)?;
    let (next, r) = f(current)?;
    remove(path)?;
    if !publish(path, &next)? {
        return Err(io(path)(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "written by another process while this one held the lock",
        )));
    }
    Ok(r)
}
