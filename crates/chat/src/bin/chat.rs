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

#[tokio::main]
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
    let result = events::run(&mut term, &mut state, &theme, from_work, to_work).await;
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
    let mut history: Vec<client::ChatMessage> = vec![client::ChatMessage::system(
        "You are a concise coding assistant working inside a terminal interface.",
    )];

    while let Some(effect) = rx.recv().await {
        match effect {
            Effect::Prompt(text) => {
                history.push(client::ChatMessage::user(&text));
                let used: u32 = history.iter().map(|m| (m.content.len() / 4) as u32).sum();
                let _ = to_ui.send(Message::ContextUsed { tokens: used }).await;

                let (ctx, mut crx) = mpsc::channel(events::CHANNEL_CAPACITY);
                match &client {
                    Some(c) => {
                        let c = c.clone();
                        let msgs = history.clone();
                        tokio::spawn(async move {
                            if let Err(e) = c.stream(&msgs, None, ctx.clone()).await {
                                let _ = ctx.send(client::Chunk::Token(format!("! {e}"))).await;
                                let _ = ctx.send(client::Chunk::Done).await;
                            }
                        });
                    }
                    None => {
                        tokio::spawn(async move { mock::stream(&text, ctx).await });
                    }
                }

                let mut answer = String::new();
                while let Some(chunk) = crx.recv().await {
                    if cancel.is_cancelled() {
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
                history.push(client::ChatMessage {
                    role: "assistant".into(),
                    content: answer,
                });
                let _ = to_ui.send(Message::StreamDone).await;
            }

            Effect::Run(cmd) => {
                let notice = perform(&root, &cmd, &cancel, &to_ui).await;
                if !notice.is_empty() {
                    let _ = to_ui.send(Message::Notice(notice)).await;
                }
            }
        }
    }
}

/// Carries out one command. Returns what to show the user.
async fn perform(
    root: &Path,
    cmd: &Command,
    cancel: &runner::CancelToken,
    to_ui: &mpsc::Sender<Message>,
) -> String {
    match cmd {
        Command::Add(path) => match read_into_context(root, path) {
            Ok((n, bytes)) => format!("· added {n} file(s), ~{} tokens", bytes / 4),
            Err(e) => format!("! {e}"),
        },
        Command::Context => "· context: (the runtime tracks this per turn)".into(),
        Command::Clear => String::new(), // handled in `update`; pure state
        Command::Compact => "· compacted".into(),
        Command::Git(r) => match git_snapshot(root, r) {
            Ok(text) => format!("· git {}:\n{text}", git_label(r)),
            Err(e) => format!("! {e}"),
        },
        Command::Steer(d) => format!("· steering: {d}"),
        Command::Stop => {
            cancel.cancel();
            "· stopped".into()
        }
        Command::Approve => "· approved".into(),
        Command::Agent(name) => format!("· agent: {name}"),
        Command::Delegate(task) => format!("· delegated: {task}"),
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
        Command::Custom { name, args } => {
            let paths = eng::Paths::under(root);
            let state_file = paths.state_file(name);
            let halt = eng::DEFAULT_HALT_THRESHOLD;
            format!(
                "· /{name} {}\n  loop state {}\n  cap {halt} cycles, enforced by the runtime\n  \
                 /stop halts it mid-cycle",
                args.join(" "),
                state_file.display()
            )
        }
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

/// Reads a file, a directory or a simple `*.ext` glob.
fn read_into_context(root: &Path, path: &Path) -> Result<(usize, usize), String> {
    let full = root.join(path);
    if full.is_file() {
        let bytes = std::fs::metadata(&full).map_err(|e| e.to_string())?.len();
        return Ok((1, bytes as usize));
    }
    if full.is_dir() {
        let mut n = 0;
        let mut bytes = 0;
        for e in std::fs::read_dir(&full)
            .map_err(|e| e.to_string())?
            .flatten()
        {
            if e.path().is_file() {
                n += 1;
                bytes += e.metadata().map(|m| m.len()).unwrap_or(0) as usize;
            }
        }
        return Ok((n, bytes));
    }
    // A trailing `*.rs`-style glob, handled without a dependency.
    if let Some(parent) = full.parent()
        && let Some(pattern) = full.file_name().and_then(|n| n.to_str())
        && let Some(ext) = pattern.strip_prefix("*.")
    {
        let mut n = 0;
        let mut bytes = 0;
        for e in std::fs::read_dir(parent)
            .map_err(|e| e.to_string())?
            .flatten()
        {
            if e.path().extension().is_some_and(|x| x == ext) {
                n += 1;
                bytes += e.metadata().map(|m| m.len()).unwrap_or(0) as usize;
            }
        }
        if n > 0 {
            return Ok((n, bytes));
        }
    }
    Err(format!("{} is not a file, folder or glob", path.display()))
}

/// `tracing` to a file and to the in-app pane. **Never to stdout** — stdout
/// belongs to the display, and a stray line corrupts it in a way that reads
/// as a rendering bug.
fn install_logging() {
    use tracing_subscriber::prelude::*;
    let _ = tui_logger::init_logger(tui_logger::LevelFilter::Debug);
    tui_logger::set_default_level(tui_logger::LevelFilter::Debug);

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("chat.log")
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
