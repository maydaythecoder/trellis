use super::*;

// ---------------------------------------------------------------------------
// skills (ADR 0019)
// ---------------------------------------------------------------------------

#[test]
fn a_run_says_how_many_skills_it_listed_and_how_many_it_skipped() {
    let d = repo("skills-run");
    add_skill(&d, "profiling", "profiling", "How to profile a slow test.");
    add_skill(
        &d,
        "misnamed",
        "other-name",
        "Skipped: the name is not the directory.",
    );
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "skills"]);

    let out = run(
        &d,
        &[
            "run",
            "--model",
            &script_that_stops(&scratch("skills-run-script")),
            "t",
        ],
    );
    let said = stdout(&out);
    let repo_line = said
        .lines()
        .find(|l| l.starts_with("repo:"))
        .unwrap_or_else(|| panic!("no repo: line\n{}", combined(&out)));
    assert!(
        repo_line.contains("skills: 1 listed, 0 omitted (cap), 1 invalid and skipped"),
        "{repo_line}"
    );
}

#[test]
fn doctor_names_each_skipped_skill_and_why() {
    // A skill that is silently not loaded is one its author believes the
    // model can read.
    let d = repo("skills-doctor");
    add_skill(&d, "profiling", "profiling", "How to profile a slow test.");
    add_skill(&d, "misnamed", "other-name", "x");
    let out = run(&d, &["doctor"]);
    let said = stdout(&out);
    assert!(
        said.contains("skills:     2 found · 1 listed · 0 omitted (cap 4096 B) · 1 invalid"),
        "{said}"
    );
    assert!(
        said.contains(
            "skipped misnamed: frontmatter name `other-name` does not match the directory name `misnamed`"
        ),
        "{said}"
    );
}
