use super::*;

// ---------------------------------------------------------------------------
// init, and the repository gate
// ---------------------------------------------------------------------------

#[test]
fn init_scaffolds_and_a_second_init_changes_nothing() {
    let d = scratch("init");
    git(&d, &["init", "-q"]);
    let out = run(&d, &["init"]);
    assert!(out.status.success(), "{}", combined(&out));
    assert!(d.join(".trellis/config.toml").exists());

    std::fs::write(d.join(".trellis/config.toml"), "# mine\n").unwrap();
    assert!(run(&d, &["init"]).status.success());
    assert_eq!(
        std::fs::read_to_string(d.join(".trellis/config.toml")).unwrap(),
        "# mine\n",
        "init must never clobber a config someone tuned"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn commands_needing_a_repository_refuse_outside_one_and_name_the_fix() {
    // The failure a new user hits first. Naming `git init` is the difference
    // between a tool that seems broken and one that seems strict.
    let d = scratch("no-repo");
    for args in [vec!["index"], vec!["init"], vec!["check"]] {
        let out = run(&d, &args);
        assert!(!out.status.success(), "{args:?} succeeded outside a repo");
        let s = combined(&out);
        assert!(
            s.contains("git init") || s.contains("git repository"),
            "{args:?} must name the missing step: {s}"
        );
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn model_listing_works_outside_a_repository() {
    // Models are user state, not repository state. `trellis model` is how
    // someone gets a model *before* they have a project to use it on, so
    // gating it on a repository would invert the setup order.
    let d = scratch("no-repo-model");
    let out = run(&d, &["model"]);
    assert!(
        out.status.success(),
        "model list must not need a repository: {}",
        combined(&out)
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn vault_init_works_outside_a_repository_and_other_vault_actions_still_refuse() {
    // ADR 0022 §3, integrated: a vault may be created before there is a
    // repository to put it in. The risk on the other side is the exemption
    // widening to every `vault` action, which would let `vault check` read
    // whatever directory someone happened to be standing in.
    let d = scratch("no-repo-vault");
    let out = run(&d, &["vault", "init", "notes"]);
    assert!(out.status.success(), "{}", combined(&out));
    assert_eq!(
        std::fs::read_to_string(d.join("notes/manifest.json"))
            .unwrap()
            .trim(),
        "[]"
    );
    assert!(d.join("notes/layout.json").is_file());
    let out = run(&d, &["vault", "--vault", "notes", "check"]);
    assert!(!out.status.success(), "vault check needs a repository");
    let s = combined(&out);
    assert!(
        s.contains("git init") || s.contains("git repository"),
        "{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// init — what "Ready" is allowed to mean
// ---------------------------------------------------------------------------

#[test]
fn init_does_not_say_ready_when_a_prerequisite_is_missing() {
    // The strongest claim this command makes. Without a typechecker there is
    // no typecheck verdict, and without a verdict no run can converge — so
    // "Ready" over a missing tool is the command reporting success for work
    // it cannot do, which CLAUDE.md names as worse than saying it is not
    // built yet.
    let d = repo_with_checks(
        "init-missing",
        "trellis-no-such-typechecker",
        "trellis-no-such-runner",
    );
    let out = run(&d, &["init"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    assert!(s.contains("NOT FOUND"), "got:\n{s}");
    assert!(
        !s.contains("Ready."),
        "init called a repository ready with both checks missing:\n{s}"
    );
    assert!(
        s.contains("2 prerequisite(s) missing"),
        "and it has to say how many, not merely withhold the all-clear:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn init_counts_each_missing_prerequisite_rather_than_noticing_one() {
    // A count that saturates at one reads as a single problem and sends the
    // user back for a second round after they fix it.
    let d = repo_with_checks("init-one-missing", "git", "trellis-no-such-runner");
    let s = stdout(&run(&d, &["init"]));
    assert!(
        s.contains("1 prerequisite(s) missing"),
        "one missing tool must count as one:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn init_says_ready_when_both_checks_resolve() {
    // The other direction. A command that can never say "Ready" is as
    // useless as one that always does, and a guard asserted in one direction
    // can be inverted for free.
    let d = repo_with_checks("init-ready", "git", "git");
    let out = run(&d, &["init"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    assert!(s.contains("Ready."), "got:\n{s}");
    assert!(
        !s.contains("prerequisite(s) missing"),
        "nothing was missing, so nothing should be reported as missing:\n{s}"
    );
    // The *version the tool reported*, not merely that something was found.
    // Without this the line could carry any string — or an empty one — and
    // still read as present, which is the difference between "mypy 1.8.0"
    // and a blank the user cannot act on. Both checks are configured as
    // `git`, so both lines must carry git's own answer.
    assert_eq!(
        s.matches("git version").count(),
        2,
        "both check lines must show the version the command itself printed:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn init_reports_the_worktree_it_just_dirtied_instead_of_calling_it_clean() {
    // `init` writes into `.trellis/`, so by the time it prints this line the
    // tree it is describing is one it dirtied. Reporting "clean" there would
    // be wrong at the moment it is printed, and wrong in the direction that
    // matters: a run needs a clean tree so rollback is exact (ADR 0009), and
    // the user would find that out from the run refusing instead.
    let d = repo_with_checks("init-dirty", "git", "git");
    let s = stdout(&run(&d, &["init"]));
    assert!(
        s.contains("uncommitted change(s)"),
        "init wrote files and then called the tree clean:\n{s}"
    );
    assert!(
        !s.contains("worktree   clean"),
        "both lines cannot be true at once:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn init_calls_a_committed_worktree_clean() {
    // And the inverse, with everything init wrote committed. Without this
    // the line could be hardwired to "dirty" and still pass above.
    let d = repo_with_checks("init-clean", "git", "git");
    assert!(run(&d, &["init"]).status.success());
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "scaffold"]);

    let s = stdout(&run(&d, &["init"]));
    assert!(s.contains("worktree   clean"), "got:\n{s}");
    assert!(
        !s.contains("uncommitted change(s)"),
        "nothing is uncommitted, so nothing should be reported as such:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// trellis init behind a link (#723)
// ---------------------------------------------------------------------------

/// The names in `dir`, sorted; empty when it does not exist.
fn entries(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// `trellis init` refused, naming the link and why.
fn assert_init_refused(o: &Output, link: &str) {
    let all = combined(o);
    assert!(!o.status.success(), "init went ahead through a link: {all}");
    assert!(all.contains("symbolic link"), "the refusal says why: {all}");
    // The refusal prints the path as the OS spells it, so a `/` in `link`
    // is a `\\` on Windows.
    let native = link.replace('/', std::path::MAIN_SEPARATOR_STR);
    assert!(
        all.contains(link) || all.contains(&native),
        "and names the path {link}: {all}"
    );
    assert!(all.contains("nothing was written"), "{all}");
}

#[test]
fn init_symlink_dangling_file_links_create_nothing_outside() {
    // #723's reproduction: `exists()` follows a link, so a dangling one read
    // as absent and `fs::write` created the template at the link's target,
    // outside the repository. One case per file init creates.
    for rel in ["config.toml", "context.md", "governance/README.md"] {
        let slug = rel.replace(['/', '.'], "-");
        let outside = scratch(&format!("init-symlink-dangling-outside-{slug}"));
        let d = empty_repo(&format!("init-symlink-dangling-{slug}"));
        let link = d.join(".trellis").join(rel);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        if !link_or_skip(&outside.join("planted"), &link) {
            return;
        }

        let o = run(&d, &["init"]);
        assert_init_refused(&o, rel);
        assert!(
            entries(&outside).is_empty(),
            "{rel}: the dangling link's target was created: {:?}",
            entries(&outside)
        );
        assert!(is_link(&link), "{rel}: the link itself is left as it was");
        // Nothing beside it either: the check runs before any file is made.
        let made: Vec<_> = ["config.toml", "context.md", "governance/README.md"]
            .into_iter()
            .filter(|f| *f != rel && std::fs::symlink_metadata(d.join(".trellis").join(f)).is_ok())
            .collect();
        assert!(made.is_empty(), "{rel}: a refusal still wrote {made:?}");
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&outside);
    }
}

#[test]
fn init_symlink_config_toml_to_an_outside_file_leaves_it_untouched() {
    // A live link: never overwritten before either, but kept as if it were
    // the repository's own file, so `doctor` and every run read a config
    // trellis cannot see the source of. Refused, and the target is as it was.
    let outside = scratch("init-symlink-live-outside");
    let target = outside.join("rc.sh");
    std::fs::write(&target, "echo original\n").unwrap();
    let d = empty_repo("init-symlink-live");
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    let link = d.join(".trellis/config.toml");
    if !link_or_skip(&target, &link) {
        return;
    }

    let o = run(&d, &["init"]);
    assert_init_refused(&o, "config.toml");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "echo original\n");
    assert_eq!(entries(&outside), ["rc.sh"]);
    assert_eq!(entries(&d.join(".trellis")), ["config.toml"]);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn init_symlink_linked_dot_trellis_is_refused_and_the_outside_directory_untouched() {
    // A linked `.trellis` puts every scaffolded file wherever it points.
    let outside = scratch("init-symlink-dot-outside");
    let d = empty_repo("init-symlink-dot");
    if !link_or_skip(&outside, &d.join(".trellis")) {
        return;
    }

    let o = run(&d, &["init"]);
    assert_init_refused(&o, ".trellis");
    assert!(entries(&outside).is_empty(), "{:?}", entries(&outside));
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn init_symlink_linked_governance_dir_is_refused_and_nothing_is_written() {
    // A linked parent directory moves the write as surely as a linked
    // file: `.trellis/governance` pointing outside received README.md. The
    // refusal comes before config.toml and context.md are created too.
    let outside = scratch("init-symlink-gov-outside");
    let d = empty_repo("init-symlink-gov");
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    if !link_or_skip(&outside, &d.join(".trellis/governance")) {
        return;
    }

    let o = run(&d, &["init"]);
    assert_init_refused(&o, "governance");
    assert!(entries(&outside).is_empty(), "{:?}", entries(&outside));
    assert_eq!(entries(&d.join(".trellis")), ["governance"]);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&outside);
}
