//! Where a person's decision to trust a repository is kept, read back and
//! revoked (ADR 0023 §4). A persistence format with its own error type,
//! which changes for different reasons than the enumeration of what would
//! run and the mid-run guard in the parent module.

use super::Fingerprint;
// Named only by doc links in the item docs below.
#[cfg(doc)]
use super::{TrustGuard, TrustItem};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Whether a repository's current fingerprint is the one a person trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustState {
    Trusted,
    /// Never trusted, or revoked.
    Unknown,
    /// Trusted once, and something has changed since.
    Changed,
    /// Cannot be trusted as it stands, whatever the store says, and
    /// `trellis trust` would refuse too: `reason` names what to fix (a link
    /// under `.trellis/` that leaves it, for one). Reported before the
    /// store is consulted, so a person is never sent to a command that
    /// refuses.
    Refused {
        reason: String,
    },
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreFile {
    #[serde(default)]
    repos: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    /// The one authoritative digest: trust is given, and honoured, for
    /// exactly this [`Fingerprint::sha256`] and nothing else.
    sha256: String,
    /// Seconds since the Unix epoch.
    trusted_at: u64,
    /// [`Fingerprint::item_digests`] at the time of trust, for naming what
    /// changed and for nothing else (#136). Absent from an entry written
    /// before it was kept, which then reads exactly as it did.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    items: BTreeMap<String, String>,
}

/// One item that differs from what was trusted, by its label
/// ([`TrustItem::label`], written by the repository's configuration).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemChange {
    /// Declared then and now, with different contents.
    Changed(String),
    /// Declared now, not when trust was recorded.
    Added(String),
    /// Declared when trust was recorded, not now.
    Removed(String),
}

/// What [`TrustStore::changed_items`] can say about a repository's items
/// against its trust record. Informational: [`TrustStore::state`] alone
/// decides whether anything loads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustDiagnosis {
    /// No trust is recorded for this repository.
    NotRecorded,
    /// Trust was recorded before per-item digests were kept, so which item
    /// changed cannot be said.
    NoItemDigests,
    /// Each item that differs, by label. Empty when every item matches its
    /// recorded digest; the repository digest can still differ then (the
    /// same items declared in another order), which is not a match.
    Items(Vec<ItemChange>),
}

/// Why the trust store could not be read or written, or would not record a
/// decision. The messages are the ones these returned as strings before
/// (review round 10).
#[derive(Debug, thiserror::Error)]
pub enum TrustStoreError {
    #[error("no user configuration directory (APPDATA, XDG_CONFIG_HOME or HOME) to keep trust in")]
    NoConfigDir,
    #[error("{} is not a trust store: {source}", path.display())]
    NotAStore {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("cannot read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot create {}: {source}", path.display())]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot write {}: {source}", path.display())]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot replace {}: {source}", path.display())]
    Replace {
        path: PathBuf,
        source: std::io::Error,
    },
    /// [`Fingerprint::refused`]: the repository cannot be trusted as it
    /// stands.
    #[error("cannot trust this repository: {0}")]
    Refused(String),
}

/// A [`TrustGuard`] check that failed: what trusted code changed, or why it
/// can no longer be trusted. Prose composed from the fingerprints (which
/// files differ), not a closed set of cases, so one variant carrying it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct TrustChanged(pub String);

/// Where trust is recorded.
#[derive(Debug, Clone)]
pub struct TrustStore {
    path: PathBuf,
}

/// Set once per process by tests, so no test reads or writes the real
/// user's store. Not an environment variable: setting one in a test is
/// unsafe under edition 2024 and races every other test in the process.
static OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// Serializes read-modify-write in this process, so two writers cannot each
/// read the file, add their entry, and have the second erase the first.
static LOCK: Mutex<()> = Mutex::new(());

impl TrustStore {
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// Points [`user`](Self::user) at `path` for the rest of this process.
    /// For tests; the first call wins.
    pub fn use_for_this_process(path: PathBuf) {
        let _ = OVERRIDE.set(path);
    }

    /// The user's store: `%APPDATA%\trellis\trust.json` on Windows, else
    /// `$XDG_CONFIG_HOME/trellis/trust.json` or `~/.config/trellis/trust.json`.
    pub fn user() -> Result<Self, TrustStoreError> {
        if let Some(p) = OVERRIDE.get() {
            return Ok(Self::at(p.clone()));
        }
        let non_empty = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
        let dir = if cfg!(windows) {
            non_empty("APPDATA").map(PathBuf::from)
        } else {
            non_empty("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| non_empty("HOME").map(|h| PathBuf::from(h).join(".config")))
        };
        dir.map(|d| Self::at(d.join("trellis").join("trust.json")))
            .ok_or(TrustStoreError::NoConfigDir)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read(&self) -> Result<StoreFile, TrustStoreError> {
        match std::fs::read_to_string(&self.path) {
            Ok(s) => serde_json::from_str(&s).map_err(|source| TrustStoreError::NotAStore {
                path: self.path.clone(),
                source,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StoreFile::default()),
            Err(source) => Err(TrustStoreError::Read {
                path: self.path.clone(),
                source,
            }),
        }
    }

    fn write(&self, f: &StoreFile) -> Result<(), TrustStoreError> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|source| TrustStoreError::CreateDir {
                path: dir.to_path_buf(),
                source,
            })?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let body = serde_json::to_vec_pretty(f).expect("the store always serializes");
        std::fs::write(&tmp, body).map_err(|source| TrustStoreError::Write {
            path: tmp.clone(),
            source,
        })?;
        std::fs::rename(&tmp, &self.path).map_err(|source| TrustStoreError::Replace {
            path: self.path.clone(),
            source,
        })
    }

    /// Trusted only when the recorded repository digest is `fp.sha256`.
    /// The per-item digests beside it are not consulted: they say what
    /// changed, never that anything is trusted ([`changed_items`](Self::changed_items)).
    pub fn state(&self, root: &Path, fp: &Fingerprint) -> Result<TrustState, TrustStoreError> {
        if let Some(reason) = &fp.refused {
            return Ok(TrustState::Refused {
                reason: reason.clone(),
            });
        }
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        Ok(match self.read()?.repos.get(&store_key(root)) {
            None => TrustState::Unknown,
            Some(e) if e.sha256 == fp.sha256 => TrustState::Trusted,
            Some(_) => TrustState::Changed,
        })
    }

    /// Which items differ from the ones recorded when this repository was
    /// trusted. For `trellis trust --show` and `trellis doctor` to name what
    /// changed; never for deciding trust. Items trusted together are one
    /// decision, and a set recombined from items trusted at different times
    /// is a set nobody looked at, so a per-item match trusts nothing
    /// (ADR 0023, "Untrusted is per repository, not per item").
    pub fn changed_items(
        &self,
        root: &Path,
        fp: &Fingerprint,
    ) -> Result<TrustDiagnosis, TrustStoreError> {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let Some(entry) = self.read()?.repos.remove(&store_key(root)) else {
            return Ok(TrustDiagnosis::NotRecorded);
        };
        if entry.items.is_empty() {
            return Ok(TrustDiagnosis::NoItemDigests);
        }
        let now = fp.item_digests();
        let mut out = Vec::new();
        for (label, digest) in &now {
            match entry.items.get(label) {
                None => out.push(ItemChange::Added(label.clone())),
                Some(was) if was != digest => out.push(ItemChange::Changed(label.clone())),
                Some(_) => {}
            }
        }
        for label in entry.items.keys().filter(|l| !now.contains_key(*l)) {
            out.push(ItemChange::Removed(label.clone()));
        }
        Ok(TrustDiagnosis::Items(out))
    }

    pub fn trust(&self, root: &Path, fp: &Fingerprint) -> Result<(), TrustStoreError> {
        if let Some(why) = &fp.refused {
            return Err(TrustStoreError::Refused(why.clone()));
        }
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut f = self.read()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        f.repos.insert(
            store_key(root),
            Entry {
                sha256: fp.sha256.clone(),
                trusted_at: now,
                items: fp.item_digests(),
            },
        );
        self.write(&f)
    }

    /// Removes the entry. `Ok(false)` when there was none.
    pub fn revoke(&self, root: &Path) -> Result<bool, TrustStoreError> {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut f = self.read()?;
        let had = f.repos.remove(&store_key(root)).is_some();
        if had {
            self.write(&f)?;
        }
        Ok(had)
    }
}

/// The canonical repository root, so `C:\r` and `c:\r\.` are one entry.
fn store_key(root: &Path) -> String {
    std::fs::canonicalize(root)
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned()
}
