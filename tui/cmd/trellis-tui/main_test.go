package main

import (
	"bytes"
	"errors"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/charmbracelet/lipgloss"
)

// --version prints the build-time version, `dev` by default, and exits 0.
func TestMainVersionPrintsTheBuildVersion(t *testing.T) {
	var out, errb bytes.Buffer
	if code := run([]string{"--version"}, &out, &errb); code != 0 {
		t.Fatalf("exit %d: %s", code, errb.String())
	}
	if got := out.String(); got != "trellis-tui dev\n" {
		t.Fatalf("got %q", got)
	}
}

// Without --root nothing starts: there is no repository to run against.
func TestMainRootIsRequired(t *testing.T) {
	var out, errb bytes.Buffer
	if code := run(nil, &out, &errb); code != 2 || !strings.Contains(errb.String(), "--root") {
		t.Fatalf("exit %d, stderr %q", code, errb.String())
	}
}

// A trellis that does not exist is refused before any screen starts.
func TestMainMissingTrellisIsRefused(t *testing.T) {
	var out, errb bytes.Buffer
	code := run([]string{"--root", t.TempDir(), "--trellis", "/nonexistent/trellis"}, &out, &errb)
	if code == 0 || !strings.Contains(errb.String(), "no trellis at") {
		t.Fatalf("exit %d, stderr %q", code, errb.String())
	}
}

// --theme dark or light sets the background the colours are chosen for,
// over what the terminal answers: Windows always answers dark, so without
// it a light terminal is painted for a dark one. A renderer on a writer
// that cannot be asked answers dark, as Windows does, which is what light
// has to override.
func TestMainThemeOverridesTheBackground(t *testing.T) {
	for flag, dark := range map[string]bool{"auto": true, "dark": true, "light": false} {
		r := lipgloss.NewRenderer(&bytes.Buffer{})
		setBackground(r, flag)
		if got := r.HasDarkBackground(); got != dark {
			t.Errorf("--theme %s: dark background %v, want %v", flag, got, dark)
		}
	}
	if a, err := parseArgs([]string{"--root", "r"}, &bytes.Buffer{}); err != nil || a.theme != "auto" {
		t.Fatalf("without --theme: %+v %v", a, err)
	}
	if a, err := parseArgs([]string{"--root", "r", "--theme", "light"}, &bytes.Buffer{}); err != nil || a.theme != "light" {
		t.Fatalf("--theme light: %+v %v", a, err)
	}
	var out, errb bytes.Buffer
	if code := run([]string{"--root", t.TempDir(), "--theme", "sepia"}, &out, &errb); code != 2 || !strings.Contains(errb.String(), "--theme") {
		t.Fatalf("--theme sepia: exit %d, stderr %q", code, errb.String())
	}
}

// The words after the flags are the task.
func TestMainTaskIsTheRemainingWords(t *testing.T) {
	a, err := parseArgs([]string{"--root", "r", "--model", "m", "fix", "the", "bug"}, &bytes.Buffer{})
	if err != nil || a.task != "fix the bug" || a.model != "m" || a.root != "r" {
		t.Fatalf("%+v %v", a, err)
	}
}

// fakeChild stands in for the trellis rpc child. Shutdown exits it, or, with
// hang set, waits until the test ends, as a child that never exits would.
type fakeChild struct {
	exited    chan struct{}
	hang      chan struct{}
	shutdowns atomic.Int32
	stderr    []string
	dropped   int
}

func newFakeChild(t *testing.T, hang bool) *fakeChild {
	f := &fakeChild{exited: make(chan struct{})}
	if hang {
		f.hang = make(chan struct{})
		t.Cleanup(func() { close(f.hang) })
	}
	return f
}

func (f *fakeChild) Exited() <-chan struct{} { return f.exited }

func (f *fakeChild) Shutdown() error {
	f.shutdowns.Add(1)
	if f.hang != nil {
		<-f.hang
		return nil
	}
	close(f.exited)
	return nil
}

func (f *fakeChild) Stderr() ([]string, int) { return f.stderr, f.dropped }

// #845: Bubble Tea turns SIGTERM into a quit and Run returns no error, and
// main then exited 1 without sending quit or waiting for the child, saying
// the child had exited. finish sends quit and waits on every path where the
// child is still running, and names what happened.
func TestMainFinishShutsDownWhenTheChildHasNotExited(t *testing.T) {
	cases := []struct {
		name     string
		quitting bool
		runErr   error
		code     int
		says     string
	}{
		{"a signal", false, nil, 1, "stopped by a signal"},
		{"SIGINT", false, errors.New("program was interrupted"), 1, "program was interrupted"},
		{"the screen failed", false, errors.New("could not open a TTY"), 1, "could not open a TTY"},
		{"a signal while quitting", true, nil, 0, "stopped before trellis rpc finished quitting"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			c := newFakeChild(t, false)
			var errb bytes.Buffer
			code := finish(c, tc.quitting, tc.runErr, &errb, time.Minute)
			if code != tc.code {
				t.Fatalf("exit %d, want %d: %s", code, tc.code, errb.String())
			}
			if n := c.shutdowns.Load(); n != 1 {
				t.Fatalf("Shutdown called %d times, want 1", n)
			}
			got := errb.String()
			if !strings.Contains(got, tc.says) || !strings.Contains(got, "asking trellis rpc to quit") {
				t.Fatalf("stderr %q does not say %q and that it asked trellis to quit", got, tc.says)
			}
			if strings.Contains(got, "exited without being asked") {
				t.Fatalf("stderr %q blames the child", got)
			}
		})
	}
}

// A child that does not exit after quit is waited for only within the
// bound, and left running, never killed: the program says so and exits.
func TestMainFinishWaitsForTheChildWithinItsBound(t *testing.T) {
	c := newFakeChild(t, true)
	var errb bytes.Buffer
	start := time.Now()
	if code := finish(c, false, nil, &errb, 50*time.Millisecond); code != 1 {
		t.Fatalf("exit %d: %s", code, errb.String())
	}
	if took := time.Since(start); took > 10*time.Second {
		t.Fatalf("finish took %s with a bound of 50ms", took)
	}
	if got := errb.String(); !strings.Contains(got, "left to finish, not killed") {
		t.Fatalf("stderr %q", got)
	}
}

// A child that exited by itself is not sent quit; when nobody asked it to
// exit, its last stderr lines are shown as text, and how many were not
// kept. A person who quit and saw it exit gets 0 and no message.
func TestMainFinishSaysWhenTheChildExitedUnasked(t *testing.T) {
	c := newFakeChild(t, false)
	close(c.exited)
	c.stderr = []string{"error: no .git entry at /tmp/x", "bell\x07"}
	c.dropped = 3
	var errb bytes.Buffer
	if code := finish(c, false, nil, &errb, time.Minute); code != 1 {
		t.Fatalf("exit %d", code)
	}
	got := errb.String()
	for _, want := range []string{"exited without being asked", "(3 earlier lines not kept)", "error: no .git entry at /tmp/x"} {
		if !strings.Contains(got, want) {
			t.Fatalf("stderr %q lacks %q", got, want)
		}
	}
	if strings.Contains(got, "\x07") {
		t.Fatalf("a control character reached the terminal: %q", got)
	}
	if n := c.shutdowns.Load(); n != 0 {
		t.Fatalf("Shutdown called %d times on an exited child", n)
	}
	errb.Reset()
	if code := finish(c, true, nil, &errb, time.Minute); code != 0 || errb.Len() != 0 {
		t.Fatalf("a quit that ended cleanly: exit %d, stderr %q", code, errb.String())
	}
}

// The risk (#857): a mistyped --register drawn as serious, or as playful,
// without a word, so the person cannot tell the dial did not take. Serious
// is the default; anything but serious or playful is refused, as --theme's
// values are.
func TestMainRegisterIsSeriousOrPlayful(t *testing.T) {
	if a, err := parseArgs([]string{"--root", "r"}, &bytes.Buffer{}); err != nil || a.reg != "serious" {
		t.Fatalf("without --register: %+v %v", a, err)
	}
	if a, err := parseArgs([]string{"--root", "r", "--register", "playful"}, &bytes.Buffer{}); err != nil || a.reg != "playful" {
		t.Fatalf("--register playful: %+v %v", a, err)
	}
	var out, errb bytes.Buffer
	if code := run([]string{"--root", t.TempDir(), "--register", "cheerful"}, &out, &errb); code != 2 || !strings.Contains(errb.String(), "--register") {
		t.Fatalf("--register cheerful: exit %d, stderr %q", code, errb.String())
	}
}

// The risk (#952): a mistyped --control sent to trellis, which refuses it
// only once a prompt is sent, or dropped, so the run takes the
// repository's mode while the person believes they chose one. approve is
// the default, the desktop's; anything but approve or auto is refused
// before the child starts.
func TestMainControlIsApproveOrAuto(t *testing.T) {
	if a, err := parseArgs([]string{"--root", "r"}, &bytes.Buffer{}); err != nil || a.control != "approve" {
		t.Fatalf("without --control: %+v %v", a, err)
	}
	if a, err := parseArgs([]string{"--root", "r", "--control", "auto"}, &bytes.Buffer{}); err != nil || a.control != "auto" {
		t.Fatalf("--control auto: %+v %v", a, err)
	}
	for _, bad := range []string{"Auto", "yolo", ""} {
		var out, errb bytes.Buffer
		if code := run([]string{"--root", t.TempDir(), "--control", bad}, &out, &errb); code != 2 || !strings.Contains(errb.String(), "--control") {
			t.Fatalf("--control %q: exit %d, stderr %q", bad, code, errb.String())
		}
	}
}
