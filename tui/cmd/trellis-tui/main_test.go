package main

import (
	"bytes"
	"testing"
)

// --version prints the build-time version, `dev` by default, and exits 0.
func TestMainVersionPrintsTheBuildVersion(t *testing.T) {
	var out, errb bytes.Buffer
	if code := run([]string{"--version"}, &out, &errb); code != 0 {
		t.Fatalf("exit %d", code)
	}
	if got := out.String(); got != "trellis-tui dev\n" {
		t.Fatalf("got %q", got)
	}
}

// Anything else starts no screen and says the TUI is not built yet,
// exiting non-zero, so nothing appears to work that does not.
func TestMainAnythingElseSaysNotBuilt(t *testing.T) {
	for _, args := range [][]string{nil, {"--root", "."}, {"do something"}} {
		var out, errb bytes.Buffer
		if code := run(args, &out, &errb); code == 0 {
			t.Fatalf("%q exited 0", args)
		}
		if out.Len() != 0 || !bytes.Contains(errb.Bytes(), []byte("not built yet")) {
			t.Fatalf("%q: stdout %q stderr %q", args, out.String(), errb.String())
		}
	}
}
