package model

import (
	"strings"

	"github.com/maydaythecoder/trellis/tui/internal/frames"
	"github.com/maydaythecoder/trellis/tui/internal/ring"
	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// The status bar's model and tier are trellis's answers, never the
// client's: get_state names the model in use and the tier the probe
// reported (#269). Until get_state has a tier the bar says "probing…".

// Purposes a sent command's answer is read for, by its id.
const (
	forState    = "state"
	forModel    = "model"
	forPaths    = "paths"
	forCodebase = "codebase"
	forRadius   = "radius"
)

// ask sends a command whose answer is read for purpose. The newest
// UnansweredCap ids are remembered in order (caps.go): an id pushed out
// takes its purpose with it, so a child that stops answering cannot grow
// the map, and an answer that comes for it later is ignored. A forgotten
// classify would leave the task line waiting for good, so it ends the wait
// and says so.
func (m *Model) ask(kind, purpose string, fields map[string]any) string {
	id := m.send(kind, fields)
	if m.purposes == nil {
		m.purposes = map[string]string{}
		m.asked = ring.New[string](UnansweredCap)
	}
	if old, full := m.asked.Oldest(); full {
		if p, waiting := m.purposes[old]; waiting {
			delete(m.purposes, old)
			m.forgotten++
			if p == forClassify && m.classifying {
				m.classifying, m.classifyText = false, ""
				m.push(view.RoleNote, "· trellis never said whether that was a question or a task; nothing was run")
			}
		}
	}
	m.asked.Push(id)
	m.purposes[id] = purpose
	return id
}

// answered returns, and forgets, what a response's id was asked for.
func (m *Model) answered(r *frames.ResponseFields) string {
	if r.ID == nil {
		return ""
	}
	p, ok := m.purposes[*r.ID]
	if ok {
		delete(m.purposes, *r.ID)
	}
	return p
}

// refreshState asks get_state again: after a model is chosen and after a
// run, when the model or the tier may have changed.
func (m *Model) refreshState() { m.ask("get_state", forState, nil) }

// stateAnswered reads get_state's model and tier into the status bar.
func (m *Model) stateAnswered(data any) {
	d, ok := data.(map[string]any)
	if !ok {
		return
	}
	if s, isStr := d["model"].(string); isStr {
		m.ModelName = view.Plain(s)
	} else if d["model"] == nil {
		m.ModelName = ""
	}
	if s, isStr := d["tier"].(string); isStr && strings.TrimSpace(s) != "" {
		// The bar writes "tier " before the label; a label that already
		// starts with the word would read "tier tier".
		m.Tier = view.Plain(strings.TrimPrefix(s, "tier "))
	}
	// Where it is names the repository trellis serves, as trellis says it.
	if s, isStr := d["root"].(string); isStr {
		m.strip.Root = view.Plain(s)
	}
	m.pendingListed(d)
}

// runtimeText is what a runtime said of itself, for Where it is: the
// runtime's name and what it serves, or what it serves alone when it gave
// no name.
func runtimeText(r *frames.RuntimeFields) string {
	if r == nil {
		return ""
	}
	served := view.Plain(r.Served)
	if r.Runtime == nil || strings.TrimSpace(*r.Runtime) == "" {
		return served
	}
	return view.Plain(*r.Runtime) + " · " + served
}
