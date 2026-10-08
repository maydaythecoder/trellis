//! The TRELLIS desktop interface (ADR 0048): one window whose page shows
//! what one `trellis rpc` child answers.
//!
//!     trellis-desktop [--root <repo>] [--trellis <path>]
//!
//! At startup the core starts the child (`child.rs`, the one file that
//! spawns), and emits each stdout line that parses as JSON to the page as an
//! `rpc-frame` event carrying the parsed value. The page's one command,
//! `rpc_send`, forwards a `get_state` and refuses anything else. When the
//! app exits, the child is sent `quit` and waited for, never killed.
//!
//! With no `--root`, the repository is the current directory or the nearest
//! parent holding a `.git` entry, as trellis itself finds one. Outside any
//! repository no child is started, and `rpc_send`'s refusal, which the page
//! shows, says so and how to name one.

// A windowed program: without this Windows opens a console beside the
// window. Tests keep the console so their output is seen.
#![cfg_attr(not(test), windows_subsystem = "windows")]

mod child;
// The reducer the window feeds from #296 on. Until then the window forwards
// frames as they are, and only the tests build it.
#[cfg(test)]
mod frames;
#[cfg(test)]
mod state;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use tauri::{Emitter, Manager};

const USAGE: &str = "usage: trellis-desktop [--root <repo>] [--trellis <path>]";

/// What `rpc_send` writes to: the child, or the reason there is none.
enum Link {
    Child(child::Child),
    NoChild(String),
}

/// The window's managed state.
struct Core(Mutex<Link>);

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
                eprintln!("unknown argument: {other}\n{USAGE}");
                std::process::exit(2);
            }
        }
    }
    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![rpc_send])
        .setup(move |app| {
            let link = start(&trellis, root, app.handle().clone());
            app.manage(Core(Mutex::new(link)));
            Ok(())
        })
        .build(tauri::generate_context!());
    match app {
        Ok(app) => app.run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                shut_down(app);
            }
        }),
        Err(e) => {
            eprintln!("trellis-desktop: {e}");
            std::process::exit(1);
        }
    }
}

/// Starts the child for the startup root and its stdout reader, or says why
/// there is no child.
fn start(trellis: &Path, root: Option<PathBuf>, app: tauri::AppHandle) -> Link {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => return Link::NoChild(format!("cannot read the current directory: {e}")),
    };
    let root = match startup_root(root, &cwd) {
        Ok(r) => r,
        Err(why) => return Link::NoChild(why),
    };
    match child::Child::spawn(trellis, &root) {
        Ok(mut c) => {
            if let Some(out) = c.take_stdout() {
                forward_frames(out, app);
            }
            Link::Child(c)
        }
        Err(e) => Link::NoChild(format!(
            "could not start `{} --root {} rpc`: {e}",
            trellis.display(),
            root.display()
        )),
    }
}

/// The repository the window opens: `--root` as given, else `cwd` or the
/// nearest parent holding a `.git` entry. `Err` carries what the page shows
/// when there is none.
fn startup_root(explicit: Option<PathBuf>, cwd: &Path) -> Result<PathBuf, String> {
    if let Some(root) = explicit {
        return Ok(root);
    }
    cwd.ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            format!(
                "No repository was found at {} or above it, so trellis was not started. \
                 Start trellis-desktop inside a repository, or name one with --root <repo>.",
                cwd.display()
            )
        })
}

/// Emits each stdout line that parses as JSON as an `rpc-frame` event, on a
/// thread of its own, until the child's output ends.
fn forward_frames(mut out: child::Stdout, app: tauri::AppHandle) {
    std::thread::spawn(move || {
        loop {
            match out.next_line() {
                Ok(Some(line)) => {
                    let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    if let Err(e) = app.emit("rpc-frame", frame) {
                        eprintln!("trellis-desktop: cannot emit a frame: {e}");
                        return;
                    }
                }
                Ok(None) => return,
                Err(e) => {
                    eprintln!("trellis-desktop: cannot read trellis rpc: {e}");
                    return;
                }
            }
        }
    });
}

/// The page's one command: forwards a `get_state` to the child as one line.
#[tauri::command]
fn rpc_send(line: Value, core: tauri::State<'_, Core>) -> Result<(), String> {
    check_line(&line)?;
    let mut link = core.0.lock().unwrap_or_else(PoisonError::into_inner);
    match &mut *link {
        Link::Child(c) => c
            .send(&line)
            .map_err(|e| format!("cannot write to trellis rpc: {e}")),
        Link::NoChild(why) => Err(why.clone()),
    }
}

/// `Ok` for an object whose `type` is `get_state`; otherwise an error naming
/// the type it refused. The page can ask for state and nothing else until
/// #296 widens this.
fn check_line(line: &Value) -> Result<(), String> {
    match line.get("type").and_then(Value::as_str) {
        Some("get_state") => Ok(()),
        Some(other) => Err(format!(
            "rpc_send forwards only get_state; refused type {other:?}"
        )),
        None => Err("rpc_send forwards only get_state; refused a line with no string type".into()),
    }
}

/// Sends the child `quit` and waits for it (ADR 0048 section 3).
fn shut_down(app: &tauri::AppHandle) {
    let Some(core) = app.try_state::<Core>() else {
        return;
    };
    let mut link = core.0.lock().unwrap_or_else(PoisonError::into_inner);
    let closing = Link::NoChild("the desktop is closing".into());
    if let Link::Child(c) = std::mem::replace(&mut *link, closing)
        && let Err(e) = c.shutdown()
    {
        eprintln!("trellis-desktop: shutting down trellis rpc: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest_file(name: &str) -> String {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(name);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }

    /// A fresh empty directory under the system temp directory.
    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "trellis-desktop-main-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn rpc_send_forwards_only_get_state() {
        // The risk: the page drives the child past reading state, a `quit`
        // or a `prompt` from a page that is only meant to show the state.
        assert_eq!(
            check_line(&json!({"id": "desktop-1", "type": "get_state"})),
            Ok(())
        );
        for ty in ["quit", "prompt", "set_model", "abort", "bash", "GET_STATE"] {
            let err = check_line(&json!({"id": "x", "type": ty})).unwrap_err();
            assert!(err.contains(&format!("{ty:?}")), "{ty}: {err}");
        }
        for line in [
            json!({"id": "x"}),
            json!({"type": 1}),
            json!(["get_state"]),
            json!("get_state"),
            json!(null),
        ] {
            let err = check_line(&line).unwrap_err();
            assert!(err.contains("no string type"), "{line}: {err}");
        }
    }

    #[test]
    fn tauri_conf_has_one_window_no_dev_hooks_and_no_bundle() {
        // The risk: a dev server, a build hook or a bundle the ADR keeps
        // out; a second window; a CSP that lets inline or remote script
        // run; or a capability granting more than the page needs.
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
        let security = &conf["app"]["security"];
        assert_eq!(
            security["csp"],
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; \
             connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; \
             form-action 'none'"
        );
        let caps = security["capabilities"].as_array().unwrap();
        assert_eq!(caps.len(), 1, "one capability");
        assert_eq!(caps[0]["windows"], json!(["main"]));
        assert_eq!(
            caps[0]["permissions"],
            json!([
                "core:event:allow-listen",
                "core:event:allow-unlisten",
                "allow-rpc-send"
            ])
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

    #[test]
    fn startup_root_is_the_repository_holding_the_current_directory() {
        // The risk: started inside a repository, the window opens a
        // subdirectory as if it were the root, or nothing at all.
        let repo = scratch("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let sub = repo.join("src").join("deep");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(startup_root(None, &repo), Ok(repo.clone()));
        assert_eq!(startup_root(None, &sub), Ok(repo.clone()));
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn startup_root_outside_a_repository_says_how_to_name_one() {
        // The risk: started outside a repository, the window runs trellis
        // on a folder that is not one, or shows nothing and says nothing.
        let dir = scratch("plain");
        assert!(
            !dir.ancestors().any(|d| d.join(".git").exists()),
            "the temp directory {} must not be inside a repository",
            dir.display()
        );
        let why = startup_root(None, &dir).unwrap_err();
        assert!(why.contains("No repository was found"), "{why}");
        assert!(why.contains("--root <repo>"), "{why}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn startup_root_named_with_root_is_used_as_given() {
        let dir = scratch("named");
        assert_eq!(
            startup_root(Some(dir.clone()), Path::new("/")),
            Ok(dir.clone())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
