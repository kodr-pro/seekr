use lru_cache::LruCache;

#[test]
fn new_panics_on_zero_capacity() {
    let result = std::panic::catch_unwind(|| LruCache::new(0));
    assert!(result.is_err(), "capacity 0 must panic");
}

#[test]
fn put_then_get_roundtrip() {
    let mut c = LruCache::new(2);
    c.put("a", "1".into());
    c.put("b", "2".into());
    assert_eq!(c.get("a"), Some("1".into()));
    assert_eq!(c.get("b"), Some("2".into()));
    assert_eq!(c.len(), 2);
    assert!(!c.is_empty());
}

#[test]
fn get_returns_none_for_missing_key() {
    let mut c = LruCache::new(2);
    assert_eq!(c.get("nope"), None);
}

#[test]
fn update_refreshes_value_without_growing() {
    let mut c = LruCache::new(2);
    c.put("a", "1".into());
    c.put("a", "9".into());
    assert_eq!(c.len(), 1);
    assert_eq!(c.get("a"), Some("9".into()));
}
