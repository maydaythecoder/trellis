// The checks are tested from outside the package: a goroutine started in
// package leaktest itself is the checker's own and is never reported.
package leaktest_test

import (
	"fmt"
	"io"
	"strings"
	"testing"
	"time"

	"github.com/maydaythecoder/trellis/tui/internal/leaktest"
)

// recorder is a testing.TB that keeps what Check reported instead of
// failing the test that runs it. Check calls only Helper and Errorf.
type recorder struct {
	testing.TB
	failed string
}

func (r *recorder) Helper() {}

func (r *recorder) Errorf(format string, args ...any) { r.failed = fmt.Sprintf(format, args...) }

// #846: a goroutine started during the test and still running at the
// deadline fails it, with its stack in the report so the reader sees
// where it is stuck.
func TestLeaktestCheckNamesAGoroutineLeftRunning(t *testing.T) {
	release := make(chan struct{})
	defer close(release)
	r := &recorder{}
	done := leaktest.CheckWithin(r, 50*time.Millisecond)
	go func() { <-release }()
	done()
	if !strings.Contains(r.failed, "1 goroutine(s) still running") ||
		!strings.Contains(r.failed, "leaktest_test.TestLeaktestCheckNamesAGoroutineLeftRunning") {
		t.Fatalf("the goroutine left running was not reported: %q", r.failed)
	}
}

// One that ends before the deadline is not a leak: Check waits for it,
// backing off, rather than failing on a goroutine already on its way out.
func TestLeaktestCheckWaitsForAGoroutineThatEnds(t *testing.T) {
	r := &recorder{}
	done := leaktest.CheckWithin(r, 2*time.Second)
	go func() { time.Sleep(20 * time.Millisecond) }()
	done()
	if r.failed != "" {
		t.Fatalf("a goroutine that ended was reported: %q", r.failed)
	}
}

// One already running when Check was called is the earlier test's or the
// runtime's, not this test's.
func TestLeaktestCheckIgnoresWhatRanBefore(t *testing.T) {
	release := make(chan struct{})
	defer close(release)
	go func() { <-release }()
	time.Sleep(10 * time.Millisecond)
	r := &recorder{}
	leaktest.CheckWithin(r, 50*time.Millisecond)()
	if r.failed != "" {
		t.Fatalf("a goroutine that ran before Check was reported: %q", r.failed)
	}
}

// plant leaves a goroutine blocked forever on a channel nothing else can
// reach once plant returns. It stays for the life of this test binary,
// which is the point: the profile has to find it.
func plant() {
	ch := make(chan int)
	go func() { <-ch }()
}

// #846: the goroutineleak profile names a goroutine blocked forever on a
// channel no one can send on. A Go without the profile fails here, since
// go.mod's toolchain line asks for one that has it.
func TestLeaktestProfileNamesAGoroutineBlockedForever(t *testing.T) {
	plant()
	time.Sleep(10 * time.Millisecond)
	if n := leaktest.Leaked(io.Discard); n < 1 {
		t.Fatalf("the goroutineleak profile named %d goroutine(s); one was left blocked forever", n)
	}
}
