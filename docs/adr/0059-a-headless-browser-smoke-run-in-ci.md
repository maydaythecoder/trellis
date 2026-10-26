# ADR 0059 — A headless browser smoke run of the desktop page in CI

**Status:** Accepted (owner-directed). Built (#797).
**Date:** 2026-10-26
**Issue:** #797 (story S20, #790).
**Decided by:** the owner, 2026-10-25 about 22:50 New York time, in the
session: a headless browser smoke run is allowed in CI. The method below was
chosen by the session under the owner's instruction of the same evening to
work unattended; it is reversible.
**Holds:** ADR 0048 §1 and §5 (the desktop is its own workspace and lock; its
page is static HTML, CSS and ES modules with no Node, npm or bundler) and the
CLAUDE.md rule that a new crate dependency needs an ADR: this adds none.

## Context

The desktop's tests hold its Rust core (`desktop/src/`) and read its page's
source as text (`desktop/src/lockdown.rs`); none of them runs the page.
Every page module's start ends in `.catch(console.error)`, which a release
window never shows, so a module that fails to start does so silently. A
listener a section adds and never removes, or nodes it keeps after it is
left, grow with every visit and every run, and nothing measured them. The
owner asked for no leaking memory and a fully running app, and allowed a
headless browser in CI to check it.

What was there to build on: GitHub's `ubuntu-latest` image ships Chrome.
Chrome speaks the DevTools Protocol over a pipe when started with
`--remote-debugging-pipe` (it reads commands on fd 3 and writes on fd 4, one
JSON message per NUL byte), which Python's standard library can drive. The
desktop already serves its page over HTTP for `trellis --web`
(`desktop/src/web.rs`, ADR 0050 §5), with the same four commands and events
the window has, and the QA fixture (`qa/fixtures/trellis-demo.bundle`) and
its scripted models (`qa/fixtures/scripted-*.json`) run a task with no model
called.

## Decision

1. `qa/desktop_smoke.py`, standard library only, starts `trellis-desktop
   --web` against a fresh clone of the fixture (through
   `qa/demo_fixture.py`, at the baseline `behaviour.json` names) with an
   empty `TRELLIS_CONFIG_DIR`, opens the page in the runner's headless Chrome
   over the pipe, and sends the child its commands through the page's own
   `rpc_send` endpoint, as the page does. Chrome runs without its sandbox,
   which needs user namespaces that a container's root or an Ubuntu 24.04
   runner's AppArmor policy can refuse; it opens nothing but the
   repository's own page and the self-test's, from 127.0.0.1.
2. It fails on an uncaught exception (`Runtime.exceptionThrown`), a
   `console.error` (`Runtime.consoleAPICalled`) and a log entry at level
   error (`Log.entryAdded`, where a Content-Security-Policy refusal shows),
   at any point of the run. One request is not the page's: the browser asks
   for `/favicon.ico` on its own while the page names no icon (until #835),
   and the server's refusal of it is ignored.
3. After a warm-up (one task, one walk), it counts documents, nodes and
   listeners (`Memory.getDOMCounters`, after `HeapProfiler.collectGarbage`).
   It walks into every level of the page and out again, five times, and
   all three counts must be where they were. It then runs five tasks to
   their `run_end` with `scripted-done-only.json`, and the document and
   listener counts must be where they were. The node count is printed and
   not judged after the tasks, since a task adds transcript rows that the
   page keeps up to its cap (#814 holds the cap). *Amended 2026-10-26
   (#826):* the walk goes into each of the overview's panels and out with
   ESC, then across the sibling rail, since the levels replaced the
   section rail it first walked; and the warm-up walks until a walk leaves
   the counts where the one before left them (at most four walks), since a
   section asks for what it shows only while it is shown, the Atlas in
   steps, and on a slow machine one walk can end before the page has drawn
   all it will.
4. The JS heap (`Runtime.getHeapUsage`) is printed as a trend and not
   judged: the protocol marks it experimental.
5. `--self-test` runs the same checks over three pages the script serves
   itself: a clean one must pass, one that logs a console error must fail,
   and one that adds a listener on every cycle and never removes it must
   fail.
6. CI runs the self-test and then the smoke run in the `desktop` job on
   Linux, after the desktop is built and tested. It is CI-only: nothing in
   the product or the desktop's build depends on it.

## Consequences

- A page module that throws, logs an error, breaks the CSP or leaves
  listeners or nodes behind fails the desktop job on Linux.
- The run takes the job a little longer: one Chrome, one fixture clone and
  six scripted tasks.
- The script reaches the desktop the way `trellis --web` does, so a change
  to the web view's URL line, its token or its `rpc_send` endpoint has to
  keep the script working, and the desktop job says so when it does not.

## Alternatives rejected

- **Playwright or Puppeteer.** They would bring Node and npm into CI for
  the desktop, which ADR 0048 §5 refuses for the page; the protocol they
  speak is reachable from the standard library.
- **A Rust DevTools crate (`headless_chrome`, `chromiumoxide`).** Either
  would add tungstenite, tokio and more to `desktop/Cargo.lock`, a new
  dependency tree audited by `cargo deny` and `cargo audit` for a check that
  runs only in CI.
- **WebDriver (Selenium or chromedriver).** A driver binary to download and
  pin, and a second protocol in front of the one Chrome already speaks.
- **Judging the node count after the tasks.** The transcript grows with
  each task, legitimately, until its cap; a fixed node count there would
  fail on correct pages.
- **Running it on Windows.** The pipe is passed as fds 3 and 4, which
  Windows does not have; the page and its modules are the same files on
  both platforms.

## Status

Accepted (owner-directed). Built: `qa/desktop_smoke.py` and its CI step
(#797).

## Reversibility

Removing the CI step and the script removes it entirely; nothing else
depends on them.

## What it does not cover

It exercises Chromium, the engine WebView2 uses on Windows, and not
WebKitGTK, which the Linux window uses. A leak or an error in the page's own
code shows in both; one that only WebKitGTK's engine produces does not show
here. It runs the web view, not the Tauri window: the IPC between the window
and its core is held by the desktop's cargo tests, not by this run.
