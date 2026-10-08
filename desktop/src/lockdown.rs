//! What the main window may load and open (ADR 0048 section 5).
//!
//! The page's one channel is Tauri IPC to the core: it fetches nothing and
//! links nowhere. So the window may navigate only within the app's own
//! origin, and may open no second window at all. `main.rs` installs both
//! predicates on the main window. The tests below also hold the lines
//! `tauri.conf.json` draws (the CSP, the capability, no dev server or build
//! hook, a frozen prototype), so an edit that widens one fails here.

use tauri::Url;

/// The app's own origin: `tauri://localhost` where the webview serves the
/// custom scheme as is (macOS, Linux), `http://tauri.localhost` where it
/// serves it over http (Windows' WebView2). No port: the app's origin has
/// none, and a port there is some other server.
pub(crate) fn navigation_allowed(url: &Url) -> bool {
    let host = url.host_str();
    url.port().is_none()
        && match url.scheme() {
            "tauri" => host == Some("localhost"),
            "http" => host == Some("tauri.localhost"),
            _ => false,
        }
}

/// Every new-window request (`window.open`, a `target="_blank"` link) is
/// refused, the app's own origin included: the desktop is one window, and a
/// second one would be a webview the core never set up.
pub(crate) fn new_window_allowed(_url: &Url) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use std::path::Path;

    fn conf() -> Value {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
        let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        serde_json::from_str(&text).unwrap()
    }

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn csp_has_no_unsafe_source_and_no_network_origin() {
        // The risk: a CSP edit that lets inline or eval'd script run, or
        // lets the page reach a network origin, so that markup in
        // trellis- or model-supplied text could act or send data out.
        let csp = conf()["app"]["security"]["csp"]
            .as_str()
            .expect("the CSP is one string")
            .to_owned();
        let mut directives: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for d in csp.split(';').map(str::trim).filter(|d| !d.is_empty()) {
            let mut words = d.split_whitespace();
            let name = words.next().unwrap();
            assert!(
                directives.insert(name, words.collect()).is_none(),
                "{name} given twice"
            );
        }
        let expected: BTreeMap<&str, Vec<&str>> = [
            ("default-src", vec!["'none'"]),
            ("script-src", vec!["'self'"]),
            ("style-src", vec!["'self'"]),
            ("img-src", vec!["'self'"]),
            ("connect-src", vec!["ipc:", "http://ipc.localhost"]),
            ("object-src", vec!["'none'"]),
            ("base-uri", vec!["'none'"]),
            ("form-action", vec!["'none'"]),
        ]
        .into_iter()
        .collect();
        assert_eq!(directives, expected);
        for source in directives.values().flatten() {
            let s = source.to_ascii_lowercase();
            assert!(
                !s.contains("unsafe-") && !s.contains("data:") && !s.contains('*'),
                "unsafe source {source}"
            );
            assert!(
                !(s.starts_with("http:") || s.starts_with("https:")) || s == "http://ipc.localhost",
                "network origin {source}"
            );
        }
    }

    #[test]
    fn main_capability_grants_only_listen_unlisten_and_the_apps_commands() {
        // The risk: a capability that grants a core default set (window,
        // webview, path, app...), a command the page does not call, a remote
        // origin, or a window besides main. This is the one place the
        // expected set is written.
        let security = &conf()["app"]["security"];
        let caps = security["capabilities"].as_array().unwrap();
        assert_eq!(caps.len(), 1, "one capability");
        let cap = caps[0].as_object().unwrap();
        assert!(!cap.contains_key("remote"), "a remote origin");
        assert_eq!(cap["windows"], json!(["main"]));
        assert!(!cap.contains_key("webviews"), "a webview besides main");
        assert_eq!(
            cap["permissions"],
            json!([
                "core:event:allow-listen",
                "core:event:allow-unlisten",
                "allow-rpc-send",
                "allow-get-trails",
                "allow-open-repository",
                "allow-get-overview"
            ])
        );
    }

    #[test]
    fn no_dev_url_and_no_build_hooks() {
        // The risk: a dev server the window would load instead of the
        // shipped files, a hook that runs a toolchain the ADR refuses, or a
        // bundle step the desktop does not have.
        let conf = conf();
        let build = conf["build"].as_object().unwrap();
        for key in ["devUrl", "beforeDevCommand", "beforeBuildCommand"] {
            assert!(!build.contains_key(key), "build.{key}");
        }
        assert_eq!(build["frontendDist"], "ui");
        assert_eq!(conf["bundle"]["active"], false);
    }

    #[test]
    fn freeze_prototype_is_on() {
        // The risk: page script that replaces a built-in prototype's
        // method and so changes what Tauri's IPC script does.
        assert_eq!(conf()["app"]["security"]["freezePrototype"], true);
    }

    #[test]
    fn navigation_stays_on_the_app_origin() {
        // The risk: the window navigating to a page that is not the app's,
        // which would then hold the capability the app's page holds.
        for ok in [
            "tauri://localhost/",
            "tauri://localhost/index.html",
            "http://tauri.localhost/",
            "http://tauri.localhost/index.html",
        ] {
            assert!(navigation_allowed(&url(ok)), "refused {ok}");
        }
        for bad in [
            "https://example.com/",
            "http://127.0.0.1:8000/",
            "file:///",
            "http://tauri.localhost:8000/",
            "https://tauri.localhost/",
        ] {
            assert!(!navigation_allowed(&url(bad)), "allowed {bad}");
        }
    }

    #[test]
    fn new_window_requests_are_refused() {
        // The risk: `window.open` or a `target="_blank"` link opening a
        // second webview, on a remote page or the app's own.
        for bad in ["https://example.com/", "tauri://localhost/", "about:blank"] {
            assert!(!new_window_allowed(&url(bad)), "allowed {bad}");
        }
    }
}
