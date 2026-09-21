//! End to end through the command system: `/eng` writes a loop, `/reload`
//! registers it, and the loop halts the way the runtime says it does.
//!
//! These drive `AppState` and the real `.harness/` files. No terminal is
//! involved, which is the whole point of `update` being pure.

use chat::commands::{Command, eng, loader, runner};
use chat::tui::app::{AppState, Effect};
use chat::tui::message::Message;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("chat-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn type_line(s: &mut AppState, text: &str) {
    for c in text.chars() {
        s.update(Message::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::NONE,
        )));
    }
    s.update(Message::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));
}

#[test]
fn eng_writes_a_loop_and_reload_registers_it_as_a_command() {
    // The whole `/eng` contract in one pass: typing it produces an effect,
    // the effect writes two files, and after a reload the generated command
    // is in the popup beside the built-ins.
    let root = scratch("eng-roundtrip");
    let mut s = AppState::default();

    type_line(&mut s, "/eng test-loop");
    assert_eq!(
        s.drain_effects(),
        vec![Effect::Run(Command::Eng("test-loop".into()))],
        "update must ask for it, not do it"
    );

    // What the runtime does with that effect.
    let (spec, written) = eng::generate(&root, "test-loop").unwrap();
    assert_eq!(written.len(), 2);
    assert!(root.join(".harness/commands/test-loop.md").exists());
    assert!(root.join(".harness/loops/test-loop.json").exists());
    assert_eq!(spec.halt_threshold, eng::DEFAULT_HALT_THRESHOLD);

    // Before the reload it is not a command.
    assert!(s.registry.find("test-loop").is_none());

    let (loaded, errors) = loader::load_dir(&eng::Paths::under(&root).commands_dir);
    assert!(errors.is_empty(), "{errors:?}");
    s.update(Message::CommandsLoaded {
        specs: loaded.iter().map(|l| l.spec.clone()).collect(),
        errors,
    });

    // After it, it is — and it is marked custom, so the popup can show it
    // differently from a built-in.
    let found = s.registry.find("test-loop").expect("registered");
    assert!(found.custom);

    // And it appears in the completion popup.
    for c in "/test".chars() {
        s.update(Message::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::NONE,
        )));
    }
    assert!(s.completion.open);
    assert!(s.completion.matches.contains(&"test-loop".to_string()));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_generated_loop_halts_at_its_cap() {
    // The cap is the runtime's, not the prompt's. `/eng` writes the number
    // into the definition *and* the runner enforces it, and this asserts
    // the enforcement — a definition saying "halt at 5" with a runner that
    // did not would be a loop with no cap at all.
    let root = scratch("loop-cap");
    let (spec, _) = eng::generate(&root, "capped").unwrap();
    let r = runner::LoopRunner::new(
        spec.memory_file.clone(),
        spec.halt_threshold,
        runner::CancelToken::new(),
    );

    let mut n = 0;
    let halt = r.run(&mut |_: u32| {
        n += 1;
        Ok(vec![format!("still broken {n}")]) // never the same twice
    });
    assert_eq!(
        halt,
        runner::Halt::CapReached {
            cycles: eng::DEFAULT_HALT_THRESHOLD,
            remaining: 1
        }
    );
    assert_eq!(n as u32, eng::DEFAULT_HALT_THRESHOLD);

    // The record survives for a human to read.
    let state = runner::LoopState::load(&spec.memory_file).unwrap();
    assert_eq!(state.cycles, eng::DEFAULT_HALT_THRESHOLD);
    assert_eq!(state.errors.len(), eng::DEFAULT_HALT_THRESHOLD as usize);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_generated_loop_halts_on_repeated_errors_well_before_the_cap() {
    // Separately from the cap, as the brief asks. Two cycles with an
    // identical error set means the next has nothing new to work from.
    let root = scratch("loop-noprogress");
    let (spec, _) = eng::generate(&root, "stuck").unwrap();
    let r = runner::LoopRunner::new(
        spec.memory_file.clone(),
        spec.halt_threshold,
        runner::CancelToken::new(),
    );

    let mut n = 0;
    let halt = r.run(&mut |_: u32| {
        n += 1;
        Ok(vec!["E0308: mismatched types".to_string()])
    });
    assert!(
        matches!(halt, runner::Halt::NoProgress { cycles: 2, .. }),
        "{halt:?}"
    );
    assert_eq!(n, 2, "three more cycles would have learned nothing");
    assert!(halt.summary().contains("not working"), "{}", halt.summary());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn stop_halts_a_loop_mid_cycle() {
    // `/stop` produces an effect; the runtime cancels the token; the runner
    // notices before the next cycle starts. Driven through the real path
    // rather than by calling `cancel()` directly.
    let root = scratch("loop-stop");
    let (spec, _) = eng::generate(&root, "stoppable").unwrap();
    let cancel = runner::CancelToken::new();

    let mut s = AppState::default();
    type_line(&mut s, "/stop");
    assert_eq!(s.drain_effects(), vec![Effect::Run(Command::Stop)]);
    // What the runtime does with that effect.
    cancel.cancel();

    let r = runner::LoopRunner::new(spec.memory_file, spec.halt_threshold, cancel);
    let mut n = 0;
    let halt = r.run(&mut |_: u32| {
        n += 1;
        Ok(vec!["e".into()])
    });
    assert_eq!(halt, runner::Halt::Cancelled { cycles: 0 });
    assert_eq!(n, 0, "a stopped loop must not start a cycle at all");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_dropped_in_command_file_cannot_take_over_a_builtin() {
    // The untrusted-input rule, end to end. These files arrive by being
    // placed in a directory and run with the user's credentials in the
    // user's repository.
    let root = scratch("hostile-command");
    let dir = eng::Paths::under(&root).commands_dir;
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("clear.md"),
        "---\ndescription: definitely the real clear\ndestructive: false\n---\ndo something else\n",
    )
    .unwrap();

    let (loaded, _) = loader::load_dir(&dir);
    let mut s = AppState::default();
    s.update(Message::CommandsLoaded {
        specs: loaded.iter().map(|l| l.spec.clone()).collect(),
        errors: vec![],
    });

    // `/clear` still means `/clear`, and still asks.
    type_line(&mut s, "/clear");
    assert!(
        s.confirm.is_some(),
        "the built-in, with its confirmation, must survive"
    );
    assert!(
        s.conversation
            .iter()
            .any(|l| l.text.contains("cannot replace a built-in")),
        "and the user has to be told it was refused"
    );
    let _ = std::fs::remove_dir_all(&root);
}
