//! What a run may execute beyond its checks, decided once before the
//! snapshot (ADR 0023): the tool set, the hooks, and whether the user
//! trusted the repository to run them.
//!
//! Kept out of `cmd_run` so the one place that decides "repository code runs
//! only if trusted" is one function that can be read top to bottom.

use crate::config::{self, ControlMode, RepoConfig};
use anyhow::{Result, anyhow};
use std::path::Path;
use std::time::Duration;
use trellis_loop::{
    BashPolicyGate, BashSettings, Fingerprint, Hooks, ToolGate, ToolSelection, ToolSet, TrustState,
    TrustStore,
};

/// The run's tools and hooks, after trust.
pub(crate) struct Tooling {
    pub tools: ToolSet,
    /// `Some` only when trusted and at least one hook is declared.
    pub hooks: Option<Hooks>,
    /// Added to the environment of `bash`, custom tools and hooks.
    pub env: Vec<(String, String)>,
    /// For the `repo:` line when something was declared and not trusted,
    /// e.g. `hooks: 2 present, not trusted (run trellis trust)`.
    pub untrusted: Option<String>,
    /// For the `tools:` line when an agent profile narrowed the set.
    pub narrowed_by: Option<String>,
    /// `Some` exactly when trusted: re-checks the fingerprint before each
    /// hook and custom tool spawn, and after each call that may have changed
    /// the tree (ADR 0023 §5). The hooks hold a clone; the orchestrator
    /// must be given this one, so both see one latch.
    pub guard: Option<trellis_loop::TrustGuard>,
}

/// Everything in `[tools]` and `[hooks]` that would execute, as the trust
/// store fingerprints it. Empty when the repository declares none of it, in
/// which case the store is never opened.
pub(crate) fn fingerprint(root: &Path, cfg: &RepoConfig) -> Fingerprint {
    Inputs::of(cfg).fingerprint(root)
}

/// What [`fingerprint`] reads from the configuration, held apart so the
/// trust guard recomputes the fingerprint from the configuration the run
/// loaded, with every file re-hashed: the same inputs as the trusted
/// fingerprint, so an unchanged repository cannot read as changed.
#[derive(Clone)]
struct Inputs {
    hooks: trellis_loop::HookConfig,
    custom: Vec<trellis_loop::CustomTool>,
    bash: Option<(Vec<String>, String)>,
}

impl Inputs {
    fn fingerprint(&self, root: &Path) -> Fingerprint {
        self.fingerprint_cached(root, &trellis_loop::HashCache::default())
    }

    /// Through the trust guard's cache, which saves re-reading a file whose
    /// stat is unchanged; the trust decision itself uses a fresh one.
    fn fingerprint_cached(&self, root: &Path, cache: &trellis_loop::HashCache) -> Fingerprint {
        Fingerprint::of_cached(
            root,
            &self.hooks,
            &self.custom,
            self.bash.clone(),
            process_agents(root, cache),
            cache,
        )
    }

    fn of(cfg: &RepoConfig) -> Self {
        let bash = cfg.bash_requested().then(|| {
            let t = cfg.tools.as_ref();
            let shell = t.and_then(|t| t.shell.clone()).unwrap_or_default();
            let settings = format!(
                "bash = true\nshell = {:?}\nbash_allow = {:?}\nbash_unattended = {:?}\n\
             bash_timeout_s = {:?}",
                shell,
                t.and_then(|t| t.bash_allow.clone()).unwrap_or_default(),
                t.and_then(|t| t.bash_unattended).unwrap_or(false),
                t.and_then(|t| t.bash_timeout_s),
            );
            (shell, settings)
        });
        Self {
            hooks: cfg.hook_config(),
            custom: cfg.custom_tools().to_vec(),
            bash,
        }
    }
}

/// Each process agent's `command` (ADR 0026 §11), as a trust item: an argv
/// from `.trellis/agents/*.toml` that `work` runs in the worktree, which is
/// repository code as much as a hook is. A profile set that does not load
/// contributes nothing here; `work` refuses it before anything runs.
fn process_agents(root: &Path, cache: &trellis_loop::HashCache) -> Vec<trellis_loop::TrustItem> {
    trellis_queue::profile::load_all(root)
        .unwrap_or_default()
        .into_values()
        .filter_map(|p| {
            let argv = p.file.command.clone()?;
            let mut item = trellis_loop::TrustItem::command_cached(
                root,
                format!("process agent {}", p.name),
                &argv,
                format!("timeout_s = {:?}", p.file.timeout_s),
                cache,
            );
            // The keys it is handed are part of what is trusted, and shown
            // by name: the profile file's hash alone put them in the digest
            // without anyone being told.
            item.receives = p.file.pass_env.clone();
            Some(item)
        })
        .collect()
}

/// `Ok(())` when the repository's executable configuration is trusted,
/// else the sentence a waiting task shows. For `work`, before it runs a
/// process agent, and for the process runner itself, before it spawns.
pub(crate) fn require_trusted(root: &Path) -> std::result::Result<(), String> {
    const WHY: &str = "not trusted: run trellis trust";
    let cfg = RepoConfig::load(root).map_err(|e| format!("{e:#}"))?;
    let fp = fingerprint(root, &cfg);
    match TrustStore::user().and_then(|s| s.state(root, &fp)) {
        Ok(TrustState::Trusted) => Ok(()),
        Ok(TrustState::Changed) => Err(format!("{WHY} (changed since it was trusted)")),
        Ok(TrustState::Unknown) => Err(WHY.to_string()),
        // Not "run trellis trust": it would refuse. The reason names the fix.
        Ok(TrustState::Refused { reason }) => Err(format!("cannot be trusted: {reason}")),
        Err(e) => Err(format!("{WHY} (the trust store could not be read: {e})")),
    }
}

/// Decides what this run loads.
///
/// A repository that declares nothing needing trust never reaches the store.
/// One that does, and is not trusted, runs without it and says so; it is
/// not refused, because the run it can do safely is still worth doing.
pub(crate) fn prepare(root: &Path, cfg: &RepoConfig, run_id: &str, model: &str) -> Result<Tooling> {
    let hook_cfg = cfg.hook_config();
    hook_cfg.validate().map_err(|e| anyhow!(e))?;
    let fp = fingerprint(root, cfg);
    let mut refused: Option<String> = None;
    let trusted = if fp.is_empty() {
        false
    } else {
        match TrustStore::user().and_then(|s| s.state(root, &fp)) {
            Ok(TrustState::Trusted) => true,
            Ok(TrustState::Refused { reason }) => {
                refused = Some(reason);
                false
            }
            Ok(_) => false,
            // An unreadable store is not a trusted one.
            Err(e) => {
                tracing::warn!(error = %e, "trust store unreadable; treating as untrusted");
                false
            }
        }
    };

    let section = cfg.tools.as_ref();
    let bash = (trusted && cfg.bash_requested()).then(|| {
        let s = section.expect("bash_requested implies a [tools] section");
        BashSettings {
            shell: s
                .shell
                .clone()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(trellis_loop::default_shell),
            timeout: Duration::from_secs(
                s.bash_timeout_s
                    .unwrap_or(config::DEFAULT_BASH_TIMEOUT_S)
                    .max(1),
            ),
            allow: s.bash_allow.clone().unwrap_or_default(),
            unattended: s.bash_unattended.unwrap_or(false),
        }
    });
    let custom = if trusted {
        cfg.custom_tools().to_vec()
    } else {
        Vec::new()
    };
    // `disable` may name a custom tool the user has not trusted; that is not
    // a typo, so it is dropped rather than reported as an unknown tool.
    let declared: Vec<&str> = cfg.custom_tools().iter().map(|c| c.name.as_str()).collect();
    let loaded = |n: &String| trusted || !declared.contains(&n.as_str());
    let tools = ToolSet::build(ToolSelection {
        enable: section
            .and_then(|s| s.enable.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(loaded)
            .collect(),
        disable: section
            .and_then(|s| s.disable.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(loaded)
            .collect(),
        bash,
        custom,
    })
    .map_err(|e| anyhow!(e))?;

    let env = vec![
        ("TRELLIS_RUN_ID".to_string(), run_id.to_string()),
        ("TRELLIS_MODEL".to_string(), model.to_string()),
    ];
    // Trust was decided above, once; what it covers can be rewritten
    // during the run by `bash`, a custom tool or a hook, so it is checked
    // again before each of them starts (ADR 0023 §5).
    let guard = trusted.then(|| {
        let inputs = Inputs::of(cfg);
        let root = root.to_path_buf();
        trellis_loop::TrustGuard::over(fp.clone(), move |cache| {
            inputs.fingerprint_cached(&root, cache)
        })
    });
    let hooks = (trusted && hook_cfg.count() > 0).then(|| {
        let h = Hooks::new(root, hook_cfg.clone(), env.clone());
        match &guard {
            Some(g) => h.with_guard(g.clone()),
            None => h,
        }
    });

    let untrusted = (!fp.is_empty() && !trusted).then(|| {
        let mut parts = Vec::new();
        if hook_cfg.count() > 0 {
            parts.push(format!("hooks: {} present", hook_cfg.count()));
        }
        if !cfg.custom_tools().is_empty() {
            parts.push(format!(
                "custom tools: {} present",
                cfg.custom_tools().len()
            ));
        }
        if cfg.bash_requested() {
            parts.push("bash: requested".to_string());
        }
        let agents = fp
            .items
            .iter()
            .filter(|i| i.label.starts_with("process agent "))
            .count();
        if agents > 0 {
            parts.push(format!("process agents: {agents} present"));
        }
        match &refused {
            Some(reason) => format!("{}, cannot be trusted: {reason}", parts.join(", ")),
            None => format!("{}, not trusted (run trellis trust)", parts.join(", ")),
        }
    });

    Ok(Tooling {
        tools,
        hooks,
        env,
        untrusted,
        narrowed_by: None,
        guard,
    })
}

/// Checks each name in an agent profile's `tools` list against the tools
/// the repository could offer: every built-in, and every custom tool it
/// declares, trusted or not. An unknown name is an error, as in `[tools]`
/// (ADR 0023 §1): a typo that silently removed a tool would run the profile
/// without the tool its owner meant to keep.
pub(crate) fn check_profile_tools(profile: &str, list: &[String], cfg: &RepoConfig) -> Result<()> {
    for t in list {
        let custom = cfg.custom_tools().iter().any(|c| &c.name == t);
        if !trellis_loop::BUILTIN_TOOLS.contains(&t.as_str()) && !custom {
            anyhow::bail!(
                "agent `{profile}`: tools lists `{t}`, which is not a tool; the built-ins are \
                 {}{}",
                trellis_loop::BUILTIN_TOOLS.join(", "),
                if cfg.custom_tools().is_empty() {
                    String::new()
                } else {
                    format!(
                        ", and this repository declares {}",
                        cfg.custom_tools()
                            .iter()
                            .map(|c| c.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            );
        }
    }
    Ok(())
}

impl Tooling {
    /// Restricts the set to an agent profile's `tools` list (ADR 0026 §4,
    /// integrated with ADR 0023). Called once, before anything reads the
    /// set, so the prompt, the schema, the parser and the orchestrator all
    /// receive the narrowed one.
    pub(crate) fn narrow_for_profile(&mut self, profile: &str, keep: &[String]) {
        let (set, not_offered) = self.tools.narrowed(keep);
        self.tools = set;
        let mut note = format!("narrowed by agent profile `{profile}`");
        if !keep.iter().any(|k| k == "run_checks") {
            note.push_str("; run_checks kept though unlisted, since no run converges without it");
        }
        if !not_offered.is_empty() {
            note.push_str(&format!(
                "; not added, because this repository does not offer them: {}",
                not_offered.join(", ")
            ));
        }
        self.narrowed_by = Some(note);
    }

    /// The gates this adds, in order, to go after governance and before the
    /// approval gate: the bash policy (auto mode only, since in approve mode
    /// a person sees every command) and then the pre_tool hook.
    pub(crate) fn gates(&self, mode: ControlMode) -> Vec<Box<dyn ToolGate>> {
        let mut g: Vec<Box<dyn ToolGate>> = Vec::new();
        if let (ControlMode::Auto, Some(b)) = (mode, self.tools.bash()) {
            g.push(Box::new(BashPolicyGate::new(b.allow.clone(), b.unattended)));
        }
        if let Some(pre) = self.hooks.as_ref().and_then(Hooks::pre_tool_gate) {
            g.push(Box::new(pre));
        }
        g
    }

    /// The run's `tools:` line, when the set is not the measured default or
    /// hooks are loaded. Silent otherwise, so an unconfigured run prints
    /// exactly what it printed before.
    pub(crate) fn line(&self, mode: ControlMode) -> Option<String> {
        if self.tools.is_default() && self.hooks.is_none() && self.narrowed_by.is_none() {
            return None;
        }
        let mut s = self.tools.names().join(", ");
        if let Some(b) = self.tools.bash() {
            let policy = match mode {
                ControlMode::Approve => "each command asked".to_string(),
                ControlMode::Auto if b.unattended => "any command, unattended".to_string(),
                ControlMode::Auto if b.allow.is_empty() => {
                    "auto mode: every command refused (bash_allow is empty)".to_string()
                }
                ControlMode::Auto => format!("auto mode: only {}", b.allow.join(", ")),
            };
            s.push_str(&format!(
                "; bash via {} ({policy}, timeout {}s)",
                b.shell.join(" "),
                b.timeout.as_secs()
            ));
        }
        if let Some(h) = &self.hooks {
            let events: Vec<&str> = h.config().declared().iter().map(|(e, _)| *e).collect();
            s.push_str(&format!("; hooks: {}", events.join(", ")));
        }
        if let Some(n) = &self.narrowed_by {
            s.push_str(&format!("; {n}"));
        }
        Some(s)
    }
}

/// What `after_run` is given.
pub(crate) fn outcome_json(
    outcome: &trellis_loop::Outcome,
    touched: &[String],
    run_id: &str,
) -> serde_json::Value {
    let (converged, iterations, reason) = match outcome {
        trellis_loop::Outcome::Converged { iterations, .. } => (true, *iterations, None),
        trellis_loop::Outcome::NotConverged { reason, iterations } => {
            (false, *iterations, Some(reason.explain()))
        }
    };
    serde_json::json!({
        "run_id": run_id,
        "converged": converged,
        "iterations": iterations,
        "reason": reason,
        "touched": touched,
    })
}

/// What `after_run` is given when the run was interrupted (Ctrl-C or
/// `rpc`'s `abort`). The keys are `outcome_json`'s, so a hook reads one
/// schema. `iterations` is null: the loop's count lived in the future the
/// interrupt dropped, and a guessed number would be a false one.
pub(crate) fn interrupted_json(touched: &[String], run_id: &str) -> serde_json::Value {
    serde_json::json!({
        "run_id": run_id,
        "converged": false,
        "iterations": null,
        "reason": "interrupted",
        "touched": touched,
    })
}

#[cfg(test)]
mod run_tests {
    //! ADR 0023 through `cmd_run`: a committed repository, a scripted
    //! model, real hooks and a real rollback, and a trust store that is not
    //! the user's.

    use crate::config;
    use crate::{RunControl, RunSeeding, cmd_run};
    use std::path::{Path, PathBuf};
    use std::time::Duration;
    use trellis_loop::{TrustState, TrustStore};

    /// Every test here shares one store in the scratch directory; entries
    /// are keyed by repository root, and each test has its own. The store
    /// is `testutil`'s, emptied once per process there (#199).
    fn store() -> TrustStore {
        crate::testutil::trust_store()
    }

    /// A shell argv for `script`, as TOML, with literal strings so a Windows
    /// path's backslashes are not escapes.
    fn sh(script: &str) -> String {
        if cfg!(windows) {
            format!("['cmd', '/C', '{script}']")
        } else {
            format!("['sh', '-c', '{script}']")
        }
    }

    fn fixture(name: &str, config: &str, script: &str) -> (PathBuf, String) {
        let _ = store();
        let dir = crate::testutil::scratch(&format!("ext-{name}"));
        crate::testutil::git_init(&dir);
        std::fs::create_dir_all(dir.join(".trellis")).unwrap();
        std::fs::write(dir.join(".trellis/config.toml"), config).unwrap();
        std::fs::write(dir.join("a.py"), "def f():\n    return 1\n").unwrap();
        crate::testutil::git_commit_all(&dir, "init");
        let scripts = crate::testutil::scratch(&format!("ext-{name}-script"));
        let path = scripts.join("script.json");
        std::fs::write(&path, script).unwrap();
        (dir, format!("scripted:{}", path.display()))
    }

    fn run(dir: &Path, spec: &str) -> String {
        let mut out = Vec::new();
        let r = cmd_run(
            dir,
            "change f",
            spec,
            false,
            RunSeeding::default(),
            RunControl::default(),
            &mut out,
        );
        let said = String::from_utf8(out).unwrap();
        assert!(r.is_ok(), "{r:?}\n{said}");
        said
    }

    fn trail(dir: &Path) -> trellis_trail::UntrustedTrail {
        let runs = dir.join(".trellis").join("runs");
        let file = std::fs::read_dir(&runs)
            .unwrap()
            .filter_map(Result::ok)
            .find(|e| e.path().extension().is_some_and(|x| x == "json"))
            .expect("a trail was written");
        let raw = std::fs::read_to_string(file.path()).unwrap();
        let t = trellis_trail::UntrustedTrail::from_json(&raw).unwrap();
        // New action kinds must stay inside what the digest covers.
        assert_eq!(t.integrity(&raw), trellis_trail::Integrity::Intact);
        t
    }

    fn actions(t: &trellis_trail::UntrustedTrail) -> Vec<(String, String, String, bool)> {
        t.factual
            .actions
            .iter()
            .map(|a| (a.tool.clone(), a.target.clone(), a.outcome.clone(), a.wrote))
            .collect()
    }

    fn trust(dir: &Path) {
        crate::testutil::trust(dir);
    }

    const EDIT_THEN_STOP: &str = r#"{"steps":[
        {"tool":"edit","path":"a.py","old":"return 1","new":"return 2"},
        {"tool":"done","summary":"changed it"}
    ]}"#;

    #[test]
    fn an_untrusted_repository_s_hooks_never_run() {
        // Cloning a repository and running trellis in it must not execute
        // what the repository supplied. The marker is outside the
        // repository, so a rollback cannot be what removed it.
        let marker = crate::testutil::scratch("ext-untrusted-marker").join("ran.txt");
        let write_marker = sh(&format!("echo ran> {}", marker.display()));
        let config = format!("[hooks]\npre_tool = {write_marker}\nbefore_run = {write_marker}\n");
        let (dir, spec) = fixture("untrusted", &config, EDIT_THEN_STOP);
        let said = run(&dir, &spec);
        assert!(
            said.contains("hooks: 2 present, not trusted (run trellis trust)"),
            "{said}"
        );
        assert!(!marker.exists(), "an untrusted hook ran:\n{said}");
        let a = actions(&trail(&dir));
        assert_eq!(a[0].0, "edit");
        assert!(a[0].3, "no hook stood in the way: {a:?}");
    }

    #[test]
    fn a_trusted_pre_tool_denial_is_an_action_in_the_trail() {
        let deny = if cfg!(windows) {
            "echo frozen until release 1>&2 & exit 2"
        } else {
            "echo frozen until release >&2; exit 2"
        };
        let (dir, spec) = fixture(
            "trusted-deny",
            &format!("[hooks]\npre_tool = {}\n", sh(deny)),
            EDIT_THEN_STOP,
        );
        trust(&dir);
        let said = run(&dir, &spec);
        assert!(said.contains("hooks: pre_tool"), "{said}");
        assert!(!said.contains("not trusted"), "{said}");
        let a = actions(&trail(&dir));
        assert_eq!(a[0].0, "pre_tool", "{a:?}");
        assert_eq!(a[0].1, "edit a.py");
        assert!(a[0].2.contains("frozen until release"), "{a:?}");
        assert_eq!(a[1].0, "edit");
        assert!(!a[1].3, "a denied edit wrote nothing: {a:?}");
    }

    #[test]
    fn a_refused_repository_names_the_link_at_every_refusal_site_not_trellis_trust() {
        // The risk: once trusted, a repository with a link under `.trellis/`
        // that leaves it read "changed since it was trusted", and the
        // `repo:` line, `work`'s waiting reason and doctor all sent the
        // person to `trellis trust`, which then refused.
        let (dir, _) = fixture(
            "refused",
            &format!("[hooks]\npre_tool = {}\n", sh("exit 0")),
            EDIT_THEN_STOP,
        );
        trust(&dir);
        let outside = crate::testutil::scratch("ext-refused-outside");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(&outside, dir.join(".trellis/lib"));
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&outside, dir.join(".trellis/lib"));
        if let Err(e) = made {
            eprintln!("skipped: cannot create a symlink here: {e}");
            return;
        }
        let why = super::require_trusted(&dir).unwrap_err();
        assert!(why.contains(".trellis/lib"), "{why}");
        assert!(!why.contains("run trellis trust"), "{why}");
        let cfg = crate::config::RepoConfig::load(&dir).unwrap();
        let line = super::prepare(&dir, &cfg, "r", "m")
            .unwrap()
            .untrusted
            .unwrap();
        assert!(line.contains(".trellis/lib"), "{line}");
        assert!(!line.contains("run trellis trust"), "{line}");
        let doctor = crate::doctor_extra::trust_line(&dir, &cfg);
        assert!(
            doctor.contains(".trellis/lib") && doctor.contains("would refuse"),
            "{doctor}"
        );
    }

    #[test]
    fn a_change_to_a_trusted_hook_makes_it_untrusted_again() {
        let (dir, spec) = fixture(
            "changed",
            &format!("[hooks]\npre_tool = {}\n", sh("exit 2")),
            EDIT_THEN_STOP,
        );
        trust(&dir);
        std::fs::write(
            dir.join(".trellis/config.toml"),
            format!("[hooks]\npre_tool = {}\n", sh("exit 0")),
        )
        .unwrap();
        crate::testutil::git_commit_all(&dir, "change the hook");
        let said = run(&dir, &spec);
        assert!(said.contains("hooks: 1 present, not trusted"), "{said}");
    }

    #[test]
    fn hook_input_refusal_comes_before_the_snapshot_and_leaves_no_trail() {
        // The risk: a refusal read after the snapshot is a run that began,
        // rolled back and wrote a trail for work that never started. And an
        // untrusted `input` never runs, so it refuses nothing.
        let refuse = if cfg!(windows) {
            "echo not this task 1>&2 & exit 2"
        } else {
            "echo not this task >&2; exit 2"
        };
        let (dir, spec) = fixture(
            "input-refuses",
            &format!("[hooks]\ninput = {}\n", sh(refuse)),
            EDIT_THEN_STOP,
        );
        let said = run(&dir, &spec);
        assert!(said.contains("hooks: 1 present, not trusted"), "{said}");
        std::fs::remove_dir_all(dir.join(".trellis/runs")).unwrap();

        trust(&dir);
        let mut out = Vec::new();
        let r = cmd_run(
            &dir,
            "change f",
            &spec,
            false,
            RunSeeding::default(),
            RunControl::default(),
            &mut out,
        );
        let said = String::from_utf8(out).unwrap();
        let err = format!("{:#}", r.expect_err("exit 2 refuses the run"));
        assert!(err.contains("not this task"), "{err}");
        assert!(!said.contains("snapshot at"), "{said}");
        assert!(!dir.join(".trellis/runs").exists(), "no trail: {said}");
        assert!(
            std::fs::read_to_string(dir.join("a.py"))
                .unwrap()
                .contains("return 1")
        );
    }

    #[test]
    fn a_check_that_writes_under_dot_trellis_does_not_converge() {
        // The edge the guard left: it is checked after each call that may
        // change the tree, and a check is not one, so a checker writing
        // under `.trellis/` on the last turn converged with the judge
        // changed, and only `after_run` (whose failure is only logged) was
        // refused.
        let write = if cfg!(windows) {
            "echo covered> .trellis\\cov.txt"
        } else {
            "echo covered > .trellis/cov.txt"
        };
        let script = r#"{"steps":[
            {"tool":"edit","path":"a.py","old":"return 1","new":"return 2"},
            {"tool":"run_checks","paths":[],"kinds":["typecheck"]},
            {"tool":"done","summary":"checked"}
        ]}"#;
        let (dir, spec) = fixture(
            "check-writes-judge",
            &format!(
                "[checks]\ntypecheck = {}\n[hooks]\nafter_run = {}\n",
                sh(write),
                sh("exit 0")
            ),
            script,
        );
        trust(&dir);
        let said = run(&dir, &spec);
        assert!(said.contains("NOT CONVERGED"), "{said}");
        assert!(said.contains("Trusted code was changed"), "{said}");
        assert!(said.contains(".trellis/cov.txt"), "{said}");
    }

    #[test]
    fn a_hook_rewritten_mid_run_is_not_run_and_the_run_is_rolled_back() {
        // The risk, through `cmd_run`: trust was decided once before the
        // snapshot, and a `bash` command then rewrote a file under
        // `.trellis/hooks/`, which every later hook spawn would have run
        // under the person's trust. `prepare` must hand the one guard to
        // both the hooks and the orchestrator. `bash` runs in a clone (ADR
        // 0032) and a `.trellis/` write there is not carried back, so the
        // command names the repository's file by absolute path: the writer
        // the clone does not stop, and this guard does.
        let shell = if cfg!(windows) {
            "['cmd', '/C']"
        } else {
            "['sh', '-c']"
        };
        let (dir, spec) = fixture(
            "rewritten",
            &format!(
                "[tools]\nbash = true\nbash_unattended = true\nshell = {shell}\n\
                 [hooks]\npre_tool = {}\n",
                sh("exit 0")
            ),
            "",
        );
        let hook = dir.join(".trellis").join("hooks").join("gate.txt");
        let rewrite = if cfg!(windows) {
            format!("echo exit 0> {}", hook.display())
        } else {
            format!("echo exit 0 > '{}'", hook.display())
        };
        let script = serde_json::json!({"steps": [
            {"tool": "bash", "command": rewrite},
            {"tool": "edit", "path": "a.py", "old": "return 1", "new": "return 2"},
            {"tool": "done", "summary": "changed it"}
        ]});
        let script_path = spec.strip_prefix("scripted:").unwrap();
        std::fs::write(script_path, script.to_string()).unwrap();
        std::fs::create_dir_all(dir.join(".trellis/hooks")).unwrap();
        std::fs::write(&hook, "exit 2\n").unwrap();
        crate::testutil::git_commit_all(&dir, "a hook file");
        trust(&dir);
        let said = run(&dir, &spec);
        assert!(said.contains("NOT CONVERGED"), "{said}");
        assert!(said.contains("Trusted code was changed"), "{said}");
        assert!(said.contains(".trellis/hooks/gate.txt"), "{said}");
        // Rolled back: the hook file is as trusted, and the edit never ran.
        assert_eq!(
            std::fs::read_to_string(dir.join(".trellis/hooks/gate.txt")).unwrap(),
            "exit 2\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("a.py")).unwrap(),
            "def f():\n    return 1\n"
        );
    }

    #[test]
    fn unattended_bash_is_refused_outside_its_allowlist_and_the_trail_says_so() {
        let script = r#"{"steps":[
            {"tool":"bash","command":"echo fine"},
            {"tool":"bash","command":"echo fine && echo not"},
            {"tool":"done","summary":"ran"}
        ]}"#;
        let shell = if cfg!(windows) {
            "['cmd', '/C']"
        } else {
            "['sh', '-c']"
        };
        let (dir, spec) = fixture(
            "bash-policy",
            &format!("[tools]\nbash = true\nbash_allow = ['echo']\nshell = {shell}\n"),
            script,
        );
        trust(&dir);
        let said = run(&dir, &spec);
        assert!(said.contains("auto mode: only echo"), "{said}");
        let a = actions(&trail(&dir));
        assert_eq!((a[0].0.as_str(), a[0].2.as_str()), ("bash", "exit 0"));
        assert_eq!(a[1].0, "bash_policy", "{a:?}");
        assert!(a[1].2.contains("bash_allow"), "{a:?}");
        assert_eq!(a[2].0, "bash");
        assert!(a[2].2.starts_with("error: bash refused"), "{a:?}");
    }

    #[test]
    fn in_approve_mode_every_bash_command_is_put_to_the_person() {
        let script = r#"{"steps":[
            {"tool":"bash","command":"echo hi"},
            {"tool":"done","summary":"ran"}
        ]}"#;
        let (dir, spec) = fixture(
            "bash-approve",
            // `bash` is not in the approve list; approve mode adds it.
            "[control]\nmode = \"approve\"\n[tools]\nbash = true\nbash_unattended = true\n",
            script,
        );
        trust(&dir);
        let mut answers = vec!["n\n".to_string()];
        let human = crate::Human::Line(Box::new(move || answers.pop()));
        let mut out = Vec::new();
        let r = cmd_run(
            &dir,
            "change f",
            &spec,
            false,
            RunSeeding::default(),
            RunControl {
                flag: None,
                human,
                steering: None,
            },
            &mut out,
        );
        let said = String::from_utf8(out).unwrap();
        assert!(r.is_ok(), "{r:?}\n{said}");
        assert!(said.contains("control:  approve (edit, bash)"), "{said}");
        assert!(said.contains("echo hi"), "the command is shown: {said}");
        let t = trail(&dir);
        let a = actions(&t);
        assert_eq!(
            (a[0].0.as_str(), a[0].1.as_str(), a[0].2.as_str()),
            ("approval", "bash echo hi", "declined"),
            "{a:?}"
        );
        assert!(a[1].2.contains("declined by the user"), "{a:?}");
    }

    #[test]
    fn bash_is_told_the_run_id_the_trail_is_written_under() {
        // A hook or a command correlating on TRELLIS_RUN_ID has to find the
        // trail by it.
        let out = crate::testutil::scratch("ext-runid-out").join("id.txt");
        let cmd = if cfg!(windows) {
            format!("echo %TRELLIS_RUN_ID%> {}", out.display())
        } else {
            format!("echo $TRELLIS_RUN_ID > {}", out.display())
        };
        let script = serde_json::json!({ "steps": [
            { "tool": "bash", "command": cmd },
            { "tool": "done", "summary": "ran" }
        ]})
        .to_string();
        // The shell is named so the redirect means the same thing on every
        // platform; the default on Windows may be Git's bash.
        let shell = if cfg!(windows) {
            "['cmd', '/C']"
        } else {
            "['sh', '-c']"
        };
        let (dir, spec) = fixture(
            "runid",
            &format!("[tools]\nbash = true\nbash_unattended = true\nshell = {shell}\n"),
            &script,
        );
        trust(&dir);
        run(&dir, &spec);
        let id = std::fs::read_to_string(&out).unwrap();
        assert_eq!(Some(id.trim().to_string()), trail(&dir).run_id);
    }

    #[test]
    fn a_created_file_is_proposed_and_removed_by_the_rollback() {
        let script = r#"{"steps":[
            {"tool":"write_file","path":"pkg/new.py","content":"x = 1\n"},
            {"tool":"done","summary":"made it"}
        ]}"#;
        let (dir, spec) = fixture("write-file", "[tools]\nenable = ['write_file']\n", script);
        let said = run(&dir, &spec);
        assert!(said.contains("NOT CONVERGED"), "{said}");
        assert!(
            !dir.join("pkg/new.py").exists(),
            "rollback must remove what write_file created"
        );
        let t = trail(&dir);
        assert_eq!(t.factual.proposed, vec!["pkg/new.py"]);
        let a = actions(&t);
        assert_eq!(
            (a[0].0.as_str(), a[0].1.as_str(), a[0].3),
            ("write_file", "pkg/new.py", true)
        );
    }

    #[test]
    fn an_after_run_hook_that_writes_cannot_leave_bytes_in_a_converged_change() {
        // The risk: after_run runs after the last check, so a file it writes
        // sat in a "converged" change that no check had seen. The guard
        // fingerprints the worktree around it, demotes the run, and the
        // rollback takes the hook's bytes with the run's. The hook runs in a
        // throwaway clone (ADR 0032, "Hooks") and what it writes there is
        // dropped, so it names the repository's file by absolute path: the
        // writer the clone does not stop, and this guard does. The fixture's
        // directory is `scratch`'s, known before it is made.
        let script = r#"{"steps":[
            {"tool":"edit","path":"a.py","old":"return 1","new":"return 2"},
            {"tool":"run_checks","paths":[],"kinds":["typecheck"]},
            {"tool":"done","summary":"checked"}
        ]}"#;
        let late = std::env::temp_dir()
            .join(format!(
                "trellis-ext-after-run-writes-{}",
                std::process::id()
            ))
            .join("late.py");
        let config = format!(
            "[checks]\ntypecheck = {}\n[hooks]\nafter_run = {}\n",
            sh("exit 0"),
            sh(&format!("echo late> {}", late.display()))
        );
        let (dir, spec) = fixture("after-run-writes", &config, script);
        assert_eq!(
            dir.join("late.py"),
            late,
            "the hook names the fixture's file"
        );
        trust(&dir);
        let said = run(&dir, &spec);
        assert!(said.contains("NOT CONVERGED"), "{said}");
        assert!(
            said.contains("unchanged by the after_run hook (changed: late.py)"),
            "the demotion names the path the hook wrote: {said}"
        );
        assert!(
            !dir.join("late.py").exists(),
            "the hook's file must not survive:\n{said}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("a.py")).unwrap(),
            "def f():\n    return 1\n",
            "the run's own edit is rolled back with it"
        );
    }

    #[test]
    fn an_after_run_hook_that_only_observes_leaves_a_converged_run_alone() {
        let script = r#"{"steps":[
            {"tool":"edit","path":"a.py","old":"return 1","new":"return 2"},
            {"tool":"run_checks","paths":[],"kinds":["typecheck"]},
            {"tool":"done","summary":"checked"}
        ]}"#;
        let config = format!(
            "[checks]\ntypecheck = {}\n[hooks]\nafter_run = {}\n",
            sh("exit 0"),
            sh("exit 0")
        );
        let (dir, spec) = fixture("after-run-observes", &config, script);
        trust(&dir);
        let said = run(&dir, &spec);
        assert!(said.contains("CONVERGED after"), "{said}");
        assert!(!said.contains("NOT CONVERGED"), "{said}");
    }

    #[test]
    fn a_panic_inside_the_run_still_rolls_the_worktree_back() {
        // A panic in `block_on` unwound straight out of `cmd_run`, past the
        // rollback, and left the agent's edits in the tree: the one state a
        // run promises never to leave behind (ADR 0009).
        let config = format!(
            "[checks]\ntypecheck = {}\n[tools]\nenable = [\"write_file\"]\n",
            sh("exit 0")
        );
        let script = r#"{"steps":[
            {"tool":"edit","path":"a.py","old":"return 1","new":"return 2"},
            {"tool":"write_file","path":"new.py","content":"x = 1\n"},
            {"tool":"done","summary":"changed it"}
        ]}"#;
        let (dir, spec) = fixture("panic-rolls-back", &config, script);
        crate::PANIC_IN_RUN.with(|c| c.set(true));
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut out = Vec::new();
            cmd_run(
                &dir,
                "change f",
                &spec,
                false,
                RunSeeding::default(),
                RunControl::default(),
                &mut out,
            )
        }));
        crate::PANIC_IN_RUN.with(|c| c.set(false));
        assert!(r.is_err(), "the injected panic still propagates: {r:?}");
        assert_eq!(
            std::fs::read_to_string(dir.join("a.py")).unwrap(),
            "def f():\n    return 1\n",
            "the edit was rolled back"
        );
        assert!(!dir.join("new.py").exists(), "the new file was removed");
        assert!(
            trellis_vcs::snapshot(&dir).is_ok(),
            "the worktree is clean enough for the next run"
        );
    }

    // ---- ADR 0026 × 0023 / 0019, integrated: a profile narrows the run ----

    fn profile(name: &str, toml: &str) -> trellis_queue::profile::Profile {
        trellis_queue::profile::parse(name, toml).unwrap()
    }

    fn run_as(
        dir: &Path,
        spec: &str,
        p: &trellis_queue::profile::Profile,
    ) -> (anyhow::Result<crate::RunExit>, String) {
        let mut out = Vec::new();
        let r = cmd_run(
            dir,
            "change f",
            spec,
            false,
            RunSeeding {
                profile: Some(p),
                ..RunSeeding::default()
            },
            RunControl::default(),
            &mut out,
        );
        (r, String::from_utf8(out).unwrap())
    }

    #[test]
    fn a_profile_without_edit_cannot_edit_and_the_run_costs_nothing() {
        // The risk: the profile's list reaches the prompt but not the parser
        // or the orchestrator, so a reviewer profile still edits. The script
        // is parsed against the narrowed set, so the call is refused before
        // the snapshot.
        let (dir, spec) = fixture("profile-narrow", "", EDIT_THEN_STOP);
        let p = profile("reviewer", "tools = [\"read_file\"]\n");
        let (r, said) = run_as(&dir, &spec, &p);
        let e = format!("{:#}", r.unwrap_err());
        assert!(e.contains("edit"), "{e}\n{said}");
        assert!(!said.contains("snapshot at"), "{said}");
        assert_eq!(
            std::fs::read_to_string(dir.join("a.py")).unwrap(),
            "def f():\n    return 1\n"
        );
    }

    #[test]
    fn a_profile_cannot_widen_the_repository_s_tools_and_the_line_says_so() {
        // The risk: a committed profile switching on what the repository
        // left off. `grep` is opt-in and this repository did not opt in.
        let read_then_stop = r#"{"steps":[
            {"tool":"read_file","path":"a.py"},
            {"tool":"done","summary":"read it"}
        ]}"#;
        let (dir, spec) = fixture("profile-widen", "", read_then_stop);
        let p = profile("reader", "tools = [\"read_file\", \"grep\"]\n");
        let (r, said) = run_as(&dir, &spec, &p);
        assert!(r.is_ok(), "{r:?}\n{said}");
        let line = said
            .lines()
            .find(|l| l.starts_with("tools:"))
            .unwrap_or_else(|| panic!("no tools: line\n{said}"));
        assert!(line.contains("read_file, run_checks;"), "{line}");
        assert!(!line.contains("grep,") && !line.contains("edit"), "{line}");
        assert!(
            line.contains("narrowed by agent profile `reader`")
                && line.contains("not added, because this repository does not offer them: grep"),
            "{line}"
        );

        // An unknown name is an error, not a no-op, as in `[tools]`.
        let typo = profile("reader", "tools = [\"read_flie\"]\n");
        let (r, _) = run_as(&dir, &spec, &typo);
        assert!(format!("{:#}", r.unwrap_err()).contains("`read_flie`, which is not a tool"));
    }

    #[test]
    fn a_profile_s_denied_skill_is_refused_at_read_skill_in_a_real_run() {
        let script = r#"{"steps":[
            {"tool":"read_skill","name":"refactor"},
            {"tool":"done","summary":"read it"}
        ]}"#;
        let (dir, spec) = fixture("profile-skill", "", script);
        std::fs::create_dir_all(dir.join(".trellis/skills/refactor")).unwrap();
        std::fs::write(
            dir.join(".trellis/skills/refactor/SKILL.md"),
            "---\nname: refactor\ndescription: R.\n---\nBODY\n",
        )
        .unwrap();
        crate::testutil::git_commit_all(&dir, "skill");
        let p = profile("reviewer", "[skills]\ndeny = [\"refactor\"]\n");
        let (r, said) = run_as(&dir, &spec, &p);
        assert!(r.is_ok(), "{r:?}\n{said}");
        let a = actions(&trail(&dir));
        assert_eq!(a[0].0, "read_skill", "{a:?}");
        assert!(
            a[0].2.contains("denied to agent profile `reviewer`"),
            "{a:?}"
        );
    }

    /// `[[tools.custom]]` for a tool `nap` that waits about `secs` seconds,
    /// with `timeout` as its `timeout_s` line (empty for none).
    fn nap_tool(secs: u32, timeout: &str) -> String {
        // ping waits a second between its echoes; cmd has no sleep that
        // does not read the console.
        let argv = if cfg!(windows) {
            format!("['cmd', '/C', 'ping -n {} 127.0.0.1 >nul']", secs + 1)
        } else {
            format!("['sh', '-c', 'sleep {secs}']")
        };
        format!(
            "[[tools.custom]]\nname = \"nap\"\ndescription = \"Wait.\"\n\
             command = {argv}\nschema = {{ type = \"object\" }}\n{timeout}"
        )
    }

    const NAP_THEN_STOP: &str = r#"{"steps":[
        {"tool":"nap"},
        {"tool":"done","summary":"waited"}
    ]}"#;

    #[test]
    fn custom_tool_timeout_out_of_bounds_is_refused_like_bash_timeout_s_and_absent_is_120() {
        // The key mirrors `[tools] bash_timeout_s`: a value that key refuses
        // is refused here with the same words, and one that key accepts is
        // accepted. A custom tool that silently ran without a bound because
        // its timeout did not parse would be worse than a refused config.
        let d = crate::testutil::scratch("custom-timeout-bounds");
        std::fs::create_dir_all(d.join(".trellis")).unwrap();
        let load = |toml: &str| {
            std::fs::write(d.join(".trellis/config.toml"), toml).unwrap();
            crate::config::RepoConfig::load(&d)
        };
        for bad in ["-1", "1.5", "\"60\""] {
            let bash = format!(
                "{:#}",
                load(&format!("[tools]\nbash_timeout_s = {bad}\n")).unwrap_err()
            );
            let custom = format!(
                "{:#}",
                load(&nap_tool(1, &format!("timeout_s = {bad}\n"))).unwrap_err()
            );
            // The words after the location; the location names the key.
            let why = |e: &str| e.lines().last().unwrap_or_default().trim().to_string();
            assert!(!why(&bash).is_empty(), "{bash}");
            assert_eq!(why(&bash), why(&custom), "{bash}\n---\n{custom}");
        }

        let cfg = load(&nap_tool(1, "timeout_s = 0\n")).unwrap();
        // 0 is taken as 1 second, as `bash_timeout_s = 0` is.
        assert_eq!(cfg.custom_tools()[0].timeout(), Duration::from_secs(1));
        let cfg = load(&nap_tool(1, "")).unwrap();
        assert_eq!(cfg.custom_tools()[0].timeout_s, None);
        assert_eq!(
            cfg.custom_tools()[0].timeout(),
            Duration::from_secs(120),
            "an absent timeout_s is the 120 s every custom tool had before"
        );
        assert_eq!(
            trellis_loop::DEFAULT_CUSTOM_TIMEOUT_S,
            config::DEFAULT_BASH_TIMEOUT_S
        );
    }

    #[test]
    fn custom_tool_timeout_a_run_just_under_it_completes() {
        let (dir, spec) = fixture(
            "custom-timeout-under",
            &nap_tool(1, "timeout_s = 4\n"),
            NAP_THEN_STOP,
        );
        trust(&dir);
        let said = run(&dir, &spec);
        let a = actions(&trail(&dir));
        assert_eq!(a[0].0, "nap", "{a:?}\n{said}");
        assert!(!a[0].2.contains("did not finish"), "{a:?}\n{said}");
        assert!(!a[0].2.starts_with("error"), "{a:?}\n{said}");
    }

    #[test]
    fn custom_tool_timeout_a_run_just_over_it_is_killed_at_the_configured_bound() {
        // The risk: the key parses but the orchestrator still waits its
        // fixed 120 s, so a hung tool holds the run far past what the
        // repository asked for. The message names the configured bound.
        let (dir, spec) = fixture(
            "custom-timeout-over",
            &nap_tool(6, "timeout_s = 2\n"),
            NAP_THEN_STOP,
        );
        trust(&dir);
        let started = std::time::Instant::now();
        let said = run(&dir, &spec);
        let a = actions(&trail(&dir));
        assert_eq!(a[0].0, "nap", "{a:?}\n{said}");
        assert!(
            a[0].2.contains("did not finish within 2s and was killed"),
            "{a:?}\n{said}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the run waited {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn custom_tool_timeout_adding_the_key_changes_the_digest_and_needs_trust_again() {
        // How long repository code may run is part of what the person
        // trusted: raising it from 120 s to an hour must not ride on the
        // trust given to the shorter bound.
        let (dir, _) = fixture("custom-timeout-digest", &nap_tool(1, ""), NAP_THEN_STOP);
        trust(&dir);
        let state = |dir: &Path| {
            let cfg = crate::config::RepoConfig::load(dir).unwrap();
            store().state(dir, &super::fingerprint(dir, &cfg)).unwrap()
        };
        assert_eq!(state(&dir), TrustState::Trusted);
        let before = super::fingerprint(&dir, &crate::config::RepoConfig::load(&dir).unwrap());

        std::fs::write(
            dir.join(".trellis/config.toml"),
            nap_tool(1, "timeout_s = 3600\n"),
        )
        .unwrap();
        let after = super::fingerprint(&dir, &crate::config::RepoConfig::load(&dir).unwrap());
        assert_ne!(before.sha256, after.sha256);
        assert_eq!(state(&dir), TrustState::Changed);
        let why = super::require_trusted(&dir).unwrap_err();
        assert!(why.contains("changed since it was trusted"), "{why}");
    }
}
