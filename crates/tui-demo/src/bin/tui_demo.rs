//! `cargo run --bin tui_demo`

use tui_demo::tui;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    // `install` returns an eyre report, which is not a std error, so it is
    // bridged rather than propagated with `?`. The library returns
    // `anyhow::Result` so that it does not drag color-eyre into every
    // consumer - `trellis-cli` was inheriting 37 crates for one return type.
    color_eyre::install().map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut terminal = tui::terminal::init();
    // Bounded: an unbounded channel lets the producer outrun the draw loop
    // and grow without limit.
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    let (task_tx, mut task_rx) = tokio::sync::mpsc::channel::<String>(8);

    // In the demo, a submitted task replays the mock script. In `trellis
    // tui` the same channel starts a real run — the interface cannot tell
    // the difference, which is the point of it carrying a String out rather
    // than knowing how to run anything.
    let mock_tx = tx.clone();
    tokio::spawn(async move {
        while let Some(task) = task_rx.recv().await {
            let _ = mock_tx
                .send(tui::message::Message::AgentToken(format!(
                    "running `{task}` against the mock
"
                )))
                .await;
            tui::mock::spawn_mock(mock_tx.clone());
        }
    });

    tui::mock::spawn_mock(tx);
    let mut state = tui::app::AppState::default();
    let result = tui::events::run(&mut terminal, &mut state, rx, task_tx).await;
    tui::terminal::restore();
    result
}
