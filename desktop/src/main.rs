//! The TRELLIS desktop interface (ADR 0048): one window over one
//! `trellis rpc` child per repository.
//!
//!     trellis-desktop [--root <repo>] [--trellis <path>] [--web]
//!
//! With `--web` there is no window: the same page and the same four
//! commands are served over HTTP on 127.0.0.1 behind a per-launch token
//! (`web.rs`, ADR 0050 section 5) for `trellis --web`, which reads the URL
//! from stdout and stops the server by closing its stdin.
//!
//! Once the page has loaded, the core (`commands.rs`) opens the startup
//! repository: it starts the child (`child.rs`, the one file that spawns)
//! and turns what the child prints into events, the run panes among them
//! (`frames.rs`, `state.rs`) and the action graph's layout with them
//! (`graph.rs`, the `view` event's `graph` key). A `get_codebase` answer
//! becomes the codebase view's layout (`codebase.rs`, the `codebase`
//! event), and a `get_radius` answer its reach and doubt (`radius.rs`, the
//! `radius` event). `vault_search` and `vault_graph` answers become the
//! vault notes view (`state/vault.rs`, the `view` event's `vault` key), and
//! `get_queue` answers the board's queue view (`queue.rs`, its `queue`
//! key). The page has four commands: `rpc_send`, `get_trails`
//! (the trails view's rows, `trails.rs`), `open_repository` and
//! `get_overview`.
//!
//! With no `--root`, the repository is the current directory or the nearest
//! parent holding a `.git` entry, as trellis itself finds one. Outside any
//! repository no child is started, and the core emits the overview instead.
//!
//! A close request on the window is held while a child runs: the core sends
//! it `quit`, and the window closes once it has exited. Nothing kills it.

// A windowed program: without this Windows opens a console beside the
// window. Tests keep the console so their output is seen.
#![cfg_attr(not(test), windows_subsystem = "windows")]

mod child;
mod codebase;
mod commands;
mod frames;
mod graph;
mod lockdown;
mod queue;
mod radius;
mod state;
mod trails;
mod web;

use commands::Core;
use std::path::PathBuf;
use std::sync::{Arc, Once};
use tauri::webview::{NewWindowResponse, PageLoadEvent};
use tauri::{Manager, WebviewWindowBuilder};

const USAGE: &str = "usage: trellis-desktop [--root <repo>] [--trellis <path>] [--web]";

fn main() {
    let mut root: Option<PathBuf> = None;
    let mut trellis = PathBuf::from("trellis");
    let mut web = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--root" => root = args.next().map(PathBuf::from),
            "--trellis" => {
                if let Some(t) = args.next() {
                    trellis = PathBuf::from(t);
                }
            }
            "--web" => web = true,
            other => {
                eprintln!("unknown argument: {other}\n{USAGE}");
                std::process::exit(2);
            }
        }
    }
    if web {
        // No window and no Tauri runtime: the core's events go to the
        // pages' event streams, and the page's commands come in over http.
        let sink = Arc::new(web::WebSink::default());
        let core = Core::new(child::Trellis::new(trellis), sink.clone());
        let e = web::serve(core, sink, root);
        eprintln!("trellis-desktop: web: {e}");
        std::process::exit(1);
    }
    let started = Once::new();
    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::rpc_send,
            commands::get_trails,
            commands::open_repository,
            commands::get_overview
        ])
        .setup(move |app| {
            let core = Core::new(child::Trellis::new(trellis), Arc::new(app.handle().clone()));
            app.manage(core);
            // The main window is built here rather than by Tauri from the
            // config (`"create": false`), because only a builder takes the
            // navigation and new-window handlers (`lockdown.rs`).
            let config = app
                .config()
                .app
                .windows
                .iter()
                .find(|w| w.label == "main")
                .ok_or("tauri.conf.json has no window labelled main")?
                .clone();
            WebviewWindowBuilder::from_config(app, &config)?
                .on_navigation(lockdown::navigation_allowed)
                .on_new_window(|url, _features| {
                    if lockdown::new_window_allowed(&url) {
                        NewWindowResponse::Allow
                    } else {
                        NewWindowResponse::Deny
                    }
                })
                .build()?;
            Ok(())
        })
        // The startup work waits for the page, so the events it emits find
        // the page's listeners; once, so a reload starts nothing twice. A
        // reloaded page gets the run back by forwarding a `get_state`.
        .on_page_load(move |webview, payload| {
            if payload.event() != PageLoadEvent::Finished {
                return;
            }
            let core = webview.state::<Core>().inner().clone();
            let root = root.clone();
            started.call_once(move || {
                std::thread::spawn(move || match std::env::current_dir() {
                    Ok(cwd) => core.startup(root, &cwd),
                    Err(e) => eprintln!("trellis-desktop: cannot read the current directory: {e}"),
                });
            });
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let core = window.state::<Core>().inner().clone();
                // Only while a child runs: once it has gone, the close the
                // thread below asks for goes through.
                if let Some(taken) = core.take_child("the desktop is closing") {
                    api.prevent_close();
                    let window = window.clone();
                    std::thread::spawn(move || {
                        if let Err(e) = core.shut_down(taken) {
                            eprintln!("trellis-desktop: shutting down trellis rpc: {e}");
                        }
                        if let Err(e) = window.close() {
                            eprintln!("trellis-desktop: cannot close the window: {e}");
                        }
                    });
                }
            }
        })
        .build(tauri::generate_context!());
    match app {
        Ok(app) => app.run(|app, event| {
            // Any other way out (the process told to exit): still quit and
            // wait, never kill.
            if let tauri::RunEvent::Exit = event
                && let Some(core) = app.try_state::<Core>()
                && let Some(taken) = core.take_child("the desktop is closing")
                && let Err(e) = core.shut_down(taken)
            {
                eprintln!("trellis-desktop: shutting down trellis rpc: {e}");
            }
        }),
        Err(e) => {
            eprintln!("trellis-desktop: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use std::path::Path;

    fn manifest_file(name: &str) -> String {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(name);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }

    #[test]
    fn tauri_conf_has_one_window_no_dev_hooks_and_no_bundle() {
        // The risk: a dev server, a build hook or a bundle the ADR keeps
        // out; a second window; or a command without its permission. The
        // CSP and the capability's permissions are held in `lockdown.rs`.
        let conf: Value = serde_json::from_str(&manifest_file("tauri.conf.json")).unwrap();
        assert_eq!(conf["identifier"], "dev.trellis.desktop");
        assert_eq!(
            conf["build"],
            json!({"frontendDist": "ui"}),
            "no devUrl, no hook"
        );
        assert_eq!(conf["app"]["withGlobalTauri"], true);
        assert_eq!(conf["bundle"]["active"], false);
        let windows = conf["app"]["windows"].as_array().unwrap();
        assert_eq!(windows.len(), 1, "one window");
        let w = &windows[0];
        assert_eq!(w["label"], "main");
        assert_eq!(w["title"], "TRELLIS");
        assert_eq!(
            (w["width"].as_u64(), w["height"].as_u64()),
            (Some(1280), Some(820))
        );
        assert_eq!(
            (w["minWidth"].as_u64(), w["minHeight"].as_u64()),
            (Some(860), Some(600))
        );
        // Each command a permission, and only the commands listed here.
        assert!(
            manifest_file("build.rs").contains(
                r#"const COMMANDS: [&str; 4] = ["rpc_send", "get_trails", "open_repository", "get_overview"];"#
            ),
            "build.rs lists the four commands"
        );
    }

    #[test]
    fn page_loads_only_module_scripts_and_nothing_inline() {
        // The risk: an inline script, style or handler, which the CSP
        // refuses (so the page breaks) or which a later CSP edit would let
        // run; or a classic script where the page expects a module.
        let html = manifest_file("ui/index.html").to_lowercase();
        // Comments are not markup; the file's own comment names what it
        // leaves out.
        let mut page = String::new();
        let mut rest = html.as_str();
        while let Some(i) = rest.find("<!--") {
            page.push_str(&rest[..i]);
            let end = rest[i..].find("-->").expect("an unclosed comment");
            rest = &rest[i + end + 3..];
        }
        page.push_str(rest);

        let scripts: Vec<&str> = page.split("<script").skip(1).collect();
        assert!(!scripts.is_empty(), "the page loads main.js");
        for s in &scripts {
            let tag = &s[..s.find('>').unwrap()];
            assert!(
                tag.contains(r#"type="module""#),
                "not a module: <script{tag}>"
            );
            assert!(tag.contains("src="), "inline script: <script{tag}>");
            assert!(
                s[tag.len() + 1..].starts_with("</script>"),
                "script body: <script{s}"
            );
        }
        assert!(!page.contains("<style"), "inline <style>");
        assert!(!page.contains(" style="), "style attribute");
        let handler = page
            .split(|c: char| c.is_whitespace())
            .find(|w| w.starts_with("on") && w.contains('='));
        assert_eq!(handler, None, "inline event handler");
    }
}
