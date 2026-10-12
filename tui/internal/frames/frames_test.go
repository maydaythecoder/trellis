package frames

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"

	"github.com/maydaythecoder/trellis/tui/internal/rpc"
)

func parse(t *testing.T, line string) Frame {
	t.Helper()
	return Parse(rpc.ParseLine(line))
}

// event builds a tracing JSON line as the rpc child writes one.
func event(message string, fields map[string]any) string {
	f := map[string]any{"message": message}
	for k, v := range fields {
		f[k] = v
	}
	b, _ := json.Marshal(map[string]any{
		"timestamp": "2026-10-06T00:00:00.000000Z",
		"level":     "INFO",
		"fields":    f,
		"target":    "trellis_loop::agent",
	})
	return string(b)
}

// fieldsOf is the fields one record carries, from the contract: every list
// in an event's spec but a *_values one, and a framing type's always and
// optional keys.
func fieldsOf(spec map[string]any) []string {
	var out []string
	for k, v := range spec {
		switch vv := v.(type) {
		case []any:
			if strings.HasSuffix(k, "_values") {
				continue
			}
			for _, f := range vv {
				out = append(out, f.(string))
			}
		case map[string]any:
			if k == "optional" {
				for f := range vv {
					out = append(out, f)
				}
			}
		}
	}
	sort.Strings(out)
	return out
}

var booleans = map[string]bool{"success": true, "restored": true}

var numeric = map[string]bool{
	"turn": true, "resolved": true, "candidates": true, "unresolved": true,
	"prompt_bytes": true, "model_ms": true, "completion_tokens": true,
	"queue_wait_ms": true, "max_iterations": true, "after_step": true,
	"version": true, "exit": true,
}

var kindOf = map[string]Kind{
	"turn": Turn, "run budget": RunBudget, "steer delivered": SteerDelivered,
	"run_start": RunStart, "run_end": RunEnd, "response": Response,
	"approval_request": ApprovalRequest,
}

// The risk (ADR 0048 section 6): trellis keeps a field the contract
// promises and the TUI's parser drops it, so a pane goes blank while every
// trellis test passes. For each event and framing type in
// client_contract.json, one line carries every listed field with a value of
// its own (a boolean both ways); a field whose value does not survive the
// parse is named.
func TestFramesReadEveryContractField(t *testing.T) {
	raw, err := os.ReadFile(filepath.Join("..", "..", "..", "crates", "trellis-cli", "tests", "client_contract.json"))
	if err != nil {
		t.Fatalf("read the contract: %v", err)
	}
	var contract map[string]any
	if err := json.Unmarshal(raw, &contract); err != nil {
		t.Fatalf("the contract is not JSON: %v", err)
	}
	rpcSpec := contract["rpc"].(map[string]any)
	n := 7000
	sample := func(field, record string, spec map[string]any) any {
		if vals, ok := spec[field+"_values"].([]any); ok && len(vals) > 0 {
			return vals[len(vals)-1]
		}
		if numeric[field] {
			n++
			return n
		}
		if field == "data" {
			return map[string]any{"answer": "tok-" + record + "-" + field + "-end"}
		}
		return "tok-" + record + "-" + field + "-end"
	}
	type record struct {
		name    string
		fields  map[string]any
		isEvent bool
	}
	var records []record
	for _, group := range []string{"events", "framing"} {
		for name, sp := range rpcSpec[group].(map[string]any) {
			spec := sp.(map[string]any)
			m := map[string]any{}
			for _, f := range fieldsOf(spec) {
				switch {
				case f == "type":
					m[f] = name
				case booleans[f]:
					m[f] = true
				default:
					m[f] = sample(f, name, spec)
				}
			}
			records = append(records, record{name, m, group == "events"})
		}
	}
	line := func(r record, fields map[string]any) string {
		if r.isEvent {
			return event(r.name, fields)
		}
		b, _ := json.Marshal(fields)
		return string(b)
	}
	// What a parse kept, as text: every pointer is followed by Marshal, so
	// a value the parser read is in it and one it dropped is not.
	kept := func(fr Frame) string {
		b, err := json.Marshal(fr)
		if err != nil {
			t.Fatal(err)
		}
		return string(b)
	}
	if len(records) < 7 {
		t.Fatalf("the contract lists %d records; expected at least 7", len(records))
	}
	var lost []string
	for _, r := range records {
		fr := parse(t, line(r, r.fields))
		if want, ok := kindOf[r.name]; !ok || fr.Kind != want {
			lost = append(lost, fmt.Sprintf("%s (parsed as kind %d)", r.name, fr.Kind))
			continue
		}
		got := kept(fr)
		for f, v := range r.fields {
			if f == "type" {
				continue
			}
			ok := false
			switch vv := v.(type) {
			case bool:
				flipped := map[string]any{}
				for k, x := range r.fields {
					flipped[k] = x
				}
				flipped[f] = !vv
				ok = kept(parse(t, line(r, flipped))) != got
			case string:
				ok = strings.Contains(got, vv)
			case int:
				b, _ := json.Marshal(vv)
				ok = strings.Contains(got, string(b))
			case map[string]any:
				ok = strings.Contains(got, vv["answer"].(string))
			}
			if !ok {
				lost = append(lost, r.name+"."+f)
			}
		}
	}
	if len(lost) > 0 {
		sort.Strings(lost)
		t.Fatalf("the parser drops what client_contract.json lists: %v", lost)
	}
}

// A stdout line that is not a JSON object is text, never a frame; an
// unknown type and an unknown event are Ignored.
func TestFramesNonJSONIsTextAndUnknownIsIgnored(t *testing.T) {
	if fr := parse(t, "trellis: hello"); fr.Kind != Unparsed || fr.Text != "trellis: hello" {
		t.Fatalf("non-JSON: %+v", fr)
	}
	if fr := parse(t, `{"type":"mystery"}`); fr.Kind != Ignored {
		t.Fatalf("unknown type: %+v", fr)
	}
	if fr := parse(t, event("something else", nil)); fr.Kind != Ignored {
		t.Fatalf("unknown INFO event: %+v", fr)
	}
	if fr := FromStderr(`{"type":"run_end"}`); fr.Kind != Human {
		t.Fatalf("stderr read as data: %+v", fr)
	}
}

// A value of the wrong JSON type is absent, not coerced: a string turn
// number does not make a turn, and a negative or fractional count is not
// read as some other count.
func TestFramesWrongTypesAreAbsent(t *testing.T) {
	if fr := parse(t, event("turn", map[string]any{"turn": "3", "call": "x"})); fr.Kind != Ignored {
		t.Fatalf("a turn with a string turn number: %+v", fr)
	}
	if fr := parse(t, event("turn", map[string]any{"turn": 3})); fr.Kind != Ignored {
		t.Fatalf("a turn without a call: %+v", fr)
	}
	fr := parse(t, event("turn", map[string]any{"turn": 3, "call": "x", "model_ms": -1, "prompt_bytes": 2.5}))
	if fr.Kind != Turn || fr.TurnFields.ModelMs != nil || fr.TurnFields.PromptBytes != nil {
		t.Fatalf("negative or fractional counts were read: %+v", fr.TurnFields)
	}
	if fr := parse(t, `{"type":"approval_request","question":"q"}`); fr.Kind != Ignored {
		t.Fatalf("an approval request with no id: %+v", fr)
	}
}

// A WARN event that is not a turn is a Warning carrying its message, the
// line tui_bridge.rs puts in the transcript; an INFO one is not.
func TestFramesWarningCarriesItsMessage(t *testing.T) {
	b, _ := json.Marshal(map[string]any{"level": "WARN", "fields": map[string]any{"message": "no progress; stopping"}})
	if fr := parse(t, string(b)); fr.Kind != Warning || fr.Text != "no progress; stopping" {
		t.Fatalf("warning: %+v", fr)
	}
}
