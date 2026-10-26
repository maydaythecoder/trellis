package ring

import (
	"slices"
	"testing"
)

// #846: the ring keeps the newest N in the order they came, counts what it
// dropped, and hands out a copy, so a caller's slice does not change under
// it as the ring goes on.
func TestRingKeepsTheNewestN(t *testing.T) {
	for _, n := range []int{1, 2, 3, 7} {
		for pushed := 0; pushed <= 3*n+1; pushed++ {
			r := New[int](n)
			var all []int
			for i := 0; i < pushed; i++ {
				r.Push(i)
				all = append(all, i)
			}
			want := all[max(len(all)-n, 0):]
			if got := r.Slice(); !slices.Equal(got, want) {
				t.Fatalf("cap %d after %d: %v, want %v", n, pushed, got, want)
			}
			if r.Len() != len(want) || r.Cap() != n || r.Dropped() != len(all)-len(want) {
				t.Fatalf("cap %d after %d: len %d cap %d dropped %d", n, pushed, r.Len(), r.Cap(), r.Dropped())
			}
		}
	}
	r := New[string](2)
	r.Push("a")
	r.Push("b")
	kept := r.Slice()
	r.Push("c")
	if !slices.Equal(kept, []string{"a", "b"}) {
		t.Fatalf("a slice handed out changed as the ring went on: %v", kept)
	}
}

// Once full, a push writes into the oldest slot: no allocation and no
// copy, however many values come (the rpc client's stderr copied 500
// strings per line once full).
func TestRingPushAllocatesNothingOnceFull(t *testing.T) {
	r := New[string](500)
	for i := 0; i < 500; i++ {
		r.Push("line")
	}
	if a := testing.AllocsPerRun(1000, func() { r.Push("line") }); a != 0 {
		t.Fatalf("%v allocations per push once full", a)
	}
}

// A ring asked to keep fewer than one keeps one, rather than dropping
// every value it is given.
func TestRingBelowOneKeepsOne(t *testing.T) {
	r := New[int](0)
	r.Push(1)
	r.Push(2)
	if got := r.Slice(); !slices.Equal(got, []int{2}) || r.Dropped() != 1 {
		t.Fatalf("%v, dropped %d", got, r.Dropped())
	}
}
