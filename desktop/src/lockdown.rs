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

    fn ui_file(name: &str) -> String {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui").join(name);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }

    /// One element of `ui/index.html`: its tag and attributes (names
    /// lowercased, values as written), its parent's index, and the text of
    /// everything inside it.
    #[derive(Debug)]
    struct Element {
        tag: String,
        attrs: BTreeMap<String, Option<String>>,
        parent: Option<usize>,
        text: String,
    }

    impl Element {
        fn attr(&self, name: &str) -> Option<&str> {
            self.attrs.get(name).map(|v| v.as_deref().unwrap_or(""))
        }
    }

    /// The elements HTML closes itself: they take no end tag.
    const VOID: [&str; 8] = ["meta", "link", "br", "hr", "img", "input", "source", "wbr"];

    /// A small scanner over the page as written, not an HTML parser: no
    /// crate here parses HTML, and a new one needs an ADR. It reads tags
    /// between `<` and `>` with quoted or bare attribute values, skips
    /// comments and the doctype, gives the void elements no children, and
    /// closes an end tag back to its matching start. That is enough for a
    /// page this repository writes by hand; markup it would misread (a `>`
    /// inside an attribute value, a raw-text element other than an empty
    /// `<script>`) panics rather than passing, so a page that outgrows it
    /// fails here, not quietly.
    fn page() -> Vec<Element> {
        let html = ui_file("index.html");
        let mut out: Vec<Element> = Vec::new();
        let mut open: Vec<usize> = Vec::new();
        let mut rest = html.as_str();
        while let Some(lt) = rest.find('<') {
            let text = &rest[..lt];
            for &i in &open {
                out[i].text.push_str(text);
            }
            rest = &rest[lt..];
            if let Some(after) = rest.strip_prefix("<!--") {
                let end = after.find("-->").expect("an unclosed comment");
                rest = &after[end + 3..];
                continue;
            }
            let gt = tag_end(rest);
            let tag = &rest[1..gt];
            rest = &rest[gt + 1..];
            if tag.starts_with('!') {
                continue;
            }
            if let Some(name) = tag.strip_prefix('/') {
                let name = name.trim().to_ascii_lowercase();
                let at = open
                    .iter()
                    .rposition(|&i| out[i].tag == name)
                    .unwrap_or_else(|| panic!("</{name}> closes nothing"));
                open.truncate(at);
                continue;
            }
            let (name, attrs) = parse_tag(tag);
            let void = VOID.contains(&name.as_str()) || tag.ends_with('/');
            if name == "script" || name == "style" {
                assert!(rest.trim_start().starts_with("</"), "<{name}> has a body");
            }
            out.push(Element {
                tag: name,
                attrs,
                parent: open.last().copied(),
                text: String::new(),
            });
            if !void {
                open.push(out.len() - 1);
            }
        }
        assert!(open.is_empty(), "unclosed elements at the end of the page");
        out
    }

    /// Where the tag `s` starts with ends: its first `>` outside a quoted
    /// attribute value.
    fn tag_end(s: &str) -> usize {
        let mut quote = None;
        for (i, c) in s.char_indices() {
            match (quote, c) {
                (None, '"' | '\'') => quote = Some(c),
                (Some(q), c) if c == q => quote = None,
                (None, '>') => return i,
                _ => {}
            }
        }
        panic!("an unclosed tag");
    }

    /// `name a="x" b='y' c=z d` into the lowercased tag name and its
    /// attributes. An attribute given twice panics: the browser keeps the
    /// first, and a test that read the second would check the wrong one.
    fn parse_tag(tag: &str) -> (String, BTreeMap<String, Option<String>>) {
        let tag = tag.trim_end_matches('/');
        let name_end = tag.find(|c: char| c.is_whitespace()).unwrap_or(tag.len());
        let name = tag[..name_end].to_ascii_lowercase();
        let mut attrs = BTreeMap::new();
        let mut s = tag[name_end..].trim_start();
        while !s.is_empty() {
            let end = s
                .find(|c: char| c.is_whitespace() || c == '=')
                .unwrap_or(s.len());
            let key = s[..end].to_ascii_lowercase();
            s = s[end..].trim_start();
            let value = if let Some(v) = s.strip_prefix('=') {
                let v = v.trim_start();
                let (value, after) = match v.chars().next() {
                    Some(q @ ('"' | '\'')) => {
                        let close = v[1..].find(q).expect("an unclosed attribute value") + 1;
                        (&v[1..close], &v[close + 1..])
                    }
                    _ => {
                        let e = v.find(char::is_whitespace).unwrap_or(v.len());
                        (&v[..e], &v[e..])
                    }
                };
                s = after.trim_start();
                Some(value.to_string())
            } else {
                None
            };
            assert!(
                attrs.insert(key.clone(), value).is_none(),
                "<{name}> gives {key} twice"
            );
        }
        (name, attrs)
    }

    fn by_id<'a>(page: &'a [Element], id: &str) -> Vec<&'a Element> {
        page.iter().filter(|e| e.attr("id") == Some(id)).collect()
    }

    fn within(page: &[Element], mut i: usize, ancestor: usize) -> bool {
        while let Some(p) = page[i].parent {
            if p == ancestor {
                return true;
            }
            i = p;
        }
        false
    }

    fn index_of(page: &[Element], id: &str) -> usize {
        let found: Vec<usize> = (0..page.len())
            .filter(|&i| page[i].attr("id") == Some(id))
            .collect();
        assert_eq!(found.len(), 1, "one element with id {id}");
        found[0]
    }

    /// The six sections the rail switches, in rail order (#300 as amended
    /// 2026-10-07; overview is the landing section).
    const SECTIONS: [&str; 6] = ["overview", "run", "trails", "codebase", "board", "vault"];

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

    #[test]
    fn page_rail_buttons_control_sections_that_exist() {
        // The risk: a rail button that shows nothing, or a section no
        // button reaches; a later view shown before it is built, or a run
        // section that says it is not built. Six sections (#300 as amended
        // 2026-10-07), overview first and the one shown at load.
        let page = page();
        let navs: Vec<usize> = (0..page.len()).filter(|&i| page[i].tag == "nav").collect();
        assert_eq!(navs.len(), 1, "one nav");
        let buttons: Vec<&Element> = (0..page.len())
            .filter(|&i| page[i].tag == "button" && within(&page, i, navs[0]))
            .map(|i| &page[i])
            .collect();
        let controls: Vec<&str> = buttons
            .iter()
            .map(|b| {
                b.attr("aria-controls")
                    .expect("a rail button with no aria-controls")
            })
            .collect();
        assert_eq!(controls, SECTIONS, "the rail's buttons, in order");
        let sections: Vec<&Element> = page.iter().filter(|e| e.tag == "section").collect();
        let ids: Vec<&str> = sections.iter().filter_map(|s| s.attr("id")).collect();
        assert_eq!(ids, SECTIONS, "every section, each with a rail button");
        for id in SECTIONS {
            let found = by_id(&page, id);
            assert_eq!(found.len(), 1, "one element with id {id}");
            let s = found[0];
            assert_eq!(s.tag, "section", "#{id} is a section");
            assert_eq!(
                s.attrs.contains_key("hidden"),
                id != "overview",
                "#{id}'s hidden at load"
            );
            assert_eq!(
                s.text.contains("Not built yet"),
                id != "run",
                "#{id} and \"Not built yet\""
            );
        }
    }

    #[test]
    fn page_has_one_log_region() {
        // The risk: two live log regions, so a screen reader announces the
        // run twice or the wrong one, or a log outside the run section.
        let page = page();
        let logs: Vec<usize> = (0..page.len())
            .filter(|&i| page[i].attr("role") == Some("log"))
            .collect();
        assert_eq!(logs.len(), 1, "one role=\"log\"");
        assert!(
            within(&page, logs[0], index_of(&page, "run")),
            "the log is in #run"
        );
        assert_eq!(
            page[logs[0]].attr("id"),
            Some("pane-transcript"),
            "render.js draws the log"
        );
    }

    #[test]
    fn page_run_section_holds_the_panes_render_draws() {
        // The risk: render.js drawing into an id the page does not have, so
        // a pane trellis reported is silently never shown; or DESK-10's
        // graph container arriving with content of its own.
        let page = page();
        let run = index_of(&page, "run");
        let render = ui_file("render.js");
        let ids: Vec<&str> = render
            .split('"')
            .filter(|w| w.starts_with("pane-"))
            .collect();
        for want in [
            "pane-transcript",
            "pane-actions",
            "pane-edits",
            "pane-checks",
            "pane-radius",
            "pane-budget",
            "pane-tier",
            "pane-running",
            "pane-outcome",
        ] {
            assert!(ids.contains(&want), "render.js draws no {want}");
        }
        for id in &ids {
            assert!(
                within(&page, index_of(&page, id), run),
                "#{id} is outside #run"
            );
        }
        let graph = index_of(&page, "graph");
        assert!(within(&page, graph, run), "#graph is outside #run");
        assert!(page[graph].text.trim().is_empty(), "#graph is not empty");
        assert!(
            !page
                .iter()
                .enumerate()
                .any(|(i, e)| e.parent == Some(graph) && i != graph),
            "#graph has children"
        );
    }

    #[test]
    fn page_has_no_style_attribute() {
        // The risk: an inline style, which the CSP's `style-src 'self'`
        // refuses (so the page draws wrongly) or which a later CSP edit
        // would have to allow; and a script that writes one.
        let page = page();
        for e in &page {
            assert!(!e.attrs.contains_key("style"), "<{} style=...>", e.tag);
            assert_ne!(e.tag, "style", "a <style> element");
        }
        for name in ["main.js", "render.js"] {
            let js = ui_file(name);
            for bad in [".style", "cssText", "\"style\"", "'style'"] {
                assert!(!js.contains(bad), "{name} writes a style: {bad}");
            }
        }
    }

    #[test]
    fn page_controls_have_accessible_names() {
        // The risk: a control a screen reader announces only as "button"
        // or "edit text", so nobody using one can tell what it does.
        let page = page();
        let labelled: Vec<&str> = page
            .iter()
            .filter(|e| e.tag == "label")
            .filter_map(|e| e.attr("for"))
            .collect();
        let mut seen = 0;
        for e in page
            .iter()
            .filter(|e| ["button", "input", "textarea", "select"].contains(&e.tag.as_str()))
        {
            seen += 1;
            let named = !e.text.trim().is_empty()
                || e.attr("aria-label").is_some_and(|l| !l.trim().is_empty())
                || e.attr("id").is_some_and(|id| labelled.contains(&id));
            assert!(named, "<{} {:?}> has no accessible name", e.tag, e.attrs);
        }
        assert!(seen >= SECTIONS.len(), "the rail's buttons were read");
    }

    #[test]
    fn page_has_no_positive_tabindex() {
        // The risk: a positive tabindex, which takes the element out of
        // document order so keyboard focus jumps around the page.
        for e in page() {
            if let Some(t) = e.attr("tabindex") {
                let n: i64 = t
                    .trim()
                    .parse()
                    .unwrap_or_else(|_| panic!("<{}> tabindex={t:?} is not a number", e.tag));
                assert!(n <= 0, "<{}> tabindex={n}", e.tag);
            }
        }
    }

    /// The window's minimum width, from tauri.conf.json.
    fn min_window_width() -> f64 {
        conf()["app"]["windows"][0]["minWidth"]
            .as_f64()
            .expect("the main window's minWidth")
    }

    #[test]
    fn page_css_fits_the_minimum_window() {
        // The risk: a fixed width wider than the narrowest window the shell
        // allows, so the page scrolls sideways or clips a pane there.
        //
        // Each `width`, `min-width`, `flex-basis` and `flex` declaration
        // (and each length in `grid-template-columns`, which fixes widths
        // the same way) is read value by value. A px length must be at most
        // the window's minimum; %, vw, fr, auto, 0 and the content keywords
        // are relative to the window or its contents and pass. Any other
        // length (em, rem, ch, pt, calc() ...) fails: this test cannot
        // measure it, so it is not taken on trust.
        let max = min_window_width();
        let css = ui_file("style.css");
        let mut text = String::new();
        let mut rest = css.as_str();
        while let Some(i) = rest.find("/*") {
            text.push_str(&rest[..i]);
            let end = rest[i..].find("*/").expect("an unclosed CSS comment");
            rest = &rest[i + end + 2..];
        }
        text.push_str(rest);
        let mut read = 0;
        for decl in text.split(['{', '}', ';']) {
            let Some((prop, value)) = decl.split_once(':') else {
                continue;
            };
            let prop = prop.trim().to_ascii_lowercase();
            if ![
                "width",
                "min-width",
                "flex-basis",
                "flex",
                "grid-template-columns",
            ]
            .contains(&prop.as_str())
            {
                continue;
            }
            read += 1;
            for v in value
                .split(|c: char| c.is_whitespace() || matches!(c, ',' | '(' | ')' | '/'))
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_ascii_lowercase)
            {
                let number_end = v
                    .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
                    .unwrap_or(v.len());
                let (number, unit) = v.split_at(number_end);
                if number.is_empty() {
                    assert!(
                        [
                            "auto",
                            "none",
                            "max-content",
                            "min-content",
                            "fit-content",
                            "repeat",
                            "minmax",
                            "auto-fill",
                            "auto-fit",
                            "inherit",
                            "initial",
                            "unset",
                        ]
                        .contains(&v.as_str()),
                        "{prop}: {v} cannot be measured against the window"
                    );
                    continue;
                }
                let n: f64 = number
                    .parse()
                    .unwrap_or_else(|_| panic!("{prop}: {v} is not a number"));
                match unit {
                    "px" => assert!(n <= max, "{prop}: {v} is wider than the {max}px window"),
                    "%" | "vw" | "fr" => {}
                    // A bare number: `flex`'s grow and shrink, or a 0 length.
                    "" => {}
                    _ => panic!("{prop}: {v} is in a unit this test cannot measure"),
                }
            }
        }
        assert!(
            read > 0,
            "style.css sets no width at all: read the right file?"
        );
    }
}
