//! No child process trellis starts inherits a hosted key (ADR 0024 §3).
//!
//! The risk: trellis reads `OPENAI_API_KEY`, `ANTHROPIC_API_KEY` or a
//! provider's `key_env` from its own environment, and a child inherits that
//! environment unless the spawn removes the variable. A check, `bash`, a
//! custom tool, a hook or a process agent that prints its environment would
//! put the key in an observation, and from there in the log and the trail.
//!
//! Each spawn kind here runs a command that writes its whole environment to a
//! file outside the repository (outside, because a run that does not
//! converge is rolled back). Every dump must hold a marker variable set
//! alongside the keys, so a dump that recorded nothing cannot pass by
//! proving nothing; and none may hold a key's value.
//!
//! The keys are set on the binary's `Command`, never on this process: setting
//! a variable here is `unsafe` in edition 2024 and would leak into every
//! other test in the process.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MARKER: (&str, &str) = ("TRELLIS_TEST_VISIBLE", "visible-marker-61c0a2");
/// What trellis sets so a tool can tell an agent launched it (#317).
const AGENT_MARKER: (&str, &str) = ("AI_AGENT", "trellis");
/// Built-in names and two user-level providers' `key_env`s: every way a key
/// reaches trellis's environment. (A repository may no longer name a
/// `key_env` at all, ADR 0024 §3, so both custom names are the user's.)
///
/// The values are built at run time rather than written here. They are
/// canaries, not keys, but a key-shaped literal in source is exactly what a
/// secret scanner flags, and GitGuardian did (PR #1). Unique per process, so
/// a dump can only hold one because this test put it there.
fn keys() -> &'static [(&'static str, &'static str)] {
    static KEYS: std::sync::OnceLock<Vec<(&'static str, &'static str)>> =
        std::sync::OnceLock::new();
    KEYS.get_or_init(|| {
        [
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "TRELLIS_TEST_SECOND_USER_KEY",
            "TRELLIS_TEST_USER_KEY",
        ]
        .into_iter()
        .map(|name| {
            let value = format!(
                "canary-{}-{}",
                name.to_ascii_lowercase(),
                std::process::id()
            );
            (name, &*Box::leak(value.into_boxed_str()))
        })
        .collect()
    })
}

mod common;
use common::Scratch;

fn scratch(name: &str) -> Scratch {
    common::scratch_dir(&format!("trellis-childenv-{name}"))
}

fn git(root: &Path, args: &[&str]) {
    let out = trellis_vcs::command()
        .args(args)
        .current_dir(root)
        .output()
        .expect("git on PATH");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A shell argv, as TOML literal strings, that writes the environment to
/// `dump`. Literal strings so a Windows path's backslashes are not escapes.
fn dump_argv(dump: &Path) -> String {
    let (shell, flag) = shell();
    format!("['{shell}', '{flag}', '{}']", dump_line(dump))
}

fn shell() -> (&'static str, &'static str) {
    if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    }
}

/// The command line that prints the environment into `dump` and exits 0.
///
/// A check appends its paths to the argv. `& rem` swallows them on Windows,
/// where `cmd` would otherwise read `set > f a.py` as `set a.py`; `sh -c`
/// takes them as positional parameters and ignores them.
fn dump_line(dump: &Path) -> String {
    if cfg!(windows) {
        format!("set > {} & rem", dump.display())
    } else {
        format!("env > {}", dump.display())
    }
}

/// A committed repository with `config` as `.trellis/config.toml` and
/// `extra` files alongside. Its user configuration directory is `home`,
/// holding two providers whose `key_env`s are `TRELLIS_TEST_USER_KEY` and
/// `TRELLIS_TEST_SECOND_USER_KEY`.
struct Fixture {
    repo: Scratch,
    home: Scratch,
    dumps: Scratch,
}

fn fixture(name: &str, config: &str, extra: &[(&str, String)]) -> Fixture {
    let repo = scratch(name);
    let home = scratch(&format!("{name}-home"));
    let dumps = scratch(&format!("{name}-dumps"));
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "t@example.invalid"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["config", "core.autocrlf", "false"]);
    std::fs::create_dir_all(repo.join(".trellis")).unwrap();
    std::fs::write(repo.join("a.py"), "def f():\n    return 1\n").unwrap();
    std::fs::write(repo.join(".trellis/config.toml"), format!("{config}\n")).unwrap();
    for (path, body) in extra {
        let p = repo.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "fixture"]);

    // The user's own file (ADR 0025 §7), whose providers are as much the
    // effective configuration as the repository's.
    std::fs::create_dir_all(home.join("trellis")).unwrap();
    std::fs::write(
        home.join("trellis/config.toml"),
        "[[providers]]\nname = \"user\"\nprotocol = \"anthropic\"\n\
         base_url = \"https://user.invalid\"\nkey_env = \"TRELLIS_TEST_USER_KEY\"\n\
         [[providers]]\nname = \"second\"\nprotocol = \"openai\"\n\
         base_url = \"https://second.invalid\"\nkey_env = \"TRELLIS_TEST_SECOND_USER_KEY\"\n",
    )
    .unwrap();
    Fixture { repo, home, dumps }
}

/// The binary, with the keys and the marker in its environment and its user
/// configuration and trust store pointed at the fixture's `home`.
fn trellis(f: &Fixture, args: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_trellis"));
    c.arg("--root")
        .arg(&f.repo)
        .args(args)
        .current_dir(&f.repo)
        .env("NO_COLOR", "1")
        .env_remove("RUST_LOG")
        // So an `AI_AGENT` in a dump was put there by trellis, not inherited
        // from an agent running this suite (#317).
        .env_remove(AGENT_MARKER.0)
        .env("TRELLIS_CONFIG_DIR", f.home.join("trellis"))
        .env("APPDATA", &f.home)
        .env("XDG_CONFIG_HOME", &f.home)
        .env(MARKER.0, MARKER.1);
    for &(k, v) in keys() {
        c.env(k, v);
    }
    c.output().expect("the binary is built before its tests")
}

fn combined(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn read(p: &Path) -> String {
    String::from_utf8_lossy(&std::fs::read(p).unwrap_or_default()).into_owned()
}

/// The dump was written, by a process that saw the parent's environment
/// (the marker), and holds none of `absent`.
fn assert_scrubbed(what: &str, dump: &Path, absent: &[(&str, &str)], said: &str) {
    let text = read(dump);
    assert!(
        text.contains(MARKER.1),
        "{what}: no environment dump with the marker at {}, so nothing was proved.\n\
         dump: {text}\noutput: {said}",
        dump.display()
    );
    for (k, v) in absent {
        assert!(
            !text.contains(v),
            "{what} inherited {k}: a command that prints its environment would record the key"
        );
    }
}

/// Everything under `dir` outside `.git`: the trail and any file a command
/// left behind.
fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.file_name().is_some_and(|n| n == ".git") {
            continue;
        }
        if p.is_dir() {
            files(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn dumps_of(d: &Path, n: &str) -> PathBuf {
    d.join(format!("{n}.txt"))
}

/// A trusted model run whose two checks, `bash`, custom tool and hook each
/// write their environment to `<kind>.txt` under the fixture's `dumps`.
/// Returns the fixture and what the run printed.
fn print_environments(name: &str) -> (Fixture, String) {
    // The dump paths live in the fixture's own directory, so the tool
    // configuration goes in as a second commit once that exists.
    let f0 = fixture(name, "", &[]);
    let d = &f0.dumps;
    let (shell, flag) = shell();
    let config = format!(
        "[checks]\ntypecheck = {check}\ntests = {tests}\n\n\
         [tools]\nbash = true\nbash_unattended = true\nshell = ['{shell}', '{flag}']\n\n\
         [[tools.custom]]\nname = \"dumpenv\"\ndescription = \"Prints the environment.\"\n\
         command = {custom}\nschema = {{ type = \"object\", properties = {{}} }}\n\n\
         [hooks]\npre_tool = {hook}\n",
        check = dump_argv(&dumps_of(d, "check")),
        tests = dump_argv(&dumps_of(d, "tests")),
        custom = dump_argv(&dumps_of(d, "custom")),
        hook = dump_argv(&dumps_of(d, "hook")),
    );
    std::fs::write(f0.repo.join(".trellis/config.toml"), {
        let base = read(&f0.repo.join(".trellis/config.toml"));
        format!("{config}\n{base}")
    })
    .unwrap();
    git(
        &f0.repo,
        &["commit", "-qam", "tools that print the environment"],
    );

    let script = serde_json::json!({"steps": [
        {"tool": "bash", "command": dump_line(&dumps_of(d, "bash"))},
        {"tool": "dumpenv"},
        {"tool": "run_checks", "paths": ["a.py"], "kinds": ["typecheck", "tests"]},
        {"tool": "done", "summary": "printed environments"},
    ]});
    let script_path = f0.home.join("script.json");
    std::fs::write(&script_path, script.to_string()).unwrap();

    let trust = trellis(&f0, &["trust"]);
    assert!(trust.status.success(), "trust: {}", combined(&trust));
    let spec = format!("scripted:{}", script_path.display());
    let out = trellis(&f0, &["run", "print environments", "--model", &spec]);
    let said = combined(&out);
    (f0, said)
}

/// Whether the environment dump at `dump` holds `line` (`NAME=value`).
fn has_line(dump: &Path, line: &str) -> bool {
    read(dump).lines().any(|l| l.trim_end() == line)
}

#[test]
fn checks_bash_custom_tools_and_hooks_inherit_no_key() {
    let (f0, said) = print_environments("run");
    let d = &f0.dumps;

    for kind in ["check", "tests", "bash", "custom", "hook"] {
        assert_scrubbed(kind, &dumps_of(d, kind), keys(), &said);
    }

    // The observation path too: the terminal and the trail hold no key.
    let mut paths = Vec::new();
    files(&f0.repo, &mut paths);
    assert!(
        paths
            .iter()
            .any(|p| p.to_string_lossy().contains("runs")
                && p.extension().is_some_and(|e| e == "json")),
        "a trail was written: {said}"
    );
    let mut places = vec![("output".to_string(), said.clone())];
    for p in &paths {
        places.push((p.display().to_string(), read(p)));
    }
    for (name, text) in &places {
        for &(k, v) in keys() {
            assert!(!text.contains(v), "{name} contains {k}'s value");
        }
    }
    for p in [&f0.repo, &f0.home, &f0.dumps] {
        let _ = std::fs::remove_dir_all(p);
    }
}

#[test]
fn a_process_agent_inherits_no_key_unless_its_profile_passes_that_one() {
    let dumps = scratch("proc-out");
    let plain = dumps.join("plain.txt");
    let keyed = dumps.join("keyed.txt");
    let f = fixture(
        "proc",
        "[checks]\ntypecheck = [\"git\", \"--version\"]\ntests = [\"git\", \"--version\"]\n",
        &[
            (
                ".trellis/agents/plain.toml",
                format!("command = {}\ntimeout_s = 60\n", dump_argv(&plain)),
            ),
            (
                ".trellis/agents/keyed.toml",
                format!(
                    "command = {}\ntimeout_s = 60\npass_env = [\"ANTHROPIC_API_KEY\"]\n",
                    dump_argv(&keyed)
                ),
            ),
        ],
    );
    for who in ["plain", "keyed"] {
        let o = trellis(
            &f,
            &["queue", "add", "print the environment", "--assignee", who],
        );
        assert!(o.status.success(), "{}", combined(&o));
    }
    // A process agent's command is repository code, so it runs only once
    // the person has trusted it (ADR 0023 §4); the trust store is the
    // fixture's, never the developer's.
    let trust = trellis(&f, &["trust"]);
    assert!(trust.status.success(), "trust: {}", combined(&trust));
    let out = trellis(&f, &["work"]);
    let said = combined(&out);

    assert_scrubbed("process agent", &plain, keys(), &said);
    // `pass_env` passes exactly what it names: the Anthropic key arrives,
    // and every other key is still removed.
    let (named, rest) = keys().split_at(2);
    let (openai, anthropic) = (named[0], named[1]);
    assert!(
        read(&keyed).contains(anthropic.1),
        "pass_env = [\"ANTHROPIC_API_KEY\"] must pass it through: {said}"
    );
    let mut others = vec![openai];
    others.extend_from_slice(rest);
    assert_scrubbed("process agent with pass_env", &keyed, &others, &said);

    // Which keys a process is given is said by name, never by value.
    let show = combined(&trellis(&f, &["agents", "show", "keyed"]));
    assert!(show.contains("passed ANTHROPIC_API_KEY"), "{show}");
    assert!(!show.contains(anthropic.1), "{show}");

    for p in [&f.repo, &f.home, &f.dumps, &dumps] {
        let _ = std::fs::remove_dir_all(p);
    }
}

#[test]
fn bash_custom_tools_hooks_and_process_agents_get_the_agent_marker() {
    // #317. The risk: a tool that adapts its output when an agent launched
    // it (`AI_AGENT`) cannot tell trellis did, because the only names
    // trellis set were its own. `trellis()` removes `AI_AGENT`, so a
    // marker in a dump is one trellis added.
    let marker = format!("{}={}", AGENT_MARKER.0, AGENT_MARKER.1);
    let (f0, said) = print_environments("marker-run");
    for kind in ["bash", "custom", "hook"] {
        let dump = dumps_of(&f0.dumps, kind);
        assert_scrubbed(kind, &dump, keys(), &said);
        assert!(
            has_line(&dump, &marker),
            "{kind} has no {marker}:\n{}\noutput: {said}",
            read(&dump)
        );
    }

    let agent_dir = scratch("marker-proc-out");
    let agent = agent_dir.join("agent.txt");
    let f1 = fixture(
        "marker-proc",
        "[checks]\ntypecheck = [\"git\", \"--version\"]\ntests = [\"git\", \"--version\"]\n",
        &[(
            ".trellis/agents/plain.toml",
            format!("command = {}\ntimeout_s = 60\n", dump_argv(&agent)),
        )],
    );
    let o = trellis(
        &f1,
        &[
            "queue",
            "add",
            "print the environment",
            "--assignee",
            "plain",
        ],
    );
    assert!(o.status.success(), "{}", combined(&o));
    let trust = trellis(&f1, &["trust"]);
    assert!(trust.status.success(), "trust: {}", combined(&trust));
    let said = combined(&trellis(&f1, &["work"]));
    assert_scrubbed("process agent", &agent, keys(), &said);
    assert!(
        has_line(&agent, &marker),
        "the process agent has no {marker}:\n{}\noutput: {said}",
        read(&agent)
    );

    for p in [&f0.repo, &f0.home, &f0.dumps, &f1.repo, &f1.home, &f1.dumps] {
        let _ = std::fs::remove_dir_all(p);
    }
    let _ = std::fs::remove_dir_all(agent.parent().unwrap());
}

#[test]
fn a_grounded_check_gets_no_agent_marker_from_trellis() {
    // #317. The risk: a repository's test suite that reads `AI_AGENT`
    // behaves differently under trellis than under a person, and the
    // Verdict describes a run no person can reproduce. `trellis()` removes
    // `AI_AGENT`, so one in a check's dump was added by trellis.
    let (f0, said) = print_environments("marker-check");
    for kind in ["check", "tests"] {
        let dump = dumps_of(&f0.dumps, kind);
        assert_scrubbed(kind, &dump, keys(), &said);
        let prefix = format!("{}=", AGENT_MARKER.0);
        assert!(
            !read(&dump)
                .lines()
                .any(|l| l.to_ascii_uppercase().starts_with(&prefix)),
            "{kind}: the check was given {}:\n{}",
            AGENT_MARKER.0,
            read(&dump)
        );
    }
    for p in [&f0.repo, &f0.home, &f0.dumps] {
        let _ = std::fs::remove_dir_all(p);
    }
}

#[test]
fn service_gets_the_scrubbed_environment_and_not_its_profile_s_pass_env() {
    // ADR 0026 §12. The risk: a service is a long-running repository
    // command whose output goes to a log, and one that inherited a key, or
    // was handed the one `pass_env` gives its profile's process agent, would
    // put it in that log. The process agent waits a few seconds so the
    // service has written its dump before the task ends and it is killed.
    let dumps = scratch("svc-out");
    let svc = dumps.join("service.txt");
    let wait = if cfg!(windows) {
        "['cmd', '/C', 'ping -n 4 127.0.0.1 >NUL']"
    } else {
        "['sh', '-c', 'sleep 3']"
    };
    let f = fixture(
        "svc",
        "[checks]\ntypecheck = [\"git\", \"--version\"]\ntests = [\"git\", \"--version\"]\n",
        &[(
            ".trellis/agents/keyed.toml",
            format!(
                "command = {wait}\ntimeout_s = 60\npass_env = [\"ANTHROPIC_API_KEY\"]\n\
                 [[services]]\nname = \"env\"\ncommand = {}\n",
                dump_argv(&svc)
            ),
        )],
    );
    let o = trellis(
        &f,
        &[
            "queue",
            "add",
            "print the environment",
            "--assignee",
            "keyed",
        ],
    );
    assert!(o.status.success(), "{}", combined(&o));
    let trust = trellis(&f, &["trust"]);
    let shown = combined(&trust);
    assert!(trust.status.success(), "trust: {shown}");
    assert!(shown.contains("service keyed/env"), "{shown}");
    let said = combined(&trellis(&f, &["work"]));
    assert!(said.contains("service:  env started"), "{said}");
    assert_scrubbed("service", &svc, keys(), &said);
    for p in [&f.repo, &f.home, &f.dumps, &dumps] {
        let _ = std::fs::remove_dir_all(p);
    }
}

#[test]
fn pass_env_on_a_model_profile_is_refused() {
    // A model profile spawns nothing to pass a variable to; accepting the
    // field would read as a pass-through that happened.
    let f = fixture(
        "passenv-model",
        "",
        &[(
            ".trellis/agents/m.toml",
            "model = \"scripted:x\"\npass_env = [\"OPENAI_API_KEY\"]\n".to_string(),
        )],
    );
    let o = trellis(&f, &["agents", "list"]);
    let s = combined(&o);
    assert!(!o.status.success() && s.contains("pass_env"), "{s}");
    for p in [&f.repo, &f.home, &f.dumps] {
        let _ = std::fs::remove_dir_all(p);
    }
}

/// Non-test source lines of `text`: everything outside a `#[cfg(test)]`
/// module. Braces are counted naively, which holds for this workspace's
/// test modules (their format strings are balanced); a miscount hides
/// shipped lines from the scan, which the site count below would notice
/// only if it hid most of them.
fn non_test_lines(text: &str) -> Vec<(usize, &str)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "#[cfg(test)]"
            && lines
                .get(i + 1)
                .is_some_and(|l| l.trim_start().starts_with("mod ") && l.contains('{'))
        {
            let mut depth = 0i64;
            let mut j = i + 1;
            while j < lines.len() {
                depth += lines[j].matches('{').count() as i64;
                depth -= lines[j].matches('}').count() as i64;
                j += 1;
                if depth <= 0 {
                    break;
                }
            }
            i = j;
            continue;
        }
        out.push((i, lines[i]));
        i += 1;
    }
    out
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The interface binaries whose launch passes trellis's environment
/// unchanged, because each starts its own `trellis rpc`, which needs the
/// user's key variables for a hosted model the user chose (ADR 0052's
/// 2026-10-07 amendments, ADR 0048 section 3). Every other child is
/// scrubbed. A spawn uses the exemption by carrying `// key-exempt: <binary>`
/// on its `Command::new(` line, in `LAUNCH_FILE` only, once per binary, so
/// a third exemption is an edit here and to the ADRs, not a comment.
const INTERFACE_LAUNCHES: [&str; 2] = ["trellis-tui", "trellis-desktop"];
/// The one file whose spawns may carry the marker, relative to `crates/`.
const LAUNCH_FILE: &str = "trellis-cli/src/launch.rs";

/// The binary a `// key-exempt: <binary>` marker on `line` names.
fn exempt_binary(line: &str) -> Option<&str> {
    let (_, rest) = line.split_once("// key-exempt:")?;
    rest.split_whitespace().next()
}

/// Scans `files`, each a path relative to `crates/` (with `/`) and its
/// text, for `Command::new(` outside test modules. Returns how many spawn
/// sites it saw and, for each one that neither removes the key variables
/// nor uses the interface exemption as `INTERFACE_LAUNCHES` allows, a line
/// saying where and why.
fn unscrubbed_spawns(files: &[(&str, &str)]) -> (usize, Vec<String>) {
    let mut seen = 0;
    let mut bare = Vec::new();
    let mut exempted: Vec<&str> = Vec::new();
    for &(path, text) in files {
        let all: Vec<&str> = text.lines().collect();
        for (i, line) in non_test_lines(text) {
            if !line.contains("Command::new(") || line.trim_start().starts_with("//") {
                continue;
            }
            seen += 1;
            // The scrub may wrap the constructor or follow the builder
            // calls that configure it, before the spawn.
            let window = &all[i..(i + 16).min(all.len())];
            if window
                .iter()
                .any(|l| l.contains("secrets::scrub(") || l.contains("scrub_keys("))
            {
                continue;
            }
            let why = match exempt_binary(line) {
                None => "removes no key variable",
                Some(_) if path != LAUNCH_FILE => "is marked key-exempt outside the launch file",
                Some(b) if !INTERFACE_LAUNCHES.contains(&b) => {
                    "is marked key-exempt for a binary INTERFACE_LAUNCHES does not list"
                }
                Some(b) if exempted.contains(&b) => "exempts a binary already exempted once",
                Some(b) => {
                    exempted.push(b);
                    continue;
                }
            };
            bare.push(format!("{path}:{}: {} ({why})", i + 1, line.trim()));
        }
    }
    (seen, bare)
}

#[test]
fn every_process_spawn_in_the_workspace_removes_the_key_variables() {
    // The tests above prove the spawn kinds that exist today. This one
    // catches the next: a `Command::new` added to shipped code without a
    // scrub beside it fails here, before it can hand a key to what it runs.
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files = Vec::new();
    for c in std::fs::read_dir(crates).unwrap().flatten() {
        let src = c.path().join("src");
        if src.is_dir() {
            rust_files(&src, &mut files);
        }
    }
    // Test-only modules kept in files of their own.
    let test_file = |p: &Path| {
        let n = p.file_name().unwrap().to_string_lossy();
        n == "tests.rs" || n.ends_with("_tests.rs") || n == "testutil.rs"
    };
    let texts: Vec<(String, String)> = files
        .iter()
        .filter(|f| !test_file(f))
        .map(|f| {
            let rel = f.strip_prefix(crates).unwrap();
            (
                rel.to_string_lossy().replace('\\', "/"),
                std::fs::read_to_string(f).unwrap(),
            )
        })
        .collect();
    let texts: Vec<(&str, &str)> = texts.iter().map(|(p, t)| (&**p, &**t)).collect();
    let (seen, bare) = unscrubbed_spawns(&texts);
    assert!(
        seen >= 10,
        "the scan found only {seen} spawn sites; it is not reading the source"
    );
    assert!(
        bare.is_empty(),
        "spawns that do not remove the key variables (ADR 0024 §3):\n{}",
        bare.join("\n")
    );
}

/// A launch file whose one spawn passes the environment to `binary`.
fn marked_spawn(binary: &str) -> String {
    format!(
        "pub fn launch(p: &Path) {{\n    let _ = Command::new(p).status(); // key-exempt: {binary}\n}}\n"
    )
}

#[test]
fn child_env_interface_launches_are_the_tui_and_the_desktop() {
    // The risk: a launch added to the list in passing, which hands the
    // user's keys to one more program with no owner decision behind it.
    assert_eq!(INTERFACE_LAUNCHES.len(), 2);
    assert_eq!(INTERFACE_LAUNCHES, ["trellis-tui", "trellis-desktop"]);
}

#[test]
fn child_env_interface_a_marked_spawn_of_each_listed_binary_passes_in_the_launch_file() {
    // The exemption works where it is meant to, so the refusals below are
    // refusals of what they name and not of every marked spawn.
    for b in INTERFACE_LAUNCHES {
        let text = marked_spawn(b);
        let (seen, bare) = unscrubbed_spawns(&[(LAUNCH_FILE, &text)]);
        assert_eq!((seen, bare), (1, Vec::<String>::new()), "{b}");
    }
    let tui = marked_spawn("trellis-tui");
    let desktop = marked_spawn("trellis-desktop");
    let (seen, bare) = unscrubbed_spawns(&[(LAUNCH_FILE, &format!("{tui}{desktop}"))]);
    assert_eq!((seen, bare), (2, Vec::<String>::new()));
}

#[test]
fn child_env_interface_a_marked_spawn_naming_an_unlisted_binary_fails() {
    // The risk: a marker is a comment anyone can write, so a spawn in the
    // launch file could name any program and pass the keys to it.
    let text = marked_spawn("trellis-web");
    let (seen, bare) = unscrubbed_spawns(&[(LAUNCH_FILE, &text)]);
    assert_eq!(seen, 1);
    assert_eq!(bare.len(), 1, "{bare:?}");
    assert!(bare[0].contains("does not list"), "{bare:?}");
}

#[test]
fn child_env_interface_a_marked_desktop_spawn_outside_the_launch_file_fails() {
    // The risk: the marker copied into any other file, which turns one
    // reviewed launch into an exemption anywhere in shipped code.
    let text = marked_spawn("trellis-desktop");
    for path in ["trellis-cli/src/rpc.rs", "trellis-loop/src/launch.rs"] {
        let (seen, bare) = unscrubbed_spawns(&[(path, &text)]);
        assert_eq!(seen, 1, "{path}");
        assert_eq!(bare.len(), 1, "{path}: {bare:?}");
        assert!(bare[0].contains("outside the launch file"), "{bare:?}");
    }
}

#[test]
fn child_env_interface_a_binary_exempted_twice_fails() {
    // The risk: a second unscrubbed spawn of a listed binary, which the
    // ADRs never decided on, passing under the first one's marker.
    let once = marked_spawn("trellis-desktop");
    let (seen, bare) = unscrubbed_spawns(&[(LAUNCH_FILE, &format!("{once}{once}"))]);
    assert_eq!(seen, 2);
    assert_eq!(bare.len(), 1, "{bare:?}");
    assert!(bare[0].contains("already exempted"), "{bare:?}");
}
