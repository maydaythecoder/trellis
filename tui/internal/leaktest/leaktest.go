// Package leaktest fails a test whose goroutines outlive it (#846).
//
// Two checks, because each sees what the other cannot:
//
//   - Check reads every goroutine's stack (runtime.Stack) and reports the
//     ones that pass through this module's packages and were not running
//     when the test began. It retries with backoff from 1 µs to 100 ms
//     until a deadline, as goleak does, because a goroutine on its way out
//     is still listed for a moment. It sees a goroutine blocked reading a
//     pipe, which the profile below cannot.
//   - Main runs a package's tests and then writes Go 1.27's goroutineleak
//     profile, in which the garbage collector names every goroutine blocked
//     forever on a channel or lock nothing else can reach. It sees such a
//     goroutine wherever it started, a library's included.
//
// Counting runtime.NumGoroutine instead breaks under t.Parallel and counts
// the runtime's own goroutines, so neither check does.
package leaktest

import (
	"bytes"
	"fmt"
	"io"
	"os"
	"regexp"
	"runtime"
	"runtime/pprof"
	"strconv"
	"strings"
	"testing"
	"time"
)

// module is the prefix of every function in this module's packages, as a
// goroutine's stack names it.
const module = "github.com/maydaythecoder/trellis/tui/internal/"

// self is this package's prefix: the checker's own frames are not a leak.
const self = module + "leaktest."

// Deadline is how long Check waits for goroutines to end before it fails.
const Deadline = 5 * time.Second

var header = regexp.MustCompile(`^goroutine (\d+) \[`)

// goroutine is one stack from runtime.Stack's dump.
type goroutine struct {
	id    int
	stack string
}

// stacks is every goroutine but the caller's, in runtime.Stack's order.
func stacks() []goroutine {
	buf := make([]byte, 1<<16)
	for {
		n := runtime.Stack(buf, true)
		if n < len(buf) {
			buf = buf[:n]
			break
		}
		buf = make([]byte, 2*len(buf))
	}
	var out []goroutine
	// The first block is the goroutine that asked: the test itself.
	for i, block := range strings.Split(string(buf), "\n\n") {
		m := header.FindStringSubmatch(block)
		if i == 0 || m == nil {
			continue
		}
		id, _ := strconv.Atoi(m[1])
		out = append(out, goroutine{id: id, stack: block})
	}
	return out
}

// ours reports whether a goroutine is this module's: a frame of its stack
// is in one of the module's packages other than this one, or one of them
// started it ("created by"), so a goroutine that module code started
// inside a library is counted too.
func ours(stack string) bool {
	for _, line := range strings.Split(stack, "\n") {
		line = strings.TrimPrefix(line, "created by ")
		if strings.HasPrefix(line, module) && !strings.HasPrefix(line, self) {
			return true
		}
	}
	return false
}

// Check notes which goroutines are running and returns a function that
// fails t when a goroutine started since, whose stack passes through this
// module's packages, is still running at the Deadline. Call it first in a
// test:
//
//	defer leaktest.Check(t)()
func Check(t testing.TB) func() {
	t.Helper()
	return CheckWithin(t, Deadline)
}

// CheckWithin is Check with its own deadline, for a test of the check.
func CheckWithin(t testing.TB, deadline time.Duration) func() {
	t.Helper()
	before := map[int]bool{}
	for _, g := range stacks() {
		before[g.id] = true
	}
	return func() {
		t.Helper()
		var left []goroutine
		wait := time.Microsecond
		for end := time.Now().Add(deadline); ; {
			left = left[:0]
			for _, g := range stacks() {
				if !before[g.id] && ours(g.stack) {
					left = append(left, g)
				}
			}
			if len(left) == 0 || time.Now().After(end) {
				break
			}
			time.Sleep(wait)
			wait = min(2*wait, 100*time.Millisecond)
		}
		if len(left) > 0 {
			var dump strings.Builder
			for _, g := range left {
				dump.WriteString(g.stack)
				dump.WriteString("\n\n")
			}
			t.Errorf("%d goroutine(s) still running %v after the test:\n%s", len(left), deadline, dump.String())
		}
	}
}

// Main runs m's tests and then the goroutineleak profile, for a package's
// TestMain:
//
//	func TestMain(m *testing.M) { os.Exit(leaktest.Main(m)) }
//
// A leak the profile names fails the run even when every test passed. A Go
// without the profile (older than 1.27; go.mod's toolchain line asks for
// 1.27.2) is said on stderr and not counted as a failure.
func Main(m *testing.M) int {
	code := m.Run()
	if leaked := Leaked(os.Stderr); leaked > 0 {
		fmt.Fprintf(os.Stderr, "leaktest: the goroutineleak profile names %d goroutine(s) blocked forever\n", leaked)
		if code == 0 {
			code = 1
		}
	}
	return code
}

// Leaked writes the goroutineleak profile to w when it names a goroutine,
// and returns how many it names. Writing the profile is what runs the
// detection, so it is written every time; -1 means this Go has no such
// profile.
func Leaked(w io.Writer) int {
	p := pprof.Lookup("goroutineleak")
	if p == nil {
		fmt.Fprintln(w, "leaktest: this Go has no goroutineleak profile (1.27 added it); only Check ran")
		return -1
	}
	var buf bytes.Buffer
	if err := p.WriteTo(&buf, 1); err != nil {
		fmt.Fprintf(w, "leaktest: cannot write the goroutineleak profile: %v\n", err)
		return -1
	}
	n := p.Count()
	if n > 0 {
		w.Write(buf.Bytes())
	}
	return n
}
