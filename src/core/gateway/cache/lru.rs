use parking_lot::Mutex;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::hash::Hash;
use std::time::Duration;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheStatus {
    Hit,
    Miss,
    Expired,
}

#[derive(Debug)]
struct CacheEntry<V> {
    value: V,
    expires_at: Option<Instant>,
}

impl<V> CacheEntry<V> {
    fn new(value: V, ttl: Option<Duration>) -> Self {
        // checked_add guards against overflow on extreme TTL values;
        // overflow silently produces no expiry, which is acceptable.
        let expires_at = ttl.and_then(|d| Instant::now().checked_add(d));
        Self { value, expires_at }
    }

    fn is_expired(&self, now: Instant) -> bool {
        self.expires_at.is_some_and(|t| now >= t)
    }
}

// ---------------------------------------------------------------------------
// Inner state — all mutation goes through LocalLruCacheInner so that
// LocalLruCache only needs to acquire the lock once per public call.
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct LocalLruCacheInner<K, V> {
    capacity: usize,
    entries: HashMap<K, CacheEntry<V>>,
    /// LRU order: front = least-recently-used, back = most-recently-used.
    order: VecDeque<K>,
}

impl<K, V> LocalLruCacheInner<K, V>
where
    K: Eq + Hash + Clone,
{
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: HashMap::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
        }
    }

    /// Remove `key` from the LRU order queue. O(n) — acceptable for small capacities.
    fn remove_from_order(&mut self, key: &K) {
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
    }

    /// Move `key` to the MRU end of the queue.
    fn touch(&mut self, key: &K) {
        self.remove_from_order(key);
        self.order.push_back(key.clone());
    }

    /// Check expiry for a single key on access; remove and return true if expired.
    fn evict_if_expired(&mut self, key: &K, now: Instant) -> bool {
        if self.entries.get(key).is_some_and(|e| e.is_expired(now)) {
            self.entries.remove(key);
            self.remove_from_order(key);
            return true;
        }
        false
    }

    /// Evict from the LRU tail until the entry count is within capacity.
    ///
    /// Follows OpenResty lru-rbtree semantics: no full-table TTL scan.
    /// Expired entries that happen to sit at the LRU tail are evicted
    /// just like any other entry; entries that are expired but not at
    /// the tail survive until they are accessed (lazy expiry).
    fn evict_to_capacity(&mut self) {
        while self.entries.len() > self.capacity {
            let Some(lru_key) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&lru_key);
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// A fixed-capacity, TTL-aware LRU cache backed by a single mutex.
///
/// Expiry is **lazy**: a stale entry is only detected and removed when it is
/// accessed (`get` / `get_with_status`). Inserts never scan the table for
/// expired entries — this matches the behaviour of OpenResty's `lru-rbtree`.
///
/// Suitable for small-capacity, short-TTL plugin result caches where
/// simplicity and predictable per-operation cost matter more than
/// minimising peak memory usage.
#[derive(Debug)]
pub struct LocalLruCache<K, V> {
    capacity: usize,
    inner: Mutex<LocalLruCacheInner<K, V>>,
}

impl<K, V> LocalLruCache<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "LocalLruCache capacity must be greater than 0");
        Self {
            capacity,
            inner: Mutex::new(LocalLruCacheInner::new(capacity)),
        }
    }

    /// The maximum number of entries this cache will hold.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Current number of entries, including any that may have expired
    /// but not yet been accessed.
    pub fn len(&self) -> usize {
        self.inner.lock().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.lock().entries.is_empty()
    }

    pub fn clear(&self) {
        let mut inner = self.inner.lock();
        inner.entries.clear();
        inner.order.clear();
    }

    pub fn remove(&self, key: &K) -> Option<V> {
        let mut inner = self.inner.lock();
        if let Some(entry) = inner.entries.remove(key) {
            inner.remove_from_order(key);
            Some(entry.value)
        } else {
            None
        }
    }

    /// Insert or update `key` with the given `ttl`.
    ///
    /// `ttl = None` means the entry never expires.
    /// `ttl = Some(Duration::ZERO)` is treated as an immediate delete.
    pub fn insert(&self, key: K, value: V, ttl: Option<Duration>) {
        if ttl.is_some_and(|d| d.is_zero()) {
            self.remove(&key);
            return;
        }

        let mut inner = self.inner.lock();
        inner.entries.insert(key.clone(), CacheEntry::new(value, ttl));
        inner.touch(&key);
        inner.evict_to_capacity();
    }

    pub fn get(&self, key: &K) -> Option<V> {
        self.get_with_status(key).0
    }

    /// Returns the cached value and a `CacheStatus` describing why the
    /// lookup succeeded or failed.
    pub fn get_with_status(&self, key: &K) -> (Option<V>, CacheStatus) {
        let now = Instant::now();
        let mut inner = self.inner.lock();

        if !inner.entries.contains_key(key) {
            return (None, CacheStatus::Miss);
        }

        if inner.evict_if_expired(key, now) {
            return (None, CacheStatus::Expired);
        }

        // Safety: we confirmed the key exists above and it was not expired.
        let value = inner.entries.get(key).unwrap().value.clone();
        inner.touch(key);
        (Some(value), CacheStatus::Hit)
    }
}

#[cfg(test)]
mod tests {
    use super::{CacheStatus, LocalLruCache};
    use std::time::Duration;

    #[test]
    fn test_insert_and_get() {
        let cache = LocalLruCache::new(2);

        cache.insert("a", 1, Some(Duration::from_secs(60)));

        assert_eq!(cache.get(&"a"), Some(1));
        assert_eq!(cache.get_with_status(&"a"), (Some(1), CacheStatus::Hit));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_ttl_expiration() {
        let cache = LocalLruCache::new(2);

        cache.insert("a", 1, Some(Duration::from_millis(10)));
        std::thread::sleep(Duration::from_millis(20));

        assert_eq!(cache.get_with_status(&"a"), (None, CacheStatus::Expired));
        assert!(cache.is_empty());
    }

    #[test]
    fn test_lru_eviction() {
        let cache = LocalLruCache::new(2);

        cache.insert("a", 1, None);
        cache.insert("b", 2, None);
        assert_eq!(cache.get(&"a"), Some(1)); // touch a, now b is LRU
        cache.insert("c", 3, None);

        assert_eq!(cache.get(&"a"), Some(1));
        assert_eq!(cache.get(&"b"), None);
        assert_eq!(cache.get(&"c"), Some(3));
    }

    #[test]
    fn test_update_existing_value_refreshes_ttl_and_lru() {
        let cache = LocalLruCache::new(2);

        cache.insert("a", 1, Some(Duration::from_millis(50)));
        cache.insert("b", 2, None);
        cache.insert("a", 10, Some(Duration::from_secs(1)));
        cache.insert("c", 3, None);

        assert_eq!(cache.get(&"a"), Some(10));
        assert_eq!(cache.get(&"b"), None);
        assert_eq!(cache.get(&"c"), Some(3));
    }

    #[test]
    fn test_zero_ttl_removes_value() {
        let cache = LocalLruCache::new(2);

        cache.insert("a", 1, None);
        cache.insert("a", 2, Some(Duration::ZERO));

        assert_eq!(cache.get(&"a"), None);
        assert!(cache.is_empty());
    }

    #[test]
    fn test_remove_and_clear() {
        let cache = LocalLruCache::new(3);

        cache.insert("a", 1, None);
        cache.insert("b", 2, None);

        assert_eq!(cache.remove(&"a"), Some(1));
        assert_eq!(cache.get(&"a"), None);
        assert_eq!(cache.len(), 1);

        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn test_lru_eviction_does_not_scan_expired_entries() {
        // OpenResty lru semantics: insert never does a full TTL scan.
        // When at capacity, the LRU tail is evicted regardless of expiry state.
        //
        // Setup: capacity=2, insert a (no TTL) then b (short TTL).
        // Touch b so order is [a(LRU), b(MRU)].
        // Let b expire.
        // Insert c: capacity is full, evict LRU tail → evicts a (not b, even though b is expired).
        // b stays in the map until it is accessed (lazy expiry).
        let cache = LocalLruCache::new(2);

        cache.insert("a", 1, None);
        cache.insert("b", 2, Some(Duration::from_millis(10)));
        assert_eq!(cache.get(&"b"), Some(2)); // touch b → order: [a(LRU), b(MRU)]
        std::thread::sleep(Duration::from_millis(20)); // b is now expired but not evicted yet

        cache.insert("c", 3, None); // evicts a (LRU tail), b stays

        assert_eq!(cache.get(&"a"), None); // a was LRU-evicted
        assert_eq!(cache.get_with_status(&"b"), (None, CacheStatus::Expired)); // b expired lazily
        assert_eq!(cache.get(&"c"), Some(3));
    }
}
