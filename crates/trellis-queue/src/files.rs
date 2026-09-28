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
//! profile's pause overlay, a routine's last-fired time — is [`replace`]d
//! under an exclusive [`Lock`], itself a published file (ADR 0026 §2).

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

/// Reads `path`, or — while a [`replace`] of it is between removing the old
/// copy and linking the new one, or was cut short there — the complete new
/// copy beside it. Canonical, then the side copy, then canonical again: the
/// replacer may link and remove the side copy between the first two reads.
pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, QueueError> {
    let text = match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            match std::fs::read_to_string(side(path)) {
                Ok(t) => Ok(t),
                Err(_) => std::fs::read_to_string(path),
            }
        }
        r => r,
    }
    .map_err(io(path))?;
    serde_json::from_str(&text).map_err(|e| QueueError::Unreadable {
        path: path.to_path_buf(),
        error: e.to_string(),
    })
}

/// Whether a reader of `path` finds a record there: the file itself or a
/// replacement cut short (see [`read_json`]).
pub(crate) fn present(path: &Path) -> bool {
    path.exists() || side(path).exists()
}

/// `dir/.name.next`: where [`replace`] puts the new content before the swap.
/// A fixed name, not a unique one, so a replacement cut short is found again
/// by whoever looks next.
pub(crate) fn side(path: &Path) -> PathBuf {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    path.with_file_name(format!(".{name}.next"))
}

/// The name `side` was derived from, for a directory listing: `x.json` for
/// `.x.json.next`.
pub(crate) fn unside(name: &str) -> Option<&str> {
    name.strip_prefix('.')?.strip_suffix(".next")
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
    let linked = patiently(|| link(&tmp, dest));
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(io(dest)(e)),
    }
}

/// `hard_link`: the one call in this crate that makes a name appear.
///
/// Tests can make the Nth call on their own thread fail, which is how a
/// failure or a crash between two steps of a multi-step write is reached.
fn link(src: &Path, dst: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if fail::tick() {
        return Err(std::io::Error::other("injected failure"));
    }
    std::fs::hard_link(src, dst)
}

#[cfg(test)]
pub(crate) mod fail {
    use std::cell::Cell;
    thread_local!(static AT: Cell<u32> = const { Cell::new(0) });

    /// Makes the `n`th link from now on this thread fail; 0 disarms.
    pub(crate) fn nth_link(n: u32) {
        AT.with(|c| c.set(n));
    }

    pub(super) fn tick() -> bool {
        AT.with(|c| match c.get() {
            0 => false,
            v => {
                c.set(v - 1);
                v == 1
            }
        })
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

/// Replaces the file at `path` with `t`. The caller holds the lock that
/// covers `path`.
///
/// There is no overwrite and no `rename` (ADR 0020 §3), so the name is
/// necessarily absent for a moment. The order keeps a complete copy on disk
/// at every instant:
/// 1. publish the new content at [`side`]`(path)` — the old copy is intact
/// 2. remove `path` — the new copy is intact beside it, and [`read_json`]
///    reads it
/// 3. link the side copy to `path`, then remove the side copy
///
/// A failure or a crash after 1 and before 2 leaves the old record in place,
/// and the edit did not happen. One after 2 leaves the new record beside the
/// name: readers see it, and [`settle`] — run by the next holder of the
/// lock — links it into place. Either way the record is never in neither.
pub(crate) fn replace<T: Serialize>(path: &Path, t: &T) -> Result<(), QueueError> {
    settle(path)?;
    let next = side(path);
    if !publish(&next, t)? {
        return Err(io(&next)(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "written by another process while this one held the lock",
        )));
    }
    remove(path)?;
    patiently(|| link(&next, path)).map_err(io(path))?;
    remove(&next)
}

/// Finishes or discards a [`replace`] of `path` that was cut short. The
/// caller holds the lock that covers `path`.
///
/// With `path` present the side copy never took effect (or already has),
/// and it goes. With `path` absent the side copy is the record, and it is
/// linked into place.
pub(crate) fn settle(path: &Path) -> Result<(), QueueError> {
    let next = side(path);
    if !next.exists() {
        return Ok(());
    }
    if !path.exists() {
        match patiently(|| link(&next, path)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io(path)(e)),
        }
    }
    remove(&next)
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

/// Replaces the JSON at `path` with `f(current)` under the lock for `name`,
/// through [`replace`], so no step leaves the file in neither place.
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
    settle(path)?;
    let current = read_json_opt::<T>(path)?;
    let (next, r) = f(current)?;
    replace(path, &next)?;
    Ok(r)
}
