use std::borrow::Borrow;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use super::lru::{CacheStats, LocalLruCache};

/// A sharded wrapper around [`LocalLruCache`] that reduces lock contention
/// by partitioning entries across multiple independent LRU shards.
///
/// When `shard_count == 1`, this behaves identically to a single `LocalLruCache`
/// (the default, zero-overhead path). When `shard_count > 1`, each key is
/// deterministically routed to one shard via its hash, so concurrent operations
/// on different shards never contend on the same lock.
///
/// ## Configuration
///
/// - `shard_count = 0` or `1`: sharding disabled (single shard, same as `LocalLruCache`)
/// - `shard_count > 1`: entries are spread across N shards using
///   quotient/remainder capacities whose sum is exactly `capacity`
#[derive(Debug)]
pub struct ShardedLruCache<K, V> {
    shards: Vec<LocalLruCache<K, V>>,
}

impl<K, V> ShardedLruCache<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    /// Create a new `ShardedLruCache`.
    ///
    /// - `capacity`: total number of entries across all shards (must be > 0)
    /// - `shard_count`: number of shards. `0` or `1` means no sharding.
    pub fn new(capacity: usize, shard_count: usize) -> Self {
        assert!(capacity > 0, "ShardedLruCache capacity must be greater than 0");

        let effective_shards = shard_count.max(1);
        assert!(
            effective_shards <= capacity,
            "ShardedLruCache shard count must not exceed capacity"
        );
        let base_capacity = capacity / effective_shards;
        let remainder = capacity % effective_shards;

        let shards = (0..effective_shards)
            .map(|index| LocalLruCache::new(base_capacity + usize::from(index < remainder)))
            .collect();

        Self { shards }
    }

    #[inline]
    fn shard_index<Q>(&self, key: &Q) -> usize
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        if self.shards.len() == 1 {
            return 0;
        }
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        hasher.finish() as usize % self.shards.len()
    }

    pub fn get<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.shards[self.shard_index(key)].get(key)
    }

    pub fn insert(&self, key: K, value: V, ttl: Option<std::time::Duration>) {
        self.shards[self.shard_index(&key)].insert(key, value, ttl);
    }

    pub fn remove<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.shards[self.shard_index(key)].remove(key)
    }

    pub fn clear(&self) {
        for shard in &self.shards {
            shard.clear();
        }
    }

    pub fn len(&self) -> usize {
        self.shards.iter().map(|s| s.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.shards.iter().all(|s| s.is_empty())
    }

    pub fn capacity(&self) -> usize {
        self.shards.iter().map(|s| s.capacity()).sum()
    }

    /// Returns the number of shards.
    pub fn shard_count(&self) -> usize {
        self.shards.len()
    }

    /// Total number of expiration-index records across all shards.
    ///
    /// Diagnostic accessor for tests and benchmarks; see
    /// [`LocalLruCache::expiration_index_len`]. Torn snapshot, as with `stats`.
    #[doc(hidden)]
    pub fn expiration_index_len(&self) -> usize {
        self.shards.iter().map(|s| s.expiration_index_len()).sum()
    }

    /// Aggregated stats across all shards.
    ///
    /// Like every other aggregate here (`len`, `is_empty`, `capacity`,
    /// `get_keys`, `expiration_index_len`), this locks shards one at a time, so
    /// the result is a torn snapshot under concurrent writes rather than a
    /// consistent point-in-time view.
    pub fn stats(&self) -> CacheStats {
        let mut total = CacheStats::default();
        for shard in &self.shards {
            let s = shard.stats();
            total.hits += s.hits;
            total.misses += s.misses;
            total.expirations += s.expirations;
            total.evictions += s.evictions;
        }
        total
    }

    /// Returns all live keys in MRU order per shard, concatenated.
    ///
    /// Allocates a snapshot of the whole live key set, so it is for diagnostics
    /// and for bounded cold-path sweeps (e.g. invalidating every entry belonging
    /// to a deleted owner) — never for a per-request lookup.
    pub fn get_keys(&self) -> Vec<K> {
        let mut keys = Vec::new();
        for shard in &self.shards {
            keys.extend(shard.get_keys());
        }
        keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn test_single_shard_behaves_like_local_lru() {
        let cache = ShardedLruCache::new(4, 1);
        cache.insert("a", 1, None);
        cache.insert("b", 2, Some(Duration::from_secs(60)));
        assert_eq!(cache.get(&"a"), Some(1));
        assert_eq!(cache.get(&"b"), Some(2));
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.shard_count(), 1);
    }

    #[test]
    fn test_zero_shards_treated_as_one() {
        let cache = ShardedLruCache::new(4, 0);
        assert_eq!(cache.shard_count(), 1);
        cache.insert("x", 10, None);
        assert_eq!(cache.get(&"x"), Some(10));
    }

    #[test]
    fn test_multi_shard_insert_get() {
        let cache = ShardedLruCache::new(100, 4);
        assert_eq!(cache.shard_count(), 4);

        for i in 0..50 {
            cache.insert(format!("key-{}", i), i, None);
        }
        assert_eq!(cache.len(), 50);

        for i in 0..50 {
            assert_eq!(cache.get(&format!("key-{}", i)), Some(i));
        }
    }

    #[test]
    fn test_multi_shard_remove_and_clear() {
        let cache = ShardedLruCache::new(100, 4);
        cache.insert("a".to_string(), 1, None);
        cache.insert("b".to_string(), 2, None);

        assert_eq!(cache.remove(&"a".to_string()), Some(1));
        assert_eq!(cache.get(&"a".to_string()), None);
        assert_eq!(cache.len(), 1);

        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn test_string_key_supports_borrowed_str_lookup_and_removal() {
        let cache = ShardedLruCache::new(8, 2);

        cache.insert("key".to_string(), 1, None);

        assert_eq!(cache.get("key"), Some(1));
        assert_eq!(cache.remove("key"), Some(1));
        assert!(cache.is_empty());
    }

    #[test]
    fn test_stats_aggregated_across_shards() {
        let cache = ShardedLruCache::new(100, 4);
        cache.insert("a".to_string(), 1, None);
        cache.get(&"a".to_string()); // hit
        cache.get(&"missing".to_string()); // miss

        let s = cache.stats();
        assert_eq!(s.hits, 1);
        assert_eq!(s.misses, 1);
    }

    #[test]
    fn test_ttl_expiration_with_shards() {
        let cache = ShardedLruCache::new(10, 2);
        cache.insert("a", 1, Some(Duration::from_millis(10)));
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(cache.get(&"a"), None);
    }

    #[test]
    fn test_capacity_is_exact_across_uneven_shards() {
        let cache: ShardedLruCache<String, i32> = ShardedLruCache::new(10, 3);
        assert_eq!(cache.shard_count(), 3);
        assert_eq!(cache.capacity(), 10);
    }

    #[test]
    #[should_panic(expected = "shard count must not exceed capacity")]
    fn test_shards_cannot_exceed_capacity() {
        let _: ShardedLruCache<String, i32> = ShardedLruCache::new(2, 8);
    }

    #[test]
    fn test_expiration_index_aggregates_across_shards() {
        let cache = ShardedLruCache::new(64, 8);

        for i in 0..32 {
            cache.insert(format!("ttl-{i}"), i, Some(Duration::from_secs(60)));
        }
        for i in 0..16 {
            cache.insert(format!("no-ttl-{i}"), i, None);
        }

        assert_eq!(cache.expiration_index_len(), 32);
        assert!(cache.expiration_index_len() <= cache.capacity());
    }

    #[test]
    fn test_concurrent_shard_access_keeps_expiration_index_bounded() {
        // Each shard owns its own node pool, expiration index, and mutex, so
        // concurrent traffic must never let the two structures diverge.
        let cache = Arc::new(ShardedLruCache::new(256, 8));
        let mut handles = Vec::new();

        for worker in 0..8 {
            let cache = Arc::clone(&cache);
            handles.push(std::thread::spawn(move || {
                for round in 0..2_000 {
                    let key = format!("key-{}", (worker * 97 + round) % 512);
                    match round % 4 {
                        0 => cache.insert(key, round, None),
                        1 => cache.insert(key, round, Some(Duration::from_secs(60))),
                        2 => cache.insert(key, round, Some(Duration::from_millis(1))),
                        _ => {
                            cache.remove(&key);
                        }
                    }
                    assert!(cache.expiration_index_len() <= cache.capacity());
                }
            }));
        }

        for handle in handles {
            handle.join().expect("worker thread must not panic");
        }

        assert!(cache.len() <= cache.capacity());
        assert!(cache.expiration_index_len() <= cache.len());
    }
}
