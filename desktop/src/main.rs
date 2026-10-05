//! The TRELLIS desktop interface (ADR 0048), before its window: start one
//! `trellis rpc` child, ask for its state, print the reply, shut it down.
//!
//!     trellis-desktop --root <repo> [--trellis <path>]

mod child;

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
                eprintln!("unknown argument: {other}\nusage: trellis-desktop --root <repo> [--trellis <path>]");
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

fn run(trellis: &std::path::Path, root: &std::path::Path) -> std::io::Result<()> {
    let mut c = child::Child::spawn(trellis, root)?;
    c.send(&serde_json::json!({"id": "desktop-1", "type": "get_state"}))?;
    let reply = c.response_to("desktop-1")?;
    println!("{reply}");
    c.shutdown()?;
    Ok(())
}
