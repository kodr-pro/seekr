use lru_cache::LruCache;

#[test]
fn evicts_least_recently_used_on_overflow() {
    let mut c = LruCache::new(2);
    c.put("a", "1".into());
    c.put("b", "2".into());
    // touch "a" so "b" becomes the LRU entry
    assert_eq!(c.get("a"), Some("1".into()));
    c.put("c", "3".into()); // must evict "b"
    assert_eq!(c.get("b"), None, "b was LRU and must be evicted");
    assert_eq!(c.get("a"), Some("1".into()));
    assert_eq!(c.get("c"), Some("3".into()));
    assert_eq!(c.len(), 2);
}

#[test]
fn eviction_respects_get_order_not_insert_order() {
    let mut c = LruCache::new(3);
    c.put("x", "1".into());
    c.put("y", "2".into());
    c.put("z", "3".into());
    // recency now: x < y < z; touch x and y so z is LRU
    c.get("x");
    c.get("y");
    c.put("w", "4".into()); // evicts z
    assert_eq!(c.get("z"), None);
    assert_eq!(c.get("x"), Some("1".into()));
    assert_eq!(c.get("y"), Some("2".into()));
    assert_eq!(c.get("w"), Some("4".into()));
}

#[test]
fn capacity_one_cycles_cleanly() {
    let mut c = LruCache::new(1);
    c.put("a", "1".into());
    c.put("b", "2".into());
    assert_eq!(c.get("a"), None);
    assert_eq!(c.get("b"), Some("2".into()));
    assert_eq!(c.len(), 1);
}
