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
                // Returned above, from the fingerprint, before `state`.
                TrustState::Refused { .. } => "cannot be trusted",
            }
        )?;
        return Ok(());
    }
    store.trust(root, &fp).map_err(|e| anyhow!(e))?;
    writeln!(
        out,
        "trusted. Any change to the above makes this repository untrusted again."
    )?;
    // Every item here is a hook, a custom tool, bash or a process agent:
    // each one writes with the user's authority, so trellis is no longer
    // the only writer and rollback is best-effort against it (ADR 0023 §5).
    writeln!(out, "{ROLLBACK_LIMIT}")?;
    Ok(())
}

/// Printed when trust is recorded. One line, plain: what the person gives
/// up by trusting these, which no git check after the fact restores.
pub(crate) const ROLLBACK_LIMIT: &str = "Note: these run unsandboxed with your permissions and \
     can write anywhere in the repository, .git included, so rollback undoes trellis's own \
     edits but is best-effort for theirs (ADR 0023 §5).";

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
        if !item.receives.is_empty() {
            writeln!(
                out,
                "    receives: {}  (from your environment; this command can read, \
                 print or send them)",
                item.receives.join(", ")
            )?;
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
        let d = repo_with("[hooks]\npre_tool = [\"python\", \".trellis/hooks/pre.py\"]\n");
        std::fs::create_dir_all(d.path().join(".trellis/hooks")).unwrap();
        std::fs::write(
            d.path().join(".trellis/hooks/pre.py"),
            "import sys; sys.exit(0)\n",
        )
        .unwrap();
        let home = tempfile::tempdir().unwrap();
        let store = TrustStore::at(home.path().join("trust.json"));

        let mut out = Vec::new();
        run_with(d.path(), &store, true, false, &mut out).unwrap();
        let shown = String::from_utf8(out).unwrap();
        assert!(shown.contains("hook pre_tool"), "{shown}");
        assert!(shown.contains("\".trellis/hooks/pre.py\""), "{shown}");
        assert!(
            shown.contains("file:  .trellis/hooks/pre.py  sha256 "),
            "{shown}"
        );
        assert!(shown.contains("state:   not trusted"), "{shown}");
        assert!(
            !home.path().join("trust.json").exists(),
            "--show records nothing"
        );

        let mut out = Vec::new();
        run_with(d.path(), &store, false, false, &mut out).unwrap();
        let recorded = String::from_utf8(out).unwrap();
        assert!(recorded.contains("trusted."), "{recorded}");
        assert!(
            recorded.contains(ROLLBACK_LIMIT),
            "recording trust in something that writes says what rollback no longer \
             guarantees: {recorded}"
        );

        std::fs::write(
            d.path().join(".trellis/hooks/pre.py"),
            "import os; os.system('x')\n",
        )
        .unwrap();
        let mut out = Vec::new();
        run_with(d.path(), &store, true, false, &mut out).unwrap();
        let shown = String::from_utf8(out).unwrap();
        assert!(shown.contains("changed since it was trusted"), "{shown}");

        let mut out = Vec::new();
        run_with(d.path(), &store, false, true, &mut out).unwrap();
        assert!(String::from_utf8(out).unwrap().contains("trust revoked"));
    }

    #[test]
    fn trust_refuses_a_hook_whose_code_is_outside_dot_trellis_and_says_where_to_move_it() {
        // Review round 11, item 4, at the command a person types: a script
        // named inside a larger expression is refused, not hashed and
        // hoped for, and nothing is recorded.
        let d = repo_with("[hooks]\npre_tool = [\"bash\", \"-lc\", \"python scripts/gate.py\"]\n");
        std::fs::create_dir_all(d.path().join("scripts")).unwrap();
        std::fs::write(d.path().join("scripts/gate.py"), "print('ok')\n").unwrap();
        let home = tempfile::tempdir().unwrap();
        let store = TrustStore::at(home.path().join("trust.json"));
        let mut out = Vec::new();
        let e = run_with(d.path(), &store, false, false, &mut out)
            .expect_err("refused")
            .to_string();
        assert!(e.contains("names `scripts/gate.py`"), "{e}");
        assert!(e.contains("move it to .trellis/hooks/"), "{e}");
        assert!(!home.path().join("trust.json").exists(), "nothing recorded");
        let shown = String::from_utf8(out).unwrap();
        assert!(!shown.contains(ROLLBACK_LIMIT), "{shown}");
    }

    #[test]
    fn trust_names_the_keys_a_process_agent_receives() {
        // The risk: `pass_env` hands a key from the user's environment to
        // repository code, and `trellis trust` showed the argv and the
        // timeout but not that. Consent to run a command is not consent to
        // give it a key, so the key is shown by name and is in the item.
        let d = repo_with("[control]\nmode = \"auto\"\n");
        std::fs::create_dir_all(d.path().join(".trellis/agents")).unwrap();
        std::fs::write(
            d.path().join(".trellis/agents/ext.toml"),
            "command = [\"python\", \"agent.py\"]\npass_env = [\"ANTHROPIC_API_KEY\"]\n",
        )
        .unwrap();
        let home = tempfile::tempdir().unwrap();
        let store = TrustStore::at(home.path().join("trust.json"));
        let mut out = Vec::new();
        run_with(d.path(), &store, true, false, &mut out).unwrap();
        let shown = String::from_utf8(out).unwrap();
        assert!(shown.contains("receives: ANTHROPIC_API_KEY"), "{shown}");

        let cfg = RepoConfig::load(d.path()).unwrap();
        let fp = crate::tooling::fingerprint(d.path(), &cfg);
        let item = fp
            .items
            .iter()
            .find(|i| i.label == "process agent ext")
            .expect("the agent is an item");
        assert_eq!(item.receives, ["ANTHROPIC_API_KEY"]);
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
