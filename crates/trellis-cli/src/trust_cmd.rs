//! `trellis trust [--show | --revoke]` (ADR 0023 §4).
//!
//! Prints exactly what the repository would make trellis execute, then
//! records trust in the user's store, outside the repository. Typing the
//! command after reading its output is the consent; nothing else records it.

use crate::config::RepoConfig;
use anyhow::{Result, anyhow};
use std::io::Write;
use std::path::Path;
use trellis_loop::{Fingerprint, TrustState, TrustStore};

pub(crate) fn run(root: &Path, show: bool, revoke: bool) -> Result<()> {
    let store = TrustStore::user().map_err(|e| anyhow!(e))?;
    run_with(root, &store, show, revoke, &mut std::io::stdout())
}

pub(crate) fn run_with(
    root: &Path,
    store: &TrustStore,
    show: bool,
    revoke: bool,
    out: &mut dyn Write,
) -> Result<()> {
    if revoke {
        let had = store.revoke(root).map_err(|e| anyhow!(e))?;
        writeln!(
            out,
            "{} ({})",
            if had {
                "trust revoked: hooks, custom tools and bash will not load"
            } else {
                "nothing to revoke: this repository was not trusted"
            },
            store.path().display()
        )?;
        return Ok(());
    }

    let cfg = RepoConfig::load(root)?;
    let fp = crate::tooling::fingerprint(root, &cfg);
    if fp.is_empty() {
        writeln!(
            out,
            "nothing to trust: .trellis/config.toml declares no hooks, no custom tools and \
             no bash, and no .trellis/agents profile names a command"
        )?;
        return Ok(());
    }
    describe(&fp, out)?;
    let state = store.state(root, &fp).map_err(|e| anyhow!(e))?;
    writeln!(out)?;
    writeln!(out, "digest:  {}", fp.sha256)?;
    writeln!(out, "store:   {}", store.path().display())?;
    if let Some(why) = &fp.refused {
        writeln!(out, "state:   cannot be trusted: {why}")?;
        if show {
            return Ok(());
        }
        return Err(anyhow!("cannot trust this repository: {why}"));
    }
    if show {
        writeln!(
            out,
            "state:   {}",
            match state {
                TrustState::Trusted => "trusted",
                TrustState::Unknown => "not trusted",
                TrustState::Changed => "not trusted: changed since it was trusted",
            }
        )?;
        return Ok(());
    }
    store.trust(root, &fp).map_err(|e| anyhow!(e))?;
    writeln!(
        out,
        "trusted. Any change to the above makes this repository untrusted again."
    )?;
    Ok(())
}

/// Each item, its argv, the files it names with their digests, and what else
/// decides what runs. Printed whole: a summary of what will execute is not
/// what will execute.
fn describe(fp: &Fingerprint, out: &mut dyn Write) -> Result<()> {
    writeln!(out, "This repository would make trellis execute:")?;
    for item in &fp.items {
        writeln!(out)?;
        writeln!(out, "  {}", item.label)?;
        if !item.argv.is_empty() {
            writeln!(out, "    argv:  {:?}", item.argv)?;
        }
        for (path, sha) in &item.files {
            writeln!(out, "    file:  {path}  sha256 {sha}")?;
        }
        for line in item.detail.lines() {
            writeln!(out, "    {line}")?;
        }
    }
    writeln!(out)?;
    writeln!(
        out,
        "Read the files listed above before trusting them. They run with your permissions, \
         and rollback does not undo what they do outside the worktree."
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_with(config: &str) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".trellis")).unwrap();
        std::fs::write(d.path().join(".trellis/config.toml"), config).unwrap();
        d
    }

    #[test]
    fn trust_shows_what_will_run_records_it_and_a_change_undoes_it() {
        let d = repo_with("[hooks]\npre_tool = [\"python\", \"pre.py\"]\n");
        std::fs::write(d.path().join("pre.py"), "import sys; sys.exit(0)\n").unwrap();
        let home = tempfile::tempdir().unwrap();
        let store = TrustStore::at(home.path().join("trust.json"));

        let mut out = Vec::new();
        run_with(d.path(), &store, true, false, &mut out).unwrap();
        let shown = String::from_utf8(out).unwrap();
        assert!(shown.contains("hook pre_tool"), "{shown}");
        assert!(shown.contains("\"pre.py\""), "{shown}");
        assert!(shown.contains("file:  pre.py  sha256 "), "{shown}");
        assert!(shown.contains("state:   not trusted"), "{shown}");
        assert!(
            !home.path().join("trust.json").exists(),
            "--show records nothing"
        );

        let mut out = Vec::new();
        run_with(d.path(), &store, false, false, &mut out).unwrap();
        assert!(String::from_utf8(out).unwrap().contains("trusted."));

        std::fs::write(d.path().join("pre.py"), "import os; os.system('x')\n").unwrap();
        let mut out = Vec::new();
        run_with(d.path(), &store, true, false, &mut out).unwrap();
        let shown = String::from_utf8(out).unwrap();
        assert!(shown.contains("changed since it was trusted"), "{shown}");

        let mut out = Vec::new();
        run_with(d.path(), &store, false, true, &mut out).unwrap();
        assert!(String::from_utf8(out).unwrap().contains("trust revoked"));
    }

    #[test]
    fn a_repository_that_declares_nothing_has_nothing_to_trust() {
        let d = repo_with("[control]\nmode = \"auto\"\n");
        let home = tempfile::tempdir().unwrap();
        let store = TrustStore::at(home.path().join("trust.json"));
        let mut out = Vec::new();
        run_with(d.path(), &store, false, false, &mut out).unwrap();
        assert!(String::from_utf8(out).unwrap().contains("nothing to trust"));
        assert!(!home.path().join("trust.json").exists());
    }
}
