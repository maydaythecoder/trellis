//! The TRELLIS desktop interface (ADR 0048), before its window: start one
//! `trellis rpc` child, ask for its state, and print each `view` event the
//! child's lines produce and then the reply; shut the child down.
//!
//!     trellis-desktop --root <repo> [--trellis <path>]

mod child;
mod frames;
mod state;

use std::path::PathBuf;

fn main() {
    let mut root: Option<PathBuf> = None;
    let mut trellis = PathBuf::from("trellis");
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--root" => root = args.next().map(PathBuf::from),
            "--trellis" => {
                if let Some(t) = args.next() {
                    trellis = PathBuf::from(t);
                }
            }
            other => {
                eprintln!(
                    "unknown argument: {other}\nusage: trellis-desktop --root <repo> [--trellis <path>]"
                );
                std::process::exit(2);
            }
        }
    }
    let Some(root) = root else {
        eprintln!("usage: trellis-desktop --root <repo> [--trellis <path>]");
        std::process::exit(2);
    };
    if let Err(e) = run(&trellis, &root) {
        eprintln!("trellis-desktop: {e}");
        std::process::exit(1);
    }
}

/// One `view` event: the panes `changed` names, as the page receives them.
fn emit_view(s: &state::DesktopState, changed: state::Changed) {
    println!(
        "{}",
        serde_json::json!({"type": "view", "panes": s.view(changed)})
    );
}

fn run(trellis: &std::path::Path, root: &std::path::Path) -> std::io::Result<()> {
    // The view starts clear, and the first event carries every pane, so the
    // page has something to draw for each before any frame arrives.
    let mut view = state::DesktopState::default();
    let all = view.clear();
    emit_view(&view, all);
    let mut c = child::Child::spawn(trellis, root)?;
    c.send(&serde_json::json!({"id": "desktop-1", "type": "get_state"}))?;
    // Every stdout line goes through the reducer, whatever it is.
    let reply = c.response_to("desktop-1", |line| {
        let changed = view.update(frames::parse_stdout(line));
        if !changed.is_empty() {
            emit_view(&view, changed);
        }
    })?;
    println!("{reply}");
    c.shutdown()?;
    Ok(())
}
