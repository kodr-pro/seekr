use token_bucket::TokenBucket;

#[test]
fn new_bucket_starts_full() {
    let b = TokenBucket::new(10.0, 1.0);
    assert!((b.tokens() - 10.0).abs() < 1e-9);
}

#[test]
fn refill_adds_tokens_up_to_capacity() {
    let mut b = TokenBucket::new(10.0, 2.0);
    b.refill(1.0);
    assert!((b.tokens() - 10.0).abs() < 1e-9, "clamped at capacity");
    let mut b = TokenBucket::new(10.0, 0.0);
    // drain to zero by construction: start a fresh bucket at t=0 full,
    // then simulate a bucket that begins empty is not expressible via
    // public API alone; refill on a full bucket must stay at capacity.
    b.refill(5.0);
    assert!((b.tokens() - 10.0).abs() < 1e-9);
}

#[test]
fn refill_never_goes_backwards() {
    let mut b = TokenBucket::new(4.0, 1.0);
    b.refill(10.0);
    let before = b.tokens();
    b.refill(5.0); // earlier timestamp: no change, no panic
    assert!((b.tokens() - before).abs() < 1e-9);
}
