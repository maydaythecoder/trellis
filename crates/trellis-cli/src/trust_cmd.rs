//! `trellis trust [--show | --revoke]` (ADR 0023 §4).
//!
//! Prints exactly what the repository would make trellis execute, then
//! records trust in the user's store, outside the repository. Typing the
//! command after reading its output is the consent; nothing else records it.

use crate::config::RepoConfig;
use anyhow::{Result, anyhow};
use std::io::Write;
use std::path::Path;
use trellis_loop::{Fingerprint, ItemChange, TrustDiagnosis, TrustState, TrustStore};

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
        if state == TrustState::Changed {
            writeln!(out, "items:   {}", changed_items(store, root, &fp))?;
        }
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

/// Which declared items differ from the ones recorded at trust, in words,
/// for a repository whose [`TrustState`] is `Changed`. Diagnosis only: the
/// state was decided by the one repository digest (ADR 0023). Labels come
/// from the repository's configuration, so they are printed through
/// [`crate::plain`].
pub(crate) fn changed_items(store: &TrustStore, root: &Path, fp: &Fingerprint) -> String {
    let changes = match store.changed_items(root, fp) {
        Ok(TrustDiagnosis::Items(changes)) => changes,
        Ok(TrustDiagnosis::NoItemDigests) => {
            return "which item changed is not known: trust was recorded before trellis \
                    kept a digest per item"
                .to_string();
        }
        Ok(TrustDiagnosis::NotRecorded) => {
            return "which item changed is not known: the trust record is gone".to_string();
        }
        Err(e) => return format!("which item changed is not known: {e}"),
    };
    let mut changed = Vec::new();
    let mut added = Vec::new();
    let mut removed = Vec::new();
    for c in &changes {
        match c {
            ItemChange::Changed(l) => changed.push(crate::plain(l)),
            ItemChange::Added(l) => added.push(crate::plain(l)),
            ItemChange::Removed(l) => removed.push(crate::plain(l)),
        }
    }
    let parts: Vec<String> = [("changed", changed), ("added", added), ("removed", removed)]
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(what, v)| format!("{what}: {}", v.join(", ")))
        .collect();
    if parts.is_empty() {
        // Every item matches its recorded digest and the repository digest
        // does not: declared in another order, for one.
        "no single item differs from its recorded digest (the same items declared in \
         another order differ only as a whole)"
            .to_string()
    } else {
        parts.join("; ")
    }
}

/// Printed when trust is recorded. One line, plain: what the person gives
/// up by trusting these, which no git check after the fact restores.
pub(crate) const ROLLBACK_LIMIT: &str = "Note: these run unsandboxed with your permissions and \
     can write anywhere in the repository, .git included, so rollback undoes trellis's own \
     edits but is best-effort for theirs (ADR 0023 §5).";

/// Each item, its argv, the files it names with their digests, and what else
/// decides what runs. Printed whole: a summary of what will execute is not
/// what will execute.
///
/// Labels, variable names, paths and detail all come from the repository,
/// and this is the text a person reads to consent: each goes through
/// [`crate::plain`], so an escape sequence cannot recolour, erase or
/// rewrite what is shown. A newline in a label, variable name or path is
/// escaped too; `detail` is multi-line by design and is escaped per line, so
/// a newline in it still starts a line (#192).
fn describe(fp: &Fingerprint, out: &mut dyn Write) -> Result<()> {
    writeln!(out, "This repository would make trellis execute:")?;
    for item in &fp.items {
        writeln!(out)?;
        writeln!(out, "  {}", crate::plain(&item.label))?;
        if !item.argv.is_empty() {
            // `{:?}` already escapes control characters; `plain` again
            // would double every backslash.
            writeln!(out, "    argv:  {:?}", item.argv)?;
        }
        if !item.receives.is_empty() {
            let receives: Vec<String> = item.receives.iter().map(|v| crate::plain(v)).collect();
            writeln!(
                out,
                "    receives: {}  (from your environment; this command can read, \
                 print or send them)",
                receives.join(", ")
            )?;
        }
        // The sha is computed by trellis, not read from the repository.
        for (path, sha) in &item.files {
            writeln!(out, "    file:  {}  sha256 {sha}", crate::plain(path))?;
        }
        for line in item.detail.lines() {
            writeln!(out, "    {}", crate::plain(line))?;
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

    /// Two hooks on the events #137 added and a custom tool with the
    /// `timeout_s` #135 added, each with its script under `.trellis/`.
    fn repo_with_three_items(lint_timeout: u32) -> tempfile::TempDir {
        let d = repo_with(&three_items_config(lint_timeout));
        for (path, body) in [
            (".trellis/hooks/turn.py", "print('turn')\n"),
            (".trellis/hooks/before.py", "print('before')\n"),
            (".trellis/tools/lint.py", "print('lint')\n"),
        ] {
            let p = d.path().join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        d
    }

    fn three_items_config(lint_timeout: u32) -> String {
        format!(
            "[hooks]\nturn_end = [\"python\", \".trellis/hooks/turn.py\"]\n\
             before_run = [\"python\", \".trellis/hooks/before.py\"]\n\
             [[tools.custom]]\nname = \"lint\"\ndescription = \"Lint.\"\n\
             command = [\"python\", \".trellis/tools/lint.py\"]\n\
             schema = {{ type = \"object\" }}\ntimeout_s = {lint_timeout}\n"
        )
    }

    fn show(root: &Path, store: &TrustStore) -> String {
        let mut out = Vec::new();
        run_with(root, store, true, false, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn doctor(root: &Path, store: &TrustStore) -> String {
        let cfg = RepoConfig::load(root).unwrap();
        crate::doctor_extra::trust_line_with(
            root,
            &cfg,
            Ok::<_, std::convert::Infallible>(store.clone()),
        )
    }

    fn fingerprint(root: &Path) -> Fingerprint {
        crate::tooling::fingerprint(root, &RepoConfig::load(root).unwrap())
    }

    #[test]
    fn trust_changed_items_are_named_by_show_and_doctor_and_an_untouched_one_is_not() {
        // The risk (#136): one digest per repository means one edit
        // untrusts every hook and tool, and "changed since it was trusted"
        // alone leaves a person diffing the whole of `trellis trust` output
        // by eye to find which one to review.
        let d = repo_with_three_items(30);
        let home = tempfile::tempdir().unwrap();
        let store = TrustStore::at(home.path().join("trust.json"));
        run_with(d.path(), &store, false, false, &mut Vec::new()).unwrap();

        std::fs::write(d.path().join(".trellis/hooks/turn.py"), "import os\n").unwrap();
        std::fs::write(
            d.path().join(".trellis/config.toml"),
            three_items_config(60),
        )
        .unwrap();

        let shown = show(d.path(), &store);
        assert!(shown.contains("state:   not trusted: changed"), "{shown}");
        let items = shown
            .lines()
            .find(|l| l.starts_with("items:"))
            .unwrap_or_else(|| panic!("an items line: {shown}"));
        let line = doctor(d.path(), &store);
        assert!(
            line.contains("NOT trusted: changed since trusted ("),
            "{line}"
        );
        for said in [items, line.as_str()] {
            assert!(said.contains("changed: "), "{said}");
            assert!(said.contains("hook turn_end"), "the edited script: {said}");
            assert!(said.contains("custom tool lint"), "its timeout_s: {said}");
            // Both edits are under `.trellis/`, which is its own item.
            assert!(said.contains(".trellis/ (every file)"), "{said}");
            assert!(!said.contains("before_run"), "untouched: {said}");
        }

        // Trust itself is still all or nothing: the untouched hook is not
        // trusted on its own.
        assert_eq!(
            store.state(d.path(), &fingerprint(d.path())).unwrap(),
            TrustState::Changed
        );
    }

    #[test]
    fn trust_changed_items_an_unchanged_repository_reports_nothing() {
        let d = repo_with_three_items(30);
        let home = tempfile::tempdir().unwrap();
        let store = TrustStore::at(home.path().join("trust.json"));
        run_with(d.path(), &store, false, false, &mut Vec::new()).unwrap();

        let shown = show(d.path(), &store);
        assert!(shown.contains("state:   trusted"), "{shown}");
        assert!(!shown.contains("items:"), "{shown}");
        assert!(!shown.contains("changed"), "{shown}");
        let line = doctor(d.path(), &store);
        assert!(line.ends_with("trusted; they load"), "{line}");
        assert!(!line.contains("changed"), "{line}");
        assert_eq!(
            store
                .changed_items(d.path(), &fingerprint(d.path()))
                .unwrap(),
            TrustDiagnosis::Items(Vec::new())
        );
    }

    #[test]
    fn trust_changed_items_a_store_written_before_them_is_still_honoured() {
        // The risk: the added field makes an existing trust.json unreadable
        // or untrusted, and every trusted repository silently stops loading
        // its hooks. The record below is the format as written before
        // #136: no per-item digests.
        let d = repo_with_three_items(30);
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("trust.json");
        let store = TrustStore::at(path.clone());
        let fp = fingerprint(d.path());
        let key = std::fs::canonicalize(d.path())
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let old = serde_json::json!({
            "repos": { key: { "sha256": fp.sha256, "trusted_at": 1_700_000_000u64 } }
        });
        std::fs::write(&path, serde_json::to_vec_pretty(&old).unwrap()).unwrap();

        assert_eq!(store.state(d.path(), &fp).unwrap(), TrustState::Trusted);
        assert!(show(d.path(), &store).contains("state:   trusted"));
        assert!(doctor(d.path(), &store).ends_with("trusted; they load"));
        assert_eq!(
            store.changed_items(d.path(), &fp).unwrap(),
            TrustDiagnosis::NoItemDigests
        );

        // Changed later, it is untrusted as before, and nobody is told a
        // guess about which item it was.
        std::fs::write(d.path().join(".trellis/hooks/turn.py"), "import os\n").unwrap();
        let shown = show(d.path(), &store);
        assert!(shown.contains("state:   not trusted: changed"), "{shown}");
        assert!(
            shown.contains("items:   which item changed is not known"),
            "{shown}"
        );
        assert!(!shown.contains("changed: hook"), "{shown}");
    }

    #[test]
    fn trust_changed_items_never_trust_what_the_repository_digest_does_not() {
        // The risk (ADR 0023, #136): per-item digests read as per-item
        // trust, so a set recombined from items trusted at different times,
        // or a record whose item digests match, loads code nobody reviewed
        // as a whole. Only the repository digest decides.
        let d = repo_with_three_items(30);
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("trust.json");
        let store = TrustStore::at(path.clone());
        run_with(d.path(), &store, false, false, &mut Vec::new()).unwrap();
        std::fs::write(d.path().join(".trellis/hooks/turn.py"), "import os\n").unwrap();
        let now = fingerprint(d.path());

        // Every item digest in the record now matches; the repository
        // digest is the one trusted before the edit.
        let mut rec: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let entry = rec["repos"]
            .as_object_mut()
            .unwrap()
            .values_mut()
            .next()
            .unwrap();
        entry["items"] = serde_json::to_value(now.item_digests()).unwrap();
        std::fs::write(&path, serde_json::to_vec_pretty(&rec).unwrap()).unwrap();
        assert_eq!(store.state(d.path(), &now).unwrap(), TrustState::Changed);
        assert_eq!(
            store.changed_items(d.path(), &now).unwrap(),
            TrustDiagnosis::Items(Vec::new()),
            "no item differs, and it is still not trusted"
        );
        let line = doctor(d.path(), &store);
        assert!(line.contains("NOT trusted"), "{line}");
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

    #[test]
    fn trust_describe_prints_repository_strings_inert_on_one_line() {
        // The risk (#190): labels, variable names, paths and detail come
        // from the repository, and `trellis trust` is what a person reads
        // before consenting. Printed raw, an escape sequence could hide or
        // recolour an item, and a newline could forge a line that looks
        // like trellis's own.
        let esc = '\u{1b}';
        let item = trellis_loop::TrustItem {
            label: format!("custom tool lint{esc}[2K\nhook fake"),
            argv: vec!["python".into(), format!("a{esc}b")],
            detail: format!("description = x{esc}[31m"),
            files: vec![(format!(".trellis/t{esc}[8m.py"), "ab".repeat(32))],
            receives: vec![format!("KEY{esc}[0m")],
        };
        let fp = Fingerprint {
            items: vec![item],
            sha256: "0".repeat(64),
            refused: None,
        };
        let mut out = Vec::new();
        describe(&fp, &mut out).unwrap();
        let shown = String::from_utf8(out).unwrap();

        assert!(
            !shown.contains(esc),
            "no raw escape reaches the terminal: {shown:?}"
        );
        assert!(
            shown
                .lines()
                .any(|l| l == "  custom tool lint\\u{1b}[2K\\nhook fake"),
            "the label, escaped, on one line: {shown}"
        );
        assert!(
            !shown
                .lines()
                .any(|l| l.trim_start().starts_with("hook fake")),
            "no forged line: {shown}"
        );
        assert!(shown.contains("receives: KEY\\u{1b}[0m"), "{shown}");
        assert!(
            shown.contains("file:  .trellis/t\\u{1b}[8m.py  sha256 "),
            "{shown}"
        );
        assert!(shown.contains("    description = x\\u{1b}[31m"), "{shown}");
        // argv keeps `{:?}`, escaped once, not twice.
        assert!(
            shown.contains(r#"argv:  ["python", "a\u{1b}b"]"#),
            "{shown}"
        );
    }
}
