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
    use crate::frames::{Frame, parse_stdout};
    use crate::state::{Changed, DesktopState};
    use serde_json::{Value, json};
    use std::collections::{BTreeMap, BTreeSet};
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
        page_of(&ui_file("index.html"))
    }

    /// [`page`] over `html`, so a test can scan a planted copy of the page.
    fn page_of(html: &str) -> Vec<Element> {
        let mut out: Vec<Element> = Vec::new();
        let mut open: Vec<usize> = Vec::new();
        let mut rest = html;
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

    /// The sections that are built, and so do not read "Not built yet":
    /// run (#300), overview (#562), codebase (#307), trails (#304), vault
    /// (#309) and board (#311).
    const BUILT: [&str; 6] = ["run", "overview", "codebase", "trails", "vault", "board"];

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
        // 2026-10-07), overview first and the one shown at load; the ones
        // in BUILT are built.
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
                !BUILT.contains(&id),
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
    fn page_has_one_alert_region() {
        // The risk (#302): two alert regions, so a screen reader announces a
        // failure twice or in the wrong place; or a banner dialogs.js does
        // not draw, so a failure is never said; or the approval dialog,
        // trellis messages pane or their buttons missing from the page.
        let page = page();
        let alerts: Vec<usize> = (0..page.len())
            .filter(|&i| page[i].attr("role") == Some("alert"))
            .collect();
        assert_eq!(alerts.len(), 1, "one role=\"alert\"");
        assert_eq!(page[alerts[0]].attr("id"), Some("alert"));
        assert!(
            within(&page, index_of(&page, "alert-text"), alerts[0]),
            "the alert text is in the banner"
        );
        let dialog = index_of(&page, "approval");
        assert_eq!(page[dialog].tag, "dialog");
        for (id, text) in [
            ("approval-approve", "Approve"),
            ("approval-reject", "Reject"),
        ] {
            let b = index_of(&page, id);
            assert!(within(&page, b, dialog), "#{id} is outside the dialog");
            assert_eq!(page[b].tag, "button");
            assert_eq!(page[b].text.trim(), text);
        }
        for id in ["approval-question", "approval-detail"] {
            assert!(within(&page, index_of(&page, id), dialog), "#{id}");
        }
        let messages = index_of(&page, "messages");
        assert_eq!(page[messages].tag, "details");
        let summary = page
            .iter()
            .enumerate()
            .find(|(i, e)| e.tag == "summary" && within(&page, *i, messages))
            .map(|(_, e)| e.text.trim().to_string());
        assert_eq!(summary.as_deref(), Some("trellis messages"));
        let js = ui_file("dialogs.js");
        for want in [
            "\"alert\"",
            "\"alert-text\"",
            "\"approval\"",
            "\"messages-log\"",
            "showModal()",
            "returnValue === \"approve\"",
            "\"unhandledrejection\"",
        ] {
            assert!(js.contains(want), "dialogs.js has no {want}");
        }
        for bad in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "document.write",
        ] {
            assert!(!js.contains(bad), "dialogs.js uses {bad}");
        }
        assert!(
            ui_file("main.js")
                .contains(r#"import { drawDialogs, wireDialogs } from "./dialogs.js";"#),
            "main.js does not load dialogs.js"
        );
    }

    #[test]
    fn approval_dialog_forgets_a_replaced_child_s_questions() {
        // The risk (#302 review): rpc numbers approvals per process from
        // `approval-1`, so a page that remembers which ids it answered
        // across children filters out the next child's `approval-1`: no
        // dialog opens and that run waits for ever. No JavaScript runs
        // under cargo test, so this holds dialogs.js to forgetting, on
        // `child-started`, what it answered and showed, and to marking the
        // dialogs still to close as withdrawn rather than dropping them: a
        // close already queued for the old child's dialog must not be read
        // as the answer to the new child's question. A node smoke script
        // with a fake DOM ran the module itself (see the commit).
        let js = code_of("dialogs.js");
        let started = js
            .split(r#"listen("child-started""#)
            .nth(1)
            .and_then(|r| r.split("});").next())
            .expect("dialogs.js listens for child-started");
        assert!(
            started.contains("forgetChild(doc)"),
            "child-started does not forget the last child: {started}"
        );
        let forget = js
            .split("function forgetChild(doc)")
            .nth(1)
            .and_then(|r| r.split("\n}").next())
            .expect("dialogs.js has forgetChild");
        for want in ["answered.clear()", "withdraw("] {
            assert!(forget.contains(want), "forgetChild lacks {want}");
        }
        let withdraw = js
            .split("function withdraw(dialog)")
            .nth(1)
            .and_then(|r| r.split("\n}").next())
            .expect("dialogs.js has withdraw");
        for want in [
            "for (const e of opened) e.withdrawn = true",
            "shown = null",
            "dialog.close()",
        ] {
            assert!(withdraw.contains(want), "withdraw lacks {want}");
        }
        // A close is an answer only for an entry not withdrawn.
        assert!(
            js.contains("if (entry === undefined || entry.withdrawn) return;"),
            "a withdrawn dialog's close is taken as an answer"
        );
        assert!(
            ui_file("main.js").contains("await wireDialogs(document, window, tauri);"),
            "main.js asks for something before dialogs.js listens"
        );
    }

    #[test]
    fn page_reload_asks_for_the_starters_too() {
        // The risk (#301 review): a page that loads after its child started
        // (a reload) sees no child-started, and main.js's fallback asked for
        // the state, the overview and the models but not `get_commands`, so
        // the composer offered no starters until another repository was
        // opened. No JavaScript runs under cargo test, so this holds the
        // text; a node smoke script with a fake DOM loaded main.js (see the
        // commit).
        let main = code_of("main.js");
        let fallback = main
            .split("if (await askState(tauri)) {")
            .nth(1)
            .and_then(|r| r.split('}').next())
            .expect("main.js has the reload fallback");
        assert!(
            fallback.contains("askStarters(document, tauri);"),
            "{fallback}"
        );
        assert!(
            main.contains(
                r#"import { askStarters, drawComposer, wireComposer } from "./composer.js";"#
            ),
            "main.js does not take askStarters from composer.js"
        );
        assert!(
            code_of("composer.js").contains("export function askStarters(doc, tauri)"),
            "composer.js does not export askStarters"
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
            "pane-session-tokens",
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

    /// Every `.js` file under ui/, by file name, with its `//` comments
    /// removed, so a key named only in a comment is not read: a whole-line
    /// comment, and a comment after code from its ` //`. A URL in a string
    /// (`http://`) has no space before its `//` and is kept. No file has a
    /// block comment, asserted, since one would count as code.
    fn ui_code() -> BTreeMap<String, String> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
        let mut out = BTreeMap::new();
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let name = entry.unwrap().file_name().into_string().unwrap();
            if !name.ends_with(".js") {
                continue;
            }
            let text = ui_file(&name);
            let code = text
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .map(|l| l.split_once(" //").map_or(l, |(code, _)| code))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(!code.contains("/*"), "{name} has a block comment");
            out.insert(name, code);
        }
        for want in ["main.js", "render.js", "overview.js"] {
            assert!(out.contains_key(want), "ui/{want} was not read");
        }
        out
    }

    /// The entries of the object literal `const <name> = {` in `code`, one
    /// per line, as `(key, the first string in its value)`: render.js's
    /// `key: ["pane-id", draw]` gives the pane id, overview.js's
    /// `"ov-id": draw` none.
    fn table(code: &str, name: &str) -> Vec<(String, Option<String>)> {
        let open = format!("const {name} = {{");
        let start = code.find(&open).unwrap_or_else(|| panic!("no `{open}`")) + open.len();
        let end = code[start..].find("\n};").expect("the table closes") + start;
        let entries: Vec<(String, Option<String>)> = code[start..end]
            .lines()
            .filter_map(|l| {
                let (key, value) = l.trim().split_once(':')?;
                let key = key.trim().trim_matches('"');
                let named = !key.is_empty()
                    && key
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c == '_' || c == '-');
                named.then(|| (key.to_string(), value.split('"').nth(1).map(str::to_string)))
            })
            .collect();
        assert!(!entries.is_empty(), "`{open}` has no entries");
        entries
    }

    /// The top-level function `name` in `code`, from `function name(` to
    /// its closing line.
    fn function_body<'a>(code: &'a str, name: &str) -> &'a str {
        let at = code
            .find(&format!("function {name}("))
            .unwrap_or_else(|| panic!("no function {name}"));
        let rest = &code[at..];
        &rest[..rest.find("\n}").expect("the function closes")]
    }

    /// `import { a, b } from "./m.js";` in `code`, as name -> `m.js`.
    fn imports_of(code: &str) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for line in code.lines().filter(|l| l.starts_with("import {")) {
            let (names, from) = line["import {".len()..]
                .split_once("} from \"./")
                .unwrap_or_else(|| panic!("an import from outside ui/: {line}"));
            let module = from.trim_end_matches(';').trim_end_matches('"');
            for n in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                out.insert(n.to_string(), module.to_string());
            }
        }
        out
    }

    /// The keys `snippet` reads: each `"key" in <object>`, and each
    /// `<prefix>key` for the prefixes given (`view.`, `payload.`, `o.`).
    fn keys_read(snippet: &str, prefixes: &[&str]) -> BTreeSet<String> {
        let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
        let key = |k: &str| !k.is_empty() && k.chars().all(|c| c.is_ascii_lowercase() || c == '_');
        let mut out = BTreeSet::new();
        for (i, _) in snippet.match_indices("\" in ") {
            let before = &snippet[..i];
            if let Some(q) = before.rfind('"')
                && key(&before[q + 1..])
            {
                out.insert(before[q + 1..].to_string());
            }
        }
        for prefix in prefixes {
            for (i, _) in snippet.match_indices(prefix) {
                if snippet[..i].chars().next_back().is_some_and(ident) {
                    continue;
                }
                let k: String = snippet[i + prefix.len()..]
                    .chars()
                    .take_while(|&c| ident(c))
                    .collect();
                if key(&k) {
                    out.insert(k);
                }
            }
        }
        out
    }

    /// The modules `page` runs: each `<script src>` in it, and what those
    /// import, transitively. A module that listens for the view but is never
    /// loaded draws nothing.
    fn loaded_modules(page: &[Element], ui: &BTreeMap<String, String>) -> BTreeSet<String> {
        let mut todo: Vec<String> = page
            .iter()
            .filter(|e| e.tag == "script")
            .filter_map(|e| e.attr("src").map(str::to_string))
            .collect();
        let mut out = BTreeSet::new();
        while let Some(m) = todo.pop() {
            if out.insert(m.clone()) {
                let code = ui
                    .get(&m)
                    .unwrap_or_else(|| panic!("the page loads {m}, which ui/ lacks"));
                todo.extend(imports_of(code).into_values());
            }
        }
        out
    }

    /// The view keys the page reads, each with the files that read it. The
    /// page takes the `view` event two ways: main.js's listener hands the
    /// payload whole to a draw function, found through main.js's imports
    /// (render.js's, composer.js's, model.js's...); and a module may listen
    /// for it itself (vault.js, queue.js...). A draw function that looks a
    /// key up in its module's `PANES` table reads every key the table names.
    /// A key read any other way (destructuring, a computed name) is not
    /// seen, so it is reported unread: the check fails rather than passing.
    /// Only the modules `page` loads count.
    fn view_reads(
        page: &[Element],
        ui: &BTreeMap<String, String>,
    ) -> BTreeMap<String, BTreeSet<String>> {
        let loaded = loaded_modules(page, ui);
        let mut reads: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut note = |keys: BTreeSet<String>, file: &str| {
            for k in keys {
                reads.entry(k).or_default().insert(file.to_string());
            }
        };
        let mut listeners = 0;
        for (file, code) in ui.iter().filter(|(f, _)| loaded.contains(*f)) {
            let imports = imports_of(code);
            for body in code.split(r#"listen("view""#).skip(1) {
                let body = body.split("});").next().unwrap_or(body);
                listeners += 1;
                note(keys_read(body, &["view.", "payload."]), file);
                for line in body.lines().map(str::trim) {
                    // `drawX(document, event.payload, ...)`: the view, whole.
                    let name: String = line
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .collect();
                    if name.is_empty()
                        || !line[name.len()..].starts_with('(')
                        || !line.contains("event.payload")
                    {
                        continue;
                    }
                    let module = imports.get(&name).unwrap_or(file);
                    let callee = ui
                        .get(module)
                        .unwrap_or_else(|| panic!("{file} imports {name} from {module}, not read"));
                    let f = function_body(callee, &name);
                    note(keys_read(f, &["view.", "payload."]), module);
                    if f.contains("PANES[") {
                        note(
                            table(callee, "PANES").into_iter().map(|(k, _)| k).collect(),
                            module,
                        );
                    }
                }
            }
        }
        assert!(listeners > 0, "no module listens for the view event");
        reads
    }

    /// The view keys nothing on the page reads, and the keys the page reads
    /// that the view does not carry, each `key (files)`.
    fn view_cross_check(
        view: &BTreeSet<String>,
        page: &[Element],
        ui: &BTreeMap<String, String>,
    ) -> (Vec<String>, Vec<String>) {
        let reads = view_reads(page, ui);
        let undrawn = view
            .iter()
            .filter(|k| !reads.contains_key(*k))
            .cloned()
            .collect();
        let unsent = reads
            .iter()
            .filter(|(k, _)| !view.contains(*k))
            .map(|(k, files)| format!("{k} ({files:?})"))
            .collect();
        (undrawn, unsent)
    }

    /// A loop event as `trellis rpc` writes it: a `tracing` JSON line, its
    /// fields beside `message` (client_contract.json's `rpc.events`).
    fn loop_event(message: &str, fields: Value) -> String {
        let mut f = fields.as_object().cloned().expect("fields are an object");
        f.insert("message".into(), json!(message));
        json!({"timestamp": "2026-10-10T12:00:00.000000Z", "level": "INFO",
               "fields": f, "target": "trellis_loop::agent"})
        .to_string()
    }

    /// A whole run as the child prints it, in client_contract.json's shapes:
    /// its start, its budget, a turn each for a radius, an edit and a check,
    /// an approval question, a stderr line, a stdout line that is not JSON,
    /// its end, and the `get_state` answer that reports its tier.
    fn whole_run() -> Vec<Frame> {
        let stdout = [
            json!({"type": "run_start", "version": 1, "trellis": "0.1.0",
                   "task": "rename f to g", "model": "scripted:x", "root": "/w/app",
                   "id": "desktop-prompt-1"})
            .to_string(),
            loop_event("run budget", json!({"max_iterations": 8})),
            loop_event(
                "turn",
                json!({"turn": 1, "call": "blast_radius app.core.f", "verdict": "None",
                       "check_kind": "", "observation": "4 callers resolved",
                       "resolved": 4, "candidates": 1, "unresolved": 2,
                       "model_ms": 1200, "prompt_bytes": 2048, "completion_tokens": 40}),
            ),
            loop_event(
                "turn",
                json!({"turn": 2, "call": "edit app/core.py", "verdict": "None",
                       "check_kind": "", "observation": "applied: true (+1 -1)",
                       "model_ms": 900, "prompt_bytes": 2300, "completion_tokens": 55,
                       "queue_wait_ms": 100}),
            ),
            loop_event(
                "turn",
                json!({"turn": 3, "call": "run_checks app/core.py", "verdict": "Some(true)",
                       "check_kind": "Tests", "observation": "3 passed",
                       "model_ms": 700, "prompt_bytes": 2500, "completion_tokens": 12}),
            ),
            json!({"type": "approval_request", "id": "approval-1",
                   "question": "Run bash?", "detail": "pytest -q"})
            .to_string(),
        ];
        let end = [
            json!({"type": "run_end", "outcome": "converged", "exit": 0,
                   "id": "desktop-prompt-1", "run_id": "20261010-120000-000"})
            .to_string(),
            json!({"type": "response", "command": "get_state", "success": true,
                   "id": "desktop-state",
                   "data": {"running": false, "classifying": false, "run_request_id": null,
                            "model": "scripted:x", "root": "/w/app",
                            "last_run": {"outcome": "converged", "exit": 0,
                                         "id": "desktop-prompt-1",
                                         "run_id": "20261010-120000-000"},
                            "pending_approvals": [], "tier": "scripted"}})
            .to_string(),
        ];
        let typed = |line: &String| {
            let f = parse_stdout(line);
            assert!(
                !matches!(f, Frame::Ignored | Frame::Unparsed(_)),
                "not a frame the desktop reads: {line}"
            );
            f
        };
        let mut frames: Vec<Frame> = stdout.iter().map(typed).collect();
        frames.push(Frame::Human("trellis: indexed 12 modules".into()));
        let text = parse_stdout("Traceback (most recent call last):");
        assert!(matches!(text, Frame::Unparsed(_)), "{text:?}");
        frames.push(text);
        frames.extend(end.iter().map(typed));
        frames
    }

    #[test]
    fn view_renders_every_pane_key() {
        // The risk (#513, ADR 0052 section 1): the reducer gains a pane the
        // page never draws, so a run the core reports in full shows part of
        // it, and every test still passes; or the page reads a key the
        // reducer no longer sends, so a pane stays blank. No JavaScript runs
        // under cargo test (ADR 0048 section 5), so this crosses names: the
        // keys of the view the reducer serializes after a whole run, against
        // the keys the page's modules read from the `view` event.
        let mut s = DesktopState::default();
        for (i, frame) in whole_run().into_iter().enumerate() {
            let what = format!("{frame:?}");
            let changed = s.update(frame);
            assert!(!changed.is_empty(), "frame {i} changed no pane: {what}");
        }
        // What the core sends for a forwarded get_state (commands.rs's
        // `resend`): every pane, so every key the view has.
        let view = s.view(Changed::everything());
        assert_eq!(view["edits"], json!(["app/core.py"]), "{view}");
        assert_eq!(view["checks"][0]["result"], "passed", "{view}");
        assert_eq!(view["radius"]["resolved"], 4, "{view}");
        assert_eq!(view["budget"]["turn"], 3, "{view}");
        assert_eq!(view["outcome"]["outcome"], "converged", "{view}");
        assert_eq!(view["tier"], "scripted", "{view}");
        assert!(view["alert"].is_string(), "{view}");
        assert_eq!(view["stderr"], json!(["trellis: indexed 12 modules"]));
        let keys: BTreeSet<String> = view
            .as_object()
            .expect("the view is an object")
            .keys()
            .cloned()
            .collect();

        let ui = ui_code();
        let page = page();
        let (undrawn, unsent) = view_cross_check(&keys, &page, &ui);
        assert!(
            undrawn.is_empty(),
            "the page draws no pane for view key(s) {undrawn:?}: no module the page \
             loads reads them from the view event (render.js's PANES, or a \
             `\"key\" in view`)"
        );
        assert!(
            unsent.is_empty(),
            "the page reads view key(s) the reducer never sends: {unsent:?}"
        );

        // The check refuses each gap: a pane the reducer gains, a pane
        // render.js stops drawing, a key the page reads that no pane is, and
        // a module that draws a pane but that the page never loads.
        let mut gained = keys.clone();
        gained.insert("planted_pane".into());
        assert_eq!(view_cross_check(&gained, &page, &ui).0, ["planted_pane"]);
        let mut dropped = ui.clone();
        let render = dropped.get_mut("render.js").unwrap();
        let tier = "  tier: [\"pane-tier\", tier],\n";
        assert!(render.contains(tier), "render.js's tier entry moved");
        *render = render.replace(tier, "");
        assert_eq!(view_cross_check(&keys, &page, &dropped).0, ["tier"]);
        let mut extra = ui.clone();
        let render = extra.get_mut("render.js").unwrap();
        *render = render.replace(
            "const PANES = {",
            "const PANES = {\n  planted_key: [\"pane-tier\", tier],",
        );
        assert_eq!(
            view_cross_check(&keys, &page, &extra).1,
            [r#"planted_key ({"render.js"})"#]
        );
        let script = r#"<script type="module" src="vault.js"></script>"#;
        let html = ui_file("index.html");
        assert!(
            html.contains(script),
            "index.html loads vault.js differently"
        );
        let unloaded = page_of(&html.replace(script, ""));
        assert_eq!(view_cross_check(&keys, &unloaded, &ui).0, ["vault"]);
    }

    /// What in the section `section` nothing writes: each element id there
    /// that is neither in `written` nor a label (an id an `aria-labelledby`
    /// names, a heading the page writes itself), and each region holding no
    /// written element, by its heading; then each id in `written` that is
    /// not one element inside the section.
    fn unfilled(page: &[Element], section: &str, written: &BTreeSet<String>) -> Vec<String> {
        let s = index_of(page, section);
        let labels: BTreeSet<&str> = page
            .iter()
            .filter_map(|e| e.attr("aria-labelledby"))
            .flat_map(str::split_whitespace)
            .collect();
        let mut out = Vec::new();
        for (i, e) in page.iter().enumerate() {
            if !within(page, i, s) {
                continue;
            }
            if e.attr("role") == Some("region")
                && !(0..page.len()).any(|j| {
                    within(page, j, i) && page[j].attr("id").is_some_and(|id| written.contains(id))
                })
            {
                let title = e
                    .attr("aria-labelledby")
                    .and_then(|l| by_id(page, l).first().map(|h| h.text.trim().to_string()))
                    .unwrap_or_default();
                out.push(format!("the region \"{title}\""));
            }
            if let Some(id) = e.attr("id")
                && !labels.contains(id)
                && !written.contains(id)
            {
                out.push(format!("#{id}"));
            }
        }
        for id in written {
            let found: Vec<usize> = (0..page.len())
                .filter(|&i| page[i].attr("id") == Some(id.as_str()))
                .collect();
            if found.len() != 1 || !within(page, found[0], s) {
                out.push(format!("#{id} (drawn, but not one element in #{section})"));
            }
        }
        out
    }

    /// The ids render.js's `drawView` writes to: its `PANES` table's.
    fn render_writes(render: &str) -> BTreeSet<String> {
        table(render, "PANES")
            .into_iter()
            .map(|(k, id)| id.unwrap_or_else(|| panic!("render.js's {k} names no element")))
            .collect()
    }

    #[test]
    fn view_renders_every_run_section() {
        // The risk (#513): a pane in the run section that nothing fills, so
        // the page shows an empty box under a title as if trellis reported
        // nothing; or render.js drawing into an id the run section lacks.
        let ui = ui_code();
        let render = &ui["render.js"];
        let draw = function_body(render, "drawView");
        for want in ["PANES[key]", "getElementById(entry[0])"] {
            assert!(
                draw.contains(want),
                "drawView does not draw through PANES: {want}"
            );
        }
        let written = render_writes(render);
        let page = page();
        let gaps = unfilled(&page, "run", &written);
        assert!(
            gaps.is_empty(),
            "in the run section, render.js never writes to {gaps:?}"
        );

        // The check refuses each gap: a pane added to the page that nothing
        // draws, and a pane render.js stops drawing.
        let html = ui_file("index.html");
        let at = r#"<div class="pane pane-graph""#;
        assert!(html.contains(at), "the graph pane moved");
        let planted = html.replace(
            at,
            &format!(
                r#"<div class="pane" role="region" aria-labelledby="planted-title"><h3 id="planted-title">Planted</h3><ul id="pane-planted"></ul></div>{at}"#
            ),
        );
        assert_eq!(
            unfilled(&page_of(&planted), "run", &written),
            [r#"the region "Planted""#, "#pane-planted"]
        );
        let mut fewer = written.clone();
        fewer.remove("pane-edits");
        assert_eq!(
            unfilled(&page, "run", &fewer),
            [r#"the region "Edits""#, "#pane-edits"]
        );
    }

    #[test]
    fn view_renders_every_overview_pane() {
        // The risk (#513 as amended 2026-10-07): the overview trellis prints
        // gains a part overview.js never reads, or the overview section has
        // a pane nothing fills, so bare `trellis` lands on a page that
        // leaves out what `trellis overview` said. The overview is its own
        // object, not the reducer's view, so its keys come from
        // client_contract.json's `overview` entry, which trellis's own tests
        // hold the binary to.
        let contract: Value = serde_json::from_str(include_str!(
            "../../crates/trellis-cli/tests/client_contract.json"
        ))
        .expect("the contract is JSON");
        let ov = &contract["overview"];
        let top = |path: &str| path.split(['.', '[']).next().unwrap_or(path).to_string();
        let mut keys: BTreeSet<String> = BTreeSet::new();
        for list in ["always", "outside_a_repository"] {
            for p in ov[list].as_array().expect("a list of paths") {
                keys.insert(top(p.as_str().expect("a path")));
            }
        }
        keys.extend(
            ov["optional"]
                .as_object()
                .expect("optional paths")
                .keys()
                .map(|p| top(p)),
        );
        assert!(keys.len() >= 6, "the overview entry was read: {keys:?}");

        let ui = ui_code();
        let js = &ui["overview.js"];
        let read = keys_read(js, &["o."]);
        let unread: Vec<&String> = keys.iter().filter(|k| !read.contains(*k)).collect();
        assert!(unread.is_empty(), "overview.js never reads {unread:?}");
        let unlisted: Vec<&String> = read.iter().filter(|k| !keys.contains(*k)).collect();
        assert!(
            unlisted.is_empty(),
            "overview.js reads {unlisted:?}, which the overview does not carry"
        );

        let draw = function_body(js, "drawOverview");
        for want in ["Object.entries(PANES)", "getElementById(id)"] {
            assert!(
                draw.contains(want),
                "drawOverview does not draw through PANES: {want}"
            );
        }
        let written: BTreeSet<String> = table(js, "PANES").into_iter().map(|(id, _)| id).collect();
        let page = page();
        let gaps = unfilled(&page, "overview", &written);
        assert!(
            gaps.is_empty(),
            "in the overview section, overview.js never writes to {gaps:?}"
        );

        // The check refuses each gap: a part the overview gains, and a pane
        // overview.js stops drawing.
        let mut gained = keys.clone();
        gained.insert("planted".into());
        assert_eq!(
            gained
                .iter()
                .filter(|k| !read.contains(*k))
                .collect::<Vec<_>>(),
            ["planted"]
        );
        let mut fewer = written.clone();
        fewer.remove("ov-board");
        assert_eq!(
            unfilled(&page, "overview", &fewer),
            [r#"the region "Board""#, "#ov-board"]
        );
    }

    #[test]
    fn codebase_section_holds_what_codebase_js_draws() {
        // The risk (#307): codebase.js drawing into an id the page lacks,
        // so the graph or the details never show; a search box with no
        // label; or the codebase section still saying it is not built.
        let page = page();
        let section = index_of(&page, "codebase");
        let js = ui_file("codebase.js");
        for id in [
            "cb-status",
            "cb-crumbs",
            "cb-back",
            "cb-graph",
            "cb-details",
            "cb-search",
            "cb-results",
        ] {
            assert!(
                js.contains(&format!("\"{id}\"")),
                "codebase.js draws no {id}"
            );
            assert!(
                within(&page, index_of(&page, id), section),
                "#{id} is outside #codebase"
            );
        }
        let label = page
            .iter()
            .find(|e| e.tag == "label" && e.attr("for") == Some("cb-search"))
            .expect("the search box has a label");
        assert!(!label.text.trim().is_empty(), "an empty label");
        assert_eq!(
            page[index_of(&page, "cb-back")].tag,
            "button",
            "Back is a button"
        );
        let graph = index_of(&page, "cb-graph");
        assert_eq!(
            page[graph].attr("tabindex"),
            Some("0"),
            "the graph takes keys"
        );
        assert!(!page[section].text.contains("Not built yet"));
    }

    #[test]
    fn radius_pane_holds_what_radius_js_draws() {
        // The risk (#308): radius.js drawing into an id the page lacks, so
        // a radius trellis answered is never shown; a symbol field with no
        // label; or the pane outside the section the rail shows it in.
        let page = page();
        let section = index_of(&page, "codebase");
        let js = ui_file("radius.js");
        for id in ["rd-form", "rd-symbol", "rd-status", "rd-result"] {
            assert!(js.contains(&format!("\"{id}\"")), "radius.js draws no {id}");
            assert!(
                within(&page, index_of(&page, id), section),
                "#{id} is outside #codebase"
            );
        }
        assert!(
            page.iter()
                .any(|e| e.tag == "label" && e.attr("for") == Some("rd-symbol")),
            "the symbol field has no label"
        );
        let html = ui_file("index.html");
        assert!(
            html.contains(r#"<script type="module" src="radius.js"></script>"#),
            "index.html does not load radius.js as a module"
        );
    }

    #[test]
    fn trails_section_holds_what_trails_js_draws() {
        // The risk (#304): trails.js drawing into an id the page lacks, so
        // the list or a record never shows; a filter with no label; or the
        // section still saying it is not built.
        let page = page();
        let section = index_of(&page, "trails");
        let js = ui_file("trails.js");
        for id in ["tr-status", "tr-filter", "tr-list", "tr-detail"] {
            assert!(js.contains(&format!("\"{id}\"")), "trails.js draws no {id}");
            assert!(
                within(&page, index_of(&page, id), section),
                "#{id} is outside #trails"
            );
        }
        assert!(
            page.iter()
                .any(|e| e.tag == "label" && e.attr("for") == Some("tr-filter")),
            "the filter has no label"
        );
        assert!(!page[section].text.contains("Not built yet"));
    }

    #[test]
    fn vault_section_holds_what_vault_js_draws() {
        // The risk (#309): vault.js drawing into an id the page lacks, so
        // a search trellis answered is never shown; a search box with no
        // label; or the section still saying it is not built.
        let page = page();
        let section = index_of(&page, "vault");
        let js = ui_file("vault.js");
        for id in ["vt-status", "vt-form", "vt-query", "vt-hits", "vt-note"] {
            assert!(js.contains(&format!("\"{id}\"")), "vault.js draws no {id}");
            assert!(
                within(&page, index_of(&page, id), section),
                "#{id} is outside #vault"
            );
        }
        assert!(
            page.iter()
                .any(|e| e.tag == "label" && e.attr("for") == Some("vt-query")),
            "the search box has no label"
        );
        assert!(!page[section].text.contains("Not built yet"));
        let html = ui_file("index.html");
        assert!(
            html.contains(r#"<script type="module" src="vault.js"></script>"#),
            "index.html does not load vault.js as a module"
        );
    }

    #[test]
    fn vault_js_writes_text_links_nothing_and_sends_only_vault_reads() {
        // The risk (#309, ADR 0017, S10): a note's title, path or issue
        // parsed as markup or drawn as a link a reader might follow; or a
        // control that writes the vault, which stays a `trellis vault`
        // command the person runs (#70). Read with its `//` comments
        // removed, which name what it does not do; it has no string that
        // holds `//`.
        let js: String = ui_file("vault.js")
            .lines()
            .map(|l| l.split_once("//").map_or(l, |(code, _)| code))
            .collect::<Vec<_>>()
            .join("\n");
        for bad in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "createElement(\"a\")",
            "href",
            "get_trails",
            "open_repository",
            "\"prompt\"",
        ] {
            assert!(!js.contains(bad), "vault.js uses {bad}");
        }
        assert!(js.contains("textContent"), "vault.js writes no text");
        // Every type it sends is one of the two reads.
        let sent: Vec<&str> = js
            .split("type: \"")
            .skip(1)
            .map(|rest| &rest[..rest.find('"').unwrap()])
            .collect();
        assert_eq!(sent, ["vault_graph", "vault_search"], "{sent:?}");
    }

    /// `name` under ui/ with its `//` comments removed, so a word named
    /// only in a comment does not count. Each file read this way has no
    /// string that holds `//`.
    fn code_of(name: &str) -> String {
        ui_file(name)
            .lines()
            .map(|l| l.split_once("//").map_or(l, |(code, _)| code))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn board_section_holds_what_queue_js_draws() {
        // The risk (#311): queue.js drawing into an id the page lacks, so
        // the counts or the tasks trellis answered are never shown; a
        // Refresh with no name; or the section still saying it is not
        // built.
        let page = page();
        let section = index_of(&page, "board");
        let js = code_of("queue.js");
        for id in [
            "qb-status",
            "qb-refresh",
            "qb-counts",
            "qb-tasks",
            "qb-prci",
        ] {
            assert!(js.contains(&format!("\"{id}\"")), "queue.js draws no {id}");
            assert!(
                within(&page, index_of(&page, id), section),
                "#{id} is outside #board"
            );
        }
        assert!(!page[section].text.contains("Not built yet"));
        let html = ui_file("index.html");
        assert!(
            html.contains(r#"<script type="module" src="queue.js"></script>"#),
            "index.html does not load queue.js as a module"
        );
    }

    #[test]
    fn queue_js_draws_the_view_and_asks_only_for_reads() {
        // The risk (#311): the board counting tasks itself, so it can
        // disagree with the core's eight cells or add a sum; a task's text
        // parsed as markup; or a control that writes the queue, whose
        // writes are `trellis queue` and `trellis work` (ADR 0020 section
        // 5). No JavaScript runs under cargo test, so this holds the file
        // to what it reads and sends.
        let js = code_of("queue.js");
        for bad in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "createElement(\"a\")",
            "href",
            "\"prompt\"",
            // Arithmetic on the counts: a sum, or a count derived from the
            // tasks rather than taken from the view.
            "reduce(",
            "c.count +",
            "+ c.count",
            "+= c",
            ".length +",
            "filter((t) => t.status",
        ] {
            assert!(!js.contains(bad), "queue.js uses {bad}");
        }
        assert!(js.contains("textContent"), "queue.js writes no text");
        // What each task shows: the ticket's four, and the chain (#559).
        for field in [
            "t.task",
            "t.priority",
            "t.labels",
            "t.waiting",
            "t.blockers",
            "t.parent",
            "t.error",
            "c.count",
            "c.status",
        ] {
            assert!(js.contains(field), "queue.js does not draw {field}");
        }
        let sent: Vec<&str> = js
            .split("type: \"")
            .skip(1)
            .map(|rest| &rest[..rest.find('"').unwrap()])
            .collect();
        assert_eq!(sent, ["get_queue"], "{sent:?}");
        let invoked: Vec<&str> = js
            .split("invoke(\"")
            .skip(1)
            .map(|rest| &rest[..rest.find('"').unwrap()])
            .collect();
        assert_eq!(invoked, ["rpc_send", "get_overview"], "{invoked:?}");
        // When it asks: on opening, on Refresh, and after each run_end.
        assert!(js.contains(r#"aria-controls="board""#), "{js}");
        assert!(js.contains(r#"getElementById("qb-refresh").addEventListener("click""#));
        assert!(js.contains(r#"f.type === "run_end") askQueue"#));
    }

    #[test]
    fn board_reads_pr_ci_once_at_a_time_and_only_the_newest() {
        // The risk (#311 review): each board click ran `trellis overview`
        // and two `gh` calls with no guard, so clicks stacked them, and a
        // slow answer for the last repository, read after another was
        // opened, was drawn over the new one's. No JavaScript runs under
        // cargo test, so this holds the text; a node smoke script with a
        // fake DOM ran queue.js (see the commit).
        let js = code_of("queue.js");
        let ask = js
            .split("async function askPrCi(doc, tauri)")
            .nth(1)
            .and_then(|r| r.split("\n}").next())
            .expect("queue.js has askPrCi");
        for want in [
            "if (prciInFlight) return;",
            "prciInFlight = true;",
            "prciReads += 1;",
            "const mine = prciReads;",
            "if (mine !== prciReads) return;",
            "prciInFlight = false;",
        ] {
            assert!(ask.contains(want), "askPrCi lacks {want}");
        }
        let started = js
            .split(r#"listen("child-started""#)
            .nth(1)
            .and_then(|r| r.split("});").next())
            .expect("queue.js listens for child-started");
        for want in ["prciReads += 1;", "prciInFlight = false;"] {
            assert!(started.contains(want), "child-started lacks {want}");
        }
    }

    #[test]
    fn board_shows_pr_ci_or_its_reason() {
        // The risk (#311 as amended 2026-10-07): the board drops the PR
        // and CI state `gh` reported, or, with `gh` missing or logged out,
        // says nothing; or it draws them differently from the overview.
        // Both draw through overview.js's drawPrCi, which
        // eagle_eye_board_draws_pr_ci_entries_and_the_unavailable_reason
        // holds to every field and the reason.
        let js = code_of("queue.js");
        assert!(
            js.contains(r#"import { drawPrCi } from "./overview.js";"#),
            "queue.js does not draw PR and CI with overview.js's drawPrCi"
        );
        assert!(
            js.contains("drawPrCi(doc, b.pr_ci)"),
            "not from board.pr_ci"
        );
        assert!(
            js.contains("PR and CI state unavailable: ${err}"),
            "an overview that cannot be read is said"
        );
        let ov = overview_code();
        let f = &ov[ov
            .find("export function drawPrCi")
            .expect("drawPrCi is exported")..];
        let f = &f[..f.find("\n}").unwrap()];
        for want in [
            "pr.available === true",
            "PR and CI state unavailable: ${str(pr.reason)",
            "list(pr.pull_requests)",
            "list(pr.runs)",
        ] {
            assert!(f.contains(want), "drawPrCi lacks {want}");
        }
        assert!(
            ov.contains("out.push(...drawPrCi(doc, b.pr_ci))"),
            "the overview's board pane draws through drawPrCi too"
        );
    }

    /// `overview.js` with its `//` comments removed, so a field named only
    /// in a comment does not count as read. It has no string or regex that
    /// holds `//`, which this would cut short.
    fn overview_code() -> String {
        ui_file("overview.js")
            .lines()
            .map(|l| l.split_once("//").map_or(l, |(code, _)| code))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Every key `v` and its arrays' objects carry, at any depth.
    fn keys_of(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                for (k, v) in o {
                    out.push(k.clone());
                    keys_of(v, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|v| keys_of(v, out)),
            _ => {}
        }
    }

    /// What `trellis --json overview` prints for a repository with one
    /// trail, one queued chain and `gh` logged in (client_contract.json's
    /// `overview` entry, #558), and the `pr_ci` it prints when `gh` cannot
    /// be used.
    fn overview_fixture() -> (Value, Value) {
        let available = json!({
            "recent": [{"path": "/w/app", "last_run": {
                "run_id": "20261007-093000-000", "started_at": "2026-10-07T09:30:00Z",
                "outcome": "converged", "integrity": "intact", "task": "rename the parser"}}],
            "root": "/w/app",
            "last_run": {
                "run_id": "20261007-093000-000", "started_at": "2026-10-07T09:30:00Z",
                "outcome": "converged", "integrity": "intact", "task": "rename the parser",
                "turns": 6, "edits": 2, "checks": [{"kind": "pytest", "passed": true}],
                "verified": 3, "reviewed": 1, "tier": "constrained"},
            "codebase": {"modules_indexed": 12, "bound_call_sites": 90, "ambiguous": 4,
                "unresolved": 7, "modules": [], "modules_not_drawn": 0},
            "codebase_reason": null,
            "board": {
                "tasks": [{"id": "q-2", "state": "pending", "status": "todo", "priority": "p2",
                    "task": "tidy the parser", "labels": [], "assignee": null,
                    "waiting": "after q-1", "blockers": ["q-1"], "parent": "q-0"}],
                "pr_ci": {"available": true,
                    "pull_requests": [{"number": 41, "title": "<b>parser</b>", "head": "t41",
                        "draft": true,
                        "checks": {"passed": 2, "failed": 1, "pending": 0, "other": 0}}],
                    "runs": [{"id": 900, "workflow": "ci", "branch": "t41",
                        "status": "completed", "conclusion": "failure",
                        "created_at": "2026-10-07T09:31:00Z"}]}}
        });
        let unavailable =
            json!({"available": false, "reason": "gh is not logged in; run gh auth login"});
        (available, unavailable)
    }

    #[test]
    fn eagle_eye_section_has_four_labelled_panes() {
        // The risk: an overview pane a screen reader cannot name, a pane
        // missing, or overview.js drawing into an id the page lacks, so a
        // part of the overview trellis printed is never shown.
        let page = page();
        let overview = index_of(&page, "overview");
        let regions: Vec<usize> = (0..page.len())
            .filter(|&i| page[i].attr("role") == Some("region") && within(&page, i, overview))
            .collect();
        let titles: Vec<String> = regions
            .iter()
            .map(|&i| {
                let label = page[i]
                    .attr("aria-labelledby")
                    .expect("an overview pane with no aria-labelledby");
                let h = index_of(&page, label);
                assert!(within(&page, h, i), "#{label} is outside its pane");
                page[h].text.trim().to_string()
            })
            .collect();
        assert_eq!(
            titles,
            ["Recent repositories", "Last run", "Codebase", "Board"],
            "the overview's panes, in order"
        );
        let js = overview_code();
        for (&region, id) in
            regions
                .iter()
                .zip(["ov-recent", "ov-last-run", "ov-codebase", "ov-board"])
        {
            assert!(
                within(&page, index_of(&page, id), region),
                "#{id} is outside its pane"
            );
            assert!(
                js.contains(&format!("\"{id}\"")),
                "overview.js draws no {id}"
            );
        }
        assert!(
            !page[overview].text.contains("Not built yet"),
            "#overview still says it is not built"
        );
    }

    #[test]
    fn eagle_eye_overview_js_parses_no_html() {
        // The risk: a trail task, a queue task or a PR title from `gh`
        // written as markup, so text someone else wrote runs or reshapes
        // the page (ADR 0007, S10).
        let js = ui_file("overview.js");
        for bad in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "document.write",
            "eval(",
            "Function(",
            "DOMParser",
            "createContextualFragment",
        ] {
            assert!(!js.contains(bad), "overview.js uses {bad}");
        }
        assert!(js.contains("textContent"), "overview.js writes no text");
    }

    #[test]
    fn eagle_eye_overview_is_the_first_rail_button() {
        // The risk: the window landing on another section, or an overview
        // section nothing draws, so bare `trellis` opens on an empty page.
        let page = page();
        let first = page
            .iter()
            .find(|e| e.tag == "button" && e.attr("aria-controls").is_some())
            .expect("a rail button");
        assert_eq!(first.attr("aria-controls"), Some("overview"));
        assert_eq!(first.attr("aria-current"), Some("page"));
        assert!(
            !page[index_of(&page, "overview")]
                .attrs
                .contains_key("hidden"),
            "#overview is hidden at load"
        );
        let main = ui_file("main.js");
        assert!(
            main.contains(r#"import { drawOverview } from "./overview.js";"#),
            "main.js does not load overview.js"
        );
        assert!(
            main.contains(r#"listen("overview""#) && main.contains(r#"invoke("get_overview")"#),
            "main.js draws neither the overview event nor get_overview"
        );
    }

    #[test]
    fn eagle_eye_board_draws_pr_ci_entries_and_the_unavailable_reason() {
        // The risk: the board drops a PR or CI field `gh` reported, or,
        // with `gh` missing or logged out, shows nothing and says nothing.
        // No JavaScript runs under cargo test, so this holds overview.js to
        // reading every field the fixture's `pr_ci` carries, each as a
        // property, and to the words it puts before the reason.
        let (o, unavailable) = overview_fixture();
        let js = overview_code();
        let mut keys = vec!["tasks".to_string(), "pr_ci".to_string()];
        keys_of(&o["board"]["pr_ci"], &mut keys);
        keys_of(&unavailable, &mut keys);
        // A task's chain and how it reads; its labels, assignee and status
        // are the queue view's, not the board's.
        keys.extend(
            [
                "id", "state", "priority", "task", "waiting", "blockers", "parent",
            ]
            .map(String::from),
        );
        keys.sort();
        keys.dedup();
        let missing: Vec<&String> = keys
            .iter()
            .filter(|k| !js.contains(&format!(".{k}")))
            .collect();
        assert!(missing.is_empty(), "overview.js reads no {missing:?}");
        assert!(
            js.contains("pr.available === true"),
            "the PR and CI lists are drawn only when gh was available"
        );
        assert!(
            js.contains("PR and CI state unavailable: ${str(pr.reason)"),
            "the unavailable reason is drawn after its label"
        );
        for k in ["pull_requests", "runs"] {
            assert!(
                js.contains(&format!("list(pr.{k})")),
                "pr_ci.{k} is not drawn as a list"
            );
        }
    }

    #[test]
    fn eagle_eye_last_run_draws_every_field_and_says_what_it_lacks() {
        // The risk: a last-run figure the overview carries left undrawn,
        // or one it does not carry (budget, approvals) drawn as a number.
        let (o, _) = overview_fixture();
        let js = overview_code();
        let mut keys = Vec::new();
        keys_of(&o["last_run"], &mut keys);
        keys_of(&o["codebase"], &mut keys);
        keys.retain(|k| !["modules", "modules_not_drawn"].contains(&k.as_str()));
        let missing: Vec<&String> = keys
            .iter()
            .filter(|k| !js.contains(&format!(".{k}")))
            .collect();
        assert!(missing.is_empty(), "overview.js reads no {missing:?}");
        for row in [
            r#"["budget", "not reported in the overview"]"#,
            r#"["approvals", "not reported in the overview"]"#,
        ] {
            assert!(js.contains(row), "overview.js lacks {row}");
        }
        for bad in ["/ c.", "/ t.", "%"] {
            assert!(!js.contains(bad), "overview.js computes a ratio: {bad}");
        }
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
        for name in [
            "main.js",
            "render.js",
            "overview.js",
            "composer.js",
            "dialogs.js",
            "model.js",
            "settings.js",
            "status.js",
            "graph.js",
            "codebase.js",
            "radius.js",
            "trails.js",
            "vault.js",
            "queue.js",
        ] {
            let js = ui_file(name);
            for bad in [".style", "cssText", "\"style\"", "'style'"] {
                assert!(!js.contains(bad), "{name} writes a style: {bad}");
            }
        }
    }

    #[test]
    fn model_gate_composer_and_send_are_disabled_in_the_page_as_served() {
        // The risk (#564): prompt input usable for the moment between the
        // page loading and the first view, or for good when the view never
        // comes (no child, a child that exited), so a task can be typed and
        // sent before any model is chosen. Served disabled, only
        // composer.js opens it, and only on the view's `model`.
        let page = page();
        let composer = index_of(&page, "composer");
        for id in ["composer-text", "composer-send", "composer-control"] {
            let i = index_of(&page, id);
            assert!(within(&page, i, composer), "#{id} is outside the composer");
            assert!(
                page[i].attrs.contains_key("disabled"),
                "#{id} is not disabled as served"
            );
        }
        // The model pane is open at launch, above every section, and holds
        // what model.js draws into.
        let pane = index_of(&page, "model-pane");
        assert_eq!(page[pane].tag, "details");
        assert!(page[pane].attrs.contains_key("open"), "closed at launch");
        let first_section = (0..page.len())
            .find(|&i| page[i].tag == "section")
            .expect("a section");
        assert!(pane < first_section, "the model pane is below a section");
        let js = ui_file("model.js");
        for id in [
            "model-pane",
            "model-summary",
            "model",
            "model-current",
            "model-options",
            "model-typed",
            "model-save",
            "model-refresh",
            "model-probed",
            "model-providers",
            "model-note",
        ] {
            assert!(
                within(&page, index_of(&page, id), pane) || id == "model-pane",
                "#{id} is outside the model pane"
            );
            assert!(js.contains(&format!("\"{id}\"")), "model.js draws no {id}");
        }
        // The gate is the view's `model`, in composer.js, and nothing else
        // there sets the four controls' disabled state; the form's submit
        // handler refuses too, so Enter in a control a page bug left open
        // sends nothing (#564 review).
        let composer_js = ui_file("composer.js");
        for want in [
            "p.text.disabled = !modelChosen",
            "p.send.disabled = !modelChosen",
            "\"model\" in view",
            "if (!modelChosen)",
        ] {
            assert!(composer_js.contains(want), "composer.js lacks {want}");
        }
        assert!(
            ui_file("main.js").contains(
                r#"import { askModels, drawModelPane, wireModelPane } from "./model.js";"#
            ),
            "main.js does not load model.js"
        );
    }

    #[test]
    fn model_gate_model_js_parses_no_html() {
        // The risk: a spec, an address, a runtime's error or a provider
        // name, all trellis- or configuration-supplied, written as markup,
        // so text someone else wrote runs or reshapes the page (ADR 0007,
        // S10); or the pane sending a model by any path but the core's
        // `rpc_send`, where the model guard sits.
        let js = ui_file("model.js");
        for bad in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "document.write",
            "eval(",
            "Function(",
            "DOMParser",
            "createContextualFragment",
            "fetch(",
        ] {
            assert!(!js.contains(bad), "model.js uses {bad}");
        }
        assert!(js.contains("textContent"), "model.js writes no text");
        let invokes: Vec<&str> = js
            .split("invoke(\"")
            .skip(1)
            .map(|r| r.split('"').next().unwrap_or(""))
            .collect();
        assert_eq!(invokes, ["rpc_send"], "model.js calls only rpc_send");
    }

    #[test]
    fn session_tokens_pane_is_drawn_as_text_from_trellis_s_answer() {
        // The risk (#313): the pane titled for the session's tokens sits
        // outside the run section, writes a null figure as 0, joins the two
        // figures, or is never refreshed, so it shows a stale count; or the
        // page asks for it outside rpc_send.
        let page = page();
        let pane = index_of(&page, "pane-session-tokens");
        let region = page[pane].parent.expect("a pane in a region");
        assert_eq!(page[region].attr("role"), Some("region"));
        let title = index_of(
            &page,
            page[region]
                .attr("aria-labelledby")
                .expect("a labelled pane"),
        );
        assert_eq!(page[title].text.trim(), "Session tokens");
        let render = ui_file("render.js");
        for want in [
            "` · prompt ${tokens(m.prompt_tokens)}`",
            "` · completion ${tokens(m.completion_tokens)}`",
            r#"isCount(v) ? String(v) : "unknown""#,
            "el(doc, \"code\", model)",
        ] {
            assert!(render.contains(want), "render.js lacks {want}");
        }
        let main = ui_file("main.js");
        for want in [
            r#"type: "get_session_stats""#,
            r#"listen("rpc-frame""#,
            r#"f.type === "run_end") askSessionTokens(tauri)"#,
        ] {
            assert!(main.contains(want), "main.js lacks {want}");
        }
        let started = main
            .split(r#"listen("child-started""#)
            .nth(1)
            .and_then(|r| r.split("});").next())
            .expect("main.js listens for child-started");
        assert!(
            started.contains("askSessionTokens(tauri)"),
            "main.js does not ask on child-started"
        );
    }

    /// Whether `field` names a key or an endpoint: what the page may not
    /// ask the user for, since a key never reaches the page (ADR 0010) and
    /// an endpoint is the user's config's to name (ADR 0024 §3).
    fn names_a_key_or_endpoint(field: &str) -> bool {
        let f = field.to_ascii_lowercase();
        ["key", "token", "endpoint", "base_url"]
            .iter()
            .any(|w| f.contains(w))
    }

    /// Every `input` and `textarea` in `page` naming a key or an endpoint
    /// by its `id`, `name`, `aria-label` or the text of a `label` for it.
    fn key_or_endpoint_fields(page: &[Element]) -> Vec<String> {
        let label_for = |id: &str| -> Vec<String> {
            page.iter()
                .filter(|e| e.tag == "label" && e.attr("for") == Some(id))
                .map(|e| e.text.clone())
                .collect()
        };
        page.iter()
            .filter(|e| ["input", "textarea"].contains(&e.tag.as_str()))
            .filter_map(|e| {
                let mut said: Vec<String> = ["id", "name", "aria-label"]
                    .iter()
                    .filter_map(|a| e.attr(a).map(str::to_string))
                    .collect();
                if let Some(id) = e.attr("id") {
                    said.extend(label_for(id));
                }
                said.iter()
                    .any(|s| names_a_key_or_endpoint(s))
                    .then(|| format!("<{} {:?}>", e.tag, e.attrs))
            })
            .collect()
    }

    #[test]
    fn page_has_no_key_or_endpoint_field() {
        // The risk (#306, ADR 0010, ADR 0024 §3): a field that takes a key
        // puts the user's key in the page, and one that takes an endpoint
        // lets the page choose where a key is sent. The model is chosen by
        // spec, and a hosted spec only as the user's own config wrote it.
        let page = page();
        let fields = key_or_endpoint_fields(&page);
        assert!(fields.is_empty(), "key or endpoint fields: {fields:?}");
        assert!(
            page.iter().any(|e| e.tag == "input"),
            "no input was read: the scan proves nothing"
        );
        // The scan refuses each way of naming one.
        for planted in [
            r#"<input id="api-key">"#,
            r#"<input name="token">"#,
            r#"<textarea aria-label="Endpoint"></textarea>"#,
            r#"<input id="x"><label for="x">Your base_url</label>"#,
        ] {
            let mut elements = Vec::new();
            let mut rest = planted;
            while let Some(lt) = rest.find('<') {
                let gt = tag_end(&rest[lt..]) + lt;
                let tag = &rest[lt + 1..gt];
                let after = &rest[gt + 1..];
                if !tag.starts_with('/') {
                    let (name, attrs) = parse_tag(tag);
                    let text = after.split('<').next().unwrap_or("").to_string();
                    elements.push(Element {
                        tag: name,
                        attrs,
                        parent: None,
                        text,
                    });
                }
                rest = after;
            }
            assert_eq!(
                key_or_endpoint_fields(&elements).len(),
                1,
                "{planted} passed the scan"
            );
        }
    }

    #[test]
    fn page_picker_status_bar_and_repository_switch_are_drawn() {
        // The risk (#306): a picker that sends the repository's spec as the
        // user's, a "repository default" that names a model, a catalogue
        // shown as runnable models, a status bar that guesses, or a
        // repository switch that hides the core's refusal.
        let page = page();
        let bar = index_of(&page, "status-bar");
        let status = ui_file("status.js");
        for id in [
            "status-trellis",
            "status-root",
            "status-model",
            "status-run",
            "status-child",
        ] {
            assert!(within(&page, index_of(&page, id), bar), "#{id}");
            assert!(
                status.contains(&format!("\"{id}\"")),
                "status.js draws no {id}"
            );
        }
        for want in [
            r#"listen("child-started""#,
            r#"listen("rpc-exited""#,
            r#"listen("no-child""#,
            "m.source",
            "view.running === true",
            "version not reported",
        ] {
            assert!(status.contains(want), "status.js lacks {want}");
        }
        let settings = ui_file("settings.js");
        for want in [
            r#"REPOSITORY_DEFAULT = "repository default""#,
            r#"type: "set_model", configured: true"#,
            r#"type: "set_model", model: user"#,
            r#"invoke("open_repository", { path: p })"#,
            "note.textContent = String(err)",
            "e.download_gib",
            "e.license",
        ] {
            assert!(settings.contains(want), "settings.js lacks {want}");
        }
        // The repository's spec is never sent as a model: the choices name
        // only `user`, a listed local spec or the typed one.
        assert!(
            !settings.contains("configured.repository"),
            "settings.js reads the repository's spec as a choice"
        );
        let pane = index_of(&page, "model-pane");
        for id in ["model-catalog", "repo-form", "repo-path", "repo-note"] {
            assert!(within(&page, index_of(&page, id), pane), "#{id}");
        }
        assert!(
            page[index_of(&page, "model-catalog-title")]
                .text
                .contains("weights trellis model pull fetches")
        );
        for name in ["settings.js", "status.js"] {
            let js = ui_file(name);
            for bad in [
                "innerHTML",
                "outerHTML",
                "insertAdjacentHTML",
                "document.write",
                "fetch(",
            ] {
                assert!(!js.contains(bad), "{name} uses {bad}");
            }
        }
        assert!(
            ui_file("model.js").contains(r#"import { modelChoices } from "./settings.js";"#),
            "the picker's choices are not settings.js's"
        );
    }

    /// One of `settings.js`'s choices, from `value: <value>,` to its close.
    fn settings_choice(value: &str) -> String {
        let settings = code_of("settings.js");
        let at = settings
            .find(&format!("value: {value},"))
            .unwrap_or_else(|| panic!("settings.js offers no {value}"));
        let rest = &settings[at..];
        rest[..rest.find("});").expect("a choice closes")].to_string()
    }

    #[test]
    fn page_picker_preselects_the_user_s_configured_spec() {
        // The risk (#564 review): ADR 0048 section 5's owner-directed
        // amendment asks for the user's configured spec preselected, and
        // the picker preselected the spec the configuration resolves to,
        // where the repository's wins over the user's. No JavaScript runs
        // under cargo test, so this holds the text; a node smoke script
        // with a fake DOM ran modelChoices (see the commit).
        assert!(
            settings_choice(r#""user""#).contains("preselect: true,"),
            "the user's spec is not preselected"
        );
        assert!(
            settings_choice(r#""default""#)
                .contains("preselect: user === null && resolved !== null,"),
            "repository default is preselected over the user's spec"
        );
        assert!(
            settings_choice(r#""typed""#)
                .contains("preselect: choices.every((c) => !c.preselect),"),
            "the typed spec is not the last resort"
        );
    }

    #[test]
    fn page_picker_keys_local_choices_by_spec() {
        // The risk (#306 review): local choices keyed by their place in the
        // list, and the selection kept across redraws by that key, so after
        // "Look again" lists the runtimes in another order the radio kept
        // selected names another spec, which Use this model then sends. No
        // JavaScript runs under cargo test, so this holds the text; a node
        // smoke script with a fake DOM ran the pane (see the commit).
        let local = settings_choice("`local:${spec}`");
        assert!(local.contains("preselect: false,"), "{local}");
        let settings = code_of("settings.js");
        assert!(
            !settings.contains("local-${i}"),
            "a local choice is keyed by its place in the list"
        );
        assert!(
            settings.contains("seen.has(spec)"),
            "a spec listed twice is offered twice under one key"
        );
        // A radio's id is its place in this drawing, never the spec, which
        // is a runtime's text and may hold what an id may not.
        assert!(
            code_of("model.js").contains("input.id = `model-choice-${k}`;"),
            "model.js builds an id from the choice"
        );
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

    /// `style.css` with its comments removed, as `(selector, declarations)`
    /// for each innermost rule, in order; an `@media` block's own rules
    /// are listed, not the block.
    fn css_rules() -> Vec<(String, String)> {
        let css = ui_file("style.css");
        let mut text = String::new();
        let mut rest = css.as_str();
        while let Some(i) = rest.find("/*") {
            text.push_str(&rest[..i]);
            let end = rest[i..].find("*/").expect("an unclosed CSS comment");
            rest = &rest[i + end + 2..];
        }
        text.push_str(rest);
        let mut open: Vec<String> = Vec::new();
        let mut segment = String::new();
        let mut rules = Vec::new();
        for c in text.chars() {
            match c {
                '{' => {
                    open.push(segment.trim().to_string());
                    segment.clear();
                }
                '}' => {
                    let selector = open.pop().expect("an unbalanced }");
                    if !segment.trim().is_empty() {
                        rules.push((selector, segment.trim().to_string()));
                    }
                    segment.clear();
                }
                _ => segment.push(c),
            }
        }
        assert!(open.is_empty(), "an unclosed CSS block");
        rules
    }

    #[test]
    fn page_hidden_attribute_hides_whatever_class_sets_display() {
        // The risk (#302 review): the page shows and hides by the `hidden`
        // attribute, never a style (main.js, dialogs.js, composer.js), but
        // the browser's `[hidden] { display: none }` is a user-agent rule
        // that any author rule setting `display` beats: `.alert { display:
        // flex }` drew the empty red banner and its Dismiss button on every
        // load. One author rule, `!important`, keeps `hidden` hidden for
        // every class, and no other rule may set `display` as important.
        let rules = css_rules();
        assert!(
            rules.iter().any(|(_, d)| d.contains("display")),
            "style.css was read: no display rule at all"
        );
        let hidden: Vec<&(String, String)> = rules
            .iter()
            .filter(|(s, _)| s.split(',').any(|s| s.trim() == "[hidden]"))
            .collect();
        assert_eq!(hidden.len(), 1, "one [hidden] rule: {hidden:?}");
        let decls: String = hidden[0].1.split_whitespace().collect();
        assert!(
            decls.contains("display:none!important"),
            "[hidden] {{ {} }} does not beat a class's display",
            hidden[0].1
        );
        for (s, d) in &rules {
            if s.split(',').any(|s| s.trim() == "[hidden]") {
                continue;
            }
            let d: String = d.split_whitespace().collect();
            assert!(
                !d.contains("!important"),
                "{s} {{ {d} }} is important, and could beat [hidden]"
            );
        }
        // The banner the review found is one such element: hidden as served,
        // with a class whose rule sets display.
        let page = page();
        let alert = &page[index_of(&page, "alert")];
        assert!(
            alert.attrs.contains_key("hidden"),
            "the banner is hidden as served"
        );
        assert!(
            rules
                .iter()
                .any(|(s, d)| s.trim() == ".alert" && d.contains("display")),
            "the case this holds is gone: .alert sets no display"
        );
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
