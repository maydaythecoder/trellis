use super::*;

// ---------------------------------------------------------------------------
// the user's directory and installed packages (#248)
// ---------------------------------------------------------------------------

/// `trellis <args>` in `root` with `user` as the whole user level.
fn run_as(root: &Path, user: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trellis"))
        .args(args)
        .current_dir(root)
        .env("NO_COLOR", "1")
        .env("TRELLIS_CONFIG_DIR", user)
        .output()
        .unwrap()
}

/// A user directory with package `kit` installed by hand: one skill,
/// `tidy`, and one template, `kitfix`.
fn user_with_kit(name: &str) -> PathBuf {
    let user = scratch(name);
    let packages = user.join("packages");
    std::fs::create_dir_all(packages.join("kit/skills/tidy")).unwrap();
    std::fs::create_dir_all(packages.join("kit/prompts")).unwrap();
    std::fs::write(
        packages.join("packages.toml"),
        format!(
            "[[package]]\nname = \"kit\"\nsource = \"/s/kit\"\nref = \"v1\"\ncommit = \"{}\"\n",
            "a".repeat(40)
        ),
    )
    .unwrap();
    std::fs::write(
        packages.join("kit/skills/tidy/SKILL.md"),
        "---\nname: tidy\ndescription: Tidy a module.\n---\n# tidy\n",
    )
    .unwrap();
    std::fs::write(
        packages.join("kit/prompts/kitfix.md"),
        "Fix the thing the kit knows about.\n",
    )
    .unwrap();
    user
}

#[test]
fn doctor_reports_the_user_s_file_and_an_installed_package_s_skills() {
    // The risk: doctor, where a person goes to find out what a run will
    // load, saying a user file is layered when it is absent or absent when
    // it is layered, or not finding a package's skills that a run loads.
    let d = repo("doctor-user-level");
    let user = user_with_kit("doctor-user-level-cfg");
    let s = stdout(&run_as(&d, &user, &["doctor"]));
    assert!(
        s.contains("config.toml — not present; the repository's config alone"),
        "{s}"
    );
    assert!(s.contains("user skills: 1 loaded"), "{s}");
    std::fs::write(user.join("config.toml"), "").unwrap();
    let s = stdout(&run_as(&d, &user, &["doctor"]));
    assert!(
        s.contains("config.toml — layered under the repository's"),
        "{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn a_run_finds_a_template_only_an_installed_package_carries() {
    let d = repo("installed-template");
    let user = user_with_kit("installed-template-cfg");
    let script = script_that_stops(&d);
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "script"]);
    let out = run_as(
        &d,
        &user,
        &["run", "--prompt", "kitfix", "--model", &script],
    );
    let s = combined(&out);
    assert!(!s.contains("no prompt"), "{s}");
    assert!(s.contains("model:"), "the run started: {s}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn install_list_prints_what_the_manifest_records() {
    // The risk: `trellis install` dispatched and then doing nothing,
    // exiting 0 with no output.
    let d = scratch("install-list");
    let user = user_with_kit("install-list-cfg");
    let out = run_as(&d, &user, &["install", "--list"]);
    assert!(out.status.success(), "{}", combined(&out));
    assert!(
        stdout(&out).contains("kit  source /s/kit  ref v1  commit"),
        "{}",
        combined(&out)
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn no_model_message_is_the_same_from_run_and_from_bare_trellis_on_a_pipe() {
    // #376, ADR 0050 section 2: two commands that refuse for the same reason
    // used to say it two ways, and neither mentioned the picker. The risk is
    // drift: one wording updated and the other not. So the text `trellis
    // run` prints must appear, whole, in what bare `trellis` prints when a
    // pipe sends it to the line session.
    let d = repo("no-model-message");
    let config = std::env::temp_dir()
        .join("trellis-tests-no-user-config")
        .join("config.toml");

    let out = run(&d, &["run", "x"]);
    assert!(!out.status.success(), "a run with no model must refuse");
    let err = String::from_utf8_lossy(&out.stderr);
    let said = err
        .trim()
        .strip_prefix("Error: ")
        .unwrap_or_else(|| panic!("an error, not:\n{err}"));
    // What it has to name: the picker, one complete `--model` form, and the
    // user's own config file.
    assert!(said.contains("  trellis "), "names bare trellis:\n{said}");
    assert!(
        said.contains("--model llamacpp:http://127.0.0.1:8080"),
        "a concrete --model:\n{said}"
    );
    assert!(
        said.contains(&config.display().to_string()),
        "the user config file:\n{said}"
    );

    let bare = {
        use std::io::Write;
        use std::process::Stdio;
        let mut child = Command::new(env!("CARGO_BIN_EXE_trellis"))
            .current_dir(&d)
            .env("NO_COLOR", "1")
            .env(
                "TRELLIS_CONFIG_DIR",
                std::env::temp_dir().join("trellis-tests-no-user-config"),
            )
            // As in `run`: the two texts are compared byte for byte (#783).
            .env("RUST_BACKTRACE", "0")
            .env("RUST_LIB_BACKTRACE", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"x\n/exit\n")
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let shown = combined(&bare);
    assert!(
        shown.contains(said),
        "bare trellis must print the same text as `trellis run`.\nrun:\n{said}\nbare:\n{shown}"
    );
    let _ = std::fs::remove_dir_all(&d);
}
