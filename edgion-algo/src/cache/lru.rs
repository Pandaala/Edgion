use parking_lot::Mutex;
use std::borrow::Borrow;
use std::collections::{BTreeSet, HashMap};
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
    /// `prune_expired` or explicit `remove` do not count.
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
    /// Live TTL deadlines ordered earliest-first, keyed by `(deadline, node index)`.
    ///
    /// Holds exactly one record per occupied node that carries a TTL: every
    /// path that assigns, rewrites, or releases a deadline updates this index in
    /// the same step, so it never accumulates stale records and its size is
    /// bounded by `capacity` no matter how often a key is updated. Entries
    /// inserted without a TTL never enter the index.
    ///
    /// The node index disambiguates entries that share a deadline instant.
    expiration_index: BTreeSet<(Instant, usize)>,
}

impl<K, V> LocalLruCacheInner<K, V>
where
    K: Eq + Hash + Clone,
{
    fn new(capacity: usize) -> Self {
        let mut nodes = Vec::with_capacity(capacity);
        for idx in 0..capacity {
            let next_free = if idx + 1 < capacity { Some(idx + 1) } else { None };
            let node = CacheNode {
                next_free,
                ..CacheNode::default()
            };
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
            expiration_index: BTreeSet::new(),
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
        // insert() drains every expired entry before reaching here, so the tail
        // is effectively always live and this check is defensive rather than
        // load-bearing: it only fires for an entry whose deadline falls in the
        // sub-microsecond window between insert()'s clock read and this one.
        // Such an entry is then accounted as cleanup, not eviction, which is the
        // conservative direction for the counter contract documented on
        // CacheStats::evictions.
        let now = Instant::now();
        let is_live = self.nodes[tail].occupied && !self.nodes[tail].is_expired(now);
        self.remove_index(tail);

        // remove_index() returns the released tail to the free list. Consume
        // that slot here before handing it to the caller; otherwise the next
        // allocation can reuse the same occupied node and corrupt the list.
        debug_assert_eq!(self.free_head, Some(tail));
        self.free_head = self.nodes[tail].next_free;
        self.nodes[tail].next_free = None;
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

        // Drop the expiration record before release_index() resets the node and
        // clears the deadline the record is keyed by. This is the single release
        // funnel for eviction, lazy expiry, and explicit removal alike.
        if let Some(expires_at) = self.nodes[idx].expires_at {
            self.expiration_index.remove(&(expires_at, idx));
        }

        self.detach(idx);
        self.map.remove(&key);
        self.len -= 1;

        let value = self.nodes[idx].value.take();
        self.release_index(idx);
        value
    }

    /// Keeps the expiration index in sync with one node's deadline change.
    ///
    /// The index holds at most one record per node, so rewriting a TTL must drop
    /// the previous record before adding the new one; clearing a TTL only drops.
    fn sync_expiration_record(&mut self, idx: usize, previous: Option<Instant>, next: Option<Instant>) {
        if previous == next {
            return;
        }
        if let Some(previous) = previous {
            self.expiration_index.remove(&(previous, idx));
        }
        if let Some(next) = next {
            self.expiration_index.insert((next, idx));
        }
    }

    /// Reclaims every entry whose deadline has passed, earliest deadline first.
    ///
    /// Expiry is lazy and request-driven: the index is a passive ordered
    /// structure with no timer or background task behind it, so this runs only
    /// when a caller actually needs a slot. When the earliest deadline is still
    /// in the future the loop exits after a single lookup, which is what keeps
    /// insertion cost independent of cache capacity.
    fn prune_expired(&mut self, now: Instant) {
        while let Some(&(deadline, idx)) = self.expiration_index.first() {
            if deadline > now {
                break;
            }

            // remove_index() drops the matching record, which is what advances
            // the loop. A record that no longer describes its node would spin
            // here forever, so drop such a record directly instead.
            //
            // The else branch is unreachable while the index is in sync, and it
            // deliberately degrades gracefully rather than tripping: this runs
            // on a data-plane path under the shard mutex, where the project
            // requires a safe fallback over a panic. Dropping the orphan record
            // restores the invariant and costs at most one skipped reclamation
            // (the entry still expires lazily in get()). There is no plugin log
            // or tracing sink reachable from here to leave a trail in.
            let node_deadline = self.nodes[idx].expires_at.filter(|_| self.nodes[idx].occupied);
            if node_deadline == Some(deadline) {
                self.remove_index(idx);
            } else {
                self.expiration_index.remove(&(deadline, idx));
            }
        }
    }

    fn insert(&mut self, key: K, value: V, ttl: Option<Duration>) -> bool {
        let now = Instant::now();

        if let Some(&idx) = self.map.get(&key) {
            let previous_expires_at = self.nodes[idx].expires_at;
            let next_expires_at = ttl.and_then(|ttl| now.checked_add(ttl));
            self.nodes[idx].value = Some(value);
            self.nodes[idx].expires_at = next_expires_at;
            self.sync_expiration_record(idx, previous_expires_at, next_expires_at);
            self.touch(idx);
            return false; // key-update: no eviction
        }

        // Only prune when no free slot is available: alloc_index would
        // otherwise be forced to displace the LRU tail. When free_head is Some,
        // alloc_index returns immediately without touching the list, so the
        // pruning pass is pure overhead. This preserves the "prune before
        // forced eviction of a live entry" invariant exercised by
        // `test_eviction_count_zero_when_expired_entry_freed_before_displacement`.
        if self.free_head.is_none() {
            self.prune_expired(now);
        }

        let (idx, evicted) = self.alloc_index();
        let expires_at = ttl.and_then(|ttl| now.checked_add(ttl));
        self.nodes[idx].key = Some(key.clone());
        self.nodes[idx].value = Some(value);
        self.nodes[idx].expires_at = expires_at;
        self.nodes[idx].occupied = true;
        self.nodes[idx].next_free = None;
        if let Some(expires_at) = expires_at {
            // alloc_index() hands back a reset node, so there is no previous
            // record for this slot to drop.
            self.expiration_index.insert((expires_at, idx));
        }

        self.attach_head(idx);
        self.map.insert(key, idx);
        self.len += 1;

        evicted
    }

    fn get<Q>(&mut self, key: &Q) -> (Option<V>, CacheStatus)
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
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

    fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let idx = *self.map.get(key)?;
        self.remove_index(idx)
    }

    fn clear(&mut self) {
        self.map.clear();
        // clear() resets the node pool directly instead of going through
        // remove_index(), so the expiration index must be dropped explicitly.
        self.expiration_index.clear();
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

/// A process-local TTL/LRU cache with a fixed entry capacity and ordinary
/// in-process locking.
///
/// Keeps a fixed-capacity node pool plus a doubly-linked LRU list under one
/// mutex: O(1) touch/insert/remove. The node pool and the key map are allocated
/// once at construction; the expiration index is the one structure that grows
/// and shrinks at runtime, allocating a B-tree node roughly every 11 TTL entries.
///
/// `V` is fully caller-defined. If cloning values is expensive, callers can
/// store `Arc<T>` as the value type to keep cache hits cheap.
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

    /// Number of records currently held by the expiration index.
    ///
    /// Diagnostic accessor for tests and benchmarks; not part of the cache
    /// contract. The index holds exactly one record per live entry that carries
    /// a TTL, so the result never exceeds [`Self::capacity`] regardless of how
    /// often a key is updated.
    #[doc(hidden)]
    pub fn expiration_index_len(&self) -> usize {
        self.inner.lock().expiration_index.len()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().len
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clears all entries and resets all stats counters to zero.
    ///
    /// Called on config reload or full invalidation. Prior hit-rate statistics
    /// are not meaningful after the cache population changes completely.
    /// If you need to preserve historical stats, call [`Self::stats`] before `clear()`.
    pub fn clear(&self) {
        self.inner.lock().clear();
        // Reset counters after the lock is released — the inner clear has no
        // access to the outer struct's atomics.
        self.hits.store(0, Relaxed);
        self.misses.store(0, Relaxed);
        self.expirations.store(0, Relaxed);
        self.evictions.store(0, Relaxed);
    }

    pub fn remove<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
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

    /// Returns current cache keys in MRU order (most-recently-used first).
    ///
    /// Acquires the mutex and walks the full LRU list — O(n). All concurrent
    /// `get`, `insert`, `remove`, and `clear` calls will block for the entire
    /// duration of the walk. Do not call on large caches from latency-sensitive
    /// code paths. Intended for debugging and admin inspection, and for bounded
    /// cold-path sweeps (e.g. invalidating every entry belonging to a deleted
    /// owner) — never for a per-request lookup.
    ///
    /// Expired-but-not-yet-pruned entries are excluded from the result.
    /// As a result, `get_keys().len()` may be less than `len()` when lazy-expired
    /// entries are present. This is by design and consistent with the lazy expiry
    /// model.
    pub fn get_keys(&self) -> Vec<K> {
        let inner = self.inner.lock();
        let now = Instant::now();
        let mut keys = Vec::with_capacity(inner.len); // may over-allocate if expired entries are present
        let mut cursor = inner.head;
        while let Some(idx) = cursor {
            let node = &inner.nodes[idx];
            if node.occupied && !node.is_expired(now) {
                if let Some(ref k) = node.key {
                    keys.push(k.clone());
                }
            }
            cursor = node.next;
        }
        keys
    }

    pub fn get<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.get_with_status(key).0
    }

    pub fn get_with_status<Q>(&self, key: &Q) -> (Option<V>, CacheStatus)
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let result = self.inner.lock().get(key); // MutexGuard dropped here
        match result.1 {
            CacheStatus::Hit => {
                self.hits.fetch_add(1, Relaxed);
            }
            CacheStatus::Miss => {
                self.misses.fetch_add(1, Relaxed);
            }
            CacheStatus::Expired => {
                self.expirations.fetch_add(1, Relaxed);
            }
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
    fn test_lru_ttl_scenarios() {
        struct TtlScenario {
            /// Scenario label — kept for readability in the table below.
            #[allow(dead_code)]
            name: &'static str,
            run: fn(),
        }

        let scenarios: &[TtlScenario] = &[
            TtlScenario {
                name: "ttl_expiration",
                run: || {
                    // Basic TTL: an entry past its deadline must return Expired
                    // and the cache must report empty after the access drains it.
                    let cache = LocalLruCache::new(2);
                    cache.insert("a", 1, Some(Duration::from_millis(10)));
                    std::thread::sleep(Duration::from_millis(20));
                    assert_eq!(cache.get_with_status(&"a"), (None, CacheStatus::Expired));
                    assert!(cache.is_empty());
                },
            },
            TtlScenario {
                name: "zero_ttl_removes_value",
                run: || {
                    // Duration::ZERO means "already expired at insert time": the
                    // value must be unreachable immediately after the call returns.
                    let cache = LocalLruCache::new(2);
                    cache.insert("a", 1, None);
                    cache.insert("a", 2, Some(Duration::ZERO));
                    assert_eq!(cache.get(&"a"), None);
                    assert!(cache.is_empty());
                },
            },
            TtlScenario {
                name: "insert_prunes_expired_before_evicting_valid_entry",
                run: || {
                    // Invariant: MRU expired entry is pruned before any live entry
                    // is displaced by LRU eviction.
                    //
                    // "b" is touched after insert so it becomes MRU. After it
                    // expires the next insert("c") must prune "b" (not evict "a"),
                    // even though "b" would normally be the *last* to be evicted.
                    let cache = LocalLruCache::new(2);
                    cache.insert("a", 1, None);
                    cache.insert("b", 2, Some(Duration::from_millis(10)));
                    assert_eq!(cache.get(&"b"), Some(2)); // touch b -> b is MRU
                    std::thread::sleep(Duration::from_millis(20));
                    cache.insert("c", 3, None);
                    // "a" must survive; "b" pruned despite being MRU; "c" inserted.
                    assert_eq!(cache.get(&"a"), Some(1));
                    assert_eq!(cache.get_with_status(&"b"), (None, CacheStatus::Miss));
                    assert_eq!(cache.get(&"c"), Some(3));
                },
            },
            TtlScenario {
                name: "get_keys_excludes_expired_entries",
                run: || {
                    // get_keys() must filter out expired entries even before
                    // they are pruned from the node pool.
                    let cache = LocalLruCache::new(4);
                    cache.insert("a", 1, None);
                    cache.insert("b", 2, Some(Duration::from_millis(10)));
                    std::thread::sleep(Duration::from_millis(20));
                    // "b" is expired but not yet pruned — still counted by len()
                    let keys = cache.get_keys();
                    assert!(!keys.contains(&"b"), "expired entry must not appear in get_keys");
                    assert!(keys.contains(&"a"));
                    // Documented invariant: get_keys().len() may be < len()
                    assert!(cache.get_keys().len() <= cache.len());
                },
            },
        ];

        for s in scenarios {
            (s.run)();
        }
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
    fn test_string_key_supports_borrowed_str_lookup_and_removal() {
        let cache = LocalLruCache::new(2);

        cache.insert("key".to_string(), 1, None);

        assert_eq!(cache.get("key"), Some(1));
        assert_eq!(cache.remove("key"), Some(1));
        assert!(cache.is_empty());
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
    fn test_repeated_full_capacity_evictions_preserve_node_pool_and_list() {
        let cache = LocalLruCache::new(4);

        for key in 0..128 {
            cache.insert(key, key, None);
            assert!(cache.len() <= cache.capacity());
        }

        assert_eq!(cache.len(), 4);
        assert_eq!(cache.get_keys(), vec![127, 126, 125, 124]);
        for key in 0..124 {
            assert_eq!(cache.get(&key), None);
        }
        for key in 124..128 {
            assert_eq!(cache.get(&key), Some(key));
        }
        assert_eq!(cache.stats().evictions, 124);
    }

    #[test]
    fn test_eviction_count_zero_when_expired_entry_freed_before_displacement() {
        // If prune_expired frees a slot, no live entry is displaced.
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

    #[test]
    fn test_get_keys_mru_order() {
        let cache = LocalLruCache::new(4);

        cache.insert("a", 1, None);
        cache.insert("b", 2, None);
        cache.insert("c", 3, None);
        cache.get(&"a"); // touch "a" -> now MRU order is: a, c, b

        let keys = cache.get_keys();
        assert_eq!(keys, vec!["a", "c", "b"]);
    }

    #[test]
    fn test_get_keys_empty_cache() {
        let cache: LocalLruCache<&str, i32> = LocalLruCache::new(4);
        assert!(cache.get_keys().is_empty());
    }

    // ---------------------------------------------------------------
    // Expiration index: one record per live TTL entry, kept in sync by
    // every path that assigns, rewrites, or releases a deadline.
    // ---------------------------------------------------------------

    #[test]
    fn test_entries_without_ttl_never_enter_expiration_index() {
        let cache = LocalLruCache::new(4);

        cache.insert("a", 1, None);
        cache.insert("b", 2, None);

        assert_eq!(cache.len(), 2);
        assert_eq!(cache.expiration_index_len(), 0);
    }

    #[test]
    fn test_expiration_index_tracks_only_ttl_entries() {
        let cache = LocalLruCache::new(4);

        cache.insert("a", 1, None);
        cache.insert("b", 2, Some(Duration::from_secs(60)));
        cache.insert("c", 3, Some(Duration::from_secs(60)));

        assert_eq!(cache.len(), 3);
        assert_eq!(cache.expiration_index_len(), 2);
    }

    #[test]
    fn test_ttl_extension_replaces_the_previous_expiration_record() {
        let cache = LocalLruCache::new(4);

        cache.insert("a", 1, Some(Duration::from_millis(10)));
        // Extending the TTL must drop the old record rather than add a second
        // one, otherwise the stale deadline would expire a live entry.
        cache.insert("a", 2, Some(Duration::from_secs(60)));
        assert_eq!(cache.expiration_index_len(), 1);

        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(cache.get(&"a"), Some(2));
        assert_eq!(cache.expiration_index_len(), 1);
    }

    #[test]
    fn test_clearing_ttl_on_update_drops_the_expiration_record() {
        let cache = LocalLruCache::new(4);

        cache.insert("a", 1, Some(Duration::from_millis(10)));
        cache.insert("a", 2, None);
        assert_eq!(cache.expiration_index_len(), 0);

        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(cache.get(&"a"), Some(2));
    }

    #[test]
    fn test_repeated_updates_keep_expiration_index_bounded() {
        // Documented bound: at most one record per live TTL entry, so the index
        // never exceeds capacity no matter how often a key is rewritten.
        let cache = LocalLruCache::new(4);

        for round in 0..10_000 {
            cache.insert("a", round, Some(Duration::from_secs(60)));
            assert!(cache.expiration_index_len() <= cache.capacity());
        }

        assert_eq!(cache.len(), 1);
        assert_eq!(cache.expiration_index_len(), 1);
        assert_eq!(cache.get(&"a"), Some(9_999));
    }

    #[test]
    fn test_shortened_ttl_is_pruned_at_its_new_deadline() {
        // The rewrite path must index the entry under its NEW deadline. If the
        // index kept the old, later deadline, prune_expired would find nothing
        // to reclaim and would displace the live LRU tail instead.
        let cache = LocalLruCache::new(2);

        cache.insert("a", 1, Some(Duration::from_secs(60)));
        cache.insert("b", 2, None);
        cache.insert("a", 10, Some(Duration::from_millis(10)));
        std::thread::sleep(Duration::from_millis(20));

        cache.insert("c", 3, None);

        assert_eq!(cache.get_with_status(&"a"), (None, CacheStatus::Miss));
        assert_eq!(cache.get(&"b"), Some(2), "live entry must not be displaced");
        assert_eq!(cache.get(&"c"), Some(3));
        assert_eq!(cache.stats().evictions, 0, "the slot came from expiry, not eviction");
        assert_eq!(cache.expiration_index_len(), 0);
    }

    #[test]
    fn test_extended_ttl_survives_pruning_at_its_old_deadline() {
        // Mirror of the case above: after an extension, the old deadline must
        // leave the index entirely. A record left behind at the old deadline
        // would make prune_expired consider a live entry reclaimable.
        let cache = LocalLruCache::new(2);

        cache.insert("a", 1, Some(Duration::from_millis(10)));
        cache.insert("b", 2, None);
        cache.insert("a", 10, Some(Duration::from_secs(60)));
        std::thread::sleep(Duration::from_millis(20));

        // Full cache, nothing genuinely expired: "b" is the LRU tail because the
        // rewrite of "a" touched it to the head.
        cache.insert("c", 3, None);

        assert_eq!(
            cache.get(&"a"),
            Some(10),
            "extended entry must survive its old deadline"
        );
        assert_eq!(cache.get_with_status(&"b"), (None, CacheStatus::Miss));
        assert_eq!(cache.get(&"c"), Some(3));
        assert_eq!(cache.stats().evictions, 1);
        assert_eq!(
            cache.expiration_index_len(),
            1,
            "exactly one record, at the new deadline"
        );
    }

    #[test]
    fn test_explicit_removal_reclaims_the_expiration_record() {
        let cache = LocalLruCache::new(4);

        cache.insert("a", 1, Some(Duration::from_secs(60)));
        cache.insert("b", 2, Some(Duration::from_secs(60)));
        assert_eq!(cache.remove(&"a"), Some(1));

        assert_eq!(cache.expiration_index_len(), 1);
    }

    #[test]
    fn test_clear_drops_all_expiration_records() {
        // clear() resets the node pool without going through remove_index(), so
        // it is the one path that must drop the index explicitly.
        let cache = LocalLruCache::new(4);

        cache.insert("a", 1, Some(Duration::from_secs(60)));
        cache.insert("b", 2, Some(Duration::from_secs(60)));
        cache.clear();

        assert_eq!(cache.expiration_index_len(), 0);

        // A reused slot must not inherit the cleared entry's deadline.
        cache.insert("c", 3, None);
        assert_eq!(cache.expiration_index_len(), 0);
        assert_eq!(cache.get(&"c"), Some(3));
    }

    #[test]
    fn test_lru_eviction_reclaims_the_displaced_entry_record() {
        let cache = LocalLruCache::new(2);

        cache.insert("a", 1, Some(Duration::from_secs(60)));
        cache.insert("b", 2, Some(Duration::from_secs(60)));
        cache.insert("c", 3, Some(Duration::from_secs(60)));

        assert_eq!(cache.len(), 2);
        assert_eq!(cache.stats().evictions, 1);
        assert_eq!(cache.expiration_index_len(), 2);
    }

    #[test]
    fn test_all_expired_cache_reclaims_every_slot_without_eviction() {
        let cache = LocalLruCache::new(4);

        for key in 0..4 {
            cache.insert(key, key, Some(Duration::from_millis(10)));
        }
        assert_eq!(cache.expiration_index_len(), 4);
        std::thread::sleep(Duration::from_millis(20));

        // The cache is full but every entry is expired: the insert must reclaim
        // all four slots and displace no live entry.
        cache.insert(99, 99, None);

        assert_eq!(cache.len(), 1);
        assert_eq!(cache.stats().evictions, 0);
        assert_eq!(cache.expiration_index_len(), 0);
        assert_eq!(cache.get(&99), Some(99));
    }

    #[test]
    fn test_pruning_stops_at_the_first_live_deadline() {
        let cache = LocalLruCache::new(4);

        for key in 0..4 {
            cache.insert(key, key, Some(Duration::from_secs(60)));
        }
        // Nothing is expired, so the full cache must fall back to LRU eviction
        // and keep every remaining live record indexed.
        cache.insert(4, 4, Some(Duration::from_secs(60)));

        assert_eq!(cache.len(), 4);
        assert_eq!(cache.stats().evictions, 1);
        assert_eq!(cache.expiration_index_len(), 4);
        assert_eq!(cache.get(&0), None);
        for key in 1..5 {
            assert_eq!(cache.get(&key), Some(key));
        }
    }

    #[test]
    fn test_reused_slot_does_not_carry_over_a_stale_deadline() {
        // Slot indices are recycled through the free list, so a record left
        // behind by a previous tenant would expire the new occupant early.
        let cache = LocalLruCache::new(2);

        cache.insert("a", 1, Some(Duration::from_millis(10)));
        cache.insert("b", 2, None);
        std::thread::sleep(Duration::from_millis(20));

        // "a" is expired; "c" reclaims its slot.
        cache.insert("c", 3, None);
        assert_eq!(cache.expiration_index_len(), 0);

        // Well past "a"'s original deadline, the slot's new tenant must survive.
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(cache.get(&"c"), Some(3));
        assert_eq!(cache.get(&"b"), Some(2));
        assert_eq!(cache.stats().evictions, 0);
    }

    #[test]
    fn test_mixed_expiration_insert_reclaims_expired_before_evicting_live() {
        let cache = LocalLruCache::new(4);

        cache.insert("live-1", 1, None);
        cache.insert("short", 2, Some(Duration::from_millis(10)));
        cache.insert("live-2", 3, Some(Duration::from_secs(60)));
        cache.insert("live-3", 4, None);
        std::thread::sleep(Duration::from_millis(20));

        cache.insert("new", 5, None);

        assert_eq!(cache.stats().evictions, 0);
        assert_eq!(cache.get_with_status(&"short"), (None, CacheStatus::Miss));
        assert_eq!(cache.get(&"live-1"), Some(1));
        assert_eq!(cache.get(&"live-2"), Some(3));
        assert_eq!(cache.get(&"live-3"), Some(4));
        assert_eq!(cache.get(&"new"), Some(5));
        assert_eq!(cache.expiration_index_len(), 1);
    }

    #[test]
    fn test_clear_resets_stats_counters() {
        let cache = LocalLruCache::new(4);

        cache.insert("a", 1, None);
        cache.get(&"a"); // hit
        cache.get(&"b"); // miss

        // Sanity: stats are non-zero before clear
        let before = cache.stats();
        assert!(before.hits > 0 || before.misses > 0);

        cache.clear();

        let after = cache.stats();
        assert_eq!(after.hits, 0);
        assert_eq!(after.misses, 0);
        assert_eq!(after.expirations, 0);
        assert_eq!(after.evictions, 0);
    }
}
