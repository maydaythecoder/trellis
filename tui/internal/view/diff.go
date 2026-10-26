package view

import (
	"fmt"
	"strings"
)

// The code level (#853), the terminal face of the desktop's Code level
// (#831, desktop/src/state.rs `code`), so the two clients say the same
// thing about a file. Inside the edits pane each file the run wrote, or an
// approval asks to write, is one row, in the order it was first touched,
// with its two line counts summed over its writes and its state word: `in
// the worktree`, `awaiting your approval`, `left in the tree`, `rolled
// back`, or `not reported` with why one level down. ⏎ goes one level more,
// to the file's diff: each write's hunk under its @@ header, + and - in the
// sign column and the colour on the text only (the Diff card). Every word
// here comes from what trellis sent; nothing in the view judges a change.
//
// A diff is repository text and model output (CLAUDE.md rule 10): each
// line is split out first and made plain on its own, so no escape sequence
// or control character in it reaches the terminal, and a line's role comes
// from its sign column alone, never from its words.

// Asked is an approval question waiting on the person, as the model holds
// it: its question and detail already made plain line by line.
type Asked struct {
	ID       string
	Question string
	Detail   string
	// OnScreen is the question the approval box shows, the only one
	// answered: the rest wait behind it, unseen.
	OnScreen bool
}

// codeFile is one row of the code level and what its diff level draws.
type codeFile struct {
	path string
	// added and removed are summed over every write and every question
	// waiting on the file; nil once any write did not report its counts (a
	// file write_file created), so a part is never drawn as the whole.
	added, removed *uint64
	hunks          []hunk
	word           string
	tone           Role
	// why is what a `not reported` state could not say, drawn one level
	// down; "" for every other state.
	why string
	// answerable is whether the question on screen asks about this file,
	// and so can be answered at its diff.
	answerable bool
}

// hunk is one write's lines, or one waiting question's, under its label.
type hunk struct {
	label     string
	labelRole Role
	lines     []diffLine
}

// diffLine is one line of a hunk: the sign column and the text, painted in
// the role the sign gives it.
type diffLine struct {
	sign, text string
	role       Role
}

// signed is a diff line's role and sign column from its first byte, the
// unified diff's own column: + added, - removed, a space context. Any other
// line is trellis's note (a cut, say) and is drawn whole as a label.
func signed(l string) (sign string, role Role, ok bool) {
	switch {
	case strings.HasPrefix(l, "+"):
		return "+", RoleDiffAdd, true
	case strings.HasPrefix(l, "-"):
		return "-", RoleDiffDel, true
	case strings.HasPrefix(l, " "):
		return " ", RoleText, true
	}
	return "", RoleLabel, false
}

// appliedHunk is an applied edit's diff as trellis sent it on the turn
// (#802): --- and +++ naming the path, which the level's title already
// does, one @@ header, then each line behind its sign with no space. A
// diff with no @@ header is drawn as notes, with a label that says so.
func appliedHunk(e Edit) hunk {
	turn := fmt.Sprintf("turn %d", e.Turn)
	if e.Diff == nil {
		return hunk{label: turn + " · trellis sends a diff only with an applied edit, not with a file write_file created", labelRole: RoleLabel}
	}
	lines := strings.Split(strings.TrimRight(*e.Diff, "\n"), "\n")
	at := -1
	for i, l := range lines {
		if strings.HasPrefix(l, "@@") {
			at = i
			break
		}
	}
	if at < 0 {
		h := hunk{label: turn + " · trellis's diff carried no @@ header", labelRole: RoleLabel}
		for _, l := range lines {
			h.lines = append(h.lines, diffLine{text: Plain(l), role: RoleLabel})
		}
		return h
	}
	h := hunk{label: Plain(lines[at]) + " · " + turn, labelRole: RoleDiffHunk}
	for _, l := range lines[at+1:] {
		sign, role, ok := signed(l)
		if !ok {
			h.lines = append(h.lines, diffLine{text: Plain(l), role: role})
			continue
		}
		h.lines = append(h.lines, diffLine{sign: sign, text: Plain(l[1:]), role: role})
	}
	return h
}

// askedHunk is a waiting question's diff after the lines that name its
// file, in the approve gate's shape: each line behind its sign and a
// space, and no @@ header, since no turn has applied it yet.
func askedHunk(detail string, skip int) hunk {
	h := hunk{label: "awaiting your approval · not applied yet", labelRole: RoleNote}
	lines := strings.Split(detail, "\n")
	for _, l := range lines[min(skip, len(lines)):] {
		sign, role, ok := signed(l)
		if !ok {
			h.lines = append(h.lines, diffLine{text: Plain(l), role: role})
			continue
		}
		h.lines = append(h.lines, diffLine{sign: sign, text: Plain(strings.TrimPrefix(l[1:], " ")), role: role})
	}
	return h
}

// ApprovalFile is the file an approval question asks to write, and how
// many of its detail's lines name it: the approve gate's `edit <path>
// (+a -r)` with `--- <path>` and `+++ <path>` heading its diff, or
// `write_file <path> (+n new)` with `+++ <path> (new file)`. The shape is
// the gate's (trellis_loop::gate), as the desktop's approval_file reads
// it, so a question in any other shape ties to no file and is answered in
// the approval box alone.
func ApprovalFile(question, detail string) (path string, skip int, ok bool) {
	lines := strings.Split(detail, "\n")
	if rest, found := strings.CutPrefix(lines[0], "+++ "); found {
		if p, isNew := strings.CutSuffix(rest, " (new file)"); isNew {
			return p, 1, strings.HasPrefix(question, "write_file "+p+" (")
		}
	}
	p, found := strings.CutPrefix(lines[0], "--- ")
	if !found || len(lines) < 2 || lines[1] != "+++ "+p || !strings.HasPrefix(question, "edit "+p+" (") {
		return "", 0, false
	}
	return p, 2, true
}

// fileState is a file's state word, its tone and, for `not reported`, why
// (#831's file_state): a question waiting on a write to it; a run in
// flight, whose writes are in the worktree until run_end; then run_end's
// outcome and restored. A run whose end did not say whether the tree was
// put back is never guessed either way. The tones keep the TUI's meanings
// (paint.go): ok and down are a check's alone, so a file the person has to
// review is signal, as the status bar's `to review` is, and one that is
// gone is a label.
func fileState(s State, awaiting bool) (word string, tone Role, why string) {
	switch {
	case s.Running && awaiting:
		return "awaiting your approval", RoleNote, ""
	case s.Running:
		return "in the worktree", RoleYou, ""
	case s.Outcome == "":
		return "not reported", RoleLabel, "trellis rpc exited before a run_end said what became of the run's writes"
	case s.Now.Outcome == "converged":
		return "left in the tree", RoleYou, ""
	case s.Restored == nil:
		return "not reported", RoleLabel, "run_end did not say whether the tree was restored"
	case *s.Restored:
		return "rolled back", RoleLabel, ""
	}
	return "left in the tree", RoleYou, ""
}

// sum is a running count plus n, or nil once either is not reported.
func sum(acc, n *uint64) *uint64 {
	if acc == nil || n == nil {
		return nil
	}
	v := *acc + *n
	return &v
}

// codeFiles is the code level's files from the writes the panes hold and
// the questions waiting.
func codeFiles(s State) []codeFile {
	type asked struct {
		a    Asked
		path string
		skip int
	}
	var paths []string
	seen := map[string]bool{}
	add := func(p string) {
		if !seen[p] {
			seen[p] = true
			paths = append(paths, p)
		}
	}
	for _, e := range s.Edits {
		add(e.Path)
	}
	var waiting []asked
	for _, a := range s.Asked {
		if p, skip, ok := ApprovalFile(a.Question, a.Detail); ok {
			waiting = append(waiting, asked{a, p, skip})
			add(p)
		}
	}
	files := make([]codeFile, 0, len(paths))
	for _, p := range paths {
		zero := uint64(0)
		f := codeFile{path: p, added: &zero, removed: &zero}
		for _, e := range s.Edits {
			if e.Path != p {
				continue
			}
			f.added, f.removed = sum(f.added, e.Added), sum(f.removed, e.Removed)
			f.hunks = append(f.hunks, appliedHunk(e))
		}
		awaiting := false
		for _, w := range waiting {
			if w.path != p {
				continue
			}
			h := askedHunk(w.a.Detail, w.skip)
			var a, r uint64
			for _, l := range h.lines {
				switch l.sign {
				case "+":
					a++
				case "-":
					r++
				}
			}
			f.added, f.removed = sum(f.added, &a), sum(f.removed, &r)
			f.hunks = append(f.hunks, h)
			awaiting = true
			f.answerable = f.answerable || w.a.OnScreen
		}
		f.word, f.tone, f.why = fileState(s, awaiting)
		files = append(files, f)
	}
	return files
}

// CodePaths is the code level's files in the order it lists them, for the
// model's cursor.
func CodePaths(s State) []string {
	var out []string
	for _, f := range codeFiles(s) {
		out = append(out, f.path)
	}
	return out
}

// chosen is the file the cursor is on: Nav.File, or the first file when
// that names none; ok is false with no file at all.
func chosen(s State, files []codeFile) (at int, ok bool) {
	for i, f := range files {
		if f.path == s.Nav.File {
			return i, true
		}
	}
	return 0, len(files) > 0
}

// counts is a file's two line counts as the Diff card's head draws them:
// added in diff-add and removed in diff-del, never one figure, and Dash
// when either one was not reported.
func counts(f codeFile) []Span {
	if f.added == nil || f.removed == nil {
		return []Span{{Text: Dash, Role: RoleLabel}}
	}
	return []Span{
		{Text: fmt.Sprintf("+%d", *f.added), Role: RoleDiffAdd},
		{Text: " "},
		{Text: fmt.Sprintf("−%d", *f.removed), Role: RoleDiffDel},
	}
}

// edits is the edits pane: at the top level each file with its two
// counts, and gone into, the code level, with the cursor's file in signal
// and each file's state word. The path is cut first, so the counts and
// the word keep their places.
func edits(s State, w, h int) []string {
	inner := w - 2
	in := s.Nav.Open == PanelEdits
	files := codeFiles(s)
	at, _ := chosen(s, files)
	rows := notKept(s.Dropped.Edits, "edits")
	if in && len(files) == 0 {
		rows = append(rows, Row{Text: "  this run has written no file yet", Role: RoleLabel})
	}
	for i, f := range files {
		right := counts(f)
		mark, role := "~ ", RoleText
		if in {
			right = append(right, Span{Text: "  "}, statusSpan(f.word, f.tone))
			mark = "  "
			if i == at {
				mark, role = "› ", RoleYou
			}
		}
		room := max(inner-spansWidth(right)-Width(mark)-1, 1)
		left := []Span{{Text: mark + Cut(Plain(f.path), room), Role: role}}
		rows = append(rows, Row{Spans: spread(left, right, inner)})
	}
	return paneBox(s, PanelEdits, "edits", "", rows, w, h)
}

// codeRows is the diff level's rows at inner width w: the file's counts and
// state, why a state is not reported, the keys that answer a question
// waiting on it while it is set aside here, and then each hunk under its label,
// every line wrapped to the width with its continuation under its text.
func codeRows(s State, f codeFile, w int) []Row {
	head := append(append([]Span{{Text: " "}}, counts(f)...), Span{Text: "  "}, statusSpan(f.word, f.tone))
	rows := []Row{{Spans: head}}
	if f.why != "" {
		rows = append(rows, Row{Text: " " + f.why, Role: RoleLabel})
	}
	// In the pane, since at 80 columns the status bar has no room for them.
	if f.answerable && s.Reviewing {
		rows = append(rows, Row{Text: " " + keyRow(key{"y", "accept this change"}, key{"n", "reject"}, key{"ESC", "back to the question"}), Role: RoleYou})
	}
	for _, h := range f.hunks {
		rows = append(rows, Row{}, Row{Text: " " + h.label, Role: h.labelRole})
		for _, l := range h.lines {
			if l.sign == "" {
				for _, part := range WrapHard(l.text, max(w-1, 1)) {
					rows = append(rows, Row{Text: " " + part, Role: l.role})
				}
				continue
			}
			for i, part := range WrapHard(l.text, max(w-3, 1)) {
				sign := l.sign
				if i > 0 {
					sign = " "
				}
				rows = append(rows, Row{Spans: []Span{{Text: " " + sign + " "}, {Text: part, Role: l.role}}})
			}
		}
	}
	return rows
}

// codeLevel is the diff level for the chosen file at w by h: its path on
// the top border, and when its rows do not fit, which are in view on the
// bottom one.
func codeLevel(s State, w, h int) []string {
	files := codeFiles(s)
	at, ok := chosen(s, files)
	if !ok || (s.Nav.File != "" && files[at].path != s.Nav.File) {
		rows := rowsOf(RoleLabel, "  this run has no write to this file now: a new run starts with none")
		return Box(s.Theme, Plain(s.Nav.File), "", rows, w, h)
	}
	f := files[at]
	rows := codeRows(s, f, w-2)
	innerH := max(h-2, 0)
	from := clamp(s.Nav.Scroll, 0, max(len(rows)-innerH, 0))
	bottom := ""
	if len(rows) > innerH {
		to := min(from+innerH, len(rows))
		bottom = fmt.Sprintf("rows %d–%d of %d · %s", from+1, to, len(rows), Keycap("↑")+Keycap("↓")+" "+Keycap("PGUP")+Keycap("PGDN"))
	}
	return Box(s.Theme, Plain(f.path), bottom, rows[from:], w, h)
}

// CodeTop is the furthest the diff level scrolls at w by h, the view's own
// limit, so a key never moves the offset somewhere it cannot show (#843).
func CodeTop(s State, w, h int) int {
	w, h = max(w, MinWidth), max(h, MinHeight)
	files := codeFiles(s)
	at, ok := chosen(s, files)
	if !ok {
		return 0
	}
	_, bodyH := layout(s, w, h)
	return max(len(codeRows(s, files[at], w-railW-2))-max(bodyH-2, 0), 0)
}
