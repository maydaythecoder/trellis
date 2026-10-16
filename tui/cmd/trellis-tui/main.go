// Command trellis-tui is the Go TUI, a client of `trellis rpc` (ADR 0052).
//
// This build has no screen yet: `--version` answers, and every other
// invocation says the TUI is not built and exits non-zero, rather than
// starting something that looks like it works (CLAUDE.md: a command that
// appears to work without doing the work is worse than one that says it is
// not built yet). The screen comes in #508.
package main

import (
	"fmt"
	"io"
	"os"
)

// version is set at build time with
// `-ldflags "-X main.version=<version>"`; a plain build says dev.
var version = "dev"

func main() {
	os.Exit(run(os.Args[1:], os.Stdout, os.Stderr))
}

func run(args []string, stdout, stderr io.Writer) int {
	if len(args) == 1 && args[0] == "--version" {
		fmt.Fprintf(stdout, "trellis-tui %s\n", version)
		return 0
	}
	fmt.Fprintln(stderr, "trellis-tui: the TUI is not built yet; this build answers only --version (#508)")
	return 1
}
