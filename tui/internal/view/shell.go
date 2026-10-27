package view

import (
	"fmt"
	"strings"
)

// The shell (#851; docs/design/system/faces.md and the Shell card): the
// Trail on the top row, the status bar with the live keys on the last, and
// the stage between, which is all that changes as the person burrows. The
// run view's top level is the transcript and its panes; ⏎ on a pane Tab
// selected goes into it, and ESC comes back out. Inside a pane, the other
// panes collapse to a rail three columns wide down the left, their titles
// written downward.

// Panel is one of the run view's panes, which Tab selects at the top level
// and ⏎ goes into.
type Panel string

const (
	PanelWhere   Panel = "where"
	PanelActions Panel = "actions"
	PanelEdits   Panel = "edits"
	PanelChecks  Panel = "checks"
	PanelRadius  Panel = "radius"
	// PanelSummary is the summary level (#854): no box at the top level,
	// reached from the Now strip's way in, or the panes' row once folded.
	PanelSummary Panel = "summary"
	// PanelPlan is the plan level (#857): the model's latest plan, with no
	// box at the top level either, reached as the summary is.
	PanelPlan Panel = "plan"
)

// Panels is every pane, in the order Tab visits them and the rail lists
// them: the order they stand in the right column, then the plan and the
// summary.
var Panels = []Panel{PanelWhere, PanelActions, PanelEdits, PanelChecks, PanelRadius, PanelPlan, PanelSummary}

// boxless is whether p has no box at the top level: the plan and the
// summary stand in no column, so the rail does not list them.
func boxless(p Panel) bool { return p == PanelPlan || p == PanelSummary }

// Nav is where the person is in the run view.
type Nav struct {
	// Open is the pane gone into from the top level; "" at the top.
	Open Panel
	// Selected is the pane Tab last selected at the top level, which ⏎
	// goes into; "" when none is.
	Selected Panel
	// File is the code level's cursor, the path of the file ⏎ shows the
	// diff of (#853); "" is the first file.
	File string
	// Diff is set at the diff level, one below the edits pane, which
	// shows File's diff from row Scroll down.
	Diff   bool
	Scroll int
}

// Levels is the depth the Trail's gauge counts to, as the desktop's does:
// the run view, a pane, and the two levels below a pane that the code and
// summary levels add (#853, #854).
const Levels = 4

// FoldWidth and FoldHeight are the screen below which the right column
// folds: the transcript takes the whole stage and the panes become one row
// under it, each a way in (#851). At 80 columns the column kept 36 and
// left the transcript 44; under 30 rows the panes below the Trail, the Now
// strip and the status bar had no room for their own rows. The fold is
// the screen's, not the stage's, so an @ list that grows does not fold
// and unfold the panes under the person's eyes.
const (
	FoldWidth  = 100
	FoldHeight = 30
)

// railW is the sibling rail's width inside a pane: a title's letter and a
// column either side of it.
const railW = 3

// folded is whether the right column folds on a w by h screen.
func folded(w, h int) bool { return w < FoldWidth || h < FoldHeight }

// level is the Trail's depth: 1 at the top, 2 inside a pane, 3 at a
// file's diff.
func level(s State) int {
	switch {
	case s.Nav.Open == PanelEdits && s.Nav.Diff:
		return 3
	case s.Nav.Open != "":
		return 2
	}
	return 1
}

// trail is the top row: where the person is, each level after the first
// one more segment, and on the right the depth against the gauge, the key
// that goes back out, and the program's name.
func trail(s State, w int) []Span {
	left := " Run"
	if s.Nav.Open != "" {
		left += " ⟩ " + string(s.Nav.Open)
	}
	if level(s) == 3 {
		left += " ⟩ " + Plain(diffPath(s))
	}
	right := fmt.Sprintf("L%d / %d", level(s), Levels)
	if s.Nav.Open != "" {
		right += "  " + keyRow(key{"ESC", "out"})
	}
	right += "  TRELLIS "
	return spread([]Span{{Text: left, Role: RoleLabel}}, []Span{{Text: right, Role: RoleLabel}}, w)
}

// keycaps are the keys that act at the level shown, for the status bar:
// TAB, ⏎ and the transcript's keys at the top level, ESC and the scroll
// while ^O's call box is up, ESC inside a pane, the cursor and ⏎ at the
// code level, the scroll at a diff, and ^C always. While a question is on
// screen its keys are the ones that act: y and n, and c where it asks to
// write a file, whose diff then answers it in place (#853).
func keycaps(s State) string {
	if q, ok := onScreen(s); ok {
		if s.Reviewing {
			return keyRow(key{"y", "accept"}, key{"n", "reject"}, key{"ESC", "to the question"}, stopKey(s))
		}
		if _, _, file := ApprovalFile(q.Question, q.Detail); file {
			return keyRow(key{"y", "accept"}, key{"n", "reject"}, key{"c", "review in code"}, stopKey(s))
		}
		return keyRow(key{"y", "accept"}, key{"n", "reject"}, stopKey(s))
	}
	if s.CallOpen {
		return keyRow(key{"ESC", "close"}, key{"↑↓", "scroll"}, stopKey(s))
	}
	switch level(s) {
	case 3:
		return keyRow(key{"ESC", "out"}, key{"↑↓", "scroll"}, stopKey(s))
	case 2:
		switch s.Nav.Open {
		case PanelEdits:
			return keyRow(key{"ESC", "out"}, key{"↑↓", "file"}, key{"⏎", "diff"}, stopKey(s))
		case PanelSummary:
			return keyRow(key{"ESC", "out"}, key{"↑↓", "scroll"}, stopKey(s))
		case PanelPlan:
			return planKeys(s)
		}
		return keyRow(key{"ESC", "out"}, stopKey(s))
	}
	return keyRow(append([]key{{"TAB", "select"}, {"⏎", "in"}}, append(logKeys(s), stopKey(s))...)...)
}

// logKeys are the transcript's own keys at the top level (#948): ^F the
// next filter, ^N quiet mode on or off, and ^O while a turn's call is
// drawn for it to open. Which filter is on, and quiet, the transcript's
// title says; a label that changed with them would make the row longer
// in one state than the other, and at 120 columns the longer one did not
// fit beside the worktree.
func logKeys(s State) []key {
	keys := []key{{"^F", "filter"}, {"^N", "quiet"}}
	if HasCall(s) {
		keys = append(keys, key{"^O", "call"})
	}
	return keys
}

// onlyHere is the key the status bar alone shows, kept when the row runs
// short: c under a question that asks to write a file, since the box's
// border has room for y and n only, and at 80 columns c was cut with the
// rest and the code level could not be reached while a question waited
// (#853).
func onlyHere(s State) (key, bool) {
	q, ok := onScreen(s)
	if !ok || s.Reviewing {
		return key{}, false
	}
	_, _, file := ApprovalFile(q.Question, q.Detail)
	return key{"c", "review in code"}, file
}

// onScreen is the question the approval box shows, if one is up.
func onScreen(s State) (Asked, bool) {
	for _, a := range s.Asked {
		if a.OnScreen {
			return a, true
		}
	}
	return Asked{}, false
}

// diffPath is the path the diff level shows: the cursor's file.
func diffPath(s State) string {
	files := codeFiles(s)
	if at, ok := chosen(s, files); ok && (s.Nav.File == "" || files[at].path == s.Nav.File) {
		return files[at].path
	}
	return s.Nav.File
}

// stopKey is ^C, the only key that stops a run (owner decision,
// 2026-10-25): with a run in flight it stops it and then leaves, and with
// none it leaves.
func stopKey(s State) key {
	if s.Running {
		return key{"^C", "stop"}
	}
	return key{"^C", "leave"}
}

// statusBar is the last row: the live keys on the left, and on the right
// the worktree as a Status, the model and the tier (barRight). When the
// row runs short the model is cut first, then the keys give way to the
// one key nothing else on screen shows, if there is one, then to ^C
// alone, then to nothing: what the worktree holds matters more than a key
// the Trail and the strip already imply.
func statusBar(s State, w int) []Span {
	// Two spaces at least between the keys and the Status, so the last key
	// and the worktree's word never read as one phrase.
	const gap = 2
	tries := []string{" " + keycaps(s)}
	if k, ok := onlyHere(s); ok {
		tries = append(tries, " "+keyRow(k))
	}
	for _, keys := range append(tries, " "+keyRow(stopKey(s)), "") {
		right := append(barRight(s, max(w-Width(keys)-gap-1, 1)), Span{Text: " "})
		if Width(keys)+gap+spansWidth(right) <= w {
			return spread([]Span{{Text: keys, Role: RoleLabel}}, right, w)
		}
	}
	return append([]Span{{Text: " "}}, barRight(s, w-1)...)
}

// spread is left and right on one row of w cells, with the room between
// them; when both do not fit, line cuts the right first.
func spread(left, right []Span, w int) []Span {
	gap := max(w-spansWidth(left)-spansWidth(right), 1)
	out := append(append([]Span{}, left...), Span{Text: strings.Repeat(" ", gap)})
	return append(out, right...)
}

// levelWays is the Now strip's ways into the plan and summary levels at
// the top level beside the panes (#854, #857), as the wireframe's "Summary
// so far": each in brackets and in signal, with the key that goes in, once
// Tab selects it. Folded, the panes' row is the way in instead.
func levelWays(s State) []Span {
	word := "summary"
	if s.Running {
		word = "summary so far"
	}
	var out []Span
	for _, w := range []struct {
		p    Panel
		word string
	}{{PanelPlan, "plan"}, {PanelSummary, word}} {
		if out != nil {
			out = append(out, Span{Text: "  "})
		}
		if s.Nav.Selected == w.p {
			out = append(out, Span{Text: "[" + w.word + "]  " + keyRow(key{"⏎", "in"}), Role: RoleYou})
		} else {
			out = append(out, Span{Text: w.word, Role: RoleLabel})
		}
	}
	return append(out, Span{Text: " "})
}

// stage is the rows between the Now strip and the messages row, h rows of
// w cells on a screen fold says is folded or not: the top level's
// transcript and panes, or the pane gone into with the rail beside it.
func stage(s State, fold bool, w, h int) []string {
	if s.Nav.Open != "" {
		return inside(s, w, h)
	}
	if fold {
		out := transcript(s, w, h-1)
		return append(out, foldedRow(s, w))
	}
	leftW, rightW := columns(s, w)
	left := transcript(s, leftW, h)
	right := rightColumn(s, rightW, h)
	out := make([]string, h)
	for i := range out {
		out[i] = left[i] + right[i]
	}
	return out
}

// rightColumn is the panes beside the transcript, h rows of w cells. Where
// it is takes the top when the panes below keep room for their own rows.
func rightColumn(s State, w, h int) []string {
	whereRows := 0
	if h-radiusH-whereH >= minPaneRows {
		whereRows = whereH
	}
	rest := h - radiusH - whereRows
	actionsH := rest / 2
	editsH := rest / 4
	checksH := rest - actionsH - editsH
	var out []string
	if whereRows > 0 {
		out = append(out, where(s, w, whereRows)...)
	}
	out = append(out, actions(s, w, actionsH)...)
	out = append(out, edits(s, w, editsH)...)
	out = append(out, checks(s, w, checksH)...)
	return append(out, radius(s, w, radiusH)...)
}

// foldedRow is the panes as one row under a folded transcript: each pane's
// name with how many rows it holds, the selected one in brackets and in
// signal, and how to go in.
func foldedRow(s State, w int) string {
	counts := map[Panel]string{
		PanelActions: fmt.Sprint(len(s.Actions) + s.Dropped.Actions),
		PanelEdits:   fmt.Sprint(len(s.Edits) + s.Dropped.Edits),
		PanelChecks:  fmt.Sprint(len(s.Checks) + s.Dropped.Checks),
	}
	var parts []string
	for _, p := range Panels {
		name := string(p)
		if c := counts[p]; c != "" {
			name += " " + c
		}
		if p == s.Nav.Selected {
			name = "[" + name + "]"
		}
		parts = append(parts, name)
	}
	row := " " + strings.Join(parts, " · ")
	if s.Nav.Selected != "" {
		row += "   " + keyRow(key{"⏎", "in"})
	} else {
		row += "   " + keyRow(key{"TAB", "select"})
	}
	role := RoleLabel
	if s.Nav.Selected != "" {
		role = RoleYou
	}
	return fill(s.Theme, role, row, w)
}

// inside is a pane gone into: the rail of every pane down the left, the
// open one in signal, and the pane over the rest of the stage.
func inside(s State, w, h int) []string {
	r := rail(s, h)
	body := pane(s, s.Nav.Open, w-railW, h)
	if level(s) == 3 {
		body = codeLevel(s, w-railW, h)
	}
	out := make([]string, h)
	for i := range out {
		out[i] = r[i] + body[i]
	}
	return out
}

// pane draws p at w by h.
func pane(s State, p Panel, w, h int) []string {
	switch p {
	case PanelWhere:
		return where(s, w, h)
	case PanelActions:
		return actions(s, w, h)
	case PanelEdits:
		return edits(s, w, h)
	case PanelChecks:
		return checks(s, w, h)
	case PanelSummary:
		return summary(s, w, h)
	case PanelPlan:
		return plan(s, w, h)
	}
	return radius(s, w, h)
}

// rail is the right column's panes' titles written downward, three columns
// wide and h rows tall, each title given an equal share of the rows and cut
// to it. The summary and the plan have no title on it: they stand in no
// column, and a sixth title would cut every other one shorter (#854).
func rail(s State, h int) []string {
	var panes []Panel
	for _, p := range Panels {
		if !boxless(p) {
			panes = append(panes, p)
		}
	}
	out := make([]string, 0, h)
	share := max(h/len(panes), 2)
	for _, p := range panes {
		role := RoleLabel
		if p == s.Nav.Open {
			role = RoleYou
		}
		letters := []rune(string(p))
		for i := 0; i < share-1 && len(out) < h; i++ {
			c := " "
			if i < len(letters) {
				c = string(letters[i])
			}
			out = append(out, " "+paint(s.Theme, role, c)+" ")
		}
		if len(out) < h {
			out = append(out, strings.Repeat(" ", railW))
		}
	}
	for len(out) < h {
		out = append(out, strings.Repeat(" ", railW))
	}
	return out
}

// paneBox is Box for a pane: its frame in signal with heavy corners when
// Tab has selected it at the top level, as the faces doc draws a selected
// panel, and in line otherwise.
func paneBox(s State, p Panel, title, bottom string, rows []Row, w, h int) []string {
	if s.Nav.Open == "" && s.Nav.Selected == p {
		return frameBox(s.Theme, RoleYou, heavyCorners, title, bottom, rows, w, h)
	}
	return Box(s.Theme, title, bottom, rows, w, h)
}
