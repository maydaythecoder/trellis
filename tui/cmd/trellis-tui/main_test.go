package main

import (
	"bytes"
	"strings"
	"testing"

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
