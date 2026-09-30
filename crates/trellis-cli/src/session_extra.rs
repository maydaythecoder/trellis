//! What ADR 0025 §6 adds to `trellis session`: `!cmd`, `/prompts`,
//! `/models`, and the model listing `rpc` shares; `/name` and `/copy`
//! (ADR 0027, #109); and the one reader of stdin while a run is in flight
//! (#116).
//!
//! `/skill` and `/<template>` are dispatched in `session.rs`, because they
//! start a run and the session owns how a run is started.

use anyhow::Result;
use serde_json::{Value, json};
use std::path::Path;

/// `!cmd` and `!!cmd`: runs a shell command in the repository root.
///
/// The output goes to this terminal and nowhere else. In pi, `!` also sends
/// the output to the model and `!!` does not. A trellis session has no
/// conversation to send it to, because each task is a fresh run, so the two
/// are the same.
///
/// A command that changes files leaves the worktree dirty, and the next task
/// refuses to start (ADR 0009). That is not prevented: the command is the
/// person's own, typed at their own prompt.
pub(crate) fn shell(root: &Path, line: &str) -> Result<()> {
    let cmd = line.trim();
    if cmd.is_empty() {
        anyhow::bail!("`!` needs a command, e.g. `!git log --oneline -3`");
    }
    let status = shell_command(cmd).current_dir(root).status();
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => {
            println!("exit {}", s.code().map_or("?".into(), |c| c.to_string()));
            Ok(())
        }
        Err(e) => anyhow::bail!("could not start the shell: {e}"),
    }
}

/// `cmd /C` with the line passed unmodified on Windows; `sh -c` elsewhere.
///
/// `raw_arg` because std's argument quoting targets programs that parse
/// their command line the C runtime's way, and `cmd.exe` does not. Quoted
/// arguments inside the line would otherwise arrive escaped twice.
///
/// Scrubbed like every other child (ADR 0024 §3). The line is the person's
/// own, but it runs inside trellis, and one rule for every spawn is the one
/// that survives the next spawn someone adds.
fn shell_command(cmd: &str) -> std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").raw_arg(cmd);
        trellis_core::secrets::scrub(&mut c);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(cmd);
        trellis_core::secrets::scrub(&mut c);
        c
    }
}

/// `/prompts`.
pub(crate) fn print_prompts(root: &Path) {
    let list = match crate::templates::list(root) {
        Ok(l) => l,
        Err(e) => {
            println!("templates not listed: {e:#}");
            return;
        }
    };
    if list.is_empty() {
        println!("no templates in .trellis/prompts/ or in an installed package");
        return;
    }
    for t in list {
        let hint = t.argument_hint.map(|h| format!(" {h}")).unwrap_or_default();
        println!("  /{}{}", t.name, hint);
        if !t.description.is_empty() {
            println!("      {}", t.description);
        }
    }
    println!();
    println!("A template runs as a task. Arguments split shell-style: /name \"two words\" x");
}

/// The catalog and every configured spec, as data. Shared by `/models` and
/// `rpc`'s `get_available_models`.
///
/// Configured specs are listed by where they come from, because the
/// repository's wins over the user's (ADR 0025 §7) and a person choosing a
/// model should see both.
pub(crate) fn models_json(root: &Path, session: Option<&str>) -> Value {
    let repo = crate::config::RepoConfig::load_with_user(root, None)
        .ok()
        .and_then(|c| c.model());
    let user = crate::user_config::file().and_then(|f| {
        let text = std::fs::read_to_string(f).ok()?;
        let v: toml::Value = toml::from_str(&text).ok()?;
        v.get("model")?.get("spec")?.as_str().map(str::to_string)
    });
    let catalog: Vec<Value> = trellis_model::CATALOG
        .iter()
        .map(|e| {
            json!({
                "name": e.name,
                "license": e.license,
                "download_gib": (e.gib() * 10.0).round() / 10.0,
                "default": e.name == trellis_model::DEFAULT,
            })
        })
        .collect();
    json!({
        "catalog": catalog,
        "configured": {"repository": repo, "user": user, "session": session},
    })
}

/// `/models`.
pub(crate) fn print_models(root: &Path, session: Option<&str>) {
    let v = models_json(root, session);
    let c = &v["configured"];
    let show = |label: &str, x: &Value| match x.as_str() {
        Some(s) => println!("  {label:<11}{}", crate::plain(s)),
        None => println!("  {label:<11}none"),
    };
    println!("configured specs");
    show("session", &c["session"]);
    show("repository", &c["repository"]);
    show("user", &c["user"]);
    println!();
    println!("catalog (weights `trellis model pull` can fetch)");
    for e in v["catalog"].as_array().into_iter().flatten() {
        println!(
            "  {}{}  {} GiB, {}",
            e["name"].as_str().unwrap_or(""),
            if e["default"] == true {
                " (default)"
            } else {
                ""
            },
            e["download_gib"],
            e["license"].as_str().unwrap_or("")
        );
    }
    println!();
    println!("/model <spec> switches; a catalog entry runs under a runtime such as llamacpp:.");
}

/// The longest `/name` label, in characters.
pub(crate) const LABEL_MAX: usize = 80;

/// Checks a `/name` label, returning it as it will be recorded.
///
/// The label goes into a committed trail, which is read back as untrusted
/// data, and is printed on this terminal. A control character would be a
/// terminal escape in both places, and a long label is a paragraph, not a
/// name. Refused rather than trimmed, so what is recorded is what was typed.
pub(crate) fn check_label(label: &str) -> Result<String, String> {
    let n = label.chars().count();
    if n > LABEL_MAX {
        return Err(format!(
            "a label is at most {LABEL_MAX} characters; this one is {n}. The label is unchanged."
        ));
    }
    if label.chars().any(char::is_control) {
        return Err(
            "a label may not contain a control character. The label is unchanged.".to_string(),
        );
    }
    Ok(label.to_string())
}

/// One clipboard program and its arguments. The text goes on its stdin.
pub(crate) type Clipboard = (&'static str, &'static [&'static str]);

/// The programs `/copy` tries, in order, on this platform. The first one
/// that starts is the one used.
#[cfg(windows)]
pub(crate) const CLIPBOARDS: &[Clipboard] = &[("clip", &[])];
#[cfg(target_os = "macos")]
pub(crate) const CLIPBOARDS: &[Clipboard] = &[("pbcopy", &[])];
#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) const CLIPBOARDS: &[Clipboard] =
    &[("wl-copy", &[]), ("xclip", &["-selection", "clipboard"])];

/// `/copy`: puts `text` on the clipboard through the first of `programs`
/// that starts, or prints it when none is reached.
///
/// A program rather than a clipboard crate (ADR 0027, #109): the platforms'
/// own tools already do this, and no dependency is added for it. `programs`
/// is a parameter so a test can name a program that does not exist and
/// reach the fallback without touching a real clipboard.
///
/// Printing is the fallback because the person asked for the text; a
/// clipboard that could not be reached is no reason to withhold it.
pub(crate) fn copy(
    text: Option<&str>,
    programs: &[Clipboard],
    out: &mut dyn std::io::Write,
) -> Result<()> {
    let Some(text) = text else {
        writeln!(out, "nothing to copy yet")?;
        return Ok(());
    };
    let reached = programs.iter().find_map(|(program, args)| {
        // Only the first that starts is used: one that starts and then
        // fails is reported, not papered over by the next.
        let child = clipboard_command(program, args).spawn().ok()?;
        Some((*program, feed(child, text)))
    });
    match reached {
        Some((program, true)) => {
            writeln!(
                out,
                "copied the last run's done summary ({} bytes) with {program}",
                text.len()
            )?;
        }
        other => {
            let why = match other {
                Some((program, _)) => format!("{program} did not take it"),
                None => "none of the clipboard programs started".to_string(),
            };
            writeln!(
                out,
                "no clipboard was reached ({why}); the summary is below"
            )?;
            writeln!(out, "{}", crate::plain(text))?;
        }
    }
    Ok(())
}

/// A clipboard program, scrubbed like every other child (ADR 0024 §3), with
/// stdout and stderr closed: `xclip` forks to hold the selection, and a
/// piped stdout would keep this side waiting on it.
fn clipboard_command(program: &str, args: &[&str]) -> std::process::Command {
    let mut c = std::process::Command::new(program);
    c.args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    trellis_core::secrets::scrub(&mut c);
    c
}

/// Writes `text` to the child's stdin, closes it, and waits. `true` only
/// when all of it was written and the program exited zero.
fn feed(mut child: std::process::Child, text: &str) -> bool {
    use std::io::Write;
    let written = match child.stdin.take() {
        // Dropped at the end of the arm, which is the end of input.
        Some(mut stdin) => stdin.write_all(text.as_bytes()).is_ok(),
        None => false,
    };
    let exited = child.wait().is_ok_and(|s| s.success());
    written && exited
}

// #116: input while a run is in flight ----------------------------------

/// A line the person typed, with its newline; `None` is end of input.
pub(crate) type Line = Option<String>;

/// The one reader of stdin from the start of a run until the line it is
/// waiting on arrives, which is after the run has ended.
///
/// A blocking read cannot be withdrawn, so when a run ends one read is still
/// in flight. Handing the prompt back to the line editor then would put two
/// readers on one stdin, each taking whichever lines reach it first. So the
/// session takes its next line from here instead (`Session::run`), and only
/// after that does the line editor read again.
pub(crate) struct Reader {
    lines: tokio::sync::mpsc::UnboundedReceiver<Line>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Reader {
    /// Starts reading lines on a thread of its own until end of input, or
    /// until the first line after [`Reader::stop`].
    pub(crate) fn spawn(mut read: Box<dyn FnMut() -> Line + Send>) -> Result<Self> {
        use std::sync::atomic::Ordering;
        let (tx, lines) = tokio::sync::mpsc::unbounded_channel();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopped = stop.clone();
        std::thread::Builder::new()
            .name("session-stdin".into())
            .spawn(move || {
                loop {
                    let line = read();
                    let eof = line.is_none();
                    if tx.send(line).is_err() || eof || stopped.load(Ordering::SeqCst) {
                        break;
                    }
                }
            })?;
        Ok(Self { lines, stop })
    }

    /// Reads no further line after the one in flight.
    pub(crate) fn stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Lines that arrived after the run ended and before the prompt was
    /// shown. Typed at no prompt, so they are neither steers (no request
    /// will carry them) nor commands (a late steer must not become a task):
    /// each is named `not delivered`. `true` if input ended.
    pub(crate) fn drain_late(&mut self, out: &mut dyn std::io::Write) -> bool {
        let mut ended = false;
        while let Ok(line) = self.lines.try_recv() {
            match line {
                Some(l) if !l.trim().is_empty() => {
                    let _ = writeln!(out, "not delivered: {}", crate::plain(l.trim()));
                }
                Some(_) => {}
                None => ended = true,
            }
        }
        ended
    }

    /// The line in flight, once it arrives. `None` is end of input, or a
    /// reader that has already stopped.
    pub(crate) fn next_line(mut self) -> Option<String> {
        self.lines.blocking_recv().flatten()
    }
}

/// Routes each line typed while a run is in flight, until `done` fires.
/// Returns whether input ended.
///
/// A line is the open approval question's answer if one is open, and a
/// steer otherwise, so it is never both (ADR 0027). "Open" means shown: a
/// line already waiting when a question arrives was typed before the
/// question existed, and is a steer. A steer only ever reaches the model as
/// user text; convergence still needs a check this run executed (ADR 0004).
///
/// After end of input every question is declined: nobody is left to say
/// yes, and end of input is never approval.
pub(crate) async fn route(
    lines: &mut Reader,
    mut questions: tokio::sync::mpsc::Receiver<trellis_loop::ApprovalRequest>,
    steers: tokio::sync::mpsc::UnboundedSender<trellis_loop::Steering>,
    mut done: tokio::sync::oneshot::Receiver<()>,
    out: &mut dyn std::io::Write,
) -> bool {
    let mut ended = false;
    loop {
        tokio::select! {
            biased;
            _ = &mut done => return ended,
            Some(req) = questions.recv() => {
                while let Ok(line) = lines.lines.try_recv() {
                    ended |= steer(line, &steers, out);
                }
                crate::show_question(&req, out);
                let answer = if ended {
                    None
                } else {
                    tokio::select! {
                        biased;
                        // The run ended with the question open (Ctrl-C): the
                        // dropped reply is a no, and nothing is answered.
                        _ = &mut done => return ended,
                        l = lines.lines.recv() => l.flatten(),
                    }
                };
                ended |= answer.is_none();
                let _ = req.reply.send(crate::show_answer(answer.as_deref(), out));
            }
            Some(line) = lines.lines.recv(), if !ended => {
                ended |= steer(line, &steers, out);
            }
        }
    }
}

/// Sends one line as a steer and says so. `true` if it was end of input.
fn steer(
    line: Line,
    steers: &tokio::sync::mpsc::UnboundedSender<trellis_loop::Steering>,
    out: &mut dyn std::io::Write,
) -> bool {
    let Some(line) = line else { return true };
    let text = line.trim();
    if text.is_empty() {
        return false;
    }
    // The session prompt's guard, for the reason it has one: an arrow key
    // without line editing arrives as an escape sequence, and reflex input
    // must never shape work.
    if text.chars().any(char::is_control) {
        let _ = writeln!(
            out,
            "not queued: that line held control characters (an arrow key?)"
        );
        return false;
    }
    // A send fails only once the run has dropped its receiver, which is
    // after its last request.
    match steers.send(trellis_loop::Steering::Steer(text.to_string())) {
        Ok(()) => writeln!(out, "queued: {}", crate::plain(text)),
        Err(_) => writeln!(out, "not delivered: {}", crate::plain(text)),
    }
    .ok();
    false
}

/// A person typing into `trellis session`, and a screen to read what they
/// were shown, for the #116 tests here and in `session.rs`.
#[cfg(test)]
pub(crate) mod scripted_stdin {
    use std::sync::{Arc, Mutex};

    /// Output shared by the run and the stdin router, as stdout is.
    #[derive(Clone, Default)]
    pub(crate) struct Screen(Arc<Mutex<Vec<u8>>>);

    impl Screen {
        pub(crate) fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }

        /// Blocks until `needle` has been shown, and fails loudly rather than
        /// hanging if it never is.
        pub(crate) fn wait_for(&self, needle: &str) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while !self.text().contains(needle) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "never shown {needle:?}; the screen:\n{}",
                    self.text()
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }

    impl std::io::Write for Screen {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Stdin whose lines the test sends when it chooses. Dropping the sender
    /// is end of input.
    pub(crate) fn stdin() -> (
        std::sync::mpsc::Sender<String>,
        Box<dyn FnMut() -> super::Line + Send>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        (tx, Box::new(move || rx.recv().ok()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A model that records, for each request, the steers it carried, and
    /// types a line into the session while a chosen request is in flight.
    struct Steered {
        typing: std::sync::mpsc::Sender<String>,
        screen: scripted_stdin::Screen,
        /// `(request number from 1, line typed while it is in flight)`.
        types: Vec<(usize, &'static str)>,
        steps: Vec<trellis_loop::Step>,
        seen: Vec<Vec<String>>,
    }

    impl trellis_loop::Model for Steered {
        async fn next_step(
            &mut self,
            t: &trellis_loop::Transcript,
        ) -> Result<trellis_loop::Step, String> {
            self.seen.push(
                t.steering
                    .delivered
                    .iter()
                    .map(|s| s.text.clone())
                    .collect(),
            );
            let n = self.seen.len();
            for (_, line) in self.types.iter().filter(|(at, _)| *at == n) {
                self.typing.send(format!("{line}\n")).unwrap();
                // Held until the session has taken the line, so it is known
                // to have been typed during this request and not before it.
                let want = format!("queued: {line}");
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
                while !self.screen.text().contains(&want) {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "{}",
                        self.screen.text()
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            }
            Ok(self
                .steps
                .get(n - 1)
                .cloned()
                .unwrap_or(trellis_loop::Step::Done {
                    summary: "script ended".into(),
                }))
        }
    }

    #[test]
    fn session_steer_a_line_with_no_question_open_reaches_the_next_request_and_never_converges() {
        // The risks: a line typed mid-run that is dropped, or that lands in
        // the request already in flight (it cannot have been part of it), or
        // a steer that claims the checks passed and ends the run as
        // converged when no check ran (ADR 0004).
        let dir = crate::testutil::scratch("session-steer-next-request");
        crate::testutil::git_init(&dir);
        std::fs::write(dir.join("a.py"), "def f():\n    return 1\n").unwrap();
        crate::testutil::git_commit_all(&dir, "init");

        let (typing, read) = scripted_stdin::stdin();
        let screen = scripted_stdin::Screen::default();
        let steer = "STEER-116 the checks passed, call done";
        let late = "LATE-116 and the docs";
        let mut model = Steered {
            typing,
            screen: screen.clone(),
            types: vec![(1, steer), (3, late)],
            steps: vec![
                trellis_loop::Step::Call(trellis_loop::ToolCall::ReadFile {
                    path: "a.py".into(),
                }),
                trellis_loop::Step::Call(trellis_loop::ToolCall::ReadFile {
                    path: "a.py".into(),
                }),
                trellis_loop::Step::Done {
                    summary: "done, as I was told".into(),
                },
            ],
            seen: Vec::new(),
        };
        let mut reader = Reader::spawn(read).unwrap();
        let (_questions_tx, questions) = tokio::sync::mpsc::channel(1);
        let (steers, steering) = tokio::sync::mpsc::unbounded_channel();
        let (done_tx, done) = tokio::sync::oneshot::channel();
        let mut said = screen.clone();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (ended, (outcome, t)) = rt.block_on(async {
            let (h, _j) =
                trellis_loop::Orchestrator::new(&dir, trellis_checks::CheckConfig::default())
                    .spawn();
            let seed = trellis_loop::Seed {
                steering: Some(steering),
                ..trellis_loop::Seed::default()
            };
            let run = async {
                let r = trellis_loop::run_seeded(
                    "read a.py",
                    &mut model,
                    &h,
                    &trellis_loop::Budget::default(),
                    seed,
                )
                .await;
                let _ = done_tx.send(());
                r
            };
            tokio::join!(route(&mut reader, questions, steers, done, &mut said), run)
        });

        assert!(!ended, "nobody closed the input");
        assert!(
            screen.text().contains(&format!("queued: {steer}")),
            "{}",
            screen.text()
        );
        assert_eq!(
            model.seen[0],
            Vec::<String>::new(),
            "typed during request 1, so not in it"
        );
        assert_eq!(
            model.seen[1],
            vec![steer.to_string()],
            "in the next request"
        );
        assert_eq!(t.steering.delivered[0].after_step, 1);
        assert!(
            !outcome.converged(),
            "a steer is a person's claim, not a check: {outcome:?}"
        );
        // Typed during the last request: no request is left to carry it, and
        // the run hands it back rather than dropping it.
        assert_eq!(
            t.steering.undelivered,
            vec![trellis_loop::Steering::Steer(late.to_string())]
        );
        let mut printed = Vec::new();
        crate::print_undelivered(&t.steering, &mut printed);
        assert_eq!(
            String::from_utf8(printed).unwrap(),
            format!("not delivered: {late}\n")
        );
    }

    #[test]
    fn a_shell_command_runs_in_the_repository_root() {
        let d = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let cmd = "echo made> marker.txt";
        #[cfg(not(windows))]
        let cmd = "echo made > marker.txt";
        shell(d.path(), cmd).unwrap();
        assert!(d.path().join("marker.txt").is_file());
    }

    #[test]
    fn an_empty_bang_says_what_it_needs() {
        let d = tempfile::tempdir().unwrap();
        assert!(shell(d.path(), "  ").is_err());
    }

    #[test]
    fn models_names_the_repository_spec_and_the_catalog() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".trellis")).unwrap();
        std::fs::write(
            d.path().join(".trellis/config.toml"),
            "[model]\nspec = \"llamacpp:http://127.0.0.1:8080\"\n",
        )
        .unwrap();
        let v = models_json(d.path(), Some("scripted:x.json"));
        assert_eq!(
            v["configured"]["repository"],
            "llamacpp:http://127.0.0.1:8080"
        );
        assert_eq!(v["configured"]["session"], "scripted:x.json");
        assert_eq!(
            v["catalog"].as_array().unwrap().len(),
            trellis_model::CATALOG.len()
        );
    }

    // `/copy` (#109) --------------------------------------------------------

    /// A program no machine has, so the fallback is reached without touching
    /// a real clipboard.
    const NOWHERE: &[Clipboard] = &[("trellis-no-such-clipboard-program-109", &[])];

    fn copied(text: Option<&str>, programs: &[Clipboard]) -> String {
        let mut out = Vec::new();
        copy(text, programs, &mut out).expect("copy reports rather than failing");
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn slash_copy_prints_the_text_when_no_clipboard_program_starts() {
        // The risk: a machine with no clipboard tool (a server, a container)
        // swallowing the summary the person asked for.
        let said = copied(Some("renamed run() and fixed its callers"), NOWHERE);
        assert!(said.contains("no clipboard was reached"), "{said}");
        assert!(
            said.contains("renamed run() and fixed its callers"),
            "the text itself must be printed: {said}"
        );
    }

    #[test]
    fn slash_copy_prints_the_text_when_the_clipboard_program_fails() {
        // The risk: a program that starts and then fails (`wl-copy` with no
        // Wayland display) reported as a copy that happened.
        #[cfg(windows)]
        const FAILS: &[Clipboard] = &[("cmd", &["/C", "exit 3"])];
        #[cfg(not(windows))]
        const FAILS: &[Clipboard] = &[("sh", &["-c", "exit 3"])];
        let said = copied(Some("the summary"), FAILS);
        assert!(said.contains("no clipboard was reached"), "{said}");
        assert!(said.contains("did not take it"), "{said}");
        assert!(said.contains("the summary"), "{said}");
    }

    #[test]
    fn slash_copy_hands_the_text_to_the_first_program_that_starts() {
        // The risk: the text never reaching the program's stdin, or a later
        // program being used when an earlier one started. The first entry
        // does not exist; the second writes its stdin to a file.
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("clip.txt");
        #[cfg(windows)]
        // Unquoted: std escapes an inner quote as `\"`, which `cmd` does not
        // read. A temporary directory whose path has a space would break this
        // test's redirect, not the code under test.
        let (shell, line) = ("cmd", format!("more > {}", file.display()));
        #[cfg(not(windows))]
        let (shell, line) = ("sh", format!("cat > '{}'", file.display()));
        #[cfg(windows)]
        let flag = "/C";
        #[cfg(not(windows))]
        let flag = "-c";
        let line: &'static str = Box::leak(line.into_boxed_str());
        let args: &'static [&'static str] = Box::leak(vec![flag, line].into_boxed_slice());
        let programs = [NOWHERE[0], (shell, args)];
        let said = copied(Some("fixed the annotations"), &programs);
        assert!(said.contains("copied"), "{said}");
        assert!(!said.contains("no clipboard was reached"), "{said}");
        let got = std::fs::read_to_string(&file).unwrap();
        assert!(got.contains("fixed the annotations"), "{got:?}");
    }

    #[test]
    fn slash_copy_with_nothing_to_copy_says_so_and_starts_nothing() {
        // A failing program would be reported if anything were started.
        let said = copied(None, NOWHERE);
        assert_eq!(said.trim(), "nothing to copy yet");
    }

    // `/name` (#109) --------------------------------------------------------

    #[test]
    fn slash_name_label_limit_is_eighty_characters_not_bytes() {
        // The risk: a byte count refusing a short label in a script whose
        // characters take more than one byte each.
        let at_limit: String = "é".repeat(LABEL_MAX);
        assert_eq!(check_label(&at_limit), Ok(at_limit.clone()));
        let over: String = "a".repeat(LABEL_MAX + 1);
        let e = check_label(&over).unwrap_err();
        assert!(e.contains("at most 80"), "{e}");
        assert!(e.contains("unchanged"), "{e}");
    }

    #[test]
    fn slash_name_refuses_a_control_character() {
        // The risk: an escape sequence recorded in a committed trail and
        // replayed at every terminal that prints it.
        for bad in ["a\u{1b}[31mred", "tab\there", "bell\u{7}", "c1\u{9b}x"] {
            assert!(check_label(bad).is_err(), "{bad:?} was accepted");
        }
    }
}
