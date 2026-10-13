// Package frames types one line from the `trellis rpc` child (ADR 0025
// section 5, ADR 0048 section 6). It is the Go counterpart of
// desktop/src/frames.rs, and reads the same fields.
//
// The child's stdout carries two shapes. Framing records (run_start,
// run_end, response, approval_request) have a top-level "type". The loop's
// own events are tracing JSON lines, {"fields": {"message": ..}, ..}, told
// apart by fields.message. A stderr line is Human whatever it holds, and is
// never read as data.
//
// Everything here is untrusted: it is model or server text. A value of the
// wrong JSON type is treated as absent, an absent field stays nil (no 0 and
// no empty string stands in for something trellis did not report), and a
// type or event this package does not know is Ignored. Only the fields
// crates/trellis-cli/tests/client_contract.json lists are read; the rest are
// unversioned.
package frames

import (
	"encoding/json"
	"math"

	"github.com/maydaythecoder/trellis/tui/internal/rpc"
)

// Kind says which shape a Frame is.
type Kind int

const (
	// Ignored is a JSON object of a type, or an event, the TUI does not read.
	Ignored Kind = iota
	// Unparsed is a stdout line that is not a JSON object, kept as text.
	Unparsed
	// Human is a stderr line: trellis's human text.
	Human
	Turn
	RunBudget
	SteerDelivered
	RunStart
	RunEnd
	Response
	ApprovalRequest
	// Warning is a loop event at WARN or ERROR that is not a turn, such as
	// "no progress; stopping": the explanation for a run about to end, shown
	// as text (tui_bridge.rs does the same).
	Warning
	// ModelCall is the loop's "model call" event, sent before each call to
	// the model with the turn number the turn it produces will carry
	// (#614).
	ModelCall
	// RuntimeIdentified is colibri's self-report, once per run (#627).
	RuntimeIdentified
)

// Frame is one typed line. Only the fields of its Kind are set.
type Frame struct {
	Kind Kind
	// Text is the line for Unparsed and Human, and the message for Warning.
	Text string

	TurnFields     *TurnFields
	MaxIterations  *uint64 // RunBudget
	AfterStep      *uint64 // SteerDelivered
	RunStartFields *RunStartFields
	RunEndFields   *RunEndFields
	ResponseFields *ResponseFields
	Approval       *ApprovalFields
	// ModelCallTurn is the turn a ModelCall names.
	ModelCallTurn *uint64
	Runtime       *RuntimeFields
}

// TurnFields is a turn event. Turn and Call are what make an event a turn,
// the test tui_bridge.rs and rpc.rs apply, so both are required.
type TurnFields struct {
	Turn uint64
	Call string
	// Verdict is the loop's Debug rendering, "Some(true)", "Some(false)" or
	// "None", kept as sent.
	Verdict     *string
	CheckKind   *string
	Observation *string
	// Present only on a blast_radius turn.
	Resolved   *uint64
	Candidates *uint64
	Unresolved *uint64
	// Absent when the driver sent no prompt (a scripted run).
	PromptBytes *uint64
	// The wall time of the step's model call, queue wait included.
	ModelMs          *uint64
	CompletionTokens *uint64
	QueueWaitMs      *uint64
}

// RuntimeFields is the "runtime identified" event (client_contract.json).
// Served is what makes it name a runtime, so an event that lacks it is
// Ignored. Each count is colibri's own and nil when it did not report one
// (a liveness-only or failed /health): never a 0 standing in for it. The
// three tier counts stay three; nothing here adds them or takes a share,
// as desktop/src/frames.rs keeps them.
type RuntimeFields struct {
	Runtime   *string
	Served    string
	KVSlots   *uint64
	TiersVRAM *uint64
	TiersRAM  *uint64
	TiersDisk *uint64
}

type RunStartFields struct {
	Version *uint64
	Trellis *string
	Task    *string
	Model   *string
	Root    *string
	// The id of the prompt that started the run.
	ID *string
}

type RunEndFields struct {
	Outcome  *string
	Exit     *int64
	ID       *string
	RunID    *string
	Restored *bool
	Error    *string
}

type ResponseFields struct {
	Command *string
	Success *bool
	ID      *string
	// Data is the response's data, as decoded JSON, or nil.
	Data  any
	Error *string
}

// ApprovalFields is an approval question. Without an id it could not be
// answered, so a request that lacks one is Ignored.
type ApprovalFields struct {
	ID       string
	Question *string
	Detail   *string
}

// FromStderr types a stderr line: Human, whatever it holds.
func FromStderr(text string) Frame { return Frame{Kind: Human, Text: text} }

// Parse types one stdout line. It never fails: a line that is not a JSON
// object is Unparsed, shown as text and read as nothing else.
func Parse(l rpc.Line) Frame {
	o := l.JSON
	if o == nil {
		return Frame{Kind: Unparsed, Text: l.Text}
	}
	if ty, ok := o["type"]; ok {
		switch ty {
		case "run_start":
			return Frame{Kind: RunStart, RunStartFields: &RunStartFields{
				Version: u(o, "version"),
				Trellis: s(o, "trellis"),
				Task:    s(o, "task"),
				Model:   s(o, "model"),
				Root:    s(o, "root"),
				ID:      s(o, "id"),
			}}
		case "run_end":
			return Frame{Kind: RunEnd, RunEndFields: &RunEndFields{
				Outcome:  s(o, "outcome"),
				Exit:     i(o, "exit"),
				ID:       s(o, "id"),
				RunID:    s(o, "run_id"),
				Restored: b(o, "restored"),
				Error:    s(o, "error"),
			}}
		case "response":
			return Frame{Kind: Response, ResponseFields: &ResponseFields{
				Command: s(o, "command"),
				Success: b(o, "success"),
				ID:      s(o, "id"),
				Data:    o["data"],
				Error:   s(o, "error"),
			}}
		case "approval_request":
			id := s(o, "id")
			if id == nil {
				return Frame{Kind: Ignored}
			}
			return Frame{Kind: ApprovalRequest, Approval: &ApprovalFields{
				ID:       *id,
				Question: s(o, "question"),
				Detail:   s(o, "detail"),
			}}
		}
		return Frame{Kind: Ignored}
	}
	f, ok := o["fields"].(map[string]any)
	if !ok {
		return Frame{Kind: Ignored}
	}
	msg := s(f, "message")
	switch {
	case msg != nil && *msg == "turn":
		turn, call := u(f, "turn"), s(f, "call")
		if turn == nil || call == nil {
			return Frame{Kind: Ignored}
		}
		return Frame{Kind: Turn, TurnFields: &TurnFields{
			Turn:             *turn,
			Call:             *call,
			Verdict:          s(f, "verdict"),
			CheckKind:        s(f, "check_kind"),
			Observation:      s(f, "observation"),
			Resolved:         u(f, "resolved"),
			Candidates:       u(f, "candidates"),
			Unresolved:       u(f, "unresolved"),
			PromptBytes:      u(f, "prompt_bytes"),
			ModelMs:          u(f, "model_ms"),
			CompletionTokens: u(f, "completion_tokens"),
			QueueWaitMs:      u(f, "queue_wait_ms"),
		}}
	case msg != nil && *msg == "model call":
		// Without its turn it names nothing.
		turn := u(f, "turn")
		if turn == nil {
			return Frame{Kind: Ignored}
		}
		return Frame{Kind: ModelCall, ModelCallTurn: turn}
	case msg != nil && *msg == "runtime identified":
		served := s(f, "served")
		if served == nil {
			return Frame{Kind: Ignored}
		}
		return Frame{Kind: RuntimeIdentified, Runtime: &RuntimeFields{
			Runtime:   s(f, "runtime"),
			Served:    *served,
			KVSlots:   u(f, "kv_slots"),
			TiersVRAM: u(f, "tiers_vram"),
			TiersRAM:  u(f, "tiers_ram"),
			TiersDisk: u(f, "tiers_disk"),
		}}
	case msg != nil && *msg == "run budget":
		return Frame{Kind: RunBudget, MaxIterations: u(f, "max_iterations")}
	case msg != nil && *msg == "steer delivered":
		return Frame{Kind: SteerDelivered, AfterStep: u(f, "after_step")}
	case msg != nil:
		if lvl := s(o, "level"); lvl != nil && (*lvl == "WARN" || *lvl == "ERROR") {
			return Frame{Kind: Warning, Text: *msg}
		}
	}
	return Frame{Kind: Ignored}
}

func s(o map[string]any, k string) *string {
	if v, ok := o[k].(string); ok {
		return &v
	}
	return nil
}

func b(o map[string]any, k string) *bool {
	if v, ok := o[k].(bool); ok {
		return &v
	}
	return nil
}

// u reads a non-negative whole number. encoding/json decodes every number
// as float64, so a fraction, a negative or a value past 2^53 (where float64
// stops being exact) is absent rather than rounded into something trellis
// did not say.
func u(o map[string]any, k string) *uint64 {
	f, ok := num(o[k])
	if !ok || f < 0 || f != math.Trunc(f) || f > 1<<53 {
		return nil
	}
	v := uint64(f)
	return &v
}

func i(o map[string]any, k string) *int64 {
	f, ok := num(o[k])
	if !ok || f != math.Trunc(f) || math.Abs(f) > 1<<53 {
		return nil
	}
	v := int64(f)
	return &v
}

func num(v any) (float64, bool) {
	switch n := v.(type) {
	case float64:
		return n, true
	case json.Number:
		f, err := n.Float64()
		return f, err == nil
	}
	return 0, false
}
