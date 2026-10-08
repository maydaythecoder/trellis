//! Tauri's build step: it reads `tauri.conf.json`, embeds the Windows icon
//! and generates the app's permissions.
//!
//! The command list matters. Without it Tauri lets every window call every
//! registered command, and the capability in `tauri.conf.json` would limit
//! nothing; with it, `rpc_send` gets an `allow-rpc-send` permission and a
//! window may call it only if its capability grants that (ADR 0048 section 5).

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(&["rpc_send"])),
    )
    .expect("tauri-build failed");
}
