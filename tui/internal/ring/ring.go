// Package ring keeps the newest values of a stream that has no end, in a
// fixed number of slots (#846).
//
// The rpc client kept trellis's stderr in a slice and, once it held its
// cap, copied the whole slice for every new line: 500 strings moved per
// line of a chatty child. A ring writes each new value into the slot of
// the oldest instead, so once full a push allocates nothing and moves
// nothing, and what is dropped is counted, so a pane can say that some
// lines are not shown.
//
// A Ring is not safe for concurrent use; its owner holds the lock.
package ring

// Ring holds at most Cap values, the newest last.
type Ring[T any] struct {
	buf     []T
	oldest  int // where the oldest value is, once buf is full
	dropped int
}

// New makes a ring that keeps the newest n values. n below 1 keeps one:
// a ring that kept nothing would drop every value it was given.
func New[T any](n int) *Ring[T] {
	return &Ring[T]{buf: make([]T, 0, max(n, 1))}
}

// Push adds v as the newest value, dropping the oldest when the ring is
// full.
func (r *Ring[T]) Push(v T) {
	if len(r.buf) < cap(r.buf) {
		r.buf = append(r.buf, v)
		return
	}
	r.buf[r.oldest] = v
	r.oldest = (r.oldest + 1) % len(r.buf)
	r.dropped++
}

// Len is how many values the ring holds.
func (r *Ring[T]) Len() int { return len(r.buf) }

// Cap is how many values the ring keeps at most.
func (r *Ring[T]) Cap() int { return cap(r.buf) }

// Dropped is how many values were pushed out to make room for newer ones.
func (r *Ring[T]) Dropped() int { return r.dropped }

// Slice is a copy of the values held, oldest first. A copy, so a caller
// may keep it while the ring goes on.
func (r *Ring[T]) Slice() []T {
	out := make([]T, 0, len(r.buf))
	out = append(out, r.buf[r.oldest:]...)
	return append(out, r.buf[:r.oldest]...)
}
