//! `cargo run --bin chat` — the interface, wired to a model or to the mock.
//!
//! This file is the *runtime*: the half that performs effects. `update`
//! produces `Effect`s and never carries one out, so everything that touches
//! the filesystem, git, a process or the network is here, on its own task.
//!
//! Set `CHAT_MODEL` and `OPENAI_BASE_URL` to talk to a real endpoint:
//!
//! ```text
//! ollama pull qwen3:8b
//! CHAT_MODEL=qwen3:8b cargo run --bin chat
//! ```
//!
//! With neither set, or with `CHAT_MOCK=1`, it runs against the mock stream
//! — which is the point of the mock: Part B does not block on Part A.

use chat::commands::{Command, GitRef, eng, loader, runner};
use chat::model::{client, mock};
use chat::tui::app::{AppState, Effect};
use chat::tui::message::Message;
use chat::tui::theme::Theme;
use chat::tui::{events, terminal};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;

#[tokio::main(flavor = "current_thread")]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    // Refused rather than attempted when there is no terminal to drive.
    //
    // Found by running it: `EventStream` waits on console input that never
    // arrives from a pipe, so the interface paints one frame and then hangs
    // until it is killed — and a kill runs no destructors, which is the one
    // way to leave a terminal broken that no guard here can cover. Checked
    // before `install_logging` so the message is the only thing on stdout.
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!(
            "`chat` needs a terminal, and this is a pipe or a redirect.\n\
             Run it directly. For a non-interactive check of the model, use\n\
             `cargo run --bin toolcall_smoke`."
        );
        std::process::exit(2);
    }

    install_logging();

    let root = std::env::current_dir()?;
    let theme = Theme::default();

    let (to_ui, from_work) = mpsc::channel::<Message>(events::CHANNEL_CAPACITY);
    let (to_work, effects) = mpsc::channel::<Effect>(events::CHANNEL_CAPACITY);

    let mut state = AppState::default();
    let use_mock = std::env::var("CHAT_MOCK").is_ok()
        || (std::env::var("CHAT_MODEL").is_err() && std::env::var("OPENAI_BASE_URL").is_err());

    let model_client = if use_mock {
        state.model = "mock".into();
        state.tier = "mock stream".into();
        None
    } else {
        let config = client::Config::default();
        state.model = config.model.clone();
        // Ollama does not expose grammar-constrained decoding; llama.cpp
        // does. Naming which one is in play matters because A.5's whole
        // point is that the grammar handles format and the model handles
        // judgment — a user who cannot see which tier they are on cannot
        // read a malformed-output failure correctly.
        state.tier = if config.base_url.contains("11434") {
            "ollama (no grammar)".into()
        } else {
            "openai-compatible".into()
        };
        state.context_window = 32_768;
        Some(Arc::new(client::Client::new(config)?))
    };

    // Custom commands, before the first frame, so the popup is complete the
    // first time it opens.
    let (loaded, errors) = loader::load_dir(&eng::Paths::under(&root).commands_dir);
    let _ = to_ui
        .send(Message::CommandsLoaded {
            specs: loaded.iter().map(|l| l.spec.clone()).collect(),
            errors,
        })
        .await;

    let cancel = runner::CancelToken::new();
    let worker = tokio::spawn(run_effects(
        root.clone(),
        effects,
        to_ui.clone(),
        model_client.clone(),
        cancel.clone(),
    ));
    terminal::install_signal_handlers(to_ui.clone());

    let mut term = terminal::init();
    let result = events::run(
        &mut term,
        &mut state,
        &theme,
        from_work,
        to_work,
        cancel.clone(),
    )
    .await;
    terminal::restore();

    // Stopped *after* the terminal is handed back, so a child process
    // writing on its way out lands on a sane screen rather than into the
    // alternate buffer.
    cancel.cancel();
    worker.abort();
    result?;
    Ok(())
}

/// Drains effects and performs them. The only place IO happens.
async fn run_effects(
    root: PathBuf,
    mut rx: mpsc::Receiver<Effect>,
    to_ui: mpsc::Sender<Message>,
    client: Option<Arc<client::Client>>,
    cancel: runner::CancelToken,
) {
    // Rebuilt here rather than shared with the UI: the runtime needs the
    // message history to send, and the UI needs it to draw, and the two are
    // different shapes.
    const SYSTEM: &str = "You are a concise coding assistant working inside a terminal interface.";
    let mut history: Vec<client::ChatMessage> = vec![client::ChatMessage::system(SYSTEM)];
    // What `/add` injected, kept beside the history so `/context` can say
    // what is loaded and what each item costs. Held here rather than in
    // `AppState` because it is the runtime that sends it.
    let mut context: Vec<(String, String)> = Vec::new();

    // Loops get their own cancellation, replaced after every `/stop`.
    //
    // Sharing one token with the stream path meant a `/stop` left it
    // cancelled forever: the next loop halted before its first cycle. A
    // fresh token per stop is what lets `/stop` mean "stop what is running"
    // rather than "stop everything from now on".
    let mut loop_cancel = runner::CancelToken::new();

    while let Some(effect) = rx.recv().await {
        match effect {
            Effect::Prompt(text) => {
                // A previous `/stop` left this set; clearing it here rather
                // than when the stop was handled means two stops in a row
                // cannot race a reset into reviving something meant to die.
                cancel.reset();
                history.push(client::ChatMessage::user(&text));
                let _ = to_ui
                    .send(Message::ContextUsed {
                        tokens: total(&history),
                    })
                    .await;

                let (ctx, mut crx) = mpsc::channel(events::CHANNEL_CAPACITY);
                match &client {
                    Some(c) => {
                        let c = c.clone();
                        let msgs = history.clone();
                        tokio::spawn(async move {
                            // Offered, then retried without if the model
                            // cannot take them.
                            //
                            // Not every weight supports tools, and the
                            // endpoint refuses the whole request when one
                            // does not — ollama answers "llama3:latest does
                            // not support tools" and returns nothing. So
                            // offering them unconditionally turns a model
                            // that worked into one that fails every turn,
                            // which is what happened the moment they were
                            // wired up. One retry, and the user is told the
                            // model is answering without tools rather than
                            // left wondering why the graph never fills.
                            let offered = tools();
                            let first = c.stream(&msgs, Some(&offered), ctx.clone()).await;
                            if let Err(e) = first {
                                if e.contains("does not support tools") {
                                    let _ = ctx
                                        .send(client::Chunk::Token(
                                            "! this model does not support tools;                                              answering without them
"
                                                .into(),
                                        ))
                                        .await;
                                    if let Err(e2) = c.stream(&msgs, None, ctx.clone()).await {
                                        let _ =
                                            ctx.send(client::Chunk::Token(format!("! {e2}"))).await;
                                    }
                                } else {
                                    let _ = ctx.send(client::Chunk::Token(format!("! {e}"))).await;
                                }
                                let _ = ctx.send(client::Chunk::Done).await;
                            }
                        });
                    }
                    None => {
                        tokio::spawn(async move { mock::stream(&text, ctx).await });
                    }
                }

                let mut answer = String::new();
                let mut interrupted = false;
                while let Some(chunk) = crx.recv().await {
                    if cancel.is_cancelled() {
                        interrupted = true;
                        break;
                    }
                    match chunk {
                        client::Chunk::Token(t) => {
                            answer.push_str(&t);
                            let _ = to_ui.send(Message::StreamToken(t)).await;
                        }
                        client::Chunk::ToolCall { name, .. } => {
                            let _ = to_ui.send(Message::ToolCalled { name }).await;
                        }
                        client::Chunk::Done => break,
                    }
                }
                // An interrupted answer is **discarded**, not stored.
                //
                // Hand a model a truncated assistant turn and it does the
                // obvious thing: it continues it. Measured against llama3 —
                // a twelve-paragraph essay interrupted at paragraph two,
                // then a request for a single word, and the reply was
                // paragraph five of the essay. The same request against a
                // cleared history answered correctly, which is what isolates
                // the cause to the history rather than to the model.
                //
                // Marking it inline was tried first and did not hold: too
                // weak a steer for an 8B model against the pull of an
                // unfinished paragraph. Dropping the partial turn is also
                // the honest reading — the user stopped that answer because
                // they did not want it.
                if interrupted {
                    // A short assistant turn, not a system message and not
                    // the partial text.
                    //
                    // Three shapes were tried against llama3, in this
                    // order. Storing the truncated answer made the model
                    // continue it — an essay interrupted at paragraph two,
                    // then a request for one word, and the reply was
                    // paragraph five. Marking it inline
                    // (`[interrupted; do not continue]`) did not hold: too
                    // weak a steer for an 8B model against the pull of an
                    // unfinished paragraph. Injecting a *system* note was
                    // worse — a second system message mid-conversation made
                    // it echo the first one back verbatim as its answer.
                    //
                    // An assistant turn keeps the roles alternating, which
                    // is what the chat template expects, and gives the model
                    // nothing to continue from.
                    history.push(client::ChatMessage {
                        role: "assistant".into(),
                        content: "(interrupted)".into(),
                    });
                } else {
                    history.push(client::ChatMessage {
                        role: "assistant".into(),
                        content: answer,
                    });
                }
                // After the reply, not only before the prompt: otherwise
                // the figure shown is always half a turn stale.
                let _ = to_ui
                    .send(Message::ContextUsed {
                        tokens: total(&history),
                    })
                    .await;
                let _ = to_ui.send(Message::StreamDone).await;
            }

            // These three own the history and the context store, so they
            // are handled here rather than in `perform`, which holds no
            // state and cannot reach either.
            Effect::Run(Command::Clear) => {
                history.clear();
                history.push(client::ChatMessage::system(SYSTEM));
                context.clear();
                let _ = to_ui.send(Message::ContextUsed { tokens: 0 }).await;
            }
            Effect::Run(Command::Add(path)) => {
                let notice = match read_into_context(&root, &path) {
                    Ok(files) if files.is_empty() => {
                        "! nothing matched; no file was added".to_string()
                    }
                    Ok(files) => {
                        let mut tokens = 0u32;
                        let added = files.len();
                        for (name, body) in files {
                            tokens += estimate(&body);
                            // A system message: reference material, not
                            // something the user said.
                            history.push(client::ChatMessage::system(format!(
                                "File `{name}`:\n{body}"
                            )));
                            context.push((name, body));
                        }
                        format!("· added {added} file(s), ~{tokens} tokens (estimated)")
                    }
                    Err(e) => format!("! {e}"),
                };
                let _ = to_ui
                    .send(Message::ContextUsed {
                        tokens: total(&history),
                    })
                    .await;
                let _ = to_ui.send(Message::Notice(notice)).await;
            }
            Effect::Run(Command::Context) => {
                let mut lines = vec![format!(
                    "· context: ~{} tokens across {} message(s)",
                    total(&history),
                    history.len()
                )];
                if context.is_empty() {
                    lines.push("  nothing injected — /add <path> loads a file".into());
                } else {
                    for (name, body) in &context {
                        lines.push(format!("  {name:<44} ~{} tokens", estimate(body)));
                    }
                }
                let _ = to_ui.send(Message::Notice(lines.join("\n"))).await;
            }
            // A sub-agent: its own history, and only its result comes back.
            //
            // It was a stub that formatted the task into a string. The
            // point of delegating is that the sub-agent's turns do not
            // enter the main conversation — that is what keeps a long
            // side-quest from eating the window everything else shares —
            // so this builds a history of its own and returns one line.
            Effect::Run(Command::Delegate(task)) => {
                let (c, mock, ui, tok) = (
                    client.clone(),
                    client.is_none(),
                    to_ui.clone(),
                    loop_cancel.clone(),
                );
                let _ = ui
                    .send(Message::Notice(format!("· delegating: {task}")))
                    .await;
                tokio::spawn(async move {
                    // Fresh, and deliberately not a clone of the parent's:
                    // an isolated agent that inherited the conversation
                    // would be isolated in name only.
                    let sub = vec![
                        client::ChatMessage::system(
                            "You are a sub-agent. Answer the task alone, in at most three                              sentences. You cannot see the parent conversation.",
                        ),
                        client::ChatMessage::user(&task),
                    ];
                    let (tx, mut rx) = mpsc::channel(events::CHANNEL_CAPACITY);
                    match c {
                        Some(c) => {
                            tokio::spawn(async move {
                                if let Err(e) = c.stream(&sub, None, tx.clone()).await {
                                    let _ = tx.send(client::Chunk::Token(format!("! {e}"))).await;
                                    let _ = tx.send(client::Chunk::Done).await;
                                }
                            });
                        }
                        None => {
                            let t = task.clone();
                            tokio::spawn(async move { mock::stream(&t, tx).await });
                        }
                    }
                    let _ = mock;

                    let mut answer = String::new();
                    while let Some(chunk) = rx.recv().await {
                        if tok.is_cancelled() {
                            let _ = ui.send(Message::Notice("· sub-agent stopped".into())).await;
                            return;
                        }
                        match chunk {
                            client::Chunk::Token(t) => answer.push_str(&t),
                            client::Chunk::ToolCall { .. } => {}
                            client::Chunk::Done => break,
                        }
                    }
                    // One line back, not a transcript. The result is what
                    // the parent asked for; the working is the sub-agent's.
                    let summary: String = answer
                        .split_whitespace()
                        .take(40)
                        .collect::<Vec<_>>()
                        .join(" ");
                    let _ = ui
                        .send(Message::Notice(format!(
                            "· sub-agent returned ({} chars, {} kept in the main history):
  {summary}",
                            answer.len(),
                            0
                        )))
                        .await;
                });
            }
            Effect::Run(Command::Stop) => {
                cancel.cancel();
                loop_cancel.cancel();
                // Replaced, not reused: a cancelled token stays cancelled,
                // and the next loop would halt before its first cycle.
                loop_cancel = runner::CancelToken::new();
                let _ = to_ui.send(Message::Notice("· stopped".into())).await;
            }
            // Handled in the event loop, which owns the terminal. Reaching
            // here would mean it was forwarded instead of performed.
            Effect::Suspend => {}
            // A generated loop, actually run.
            //
            // It used to print a description of itself — the cap, the state
            // file, the fact that `/stop` would halt it — and execute
            // nothing. `/eng` generated a loop and the command it registered
            // was decorative, so none of the halts it advertised could be
            // observed.
            //
            // The cap, the cancellation and the no-progress halt are the
            // runner's, not this arm's: it supplies the work and reports
            // the outcome.
            Effect::Run(Command::Custom { name, args }) => {
                let paths = eng::Paths::under(&root);
                let file = paths.command_file(&name);
                let notice = match std::fs::read_to_string(&file) {
                    Err(e) => Some(format!("! {}: {e}", file.display())),
                    Ok(text) => {
                        match loader::parse_loaded(&text, &format!("{name}.md")) {
                            Err(e) => Some(format!("! {e}")),
                            Ok(loaded) => {
                                let commands = loader::verification_commands(&loaded.body);
                                if commands.is_empty() {
                                    // Not "nothing to do": a loop that cannot
                                    // check its own work would report success
                                    // for having run nothing.
                                    Some(format!(
                                        "! /{name} names no verification commands, so it cannot \
                                         verify its own work. Nothing was run."
                                    ))
                                } else {
                                    let dir = args.first().cloned().unwrap_or_else(|| ".".into());
                                    let _ = to_ui
                                        .send(Message::Notice(format!(
                                            "· /{name} over `{dir}` — {} check(s), cap {} \
                                             cycles, /stop halts it",
                                            commands.len(),
                                            eng::DEFAULT_HALT_THRESHOLD
                                        )))
                                        .await;
                                    // Spawned, never awaited. Awaiting it
                                    // here blocks this loop, so the `/stop`
                                    // meant to interrupt the cycle sits
                                    // unread in the channel until the loop
                                    // it was meant to stop has finished —
                                    // which a QA run observed as `/stop`
                                    // doing nothing and the loop running on
                                    // to the next cycle.
                                    loop_cancel.reset();
                                    tokio::spawn(run_loop(
                                        name.clone(),
                                        paths.state_file(&name),
                                        commands,
                                        root.clone(),
                                        loop_cancel.clone(),
                                        to_ui.clone(),
                                    ));
                                    None
                                }
                            }
                        }
                    }
                };
                if let Some(n) = notice {
                    let _ = to_ui.send(Message::Notice(n)).await;
                }
            }
            Effect::Run(cmd) => {
                let notice = perform(&root, &cmd, &to_ui).await;
                if !notice.is_empty() {
                    let _ = to_ui.send(Message::Notice(notice)).await;
                }
            }
        }
    }
}

/// The tools this interface offers the model.
///
/// **It offered none.** The stream was opened with `None`, so no definitions
/// ever reached the endpoint, so no model could ever answer with a
/// `tool_calls` array — which made the `Chunk::ToolCall` arm that forwards
/// one to the interface unreachable in production. The tool pane and the
/// graph could only ever have been filled by the mock, which synthesises a
/// call it was never offered.
///
/// The names match `tui::widgets::graph::KNOWN_TOOLS`, so a call that comes
/// back lands on a node the graph already draws rather than appending an
/// unknown one.
fn tools() -> Vec<client::Tool> {
    vec![
        client::Tool::new(
            "read_file",
            "Read a file from the repository and return its contents.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "path relative to the repository root" }
                },
                "required": ["path"]
            }),
        ),
        client::Tool::new(
            "run_tests",
            "Run the project's test suite, optionally filtered to matching test names.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "filter": { "type": "string", "description": "substring of the test names to run" }
                }
            }),
        ),
        client::Tool::new(
            "edit",
            "Replace an exact string in a file. `old` must occur exactly once.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old": { "type": "string" },
                    "new": { "type": "string" }
                },
                "required": ["path", "old", "new"]
            }),
        ),
        client::Tool::new(
            "blast_radius",
            "Which first-party call sites reach a symbol, with its coverage.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "dotted symbol name" }
                },
                "required": ["symbol"]
            }),
        ),
    ]
}

/// Four bytes to a token.
///
/// Crude, and labelled an estimate everywhere it is shown: the real number
/// depends on a tokeniser this crate does not carry, and a precise-looking
/// wrong figure is worse than a rough honest one.
fn estimate(text: &str) -> u32 {
    u32::try_from(text.len() / 4).unwrap_or(u32::MAX)
}

/// What the whole history costs, by that estimate.
fn total(history: &[client::ChatMessage]) -> u32 {
    history.iter().map(|m| estimate(&m.content)).sum()
}

/// Carries out one command. Returns what to show the user.
async fn perform(root: &Path, cmd: &Command, to_ui: &mpsc::Sender<Message>) -> String {
    match cmd {
        // Add, Context and Clear are handled in `run_effects`, which owns
        // the history and the context store. Reaching them here would mean
        // two places could answer the same command differently.
        Command::Add(_) | Command::Context | Command::Clear => String::new(),
        Command::Compact => "· compacted".into(),
        Command::Git(r) => match git_snapshot(root, r) {
            Ok(text) => format!("· git {}:\n{text}", git_label(r)),
            Err(e) => format!("! {e}"),
        },
        Command::Steer(d) => format!("· steering: {d}"),
        // Handled in `run_effects`: it must never be queued behind the
        // loop it is meant to interrupt.
        Command::Stop => String::new(),
        Command::Approve => "· approved".into(),
        Command::Agent(name) => format!("· agent: {name}"),
        // Runs in `run_effects`, which can spawn and be cancelled.
        Command::Delegate(_) => String::new(),
        Command::Eng(name) => match eng::generate(root, name) {
            Ok((spec, written)) => {
                let mut out = spec.summary();
                out.push(String::new());
                for p in written {
                    out.push(format!("  wrote {}", p.display()));
                }
                out.join("\n")
            }
            Err(e) => format!("! could not write the loop: {e}"),
        },
        Command::Reload => {
            let (loaded, errors) = loader::load_dir(&eng::Paths::under(root).commands_dir);
            let _ = to_ui
                .send(Message::CommandsLoaded {
                    specs: loaded.iter().map(|l| l.spec.clone()).collect(),
                    errors,
                })
                .await;
            String::new()
        }
        // Runs in `run_effects`, which can spawn and be cancelled.
        Command::Custom { .. } => String::new(),
    }
}

fn git_label(r: &GitRef) -> &'static str {
    match r {
        GitRef::Branch => "branch",
        GitRef::Diff => "diff",
        GitRef::Status => "status",
    }
}

fn git_snapshot(root: &Path, r: &GitRef) -> Result<String, String> {
    let args: &[&str] = match r {
        GitRef::Branch => &["rev-parse", "--abbrev-ref", "HEAD"],
        GitRef::Diff => &["diff", "--stat"],
        GitRef::Status => &["status", "--porcelain"],
    };
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(if text.is_empty() {
        "(nothing)".into()
    } else {
        text
    })
}

/// Reads a file, a directory, or a simple `*.ext` glob, returning contents.
///
/// **Contents, not a byte count.** It used to return `(files, bytes)` and
/// `/add` reported "added 1 file(s), ~7276 tokens" having read the file's
/// *metadata* and thrown the file away: the context figure did not move,
/// the gauge stayed at 0%, and the model never saw a line of it. The
/// command counted the cost of work it did not do.
///
/// Binary files are skipped rather than injected. A GGUF read as UTF-8 is
/// megabytes of replacement characters, and it would be counted as context
/// and sent.
fn read_into_context(root: &Path, path: &Path) -> Result<Vec<(String, String)>, String> {
    /// Refuses anything that is not text, and anything absurdly large.
    ///
    /// The cap is per file and generous: a 2 MB source file is already far
    /// past any local model's window, so refusing it is kinder than
    /// silently filling the context with it.
    const MAX_BYTES: u64 = 2 * 1024 * 1024;

    fn read_one(p: &Path, label: &str) -> Option<(String, String)> {
        let len = std::fs::metadata(p).ok()?.len();
        if len > MAX_BYTES {
            return None;
        }
        let body = std::fs::read_to_string(p).ok()?;
        // `read_to_string` already rejects invalid UTF-8; a NUL is the
        // remaining giveaway for a binary that happens to decode.
        if body.contains('\0') {
            return None;
        }
        Some((label.to_string(), body))
    }

    let full = root.join(path);

    if full.is_file() {
        return read_one(&full, &path.display().to_string())
            .map(|f| vec![f])
            .ok_or_else(|| {
                format!(
                    "{} is not readable text under {} MiB",
                    path.display(),
                    MAX_BYTES / 1024 / 1024
                )
            });
    }

    // A directory, or a `*.ext` glob in one. Both walk one level: recursing
    // into a repository from `/add .` would load the whole tree, which is
    // never what the user meant and is expensive to discover.
    let (dir, ext) = if full.is_dir() {
        (full.clone(), None)
    } else {
        match (full.parent(), full.file_name().and_then(|n| n.to_str())) {
            (Some(parent), Some(pattern)) if pattern.starts_with("*.") => (
                parent.to_path_buf(),
                pattern.strip_prefix("*.").map(str::to_string),
            ),
            _ => return Err(format!("{} is not a file, folder or glob", path.display())),
        }
    };

    let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut out: Vec<(String, String)> = Vec::new();
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| match &ext {
            Some(want) => p.extension().is_some_and(|x| x == want.as_str()),
            None => true,
        })
        .collect();
    // Sorted, so `/add src/` loads the same thing in the same order twice.
    paths.sort();

    for p in paths {
        let label = p
            .strip_prefix(root)
            .unwrap_or(&p)
            .display()
            .to_string()
            .replace('\\', "/");
        if let Some(f) = read_one(&p, &label) {
            out.push(f);
        }
    }
    Ok(out)
}

/// Drives one generated loop, reporting each cycle as it lands.
///
/// The halts belong to `LoopRunner` — the cap, the cancellation and the
/// no-progress rule are enforced there rather than by anything here, which
/// is the whole reason that type exists. This supplies the work and turns
/// the outcome into lines.
///
/// `spawn_blocking`, because a cycle shells out and a check can take
/// minutes. Running it on the runtime's thread would stall every other
/// effect, including the `/stop` that is supposed to interrupt it.
async fn run_loop(
    name: String,
    state_file: PathBuf,
    commands: Vec<String>,
    root: PathBuf,
    cancel: runner::CancelToken,
    to_ui: mpsc::Sender<Message>,
) {
    // A fresh token for this loop, linked to the shared one. `/stop` sets
    // the shared token; this one lets the loop finish without leaving the
    // session permanently cancelled for the next command.
    let ui = to_ui.clone();
    let handle = tokio::task::spawn_blocking(move || {
        // The runner owns one clone for its between-step checks; the
        // cycle body needs its own so it can kill a child mid-command.
        let cycle_cancel = cancel.clone();
        let runner = runner::LoopRunner::new(state_file, eng::DEFAULT_HALT_THRESHOLD, cancel);
        runner.run(&mut |cycle: u32| {
            let mut failures = Vec::new();
            for c in &commands {
                let out = shell(c, &root, &cycle_cancel)?;
                if !out.0 {
                    // The first line of the checker's own output, so the
                    // error set is about what failed rather than that
                    // something did. The no-progress halt compares these,
                    // so a stable message is what makes it fire.
                    let first = out
                        .1
                        .lines()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("(no output)")
                        .chars()
                        .take(120)
                        .collect::<String>();
                    failures.push(format!("{c}: {first}"));
                }
            }
            let _ = ui.blocking_send(Message::Notice(format!(
                "  cycle {} — {} failing",
                cycle + 1,
                failures.len()
            )));
            Ok(failures)
        })
    });

    match handle.await {
        Ok(halt) => {
            let mut lines = vec![format!("· /{name}: {}", halt.summary())];
            if let runner::Halt::NoProgress { repeated, .. } = &halt {
                for r in repeated.iter().take(3) {
                    lines.push(format!("    {r}"));
                }
            }
            let _ = to_ui.send(Message::Notice(lines.join("\n"))).await;
        }
        Err(e) => {
            let _ = to_ui
                .send(Message::Notice(format!("! /{name} did not finish: {e}")))
                .await;
        }
    }
}

/// Runs one shell command in `root`, killing it if `cancel` fires.
///
/// Polled rather than simply waited on. `/stop` describes itself as "kill
/// running child processes and halt the loop", and it did neither to a
/// child: the runner checks cancellation between steps, so a `/stop` during
/// a four-second check was noticed only after that check had finished and
/// the process it spawned had run to completion. A QA run saw `· stopped`
/// land, cycle 2 finish anyway, and the halt report arrive at cycle 3.
///
/// `Err` only when the shell could not start — which the runner turns into
/// `Halt::Failed`, never into zero errors. A checker that failed to launch
/// must not read as a passing check.
fn shell(
    command: &str,
    root: &Path,
    cancel: &runner::CancelToken,
) -> Result<(bool, String), String> {
    use std::io::Read;

    let (program, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    let mut child = std::process::Command::new(program)
        .arg(flag)
        .arg(command)
        .current_dir(root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run `{command}`: {e}"))?;

    // 50ms is short enough that a stop feels immediate and long enough that
    // the poll costs nothing against a check measured in seconds.
    const POLL: std::time::Duration = std::time::Duration::from_millis(50);
    loop {
        match child.try_wait() {
            Err(e) => return Err(format!("`{command}`: {e}")),
            Ok(Some(status)) => {
                let mut text = String::new();
                if let Some(mut out) = child.stdout.take() {
                    let _ = out.read_to_string(&mut text);
                }
                if let Some(mut err) = child.stderr.take() {
                    let _ = err.read_to_string(&mut text);
                }
                return Ok((status.success(), text));
            }
            Ok(None) => {
                if cancel.is_cancelled() {
                    // Best effort: the child may have exited between the
                    // check and here, which is not an error.
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok((false, format!("`{command}` killed by /stop")));
                }
                std::thread::sleep(POLL);
            }
        }
    }
}

/// `tracing` to a file and to the in-app pane. **Never to stdout** — stdout
/// belongs to the display, and a stray line corrupts it in a way that reads
/// as a rendering bug.
fn install_logging() {
    use tracing_subscriber::prelude::*;
    let _ = tui_logger::init_logger(tui_logger::LevelFilter::Debug);
    tui_logger::set_default_level(tui_logger::LevelFilter::Debug);

    // Into the user's state directory, never the working directory.
    //
    // It was `chat.log` relative to cwd, so running the interface inside a
    // repository left a modified file in it — `/git status` during a QA run
    // showed `M chat.log`. For a harness whose runs refuse to start on a
    // dirty worktree, a log file that dirties it is the tool blocking
    // itself. `CHAT_LOG` overrides for anyone who wants it somewhere else.
    let log_path = std::env::var_os("CHAT_LOG")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let base = std::env::var_os("XDG_STATE_HOME")
                .or_else(|| std::env::var_os("LOCALAPPDATA"))
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
                .unwrap_or_else(std::env::temp_dir);
            base.join("trellis-chat")
        })
        .join("chat.log");
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();

    let registry = tracing_subscriber::registry().with(tui_logger::TuiTracingSubscriberLayer);
    match file {
        Some(f) => {
            let _ = registry
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(f)
                        .with_ansi(false),
                )
                .try_init();
        }
        None => {
            let _ = registry.try_init();
        }
    }
}
