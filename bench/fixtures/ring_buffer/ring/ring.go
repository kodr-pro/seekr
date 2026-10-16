// Package ringbuffer provides a fixed-capacity FIFO ring.
package ring

// Ring is a fixed-capacity FIFO queue. Once full, Push returns false
// (no overwrite). Pop removes and returns the oldest element; ok is
// false when empty. Len and Cap report occupancy.
type Ring struct {
	// Step 1: replace these internals with your own design.
	capacity int
}

// New returns a Ring holding at most capacity elements.
// Panics if capacity is 0.
func New(capacity int) *Ring {
	panic("Step 1")
}

// Push appends v. Returns false (leaving the ring unchanged) when full.
func (r *Ring) Push(v int) bool {
	panic("Step 1")
}

// Pop removes and returns the oldest element and whether it existed.
func (r *Ring) Pop() (v int, ok bool) {
	panic("Step 1")
}

// Len is the current number of elements.
func (r *Ring) Len() int {
	panic("Step 1")
}

// Cap is the fixed capacity.
func (r *Ring) Cap() int {
	panic("Step 1")
}
