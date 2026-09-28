//! The trust store (ADR 0023 §4).
//!
//! Cloning a repository and running trellis in it must not execute code the
//! repository supplied until a person has said so. The record of that lives
//! **outside** the repository, in the user's configuration directory, so a
//! repository cannot trust itself; and it is a digest over everything that
//! would run, so changing any of it (a hook's argv, a script a hook names, a
//! custom tool's schema, the bash settings) makes the repository untrusted
//! again.
//!
//! What trust does not cover is said aloud in the ADR: `[checks]` commands
//! have always run without it.

use crate::hooks::HookConfig;
use crate::registry::CustomTool;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// One thing that would execute, as shown by `trellis trust`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrustItem {
    /// e.g. `hook pre_tool`, `custom tool lint`, `bash`.
    pub label: String,
    /// The argv, when the item is a command.
    pub argv: Vec<String>,
    /// Anything else that decides what runs: a custom tool's description and
    /// schema, the bash settings.
    pub detail: String,
    /// Repository files the argv names, with their sha256.
    pub files: Vec<(String, String)>,
}

impl TrustItem {
    /// A command, with every argument that names a file inside `root`
    /// hashed. An argument is taken to name a file when it is a relative
    /// path with no `..`, or an absolute path under `root`, and a regular
    /// file is there. A program found on `PATH` is not repository code and is
    /// not hashed; a script the repository ships is.
    pub fn command(root: &Path, label: String, argv: &[String], detail: String) -> Self {
        let mut files = Vec::new();
        for arg in argv {
            let p = Path::new(arg);
            let full = if p.is_absolute() {
                if !p.starts_with(root) {
                    continue;
                }
                p.to_path_buf()
            } else {
                if arg.is_empty() || p.components().any(|c| c.as_os_str() == "..") {
                    continue;
                }
                root.join(p)
            };
            if let Ok(bytes) = std::fs::read(&full)
                && full.is_file()
            {
                let rel = full
                    .strip_prefix(root)
                    .unwrap_or(&full)
                    .to_string_lossy()
                    .replace('\\', "/");
                files.push((rel, hex(&Sha256::digest(&bytes))));
            }
        }
        Self {
            label,
            argv: argv.to_vec(),
            detail,
            files,
        }
    }
}

/// Everything a repository would execute, and one digest over all of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub items: Vec<TrustItem>,
    pub sha256: String,
}

impl Fingerprint {
    /// Items for the hooks, the custom tools, and the bash settings (given
    /// as text: whatever `[tools]` said about bash, rendered stably by the
    /// caller). An empty fingerprint means there is nothing to trust.
    pub fn of(
        root: &Path,
        hooks: &HookConfig,
        custom: &[CustomTool],
        bash: Option<(Vec<String>, String)>,
    ) -> Self {
        Self::of_with(root, hooks, custom, bash, Vec::new())
    }

    /// As [`of`](Self::of), with further items the binary knows about and
    /// this crate does not: process agents (ADR 0026 §11), whose profiles
    /// the queue crate reads.
    pub fn of_with(
        root: &Path,
        hooks: &HookConfig,
        custom: &[CustomTool],
        bash: Option<(Vec<String>, String)>,
        extra: Vec<TrustItem>,
    ) -> Self {
        let mut items = Vec::new();
        for (event, argv) in hooks.declared() {
            items.push(TrustItem::command(
                root,
                format!("hook {event}"),
                argv,
                format!("timeout_s = {:?}", hooks.timeout_s),
            ));
        }
        for c in custom {
            items.push(TrustItem::command(
                root,
                format!("custom tool {}", c.name),
                &c.command,
                format!("description = {:?}\nschema = {}", c.description, c.schema),
            ));
        }
        if let Some((shell, settings)) = bash {
            items.push(TrustItem::command(
                root,
                "bash".to_string(),
                &shell,
                settings,
            ));
        }
        items.extend(extra);
        let canonical = serde_json::to_vec(&items).expect("a list of strings always serializes");
        Self {
            sha256: hex(&Sha256::digest(&canonical)),
            items,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Whether a repository's current fingerprint is the one a person trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustState {
    Trusted,
    /// Never trusted, or revoked.
    Unknown,
    /// Trusted once, and something has changed since.
    Changed,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreFile {
    #[serde(default)]
    repos: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    sha256: String,
    /// Seconds since the Unix epoch.
    trusted_at: u64,
}

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
    pub fn user() -> Result<Self, String> {
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
            .ok_or_else(|| {
                "no user configuration directory (APPDATA, XDG_CONFIG_HOME or HOME) to keep \
                 trust in"
                    .to_string()
            })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read(&self) -> Result<StoreFile, String> {
        match std::fs::read_to_string(&self.path) {
            Ok(s) => serde_json::from_str(&s)
                .map_err(|e| format!("{} is not a trust store: {e}", self.path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StoreFile::default()),
            Err(e) => Err(format!("cannot read {}: {e}", self.path.display())),
        }
    }

    fn write(&self, f: &StoreFile) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let body = serde_json::to_vec_pretty(f).expect("the store always serializes");
        std::fs::write(&tmp, body).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| format!("cannot replace {}: {e}", self.path.display()))
    }

    pub fn state(&self, root: &Path, fp: &Fingerprint) -> Result<TrustState, String> {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        Ok(match self.read()?.repos.get(&key(root)) {
            None => TrustState::Unknown,
            Some(e) if e.sha256 == fp.sha256 => TrustState::Trusted,
            Some(_) => TrustState::Changed,
        })
    }

    pub fn trust(&self, root: &Path, fp: &Fingerprint) -> Result<(), String> {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut f = self.read()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        f.repos.insert(
            key(root),
            Entry {
                sha256: fp.sha256.clone(),
                trusted_at: now,
            },
        );
        self.write(&f)
    }

    /// Removes the entry. `Ok(false)` when there was none.
    pub fn revoke(&self, root: &Path) -> Result<bool, String> {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut f = self.read()?;
        let had = f.repos.remove(&key(root)).is_some();
        if had {
            self.write(&f)?;
        }
        Ok(had)
    }
}

/// The canonical repository root, so `C:\r` and `c:\r\.` are one entry.
fn key(root: &Path) -> String {
    std::fs::canonicalize(root)
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hooks(argv: &[&str]) -> HookConfig {
        HookConfig {
            pre_tool: Some(argv.iter().map(|s| s.to_string()).collect()),
            ..Default::default()
        }
    }

    #[test]
    fn a_change_to_a_script_a_hook_names_makes_the_repository_untrusted_again() {
        // The argv alone is not what runs; the script it names is. Trusting
        // `python hook.py` and then running a different hook.py would be
        // trust in a file name.
        let repo = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("hook.py"), "print('ok')\n").unwrap();
        let store = TrustStore::at(home.path().join("trust.json"));
        let h = hooks(&["python", "hook.py"]);

        let fp = Fingerprint::of(repo.path(), &h, &[], None);
        assert_eq!(fp.items[0].files.len(), 1, "the script is hashed");
        assert_eq!(store.state(repo.path(), &fp).unwrap(), TrustState::Unknown);
        store.trust(repo.path(), &fp).unwrap();
        assert_eq!(store.state(repo.path(), &fp).unwrap(), TrustState::Trusted);

        std::fs::write(repo.path().join("hook.py"), "import os; os.system('x')\n").unwrap();
        let fp2 = Fingerprint::of(repo.path(), &h, &[], None);
        assert_eq!(store.state(repo.path(), &fp2).unwrap(), TrustState::Changed);
    }

    #[test]
    fn a_change_to_any_definition_changes_the_digest() {
        let repo = tempfile::tempdir().unwrap();
        let base = Fingerprint::of(repo.path(), &hooks(&["a"]), &[], None);
        let other_argv = Fingerprint::of(repo.path(), &hooks(&["b"]), &[], None);
        let with_bash = Fingerprint::of(
            repo.path(),
            &hooks(&["a"]),
            &[],
            Some((vec!["sh".into(), "-c".into()], "unattended = false".into())),
        );
        let unattended = Fingerprint::of(
            repo.path(),
            &hooks(&["a"]),
            &[],
            Some((vec!["sh".into(), "-c".into()], "unattended = true".into())),
        );
        let custom = Fingerprint::of(
            repo.path(),
            &hooks(&["a"]),
            &[CustomTool {
                name: "lint".into(),
                description: "d".into(),
                command: vec!["x".into()],
                schema: serde_json::json!({ "type": "object" }),
            }],
            None,
        );
        let all = [&base, &other_argv, &with_bash, &unattended, &custom];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a.sha256, b.sha256);
            }
        }
    }

    #[test]
    fn a_file_outside_the_repository_is_not_taken_as_repository_code() {
        let repo = tempfile::tempdir().unwrap();
        let fp = Fingerprint::of(repo.path(), &hooks(&["python", "../evil.py"]), &[], None);
        assert!(fp.items[0].files.is_empty());
    }

    #[test]
    fn revoking_forgets_and_other_repositories_are_untouched() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let store = TrustStore::at(home.path().join("sub").join("trust.json"));
        let fa = Fingerprint::of(a.path(), &hooks(&["x"]), &[], None);
        let fb = Fingerprint::of(b.path(), &hooks(&["x"]), &[], None);
        store.trust(a.path(), &fa).unwrap();
        store.trust(b.path(), &fb).unwrap();
        assert!(store.revoke(a.path()).unwrap());
        assert!(!store.revoke(a.path()).unwrap());
        assert_eq!(store.state(a.path(), &fa).unwrap(), TrustState::Unknown);
        assert_eq!(store.state(b.path(), &fb).unwrap(), TrustState::Trusted);
    }

    #[test]
    fn nothing_declared_is_nothing_to_trust() {
        let repo = tempfile::tempdir().unwrap();
        assert!(Fingerprint::of(repo.path(), &HookConfig::default(), &[], None).is_empty());
    }
}
