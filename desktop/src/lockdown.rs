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

    /// The seven sections the rail switches, in rail order (#300 as amended
    /// 2026-10-07; overview is the landing section). The seventh, atlas,
    /// sits after codebase: the owner asked on 2026-10-11 for the Atlas
    /// "without removing the other ones", so it is added beside them and
    /// every other section keeps its place.
    const SECTIONS: [&str; 7] = [
        "overview", "run", "trails", "codebase", "atlas", "board", "vault",
    ];

    /// The sections that are built, and so do not read "Not built yet":
    /// run (#300), overview (#562), codebase (#307), trails (#304), vault
    /// (#309), board (#311) and atlas (the owner's request of 2026-10-11).
    const BUILT: [&str; 7] = [
        "run", "overview", "codebase", "trails", "vault", "board", "atlas",
    ];

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
        // section that says it is not built. Seven sections (#300 as amended
        // 2026-10-07, and the Atlas the owner asked for on 2026-10-11),
        // overview first and the one shown at load; the ones in BUILT are
        // built.
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
    fn approval_dialog_keys_say_escape_rejects() {
        // The risk (#658): Escape, often pressed to mean "later", rejects
        // the call without the dialog saying so. dialogs.js answers every
        // close but Approve with a no (ADR 0018), and the dialog now says
        // that in text it is described by.
        let page = page();
        let dialog = index_of(&page, "approval");
        let keys = index_of(&page, "approval-keys");
        assert!(
            within(&page, keys, dialog),
            "the keys line is outside the dialog"
        );
        let said = page[keys].text.trim();
        assert!(
            said.contains("Escape") && said.contains("rejects the call"),
            "{said}"
        );
        let described = page[dialog]
            .attr("aria-describedby")
            .expect("the dialog has no description");
        assert!(
            described.split_whitespace().any(|id| id == "approval-keys"),
            "the dialog is not described by its keys: {described}"
        );
        assert!(
            code_of("dialogs.js").contains(r#"const yes = dialog.returnValue === "approve";"#),
            "dialogs.js no longer answers a close with a no, so the line is wrong"
        );
    }

    #[test]
    fn approval_dialog_keys_details_take_focus_and_show_it() {
        // The risk (#658): a diff longer than its box that only a mouse can
        // scroll, or one that takes focus with no name and no outline.
        let page = page();
        let detail = &page[index_of(&page, "approval-detail")];
        assert_eq!(detail.tag, "pre");
        assert_eq!(
            detail.attr("tabindex"),
            Some("0"),
            "the details take no focus"
        );
        assert!(
            detail
                .attr("aria-label")
                .is_some_and(|l| !l.trim().is_empty()),
            "the details have no name"
        );
        let rules = css_rules();
        let squeeze = |d: &str| d.split_whitespace().collect::<String>();
        assert!(
            rules.iter().any(
                |(s, d)| s.trim() == ".approval-detail" && squeeze(d).contains("overflow:auto")
            ),
            "the details do not scroll"
        );
        assert!(
            rules.iter().any(|(s, d)| s
                .split(',')
                .any(|s| s.trim() == ".approval-detail:focus-visible")
                && squeeze(d).contains("outline:2px")),
            "focus on the details draws no outline"
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
                r#"import { askStarters, composerStopped, drawComposer, wireComposer } from "./composer.js";"#
            ),
            "main.js does not take askStarters from composer.js"
        );
        assert!(
            code_of("composer.js").contains("export function askStarters(doc, tauri)"),
            "composer.js does not export askStarters"
        );
    }

    /// The text of `code` from `start` to the first `end` after it.
    fn between<'a>(code: &'a str, start: &str, end: &str) -> &'a str {
        code.split(start)
            .nth(1)
            .and_then(|r| r.split(end).next())
            .unwrap_or_else(|| panic!("no `{start}`"))
    }

    /// The listener `code` registers for `event`, up to its closing `});`.
    fn listener<'a>(code: &'a str, event: &str) -> &'a str {
        between(code, &format!("listen(\"{event}\""), "});")
    }

    #[test]
    fn no_child_wording_composer_names_the_missing_child_before_the_model() {
        // The risk (#638): with no trellis rpc child (launched outside a
        // repository, or after the child exited) the composer said "choose
        // a model first", blaming the model for a session that does not
        // exist, and kept saying it after a new child started. No
        // JavaScript runs under cargo test, so this holds the text; a node
        // smoke script with a fake DOM ran the module (see the commit).
        let js = code_of("composer.js");
        for event in ["no-child", "rpc-exited"] {
            assert!(
                listener(&js, event).contains("composerStopped(doc, "),
                "composer.js does not take {event} as not running"
            );
        }
        assert!(
            listener(&js, "rpc-exited").contains("exitText(event.payload)"),
            "the exit is not worded through status.js's exitText"
        );
        assert!(js.contains(r#"import { exitText } from "./status.js";"#));
        assert!(
            listener(&js, "child-started").contains("stopped = null;"),
            "a new child leaves the composer saying none runs"
        );
        let stopped = function_body(&js, "composerStopped");
        assert!(
            stopped.contains("trellis rpc is not running: ${why}"),
            "{stopped}"
        );
        // The label says the child is missing before it says anything about
        // a model, and Enter in a control a page bug left open is refused
        // for the same reason first.
        let draw = function_body(&js, "drawRunning");
        let not_running = draw
            .find("\"Task (trellis rpc is not running)\"")
            .expect("drawRunning has a not-running label");
        let gate = draw
            .find("\"Task (choose a model first)\"")
            .expect("drawRunning keeps the model gate's label");
        assert!(
            not_running < gate,
            "the model gate is checked first: {draw}"
        );
        assert!(draw.contains("stopped !== null"), "{draw}");
        let submit = between(&js, "addEventListener(\"submit\"", "});");
        let refused = submit
            .find("if (stopped !== null)")
            .expect("submit checks the child");
        let model = submit
            .find("if (!modelChosen)")
            .expect("submit checks the model");
        assert!(refused < model, "submit blames the model first: {submit}");
    }

    #[test]
    fn no_child_wording_model_pane_stops_asking_once_no_child_runs() {
        // The risk (#638): the model pane said "Asking trellis which models
        // there are." for ever when no child existed to answer, and opened
        // itself after an exit to ask for a choice nobody could take.
        let js = code_of("model.js");
        for event in ["no-child", "rpc-exited"] {
            assert!(
                listener(&js, event).contains("modelPaneStopped(doc, "),
                "model.js does not take {event} as not running"
            );
        }
        assert!(listener(&js, "rpc-exited").contains("exitText(event.payload)"));
        assert!(js.contains(r#"import { exitText } from "./status.js";"#));
        let started = listener(&js, "child-started");
        for want in ["stopped = null;", "askModels(doc, tauri)"] {
            assert!(started.contains(want), "child-started lacks {want}");
        }
        let found = function_body(&js, "drawFound");
        let not_running = found
            .find("trellis rpc is not running")
            .expect("drawFound has a not-running line");
        let asking = found
            .find("Asking trellis which models there are.")
            .expect("drawFound keeps its asking line");
        assert!(
            not_running < asking,
            "the pane says it is asking first: {found}"
        );
        let draw = function_body(&js, "drawModel");
        let branch = between(draw, "if (!chosen && stopped !== null) {", "\n  }");
        assert!(
            branch.contains("trellis rpc is not running: ${stopped}"),
            "{branch}"
        );
        assert!(branch.contains("return;"), "{branch}");
        assert!(
            !branch.contains("pane.open"),
            "the pane opens with no child: {branch}"
        );
        assert!(
            draw.find("if (!chosen && stopped !== null)") < draw.find("p.pane.open = true"),
            "the pane opens before the child is checked: {draw}"
        );
    }

    #[test]
    fn no_child_wording_main_js_words_the_exit_and_writes_no_raw_json() {
        // The risk (#638): `#status` read `trellis rpc exited:
        // {"code":null,"success":false}`, the core's record shown as JSON
        // rather than said.
        let main = code_of("main.js");
        assert!(
            !main.contains("json(event.payload)"),
            "main.js writes an event's payload as JSON"
        );
        for line in main.lines().filter(|l| l.contains("show(")) {
            assert!(!line.contains("json("), "main.js shows JSON: {line}");
        }
        let exited = listener(&main, "rpc-exited");
        assert!(
            exited.contains("show(\"status\", `trellis rpc ${exitText(event.payload)}`)"),
            "{exited}"
        );
        assert!(
            main.contains(r#"import { drawStatus, exitText, wireStatus } from "./status.js";"#)
        );
    }

    #[test]
    fn no_child_wording_overview_keeps_open_for_the_root_after_an_exit() {
        // The risk (#638): the obvious restart, Open on the repository whose
        // child exited, stayed disabled as "Open now", and a root missing
        // from the recent list had no way to start again but typing its
        // path.
        let js = code_of("overview.js");
        let recent = function_body(&js, "recent");
        assert!(
            recent.contains("const live = path === open && childRuns;"),
            "{recent}"
        );
        assert!(recent.contains("b.disabled = live;"), "{recent}");
        assert!(!recent.contains("b.disabled = path === open"), "{recent}");
        let root = function_body(&js, "rootLine");
        let again = between(root, "if (root !== null && !childRuns) {", "\n  }");
        for want in ["\"Start again\"", "actions.open(root)"] {
            assert!(again.contains(want), "rootLine lacks {want}: {again}");
        }
        let child = function_body(&js, "drawOverviewChild");
        for want in [
            "childRuns = runs;",
            "drawOverview(doc, lastOverview, lastActions)",
        ] {
            assert!(child.contains(want), "drawOverviewChild lacks {want}");
        }
        let main = code_of("main.js");
        for (event, runs) in [
            ("child-started", "true"),
            ("no-child", "false"),
            ("rpc-exited", "false"),
        ] {
            let want = format!("drawOverviewChild(document, {runs});");
            assert!(
                listener(&main, event).contains(&want),
                "{event} lacks {want}"
            );
        }
    }

    #[test]
    fn no_child_wording_late_page_takes_the_core_s_refusal_as_not_running() {
        // The risk (#638): the core emits `no-child` and `rpc-exited` once,
        // so a page that loaded after them (a reload, or listeners still
        // registering when startup ran) heard neither and kept asking for a
        // model. Its `get_state` refusal is the same fact, taken only when
        // it is the core's no-child refusal and no child started since it
        // was asked, so a refusal that crossed a new child's start does not
        // mark that child stopped.
        let main = code_of("main.js");
        let prefix = between(&main, "const NO_CHILD = \"", "\";");
        assert!(
            include_str!("commands.rs").contains(&format!("\"{prefix}{{}}\"")),
            "main.js's NO_CHILD is not how rpc_send's no-child refusal begins: {prefix}"
        );
        let ask = function_body(&main, "askState");
        assert!(ask.contains("const heard = childrenStarted;"), "{ask}");
        let taken = between(
            ask,
            "if (said.startsWith(NO_CHILD) && childrenStarted === heard) {",
            "\n    }",
        );
        for want in [
            "composerStopped(document, why);",
            "modelPaneStopped(document, why);",
            "drawOverviewChild(document, false);",
        ] {
            assert!(taken.contains(want), "the refusal path lacks {want}");
        }
        assert!(listener(&main, "child-started").contains("childrenStarted += 1;"));
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
        let mut written = render_writes(render);
        // The actions log's jump button (#643) is no pane: render.js's
        // `transcript` shows and hides it, which the log_scroll_ tests hold,
        // so it counts as written here rather than as a box nothing fills.
        assert!(
            function_body(render, "transcript").contains(r#"getElementById("log-jump")"#),
            "transcript no longer writes the jump button"
        );
        written.insert("log-jump".to_string());
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
        let mut written: BTreeSet<String> =
            table(js, "PANES").into_iter().map(|(id, _)| id).collect();
        // The open-repository form sits beside Recent repositories (#645);
        // it is settings.js's, which looks up each of its ids, so they are
        // written here rather than boxes nothing fills. The repo_switch_
        // tests hold where the form sits and that nothing else is in it.
        let switch = function_body(&ui["settings.js"], "wireRepositorySwitch");
        for id in ["repo-form", "repo-path", "repo-note"] {
            assert!(
                switch.contains(&format!("getElementById(\"{id}\")")),
                "settings.js no longer wires #{id}"
            );
            written.insert(id.to_string());
        }
        // The Open refusal's line (#659) is overview.js's too: recent()
        // makes it with each list and drawOpenRefusal writes into it. The
        // overview_placeholders_ tests hold both.
        assert!(
            function_body(js, "drawOpenRefusal").contains("getElementById(\"ov-recent-status\")"),
            "overview.js no longer writes the Open refusal into #ov-recent-status"
        );
        written.insert("ov-recent-status".to_string());
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
            "cb-modules",
            "cb-modules-title",
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
    fn codebase_list_regions_and_circles_leave_the_tab_order() {
        // The risk (#648): every module circle and every region a tab
        // stop, so a package of hundreds of modules is hundreds of stops
        // between the graph and the rest of the page, each named only by a
        // tooltip. The graph itself stays one stop, for pan and zoom.
        let js = code_of("codebase.js");
        assert!(
            !js.contains("tabindex: 0"),
            "codebase.js puts a drawn element in the tab order"
        );
        assert!(
            function_body(&js, "drawRegions").contains(
                r#"make(doc, "g", { class: "cb-region", tabindex: -1, role: "button" })"#
            ),
            "a region is in the tab order"
        );
        let package = function_body(&js, "drawPackage");
        let circle = package
            .split(r#"make(doc, "circle", {"#)
            .nth(1)
            .and_then(|r| r.split("});").next())
            .expect("drawPackage makes a circle per module");
        assert!(
            circle.contains("tabindex: -1,"),
            "a module circle is in the tab order: {circle}"
        );
        let page = page();
        assert_eq!(
            page[index_of(&page, "cb-graph")].attr("tabindex"),
            Some("0")
        );
    }

    #[test]
    fn codebase_list_does_what_the_graph_does_and_marks_the_same_selection() {
        // The risk (#648): a list that opens or selects something other
        // than what its region or circle does, a selection marked in the
        // graph but not the list (or the other way), figures joined into
        // one number, or a list left over from the last repository.
        let js = code_of("codebase.js");
        let list = function_body(&js, "drawList");
        for want in [
            "() => enter(doc, ctx, i, null)",
            "() => select(doc, ctx, ctx.pkg, k)",
            "ctx.items[k] = b;",
            "markSelected(ctx);",
            r#"`Modules in ${str(p.package) ?? "this package"}`"#,
            "`edges in ${edgesIn(ctx, m)} · edges out ${figure(m && m.edges_out)} · `",
            "`doubt ${figure(m && m.doubt)}`",
        ] {
            assert!(list.contains(want), "drawList lacks {want}");
        }
        assert!(
            function_body(&js, "drawRegions")
                .contains("activate(g, ctx, () => enter(doc, ctx, i, null));"),
            "a region opens something else than its list entry"
        );
        assert!(
            function_body(&js, "drawPackage")
                .contains("activate(c, ctx, () => select(doc, ctx, ctx.pkg, k));"),
            "a circle selects something else than its list entry"
        );
        let mark = function_body(&js, "markSelected");
        assert!(
            mark.contains(r#"if (b) b.setAttribute("aria-pressed", on(k) ? "true" : "false");"#),
            "the list does not follow the selection: {mark}"
        );
        // The details and the list say edges in the same way.
        assert!(function_body(&js, "drawDetails").contains(r#"row("edges in", edgesIn(ctx, m));"#));
        for f in ["draw", "clear"] {
            assert!(
                function_body(&js, f).contains("drawList(doc, ctx);"),
                "{f} leaves the list as it was"
            );
        }
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
    fn trails_scan_rows_carry_the_start_time() {
        // The risk (#649, UX-15): a user looking for a particular run has
        // only the task text to go on, since the list drew no time; the
        // trail's `started_at` is drawn per row, through `recorded` so it
        // stays marked as untrusted text and a missing one says so.
        let js = ui_file("trails.js");
        let list = function_body(&js, "drawList");
        assert!(
            list.contains("recorded(doc, \"span\", r.started_at, \"start not recorded\")"),
            "drawList draws no started_at per row, or draws it bare"
        );
        let detail = function_body(&js, "drawDetail");
        assert!(
            detail.contains("r.started_at"),
            "the detail dropped the start time"
        );
    }

    #[test]
    fn trails_scan_altered_integrity_takes_the_failed_class() {
        // The risk: 51 of 52 demo trails are "altered" and read as muted
        // text identical to the intact one. Integrity is the one bare string
        // in a row, computed by trellis from the digest (trails.rs), so it
        // alone may take the failed colour; no `{untrusted}` text gets a
        // class the page reads as a verdict.
        let js = ui_file("trails.js");
        let list = function_body(&js, "drawList");
        assert!(
            list.contains("if (r.integrity === \"altered\") integrity.className = \"failed\";"),
            "an altered row's integrity element does not take the failed class"
        );
        for untrusted in ["r.task", "r.outcome", "r.label", "r.started_at"] {
            let draws = between(list, &format!("recorded(doc, \"span\", {untrusted}"), ")");
            assert!(
                !draws.contains("failed"),
                "{untrusted} is untrusted text and may not carry the failed class"
            );
        }
        let css = css_rules();
        assert!(
            css.iter()
                .any(|(s, d)| s.trim() == ".failed" && d.contains("var(--failed)")),
            "style.css has no .failed rule in the failed colour"
        );
    }

    #[test]
    fn trails_scan_filter_matches_outcome_integrity_and_label_too() {
        // The risk: the filter reads the task alone, so "altered" or
        // "not_converged" typed into it hides every run. It reads the four
        // fields, with the label above it saying so.
        let js = ui_file("trails.js");
        let body = function_body(&js, "matches");
        for field in ["row.task", "row.outcome", "row.label", "row.integrity"] {
            assert!(body.contains(field), "matches() does not read {field}");
        }
        let page = page();
        let label = page
            .iter()
            .find(|e| e.tag == "label" && e.attr("for") == Some("tr-filter"))
            .expect("the filter's label");
        for word in ["task", "outcome", "integrity", "label"] {
            assert!(
                label.text.contains(word),
                "the filter's label does not say it matches {word}: {}",
                label.text
            );
        }
    }

    /// A `log --json` line for one scripted run that ran no check, as
    /// trellis words it there: `outcome` is the trail's word.
    fn trail_line_no_check(run_id: &str) -> String {
        json!({
            "run_id": run_id,
            "integrity": "intact",
            "outcome": "no check ran",
            "trail": {
                "schema": 1,
                "run_id": run_id,
                "task": "rename f to g",
                "base_commit": "abc1234",
                "started_at": "2026-10-10T12:00:00Z",
                "provenance": {"trellis_version": "0.1.0", "model": "scripted:x", "tier": null},
                "factual": {
                    "proposed": [],
                    "radius": {"reached_resolved": 0, "reached_ambiguous": 0,
                               "widest_unbound_in_touched_files": 0, "radius_queries": 0},
                    "checks": [],
                    "check_outcomes": [],
                    "approved_by": "Human",
                    "mode_counts": {"verified": 0, "reviewed": 0},
                    "actions": []
                },
                "factual_sha256": "00ff",
                "judgment": {"tech_debt_projection": "none", "rationale": "none"}
            }
        })
        .to_string()
    }

    #[test]
    fn outcome_vocabulary_one_run_reads_the_same_in_all_three_panes() {
        // The risk (#653, UX-27): after one scripted run the Run pane said
        // `not_converged` from rpc's run_end, while the Trails list and the
        // Overview said "no check ran" from the trail, so one run read as
        // two. Every pane's outcome word comes through
        // `state::outcome_words`: the reducer's view, the trail row, and
        // the overview's newest trail all say the same thing for this run.
        let run_id = "20261010-120000-000";
        let mut s = DesktopState::default();
        s.update(parse_stdout(
            &json!({"type": "run_start", "version": 1, "trellis": "0.1.0",
                    "task": "rename f to g", "model": "scripted:x", "root": "/w/app",
                    "id": "p1"})
            .to_string(),
        ));
        s.update(parse_stdout(
            &json!({"type": "run_end", "outcome": "not_converged", "exit": 1,
                    "restored": true, "run_id": run_id, "id": "p1"})
            .to_string(),
        ));
        let run_pane = s.view(Changed::everything())["outcome"]["outcome"].clone();

        let trail_row = crate::trails::row(&trail_line_no_check(run_id));
        assert!(trail_row.get("bad_line").is_none(), "{trail_row}");
        let trails_pane = trail_row["outcome"]["untrusted"].clone();

        let overview = crate::state::word_overview(json!({
            "root": "/w/app",
            "last_run": {"run_id": run_id, "started_at": "2026-10-10T12:00:00Z",
                         "outcome": "no check ran", "integrity": "intact",
                         "task": "rename f to g", "turns": null, "edits": 0,
                         "checks": [], "verified": 0, "reviewed": 0, "tier": null}
        }));
        let overview_pane = overview["last_run"]["outcome"].clone();

        assert_eq!(run_pane, json!("not converged"), "the Run pane's word");
        assert_eq!(
            trails_pane, run_pane,
            "the Trails list's word differs from the Run pane's"
        );
        assert_eq!(
            overview_pane, run_pane,
            "the Overview's word differs from the Run pane's"
        );
        // The trail's word stays the trail's text: mapping it does not
        // promote it out of the untrusted wrapper (ADR 0007).
        assert_eq!(trail_row["outcome"], json!({"untrusted": "not converged"}));
    }

    #[test]
    fn outcome_vocabulary_maps_every_word_trellis_writes_and_invents_none() {
        // The risk: a word one side writes that the mapping does not know,
        // so two panes drift apart again; or the mapping rewording a word
        // it does not know into a guess. rpc's words are output.rs's run_end
        // arms; the trail's are query.rs's Entry::outcome arms.
        let rpc = ["converged", "not_converged", "interrupted", "error"];
        let trail = ["converged", "not converged", "no check ran"];
        let shown = ["converged", "not converged", "interrupted", "error"];
        for w in rpc.iter().chain(trail.iter()) {
            let out = crate::state::outcome_words(w);
            assert!(
                shown.contains(&out),
                "{w} maps to {out}, a word no pane lists"
            );
        }
        // The trail's own words all land in rpc's vocabulary, so a trail
        // and its run_end never disagree on one of these.
        for w in trail {
            assert!(
                rpc.map(crate::state::outcome_words)
                    .contains(&crate::state::outcome_words(w)),
                "the trail's {w} has no rpc word"
            );
        }
        // An unknown word is trellis's to word, not the desktop's.
        assert_eq!(
            crate::state::outcome_words("some new word"),
            "some new word"
        );
    }

    #[test]
    fn outcome_vocabulary_not_measured_only_where_the_trail_has_no_figure() {
        // The risk: "turns: not measured" beside a Run pane that counted
        // iterations, read as a contradiction. The two are different facts:
        // the budget's live iteration count and the trail's measured turns.
        // The overview draws the trail's figure where it has one and says
        // "not measured" only for a null, and the mapping never fills it in.
        let overview = overview_code();
        let turns = between(&overview, "[\"turns\",", "]");
        assert!(
            turns.contains("isCount(t.turns) ? String(t.turns) : \"not measured\""),
            "the turns row does not read the trail's figure: {turns}"
        );
        assert_eq!(
            overview.matches("\"not measured\"").count(),
            1,
            "\"not measured\" is said somewhere other than the turns row"
        );
        for file in ["render.js", "trails.js"] {
            assert!(
                !ui_file(file).contains("not measured"),
                "{file} says \"not measured\" for a figure that is not the trail's turns"
            );
        }
        let with = crate::state::word_overview(
            json!({"last_run": {"outcome": "no check ran", "turns": 3}}),
        );
        assert_eq!(
            with["last_run"]["turns"], 3,
            "a measured figure must survive the mapping"
        );
        let without = crate::state::word_overview(
            json!({"last_run": {"outcome": "no check ran", "turns": null}}),
        );
        assert_eq!(
            without["last_run"]["turns"],
            Value::Null,
            "an unmeasured figure stays null, never 0"
        );
        let none = crate::state::word_overview(json!({"last_run": null}));
        assert_eq!(none["last_run"], Value::Null, "no run stays no run");
    }

    /// The one `type="submit"` button inside the form `form_id`, which the
    /// module's `#<form> button[type=submit]` selector reaches.
    fn submit_button_of(page: &[Element], form_id: &str) -> usize {
        let form = index_of(page, form_id);
        let found: Vec<usize> = (0..page.len())
            .filter(|&i| {
                page[i].tag == "button"
                    && page[i].attr("type") == Some("submit")
                    && within(page, i, form)
            })
            .collect();
        assert_eq!(found.len(), 1, "one submit button in #{form_id}: {found:?}");
        found[0]
    }

    #[test]
    fn disabled_while_asked_show_radius_waits_for_its_answer() {
        // The risk (#657, UX-19): each get_radius makes trellis index the
        // repository, and the button took a second click while the first
        // answer was out, so a large repository's wait doubled. The button
        // is disabled as the question is sent, and enabled again where the
        // answer or the refusal is drawn, and when the child that owed the
        // answer is replaced or gone.
        let js = ui_file("radius.js");
        let page = page();
        let button = submit_button_of(&page, "rd-form");
        assert!(
            !page[button].attrs.contains_key("disabled"),
            "Show radius is served disabled, so nothing could ever be asked"
        );
        let toggle = function_body(&js, "submitting");
        assert!(
            toggle.contains("doc.querySelector(\"#rd-form button[type=submit]\").disabled = on;"),
            "submitting() does not set the button's disabled: {toggle}"
        );
        let ask = function_body(&js, "ask");
        let sent = ask
            .find("invoke(\"rpc_send\"")
            .expect("ask sends the question");
        let off = ask
            .find("submitting(doc, true);")
            .expect("ask disables the button");
        assert!(
            off < sent,
            "the button is disabled only after the question is sent"
        );
        let refused = between(ask, "catch (err)", "\n  }");
        assert!(
            refused.contains("submitting(doc, false);"),
            "a refused question leaves the button disabled for good"
        );
        let draw = function_body(&js, "drawRadius");
        let stale = draw
            .find("if (payload.id !== waiting) return;")
            .expect("stale answers are dropped");
        let on = draw
            .find("submitting(doc, false);")
            .expect("drawRadius enables the button");
        assert!(
            stale < on,
            "an earlier question's late answer enables the button"
        );
        for event in ["child-started", "rpc-exited"] {
            assert!(
                listener(&js, event).contains("submitting(doc, false)"),
                "{event} leaves a button disabled for an answer that never comes"
            );
        }
    }

    #[test]
    fn disabled_while_asked_vault_search_waits_for_its_answer() {
        // The risk: the same for vault_search, whose answer comes back as
        // the `vault` key of a view event: the button is enabled again only
        // by a view that carries hits or an error, not by the empty view
        // drawn at start, and by a refusal from the core.
        let js = ui_file("vault.js");
        let page = page();
        let button = submit_button_of(&page, "vt-form");
        assert!(
            !page[button].attrs.contains_key("disabled"),
            "Search is served disabled, so nothing could ever be asked"
        );
        let toggle = function_body(&js, "searching");
        assert!(
            toggle.contains("doc.querySelector(\"#vt-form button[type=submit]\").disabled = on;"),
            "searching() does not set the button's disabled: {toggle}"
        );
        let submit = between(&js, "addEventListener(\"submit\"", "\n  });");
        let sent = submit
            .find("type: \"vault_search\"")
            .expect("submit sends the search");
        let off = submit
            .find("searching(doc, true);")
            .expect("submit disables the button");
        assert!(
            off < sent,
            "the button is disabled only after the search is sent"
        );
        assert!(
            submit.contains("if (!sent) searching(doc, false);"),
            "a refused search leaves the button disabled for good"
        );
        let send = function_body(&js, "send");
        assert!(
            send.contains("return true;")
                && between(send, "catch (err)", "\n  }").contains("return false;"),
            "send() does not say whether the line went: {send}"
        );
        let draw = function_body(&js, "drawVault");
        assert!(
            draw.contains(
                "if (str(v.error) !== null || Array.isArray(v.hits)) searching(doc, false);"
            ),
            "drawVault enables the button on something other than hits or an error: {draw}"
        );
        for event in ["child-started", "rpc-exited"] {
            assert!(
                listener(&js, event).contains("searching(doc, false)"),
                "{event} leaves a button disabled for an answer that never comes"
            );
        }
    }

    #[test]
    fn overview_placeholders_every_pane_is_served_with_a_reading_state() {
        // The risk (#659, UX-21): the landing view's four panes shipped
        // empty, so during the gh-bounded overview read (up to 20 s) the
        // page showed four blank boxes under one "Reading" line. Each pane
        // container is served with a child that says what it is reading;
        // drawOverview's replaceChildren takes it away with the answer.
        let page = page();
        let section = index_of(&page, "overview");
        let mut panes = 0;
        for id in ["ov-recent", "ov-last-run", "ov-codebase", "ov-board"] {
            let pane = index_of(&page, id);
            assert!(within(&page, pane, section), "#{id} is outside #overview");
            let placeholder = page
                .iter()
                .enumerate()
                .find(|(i, e)| {
                    e.parent == Some(pane) && e.attr("class") == Some("empty") && *i > pane
                })
                .map(|(_, e)| e);
            let Some(p) = placeholder else {
                panic!("#{id} is served with no .empty placeholder child");
            };
            assert!(
                p.text.trim().starts_with("Reading"),
                "#{id}'s placeholder does not say it is reading: {:?}",
                p.text
            );
            panes += 1;
        }
        assert_eq!(panes, 4);
        // The overview line above them still says so as served.
        assert!(
            page[index_of(&page, "ov-root")].text.contains("Reading"),
            "#ov-root no longer says the overview is being read"
        );
        // Every pane is drawn through replaceChildren, so a placeholder
        // cannot outlive the answer; `only` and the per-pane drawers use it.
        let js = overview_code();
        for pane in ["recent", "lastRun", "codebase", "board"] {
            let body = function_body(&js, pane);
            assert!(
                body.contains("replaceChildren(") || body.contains("only(doc, pane"),
                "{pane}() appends beside the placeholder instead of replacing it"
            );
        }
    }

    #[test]
    fn overview_placeholders_open_refusal_is_written_beside_the_button() {
        // The risk: a refused "Open" (a run in flight, no repository at the
        // path) was written to #status at the top of the page, far from the
        // click and not announced. It goes into a role=status line inside
        // #ov-recent, served with the page and made anew by each drawing of
        // the list, so a refusal does not outlive the list it was about.
        let page = page();
        let recent = index_of(&page, "ov-recent");
        let line = index_of(&page, "ov-recent-status");
        assert_eq!(
            page[line].parent,
            Some(recent),
            "#ov-recent-status is not a row of #ov-recent"
        );
        assert_eq!(
            page[line].attr("role"),
            Some("status"),
            "the refusal line is not announced"
        );
        assert_eq!(
            page[line].text.trim(),
            "",
            "the refusal line is served with text"
        );

        let js = overview_code();
        let make = function_body(&js, "refusalLine");
        for want in [
            "line.id = \"ov-recent-status\";",
            "line.setAttribute(\"role\", \"status\");",
        ] {
            assert!(make.contains(want), "refusalLine() lacks {want}");
        }
        let recent_fn = function_body(&js, "recent");
        assert_eq!(
            recent_fn.matches("pane.append(refusalLine(doc));").count(),
            2,
            "recent() must put the line back on both its paths (an unreadable list, and the rows)"
        );
        let write = function_body(&js, "drawOpenRefusal");
        assert!(
            write.contains("doc.getElementById(\"ov-recent-status\").textContent = text;"),
            "drawOpenRefusal does not write through textContent: {write}"
        );

        let main = ui_file("main.js");
        let actions = function_body(&main, "overviewActions");
        let open = between(actions, "open_repository", "\n    }");
        assert!(
            open.contains("catch((err) => drawOpenRefusal(document, String(err)))"),
            "an Open refusal is not drawn in the Recent repositories pane: {open}"
        );
        assert!(
            !open.contains("show(\"status\""),
            "an Open refusal still goes to the page's top status line"
        );
    }

    #[test]
    fn vault_section_holds_what_vault_js_draws() {
        // The risk (#309): vault.js drawing into an id the page lacks, so
        // a search trellis answered is never shown; a search box with no
        // label; or the section still saying it is not built.
        let page = page();
        let section = index_of(&page, "vault");
        let js = ui_file("vault.js");
        for id in [
            "vt-status",
            "vt-form",
            "vt-query",
            "vt-hits",
            "vt-note",
            "vt-check",
        ] {
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
        // Every type it sends is one of the three reads.
        let sent: Vec<&str> = js
            .split("type: \"")
            .skip(1)
            .map(|rest| &rest[..rest.find('"').unwrap()])
            .collect();
        assert_eq!(
            sent,
            ["vault_graph", "vault_check", "vault_search", "vault_search"],
            "{sent:?}"
        );
    }

    #[test]
    fn vault_browse_lists_the_notes_before_a_query_and_draws_no_score() {
        // The risk (#629): the page refusing an empty query, so the vault
        // view is blank until a word is typed, when Apex lists the notes
        // on focus; a browse list drawn with a 0.0000 score beside every
        // note as if ranked; or a status line calling a browse a match.
        let js = ui_file("vault.js");
        let submit = between(&js, "addEventListener(\"submit\"", "\n  });");
        assert!(
            !submit.contains("takes at least one word"),
            "the submit handler still refuses an empty query"
        );
        let browse = function_body(&js, "browse");
        assert!(
            browse.contains("type: \"vault_search\"") && browse.contains("query: \"\""),
            "browse() does not send vault_search with an empty query: {browse}"
        );
        let started = between(&js, "listen(\"child-started\"", "\n  });");
        assert!(
            started.contains("browse(doc, tauri);"),
            "the notes are not listed when a child starts"
        );
        let hits = function_body(&js, "drawHits");
        assert!(
            hits.contains("const browse = v.browse === true;") && hits.contains("if (!browse) {"),
            "drawHits draws a score on a browse list: {hits}"
        );
        let draw = function_body(&js, "drawVault");
        let (before, after) = draw
            .split_once("v.browse === true")
            .expect("drawVault reads the browse mark");
        let line = after.lines().next().unwrap_or_default();
        assert!(
            !line.contains("matched"),
            "the browse status line says matched: {line}"
        );
        assert!(
            before.contains("Array.isArray(v.hits)"),
            "the mark is read only with a list to draw"
        );
    }

    #[test]
    fn vault_check_is_drawn_in_the_vault_section_and_never_as_a_check_row() {
        // The risk (#632): the run gate's answer, which is vault
        // validation, drawn into the run section's checks pane as if
        // trellis had reported a verdict (ADR 0004, desktop/CLAUDE.md "What
        // it shows"); or a check pane the page lacks, so the report trellis
        // answered is never shown.
        let page = page();
        let vault = index_of(&page, "vault");
        let run = index_of(&page, "run");
        for id in ["vt-check", "vt-check-ask"] {
            let at = index_of(&page, id);
            assert!(within(&page, at, vault), "#{id} is outside #vault");
            assert!(!within(&page, at, run), "#{id} is inside #run");
        }
        let ui = ui_code();
        let js = &ui["vault.js"];
        for id in ["vt-check", "vt-check-ask"] {
            assert!(js.contains(&format!("\"{id}\"")), "vault.js draws no {id}");
        }
        assert!(
            function_body(js, "drawCheck").contains("getElementById(\"vt-check\")"),
            "drawCheck does not draw into #vt-check"
        );
        assert!(
            !js.contains("pane-checks"),
            "vault.js writes into the run section's checks pane"
        );
        // render.js's checks pane reads the view's `checks` key only, and
        // nothing of the vault.
        let render = &ui["render.js"];
        assert!(!render.contains("vault"), "render.js reads the vault view");
        assert!(
            function_body(render, "checks").contains("result"),
            "render.js's checks pane no longer draws the verdict trellis reported"
        );
    }

    /// `function name(` in `code` to its closing line, with all whitespace
    /// removed, so a check on it does not depend on how a call is wrapped.
    fn squeezed(code: &str, name: &str) -> String {
        let at = code
            .find(&format!("function {name}("))
            .unwrap_or_else(|| panic!("no function {name}"));
        let rest = &code[at..];
        rest[..rest.find("\n}").expect("the function closes")]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect()
    }

    #[test]
    fn vault_relations_with_a_path_ask_for_that_note_s_graph() {
        // The risk (#630): a note's lineage, children and references drawn
        // as text only, so following one meant searching for it by hand; or
        // a relation that opens by some other line than the `vault_graph` a
        // hit sends. No JavaScript runs under cargo test, so this holds the
        // text; a node smoke script with a fake DOM clicked every relation
        // (see the commit).
        let js = code_of("vault.js");
        let opener = squeezed(&js, "opener");
        assert!(
            opener.contains("elseb.addEventListener(\"click\",()=>askNote(doc,tauri,path));"),
            "{opener}"
        );
        assert!(
            squeezed(&js, "askNote").contains("type:\"vault_graph\",note:path"),
            "a relation asks for something other than vault_graph"
        );
        // The lineage's paths go to the opener, and the children and both
        // reference lists through `named`, which does.
        let note = squeezed(&js, "drawNote");
        assert!(
            note.contains("li.append(opener(doc,tauri,str(p),"),
            "the lineage is not opened: {note}"
        );
        for (list, each) in [
            ("g.children", "named"),
            ("g.refs_out", "reference"),
            ("g.refs_in", "reference"),
        ] {
            assert!(
                note.contains(&format!("list({list},{each})")),
                "{list} is not drawn by {each}"
            );
        }
        assert!(note.contains("each(doc,tauri,x)"), "{note}");
        assert!(squeezed(&js, "reference").contains("named(doc,tauri,r)"));
        assert!(squeezed(&js, "named").contains("li.append(opener(doc,tauri,path,"));
    }

    #[test]
    fn vault_relations_without_a_path_cannot_be_activated() {
        // The risk (#630): an entry trellis gave no path for drawn as a
        // live button, sending `vault_graph` with no note, or with the
        // "path not reported" placeholder as one. The opener is disabled
        // and given no listener at all for a null path, and every caller
        // passes the entry's own path, never the text drawn in its place.
        let js = code_of("vault.js");
        let opener = squeezed(&js, "opener");
        let disabled = opener
            .find("if(path===null)b.disabled=true;")
            .expect("a path-less opener is not disabled");
        let listens = opener
            .find("elseb.addEventListener(")
            .expect("the opener listens only in the else branch");
        assert!(disabled < listens, "{opener}");
        assert_eq!(opener.matches("addEventListener").count(), 1, "{opener}");
        let named = squeezed(&js, "named");
        assert!(named.contains("constpath=str(n&&n.path);"), "{named}");
        assert!(named.contains("opener(doc,tauri,path,"), "{named}");
        let note = squeezed(&js, "drawNote");
        assert!(note.contains("opener(doc,tauri,str(p),"), "{note}");
        for squeezed_code in [&named, &note] {
            assert!(
                !squeezed_code.contains("opener(doc,tauri,str(p)??")
                    && !squeezed_code.contains("opener(doc,tauri,path??"),
                "a placeholder reaches the opener as a path"
            );
        }
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
    fn board_filter_cells_press_one_status_and_show_all_lifts_it() {
        // The risk (#656): eight counts that look clickable and are not; or
        // a filter that counts what it shows, so the board draws a figure
        // the core did not send. Each cell is a button with aria-pressed,
        // the list shows the view's tasks whose status is the pressed
        // cell's, and nothing is counted from them (the queue_js_ guards
        // still hold). No JavaScript runs under cargo test, so this holds
        // the text; a node smoke script with a fake DOM pressed the cells
        // (see the commit).
        let js = code_of("queue.js");
        let counts = function_body(&js, "drawCounts");
        for want in [
            r#"const b = el(doc, "button");"#,
            r#"b.setAttribute("aria-pressed", s !== null && s === pressed ? "true" : "false");"#,
            r#"else b.addEventListener("click", () => press(doc, s));"#,
            r#"el(doc, "button", "Show all")"#,
            "b.disabled = pressed === null;",
            r#"b.addEventListener("click", () => press(doc, null));"#,
        ] {
            assert!(counts.contains(want), "drawCounts lacks {want}");
        }
        let press = function_body(&js, "press");
        for want in [
            "pressed = status === pressed ? null : status;",
            "drawCounts(doc, lastQueue.counts);",
            "drawTasks(doc, lastQueue.tasks);",
        ] {
            assert!(press.contains(want), "press lacks {want}");
        }
        let tasks = function_body(&js, "drawTasks");
        assert!(
            tasks.contains(".filter((t) => pressed === null || str(t && t.status) === pressed)"),
            "drawTasks does not honour the pressed status: {tasks}"
        );
        // The filtered rows are drawn, never counted.
        assert_eq!(tasks.matches(".length").count(), 1, "{tasks}");
        assert!(tasks.contains("if (items.length === 0) {"), "{tasks}");
        assert!(
            js.split(r#"listen("child-started""#)
                .nth(1)
                .and_then(|r| r.split("});").next())
                .is_some_and(|s| s.contains("pressed = null;")),
            "another repository's queue keeps the last one's filter"
        );
    }

    #[test]
    fn board_filter_overview_and_board_say_the_same_status_word() {
        // The risk (#656): the same task read "pending" in the overview
        // (the queue directory, `state`) and "todo" on the board (`status`).
        // Both now draw `status`, which the overview's contract entry lists,
        // so a rename fails trellis's own contract test rather than
        // blanking the word.
        let overview_js = code_of("overview.js");
        let overview = function_body(&overview_js, "task");
        assert!(
            overview.contains(r#"el(doc, "span", str(t.status) ?? "?")"#),
            "{overview}"
        );
        assert!(!overview.contains("t.state"), "{overview}");
        assert!(code_of("queue.js").contains(r#"str(t.status) ?? "status not reported""#));
        let contract: Value = serde_json::from_str(include_str!(
            "../../crates/trellis-cli/tests/client_contract.json"
        ))
        .expect("the contract is JSON");
        assert!(
            contract["overview"]["always"]
                .as_array()
                .expect("overview.always is a list")
                .contains(&json!("board.tasks[].status")),
            "the overview's contract does not list the status the desktop reads"
        );
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
            main.contains(r#"import { drawOpenRefusal, drawOverview, drawOverviewChild } from "./overview.js";"#),
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
        // A task's chain and how it reads; its labels and assignee are the
        // queue view's, not the board's. Its status is the word the queue
        // view uses too, so a task reads the same in both (#656).
        keys.extend(
            [
                "id", "status", "priority", "task", "waiting", "blockers", "parent",
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

    /// The ids atlas.js draws into or reads, every one inside #atlas.
    const ATLAS_IDS: [&str; 30] = [
        "at-where",
        "at-open-search",
        "at-status",
        "at-lens",
        "at-win",
        "at-win-field",
        "at-win-since",
        "at-base-field",
        "at-base",
        "at-snap",
        "at-ticks",
        "at-readout",
        "at-back",
        "at-fwd",
        "at-crumbs",
        "at-zout",
        "at-zin",
        "at-zfit",
        "at-map",
        "at-canvas",
        "at-legend",
        "at-minimap",
        "at-tab-sel",
        "at-tab-chg",
        "at-tab-pin",
        "at-detail",
        "at-search",
        "at-q",
        "at-qclose",
        "at-qhint",
    ];

    #[test]
    fn atlas_section_holds_what_atlas_js_draws() {
        // The risk (the owner's request of 2026-10-11): atlas.js drawing into
        // an id the page lacks, so the map, the scrubber or the detail pane
        // never shows; a control with no name; a map the keyboard cannot
        // reach; or the section still saying it is not built.
        let page = page();
        let section = index_of(&page, "atlas");
        let js = code_of("atlas.js");
        for id in ATLAS_IDS {
            // The three tabs are named through one template, `at-tab-${t}`,
            // over their suffixes.
            let tab = id
                .strip_prefix("at-tab-")
                .is_some_and(|t| js.contains("`at-tab-${t}`") && js.contains(&format!("\"{t}\"")));
            assert!(
                js.contains(&format!("\"{id}\"")) || js.contains(&format!("#{id} ")) || tab,
                "atlas.js draws no {id}"
            );
            assert!(
                within(&page, index_of(&page, id), section),
                "#{id} is outside #atlas"
            );
        }
        for id in ["at-snap", "at-base"] {
            assert!(
                page.iter()
                    .any(|e| e.tag == "label" && e.attr("for") == Some(id)),
                "#{id} has no label"
            );
        }
        assert!(page[index_of(&page, "at-q")].attr("aria-label").is_some());
        let canvas = &page[index_of(&page, "at-canvas")];
        assert_eq!(canvas.tag, "canvas");
        assert_eq!(canvas.attr("tabindex"), Some("0"), "the map takes keys");
        assert_eq!(canvas.attr("role"), Some("img"));
        assert!(canvas.attr("aria-label").is_some());
        assert_eq!(canvas.attr("aria-describedby"), Some("at-keys"));
        assert!(
            within(&page, index_of(&page, "at-keys"), section),
            "the map's keys are said in the section"
        );
        for id in ["at-search", "at-minimap", "at-win-field", "at-base-field"] {
            assert!(
                page[index_of(&page, id)].attrs.contains_key("hidden"),
                "#{id} is shown before atlas.js decides"
            );
        }
        assert!(!page[section].text.contains("Not built yet"));
        assert!(
            ui_file("index.html").contains(r#"<script type="module" src="atlas.js"></script>"#),
            "index.html does not load atlas.js as a module"
        );
    }

    #[test]
    fn atlas_js_draws_text_and_asks_only_for_reads() {
        // The risk (ADR 0007, S10; the owner's request of 2026-10-11): a
        // module name, path, tag or commit subject, repository or git text,
        // parsed as markup or drawn as a link; the page asking trellis for
        // anything but the four reads; or a figure drawn as a share, which
        // CLAUDE.md rule 3 forbids. No JavaScript runs under cargo test, so
        // this holds the text; a node smoke script with a fake DOM and a
        // recording canvas ran atlas.js over views the core emitted for the
        // poetry data (see the commit).
        let js = code_of("atlas.js");
        for bad in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "document.write",
            "createElement(\"a\")",
            "href",
            "fetch(",
            "\"prompt\"",
            "%",
            "percent",
            "ratio",
        ] {
            assert!(!js.contains(bad), "atlas.js uses {bad}");
        }
        assert!(js.contains("textContent"), "atlas.js writes no text");
        let sent: BTreeSet<&str> = js
            .split("type: \"")
            .skip(1)
            .map(|rest| &rest[..rest.find('"').unwrap()])
            .collect();
        assert_eq!(
            sent,
            BTreeSet::from(["get_codebase", "get_history", "get_radius", "get_symbols"]),
            "atlas.js sends a type that is not one of its four reads"
        );
        let invoked: Vec<&str> = js
            .split("invoke(\"")
            .skip(1)
            .map(|rest| &rest[..rest.find('"').unwrap()])
            .collect();
        assert_eq!(invoked, ["rpc_send"], "{invoked:?}");
        // Every question carries an Atlas id, so the core keeps its
        // answers apart from the Codebase view's and the radius pane's.
        assert!(js.contains(r#"const ID = "desktop-atlas-";"#));
        assert_eq!(
            js.matches("id: nextId(A, ").count(),
            5,
            "a question with no Atlas id"
        );
        // One index at a time: nothing is asked while a question is in
        // flight here, or in the core's record.
        let pump = function_body(&js, "pump");
        for want in [
            "if (A.inflight !== null) return;",
            "snaps.some((s) => s.state === \"asked\")",
        ] {
            assert!(pump.contains(want), "pump lacks {want}");
        }
    }

    #[test]
    fn atlas_js_reads_colours_from_the_stylesheet_and_follows_the_system_theme() {
        // The risk: a map drawn in colours of its own, which ignore the
        // system theme and the page's palette; or a map that stays light
        // when the system turns dark, since the desktop has no theme toggle.
        let js = code_of("atlas.js");
        let palette = function_body(&js, "readPalette");
        for want in ["getComputedStyle(", "getPropertyValue(`--${t}`)"] {
            assert!(palette.contains(want), "readPalette lacks {want}");
        }
        assert!(
            js.contains(
                r#"win.matchMedia("(prefers-color-scheme: dark)").addEventListener("change""#
            ),
            "the map is not redrawn on a theme change"
        );
        let hex = js.char_indices().any(|(i, c)| {
            c == '#'
                && js[i + 1..]
                    .chars()
                    .take_while(char::is_ascii_hexdigit)
                    .count()
                    >= 3
                && !js[i + 1..].starts_with("at-")
        });
        assert!(!hex, "atlas.js names a colour of its own");
        // Each colour it reads is one style.css sets, in both themes.
        let css = ui_file("style.css");
        let tokens = js
            .split("const TOKENS = [")
            .nth(1)
            .and_then(|r| r.split("];").next())
            .expect("atlas.js lists its colours");
        for t in tokens
            .split(',')
            .map(|t| t.trim().trim_matches('"'))
            .filter(|t| !t.is_empty())
        {
            assert!(
                css.matches(&format!("--{t}:")).count() >= 2,
                "style.css does not set --{t} for both themes"
            );
        }
    }

    #[test]
    fn atlas_js_flies_only_when_motion_is_allowed() {
        // The risk: an animated flight across the map for someone whose
        // system asks for reduced motion; or a stylesheet rule for the
        // Atlas that moves on its own.
        let js = code_of("atlas.js");
        assert!(js.contains(r#"reduced: win.matchMedia("(prefers-reduced-motion: reduce)")"#));
        let go = function_body(&js, "go");
        let jump = go
            .find("A.reduced.matches")
            .expect("go does not ask about motion");
        let animate = go
            .find("requestAnimationFrame(step)")
            .expect("go has no flight");
        assert!(
            jump < animate,
            "a flight starts before reduced motion is asked about"
        );
        assert!(
            go[jump..animate].contains("setT(A, t);"),
            "go does not jump under reduced motion"
        );
        let (rules, moving) = crate::graph::motion_in_rules_naming("at-");
        assert!(rules > 0, "style.css has no rule for the Atlas");
        assert!(moving.is_empty(), "Atlas rules that move: {moving:?}");
    }

    #[test]
    fn atlas_js_works_without_storage_and_forgets_the_last_repository() {
        // The risk: a page where storage is refused failing to draw the map
        // because the working set threw; or, after another repository is
        // opened, the last one's map, selection and questions in flight
        // staying drawn and answerable as the new one's.
        let js = code_of("atlas.js");
        for f in ["loadPins", "savePins"] {
            let body = function_body(&js, f);
            assert!(
                body.contains("try {") && body.contains("localStorage") && body.contains("catch"),
                "{f} does not survive refused storage"
            );
        }
        let started = js
            .split(r#"listen("child-started""#)
            .nth(1)
            .and_then(|r| r.split("});").next())
            .expect("atlas.js listens for child-started");
        assert!(started.contains("reset(A, "), "{started}");
        let reset = function_body(&js, "reset");
        for want in [
            "A.view = null;",
            "A.D = null;",
            "A.sel = null;",
            "A.askedHistory = false;",
            "A.askedSymbols = false;",
            "A.inflight = null;",
            "A.pins = loadPins(A);",
        ] {
            assert!(reset.contains(want), "reset lacks {want}");
        }
    }

    #[test]
    fn atlas_shown_folds_the_model_pane_and_no_other_section_does() {
        // The risk (the end-to-end run on poetry, 2026-10-11): on a first
        // launch with no model chosen, the model pane, open above every
        // section, pushed the whole Atlas below the fold at 1440x900.
        // Showing the Atlas folds it; the user can open it again, and every
        // other section leaves it to model.js, as before.
        let js = code_of("atlas.js");
        let rail = js
            .split(r#"button[aria-controls="atlas"]"#)
            .nth(1)
            .and_then(|r| r.split("\n  }\n").next())
            .expect("atlas.js listens to its rail button");
        assert!(
            rail.contains(r#"getElementById("model-pane")"#) && rail.contains(".open = false"),
            "showing the Atlas leaves the model pane open: {rail}"
        );
        for (name, code) in ui_code() {
            if name != "model.js" && name != "atlas.js" {
                assert!(
                    !code.contains("model-pane"),
                    "{name} touches the model pane"
                );
            }
        }
    }

    #[test]
    fn atlas_wheel_scrolls_the_page_until_the_map_is_focused() {
        // The risk (the end-to-end run at 860x600): the wheel over the map
        // always zoomed it, so the detail pane below the map could not be
        // scrolled to. A plain wheel scrolls the page, and zooms only once
        // the map has focus or with Ctrl or Meta held, which a trackpad's
        // pinch sends; a one-line hint says so when the wheel was let by.
        let js = code_of("atlas.js");
        let zooms = function_body(&js, "wheelZooms");
        for want in [
            "A.doc.activeElement === A.canvas",
            "ev.ctrlKey",
            "ev.metaKey",
        ] {
            assert!(zooms.contains(want), "wheelZooms lacks {want}");
        }
        let on = function_body(&js, "onWheel");
        let gate = on
            .find("if (!wheelZooms(A, ev))")
            .expect("onWheel does not ask");
        let stop = on.find("ev.preventDefault()").expect("onWheel never zooms");
        assert!(gate < stop, "the page's scroll is stopped before asking");
        assert!(on[gate..stop].contains("hint(A);") && on[gate..stop].contains("return;"));
        let page = page();
        let hint = index_of(&page, "at-hint");
        assert!(within(&page, hint, index_of(&page, "atlas")));
        assert!(
            page[hint].attrs.contains_key("hidden"),
            "the hint shows as served"
        );
        assert!(js.contains(r#""at-hint""#), "atlas.js never shows the hint");
        // A click on the map gives it focus without scrolling the page.
        assert!(js.contains("A.canvas.focus({ preventScroll: true })"));
    }

    #[test]
    fn atlas_labels_by_kind_and_draws_a_count_only_with_its_name() {
        // The risk (the end-to-end run): a function holding nested
        // definitions drawn as "class execute()", since being a box was
        // taken for being a class; and a "15 defs" line drawn where its
        // module's name did not fit, a count with nothing to say what it
        // counts.
        let js = code_of("atlas.js");
        let symbols = function_body(&js, "drawSymbols");
        assert!(
            !symbols.contains(r#"s.holds ? "class ""#),
            "a box is called a class"
        );
        assert!(symbols.contains(r#"s.kind === "class""#));
        let place = function_body(&js, "placeLabels");
        assert!(place.contains("if (L.needs && !L.needs.placed) continue;"));
        assert!(place.contains("L.placed = true;"));
        // Each line under a name needs the name.
        for f in ["drawModule", "drawSymbols"] {
            assert!(
                function_body(&js, f).contains("needs: name"),
                "{f}'s count stands alone"
            );
        }
    }

    #[test]
    fn atlas_diff_opens_against_the_oldest_snapshot_over_two_years_of_tags() {
        // The risk (the end-to-end run): the diff opened against the
        // snapshot before the one shown, almost empty; and the history held
        // eight months, since get_history was asked for its default twelve
        // tags.
        let js = code_of("atlas.js");
        assert!(js.contains("const HISTORY_ASKED = { commits: 20, tags: 200 };"));
        let lens = function_body(&js, "setLens");
        assert!(
            !lens.contains("i - 1"),
            "the diff takes the previous snapshot"
        );
        assert!(lens.contains("oldestBase(A)"), "{lens}");
        assert!(function_body(&js, "takeView").contains("oldestBase(A)"));
        assert!(function_body(&js, "oldestBase").contains("A.D.points[0]"));
    }

    #[test]
    fn atlas_names_its_root_and_indexes_search_when_symbols_arrive() {
        // The risk (the end-to-end run): a page that loaded after its child
        // started heard no child-started, so the root crumb read
        // "repository"; and the first search open took over a second while
        // it built its index.
        let js = code_of("atlas.js");
        assert!(!js.contains(r#""repository""#), "a root with no name");
        let frame = js
            .split(r#"listen("rpc-frame""#)
            .nth(1)
            .and_then(|r| r.split("});").next())
            .expect("atlas.js reads get_state's root");
        assert!(frame.contains(r#"f.command === "get_state""#) && frame.contains("setRoot(A, "));
        assert!(function_body(&js, "takeView").contains("indexSearch(A)"));
        assert!(!function_body(&js, "openSearch").contains("indexSearch"));
    }

    #[test]
    fn atlas_head_panel_shows_definitions_and_resolved_sites_and_the_edges_apart() {
        // The risk (the end-to-end run): the HEAD panel showed no
        // definitions, and listed call sites bound across modules as one of
        // three counts beside ambiguous and unresolved, where the resolved
        // sites belong. Each line says what it counts and where it comes
        // from, and nothing draws them as parts of a whole or a share.
        let js = code_of("atlas.js");
        let head = function_body(&js, "overviewView");
        for want in [
            ".definitions",
            ".resolved_call_sites",
            "\"resolved call sites\"",
            "\"derived\"",
            "bound across modules",
        ] {
            assert!(head.contains(want), "the HEAD panel lacks {want}");
        }
        assert!(!head.contains("three counts"), "{head}");
        assert!(!function_body(&js, "moduleView").contains("three counts"));
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
            "atlas.js",
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
                r#"import { askModels, drawModelPane, modelPaneStopped, wireModelPane } from "./model.js";"#
            ),
            "main.js does not load model.js"
        );
    }

    /// The click handler `code` gives `on`, from its `addEventListener("click"`
    /// to its closing `});`.
    fn click_handler<'a>(code: &'a str, on: &str) -> &'a str {
        code.split(&format!("{on}.addEventListener(\"click\""))
            .nth(1)
            .and_then(|r| r.split("});").next())
            .unwrap_or_else(|| panic!("no click handler on {on}"))
    }

    /// composer.js's submit handler, to its closing `});`.
    fn composer_submit(js: &str) -> &str {
        js.split("addEventListener(\"submit\"")
            .nth(1)
            .and_then(|r| r.split("\n  });").next())
            .expect("composer.js has a submit handler")
    }

    #[test]
    fn composer_helper_textarea_is_described_by_what_enter_does() {
        // The risk (#644): a 3-row textarea invites Enter for a new line,
        // but Enter starts a run (a snapshot, model calls, checks and a
        // trail), and that was said only in a code comment. The line is
        // named by the textarea's aria-describedby, so a screen reader
        // says it too, and it describes the key handler composer.js has.
        let page = page();
        let composer = index_of(&page, "composer");
        let text = &page[index_of(&page, "composer-text")];
        let ids = text
            .attr("aria-describedby")
            .expect("#composer-text has no aria-describedby");
        let said: Vec<String> = ids
            .split_whitespace()
            .map(|id| {
                let i = index_of(&page, id);
                assert!(within(&page, i, composer), "#{id} is outside the composer");
                page[i].text.clone()
            })
            .collect();
        assert!(
            said.iter()
                .any(|t| t.contains("Enter sends") && t.contains("Shift+Enter adds a line")),
            "nothing the textarea names says what Enter does: {said:?}"
        );
        let js = code_of("composer.js");
        assert!(
            js.contains(r#"if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {"#)
                && js.contains("p.form.requestSubmit();"),
            "the helper line no longer describes the key handler"
        );
    }

    #[test]
    fn composer_helper_prompt_success_says_sent_and_offers_run() {
        // The risk (#644): after Send the text vanished with no word of
        // what happened. A prompt the core forwarded says so and offers the
        // Run section, through the rail's own Run button; a refusal or a
        // steer leaves that offer hidden.
        let js = code_of("composer.js");
        let submit = composer_submit(&js);
        let sent = submit
            .split("if (await send(tauri, p, line)) {")
            .nth(1)
            .and_then(|r| r.split("\n    }").next())
            .expect("the prompt has a success branch");
        for want in [
            "p.note.textContent = \"Sent to trellis; follow it in Run.\";",
            "p.follow.hidden = false;",
            "p.text.value = \"\";",
        ] {
            assert!(
                sent.contains(want),
                "the prompt success branch lacks {want}: {sent}"
            );
        }
        assert!(
            submit.trim_start().starts_with(
                ", async (e) => {\n    e.preventDefault();\n    p.follow.hidden = true;"
            ),
            "a refused or steered submit leaves the last offer shown: {submit}"
        );
        assert_eq!(js.matches("p.follow.hidden = false;").count(), 1);
        let follow = click_handler(&js, "p.follow");
        assert!(
            follow.contains(r#"doc.querySelector('nav.rail button[aria-controls="run"]')"#)
                && follow.contains("run.click()"),
            "{follow}"
        );
        let page = page();
        let button = index_of(&page, "composer-follow");
        assert!(within(&page, button, index_of(&page, "composer")));
        assert!(page[button].attrs.contains_key("hidden"), "shown as served");
        assert!(page[button].text.contains("Run"), "{}", page[button].text);
        assert!(
            page.iter().any(|e| e.tag == "button"
                && e.attr("aria-controls") == Some("run")
                && page[e.parent.expect("in the rail")].tag == "nav"),
            "the rail has no Run button for it to press"
        );
    }

    #[test]
    fn composer_helper_starter_click_fills_the_template_and_sends_nothing() {
        // The risk (#644): a starter ran on its click, one click from a
        // snapshot and a model call, with its description only in a
        // tooltip a keyboard user never sees. Its click chooses the
        // template, shown with its description, and only Send runs it.
        let js = code_of("composer.js");
        let starters = function_body(&js, "drawStarters");
        let click = click_handler(starters, "b");
        for bad in ["send(", "invoke(", "rpc_send"] {
            assert!(!click.contains(bad), "a starter click sends: {click}");
        }
        for want in ["template = { name, description };", "drawTemplate(doc);"] {
            assert!(click.contains(want), "a starter click lacks {want}");
        }
        assert!(
            !starters.contains("send("),
            "drawStarters sends: {starters}"
        );
        let template = function_body(&js, "drawTemplate");
        for want in [
            "p.template.hidden = template === null;",
            "`Template: ${template.name} — ${template.description}`",
        ] {
            assert!(template.contains(want), "drawTemplate lacks {want}");
        }
        let submit = composer_submit(&js);
        assert!(
            submit.contains("if (template !== null) line.prompt = template.name;"),
            "Send does not run the chosen template: {submit}"
        );
        let page = page();
        let line = index_of(&page, "composer-template");
        assert!(within(&page, line, index_of(&page, "composer")));
        assert!(page[line].attrs.contains_key("hidden"), "shown as served");
        assert!(within(
            &page,
            index_of(&page, "composer-template-text"),
            line
        ));
    }

    #[test]
    fn stop_confirm_dialog_has_named_cancel_and_stop_buttons() {
        // The risk (#637): the confirmation is a dialog whose buttons a
        // screen reader names only as "button", or whose text does not say
        // what Stop does (a rollback nobody can undo), so the confirmation
        // asks for consent to something it never named.
        let page = page();
        let dialog = index_of(&page, "stop-confirm");
        assert_eq!(page[dialog].tag, "dialog");
        let form = (0..page.len())
            .find(|&i| page[i].tag == "form" && page[i].parent == Some(dialog))
            .expect("the dialog holds a form");
        assert_eq!(page[form].attr("method"), Some("dialog"));
        for (id, text, value) in [
            ("stop-confirm-cancel", "Cancel", "cancel"),
            ("stop-confirm-stop", "Stop", "stop"),
        ] {
            let b = index_of(&page, id);
            assert!(within(&page, b, form), "#{id} is outside the dialog's form");
            assert_eq!(page[b].tag, "button");
            assert_eq!(page[b].attr("type"), Some("submit"));
            assert_eq!(page[b].text.trim(), text);
            assert_eq!(page[b].attr("value"), Some(value));
        }
        let title = index_of(&page, "stop-confirm-title");
        assert_eq!(
            page[dialog].attr("aria-labelledby"),
            Some("stop-confirm-title")
        );
        assert_eq!(page[title].text.trim(), "Stop the run?");
        let said = &page[dialog].text;
        assert!(
            said.contains("rolls back its edits") && said.contains("writes a trail"),
            "the dialog does not say what Stop does: {said}"
        );
        let stop = index_of(&page, "composer-stop");
        assert_eq!(page[stop].attr("aria-controls"), Some("stop-confirm"));
        assert_eq!(page[stop].attr("aria-haspopup"), Some("dialog"));
    }

    #[test]
    fn stop_confirm_stop_is_set_apart_and_uses_the_danger_token() {
        // The risk (#637): Stop drawn as Send's twin, 8 px away, so the
        // irreversible action is one slip from the routine one. It carries
        // the danger class, a rule for that class uses the failed token, and
        // a rule pushes it to the far end of its row.
        let page = page();
        let stop = &page[index_of(&page, "composer-stop")];
        let classes: Vec<&str> = stop
            .attr("class")
            .unwrap_or("")
            .split_whitespace()
            .collect();
        assert!(classes.contains(&"danger"), "#composer-stop lacks .danger");
        assert!(
            page[index_of(&page, "stop-confirm-stop")]
                .attr("class")
                .is_some_and(|c| c.split_whitespace().any(|c| c == "danger")),
            "the dialog's Stop lacks .danger"
        );
        let rules = css_rules();
        let danger: Vec<&(String, String)> = rules
            .iter()
            .filter(|(s, _)| s.split(',').any(|s| s.trim() == ".composer button.danger"))
            .collect();
        assert_eq!(
            danger.len(),
            1,
            "one rule for the composer's danger button: {danger:?}"
        );
        let decls: String = danger[0].1.split_whitespace().collect();
        assert!(
            decls.contains("color:var(--failed)"),
            ".composer button.danger {{ {} }} does not use var(--failed)",
            danger[0].1
        );
        let apart = rules
            .iter()
            .find(|(s, _)| s.trim() == "#composer-stop")
            .expect("a rule for #composer-stop");
        let decls: String = apart.1.split_whitespace().collect();
        assert!(
            decls.contains("margin-left:auto"),
            "#composer-stop {{ {} }} does not set it apart",
            apart.1
        );
    }

    #[test]
    fn stop_confirm_abort_is_sent_only_from_the_dialog_s_close() {
        // The risk (#637): a click on Stop aborting at once, or a second
        // path to `abort` beside the dialog, so the confirmation can be
        // walked around. The one `type: "abort"` in composer.js is in the
        // dialog's close handler, behind its returnValue; the click opens
        // the dialog and focuses Cancel, so Enter stops nothing.
        let js = code_of("composer.js");
        assert_eq!(
            js.matches("type: \"abort\"").count(),
            1,
            "composer.js sends abort from more than one place"
        );
        let click = click_handler(&js, "p.stop");
        for bad in ["send(", "invoke(", "abort"] {
            assert!(!click.contains(bad), "Stop's click sends: {click}");
        }
        for want in [
            "p.stopConfirm.returnValue = \"\";",
            "p.stopConfirm.showModal();",
            "p.stopCancel.focus();",
        ] {
            assert!(click.contains(want), "Stop's click lacks {want}: {click}");
        }
        let close = js
            .split("p.stopConfirm.addEventListener(\"close\"")
            .nth(1)
            .and_then(|r| r.split("});").next())
            .expect("composer.js listens for the dialog's close");
        assert!(
            close.contains("if (p.stopConfirm.returnValue !== \"stop\") return;"),
            "the close handler aborts on any close: {close}"
        );
        assert!(
            close.contains("type: \"abort\""),
            "the close handler sends no abort: {close}"
        );
        let parts = function_body(&js, "parts");
        assert!(
            parts.contains("stopConfirm: doc.getElementById(\"stop-confirm\")")
                && parts.contains("stopCancel: doc.getElementById(\"stop-confirm-cancel\")"),
            "composer.js looks up another dialog: {parts}"
        );
    }

    #[test]
    fn log_scroll_transcript_follows_only_when_already_at_the_end() {
        // The risk (#643): every view event set the log's scrollTop to its
        // end, so a reader who scrolled back to read was pulled down at each
        // turn. The end is followed only when the pane was already there,
        // measured before it is redrawn; the one other scroll to the end in
        // render.js is the jump button's own click.
        let js = code_of("render.js");
        let at_end = function_body(&js, "atEnd");
        assert!(
            at_end
                .contains("pane.scrollHeight - pane.clientHeight - pane.scrollTop <= AT_END_SLACK"),
            "atEnd measures something else: {at_end}"
        );
        let transcript = function_body(&js, "transcript");
        let measured = transcript
            .find("const follow = atEnd(pane);")
            .expect("transcript reads the end before drawing");
        let drawn = transcript.find("pane.append(").expect("transcript appends");
        assert!(
            measured < drawn,
            "the end is read after the redraw moved it"
        );
        assert_eq!(
            transcript
                .matches("pane.scrollTop = pane.scrollHeight;")
                .count(),
            1,
            "transcript scrolls to the end in more than one place"
        );
        assert!(
            transcript.contains("if (follow) {\n    pane.scrollTop = pane.scrollHeight;"),
            "the scroll to the end is not under the at-end check: {transcript}"
        );
        let scrolls: Vec<&str> = js
            .lines()
            .filter(|l| l.contains("scrollTop = "))
            .map(str::trim)
            .collect();
        assert_eq!(
            scrolls,
            [
                "pane.scrollTop = pane.scrollHeight;",
                "pane.scrollTop = pane.scrollHeight;"
            ],
            "render.js scrolls somewhere else too"
        );
        assert!(
            click_handler(function_body(&js, "wireLog"), "jump")
                .contains("pane.scrollTop = pane.scrollHeight;"),
            "the second scroll is not the jump button's"
        );
    }

    #[test]
    fn log_scroll_transcript_appends_new_lines_rather_than_replacing() {
        // The risk (#643): the view resends the whole transcript each turn,
        // and replacing every child of a role="log" region may have a screen
        // reader read the whole log again. Lines beyond those drawn are
        // appended; only a list that does not extend the drawn one (the cap
        // dropped lines, or the transcript was cleared) is drawn afresh.
        let js = code_of("render.js");
        let transcript = function_body(&js, "transcript");
        for want in [
            "drawn.every((c, i) => c.textContent === list[i])",
            "list.slice(drawn.length)",
            "if (grows && drawn.length > 0) pane.append(...rows);",
            "else fill(doc, pane, rows, \"Nothing has run yet.\", \"div\");",
        ] {
            assert!(
                transcript.contains(want),
                "transcript lacks {want}: {transcript}"
            );
        }
        let page = page();
        let log = &page[index_of(&page, "pane-transcript")];
        assert_eq!(
            log.attr("role"),
            Some("log"),
            "the appended pane is the log"
        );
    }

    #[test]
    fn log_scroll_jump_button_is_in_run_hidden_as_served_and_written_by_render_js() {
        // The risk (#643): the offer to catch up missing, shown while
        // nothing is below, or shown by a module other than the one that
        // knows what was drawn. It sits under the log in #run, hidden as
        // served; transcript shows it only when lines were added below a
        // reader who is not at the end, and hides it when the end is
        // followed; its click and a scroll to the end by hand hide it too.
        let page = page();
        let jump = index_of(&page, "log-jump");
        assert_eq!(page[jump].tag, "button");
        assert!(page[jump].attrs.contains_key("hidden"), "shown as served");
        assert!(
            within(&page, jump, index_of(&page, "run")),
            "#log-jump is outside #run"
        );
        assert_eq!(
            page[jump].parent,
            page[index_of(&page, "pane-transcript")].parent,
            "#log-jump is not beside the log"
        );
        let text = page[jump].text.trim();
        assert!(
            text.contains("New lines below") && text.contains("Jump to the end"),
            "{text}"
        );
        let js = code_of("render.js");
        let transcript = function_body(&js, "transcript");
        assert!(transcript.contains(r#"const jump = doc.getElementById("log-jump");"#));
        assert!(
            transcript.contains("if (follow) {\n    pane.scrollTop = pane.scrollHeight;\n    jump.hidden = true;\n  } else if (rows.length > 0) {\n    jump.hidden = false;\n  }"),
            "the jump button is shown or hidden on something else: {transcript}"
        );
        let wire = function_body(&js, "wireLog");
        let click = click_handler(wire, "jump");
        assert!(click.contains("jump.hidden = true;"), "{click}");
        assert!(
            wire.contains("pane.addEventListener(\"scroll\"")
                && wire.contains("if (atEnd(pane)) jump.hidden = true;"),
            "a scroll to the end by hand leaves the offer up: {wire}"
        );
        let main = code_of("main.js");
        assert!(
            main.contains(r#"import { drawView, wireLog } from "./render.js";"#)
                && main.contains("wireLog(document);"),
            "main.js does not wire the log"
        );
        for name in [
            "composer.js",
            "dialogs.js",
            "main.js",
            "overview.js",
            "status.js",
        ] {
            assert!(
                !code_of(name).contains("log-jump"),
                "{name} writes the jump button too"
            );
        }
    }

    #[test]
    fn log_scroll_messages_pane_uses_the_same_check() {
        // The risk (#643): the messages pane keeping the old rule, so a
        // reader of trellis's stderr is pulled to the end at each view while
        // the actions log is not; or a second, differently measured notion
        // of "at the end". dialogs.js imports render.js's atEnd and scrolls
        // only under it.
        let js = code_of("dialogs.js");
        assert!(
            js.contains(r#"import { atEnd } from "./render.js";"#),
            "dialogs.js measures the end its own way"
        );
        let stderr = function_body(&js, "drawStderr");
        let measured = stderr
            .find("const follow = atEnd(log);")
            .expect("drawStderr reads the end before drawing");
        let drawn = stderr
            .find("log.replaceChildren(")
            .expect("drawStderr redraws the pane");
        assert!(
            measured < drawn,
            "the end is read after the redraw moved it"
        );
        let scrolls: Vec<&str> = js
            .lines()
            .filter(|l| l.contains("scrollTop"))
            .map(str::trim)
            .collect();
        assert_eq!(
            scrolls,
            ["if (follow) log.scrollTop = log.scrollHeight;"],
            "dialogs.js scrolls somewhere unguarded"
        );
    }

    #[test]
    fn repo_switch_form_sits_beside_recent_repositories_in_the_overview() {
        // The risk (#645): the switch to another repository hidden under
        // "Model: <spec>", a pane that folds after the first choice, so a
        // user with several repositories hunts for it. It sits in the
        // overview's Recent repositories pane, settings.js wires it there by
        // the same ids, and nothing else with an id is in it (an element in
        // the overview that no module writes fails
        // view_renders_every_overview_pane).
        let page = page();
        let form = index_of(&page, "repo-form");
        assert_eq!(page[form].tag, "form");
        let overview = index_of(&page, "overview");
        assert!(
            within(&page, form, overview),
            "#repo-form is outside #overview"
        );
        assert!(
            !within(&page, form, index_of(&page, "model-pane")),
            "#repo-form is still in the model pane"
        );
        let recent = (0..page.len())
            .find(|&i| page[i].attr("aria-labelledby") == Some("ov-recent-title"))
            .expect("the Recent repositories pane");
        assert!(
            within(&page, form, recent),
            "#repo-form is not beside Recent repositories"
        );
        let ids: Vec<&str> = (0..page.len())
            .filter(|&i| i == form || within(&page, i, form))
            .filter_map(|i| page[i].attr("id"))
            .collect();
        assert_eq!(
            ids,
            ["repo-form", "repo-path", "repo-note"],
            "the form's ids"
        );
        let settings = code_of("settings.js");
        let switch = function_body(&settings, "wireRepositorySwitch");
        for id in ids {
            assert!(
                switch.contains(&format!("getElementById(\"{id}\")")),
                "settings.js does not wire #{id}"
            );
        }
        assert!(
            switch.contains(r#"invoke("open_repository", { path: p })"#),
            "the form no longer opens through the core"
        );
        assert!(
            page[form].text.contains("Open another repository"),
            "the form does not say what it does: {}",
            page[form].text
        );
    }

    #[test]
    fn repo_switch_window_title_names_the_open_repository() {
        // The risk (#645): every window titled "trellis", so someone with
        // two repositories open works in the wrong one. status.js writes
        // `document.title` from the root `child-started` carried, as text
        // (the title is plain text, never markup), and nothing else writes
        // it; with no root reported it says "trellis", never a guess.
        let status = code_of("status.js");
        let started = status
            .split(r#"listen("child-started""#)
            .nth(1)
            .and_then(|r| r.split("});").next())
            .expect("status.js listens for child-started");
        assert!(
            started.contains(
                "doc.title = str(c.root) === null ? \"trellis\" : `trellis · ${c.root}`;"
            ),
            "child-started does not title the window from the root: {started}"
        );
        assert_eq!(
            status.matches(".title").count(),
            1,
            "status.js titles twice"
        );
        for (name, code) in ui_code() {
            if name == "status.js" {
                continue;
            }
            for bad in ["doc.title", "document.title", "win.document.title"] {
                assert!(!code.contains(bad), "{name} writes the window title too");
            }
        }
        let page = page();
        let title = page
            .iter()
            .find(|e| e.tag == "title")
            .expect("the page has a <title>");
        assert_eq!(
            title.text.trim(),
            "trellis",
            "the served title is the bare name"
        );
    }

    #[test]
    fn repo_switch_catalogue_folds_under_a_closed_details() {
        // The risk (#645): at 1280x820 the model pane filled the first
        // screen and the Overview heading started at about 740 px, with the
        // catalogue the longest part. It folds under its own details, served
        // closed, which no module opens; its summary is the heading the list
        // is labelled by, so the words saying what the catalogue is stay.
        let page = page();
        let fold = index_of(&page, "model-catalog-pane");
        assert_eq!(page[fold].tag, "details");
        assert!(
            !page[fold].attrs.contains_key("open"),
            "the catalogue is served open"
        );
        assert!(
            within(&page, fold, index_of(&page, "model-pane")),
            "the catalogue left the model pane"
        );
        let summary = index_of(&page, "model-catalog-title");
        assert_eq!(page[summary].tag, "summary");
        assert_eq!(
            page[summary].parent,
            Some(fold),
            "the summary is not the fold's"
        );
        let list = index_of(&page, "model-catalog");
        assert!(
            within(&page, list, fold),
            "#model-catalog is outside the fold"
        );
        assert_eq!(
            page[list].attr("aria-labelledby"),
            Some("model-catalog-title")
        );
        for (name, code) in ui_code() {
            assert!(
                !code.contains("model-catalog-pane"),
                "{name} reaches the fold, and may open it"
            );
        }
        // The fold leaves the catalogue drawn as before: settings.js still
        // writes the list.
        assert!(
            function_body(&code_of("settings.js"), "drawCatalog")
                .contains(r#"getElementById("model-catalog")"#)
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
        // The catalogue stays in the model pane; the repository switch
        // moved to the overview, beside Recent repositories (#645), where
        // the repo_switch_ tests hold it.
        let pane = index_of(&page, "model-pane");
        assert!(
            within(&page, index_of(&page, "model-catalog"), pane),
            "#model-catalog"
        );
        let overview = index_of(&page, "overview");
        for id in ["repo-form", "repo-path", "repo-note"] {
            assert!(within(&page, index_of(&page, id), overview), "#{id}");
            assert!(
                !within(&page, index_of(&page, id), pane),
                "#{id} is still in the model pane"
            );
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

    #[test]
    fn status_bar_paths_show_a_name_with_the_whole_path_as_title() {
        // The risk (#652): two absolute paths in a one-row bar, wrapping it
        // over two or three lines on every screen; or a name with no way to
        // read the path it stands for. No JavaScript runs under cargo test,
        // so this holds the text; a node smoke script with a fake DOM ran
        // status.js with POSIX and Windows paths (see the commit).
        let js = code_of("status.js");
        let started = js
            .split(r#"listen("child-started""#)
            .nth(1)
            .and_then(|r| r.split("});").next())
            .expect("status.js listens for child-started");
        for want in [
            r#"showPath(doc, "status-trellis", str(c.trellis), "path not reported", ` · ${version}`);"#,
            r#"showPath(doc, "status-root", str(c.root), "not reported");"#,
        ] {
            assert!(started.contains(want), "child-started lacks {want}");
        }
        let show = function_body(&js, "showPath");
        for want in [
            "e.textContent = baseName(path) + after;",
            r#"e.setAttribute("title", path);"#,
            r#"e.removeAttribute("title");"#,
        ] {
            assert!(show.contains(want), "showPath lacks {want}");
        }
        // Either separator, so a Windows path is named by its file too.
        assert!(
            function_body(&js, "baseName").contains(r"path.split(/[\\/]/)"),
            "baseName splits on one separator only"
        );
    }

    #[test]
    fn status_bar_paths_cells_end_in_an_ellipsis_rather_than_wrap() {
        // The risk (#652): a long value wrapping the bar, or cut off with
        // nothing to say it was. Each cell keeps to one line and ends in an
        // ellipsis when it does not fit.
        let rules = css_rules();
        let cell: Vec<String> = rules
            .iter()
            .filter(|(s, _)| s.split(',').any(|s| s.trim() == ".status-bar dd"))
            .map(|(_, d)| d.split_whitespace().collect())
            .collect();
        assert_eq!(cell.len(), 1, "one .status-bar dd rule");
        for want in [
            "text-overflow:ellipsis",
            "white-space:nowrap",
            "overflow:hidden",
            "max-width:",
        ] {
            assert!(
                cell[0].contains(want),
                ".status-bar dd lacks {want}: {}",
                cell[0]
            );
        }
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

    #[test]
    fn focus_section_every_section_heading_takes_focus_outside_the_tab_order() {
        // The risk (#646): a rail press left focus in the rail, so Tab
        // walked the rail, the status area and the model pane before the
        // section chosen. Each of the seven sections' headings, the element
        // its aria-labelledby names, takes focus with tabindex -1: focus
        // can be moved there, and Tab does not stop on it.
        let page = page();
        for id in SECTIONS {
            let section = &page[index_of(&page, id)];
            let title = section
                .attr("aria-labelledby")
                .unwrap_or_else(|| panic!("#{id} names no heading"));
            let h = index_of(&page, title);
            assert_eq!(page[h].tag, "h2", "#{id}'s heading #{title}");
            assert!(
                within(&page, h, index_of(&page, id)),
                "#{title} is outside #{id}"
            );
            assert_eq!(
                page[h].attr("tabindex"),
                Some("-1"),
                "#{title} cannot take focus, or joins the tab order"
            );
        }
    }

    #[test]
    fn focus_section_rail_press_focuses_the_shown_section_s_heading() {
        // The risk (#646): showing a section and leaving focus on the rail
        // button, or focusing something other than the heading the section
        // is labelled by. No JavaScript runs under cargo test, so this holds
        // the text; a node smoke script with a fake DOM pressed each rail
        // button (see the commit).
        let main = code_of("main.js");
        let rail = function_body(&main, "wireRail");
        let click = rail
            .split("b.addEventListener(\"click\", () => {")
            .nth(1)
            .and_then(|r| r.split("});").next())
            .expect("each rail button has a click handler");
        let shown = click
            .find("select(b);")
            .expect("the press shows its section");
        let section = click
            .find("document.getElementById(b.getAttribute(\"aria-controls\"))")
            .expect("the press finds its section");
        let heading = click
            .find("document.getElementById(section.getAttribute(\"aria-labelledby\"))")
            .expect("the press finds the section's heading");
        let focused = click
            .find("heading.focus();")
            .expect("the press focuses it");
        assert!(
            shown < section && section < heading && heading < focused,
            "the heading is focused before its section is shown: {click}"
        );
    }

    #[test]
    fn focus_section_page_has_exactly_one_h1_a_reader_can_find() {
        // The risk (#646): a page with no h1, so heading navigation starts
        // at the sections with nothing naming the page; or an h1 hidden by
        // `display` or `visibility`, which hides it from a screen reader
        // too, or drawn above every section where it costs the first
        // screen a line.
        let page = page();
        let h1s: Vec<usize> = (0..page.len()).filter(|&i| page[i].tag == "h1").collect();
        assert_eq!(h1s.len(), 1, "one h1");
        let h1 = &page[h1s[0]];
        assert_eq!(h1.text.trim(), "trellis");
        let first_section = (0..page.len())
            .find(|&i| page[i].tag == "section")
            .expect("a section");
        assert!(h1s[0] < first_section, "the h1 comes after a section");
        let class = h1.attr("class").expect("the h1 has a class");
        let rules = css_rules();
        let rule: Vec<String> = rules
            .iter()
            .filter(|(s, _)| s.split(',').any(|s| s.trim() == format!(".{class}")))
            .map(|(_, d)| d.split_whitespace().collect())
            .collect();
        assert_eq!(rule.len(), 1, "one .{class} rule");
        for bad in ["display:none", "visibility:hidden"] {
            assert!(
                !rule[0].contains(bad),
                ".{class} {{ {} }} hides it from a reader",
                rule[0]
            );
        }
        for want in [
            "position:absolute",
            "width:1px",
            "height:1px",
            "overflow:hidden",
        ] {
            assert!(rule[0].contains(want), ".{class} lacks {want}: it is drawn");
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

    /// `style.css` with its comments removed.
    fn css_text() -> String {
        let css = ui_file("style.css");
        let mut text = String::new();
        let mut rest = css.as_str();
        while let Some(i) = rest.find("/*") {
            text.push_str(&rest[..i]);
            let end = rest[i..].find("*/").expect("an unclosed CSS comment");
            rest = &rest[i + end + 2..];
        }
        text.push_str(rest);
        text
    }

    /// The colour tokens of one theme: the custom properties of the `:root`
    /// block at `text[from..]`, by name without the leading `--`.
    fn css_tokens(text: &str, from: usize) -> BTreeMap<String, String> {
        let block = &text[from..];
        let start = block.find(":root").expect("a :root block");
        let open = block[start..].find('{').expect(":root opens") + start;
        let close = block[open..].find('}').expect(":root closes") + open;
        block[open + 1..close]
            .split(';')
            .filter_map(|d| d.split_once(':'))
            .filter_map(|(k, v)| {
                k.trim()
                    .strip_prefix("--")
                    .map(|k| (k.to_string(), v.trim().to_string()))
            })
            .collect()
    }

    /// The light and dark token tables: the first `:root` block, and the one
    /// inside the dark `@media` block.
    fn css_themes() -> [(&'static str, BTreeMap<String, String>); 2] {
        let text = css_text();
        let dark = text
            .find("@media (prefers-color-scheme: dark)")
            .expect("a dark theme block");
        [
            ("light", css_tokens(&text, 0)),
            ("dark", css_tokens(&text, dark)),
        ]
    }

    /// WCAG 2 relative luminance of a `#rrggbb` colour, computed here: no
    /// crate parses colours, and a new one needs an ADR.
    fn luminance(hex: &str) -> f64 {
        let hex = hex
            .strip_prefix('#')
            .unwrap_or_else(|| panic!("{hex}: a token this test reads must be #rrggbb"));
        assert_eq!(
            hex.len(),
            6,
            "{hex}: a token this test reads must be #rrggbb"
        );
        let channel = |i: usize| -> f64 {
            let byte =
                u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or_else(|e| panic!("#{hex}: {e}"));
            let c = f64::from(byte) / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4)
    }

    /// WCAG 2 contrast ratio between two `#rrggbb` colours.
    fn contrast(a: &str, b: &str) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn field_contrast_token_meets_three_to_one_in_both_themes() {
        // The risk (#647, UX-13): `--border` is a pane divider that measures
        // 1.4:1 against the pane, so a field drawn with it is nearly
        // invisible when empty. Fields take `--control-border`, which must
        // clear WCAG's 3:1 non-text floor against both backgrounds a field
        // sits on, in each theme. The arithmetic is checked on known pairs
        // first, so a wrong formula cannot pass a wrong colour.
        assert!((contrast("#000000", "#ffffff") - 21.0).abs() < 1e-9);
        assert!((contrast("#ffffff", "#ffffff") - 1.0).abs() < 1e-9);
        for (theme, tokens) in css_themes() {
            let token = |k: &str| {
                tokens
                    .get(k)
                    .unwrap_or_else(|| panic!("{theme}: no --{k} token"))
                    .as_str()
            };
            let control = token("control-border");
            for bg in ["pane-bg", "bg"] {
                let ratio = contrast(control, token(bg));
                assert!(
                    ratio >= 3.0,
                    "{theme}: --control-border {control} on --{bg} {} is {ratio:.2}:1, below 3:1",
                    token(bg)
                );
            }
            // The case this fixes is still the case: the divider token alone
            // would fail, so a field that falls back to it is caught below.
            assert!(
                contrast(token("border"), token("pane-bg")) < 3.0,
                "{theme}: --border now clears 3:1; the field rule below may be redundant"
            );
        }
    }

    /// Whether a CSS selector names a form field: an `input`, `select` or
    /// `textarea` element, or one of the page's field ids.
    fn names_a_field(selector: &str, field_ids: &BTreeSet<String>) -> bool {
        let last = selector.split_whitespace().last().unwrap_or("");
        let element = last.split(['[', ':', '.', '#']).next().unwrap_or("");
        ["input", "select", "textarea"].contains(&element)
            || last
                .strip_prefix('#')
                .map(|id| id.split([':', '[', '.']).next().unwrap_or(""))
                .is_some_and(|id| field_ids.contains(id))
    }

    #[test]
    fn field_contrast_every_field_border_uses_the_control_token() {
        // The risk: a field rule added later takes `--border` by habit (every
        // other bordered box does) and ships a 1.4:1 field. Each rule whose
        // selector names a field and whose declarations set a border must
        // draw it with the control token; a rule shared with a non-field
        // (the composer's select and button) may keep `--border` only if a
        // field-only rule after it overrides the colour.
        let page = page();
        let field_ids: BTreeSet<String> = page
            .iter()
            .filter(|e| ["input", "select", "textarea"].contains(&e.tag.as_str()))
            .filter_map(|e| e.attr("id").map(str::to_string))
            .collect();
        let rules = css_rules();
        let mut field_rules = 0;
        let mut overridden: BTreeSet<String> = BTreeSet::new();
        for (selectors, decls) in rules.iter().rev() {
            let parts: Vec<&str> = selectors.split(',').map(str::trim).collect();
            let fields: Vec<&str> = parts
                .iter()
                .copied()
                .filter(|s| names_a_field(s, &field_ids))
                .collect();
            if fields.is_empty() || !decls.contains("border") {
                continue;
            }
            let decls_s: String = decls.split_whitespace().collect();
            let sets_colour = decls_s.contains("border:") || decls_s.contains("border-color:");
            if !sets_colour {
                continue;
            }
            field_rules += 1;
            if decls_s.contains("var(--control-border)") {
                overridden.extend(fields.iter().map(|s| s.to_string()));
                continue;
            }
            assert!(
                decls_s.contains("var(--border)"),
                "{selectors} draws a field border in a colour this test cannot measure: {decls}"
            );
            // The one allowed shape: the rule is shared with a non-field and a
            // later field-only rule sets the colour.
            assert!(
                parts.len() > fields.len(),
                "{selectors} {{ {decls} }} draws a field with the divider token"
            );
            for f in &fields {
                assert!(
                    overridden.contains(*f),
                    "{selectors} draws {f} with the divider token and nothing later overrides it"
                );
            }
        }
        assert!(
            field_rules >= 8,
            "{field_rules} field border rules found; the trails filter, codebase search, \
             symbol, vault query, model spec, composer and atlas fields all have one"
        );
    }

    #[test]
    fn field_contrast_focus_ring_covers_every_field_and_summary() {
        // The risk: `button:focus-visible` is the page's ring, but a field or
        // a `<summary>` it does not name falls back to the engine's own,
        // which differs between WebView2 and WebKitGTK. One rule names the
        // four element types and draws the same `--focus` outline.
        let rules = css_rules();
        for element in ["input", "select", "textarea", "summary"] {
            let want = format!("{element}:focus-visible");
            let matching: Vec<&(String, String)> = rules
                .iter()
                .filter(|(s, _)| s.split(',').any(|s| s.trim() == want))
                .collect();
            assert_eq!(matching.len(), 1, "one bare {want} rule: {matching:?}");
            let decls: String = matching[0].1.split_whitespace().collect();
            assert!(
                decls.contains("outline:2pxsolidvar(--focus)"),
                "{want} {{ {} }} is not the page's focus ring",
                matching[0].1
            );
        }
    }
}
