package model

import (
	"fmt"
	"sort"
	"strings"

	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// Slash commands are addressed to this view and start no run. Each answer
// is trellis's: the model and tier from get_state, the module graph from
// get_codebase, a symbol's blast radius from get_radius. The view holds no
// index and reads no file.

// local answers a line this view handles itself, and reports whether it
// did. A line that is neither a /command nor `trellis ...` is a task.
func (m *Model) local(line string) bool {
	// A CLI invocation typed at a prompt that is not a shell: caught by
	// name, since `trellis` is the word a user reaches for here, having just
	// come from the command line.
	if rest, ok := strings.CutPrefix(line, "trellis "); ok {
		sub := ""
		if f := strings.Fields(rest); len(f) > 0 {
			sub = f[0]
		}
		m.push(view.RoleNote, "· `"+view.Plain(line)+"` is a shell command, and this is not a shell.")
		m.push(view.RoleNote, "  Leave with esc and run it, or use /"+view.Plain(sub)+" if this view has one — /help lists what it does.")
		return true
	}
	if !strings.HasPrefix(line, "/") {
		return false
	}
	fields := strings.Fields(line[1:])
	name := ""
	if len(fields) > 0 {
		name = fields[0]
	}
	args := fields[min(1, len(fields)):]
	if name == "clear" {
		// Following again: a scroll back kept from the cleared transcript
		// pointed above an empty pane, and the line saying it was cleared
		// was not in view (#855).
		m.Transcript = nil
		m.ScrollBack = 0
		m.push(view.RoleNote, "· transcript cleared")
		return true
	}
	m.push(view.RoleYou, "> "+view.Plain(line))
	switch name {
	case "help", "?":
		for _, l := range helpLines {
			m.push(view.RoleText, l)
		}
	case "model":
		m.ask("get_state", forModel, nil)
	case "graph", "radius":
		if len(args) > 0 {
			m.ask("get_radius", forRadius, map[string]any{"symbol": args[0]})
		} else {
			m.radiusLines()
		}
	case "codebase":
		m.ask("get_codebase", forCodebase, nil)
	default:
		m.push(view.RoleNote, "· `/"+view.Plain(name)+"` is not a command here. /help lists them.")
	}
	return true
}

var helpLines = []string{
	"· this view runs one task at a time against the model in the status bar.",
	"  /model           what is driving runs",
	"  /graph           the last run's blast radius, and what it does not claim",
	"  /radius <symbol> a symbol's blast radius, from trellis's index",
	"  /codebase        which modules call which, and how much is unbound",
	"  /clear           empty the transcript",
	"  /help            this",
	helpLine("@<text> "+view.Keycap("TAB"), "repository paths starting with it; "+view.Keycap("TAB")+" again inserts the first"),
	helpLine(view.Keycap("TAB"), "select a pane; "+view.Keycap("⏎")+" on an empty line goes into it"),
	helpLine(view.Keycap("ESC"), "out of a pane, or clear the line; it never stops a run"),
	helpLine(view.Keycap("^C"), "leave; a run in flight is stopped and rolled back first"),
	helpLine(view.Keycap("⏎"), "during a run, queues the line as a steer for its next turn"),
	helpLine(view.Keycap("^P"), "during a run, pause it before its next model call, or resume it"),
	helpLine(view.Keycap("^R"), "during a run, raise its turns, wall clock or completion tokens ceiling"),
	helpLine(view.Keycap("^T"), "during a run, take back the newest steer or follow-up still queued"),
	helpLine(view.Keycap("^E"), "in the plan, edit it; your version goes to the model as a steer"),
	helpLine(view.Keycap("^F"), "show only your rows, tools, checks or notes, then all again; turn dividers always show"),
	helpLine(view.Keycap("^N"), "quiet: hide the calls that wrote nothing and what calls returned; ^N again shows them"),
	helpLine(view.Keycap("^O"), "the newest turn on screen: its call as the model wrote it and what it returned, in full"),
	helpLine(view.Keycap("↑")+view.Keycap("↓"), "scroll the transcript"),
	helpLine(view.Keycap("PGUP")+view.Keycap("PGDN"), "ten lines at a time; "+view.Keycap("HOME")+" to the start, "+view.Keycap("END")+" back to following"),
	"",
	// What the screen's marks mean (#911).
	helpLine("1:23*", "a time by this view's own clock, which stops while a question waits or the run is paused"),
	helpLine(view.Dash, "a figure trellis did not report; never drawn as 0"),
	helpLine("████░░░░ 3/40", "a figure against its ceiling, the turn against max_iterations"),
	helpLine("● 2 pending", "edits a run in flight has made in the worktree"),
	helpLine("✎ 2", "actions that wrote to the tree"),
	"",
	"  A question is answered here; anything else is a task: it snapshots the",
	"  worktree, drives the model, runs the repository's own checks and rolls",
	"  back if it does not converge.",
	"",
	"  For the model catalogue, weights and config, leave and use",
	"  `trellis model` or `trellis session`.",
}

// helpLine is one /help row: what is on screen or the key to press, in
// the column the commands above use, and what it means or does.
func helpLine(what, means string) string {
	return "  " + view.Pad(what, 16) + " " + means
}

// radiusLines is /graph: the radius pane's three counts, in words.
func (m *Model) radiusLines() {
	if m.Radius == nil {
		m.push(view.RoleText, "· reach: nothing yet — a task that calls blast_radius fills it.")
	} else {
		r := m.Radius
		m.push(view.RoleText, fmt.Sprintf("· reach: %d resolved · %d candidates · %d unresolved", r.Resolved, r.Candidates, r.Unresolved))
		m.push(view.RoleText, "  drawn in the radius pane on the right, as converging edges.")
	}
	m.neverARatio()
}

// Never a ratio between the counts: they come from different populations,
// and dividing them printed "3.7% resolved" for a resolver audited at 72%
// (ADR 0014).
func (m *Model) neverARatio() {
	m.push(view.RoleText, "  separate counts, never a ratio: they come from different populations.")
	m.push(view.RoleText, "  resolved is a floor on reach, not a boundary (Constraint 1).")
}

// modelAnswered is /model, from get_state.
func (m *Model) modelAnswered(data any) {
	m.stateAnswered(data)
	model, tier := m.ModelName, m.Tier
	if model == "" {
		model = "none chosen"
	}
	if tier == "" {
		tier = "probing…"
	}
	m.push(view.RoleText, "· model  "+model)
	m.push(view.RoleText, "  tier   "+tier)
	m.push(view.RoleText, "  `trellis model` outside this view lists the weights you can fetch.")
}

// radiusAnswered is /radius <symbol>, from get_radius: reach and doubt as
// counts of their own.
func (m *Model) radiusAnswered(data any) {
	d, _ := data.(map[string]any)
	reach, _ := d["reach"].(map[string]any)
	doubt, _ := d["doubt"].(map[string]any)
	sym, _ := d["symbol"].(string)
	m.push(view.RoleText, fmt.Sprintf("· %s: reach %s resolved · %s ambiguous · %s symbols reached",
		view.Plain(sym), count(reach["resolved"]), count(reach["ambiguous"]), count(reach["symbols_reached"])))
	m.push(view.RoleText, fmt.Sprintf("  doubt: %s unbound call sites in %s touched files",
		count(doubt["unbound_in_touched_files"]), count(doubt["touched_files"])))
	m.neverARatio()
}

// codebaseAnswered is /codebase, from get_codebase: the busiest modules
// and whom they call, with bound, ambiguous and unresolved as separate
// counts.
func (m *Model) codebaseAnswered(data any) {
	d, _ := data.(map[string]any)
	m.push(view.RoleText, fmt.Sprintf("· codebase graph — %s module(s)", count(d["modules_indexed"])))
	m.push(view.RoleText, fmt.Sprintf("  %s bound call sites · %s ambiguous · %s unresolved",
		count(d["bound_call_sites"]), count(d["ambiguous"]), count(d["unresolved"])))
	mods, _ := d["modules"].([]any)
	for _, mv := range mods {
		mod, _ := mv.(map[string]any)
		name, _ := mod["module"].(string)
		var calls []string
		cs, _ := mod["calls"].([]any)
		for _, cv := range cs {
			c, _ := cv.(map[string]any)
			callee, _ := c["module"].(string)
			calls = append(calls, fmt.Sprintf("%s (%s)", view.Plain(callee), count(c["call_sites"])))
		}
		sort.Strings(calls)
		line := fmt.Sprintf("  %s  defines %s", view.Plain(name), count(mod["defines"]))
		if len(calls) > 0 {
			line += " · calls " + strings.Join(calls, ", ")
		}
		m.push(view.RoleText, line)
	}
	if n := count(d["modules_not_drawn"]); n != "0" && n != "?" {
		m.push(view.RoleText, "  "+n+" more module(s) not listed")
	}
}

// count writes a whole-number JSON value, or "?" for one that is not:
// never a 0 standing in for a figure trellis did not send.
func count(v any) string {
	f, ok := v.(float64)
	if !ok || f < 0 || f != float64(int64(f)) {
		return "?"
	}
	return fmt.Sprintf("%d", int64(f))
}
