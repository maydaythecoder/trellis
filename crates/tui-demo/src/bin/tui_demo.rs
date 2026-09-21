//! `cargo run --bin tui_demo`

use tui_demo::tui;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let mut terminal = tui::terminal::init();
    // Bounded: an unbounded channel lets the producer outrun the draw loop
    // and grow without limit.
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    tui::mock::spawn_mock(tx);
    let mut state = tui::app::AppState::default();
    let result = tui::events::run(&mut terminal, &mut state, rx).await;
    tui::terminal::restore();
    result
}
