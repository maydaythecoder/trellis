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
	// as text.
	Warning
	// ModelCall is the loop's "model call" event, sent before each call to
	// the model with the turn number the turn it produces will carry
	// (#614).
	ModelCall
	// RuntimeIdentified is colibri's self-report, once per run (#627).
	RuntimeIdentified
	// Paused is the loop's word that the run is held at the hold a pause
	// asked for, and Resumed that it goes on (#806). Only these say the run
	// is held: a pause asked for is not yet a hold.
	Paused
	Resumed
	// Plan is the model's latest plan (#810), sent only where the
	// repository enables the plan tool.
	Plan
	// Compacted is the loop's word that older observations were elided so
	// a model request fit the run's context budget, and CompactedOnRequest
	// that the request after a client's compact folded a turn (#954).
	Compacted
	CompactedOnRequest
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
	// ModelCallElapsedMs is the budget's wall-clock spend as the loop
	// checked it before the call (#799); nil when the event carried none.
	ModelCallElapsedMs *uint64
	Runtime            *RuntimeFields
	// The run's other ceilings, from RunBudget (#799): the wall clock in
	// milliseconds, completion tokens, and one prompt's bytes, which a
	// driver with no context ceiling (the scripted one) does not send.
	MaxWallClockMs      *uint64
	MaxCompletionTokens *uint64
	MaxContextBytes     *uint64
	// HoldTurn is the model call a Paused or Resumed hold is before, and
	// PausedMs how long a Resumed one held, by trellis's monotonic clock.
	HoldTurn *uint64
	PausedMs *uint64
	// PlanFields is a Plan event's.
	PlanFields *PlanFields
	// CompactedFields is a Compacted event's, and CompactedOnRequestFields
	// a CompactedOnRequest one's.
	CompactedFields          *CompactedFields
	CompactedOnRequestFields *CompactedOnRequestFields
}

// CompactedFields is a compacted event as sent: the turn the request
// produced, turns 1 to ElidedTurns elided, holding ElidedBytes, and the
// request's size after eliding beside the budget in force, two figures
// and never a share. Each is nil when the line did not carry it.
type CompactedFields struct {
	Turn         *uint64
	ElidedTurns  *uint64
	ElidedBytes  *uint64
	RequestBytes *uint64
	Budget       *uint64
}

// CompactedOnRequestFields is a compacted on request event as sent: the
// request a client's compact built, the turns it folded and its size
// before and after.
type CompactedOnRequestFields struct {
	Turn        *uint64
	TurnsFolded *uint64
	ElidedTurns *uint64
	BytesBefore *uint64
	BytesAfter  *uint64
}

// PlanFields is a plan event as sent. Steps is the plan as trellis kept
// it, a JSON array in one string, the model's words: kept as sent here and
// read, defensively, by the model package. StepsCut and TextsCut say how
// many steps past the eighth trellis dropped and how many texts it
// shortened.
type PlanFields struct {
	Turn     *uint64
	Steps    *string
	StepsCut *uint64
	TextsCut *uint64
}

// TurnFields is a turn event. Turn and Call are what make an event a turn,
// the test rpc.rs applies, so both are required.
type TurnFields struct {
	Turn uint64
	Call string
	// What the trail records for the turn (#800), from the engine's own
	// reading of the call and its whole observation, so the TUI never splits
	// Call or searches Observation for them.
	Tool   *string
	Target *string
	// True only for an edit that applied or a file write_file created.
	Wrote *bool
	// Verdict is the loop's Debug rendering, "Some(true)", "Some(false)" or
	// "None", kept as sent.
	Verdict     *string
	CheckKind   *string
	Observation *string
	// Present only on a blast_radius turn. Summarised is how many of the
	// radius's call sites its observation summarised by file instead of
	// listing them, 0 when it listed every one.
	Resolved   *uint64
	Candidates *uint64
	Unresolved *uint64
	Summarised *uint64
	// Absent when the driver sent no prompt (a scripted run).
	PromptBytes *uint64
	// The wall time of the step's model call, queue wait included.
	ModelMs          *uint64
	CompletionTokens *uint64
	QueueWaitMs      *uint64
	// ElapsedMs is the run's wall-clock spend after the turn as its budget
	// measures it, waits on a person and pauses left out (#799).
	ElapsedMs *uint64
	// DurationMs is the tool call's own time, a wait on a person left out
	// (#800). CompletionTokensUsed is the completion tokens over every reply
	// so far, the counter run budget's max_completion_tokens is checked against, never
	// a sum with prompt tokens (#799).
	DurationMs           *uint64
	CompletionTokensUsed *uint64
	// Present only on an edit that applied (#802): the whole lines it added
	// and removed, the line of the file before it where the hunk starts,
	// and the hunk as a unified diff, cut past 8192 bytes with a last line
	// saying so. The diff is repository text and model output, shown as
	// text and never interpreted (CLAUDE.md rule 10).
	LinesAdded   *uint64
	LinesRemoved *uint64
	StartLine    *uint64
	Diff         *string
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
	// ElapsedMs is the loop's whole wall-clock spend, as its budget measured
	// it. Absent for a run the loop did not finish (an interrupt, an error
	// before it started, a partitioned run), and never a partial figure.
	ElapsedMs *uint64
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
	// RequestID is the id of the prompt or follow-up whose run asks; nil
	// when it was sent with no id (trellis sends null).
	RequestID *string
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
				Outcome:   s(o, "outcome"),
				Exit:      i(o, "exit"),
				ID:        s(o, "id"),
				RunID:     s(o, "run_id"),
				Restored:  b(o, "restored"),
				Error:     s(o, "error"),
				ElapsedMs: u(o, "elapsed_ms"),
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
				ID:        *id,
				Question:  s(o, "question"),
				Detail:    s(o, "detail"),
				RequestID: s(o, "request_id"),
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
			Tool:             s(f, "tool"),
			Target:           s(f, "target"),
			Wrote:            b(f, "wrote"),
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
			ElapsedMs:        u(f, "elapsed_ms"),
			DurationMs:       u(f, "duration_ms"),
			LinesAdded:       u(f, "lines_added"),
			LinesRemoved:     u(f, "lines_removed"),
			StartLine:        u(f, "start_line"),
			Diff:             s(f, "diff"),
			// Below the rest, so their names do not realign every field above.
			CompletionTokensUsed: u(f, "completion_tokens_used"),
			Summarised:           u(f, "summarised"),
		}}
	case msg != nil && *msg == "model call":
		// Without its turn it names nothing.
		turn := u(f, "turn")
		if turn == nil {
			return Frame{Kind: Ignored}
		}
		return Frame{Kind: ModelCall, ModelCallTurn: turn, ModelCallElapsedMs: u(f, "elapsed_ms")}
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
		return Frame{Kind: RunBudget, MaxIterations: u(f, "max_iterations"),
			MaxWallClockMs: u(f, "max_wall_clock_ms"), MaxCompletionTokens: u(f, "max_completion_tokens"), MaxContextBytes: u(f, "max_context_bytes")}
	// Read whatever each carries: the event is the compaction, and a line
	// missing a figure still says one happened.
	case msg != nil && *msg == "compacted":
		return Frame{Kind: Compacted, CompactedFields: &CompactedFields{Turn: u(f, "turn"),
			ElidedTurns: u(f, "elided_turns"), ElidedBytes: u(f, "elided_bytes"),
			RequestBytes: u(f, "request_bytes"), Budget: u(f, "budget")}}
	case msg != nil && *msg == "compacted on request":
		return Frame{Kind: CompactedOnRequest, CompactedOnRequestFields: &CompactedOnRequestFields{Turn: u(f, "turn"),
			TurnsFolded: u(f, "turns_folded"), ElidedTurns: u(f, "elided_turns"),
			BytesBefore: u(f, "bytes_before"), BytesAfter: u(f, "bytes_after")}}
	case msg != nil && *msg == "steer delivered":
		return Frame{Kind: SteerDelivered, AfterStep: u(f, "after_step")}
	case msg != nil && *msg == "paused":
		return Frame{Kind: Paused, HoldTurn: u(f, "turn")}
	case msg != nil && *msg == "resumed":
		return Frame{Kind: Resumed, HoldTurn: u(f, "turn"), PausedMs: u(f, "paused_ms")}
	case msg != nil && *msg == "plan":
		return Frame{Kind: Plan, PlanFields: &PlanFields{Turn: u(f, "turn"), Steps: s(f, "steps"),
			StepsCut: u(f, "steps_cut"), TextsCut: u(f, "texts_cut")}}
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
