//! Ctrl-C through the real binary (#138, ADR 0023 §5): `after_run` runs on
//! an interrupt, before the rollback, and a second Ctrl-C stops it.
//!
//! Unix only: the signal is sent with `kill -INT`, and Windows has no way to
//! deliver a console Ctrl-C to one child process from a test. `rpc`'s
//! `abort` takes the same path and is tested on every OS in `src/rpc.rs`.
//!
//! The run has to be interrupted inside the loop's model request, not inside
//! a tool call: the orchestrator answers one request at a time, so an
//! interrupt during a long tool call waits for that call before `after_run`
//! can start. So the model is a provider stub on a loopback port that
//! answers the first request with an edit and never answers the second.
#![cfg(unix)]

use serde_json::Value;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("trellis-interrupt-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git must be on PATH");
    assert!(out.status.success(), "git {args:?}");
}

/// A variable nobody sets: the loopback stub is then used with no key, so
/// no key of this machine's reaches it.
const KEY_ENV: &str = "TRELLIS_TEST_INTERRUPT_KEY";

struct Fixture {
    root: PathBuf,
    home: PathBuf,
}

/// A committed repository whose `[hooks]` are `hooks`, and a user
/// configuration naming the provider stub at `base`.
fn fixture(root: PathBuf, hooks: &str, base: &str) -> Fixture {
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@example.invalid"]);
    git(&root, &["config", "user.name", "t"]);
    git(&root, &["config", "core.autocrlf", "false"]);
    std::fs::create_dir_all(root.join(".trellis")).unwrap();
    std::fs::write(root.join(".trellis/config.toml"), hooks).unwrap();
    std::fs::write(root.join("a.py"), "def f():\n    return 1\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "fixture"]);
    let home = scratch(&format!(
        "{}-home",
        root.file_name().unwrap().to_string_lossy()
    ));
    std::fs::create_dir_all(home.join("trellis")).unwrap();
    std::fs::write(
        home.join("trellis/config.toml"),
        format!(
            "[[providers]]\nname = \"stub\"\nprotocol = \"openai\"\n\
             base_url = \"{base}\"\nkey_env = \"{KEY_ENV}\"\n"
        ),
    )
    .unwrap();
    Fixture { root, home }
}

/// The binary, with its user configuration and trust store in the
/// fixture's `home`, never the developer's.
fn trellis(f: &Fixture) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_trellis"));
    c.current_dir(&f.root)
        .env("NO_COLOR", "1")
        .env_remove("RUST_LOG")
        .env_remove(KEY_ENV)
        .env("TRELLIS_CONFIG_DIR", f.home.join("trellis"))
        .env("XDG_CONFIG_HOME", &f.home);
    c
}

fn combined(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// Reads one HTTP request from `s`, headers and body.
fn read_request(s: &mut std::net::TcpStream) {
    s.set_read_timeout(Some(Duration::from_secs(30))).ok();
    let mut raw = Vec::new();
    let mut buf = [0u8; 16384];
    loop {
        let n = s.read(&mut buf).unwrap_or(0);
        raw.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&raw).to_string();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let len: usize = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length").then_some(v)
                })
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            if body.len() >= len || n == 0 {
                return;
            }
        } else if n == 0 {
            return;
        }
    }
}

/// An OpenAI-protocol endpoint on a loopback port. The first request is
/// answered with an edit of `a.py`; the second is held open, unanswered,
/// and the receiver hears once it has arrived. The run is then inside its
/// model request, with its edit in the tree.
fn provider_stub() -> (String, mpsc::Receiver<()>) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut s, _)) = l.accept() else { return };
        read_request(&mut s);
        let args = serde_json::json!({"path": "a.py", "old": "return 1", "new": "return 2"});
        let reply = serde_json::json!({"choices":[{"message":{"role":"assistant",
            "content":null,"tool_calls":[{"id":"c1","type":"function","function":{
            "name":"edit","arguments":args.to_string()}}]}}],
            "usage":{"completion_tokens":3}})
        .to_string();
        let out = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{reply}",
            reply.len()
        );
        s.write_all(out.as_bytes()).ok();
        drop(s);
        let Ok((mut held, _)) = l.accept() else {
            return;
        };
        read_request(&mut held);
        let _ = tx.send(());
        // Held until the client goes away: a read then returns 0 or fails.
        held.set_read_timeout(None).ok();
        let mut sink = [0u8; 1024];
        while matches!(held.read(&mut sink), Ok(n) if n > 0) {}
    });
    (format!("http://{addr}"), rx)
}

fn sigint(child: &Child) {
    let ok = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("kill(1) is on PATH")
        .success();
    assert!(ok, "kill -INT failed");
}

/// Waits for `child` to exit, killing it after `limit`.
fn wait(mut child: Child, limit: Duration) -> Output {
    let until = Instant::now() + limit;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > until {
            let _ = child.kill();
            let o = child.wait_with_output().unwrap();
            panic!("did not exit within {limit:?}:\n{}", combined(&o));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().unwrap()
}

/// Trusts the fixture, starts `trellis run` against the stub, and returns
/// once the run is waiting on its second model request with its edit in
/// the tree.
fn start_run(f: &Fixture, held: &mpsc::Receiver<()>) -> Child {
    let t = trellis(f).arg("trust").output().unwrap();
    assert!(t.status.success(), "trust: {}", combined(&t));
    let child = trellis(f)
        .args(["run", "change f", "--model", "provider:stub#m"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if held.recv_timeout(Duration::from_secs(60)).is_err() {
        panic!(
            "the run never made its second model request:\n{}",
            combined(&wait(child, Duration::from_secs(5)))
        );
    }
    assert_eq!(
        std::fs::read_to_string(f.root.join("a.py")).unwrap(),
        "def f():\n    return 2\n",
        "the interrupt is meant to catch the run with its edit in the tree"
    );
    child
}

#[test]
fn after_run_interrupted_by_sigint_runs_before_the_rollback_with_reason_interrupted() {
    // The risk: Ctrl-C rolled the run back and wrote its trail, and a hook
    // relying on `after_run` (to release a lock, post a note) never learned
    // the run had ended. It must run before the rollback, so what it wrote
    // in the worktree goes with the run's edit. It runs in a throwaway
    // clone (ADR 0032), so it names the repository's file by absolute
    // path; its stdin goes outside the repository.
    let root = scratch("sigint");
    let hooked = root.join("hooked.py");
    let payload = scratch("sigint-payload").join("payload.json");
    let hooks = format!(
        "[hooks]\nafter_run = ['sh', '-c', 'cat > {}; echo hooked > {}']\n",
        payload.display(),
        hooked.display()
    );
    let (base, held) = provider_stub();
    let f = fixture(root, &hooks, &base);
    let child = start_run(&f, &held);

    sigint(&child);
    let o = wait(child, Duration::from_secs(60));
    let said = combined(&o);
    assert_eq!(o.status.code(), Some(130), "{said}");
    assert!(said.contains("INTERRUPTED"), "{said}");

    let raw = std::fs::read_to_string(&payload)
        .unwrap_or_else(|e| panic!("after_run did not run on Ctrl-C ({e}):\n{said}"));
    let given: Value = serde_json::from_str(raw.trim())
        .unwrap_or_else(|e| panic!("after_run's stdin is not JSON ({e}): {raw:?}"));
    assert_eq!(given["reason"], "interrupted", "{given}");
    assert_eq!(given["converged"], false, "{given}");
    assert_eq!(
        given["touched"],
        serde_json::json!(["a.py"]),
        "the hook is told what the run wrote: {given}"
    );
    assert!(
        !hooked.exists(),
        "the hook's write outlived the run: it ran after the rollback, or the rollback missed it:\n{said}"
    );
    assert_eq!(
        std::fs::read_to_string(f.root.join("a.py")).unwrap(),
        "def f():\n    return 1\n",
        "the run's edit is rolled back with it"
    );
}

#[test]
fn after_run_interrupted_a_second_sigint_stops_the_hook_and_the_rollback_proceeds() {
    // The risk: a hook that hangs holds the person's Ctrl-C hostage for its
    // whole timeout. A second Ctrl-C stops it, and the rollback still runs.
    // The timeout here is two minutes, so an exit well inside it is the
    // second press's doing. `exec`, so the process the drop kills is the
    // sleep itself.
    let root = scratch("sigint-twice");
    let hooked = root.join("hooked.py");
    let started = scratch("sigint-twice-marker").join("started");
    let hooks = format!(
        "[hooks]\ntimeout_s = 120\nafter_run = ['sh', '-c', 'echo hooked > {}; echo > {}; exec sleep 60']\n",
        hooked.display(),
        started.display()
    );
    let (base, held) = provider_stub();
    let f = fixture(root, &hooks, &base);
    let child = start_run(&f, &held);

    sigint(&child);
    let until = Instant::now() + Duration::from_secs(60);
    while !started.exists() {
        assert!(
            Instant::now() < until,
            "after_run never started after the first Ctrl-C"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let second = Instant::now();
    sigint(&child);
    let o = wait(child, Duration::from_secs(90));
    let took = second.elapsed();
    let said = combined(&o);
    assert!(
        took < Duration::from_secs(30),
        "the second Ctrl-C did not stop the hook: exit came {took:?} after it:\n{said}"
    );
    assert_eq!(o.status.code(), Some(130), "{said}");
    assert!(said.contains("stopped by a second Ctrl-C"), "{said}");
    assert!(
        !hooked.exists(),
        "the rollback did not run after the hook was stopped:\n{said}"
    );
    assert_eq!(
        std::fs::read_to_string(f.root.join("a.py")).unwrap(),
        "def f():\n    return 1\n"
    );
}
