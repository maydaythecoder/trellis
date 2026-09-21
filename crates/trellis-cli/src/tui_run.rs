//! `trellis tui <task>` — the run, watched while it happens.
//!
//! The interface itself lives in the `tui-demo` crate, which knows nothing
//! about this one: `AppState`, `render` and the event loop are the same code
//! the standalone demo runs, so the thing being wired here is the *source*
//! of events and nothing else.
//!
//! The run goes through `cmd_run`, not around it. That path owns the
//! snapshot, the reachability gate, the governance rules, the rollback and
//! the trail, and a second copy of that sequence written for a UI would be a
//! second set of rules about when a change is accepted.

use crate::tui_bridge::LoopLayer;
use anyhow::Result;
use std::path::Path;
use tui_demo::tui;

pub(crate) fn run(root: &Path, task: Option<String>, model_spec: Option<String>) -> Result<()> {
    // Refused rather than attempted when there is no terminal to drive.
    // `EventStream` waits on console input, which never arrives from a pipe,
    // so the interface would paint one frame and hang — and a hang that has
    // to be killed skips every destructor, which is the one way to leave a
    // terminal broken that no guard can cover.
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!(concat!(
            "`trellis tui` needs a terminal, and this is a pipe or a redirect.
",
            "Run it directly, or use `trellis run` which works fine without one."
        ));
    }

    let Some(spec) = model_spec else {
        anyhow::bail!(concat!(
            "no model configured, and a run would have nothing to drive it.
",
            "Set one with `trellis model`, or in .trellis/config.toml, or pass
",
            "`--model <spec>` to `trellis run`."
        ));
    };

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        // Bounded. The loop can outrun a redraw during a turn burst, and an
        // unbounded channel turns that into memory growth.
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        let (task_tx, mut task_rx) = tokio::sync::mpsc::channel::<String>(8);

        // One driver, serving every task the user types. It owns the
        // subscriber install and the worker thread so the interface does
        // not have to know a run is a thread at all.
        let root = root.to_path_buf();
        let driver = tokio::spawn({
            let tx = tx.clone();
            async move {
                while let Some(task) = task_rx.recv().await {
                    let (root, spec, tx) = (root.clone(), spec.clone(), tx.clone());
                    // `spawn_blocking`, because `cmd_run` blocks on its own
                    // runtime and must not sit on a worker of this one.
                    let _ = tokio::task::spawn_blocking(move || {
                        use tracing_subscriber::layer::SubscriberExt;
                        let layer = LoopLayer::new(tx.clone());
                        let subscriber = tracing_subscriber::registry().with(layer);
                        tracing::subscriber::with_default(subscriber, || {
                            let _ = crate::cmd_run(&root, &task, &spec, false, "1", false);
                        });
                        // Whatever happened, the interface takes input again.
                        let _ = tx.blocking_send(tui::message::Message::RunFinished);
                    })
                    .await;
                }
            }
        });

        let mut state = tui::app::AppState::default();
        // A task on the command line starts immediately; without one the
        // interface opens idle and waits for the user to type.
        if let Some(t) = task {
            state.transcript.push(format!("> {t}"));
            state.running = true;
            let _ = task_tx.send(t).await;
        } else {
            state
                .transcript
                .push("type a task and press enter · esc to quit".into());
        }

        let mut terminal = tui::terminal::init();
        let result = tui::events::run(&mut terminal, &mut state, rx, task_tx).await;
        tui::terminal::restore();

        // Aborted after the terminal is handed back, so a panic inside a run
        // surfaces onto a readable screen.
        driver.abort();
        result.map_err(|e| anyhow::anyhow!("{e}"))
    })
}
