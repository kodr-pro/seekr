use token_bucket::{Allow, TokenBucket};

#[test]
fn burst_up_to_capacity_then_deny_with_retry_hint() {
    let mut b = TokenBucket::new(3.0, 1.0);
    for _ in 0..3 {
        assert!(matches!(b.try_take(0.0), Allow::Allowed { .. }));
    }
    match b.try_take(0.0) {
        Allow::Denied { retry_in_secs } => {
            assert!((retry_in_secs - 1.0).abs() < 1e-6, "one token per second");
        }
        other => panic!("expected denial, got {other:?}"),
    }
}

#[test]
fn refill_unblocks_after_waiting() {
    let mut b = TokenBucket::new(1.0, 2.0);
    assert!(matches!(b.try_take(0.0), Allow::Allowed { .. }));
    assert!(matches!(b.try_take(0.0), Allow::Denied { .. }));
    assert!(matches!(b.try_take(0.5), Allow::Allowed { .. }), "1 token refilled");
}

#[test]
fn retry_hint_scales_with_deficit() {
    let mut b = TokenBucket::new(2.0, 0.5);
    assert!(matches!(b.try_take(0.0), Allow::Allowed { .. }));
    assert!(matches!(b.try_take(0.0), Allow::Allowed { .. }));
    assert!(matches!(b.try_take(0.0), Allow::Denied { .. }));
    match b.try_take(0.25) {
        // 0.5 tok/s * 0.25s = 0.125 tokens — still short of a full token
        Allow::Denied { retry_in_secs } => {
            assert!(retry_in_secs > 0.0 && retry_in_secs <= 2.0);
        }
        other => panic!("expected denial at 0.25s, got {other:?}"),
    }
    assert!(matches!(b.try_take(2.5), Allow::Allowed { .. }));
}

#[test]
fn remaining_is_reported_on_allow() {
    let mut b = TokenBucket::new(5.0, 1.0);
    match b.try_take(0.0) {
        Allow::Allowed { remaining } => {
            assert!((remaining - 4.0).abs() < 1e-9);
        }
        other => panic!("expected allow, got {other:?}"),
    }
}
