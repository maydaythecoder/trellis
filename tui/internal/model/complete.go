package model

import (
	"strings"
	"unicode"
)

// @ completion. The paths come from trellis (rpc list_paths, R4 #502),
// which lists tracked and untracked-not-ignored files through trellis_vcs;
// this package starts no process and spawns no git (ADR 0023, go-guards.sh).

// atFragment is the @word being typed at the very end of the line, without
// its @, and whether there is one. A fragment the cursor has moved past
// (the line ends in a space) is finished and is not offered again.
func (m *Model) atFragment() (string, bool) {
	if m.Input == "" || unicode.IsSpace(rune(m.Input[len(m.Input)-1])) {
		return "", false
	}
	words := strings.Fields(m.Input)
	w := words[len(words)-1]
	if !strings.HasPrefix(w, "@") {
		return "", false
	}
	return w[1:], true
}

// tab asks trellis for the paths starting with the @fragment, or, when
// they are already listed for this fragment, inserts the first.
func (m *Model) tab() {
	if m.Running {
		return
	}
	frag, ok := m.atFragment()
	if !ok {
		return
	}
	if len(m.Offered) > 0 && m.offeredFor == frag {
		m.completeWith(m.Offered[0])
		return
	}
	m.Offered = nil
	m.offeredFor = frag
	m.pathsID = m.ask("list_paths", forPaths, map[string]any{"prefix": frag})
}

// pathsAnswered offers what list_paths returned, in its order: one path
// completes the word, several are listed, none changes nothing. An answer
// to a fragment no longer being typed is dropped.
func (m *Model) pathsAnswered(id string, data any) {
	frag, ok := m.atFragment()
	if id != m.pathsID || !ok || frag != m.offeredFor {
		return
	}
	d, _ := data.(map[string]any)
	list, _ := d["paths"].([]any)
	var paths []string
	for _, p := range list {
		if s, isStr := p.(string); isStr && s != "" {
			paths = append(paths, Plain(s))
		}
	}
	switch len(paths) {
	case 0:
		m.Offered = nil
	case 1:
		m.completeWith(paths[0])
	default:
		m.Offered = paths
	}
}

// completeWith replaces the @fragment with path and a space.
func (m *Model) completeWith(path string) {
	frag, ok := m.atFragment()
	if !ok {
		return
	}
	m.Input = m.Input[:len(m.Input)-len(frag)-1] + path + " "
	m.Offered = nil
	m.offeredFor = ""
}

// inputChanged drops completions offered for a line no longer typed.
func (m *Model) inputChanged() {
	if frag, ok := m.atFragment(); !ok || frag != m.offeredFor {
		m.Offered = nil
	}
}
