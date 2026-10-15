//! Bounded LRU cache with O(1) get/put.
//!
//! Step 1 implements the core store; step 2 wires eviction on overflow.

use std::collections::HashMap;

/// A bounded least-recently-used cache.
///
/// Capacity is fixed at construction. `get` refreshes recency;
/// `put` inserts or updates and refreshes recency; when the cache
/// exceeds capacity the least recently used entry is evicted.
pub struct LruCache {
    capacity: usize,
    // Step 1: replace these internals with your own design.
    map: HashMap<String, String>,
}

impl LruCache {
    /// Create a cache holding at most `capacity` entries.
    /// Panics if capacity is 0.
    pub fn new(capacity: usize) -> Self {
        todo!("Step 1")
    }

    /// Return the value for `key` and refresh its recency, or None.
    pub fn get(&mut self, key: &str) -> Option<String> {
        todo!("Step 1")
    }

    /// Insert or update `key` -> `value`, refresh recency, and evict
    /// the least recently used entry when size would exceed capacity.
    pub fn put(&mut self, key: &str, value: String) {
        todo!("Step 2")
    }

    /// Current number of entries.
    pub fn len(&self) -> usize {
        todo!("Step 1")
    }

    /// True when the cache holds no entries.
    pub fn is_empty(&self) -> bool {
        todo!("Step 1")
    }
}
