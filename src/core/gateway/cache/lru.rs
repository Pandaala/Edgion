use parking_lot::Mutex;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

/// A point-in-time snapshot of cache operation counts.
///
/// Obtained via [`LocalLruCache::stats`]. Because the four counters are read
/// with `Relaxed` ordering and are not loaded atomically as a group, this
/// snapshot may reflect a transient state under concurrent writes. Treat
/// `hit_ratio()` as an approximation suitable for metrics and dashboards,
/// not as a strongly consistent value.
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// Number of `get` calls that returned a live cached value.
    pub hits: u64,
    /// Number of `get` calls where the key was not found.
    pub misses: u64,
    /// Number of `get` calls where the key was found but its TTL had expired.
    pub expirations: u64,
    /// Number of `insert` calls that displaced a live (non-expired) entry
    /// because the cache was at capacity. Entries cleared by
    /// `prune_expired_from_tail` or explicit `remove` do not count.
    pub evictions: u64,
}

impl CacheStats {
    /// Hit ratio over all `get` operations: `hits / (hits + misses + expirations)`.
    ///
    /// Returns `0.0` if no `get` calls have been made yet.
    pub fn hit_ratio(&self) -> f64 {
        let total = self.hits + self.misses + self.expirations;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheStatus {
    Hit,
    Miss,
    Expired,
}

#[derive(Debug, Clone)]
struct CacheNode<K, V> {
    key: Option<K>,
    value: Option<V>,
    expires_at: Option<Instant>,
    prev: Option<usize>,
    next: Option<usize>,
    next_free: Option<usize>,
    occupied: bool,
}

impl<K, V> Default for CacheNode<K, V> {
    fn default() -> Self {
        Self {
            key: None,
            value: None,
            expires_at: None,
            prev: None,
            next: None,
            next_free: None,
            occupied: false,
        }
    }
}

impl<K, V> CacheNode<K, V> {
    fn is_expired(&self, now: Instant) -> bool {
        self.expires_at.is_some_and(|expires_at| now >= expires_at)
    }

    fn reset(&mut self) {
        self.key = None;
        self.value = None;
        self.expires_at = None;
        self.prev = None;
        self.next = None;
        self.occupied = false;
    }
}

#[derive(Debug)]
struct LocalLruCacheInner<K, V> {
    capacity: usize,
    len: usize,
    map: HashMap<K, usize>,
    nodes: Vec<CacheNode<K, V>>,
    head: Option<usize>,
    tail: Option<usize>,
    free_head: Option<usize>,
}

impl<K, V> LocalLruCacheInner<K, V>
where
    K: Eq + Hash + Clone,
{
    fn new(capacity: usize) -> Self {
        let mut nodes = Vec::with_capacity(capacity);
        for idx in 0..capacity {
            let next_free = if idx + 1 < capacity { Some(idx + 1) } else { None };
            let mut node = CacheNode::default();
            node.next_free = next_free;
            nodes.push(node);
        }

        Self {
            capacity,
            len: 0,
            map: HashMap::with_capacity(capacity),
            nodes,
            head: None,
            tail: None,
            free_head: Some(0),
        }
    }

    /// Allocates a node index for a new entry.
    ///
    /// Returns `(index, evicted)` where `evicted` is `true` only if a live
    /// (non-expired) tail entry was displaced to make room. Returns `false`
    /// when: a free-list slot was available, or the displaced tail entry was
    /// already expired (cleanup, not eviction).
    fn alloc_index(&mut self) -> (usize, bool) {
        if let Some(idx) = self.free_head {
            self.free_head = self.nodes[idx].next_free;
            self.nodes[idx].next_free = None;
            return (idx, false);
        }

        let tail = self.tail.expect("tail must exist when cache is full");
        let now = Instant::now();
        let is_live = self.nodes[tail].occupied && !self.nodes[tail].is_expired(now);
        self.remove_index(tail);
        (tail, is_live)
    }

    fn release_index(&mut self, idx: usize) {
        self.nodes[idx].reset();
        self.nodes[idx].next_free = self.free_head;
        self.free_head = Some(idx);
    }

    fn detach(&mut self, idx: usize) {
        let prev = self.nodes[idx].prev;
        let next = self.nodes[idx].next;

        match prev {
            Some(prev_idx) => self.nodes[prev_idx].next = next,
            None => self.head = next,
        }

        match next {
            Some(next_idx) => self.nodes[next_idx].prev = prev,
            None => self.tail = prev,
        }

        self.nodes[idx].prev = None;
        self.nodes[idx].next = None;
    }

    fn attach_head(&mut self, idx: usize) {
        self.nodes[idx].prev = None;
        self.nodes[idx].next = self.head;

        if let Some(head_idx) = self.head {
            self.nodes[head_idx].prev = Some(idx);
        } else {
            self.tail = Some(idx);
        }

        self.head = Some(idx);
    }

    fn touch(&mut self, idx: usize) {
        if self.head == Some(idx) {
            return;
        }
        self.detach(idx);
        self.attach_head(idx);
    }

    fn remove_index(&mut self, idx: usize) -> Option<V> {
        if !self.nodes[idx].occupied {
            return None;
        }

        let key = self.nodes[idx]
            .key
            .as_ref()
            .expect("occupied node must have key")
            .clone();

        self.detach(idx);
        self.map.remove(&key);
        self.len -= 1;

        let value = self.nodes[idx].value.take();
        self.release_index(idx);
        value
    }

    fn prune_expired_from_tail(&mut self, now: Instant) {
        let mut cursor = self.tail;

        while let Some(idx) = cursor {
            let prev = self.nodes[idx].prev;
            if self.nodes[idx].occupied && self.nodes[idx].is_expired(now) {
                self.remove_index(idx);
            }
            cursor = prev;
        }
    }

    fn insert(&mut self, key: K, value: V, ttl: Option<Duration>) -> bool {
        let now = Instant::now();

        if let Some(&idx) = self.map.get(&key) {
            self.nodes[idx].value = Some(value);
            self.nodes[idx].expires_at = ttl.and_then(|ttl| now.checked_add(ttl));
            self.touch(idx);
            return false; // key-update: no eviction
        }

        self.prune_expired_from_tail(now);

        let (idx, evicted) = self.alloc_index();
        self.nodes[idx].key = Some(key.clone());
        self.nodes[idx].value = Some(value);
        self.nodes[idx].expires_at = ttl.and_then(|ttl| now.checked_add(ttl));
        self.nodes[idx].occupied = true;
        self.nodes[idx].next_free = None;

        self.attach_head(idx);
        self.map.insert(key, idx);
        self.len += 1;

        evicted
    }

    fn get(&mut self, key: &K) -> (Option<V>, CacheStatus)
    where
        V: Clone,
    {
        let Some(&idx) = self.map.get(key) else {
            return (None, CacheStatus::Miss);
        };

        let now = Instant::now();
        if self.nodes[idx].is_expired(now) {
            self.remove_index(idx);
            return (None, CacheStatus::Expired);
        }

        let value = self.nodes[idx]
            .value
            .as_ref()
            .expect("occupied node must have value")
            .clone();
        self.touch(idx);
        (Some(value), CacheStatus::Hit)
    }

    fn remove(&mut self, key: &K) -> Option<V> {
        let idx = *self.map.get(key)?;
        self.remove_index(idx)
    }

    fn clear(&mut self) {
        self.map.clear();
        self.len = 0;
        self.head = None;
        self.tail = None;
        self.free_head = if self.capacity > 0 { Some(0) } else { None };

        for (idx, node) in self.nodes.iter_mut().enumerate() {
            node.reset();
            node.next_free = if idx + 1 < self.capacity { Some(idx + 1) } else { None };
        }
    }
}

/// A small process-local TTL/LRU cache for gateway-side short-lived results.
///
/// This cache is intentionally scoped to the "OpenResty-style local cache" use
/// case we discussed:
/// - process-local shared cache
/// - short TTL result caching
/// - fixed entry capacity
/// - ordinary in-process locking
///
/// The implementation keeps a fixed-capacity node pool plus a doubly-linked LRU
/// list under one mutex. That gives us predictable metadata allocation and keeps
/// lock-held work to O(1) for touch/insert/remove, without pulling in a heavier
/// cache dependency yet.
///
/// `V` is fully caller-defined. If cloning values is expensive, callers can
/// store `Arc<T>` as the value type and keep cache hits cheap.
#[derive(Debug)]
pub struct LocalLruCache<K, V> {
    inner: Mutex<LocalLruCacheInner<K, V>>,
    hits: AtomicU64,
    misses: AtomicU64,
    expirations: AtomicU64,
    evictions: AtomicU64,
}

impl<K, V> LocalLruCache<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "LocalLruCache capacity must be greater than 0");
        Self {
            inner: Mutex::new(LocalLruCacheInner::new(capacity)),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            expirations: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    /// Returns a point-in-time snapshot of cache operation counters.
    ///
    /// Reads four atomic counters with `Relaxed` ordering — no lock is
    /// acquired. See [`CacheStats`] for the approximation caveat.
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits.load(Relaxed),
            misses: self.misses.load(Relaxed),
            expirations: self.expirations.load(Relaxed),
            evictions: self.evictions.load(Relaxed),
        }
    }

    pub fn capacity(&self) -> usize {
        self.inner.lock().capacity
    }

    pub fn len(&self) -> usize {
        self.inner.lock().len
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&self) {
        self.inner.lock().clear();
    }

    pub fn remove(&self, key: &K) -> Option<V> {
        self.inner.lock().remove(key)
    }

    pub fn insert(&self, key: K, value: V, ttl: Option<Duration>) {
        if ttl.is_some_and(|ttl| ttl.is_zero()) {
            self.remove(&key);
            return;
        }

        let evicted = self.inner.lock().insert(key, value, ttl); // MutexGuard dropped here
        if evicted {
            self.evictions.fetch_add(1, Relaxed);
        }
    }

    pub fn get(&self, key: &K) -> Option<V> {
        self.get_with_status(key).0
    }

    pub fn get_with_status(&self, key: &K) -> (Option<V>, CacheStatus) {
        let result = self.inner.lock().get(key); // MutexGuard dropped here
        match result.1 {
            CacheStatus::Hit     => { self.hits.fetch_add(1, Relaxed); }
            CacheStatus::Miss    => { self.misses.fetch_add(1, Relaxed); }
            CacheStatus::Expired => { self.expirations.fetch_add(1, Relaxed); }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{CacheStats, CacheStatus, LocalLruCache};
    use std::sync::Arc;
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
    fn test_insert_prunes_expired_before_evicting_valid_entry() {
        let cache = LocalLruCache::new(2);

        cache.insert("a", 1, None);
        cache.insert("b", 2, Some(Duration::from_millis(10)));
        assert_eq!(cache.get(&"b"), Some(2)); // make b more recent than a
        std::thread::sleep(Duration::from_millis(20));

        cache.insert("c", 3, None);

        assert_eq!(cache.get(&"a"), Some(1));
        assert_eq!(cache.get_with_status(&"b"), (None, CacheStatus::Miss));
        assert_eq!(cache.get(&"c"), Some(3));
    }

    #[test]
    fn test_value_can_be_arc_to_reduce_clone_cost() {
        let cache = LocalLruCache::new(2);
        let value = Arc::new(vec![1, 2, 3]);

        cache.insert("a", value.clone(), None);

        let cached = cache.get(&"a").unwrap();
        assert!(Arc::ptr_eq(&cached, &value));
    }

    #[test]
    fn test_stats_initial_zeros() {
        let cache: LocalLruCache<&str, i32> = LocalLruCache::new(4);
        let s = cache.stats();
        assert_eq!(s.hits, 0);
        assert_eq!(s.misses, 0);
        assert_eq!(s.expirations, 0);
        assert_eq!(s.evictions, 0);
        assert_eq!(s.hit_ratio(), 0.0);
    }

    #[test]
    fn test_hit_ratio_zero_when_no_gets() {
        // hit_ratio() must return 0.0 (not NaN) when denominator is zero
        let s = CacheStats::default();
        assert_eq!(s.hit_ratio(), 0.0);
        assert!(!s.hit_ratio().is_nan());
    }

    #[test]
    fn test_stats_hit_miss_expiration_counts() {
        let cache = LocalLruCache::new(4);

        // Two misses
        assert_eq!(cache.get(&"x"), None);
        assert_eq!(cache.get(&"y"), None);

        // One insert + one hit
        cache.insert("a", 1, None);
        assert_eq!(cache.get(&"a"), Some(1));

        // One insert with short TTL, wait, then one expiration
        cache.insert("b", 2, Some(Duration::from_millis(10)));
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(cache.get(&"b"), None);

        let s = cache.stats();
        assert_eq!(s.hits, 1);
        assert_eq!(s.misses, 2);
        assert_eq!(s.expirations, 1);
    }

    #[test]
    fn test_hit_ratio_calculation() {
        let cache = LocalLruCache::new(4);

        cache.insert("a", 1, None);
        cache.get(&"a"); // hit
        cache.get(&"b"); // miss

        let s = cache.stats();
        // 1 hit, 1 miss, 0 expirations -> ratio = 0.5
        assert!((s.hit_ratio() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_hit_ratio_zero_when_all_misses() {
        let cache: LocalLruCache<&str, i32> = LocalLruCache::new(4);
        cache.get(&"a");
        cache.get(&"b");
        let s = cache.stats();
        assert_eq!(s.hits, 0);
        assert_eq!(s.hit_ratio(), 0.0);
    }

    #[test]
    fn test_remove_does_not_affect_stats() {
        let cache = LocalLruCache::new(4);
        cache.insert("a", 1, None);
        cache.remove(&"a");
        let s = cache.stats();
        assert_eq!(s.hits, 0);
        assert_eq!(s.misses, 0);
        assert_eq!(s.expirations, 0);
        assert_eq!(s.evictions, 0);
    }

    #[test]
    fn test_eviction_count_when_capacity_full_and_live_entry_displaced() {
        let cache = LocalLruCache::new(2);
        cache.insert("a", 1, None);
        cache.insert("b", 2, None);
        // Cache is now full with two live entries. Next insert must evict one.
        cache.insert("c", 3, None);

        let s = cache.stats();
        assert_eq!(s.evictions, 1);
    }

    #[test]
    fn test_eviction_count_zero_when_expired_entry_freed_before_displacement() {
        // If prune_expired_from_tail frees a slot, no live entry is displaced.
        let cache = LocalLruCache::new(2);
        cache.insert("a", 1, None);
        cache.insert("b", 2, Some(Duration::from_millis(10)));
        std::thread::sleep(Duration::from_millis(20));

        // "b" is expired. insert("c") prunes "b" first, freeing a slot.
        // No live entry should be evicted.
        cache.insert("c", 3, None);

        let s = cache.stats();
        assert_eq!(s.evictions, 0);
    }

    #[test]
    fn test_overwrite_existing_key_does_not_count_as_eviction() {
        let cache = LocalLruCache::new(2);
        cache.insert("a", 1, None);
        cache.insert("b", 2, None);
        // Overwriting "a" — no new slot needed, no eviction.
        cache.insert("a", 99, None);

        let s = cache.stats();
        assert_eq!(s.evictions, 0);
    }

    #[test]
    fn test_zero_ttl_insert_does_not_count_as_eviction() {
        let cache = LocalLruCache::new(2);
        cache.insert("a", 1, None);
        cache.insert("a", 2, Some(Duration::ZERO)); // removes "a", never calls inner.insert
        let s = cache.stats();
        assert_eq!(s.evictions, 0);
    }

    #[test]
    fn test_clear_reuses_preallocated_slots() {
        let cache = LocalLruCache::new(2);

        cache.insert("a", 1, None);
        cache.insert("b", 2, None);
        cache.clear();

        cache.insert("c", 3, None);
        cache.insert("d", 4, None);

        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get(&"c"), Some(3));
        assert_eq!(cache.get(&"d"), Some(4));
    }
}
