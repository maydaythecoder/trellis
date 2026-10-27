use super::*;

// ---------------------------------------------------------------------------
// #248, pass 3: `trellis vault`
// ---------------------------------------------------------------------------

/// `repo_with_vault`'s `root` and `child`, plus `lost`, whose parent does
/// not exist, and `a` and `b`, each the other's parent: one note with a
/// missing parent and two on a cycle, beside two with no issue.
fn vault_with_issues(name: &str) -> PathBuf {
    let (d, _) = repo_with_vault(name, "vault = \"notes\"\n");
    let v = d.join("notes");
    for (file, title, parent) in [
        ("lost.md", "Lost", "[[nowhere]]"),
        ("a.md", "A", "[[b]]"),
        ("b.md", "B", "[[a]]"),
    ] {
        std::fs::write(
            v.join(file),
            format!("---\ntitle: \"{title}\"\nparent: \"{parent}\"\n---\nBODY\n"),
        )
        .unwrap();
    }
    std::fs::write(
        v.join("manifest.json"),
        r#"["a.md","b.md","child.md","lost.md","root.md"]"#,
    )
    .unwrap();
    d
}

#[test]
fn vault_graph_lists_a_note_s_own_issues_and_no_other_note_s() {
    // #248. The risk: `graph` printing nothing, or a note's issues filtered
    // wrongly, so a note on a cycle or under a missing parent looks sound
    // and a sound note carries another note's problem.
    let d = vault_with_issues("vault-graph");
    let lost = stdout(&run(&d, &["vault", "graph", "lost"]));
    assert!(lost.contains("note:     lost.md \"Lost\""), "{lost}");
    assert!(lost.contains("issues:"), "{lost}");
    assert!(lost.contains("nowhere"), "{lost}");
    let on_cycle = stdout(&run(&d, &["vault", "graph", "a"]));
    assert!(on_cycle.contains("issues:"), "{on_cycle}");
    let sound = stdout(&run(&d, &["vault", "graph", "child"]));
    assert!(sound.contains("note:     child.md"), "{sound}");
    assert!(!sound.contains("issues:"), "{sound}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn vault_check_lists_issues_only_when_there_are_some_and_names_the_fix_for_a_refusal() {
    // #248. The risk: `check` saying nothing, an empty "issues:" header on
    // a sound vault, or a refused vault left without the command that
    // repairs it.
    let (d, _) = repo_with_vault("vault-check", "vault = \"notes\"\n");
    let out = run(&d, &["vault", "check"]);
    let sound = stdout(&out);
    assert!(out.status.success(), "{}", combined(&out));
    assert!(sound.contains("run gate: accepts this vault"), "{sound}");
    assert!(!sound.contains("issues:"), "{sound}");

    let d2 = vault_with_issues("vault-check-issues");
    let issues = stdout(&run(&d2, &["vault", "check"]));
    assert!(issues.contains("issues:"), "{issues}");

    std::fs::write(
        d.join("notes/child.md"),
        "---\ntitle: \"Child\"\ntags: x\nparent: \"[[root]]\"\n---\nCHILD-BODY\n",
    )
    .unwrap();
    let out = run(&d, &["vault", "check"]);
    assert_eq!(out.status.code(), Some(1), "{}", combined(&out));
    assert!(
        stdout(&out).contains("`trellis vault check --fix` repairs"),
        "{}",
        combined(&out)
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&d2);
}

#[test]
fn vault_search_finds_a_body_word_and_reports_no_reduction_on_a_small_vault() {
    // #248. The risk: `search` printing nothing, or saying it was reduced
    // to titles or lost its fuzzy matching on a vault far under the limits.
    let (d, _) = repo_with_vault("vault-search", "vault = \"notes\"\n");
    let s = stdout(&run(&d, &["vault", "search", "CHILD-BODY"]));
    assert!(s.contains("child.md"), "{s}");
    assert!(!s.contains("reduced"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

/// No raw ESC or carriage return reaches the terminal, and each shows as
/// the escape that names it.
fn assert_escaped(out: &str) {
    assert!(!out.contains('\x1b'), "{out:?}");
    assert!(!out.contains('\r'), "{out:?}");
    assert!(out.contains("\\u{1b}"), "{out:?}");
}

#[test]
fn vault_escapes_a_note_title_in_search_and_graph() {
    // #461. The risk: a note is untrusted content (a cloned repository's
    // vault), and a title holding ESC and a carriage return, printed raw,
    // can recolour or overwrite the line a person reads. The frontmatter
    // value is decoded as a JSON string, so `\u001b` and `\r` are the real
    // characters by the time the title is printed.
    let (d, _) = repo_with_vault("vault-escapes-title", "vault = \"notes\"\n");
    std::fs::write(
        d.join("notes/evil.md"),
        "---\ntitle: \"Evil\\u001b[31m\\rForged\"\nparent: \"[[root]]\"\n---\nEVIL-BODY\n",
    )
    .unwrap();
    let s = stdout(&run(&d, &["vault", "search", "EVIL-BODY"]));
    assert!(s.contains("evil.md"), "{s:?}");
    assert_escaped(&s);
    assert!(s.contains("\\r"), "{s:?}");
    for note in ["evil", "root"] {
        let g = stdout(&run(&d, &["vault", "graph", note]));
        assert!(g.contains("evil.md"), "{g:?}");
        assert_escaped(&g);
        assert!(g.contains("\\r"), "{g:?}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn vault_escapes_a_note_file_name_in_search_and_graph() {
    // #461. The risk: the same forged terminal through a path rather than
    // a title. Unix only: Windows refuses control characters in file names.
    let (d, _) = repo_with_vault("vault-escapes-path", "vault = \"notes\"\n");
    std::fs::write(
        d.join("notes/es\x1b[31mc.md"),
        "---\ntitle: \"Plain\"\nparent: \"[[root]]\"\n---\nPATH-BODY\n",
    )
    .unwrap();
    let s = stdout(&run(&d, &["vault", "search", "PATH-BODY"]));
    assert!(s.contains("Plain"), "{s:?}");
    assert_escaped(&s);
    let g = stdout(&run(&d, &["vault", "graph", "root"]));
    assert!(g.contains("Plain"), "{g:?}");
    assert_escaped(&g);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn vault_new_says_what_it_created() {
    // #248. The risk: an edit that reports nothing, so the person cannot
    // tell which file the title became.
    let (d, _) = repo_with_vault("vault-new", "vault = \"notes\"\n");
    let s = stdout(&run(&d, &["vault", "new", "Third", "--parent", "root"]));
    assert!(s.contains("created third.md (\"Third\")"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn vault_dates_says_there_is_no_file_and_seed_makes_one_dated_from_the_files() {
    // #248. The risk: `dates` silent about a missing dates.json, `--seed`
    // refusing to create one, or the seeded entries carrying no day where
    // the file times give one.
    let (d, _) = repo_with_vault("vault-dates", "vault = \"notes\"\n");
    let s = stdout(&run(&d, &["vault", "dates"]));
    assert!(s.contains("no dates.json in"), "{s}");
    assert!(s.contains("--seed"), "{s}");
    let out = run(&d, &["vault", "dates", "--seed"]);
    let s = stdout(&out);
    assert!(s.contains("dates.json: 2 seeded"), "{}", combined(&out));
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(d.join("notes/dates.json")).unwrap())
            .unwrap();
    // Written moments ago, so the file times name a day; which day depends
    // on the host's zone, so only its shape is asserted.
    let day = doc["notes"]["root.md"]["created"]["day"].as_str();
    assert!(day.is_some_and(|d| d.len() == 10), "{doc}");
    let _ = std::fs::remove_dir_all(&d);
}
