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
#[path = "../../tests/unit/cache/sharded_lru_test.rs"]
mod tests;
