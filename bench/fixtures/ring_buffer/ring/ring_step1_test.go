package ring

import "testing"

func TestNewRejectsZeroCapacity(t *testing.T) {
	defer func() {
		if recover() == nil {
			t.Fatal("capacity 0 must panic")
		}
	}()
	New(0)
}

func TestPushPopRoundtrip(t *testing.T) {
	r := New(3)
	if !r.Push(1) || !r.Push(2) || !r.Push(3) {
		t.Fatal("push under capacity must succeed")
	}
	if r.Len() != 3 {
		t.Fatalf("len = %d, want 3", r.Len())
	}
	for _, want := range []int{1, 2, 3} {
		v, ok := r.Pop()
		if !ok || v != want {
			t.Fatalf("pop = %d,%v want %d,true", v, ok, want)
		}
	}
	if _, ok := r.Pop(); ok {
		t.Fatal("pop on empty must report false")
	}
}

func TestCapReporting(t *testing.T) {
	r := New(5)
	if r.Cap() != 5 {
		t.Fatalf("cap = %d, want 5", r.Cap())
	}
	if r.Len() != 0 {
		t.Fatalf("len = %d, want 0", r.Len())
	}
}
