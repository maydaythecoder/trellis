package rpc

import (
	"bufio"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

// TestMain lets the test binary stand in for trellis: started as
// `<test binary> --root <dir> rpc` with <dir> ending in "fake-nonjson", it
// prints a line that is not JSON, then a JSON line, then answers quit and
// exits. This keeps a fake child cross-platform without a shell script and
// without changing any child's environment (go-guards.sh `cmd-env`).
func TestMain(m *testing.M) {
	if len(os.Args) == 4 && os.Args[1] == "--root" && os.Args[3] == "rpc" &&
		strings.HasSuffix(os.Args[2], "fake-nonjson") {
		fakeTrellis()
		return
	}
	os.Exit(m.Run())
}

func fakeTrellis() {
	fmt.Println("trellis: not a JSON line")
	fmt.Println(`{"type":"response","command":"hello","id":"0","success":true}`)
	fmt.Fprintln(os.Stderr, "human text on stderr")
	in := bufio.NewScanner(os.Stdin)
	for in.Scan() {
		if strings.Contains(in.Text(), `"quit"`) {
			fmt.Println(`{"type":"response","command":"quit","success":true}`)
			os.Exit(0)
		}
	}
	os.Exit(3)
}

// trellisBin is target/debug/trellis, built by `cargo build -p
// trellis-cli`. A test that needs it fails asking for it rather than
// skipping, as the desktop's tests do (#426).
func trellisBin(t *testing.T) string {
	t.Helper()
	name := "trellis"
	if runtime.GOOS == "windows" {
		name += ".exe"
	}
	p, err := filepath.Abs(filepath.Join("..", "..", "..", "target", "debug", name))
	if err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(p); err != nil {
		t.Fatalf("%s not found; run `cargo build -p trellis-cli` first: %v", p, err)
	}
	return p
}

// next reads one line, or fails the test after a bound.
func next(t *testing.T, c *Client) Line {
	t.Helper()
	select {
	case l, ok := <-c.Lines():
		if !ok {
			t.Fatal("stdout closed before the expected line")
		}
		return l
	case <-time.After(60 * time.Second):
		t.Fatal("no stdout line within 60 s")
	}
	return Line{}
}

// A get_state reply arrives from the real binary as one JSON line, carrying
// the id it was sent with. TRELLIS_CONFIG_DIR points at an empty directory
// so the test never reads the developer's own configuration.
func TestRPCGetStateReplyIsOneJSONLine(t *testing.T) {
	t.Setenv("TRELLIS_CONFIG_DIR", t.TempDir())
	c, err := Start(trellisBin(t), t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	defer c.Shutdown()
	if err := c.Send(map[string]any{"id": "s1", "type": "get_state"}); err != nil {
		t.Fatal(err)
	}
	l := next(t, c)
	if !l.Parsed() {
		t.Fatalf("get_state reply is not one JSON line: %q", l.Text)
	}
	if l.Type() != "response" || l.JSON["command"] != "get_state" || l.JSON["id"] != "s1" || l.JSON["success"] != true {
		t.Fatalf("unexpected reply: %s", l.Text)
	}
	if _, ok := l.JSON["data"].(map[string]any); !ok {
		t.Fatalf("get_state reply has no data object: %s", l.Text)
	}
}

// Shutdown sends quit and the child exits by itself, with status 0: a kill
// would end it by a signal, or on Windows with status 1, and skip the
// rollback quit waits for (ADR 0048 section 9).
func TestRPCShutdownSendsQuitAndTheChildExitsByItself(t *testing.T) {
	t.Setenv("TRELLIS_CONFIG_DIR", t.TempDir())
	c, err := Start(trellisBin(t), t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	if err := c.Quit(); err != nil {
		t.Fatal(err)
	}
	l := next(t, c)
	if l.Type() != "response" || l.JSON["command"] != "quit" || l.JSON["success"] != true {
		t.Fatalf("expected the quit response, got %q", l.Text)
	}
	done := make(chan error, 1)
	go func() { done <- c.Wait() }()
	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("the child did not exit cleanly after quit: %v", err)
		}
	case <-time.After(60 * time.Second):
		t.Fatal("the child did not exit within 60 s of quit")
	}
	st := c.cmd.ProcessState
	if st == nil || !st.Exited() || st.ExitCode() != 0 {
		t.Fatalf("child state after quit: %v", st)
	}
	if err := c.Send(map[string]any{"type": "get_state"}); err != ErrQuit {
		t.Fatalf("Send after Quit: got %v, want ErrQuit", err)
	}
}

// A stdout line that is not JSON is delivered as text, in order, not
// dropped; stderr is kept as text and never delivered as a stdout line.
func TestRPCNonJSONLineIsDeliveredAsText(t *testing.T) {
	self, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	root := filepath.Join(t.TempDir(), "fake-nonjson")
	if err := os.Mkdir(root, 0o755); err != nil {
		t.Fatal(err)
	}
	c, err := Start(self, root)
	if err != nil {
		t.Fatal(err)
	}
	first := next(t, c)
	if first.Parsed() || first.Text != "trellis: not a JSON line" {
		t.Fatalf("first line: parsed=%v text=%q; want the unparsed text", first.Parsed(), first.Text)
	}
	second := next(t, c)
	if second.Type() != "response" || second.JSON["command"] != "hello" {
		t.Fatalf("second line: %q", second.Text)
	}
	if err := c.Quit(); err != nil {
		t.Fatal(err)
	}
	third := next(t, c)
	if third.JSON["command"] != "quit" {
		t.Fatalf("third line: %q", third.Text)
	}
	if err := c.Wait(); err != nil {
		t.Fatalf("fake child: %v", err)
	}
	errs, lost := c.Stderr()
	if len(errs) != 1 || errs[0] != "human text on stderr" || lost != 0 {
		t.Fatalf("stderr kept: %q (lost %d)", errs, lost)
	}
}

// ParseLine keeps text that is JSON but not an object as text, so a bare
// number or array is never taken for a frame.
func TestRPCParseLineKeepsNonObjectsAsText(t *testing.T) {
	for _, s := range []string{"42", `["a"]`, `"s"`, "null", "", "{"} {
		if l := ParseLine(s + "\n"); l.Parsed() || l.Text != s {
			t.Errorf("ParseLine(%q) = parsed %v, text %q", s, l.Parsed(), l.Text)
		}
	}
	if l := ParseLine("{\"type\":\"turn\"}\r\n"); l.Type() != "turn" {
		t.Errorf("CRLF object not parsed: %q", l.Text)
	}
}
