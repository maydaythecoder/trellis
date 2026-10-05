//! What `trellis doctor` says about the inputs waves B, D and E added: the
//! user config in effect, context files, trust, profiles and routines, and
//! the vault.
//!
//! Each line reports what a run would find, without reading anything a run
//! would refuse on. A problem is printed on its line rather than failing the
//! command: `doctor` is where a person goes to find out what is wrong, so it
//! must get to the end.

use crate::config::RepoConfig;
use crate::{context_files, orchestrate, tooling, user_config};
use std::path::{Path, PathBuf};

/// The lines, in the order `doctor` prints them.
pub(crate) fn lines(root: &Path, cfg: &RepoConfig) -> Vec<String> {
    vec![
        user_config_line(),
        context_line(root, cfg),
        match user_config::skill_dirs() {
            Ok(dirs) => user_skills_line(root, &dirs),
            // Said on its line, so doctor still gets to the end.
            Err(e) => format!("user skills: not read: {e:#}"),
        },
        trust_line(root, cfg),
        agents_line(root),
        services_line(root),
        tz_line(),
        vault_line(root, cfg),
    ]
}

fn user_config_line() -> String {
    match user_config::file() {
        None => {
            "user cfg:   none (no TRELLIS_CONFIG_DIR, APPDATA, XDG_CONFIG_HOME or HOME)".to_string()
        }
        Some(f) if f.is_file() => format!(
            "user cfg:   {} — layered under the repository's (ADR 0025 §7)",
            crate::plain(&f.display().to_string())
        ),
        Some(f) => format!(
            "user cfg:   {} — not present; the repository's config alone",
            crate::plain(&f.display().to_string())
        ),
    }
}

fn context_line(root: &Path, cfg: &RepoConfig) -> String {
    let on = cfg.context.as_ref().and_then(|c| c.files).unwrap_or(true);
    let found = context_files::present(root, user_config::dir().as_deref());
    if found.is_empty() {
        return "context:    no context files and no system files".to_string();
    }
    let names = found
        .iter()
        .map(|(n, said)| format!("{} ({})", crate::plain(n), crate::plain(said)))
        .collect::<Vec<_>>()
        .join(", ");
    if on {
        format!("context:    {names}; the run's repo: line says which fit the cap")
    } else {
        format!(
            "context:    {names}; context files off ([context] files = false), system \
             files still apply"
        )
    }
}

/// Counted against this repository, as a run would load them: a user skill
/// whose name the repository already has is shadowed, not loaded.
fn user_skills_line(root: &Path, dirs: &[PathBuf]) -> String {
    let present: Vec<String> = dirs
        .iter()
        .filter(|d| d.is_dir())
        .map(|d| d.display().to_string())
        .collect();
    if present.is_empty() {
        return "user skills: none (the user's own; loaded without repository trust)".to_string();
    }
    let c = trellis_loop::SkillCatalog::discover_with(root, dirs);
    // `discover_with` names a user directory's invalid skill by its full
    // path; the repository's by its directory name alone.
    let invalid = c
        .invalid
        .iter()
        .filter(|i| present.iter().any(|p| i.dir.starts_with(p.as_str())))
        .count();
    format!(
        "user skills: {} loaded, {} shadowed by a skill of the same name found first, {invalid} invalid, from {} \
         (the user's own; loaded without repository trust)",
        c.user.len(),
        c.shadowed.len(),
        present
            .iter()
            .map(|p| crate::plain(p))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

pub(crate) fn trust_line(root: &Path, cfg: &RepoConfig) -> String {
    trust_line_with(root, cfg, trellis_loop::TrustStore::user())
}

/// [`trust_line`] against `store`, so a test can name its own.
pub(crate) fn trust_line_with(
    root: &Path,
    cfg: &RepoConfig,
    store: Result<trellis_loop::TrustStore, impl std::fmt::Display>,
) -> String {
    let fp = tooling::fingerprint(root, cfg);
    if fp.is_empty() {
        return "trust:      nothing declared needs trust (no hooks, custom tools or bash)"
            .to_string();
    }
    let mut what = Vec::new();
    let hooks = cfg.hook_config().count();
    if hooks > 0 {
        what.push(format!("{hooks} hook(s)"));
    }
    if !cfg.custom_tools().is_empty() {
        what.push(format!("{} custom tool(s)", cfg.custom_tools().len()));
    }
    if cfg.bash_requested() {
        what.push("bash".to_string());
    }
    let services = fp
        .items
        .iter()
        .filter(|i| i.label.starts_with("service "))
        .count();
    if services > 0 {
        what.push(format!("{services} service(s)"));
    }
    // By name, from the fingerprint so the line lists what the digest
    // covers: a repository whose only trusted item was a process agent
    // printed `trust:      : trusted`, consent with nothing shown. The name
    // is the profile's file stem, which the repository chose.
    let mut agents: Vec<String> = fp
        .items
        .iter()
        .filter_map(|i| i.label.strip_prefix("process agent "))
        .map(crate::plain)
        .collect();
    agents.sort();
    if !agents.is_empty() {
        what.push(format!(
            "{} process agent(s) ({})",
            agents.len(),
            agents.join(", ")
        ));
    }
    let state = match store.map_err(|e| e.to_string()).and_then(|s| {
        s.state(root, &fp)
            .map(|st| (s, st))
            .map_err(|e| e.to_string())
    }) {
        Ok((_, trellis_loop::TrustState::Trusted)) => "trusted; they load".to_string(),
        Ok((s, trellis_loop::TrustState::Changed)) => format!(
            "NOT trusted: changed since trusted ({}); they do not load until `trellis trust`",
            crate::trust_cmd::changed_items(&s, root, &fp)
        ),
        Ok((_, trellis_loop::TrustState::Unknown)) => {
            "NOT trusted; they do not load until `trellis trust`".to_string()
        }
        Ok((_, trellis_loop::TrustState::Refused { reason })) => {
            format!("NOT trusted, and `trellis trust` would refuse: {reason}")
        }
        Err(e) => format!("trust store unreadable ({e}); treated as not trusted"),
    };
    format!("trust:      {}: {state}", what.join(", "))
}

fn agents_line(root: &Path) -> String {
    match orchestrate::load_profiles(root) {
        Err(e) => format!("agents:     INVALID — {}", crate::plain(&format!("{e:#}"))),
        Ok(profiles) => {
            let routines = match orchestrate::load_routines(root, &profiles) {
                Ok(r) => format!("{} routine(s)", r.len()),
                Err(e) => format!("routines INVALID — {}", crate::plain(&format!("{e:#}"))),
            };
            format!("agents:     {} profile(s), {routines}", profiles.len())
        }
    }
}

/// Services whose record is still in the git directory and whose process is
/// still alive (ADR 0026 §12). A record lives while its task runs and is
/// removed once trellis has killed the service's tree and reaped it. So a
/// live one whose task is still claimed is most likely running as it should
/// (`work` is running that task), and one whose task is not claimed outlived
/// it: trellis was killed first, or the reap timed out. Only the service's
/// own pid is recorded; what it started is not listed.
fn services_line(root: &Path) -> String {
    let queue = match trellis_queue::Queue::for_repo(root) {
        Ok(q) => q,
        Err(e) => return format!("services:   not checked: {}", crate::plain(&e.to_string())),
    };
    let dir = queue.trellis_dir();
    let (alive, unreadable) = crate::services::outliving(&dir, &crate::liveness::SystemLiveness);
    // The task id comes from a file in the git directory; one that is not
    // an id's alphabet is not looked up by path.
    let claimed = |task: &str| {
        !task.is_empty()
            && task
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            && matches!(queue.find(task), Ok((trellis_queue::State::Claimed, _)))
    };
    let describe = |r: &crate::services::Record| {
        crate::plain(&format!(
            "pid {} ({}/{}, task {}: {})",
            r.pid,
            r.agent,
            r.service,
            r.task,
            r.argv.join(" ")
        ))
    };
    let (running, outlived): (Vec<_>, Vec<_>) = alive.iter().partition(|r| claimed(&r.task));
    let mut parts = Vec::new();
    if !outlived.is_empty() {
        parts.push(format!(
            "{} recorded service(s) alive after their task ended: {}; kill each (and what it \
             started), then delete its record in {}",
            outlived.len(),
            outlived
                .iter()
                .map(|r| describe(r))
                .collect::<Vec<_>>()
                .join("; "),
            crate::plain(&dir.join(crate::services::DIR).display().to_string())
        ));
    }
    if !running.is_empty() {
        parts.push(format!(
            "{} running for a task still claimed (expected while `work` runs it): {}",
            running.len(),
            running
                .iter()
                .map(|r| describe(r))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    if parts.is_empty() {
        parts.push("no recorded service outlived its task".to_string());
    }
    if !unreadable.is_empty() {
        parts.push(format!(
            "unreadable record(s): {}",
            unreadable
                .iter()
                .map(|u| crate::plain(u))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    format!("services:   {}", parts.join("; "))
}

/// Where a routine's IANA zone name is looked up (ADR 0026 §7). Bundled data
/// is as old as the build, so which source is in use is what a person needs
/// when a routine fires an hour off after a rule change.
fn tz_line() -> String {
    format!(
        "tz data:    {} — IANA names in routine schedules",
        crate::plain(&trellis_queue::cron::tz_source())
    )
}

fn vault_line(root: &Path, cfg: &RepoConfig) -> String {
    let dates = cfg.context.as_ref().and_then(|c| c.dates).unwrap_or(false);
    match cfg.vault(root, None) {
        Err(e) => format!("vault:      INVALID configuration — {e:#}"),
        Ok(None) => "vault:      not configured".to_string(),
        Ok(Some(v)) => match trellis_vault::validate(&v.dir) {
            Ok(stats) => format!(
                "vault:      {} valid ({} notes){}{}",
                crate::plain(&v.source),
                stats.notes,
                match &v.focus {
                    Some(f) => format!(", focus `{}`", crate::plain(f)),
                    None => ", no focus set".to_string(),
                },
                if dates { ", dates on" } else { "" }
            ),
            Err(trellis_vault::VaultError::Invalid { violations, .. }) => format!(
                "vault:      {} INVALID — {} violation(s); `trellis vault check` lists them",
                crate::plain(&v.source),
                violations.len()
            ),
            Err(e) => format!(
                "vault:      {} INVALID — {}",
                crate::plain(&v.source),
                crate::plain(&e.to_string())
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for (p, t) in files {
            let path = d.path().join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, t).unwrap();
        }
        d
    }

    fn all(d: &Path) -> String {
        let cfg = RepoConfig::load(d).unwrap();
        lines(d, &cfg).join("\n")
    }

    #[test]
    fn doctor_names_each_input_a_run_would_find() {
        // The risk: a run's inputs that are visible only on its repo: line,
        // after the snapshot, so a person has no way to ask beforehand.
        let d = repo(&[
            ("CLAUDE.md", "12345"),
            (".trellis/system.md", "role"),
            (
                ".trellis/agents/reviewer.toml",
                "role = \"reviewer\"\ntools = [\"read_file\"]\n",
            ),
        ]);
        let s = all(d.path());
        assert!(s.contains("user cfg:   none"), "{s}");
        assert!(s.contains("CLAUDE.md (5 B)"), "{s}");
        assert!(s.contains(".trellis/system.md (4 B)"), "{s}");
        assert!(
            s.contains("trust:      nothing declared needs trust"),
            "{s}"
        );
        assert!(s.contains("agents:     1 profile(s), 0 routine(s)"), "{s}");
        assert!(s.contains("vault:      not configured"), "{s}");
        // #47: which tz data a named routine zone is resolved against.
        assert!(s.contains("tz data:    TimeZoneDatabase("), "{s}");
    }

    #[test]
    fn user_skills_are_counted_against_the_repository_and_its_own_skills_are_not_among_them() {
        // The risk: doctor counting the repository's skills as the user's,
        // or a user skill as loaded when the repository shadows it.
        let d = repo(&[(
            ".trellis/skills/refactor/SKILL.md",
            "---\nname: refactor\ndescription: repo\n---\nb\n",
        )]);
        let user = tempfile::tempdir().unwrap();
        for (name, text) in [
            (
                "refactor",
                "---\nname: refactor\ndescription: user\n---\nb\n",
            ),
            ("mine", "---\nname: mine\ndescription: user\n---\nb\n"),
            ("broken", "no frontmatter\n"),
        ] {
            let s = user.path().join(name);
            std::fs::create_dir_all(&s).unwrap();
            std::fs::write(s.join("SKILL.md"), text).unwrap();
        }
        let missing = user.path().join("absent");
        let line = user_skills_line(d.path(), &[user.path().to_path_buf(), missing]);
        assert!(
            line.starts_with(
                "user skills: 1 loaded, 1 shadowed by a skill of the same name found first, 1 invalid"
            ),
            "{line}"
        );
        assert!(line.contains("loaded without repository trust"), "{line}");
        assert_eq!(
            user_skills_line(d.path(), &[]),
            "user skills: none (the user's own; loaded without repository trust)"
        );
    }

    #[test]
    fn declared_hooks_are_reported_untrusted_and_a_bad_profile_does_not_stop_doctor() {
        let d = repo(&[
            (".trellis/config.toml", "[tools]\nbash = true\n"),
            (".trellis/agents/bad.toml", "tools = [\"nope\"]\n"),
        ]);
        let s = all(d.path());
        // "NOT trusted", or, on a machine with no user directory at all,
        // "treated as not trusted": never "trusted; they load".
        assert!(s.contains("trust:      bash: "), "{s}");
        assert!(!s.contains("trusted; they load"), "{s}");
        assert!(s.contains("agents:     INVALID"), "{s}");
        assert!(s.contains("`nope`, which is not a tool"), "{s}");
    }

    #[test]
    fn an_invalid_vault_is_counted_and_a_valid_one_names_its_notes() {
        let d = repo(&[
            (
                ".trellis/config.toml",
                "[context]\nvault = \"notes\"\ndates = true\n",
            ),
            ("notes/a.md", "---\ntitle: \"A\"\nparent: null\n---\nbody\n"),
        ]);
        let s = all(d.path());
        assert!(s.contains("vault:      notes INVALID — 1 violation"), "{s}");
        std::fs::write(d.path().join("notes/manifest.json"), "[\n  \"a.md\"\n]\n").unwrap();
        let s = all(d.path());
        assert!(
            s.contains("vault:      notes valid (1 notes), no focus set, dates on"),
            "{s}"
        );
    }
}
