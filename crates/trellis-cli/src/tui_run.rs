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

pub(crate) fn run(root: &Path, task: &str, model_spec: Option<String>) -> Result<()> {
    // Refused rather than attempted when there is no terminal to drive.
    // `EventStream` waits on console input, which never arrives from a pipe,
    // so the interface would paint one frame and hang — and a hang that has
    // to be killed skips every destructor, which is the one way to leave a
    // terminal broken that no guard can cover.
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!(concat!(
            "`trellis tui` needs a terminal, and this is a pipe or a redirect.\n",
            "Run it directly, or use `trellis run` which works fine without one."
        ));
    }

    let Some(spec) = model_spec else {
        anyhow::bail!(concat!(
            "no model configured, and the interface would have nothing to show.\n",
            "Set one with `trellis model`, or in .trellis/config.toml, or pass\n",
            "`--model <spec>` to `trellis run`."
        ));
    };

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        // Bounded. The loop can outrun a redraw during a turn burst, and an
        // unbounded channel turns that into memory growth.
        let (tx, rx) = tokio::sync::mpsc::channel(256);

        // The subscriber is installed for the duration of the run and
        // nothing else: `cmd_run` writes to stdout, which now belongs to the
        // display, so the run happens on a worker thread with its output
        // captured by the layer rather than printed.
        let layer = LoopLayer::new(tx.clone());
        let root = root.to_path_buf();
        let task_owned = task.to_string();

        let worker = std::thread::Builder::new()
            .name("trellis-tui-run".into())
            .spawn(move || {
                use tracing_subscriber::layer::SubscriberExt;
                let subscriber = tracing_subscriber::registry().with(layer);
                tracing::subscriber::with_default(subscriber, || {
                    let _ = crate::cmd_run(&root, &task_owned, &spec, false, "1", false);
                });
                // The interface stays up after the run ends so the result can
                // be read; `q` closes it.
                let _ = tx.blocking_send(tui::message::Message::RunFinished);
            })?;

        let mut terminal = tui::terminal::init();
        let mut state = tui::app::AppState::default();
        let result = tui::events::run(&mut terminal, &mut state, rx).await;
        tui::terminal::restore();

        // Joined after the terminal is handed back, so a panic in the run
        // surfaces onto a readable screen.
        let _ = worker.join();
        result.map_err(|e| anyhow::anyhow!("{e}"))
    })
}
