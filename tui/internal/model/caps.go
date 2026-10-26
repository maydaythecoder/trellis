package model

import (
	"fmt"
	"unicode/utf8"
)

// Every list the model keeps has a limit, so a long run, a chatty child or
// a child that stops answering cannot grow the TUI's memory without bound
// (#848). What happens at each limit depends on what the list holds:
//
//   - the run panes keep the newest PaneCap actions, edits and checks, and
//     each pane says how many older ones it no longer holds;
//   - a question trellis asks while ApprovalsCap are already waiting is
//     declined at once and said so: a question left unanswered would hold
//     the run forever, and no is the answer that changes nothing (ADR 0018);
//   - a steer typed once a run has SteersCap is not sent, and stays on the
//     task line: what the person typed is never dropped;
//   - the answers still awaited are the newest UnansweredCap; an answer to
//     a forgotten command is ignored when it comes;
//   - a transcript line longer than LineCap bytes is cut, and says how much.
//   - the composer's queue lists at most QueueCap items: the oldest settled
//     one makes room for a new one, and with none settled the new one is
//     sent but not listed (#852).
//   - the model's closing words keep their first ClosingCap bytes, and the
//     summary says how many it cut (#854), as the desktop keeps 16 KiB.
//
// TranscriptCap and StderrCap (model.go) and OfferedCap (complete.go) are
// the limits that came before these.
const (
	PaneCap       = 1000
	ApprovalsCap  = 32
	SteersCap     = 100
	UnansweredCap = 256
	LineCap       = 4096
	QueueCap      = 100
	ClosingCap    = 16 * 1024
)

// capLine is l cut to at most LineCap bytes, at a character's start, with
// how much was cut said at its end. Every frame wraps every transcript line
// again, so one line of a megabyte would be wrapped on every key press.
func capLine(l string) string {
	if len(l) <= LineCap {
		return l
	}
	end := LineCap
	for end > 0 && !utf8.RuneStart(l[end]) {
		end--
	}
	return fmt.Sprintf("%s… [%d more bytes cut]", l[:end], len(l)-end)
}
