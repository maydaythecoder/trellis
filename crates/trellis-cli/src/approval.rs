//! Approve mode at a terminal (ADR 0018): who answers an approval
//! question, how a run's control is chosen, and the prompt that asks it.
//! `run_cmd::run_body` and `run_partitioned::run_partitioned` read from
//! here; `session` shares the prompt.

use crate::{config, plain};

/// Who answers an approval question in approve mode (ADR 0018).
pub(crate) enum Human {
    /// Nobody. A run in approve mode refuses to start rather than start and
    /// refuse every edit, which would look like a run that did the work.
    Absent,
    /// This terminal: the diff and the question are written to the run's
    /// output, and the answer is one line from this reader. `None` is end of
    /// input, which is a no.
    Line(Box<dyn FnMut() -> Option<String> + Send>),
    /// A caller that answers off its own thread: `rpc`, whose client shows
    /// the request and sends `approve`, and the session's in-flight reader.
    Channel(tokio::sync::mpsc::Sender<trellis_loop::ApprovalRequest>),
}

impl Human {
    /// This process's terminal, if a person is both reading it and typing
    /// into it.
    ///
    /// Stdin because the answer has to come from somebody, and a pipe is not
    /// somebody. Stdout as well because the diff and the question are written
    /// there: with stdout redirected to a file, a person at the keyboard would
    /// be answering a question they cannot see, and a `y` given blind is not
    /// an approval.
    pub(crate) fn terminal() -> Self {
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            Human::Line(Box::new(|| {
                let mut line = String::new();
                match std::io::stdin().read_line(&mut line) {
                    Ok(0) | Err(_) => None,
                    Ok(_) => Some(line),
                }
            }))
        } else {
            Human::Absent
        }
    }
}

/// How much a run asks before it acts: the flag, if any, over `[control]`.
pub(crate) struct RunControl {
    /// `--approve` or `--auto`; `None` takes the configured mode.
    pub(crate) flag: Option<config::ControlMode>,
    pub(crate) human: Human,
    /// What the person types while the run is in flight (#113), delivered
    /// between turns. Beside `human` because both are a person reaching into
    /// a running task, and on its own channel so a steer can never be read
    /// as an approval answer. `None` for every caller but `trellis tui`
    /// (#115), `session` at a terminal (#116) and `rpc`'s `steer` (#114,
    /// ADR 0025 §5).
    pub(crate) steering: Option<trellis_loop::SteeringRx>,
}

impl Default for RunControl {
    fn default() -> Self {
        Self {
            flag: None,
            human: Human::Absent,
            steering: None,
        }
    }
}

/// The exact refusal ADR 0018 names, so a script can match on it.
///
/// Both ways out are named because `session` reaches this too, and `--auto`
/// is a `run` flag: a session user told only to pass it has nowhere to pass it.
pub(crate) const NEEDS_A_TERMINAL: &str =
    "approve mode needs an interactive terminal; pass --auto, or set [control] mode = \"auto\"";

/// Whether a typed answer is a yes. Only an explicit one counts: the prompt
/// says `[y/N]`, and an empty line, a typo or end of input is the N.
fn is_yes(answer: Option<&str>) -> bool {
    matches!(
        answer.map(|a| a.trim().to_ascii_lowercase()).as_deref(),
        Some("y" | "yes")
    )
}

/// Answers approval questions on a line terminal until the channel closes.
///
/// The read runs on the blocking pool, not on the runtime thread, so a
/// Ctrl-C while the question is up still reaches the select in `cmd_run`
/// and aborts the run with a rollback. The reader is handed to the pool and
/// back each time because it is one stdin and there is only ever one
/// question open.
pub(crate) async fn answer_on_the_line(
    mut rx: tokio::sync::mpsc::Receiver<trellis_loop::ApprovalRequest>,
    mut read: Box<dyn FnMut() -> Option<String> + Send>,
    out: &mut dyn std::io::Write,
) {
    while let Some(req) = rx.recv().await {
        show_question(&req, out);
        let (back, answer) = match tokio::task::spawn_blocking(move || {
            let a = read();
            (read, a)
        })
        .await
        {
            Ok(r) => r,
            // The reader panicked. Nothing can be asked again, so every
            // later question is declined by the dropped channel.
            Err(_) => return,
        };
        read = back;
        let _ = req.reply.send(show_answer(answer.as_deref(), out));
    }
}

/// What the person typed that no request carried: a steer that arrived
/// after the last request. Printed so no text they typed goes missing
/// without a word (#116).
pub(crate) fn print_undelivered(log: &trellis_loop::SteeringLog, out: &mut dyn std::io::Write) {
    for m in &log.undelivered {
        let (trellis_loop::Steering::Steer(text) | trellis_loop::Steering::FollowUp(text)) = m;
        let _ = writeln!(out, "not delivered: {}", plain(text));
    }
}

/// An approval question as the line terminal shows it: the detail, then the
/// question with its `[y/N]`. Shared with `session`'s reader (#116), so a
/// question asked there reads exactly as one asked here.
pub(crate) fn show_question(req: &trellis_loop::ApprovalRequest, out: &mut dyn std::io::Write) {
    let _ = writeln!(out);
    for line in req.detail.lines() {
        let _ = writeln!(out, "  {}", plain(line));
    }
    let _ = write!(out, "{}? [y/N] ", plain(&req.question));
    let _ = out.flush();
}

/// Reads a typed answer (`None` is end of input), says what it was taken
/// as, and returns whether it is a yes.
pub(crate) fn show_answer(answer: Option<&str>, out: &mut dyn std::io::Write) -> bool {
    let yes = is_yes(answer);
    if answer.is_none() {
        // End of input leaves the cursor on the question line.
        let _ = writeln!(out);
    }
    let _ = writeln!(out, "  {}", if yes { "approved" } else { "declined" });
    yes
}

#[cfg(test)]
mod approval_tests {
    //! ADR 0018 end to end through `cmd_run`: real files, a real snapshot and
    //! rollback, a scripted model, and a person played by a closure.

    use super::*;
    use crate::run_cmd::{RunExit, RunSeeding, cmd_run};
    use anyhow::Result;
    use std::path::{Path, PathBuf};

    /// A committed repository in approve mode, and a script outside it.
    ///
    /// The script lives elsewhere because an untracked file inside the
    /// repository would make the worktree dirty, and a run refuses that.
    fn fixture(name: &str, script: &str) -> (PathBuf, String) {
        let dir = crate::testutil::scratch(&format!("approval-{name}"));
        crate::testutil::git_init(&dir);
        std::fs::create_dir_all(dir.join(".trellis")).unwrap();
        std::fs::write(
            dir.join(".trellis/config.toml"),
            "[control]\nmode = \"approve\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("a.py"), "def f():\n    return 1\n").unwrap();
        crate::testutil::git_commit_all(&dir, "init");
        let scripts = crate::testutil::scratch(&format!("approval-{name}-script"));
        let path = scripts.join("script.json");
        std::fs::write(&path, script).unwrap();
        (dir, format!("scripted:{}", path.display()))
    }

    const EDIT_THEN_STOP: &str = r#"{"steps":[
        {"tool":"edit","path":"a.py","old":"return 1","new":"return 2"},
        {"tool":"done","summary":"changed it"}
    ]}"#;

    /// A person who gives these answers, in order, then closes the input.
    fn answering(answers: &[&str]) -> Human {
        let mut left: Vec<String> = answers.iter().rev().map(|s| format!("{s}\n")).collect();
        Human::Line(Box::new(move || left.pop()))
    }

    fn run_with(dir: &Path, spec: &str, human: Human) -> (Result<RunExit>, String) {
        let mut out = Vec::new();
        let r = cmd_run(
            dir,
            "change f",
            spec,
            false,
            RunSeeding::default(),
            RunControl {
                flag: None,
                human,
                steering: None,
            },
            &mut out,
        );
        (r, String::from_utf8(out).unwrap())
    }

    /// The one trail the run wrote, read back the way `trellis log` reads it.
    fn trail(dir: &Path) -> trellis_trail::UntrustedFactual {
        let runs = dir.join(".trellis").join("runs");
        let file = std::fs::read_dir(&runs)
            .unwrap()
            .filter_map(Result::ok)
            .find(|e| e.path().extension().is_some_and(|x| x == "json"))
            .expect("a trail was written");
        let raw = std::fs::read_to_string(file.path()).unwrap();
        let t = trellis_trail::UntrustedTrail::from_json(&raw).unwrap();
        // Every trail here carries the new `control` field and approval
        // actions, so every read-back also checks the digest still covers
        // them: a field added to `factual` that the digest missed would make
        // each of these trails read as altered.
        assert_eq!(t.integrity(&raw), trellis_trail::Integrity::Intact);
        t.factual
    }

    fn actions(f: &trellis_trail::UntrustedFactual) -> Vec<(String, String, String, bool)> {
        f.actions
            .iter()
            .map(|a| (a.tool.clone(), a.target.clone(), a.outcome.clone(), a.wrote))
            .collect()
    }

    /// `fixture`, with `tools` appended to the approve-mode config and the
    /// repository trusted, so a `[[tools.custom]]` in it is enabled.
    fn fixture_with_tools(name: &str, tools: &str, script: &str) -> (PathBuf, String) {
        let (dir, spec) = fixture(name, script);
        std::fs::write(
            dir.join(".trellis/config.toml"),
            format!("[control]\nmode = \"approve\"\n{tools}"),
        )
        .unwrap();
        crate::testutil::git_commit_all(&dir, "tools");
        crate::testutil::trust(&dir);
        (dir, spec)
    }

    /// `write_file` enabled, and a repository tool `stamp` that exits 0.
    fn writers() -> String {
        let argv = if cfg!(windows) {
            "['cmd', '/C', 'exit 0']"
        } else {
            "['sh', '-c', 'true']"
        };
        format!(
            "[tools]\nenable = ['write_file']\n\
             [[tools.custom]]\nname = \"stamp\"\ndescription = \"Stamp.\"\n\
             command = {argv}\nschema = {{ type = \"object\" }}\n"
        )
    }

    const WRITE_THEN_STOP: &str = r#"{"steps":[
        {"tool":"write_file","path":"pkg/new.py","content":"x = 1\n"},
        {"tool":"done","summary":"made it"}
    ]}"#;

    const STAMP_THEN_STOP: &str = r#"{"steps":[
        {"tool":"stamp"},
        {"tool":"done","summary":"stamped"}
    ]}"#;

    #[test]
    fn approve_mode_asks_before_write_file_with_the_default_list() {
        // The risk (#425): the default list is `["edit"]` and an unlisted
        // tool runs unasked, so in approve mode `write_file` created files
        // without the yes the mode promises.
        let (dir, spec) = fixture_with_tools("asks-write-file", &writers(), WRITE_THEN_STOP);
        let (r, said) = run_with(&dir, &spec, answering(&["n"]));
        assert!(r.is_ok(), "{r:?}");
        assert!(
            said.contains("write_file pkg/new.py (+1 new)? [y/N]"),
            "asked before the write:\n{said}"
        );
        let t = trail(&dir);
        assert_eq!(
            t.control.as_deref(),
            Some("approve (edit, write_file, stamp)")
        );
        let a = actions(&t);
        assert_eq!((a[0].0.as_str(), a[0].2.as_str()), ("approval", "declined"));
        assert_eq!(a[1].0, "write_file");
        assert!(!a[1].3, "a declined write_file wrote nothing: {a:?}");
    }

    #[test]
    fn approve_mode_asks_before_a_custom_tool_with_the_default_list() {
        // The same hole for a repository's own tool: it runs a command that
        // can change the tree, and ran unasked in approve mode (#425).
        let (dir, spec) = fixture_with_tools("asks-custom", &writers(), STAMP_THEN_STOP);
        let (r, said) = run_with(&dir, &spec, answering(&["n"]));
        assert!(r.is_ok(), "{r:?}");
        assert!(said.contains("stamp"), "{said}");
        assert!(said.contains("[y/N]"), "asked before the tool:\n{said}");
        let a = actions(&trail(&dir));
        assert_eq!(
            (a[0].0.as_str(), a[0].2.as_str()),
            ("approval", "declined"),
            "{a:?}"
        );
        assert_eq!(a[1].0, "stamp");
        assert!(a[1].2.contains(trellis_loop::DECLINED), "{a:?}");
    }

    #[test]
    fn approve_mode_asks_nothing_under_the_auto_flag_with_both_enabled() {
        // The widened default is approve mode's; `--auto` still asks
        // nothing, and the write lands.
        let (dir, spec) = fixture_with_tools("asks-auto", &writers(), WRITE_THEN_STOP);
        let mut out = Vec::new();
        let r = cmd_run(
            &dir,
            "change f",
            &spec,
            false,
            RunSeeding::default(),
            RunControl {
                flag: Some(config::ControlMode::Auto),
                human: Human::Absent,
                steering: None,
            },
            &mut out,
        );
        assert!(r.is_ok(), "{r:?}");
        let said = String::from_utf8(out).unwrap();
        assert!(!said.contains("[y/N]"), "auto asks nothing: {said}");
        let t = trail(&dir);
        assert_eq!(t.control.as_deref(), Some("auto"));
        let a = actions(&t);
        assert_eq!((a[0].0.as_str(), a[0].3), ("write_file", true), "{a:?}");
    }

    #[test]
    fn a_queued_run_that_asks_ends_as_asked_with_the_question() {
        // #248. The risk: a restored run that ended on `ask_human` reported
        // as an ordinary rollback, so `work` never parks the task on the
        // question and the person is never asked.
        let (dir, spec) = fixture(
            "asks-human",
            r#"{"steps":[{"tool":"ask_human","question":"Keep the v1 endpoint?","options":["yes","no"]}]}"#,
        );
        let mut out = Vec::new();
        let r = cmd_run(
            &dir,
            "change f",
            &spec,
            false,
            RunSeeding {
                queued: true,
                ..RunSeeding::default()
            },
            RunControl {
                flag: Some(config::ControlMode::Auto),
                human: Human::Absent,
                steering: None,
            },
            &mut out,
        );
        let said = String::from_utf8(out).unwrap();
        assert_eq!(
            r.unwrap(),
            RunExit::Asked {
                question: "Keep the v1 endpoint?".to_string(),
                options: vec!["yes".to_string(), "no".to_string()],
            },
            "{said}"
        );
    }

    #[test]
    fn an_approve_list_someone_wrote_is_not_widened() {
        // #425 widens only the default. A repository that wrote
        // `approve = ["edit"]` asked for edits only, and gets that.
        let (dir, spec) = fixture_with_tools(
            "asks-explicit",
            &format!("approve = [\"edit\"]\n{}", writers()),
            WRITE_THEN_STOP,
        );
        let (r, said) = run_with(&dir, &spec, answering(&[]));
        assert!(r.is_ok(), "{r:?}");
        assert!(!said.contains("[y/N]"), "{said}");
        let t = trail(&dir);
        assert_eq!(t.control.as_deref(), Some("approve (edit)"));
        assert_eq!(actions(&t)[0].0, "write_file");
    }

    #[test]
    fn approve_mode_with_nobody_to_ask_refuses_before_the_snapshot() {
        // Starting and then denying every edit would be a run that looks as
        // though it tried. It must not start, and it must cost nothing: no
        // snapshot, no rollback, no trail.
        let (dir, spec) = fixture("no-tty", EDIT_THEN_STOP);
        let (r, said) = run_with(&dir, &spec, Human::Absent);
        let e = r.expect_err("approve mode with nobody to ask must refuse");
        assert_eq!(format!("{e}"), NEEDS_A_TERMINAL);
        assert!(!said.contains("snapshot"), "{said}");
        assert!(
            !dir.join(".trellis/runs").exists(),
            "no trail for a run that never began"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("a.py")).unwrap(),
            "def f():\n    return 1\n"
        );
    }

    #[test]
    fn the_auto_flag_needs_nobody_even_where_the_config_says_approve() {
        let (dir, spec) = fixture("auto-flag", EDIT_THEN_STOP);
        let mut out = Vec::new();
        let r = cmd_run(
            &dir,
            "change f",
            &spec,
            false,
            RunSeeding::default(),
            RunControl {
                flag: Some(config::ControlMode::Auto),
                human: Human::Absent,
                steering: None,
            },
            &mut out,
        );
        assert!(r.is_ok(), "{r:?}");
        let said = String::from_utf8(out).unwrap();
        assert!(said.contains("control:  auto"), "{said}");
        assert!(!said.contains("[y/N]"), "auto asks nothing: {said}");
        assert_eq!(trail(&dir).control.as_deref(), Some("auto"));
    }

    #[test]
    fn a_declined_edit_is_not_applied_and_the_trail_says_declined() {
        let (dir, spec) = fixture("declined", EDIT_THEN_STOP);
        let (r, said) = run_with(&dir, &spec, answering(&["n"]));
        assert!(r.is_ok(), "{r:?}");

        // Asked on the line, with the diff first.
        let diff_at = said.find("- ").expect("the removed line is shown");
        let question = said
            .find("edit a.py (+1 -1)? [y/N]")
            .unwrap_or_else(|| panic!("the question in the ADR's words:\n{said}"));
        assert!(
            diff_at < question,
            "the diff comes before the question:\n{said}"
        );
        assert!(
            said.contains("-     return 1") && said.contains("+     return 2"),
            "{said}"
        );
        assert!(said.contains("declined"), "{said}");

        // Not applied. The run did not converge and rolled back, so the file
        // being unchanged proves nothing on its own; the trail's `wrote` bit
        // is what says the edit never touched the tree.
        let t = trail(&dir);
        assert_eq!(t.control.as_deref(), Some("approve (edit)"));
        let a = actions(&t);
        assert_eq!(
            a[0],
            (
                "approval".to_string(),
                "edit a.py".to_string(),
                "declined".to_string(),
                false
            ),
            "{a:?}"
        );
        assert_eq!(a[1].0, "edit");
        assert!(
            a[1].2.contains("declined by the user"),
            "the model is told why: {a:?}"
        );
        assert!(!a[1].3, "a declined edit wrote nothing: {a:?}");
        assert!(t.proposed.is_empty(), "{:?}", t.proposed);
    }

    #[test]
    fn an_approved_edit_is_applied_and_the_trail_says_approved() {
        let (dir, spec) = fixture("approved", EDIT_THEN_STOP);
        let (r, said) = run_with(&dir, &spec, answering(&["y"]));
        assert!(r.is_ok(), "{r:?}");
        assert!(said.contains("approved"), "{said}");
        let a = actions(&trail(&dir));
        assert_eq!((a[0].0.as_str(), a[0].2.as_str()), ("approval", "approved"));
        assert_eq!(a[1].0, "edit");
        assert!(a[1].3, "an approved edit is applied: {a:?}");
    }

    #[test]
    fn end_of_input_on_the_question_is_a_no() {
        // A closed or piped stdin returns nothing. Reading that as yes would
        // let a run in approve mode edit with nobody watching.
        let (dir, spec) = fixture("eof", EDIT_THEN_STOP);
        let (r, said) = run_with(&dir, &spec, answering(&[]));
        assert!(r.is_ok(), "{r:?}");
        assert!(said.contains("declined"), "{said}");
        let a = actions(&trail(&dir));
        assert_eq!(a[0].2, "declined");
        assert!(!a[1].3);
    }

    #[test]
    fn only_an_explicit_yes_counts() {
        for yes in ["y", "Y", "yes", " y \r\n"] {
            assert!(is_yes(Some(yes)), "{yes:?}");
        }
        for no in ["", "\n", "n", "no", "yy", "ok"] {
            assert!(!is_yes(Some(no)), "{no:?}");
        }
        assert!(!is_yes(None));
    }

    #[test]
    fn the_interface_s_channel_is_asked_and_its_no_is_recorded() {
        // The path `trellis tui` takes: the gate's question goes out on a
        // channel rather than to a terminal, and the answer comes back on the
        // request's own `oneshot`.
        let (dir, spec) = fixture("channel", EDIT_THEN_STOP);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<trellis_loop::ApprovalRequest>(1);
        let asker = std::thread::spawn(move || {
            let req = rx.blocking_recv().expect("the gate asks");
            let q = req.question.clone();
            let _ = req.reply.send(false);
            q
        });
        let (r, said) = run_with(&dir, &spec, Human::Channel(tx));
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(asker.join().unwrap(), "edit a.py (+1 -1)");
        assert!(
            !said.contains("[y/N]"),
            "nothing is asked on the line: {said}"
        );
        let a = actions(&trail(&dir));
        assert_eq!((a[0].0.as_str(), a[0].2.as_str()), ("approval", "declined"));
        assert!(!a[1].3);
    }

    /// The run's futures are polled on `RUN_THREAD`, not on the caller's
    /// stack. Moving `block_on` back inline would leave the binary's run on
    /// a 1 MiB Windows main thread that a debug build had all but used up
    /// (#180), and nothing else would fail until a future grew past it.
    #[test]
    fn loop_stack_a_run_is_polled_on_the_run_thread_not_the_callers() {
        let (dir, spec) = fixture("loop-stack", EDIT_THEN_STOP);
        crate::RUN_POLLED_ON.with(|c| *c.borrow_mut() = None);
        let mut out = Vec::new();
        let r = cmd_run(
            &dir,
            "change f",
            &spec,
            false,
            RunSeeding::default(),
            RunControl {
                flag: Some(config::ControlMode::Auto),
                human: Human::Absent,
                steering: None,
            },
            &mut out,
        );
        assert!(r.is_ok(), "{r:?}");
        let polled_on = crate::RUN_POLLED_ON.with(|c| c.borrow_mut().take());
        assert_eq!(polled_on.as_deref(), Some(crate::RUN_THREAD));
        assert_ne!(std::thread::current().name(), Some(crate::RUN_THREAD));
    }
}
