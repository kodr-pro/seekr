//! A token-bucket rate limiter driven by explicit elapsed time.

/// Bucket with `capacity` tokens, refilled at `tokens_per_sec`.
///
/// Time is passed explicitly (`now_secs`), so behavior is deterministic
/// in tests. `tokens` never exceeds capacity; partial refills are
/// fractional.
pub struct TokenBucket {
    capacity: f64,
    tokens_per_sec: f64,
    tokens: f64,
    last_secs: f64,
}

#[derive(Debug, PartialEq)]
pub enum Allow {
    Allowed { remaining: f64 },
    Denied { retry_in_secs: f64 },
}

impl TokenBucket {
    /// New bucket, starting full.
    pub fn new(capacity: f64, tokens_per_sec: f64) -> Self {
        todo!("Step 1")
    }

    /// Refill to `now_secs` (clamped at capacity) without taking
    /// anything. Later calls must not go backwards in time.
    pub fn refill(&mut self, now_secs: f64) {
        todo!("Step 1")
    }

    /// Refill to `now_secs`, then try to take one token.
    pub fn try_take(&mut self, now_secs: f64) -> Allow {
        todo!("Step 2")
    }

    /// Current token count (after no refill).
    pub fn tokens(&self) -> f64 {
        todo!("Step 1")
    }
}
