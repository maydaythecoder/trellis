//! Tauri's build step: it reads `tauri.conf.json`, embeds the Windows icon
//! and generates the app's permissions.
//!
//! The command list matters. Without it Tauri lets every window call every
//! registered command, and the capability in `tauri.conf.json` would limit
//! nothing; with it, each command gets an `allow-<command>` permission (for
//! `rpc_send`, `allow-rpc-send`) and a window may call it only if its
//! capability grants that (ADR 0048 section 5).

/// The page's four commands (`src/commands.rs`); a test in `src/main.rs`
/// reads this line.
const COMMANDS: [&str; 4] = ["rpc_send", "get_trails", "open_repository", "get_overview"];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(&COMMANDS)),
    )
    .expect("tauri-build failed");
}
