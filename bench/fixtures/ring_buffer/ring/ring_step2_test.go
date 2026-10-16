package ring

import "testing"

func TestPushWhenFullReturnsFalse(t *testing.T) {
	r := New(2)
	r.Push(1)
	r.Push(2)
	if r.Push(3) {
		t.Fatal("push on full must return false")
	}
	if r.Len() != 2 {
		t.Fatalf("full push changed len: %d, want 2", r.Len())
	}
	// the rejected push must not corrupt order
	if v, _ := r.Pop(); v != 1 {
		t.Fatalf("oldest = %d, want 1 (rejected push leaked in)", v)
	}
}

func TestFIFOOrderAcrossWraparound(t *testing.T) {
	r := New(2)
	for i := 1; i <= 6; i++ {
		if !r.Push(i) {
			t.Fatalf("push %d failed unexpectedly", i)
		}
		if v, _ := r.Pop(); v != i {
			t.Fatalf("pop = %d, want %d", v, i)
		}
	}
}

func TestInterleavedDrainAndRefill(t *testing.T) {
	r := New(2)
	r.Push(1)
	r.Push(2)
	r.Pop()
	r.Push(3) // fits after the drain
	r.Pop()
	r.Pop()
	if r.Len() != 0 {
		t.Fatalf("len after drain = %d, want 0", r.Len())
	}
}
