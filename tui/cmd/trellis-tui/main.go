// Command trellis-tui is the Go TUI, a client of `trellis rpc` (ADR 0052).
//
//	trellis-tui --root <repo> [--trellis <path>] [--model <spec>] [task]
//	trellis-tui --version
//
// It starts one `trellis --root <repo> rpc` child, using the trellis it is
// given or, when none is given, the trellis beside its own executable; never
// one found on PATH, which could be some other build. Every run, check and
// rollback is that child's: this program sends commands and draws frames.
package main

import (
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"runtime"
	"strings"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/maydaythecoder/trellis/tui/internal/model"
	"github.com/maydaythecoder/trellis/tui/internal/rpc"
	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// version is set at build time with
// `-ldflags "-X main.version=<version>"`; a plain build says dev.
var version = "dev"

func main() {
	os.Exit(run(os.Args[1:], os.Stdout, os.Stderr))
}

// args is what the command line asked for.
type args struct {
	version bool
	root    string
	trellis string
	model   string
	task    string
}

func parseArgs(argv []string, stderr io.Writer) (args, error) {
	var a args
	fs := flag.NewFlagSet("trellis-tui", flag.ContinueOnError)
	fs.SetOutput(stderr)
	fs.BoolVar(&a.version, "version", false, "print the version and exit")
	fs.StringVar(&a.root, "root", "", "the repository trellis works on (required)")
	fs.StringVar(&a.trellis, "trellis", "", "the trellis binary (default: the one beside this executable)")
	fs.StringVar(&a.model, "model", "", "the model spec for this session, sent as set_model")
	if err := fs.Parse(argv); err != nil {
		return a, err
	}
	a.task = strings.TrimSpace(strings.Join(fs.Args(), " "))
	if a.version {
		return a, nil
	}
	if a.root == "" {
		return a, errors.New("--root <repo> is required")
	}
	return a, nil
}

// siblingTrellis is trellis beside this executable.
func siblingTrellis() (string, error) {
	exe, err := os.Executable()
	if err != nil {
		return "", fmt.Errorf("cannot find this executable to look beside it: %w", err)
	}
	name := "trellis"
	if runtime.GOOS == "windows" {
		name += ".exe"
	}
	return filepath.Join(filepath.Dir(exe), name), nil
}

func run(argv []string, stdout, stderr io.Writer) int {
	a, err := parseArgs(argv, stderr)
	if err != nil {
		if errors.Is(err, flag.ErrHelp) {
			return 0
		}
		fmt.Fprintln(stderr, "trellis-tui:", err)
		return 2
	}
	if a.version {
		fmt.Fprintf(stdout, "trellis-tui %s\n", version)
		return 0
	}
	if a.trellis == "" {
		if a.trellis, err = siblingTrellis(); err != nil {
			fmt.Fprintln(stderr, "trellis-tui:", err)
			return 1
		}
	}
	if _, err := os.Stat(a.trellis); err != nil {
		fmt.Fprintf(stderr, "trellis-tui: no trellis at %s: %v\n", a.trellis, err)
		return 1
	}
	root, err := filepath.Abs(a.root)
	if err != nil {
		fmt.Fprintln(stderr, "trellis-tui:", err)
		return 1
	}
	c, err := rpc.Start(a.trellis, root)
	if err != nil {
		fmt.Fprintln(stderr, "trellis-tui:", err)
		return 1
	}
	m := model.New(model.Options{Task: a.task, ModelSpec: a.model})
	app := model.NewApp(m, c)
	_, err = tea.NewProgram(app, tea.WithAltScreen()).Run()
	// The program has ended, on whatever path: what it left waiting on the
	// child's lines and the clock ends with it (#847).
	app.Close()
	if err != nil {
		fmt.Fprintln(stderr, "trellis-tui:", err)
		// The screen failed, not trellis: still quit and wait, so a run in
		// flight rolls back rather than being left to a killed process.
		if serr := c.Shutdown(); serr != nil {
			fmt.Fprintln(stderr, "trellis-tui:", serr)
		}
		return 1
	}
	if !m.Quitting {
		// The child exited without being asked; say why on the way out.
		lines, _ := c.Stderr()
		for _, l := range lines {
			fmt.Fprintln(stderr, view.Plain(l))
		}
		return 1
	}
	return 0
}
