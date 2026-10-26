// Command trellis-tui is the Go TUI, a client of `trellis rpc` (ADR 0052).
//
//	trellis-tui --root <repo> [--trellis <path>] [--model <spec>] [--theme auto|dark|light] [task]
//	trellis-tui --version
//
// It starts one `trellis --root <repo> rpc` child, using the trellis it is
// given or, when none is given, the trellis beside its own executable; never
// one found on PATH, which could be some other build. Every run, check and
// rollback is that child's: this program sends commands and draws frames.
//
// Before the child starts it reads the terminal's colour depth and
// background once, for the theme (internal/theme). --theme dark or light
// replaces the terminal's answer, since Windows always answers dark.
//
// The flow: run parses the flags, starts the child (internal/rpc) and hands
// it to the Bubble Tea program (internal/model's App, which feeds the
// child's lines and the keys to the Model and draws with internal/view).
// However the program ends, finish then sends the child quit and waits for
// it unless it has exited, so a run in flight rolls back and writes its
// trail, and says what ended the program.
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
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"github.com/maydaythecoder/trellis/tui/internal/model"
	"github.com/maydaythecoder/trellis/tui/internal/rpc"
	"github.com/maydaythecoder/trellis/tui/internal/theme"
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
	theme   string
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
	fs.StringVar(&a.theme, "theme", "auto", "the background the colours are chosen for: auto (ask the terminal), dark or light")
	if err := fs.Parse(argv); err != nil {
		return a, err
	}
	a.task = strings.TrimSpace(strings.Join(fs.Args(), " "))
	if a.version {
		return a, nil
	}
	switch a.theme {
	case "auto", "dark", "light":
	default:
		return a, fmt.Errorf("--theme is auto, dark or light, not %q", a.theme)
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

// setBackground sets the background r's colours are chosen for from
// --theme: auto takes what the terminal answered, and dark or light answers
// for it, since Windows always answers dark. The answer is then set on r,
// so nothing asks the terminal again: asking is an OSC 11 query, and once
// Bubble Tea is reading the terminal the reply would arrive as keys.
func setBackground(r *lipgloss.Renderer, background string) {
	switch background {
	case "dark":
		r.SetHasDarkBackground(true)
	case "light":
		r.SetHasDarkBackground(false)
	default:
		r.SetHasDarkBackground(r.HasDarkBackground())
	}
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
	// The terminal's colour depth and background, read once, before the
	// child starts and before Bubble Tea owns the terminal. Lip Gloss's
	// default renderer writes to the terminal Bubble Tea draws on, and
	// Bubble Tea's own init has already asked it for the background through
	// that renderer, so the theme is built from it rather than asking again.
	r := lipgloss.DefaultRenderer()
	r.SetColorProfile(r.ColorProfile())
	setBackground(r, a.theme)
	th := theme.New(r)
	c, err := rpc.Start(a.trellis, root)
	if err != nil {
		fmt.Fprintln(stderr, "trellis-tui:", err)
		return 1
	}
	m := model.New(model.Options{Task: a.task, ModelSpec: a.model, Theme: th})
	app := model.NewApp(m, c)
	_, err = tea.NewProgram(app, tea.WithAltScreen()).Run()
	// The program has ended, on whatever path: what it left waiting on the
	// child's lines and the clock ends with it (#847).
	app.Close()
	return finish(c, m.Quitting, err, stderr, shutdownWithin)
}

// child is what finish needs of the `trellis rpc` child: *rpc.Client, or a
// test's stand-in.
type child interface {
	Exited() <-chan struct{}
	Shutdown() error
	Stderr() ([]string, int)
}

// shutdownWithin is how long finish waits for the child after quit. A run
// in flight aborts, rolls back and writes its trail first, which takes
// seconds; past this the program exits and leaves the child to finish on
// its own, since nothing here kills it (ADR 0052).
const shutdownWithin = 30 * time.Second

// finish is every way out of the program once the child has started, and
// the exit code. Run returns on four paths: the person quit and the child
// exited (0); the child exited without being asked; the screen failed; or a
// signal ended the program, which Bubble Tea turns into a quit with no
// error (SIGTERM) or ErrInterrupted (SIGINT). Unless the child has exited,
// it is sent quit and waited for, within `within`, so a run in flight rolls
// back and writes its trail; and the message names what happened, rather
// than blaming the child for a signal (#845).
func finish(c child, quitting bool, runErr error, stderr io.Writer, within time.Duration) int {
	select {
	case <-c.Exited():
		switch {
		case runErr != nil:
			fmt.Fprintln(stderr, "trellis-tui:", runErr)
			return 1
		case quitting:
			return 0
		}
		fmt.Fprintln(stderr, "trellis-tui: trellis rpc exited without being asked to quit; its last messages:")
		lines, dropped := c.Stderr()
		if dropped > 0 {
			fmt.Fprintf(stderr, "(%d earlier lines not kept)\n", dropped)
		}
		for _, l := range lines {
			fmt.Fprintln(stderr, view.Plain(l))
		}
		return 1
	default:
	}
	switch {
	case runErr != nil:
		// The screen failed, or SIGINT interrupted the program
		// (tea.ErrInterrupted, "program was interrupted"): not trellis.
		fmt.Fprintln(stderr, "trellis-tui:", runErr)
	case quitting:
		fmt.Fprintln(stderr, "trellis-tui: stopped before trellis rpc finished quitting")
	default:
		fmt.Fprintln(stderr, "trellis-tui: stopped by a signal")
	}
	fmt.Fprintln(stderr, "trellis-tui: asking trellis rpc to quit; a run in flight rolls back first")
	done := make(chan error, 1)
	go func() { done <- c.Shutdown() }()
	timer := time.NewTimer(within)
	defer timer.Stop()
	select {
	case serr := <-done:
		if serr != nil {
			fmt.Fprintln(stderr, "trellis-tui:", serr)
			return 1
		}
	case <-timer.C:
		fmt.Fprintf(stderr, "trellis-tui: trellis rpc had not exited %s after quit; it is left to finish, not killed\n", within)
		return 1
	}
	// The person had asked to quit, and the child has now exited cleanly.
	if quitting && runErr == nil {
		return 0
	}
	return 1
}
