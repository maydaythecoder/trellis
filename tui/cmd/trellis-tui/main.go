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
	m := model.New(model.Options{Task: a.task, ModelSpec: a.model})
	app := model.NewApp(m, c)
	// Nothing paints with the theme yet. Handing it to the model changes
	// internal/model/model.go and internal/view, which #839 is changing;
	// the screen paints with it from #841.
	_ = th
	if _, err := tea.NewProgram(app, tea.WithAltScreen()).Run(); err != nil {
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
