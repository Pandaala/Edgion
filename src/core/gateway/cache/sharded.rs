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
/// - `shard_count > 1`: entries are spread across N shards, each with
///   `ceil(capacity / shard_count)` slots (minimum 1 per shard)
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
        let per_shard = (capacity + effective_shards - 1) / effective_shards; // ceil division
        let per_shard = per_shard.max(1);

        let shards = (0..effective_shards).map(|_| LocalLruCache::new(per_shard)).collect();

        Self { shards }
    }

    #[inline]
    fn shard_index(&self, key: &K) -> usize {
        if self.shards.len() == 1 {
            return 0;
        }
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        hasher.finish() as usize % self.shards.len()
    }

    pub fn get(&self, key: &K) -> Option<V> {
        self.shards[self.shard_index(key)].get(key)
    }

    pub fn insert(&self, key: K, value: V, ttl: Option<std::time::Duration>) {
        self.shards[self.shard_index(&key)].insert(key, value, ttl);
    }

    pub fn remove(&self, key: &K) -> Option<V> {
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

    /// Aggregated stats across all shards.
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
    /// Intended for debugging only.
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
    fn test_capacity_ceil_division() {
        // 10 entries across 3 shards => 4+4+4 = 12 total capacity (ceil)
        let cache: ShardedLruCache<String, i32> = ShardedLruCache::new(10, 3);
        assert_eq!(cache.shard_count(), 3);
        assert!(cache.capacity() >= 10);
    }

    #[test]
    fn test_small_capacity_with_many_shards() {
        // 2 entries across 8 shards => each shard gets 1 (minimum)
        let cache: ShardedLruCache<String, i32> = ShardedLruCache::new(2, 8);
        assert_eq!(cache.shard_count(), 8);
        // Each shard has at least 1 slot
        assert!(cache.capacity() >= 8);
    }
}
