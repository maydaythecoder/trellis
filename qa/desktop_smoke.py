#!/usr/bin/env python3
"""The desktop page in a real browser engine: errors and leaks (#797, ADR 0059).

    python3 qa/desktop_smoke.py [--chrome PATH] [--desktop PATH] [--trellis PATH]
                                [--prompts N] [--cycles N]
    python3 qa/desktop_smoke.py --self-test [--chrome PATH]

The desktop's cargo tests hold its Rust core and read its page's source; none
of them runs the page. Every module's start ends in `.catch(console.error)`,
which a release window never shows. This runs it.

It starts `trellis-desktop --web` against a fresh clone of the vendored
fixture (qa/fixtures/trellis-demo.bundle, through qa/demo_fixture.py), opens
the page in headless Chrome over the DevTools Protocol on a pipe
(`--remote-debugging-pipe`: Chrome reads commands on fd 3 and writes on fd 4,
one JSON message per NUL byte), and fails on:

- an uncaught exception (`Runtime.exceptionThrown`), a `console.error`
  (`Runtime.consoleAPICalled` of type error), or a log entry at level error
  (`Log.entryAdded`, where a Content-Security-Policy refusal shows up);
- listeners, nodes or documents that do not come back. After a warm-up
  (one task, and walks until one leaves the counts where the last did), it
  counts them (`Memory.getDOMCounters`, after `HeapProfiler.collectGarbage`),
  then walks into every level of the page and out again, N times,
  and the three counts must be where they were; then it runs N tasks to
  their `run_end` with a scripted model (qa/fixtures/scripted-done-only.json:
  no model is called), and the listener and document counts must be where
  they were. The node count is printed, not judged, after the tasks: a task
  adds transcript rows, which the page keeps up to its cap.

The JS heap (`Runtime.getHeapUsage`) is printed as a trend and not judged,
since the protocol marks it experimental.

The walk goes in and out of the desktop's levels (the Trail, #826): into
each of the overview's panels and out with ESC, then across the sibling
rail.

Standard library only: no Playwright, no npm and no new crate (ADR 0059).
Chromium only, the engine WebView2 uses; WebKitGTK is not exercised.
Linux and macOS: the pipe is passed as fds 3 and 4, which Windows lacks.

`--self-test` runs the same checks against three small pages it serves
itself: one clean, one that logs a console error, one that adds a listener
it never removes; the clean one must pass and the other two must fail.

Exit 0: clean. Exit 1: an error on the page, a count that did not come
back, or the self-test did not hold. Exit 2: Chrome, a binary or the fixture
is missing, or the arguments are wrong.
"""

import argparse
import http.client
import http.server
import json
import os
import pathlib
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse

# qa/demo_fixture.py is imported from qa/; a __pycache__ left beside it would
# be an untracked directory in this repository after every run.
sys.dont_write_bytecode = True

from demo_fixture import fixture_repo  # noqa: E402

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent
FIXTURES = HERE / "fixtures"
# The fixture's baseline, as qa/fixtures/behaviour.json names it.
BASELINE = json.loads((FIXTURES / "behaviour.json").read_text(encoding="utf-8"))["_baseline_commit"]
SCRIPT = FIXTURES / "scripted-done-only.json"

# How long one wait may take before the run fails, in seconds: a page load,
# a run to its end, a protocol answer.
WAIT = 60

# The most walks the warm-up takes to reach counts that hold (see smoke()).
WARM_WALKS = 4


class Missing(Exception):
    """A tool the run needs is not there: exit 2."""


class Chrome:
    """Headless Chrome, spoken to over the DevTools Protocol on a pipe."""

    def __init__(self, binary):
        self.profile = tempfile.mkdtemp(prefix="trellis-smoke-chrome-")
        # Chrome reads commands on its fd 3 and writes on its fd 4. Each end
        # is first moved above fd 10, so placing it at 3 or 4 in the child
        # cannot overwrite the other.
        to_chrome_r, to_chrome_w = (_high(fd) for fd in os.pipe())
        from_chrome_r, from_chrome_w = (_high(fd) for fd in os.pipe())

        def place():
            os.dup2(to_chrome_r, 3)
            os.dup2(from_chrome_w, 4)

        args = [
            binary,
            "--headless=new",
            "--remote-debugging-pipe",
            f"--user-data-dir={self.profile}",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-gpu",
            "--disable-extensions",
            "--disable-background-networking",
            # Chrome's sandbox needs unprivileged user namespaces, which a
            # container's root and an Ubuntu 24.04 runner's AppArmor policy
            # can refuse. The only pages this browser opens are the
            # repository's own page and the self-test's, from 127.0.0.1.
            "--no-sandbox",
            "about:blank",
        ]
        self.proc = subprocess.Popen(
            args, preexec_fn=place, pass_fds=(3, 4),
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        for fd in (to_chrome_r, from_chrome_w):
            os.close(fd)
        self.out = os.fdopen(to_chrome_w, "wb", buffering=0)
        self.inp = os.fdopen(from_chrome_r, "rb", buffering=0)
        self.next_id = 0
        self.answers = {}
        self.answered = threading.Condition()
        self.events = queue.Queue()
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()

    def _read(self):
        buf = b""
        while True:
            chunk = self.inp.read(65536)
            if not chunk:
                with self.answered:
                    self.answered.notify_all()
                return
            buf += chunk
            while b"\0" in buf:
                raw, buf = buf.split(b"\0", 1)
                msg = json.loads(raw)
                if "id" in msg:
                    with self.answered:
                        self.answers[msg["id"]] = msg
                        self.answered.notify_all()
                else:
                    self.events.put(msg)

    def send(self, method, params=None, session=None):
        """Sends one command and returns its result, or raises its error."""
        self.next_id += 1
        mid = self.next_id
        msg = {"id": mid, "method": method, "params": params or {}}
        if session:
            msg["sessionId"] = session
        self.out.write(json.dumps(msg).encode() + b"\0")
        end = time.monotonic() + WAIT
        with self.answered:
            while mid not in self.answers:
                left = end - time.monotonic()
                if left <= 0 or self.proc.poll() is not None:
                    raise RuntimeError(f"Chrome did not answer {method}")
                self.answered.wait(left)
            msg = self.answers.pop(mid)
        if "error" in msg:
            raise RuntimeError(f"{method}: {msg['error'].get('message')}")
        return msg.get("result", {})

    def close(self):
        try:
            self.send("Browser.close")
        except (RuntimeError, OSError):
            pass
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            # Chrome, not trellis: nothing of the repository's runs in it.
            self.proc.kill()
            self.proc.wait()
        # Chrome has exited, so the reader sees the end of its pipe and
        # returns; both ends are closed once it has.
        self.out.close()
        self.reader.join(timeout=5)
        if not self.reader.is_alive():
            self.inp.close()
        shutil.rmtree(self.profile, ignore_errors=True)


def _high(fd):
    """fd duplicated to 10 or above, the original closed."""
    import fcntl

    high = fcntl.fcntl(fd, fcntl.F_DUPFD, 10)
    os.close(fd)
    return high


class Page:
    """One tab, its errors collected as they arrive."""

    def __init__(self, chrome):
        self.chrome = chrome
        target = chrome.send("Target.createTarget", {"url": "about:blank"})["targetId"]
        self.session = chrome.send("Target.attachToTarget", {"targetId": target, "flatten": True})["sessionId"]
        self.errors = []
        self.loaded = threading.Event()
        for domain in ("Runtime", "Log", "Page", "HeapProfiler"):
            self.send(f"{domain}.enable")

    def send(self, method, params=None):
        return self.chrome.send(method, params, self.session)

    def drain(self):
        """Takes every event that has arrived, keeping this page's errors."""
        while True:
            try:
                ev = self.chrome.events.get_nowait()
            except queue.Empty:
                return
            if ev.get("sessionId") != self.session:
                continue
            method, p = ev.get("method"), ev.get("params", {})
            if method == "Runtime.exceptionThrown":
                d = p.get("exceptionDetails", {})
                what = (d.get("exception") or {}).get("description") or d.get("text", "")
                self.errors.append(f"exception: {what}")
            elif method == "Runtime.consoleAPICalled" and p.get("type") == "error":
                parts = [a.get("value", a.get("description", "")) for a in p.get("args", [])]
                self.errors.append("console.error: " + " ".join(str(x) for x in parts))
            elif method == "Log.entryAdded" and p.get("entry", {}).get("level") == "error":
                e = p["entry"]
                # The browser asks for /favicon.ico on its own when a page
                # names no icon, and the desktop's page names none until
                # #835; the server's refusal of it is not the page's error.
                if urllib.parse.urlparse(e.get("url", "")).path == "/favicon.ico":
                    continue
                self.errors.append(f"log ({e.get('source')}): {e.get('text')} {e.get('url', '')}".rstrip())
            elif method == "Page.loadEventFired":
                self.loaded.set()

    def settle(self, seconds):
        """Lets the page work for `seconds`, collecting what it reports."""
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            self.drain()
            time.sleep(0.05)
        self.drain()

    def open(self, url):
        self.loaded.clear()
        self.send("Page.navigate", {"url": url})
        end = time.monotonic() + WAIT
        while not self.loaded.is_set():
            if time.monotonic() > end:
                raise RuntimeError(f"{url} did not load within {WAIT} s")
            self.settle(0.1)

    def run(self, expression):
        """Evaluates `expression` in the page, awaiting a promise."""
        r = self.send("Runtime.evaluate", {"expression": expression, "awaitPromise": True, "returnByValue": True})
        if "exceptionDetails" in r:
            raise RuntimeError(f"{expression!r} threw: {r['exceptionDetails'].get('text')}")
        return r.get("result", {}).get("value")

    def counts(self):
        """Documents, nodes and listeners once garbage has been collected."""
        for _ in range(2):
            self.send("HeapProfiler.collectGarbage")
        c = self.send("Memory.getDOMCounters")
        return {k: c[k] for k in ("documents", "nodes", "jsEventListeners")}

    def heap(self):
        try:
            return self.send("Runtime.getHeapUsage").get("usedSize")
        except RuntimeError:
            return None


def judged(name, before, after, keys):
    """The lines that fail a check: each count in `keys` that moved."""
    return [f"{name}: {k} {before[k]} -> {after[k]}" for k in keys if after[k] != before[k]]


# --- The desktop ------------------------------------------------------------

# One walk (#826): into every level the overview's panels enter and out
# again with ESC, as a person does, then across the sibling rail from the
# first to the last and out. A section fetches what it shows when it is
# entered, so the walk also runs each fetch and its drawing, and each level
# draws the Trail and the rail under a scope that leaving it ends. Answers
# with how many levels it walked.
WALK = """(async () => {
  const pause = () => new Promise((r) => setTimeout(r, 150));
  const esc = () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  // The model chooser opens over the stage at launch (#909); a person
  // closes it with ESC before walking, and ESC closes it before it leaves
  // a level.
  const chooser = document.getElementById("model-pane");
  if (chooser.open) {
    esc(); await pause();
    if (chooser.open) throw new Error("ESC did not close the model chooser");
    if (document.getElementById("overview").hidden) throw new Error("ESC left the overview with the chooser open");
  }
  const panels = [...document.querySelectorAll("button[data-enter]")];
  if (panels.length === 0) throw new Error("no overview panels to walk");
  for (const p of panels) {
    p.click(); await pause();
    if (document.getElementById(p.dataset.enter).hidden) throw new Error(`${p.dataset.enter} was not entered`);
    esc(); await pause();
    if (document.getElementById("overview").hidden) throw new Error(`ESC did not go out of ${p.dataset.enter}`);
  }
  panels[0].click(); await pause();
  for (const s of [...document.querySelectorAll("#siblings button")]) { s.click(); await pause(); }
  esc(); await pause();
  return panels.length;
})()"""


class Desktop:
    """`trellis-desktop --web` over a clone of the fixture."""

    def __init__(self, desktop, trellis, repo):
        env = dict(os.environ)
        # The user's own configuration stays out of the run: an empty one,
        # as the desktop's own tests use (desktop/CLAUDE.md).
        self.config = tempfile.mkdtemp(prefix="trellis-smoke-config-")
        env["TRELLIS_CONFIG_DIR"] = self.config
        self.proc = subprocess.Popen(
            [desktop, "--web", "--root", str(repo), "--trellis", trellis],
            cwd=repo, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        line = self.proc.stdout.readline()
        if not line:
            raise Missing(f"{desktop} --web printed no URL: {self.proc.stderr.read().decode(errors='replace')[-500:]}")
        url = urllib.parse.urlparse(json.loads(line)["url"])
        self.host, self.port = url.hostname, url.port
        self.token = urllib.parse.parse_qs(url.query)["t"][0]
        self.page_url = f"http://{self.host}:{self.port}/{self.token}/"
        self.frames = queue.Queue()
        threading.Thread(target=self._events, daemon=True).start()
        # The desktop writes only its errors to stderr. Read as they come,
        # so one shows in the run's log and the pipe never fills and holds
        # the desktop on a write; the thread ends when the desktop exits.
        self.stderr = threading.Thread(target=self._stderr, daemon=True)
        self.stderr.start()

    def _stderr(self):
        for raw in self.proc.stderr:
            print(f"smoke: trellis-desktop stderr: {raw.decode('utf-8', errors='replace').rstrip()}")

    def _events(self):
        """The core's events, as a second page would hear them: the frames
        that say when a run has ended."""
        c = http.client.HTTPConnection(self.host, self.port)
        c.request("GET", f"/{self.token}/events", headers={"Host": f"{self.host}:{self.port}"})
        r = c.getresponse()
        event = None
        for raw in r:
            line = raw.decode("utf-8", errors="replace").rstrip("\r\n")
            if line.startswith("event:"):
                event = line[6:].strip()
            elif line.startswith("data:") and event == "rpc-frame":
                try:
                    self.frames.put(json.loads(line[5:].strip()))
                except json.JSONDecodeError:
                    pass

    def send(self, line):
        """rpc_send, as the page sends it: one command for the child."""
        c = http.client.HTTPConnection(self.host, self.port, timeout=WAIT)
        body = json.dumps({"line": line})
        c.request("POST", f"/{self.token}/invoke/rpc_send", body=body,
                  headers={"Host": f"{self.host}:{self.port}", "Content-Type": "application/json"})
        answer = json.loads(c.getresponse().read() or b"{}")
        if "error" in answer:
            raise RuntimeError(f"rpc_send {line.get('type')}: {answer['error']}")

    def wait_for(self, want, what):
        end = time.monotonic() + WAIT
        while time.monotonic() < end:
            try:
                f = self.frames.get(timeout=0.5)
            except queue.Empty:
                continue
            if want(f):
                return f
        raise RuntimeError(f"no {what} within {WAIT} s")

    def close(self):
        # Closing stdin is how the server is asked to stop: the core sends
        # the child quit and waits (ADR 0048 section 9). Nothing is killed.
        try:
            self.proc.stdin.close()
            self.proc.wait(timeout=WAIT)
        except subprocess.TimeoutExpired:
            print("smoke: trellis-desktop did not stop within the bound after its stdin closed")
        else:
            self.stderr.join(timeout=5)
            self.proc.stdout.close()
        shutil.rmtree(self.config, ignore_errors=True)


def task(d, n):
    """Runs one task to its run_end with the scripted model."""
    d.send({"type": "prompt", "task": f"smoke task {n}", "id": f"smoke-{n}"})
    d.wait_for(lambda f: f.get("type") == "run_end", f"run_end for task {n}")


def smoke(chrome_bin, desktop, trellis, prompts, cycles):
    failures = []
    with fixture_repo(None, BASELINE) as repo:
        d = Desktop(desktop, trellis, repo)
        # Set inside the try, so a Chrome that fails to start still leaves
        # the desktop asked to stop before the clone it runs in is removed.
        chrome = None
        try:
            chrome = Chrome(chrome_bin)
            page = Page(chrome)
            page.open(d.page_url)
            page.settle(3)
            d.send({"type": "set_model", "model": "scripted:" + SCRIPT.as_posix(), "id": "smoke-model"})
            d.wait_for(lambda f: f.get("type") == "response" and f.get("id") == "smoke-model", "set_model answer")
            # Warm-up: whatever the page creates once for its life is made
            # here, before anything is counted. A section asks for what it
            # shows only while it is shown, and the Atlas asks in steps (the
            # history, HEAD, its symbols, each snapshot), so on a slow
            # machine one walk can end before the page has drawn all it
            # will. The warm-up walks again until a walk leaves the counts
            # where the one before left them, at most WARM_WALKS times; a
            # page that grows on every walk grows in the counted walks too,
            # and fails there.
            task(d, 0)
            levels = page.run(WALK)
            page.settle(2)
            start = page.counts()
            for _ in range(WARM_WALKS - 1):
                page.run(WALK)
                page.settle(2)
                again = page.counts()
                if again == start:
                    break
                start = again
            heap0 = page.heap()
            print(f"smoke: warm-up done; {levels} levels; counts {start}")
            for _ in range(cycles):
                page.run(WALK)
            page.settle(2)
            walked = page.counts()
            print(f"smoke: after {cycles} walks: {walked}")
            failures += judged(f"{cycles} walks in and out of every level", start, walked,
                               ("documents", "nodes", "jsEventListeners"))
            for n in range(1, prompts + 1):
                task(d, n)
            page.settle(2)
            ran = page.counts()
            print(f"smoke: after {prompts} tasks: {ran} (nodes {ran['nodes'] - start['nodes']:+d}, "
                  "transcript rows the page keeps up to its cap; not judged)")
            failures += judged(f"{prompts} tasks to run_end", start, ran, ("documents", "jsEventListeners"))
            heap1 = page.heap()
            if heap0 is not None and heap1 is not None:
                print(f"smoke: JS heap {heap0} -> {heap1} bytes (a trend, not judged)")
            page.settle(1)
            failures += page.errors
        finally:
            if chrome is not None:
                chrome.close()
            d.close()
    return failures


# --- The self-test ----------------------------------------------------------

PAGES = {
    # Each page's cycle() does what the desktop's walk does, as the case
    # says: the clean page adds a listener and removes it; the leaking one
    # never removes it; the noisy one logs an error.
    "clean.html": """<!doctype html><title>clean</title><link rel="icon" href="data:,"><body><main id="m"></main><script>
function cycle() {
  const m = document.getElementById("m");
  const b = document.createElement("button");
  const ac = new AbortController();
  b.addEventListener("click", () => {}, { signal: ac.signal });
  m.replaceChildren(b);
  ac.abort();
  m.replaceChildren();
}
</script>""",
    "leak.html": """<!doctype html><title>leak</title><link rel="icon" href="data:,"><body><main id="m"></main><script>
const kept = [];
function cycle() {
  const b = document.createElement("button");
  b.addEventListener("click", () => {});
  kept.push(b);
}
</script>""",
    "error.html": """<!doctype html><title>error</title><link rel="icon" href="data:,"><body><main id="m"></main><script>
function cycle() { console.error("planted: a module failed to start"); }
</script>""",
}


class _Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *_):
        pass


def check_page(page, url, cycles):
    """The desktop run's checks over one self-test page: its errors, and
    the three counts over `cycles` cycles."""
    page.errors.clear()
    page.open(url)
    page.run("cycle()")
    page.settle(0.5)
    start = page.counts()
    for _ in range(cycles):
        page.run("cycle()")
    page.settle(0.5)
    return judged("cycles", start, page.counts(), ("documents", "nodes", "jsEventListeners")) + page.errors


def self_test(chrome_bin):
    site = tempfile.mkdtemp(prefix="trellis-smoke-site-")
    for name, html in PAGES.items():
        pathlib.Path(site, name).write_text(html, encoding="utf-8")
    server = http.server.ThreadingHTTPServer(
        ("127.0.0.1", 0), lambda *a, **k: _Quiet(*a, directory=site, **k))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_address[1]}/"
    chrome = Chrome(chrome_bin)
    bad = []
    try:
        page = Page(chrome)
        clean = check_page(page, base + "clean.html", 5)
        leak = check_page(page, base + "leak.html", 5)
        noisy = check_page(page, base + "error.html", 1)
        if clean:
            bad.append(f"the clean page failed: {clean}")
        if not any("jsEventListeners" in f for f in leak):
            bad.append(f"a listener added every cycle and never removed passed: {leak}")
        if not any("console.error: planted" in f for f in noisy):
            bad.append(f"a console error passed: {noisy}")
    finally:
        chrome.close()
        server.shutdown()
        server.server_close()
        shutil.rmtree(site, ignore_errors=True)
    for b in bad:
        print(f"FAIL: self-test: {b}")
    if bad:
        return 1
    print("OK: self-test (a planted console error fails, a listener never removed fails, a clean page passes)")
    return 0


def find_chrome(given):
    if given:
        if shutil.which(given) or os.path.isfile(given):
            return shutil.which(given) or given
        raise Missing(f"no Chrome at {given}")
    for name in ("google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "chrome"):
        found = shutil.which(name)
        if found:
            return found
    raise Missing("no Chrome on PATH (google-chrome, chromium); pass --chrome")


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("--chrome")
    ap.add_argument("--desktop", default=str(ROOT / "desktop" / "target" / "debug" / "trellis-desktop"))
    ap.add_argument("--trellis", default=str(ROOT / "target" / "debug" / "trellis"))
    ap.add_argument("--prompts", type=int, default=5)
    ap.add_argument("--cycles", type=int, default=5)
    a = ap.parse_args(argv)
    if os.name != "posix":
        print("smoke: the DevTools pipe is fds 3 and 4, which this platform lacks; run it on Linux or macOS")
        return 2
    try:
        chrome = find_chrome(a.chrome)
        if a.self_test:
            return self_test(chrome)
        for what, path in (("trellis-desktop", a.desktop), ("trellis", a.trellis)):
            if not os.path.isfile(path):
                raise Missing(f"no {what} at {path}; build it first")
        failures = smoke(chrome, os.path.abspath(a.desktop), os.path.abspath(a.trellis), a.prompts, a.cycles)
    except Missing as e:
        print(f"smoke: {e}")
        return 2
    for f in failures:
        print(f"FAIL: {f}")
    if failures:
        return 1
    print(f"OK: the desktop page ran {a.prompts} tasks and {a.cycles} walks with no error, "
          "and its listeners, nodes and documents came back")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
