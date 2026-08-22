//! Constrained TTL cache backed by the unchanged Pingora TinyUFO.
//!
//! [`TinyUfoTtlCache`] is intentionally not a general in-memory store. It has
//! no per-key removal operation and is suitable only for best-effort,
//! many-key lookup results whose misses can be recomputed authoritatively.
//! TinyUFO records usage before the wrapper can inspect TTL, so repeated reads
//! of an expired record may temporarily protect that dead record from eviction.
//! This can reduce hit ratio but never bypasses TTL or the memory bounds.

use edgion_resources::resources::edgion_plugins::{
    MAX_TINY_UFO_CACHE_ENTRIES, MAX_TINY_UFO_LOGICAL_BYTES, TINY_UFO_WEIGHT_UNIT_BYTES,
};
use parking_lot::Mutex;
use std::hash::Hash;
use std::mem::size_of;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tinyufo::TinyUfo;

/// Number of logical bytes represented by one TinyUFO weight unit.
pub const WEIGHT_UNIT_BYTES: usize = TINY_UFO_WEIGHT_UNIT_BYTES;
/// Absolute entry ceiling applied before TinyUFO allocates its map and sketch.
pub const MAX_CACHE_ENTRIES: usize = MAX_TINY_UFO_CACHE_ENTRIES;
/// Absolute TinyUFO weight ceiling, equal to 256 MiB of logical units.
pub const MAX_TOTAL_WEIGHT_UNITS: usize = MAX_TINY_UFO_LOGICAL_BYTES / WEIGHT_UNIT_BYTES;

const RECORD_ALLOCATION_ALLOWANCE: usize = 64;
const JSON_MAP_NODE_ALLOWANCE: usize = size_of::<usize>() * 4;

/// Validated limits for one TinyUFO TTL cache instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TinyUfoCacheLimits {
    pub max_entries: NonZeroUsize,
    pub max_logical_bytes: NonZeroUsize,
}

impl TinyUfoCacheLimits {
    pub fn try_new(max_entries: usize, max_logical_bytes: usize) -> Result<Self, TinyUfoCacheConfigError> {
        let max_entries = NonZeroUsize::new(max_entries).ok_or(TinyUfoCacheConfigError::ZeroEntries)?;
        let max_logical_bytes =
            NonZeroUsize::new(max_logical_bytes).ok_or(TinyUfoCacheConfigError::ZeroLogicalBytes)?;
        Ok(Self {
            max_entries,
            max_logical_bytes,
        })
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TinyUfoCacheConfigError {
    #[error("TinyUFO TTL cache max entries must be greater than zero")]
    ZeroEntries,
    #[error("TinyUFO TTL cache max logical bytes must be greater than zero")]
    ZeroLogicalBytes,
    #[error("TinyUFO TTL cache max entries {configured} exceeds the safety ceiling {maximum}; reduce maxEntries")]
    TooManyEntries { configured: usize, maximum: usize },
    #[error(
        "TinyUFO TTL cache logical budget {configured} bytes is smaller than one {unit}-byte weight unit; increase maxLogicalBytes"
    )]
    LogicalLimitBelowWeightUnit { configured: usize, unit: usize },
    #[error(
        "TinyUFO TTL cache logical weight {configured} units exceeds the safety ceiling {maximum}; reduce maxLogicalBytes"
    )]
    TooManyWeightUnits { configured: usize, maximum: usize },
    #[error(
        "TinyUFO TTL cache minimum entry weight {configured} units exceeds TinyUFO's per-entry limit {maximum}; adjust the cache limits"
    )]
    EntryWeightTooLarge { configured: usize, maximum: usize },
    #[error("TinyUFO TTL cache limit arithmetic overflowed; reduce the configured limits")]
    ArithmeticOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TinyUfoInsertOutcome {
    Stored,
    /// TinyLFU admission or immediate eviction did not retain the record.
    NotRetained,
    RejectedOversized,
    RejectedInvalidTtl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TinyUfoLookupStatus {
    Hit,
    Miss,
    Expired,
    KeyCollision,
}

pub(crate) mod sealed {
    pub trait Sealed {}
}

/// Reports dynamically owned logical payload bytes.
///
/// The cache adds inline key/value sizes and a conservative record allowance.
/// This is a logical retention weight, not an allocator or RSS measurement.
pub trait LogicalWeight: sealed::Sealed {
    fn logical_weight(&self) -> usize;
}

impl sealed::Sealed for String {}
impl LogicalWeight for String {
    fn logical_weight(&self) -> usize {
        self.capacity()
    }
}

impl sealed::Sealed for serde_json::Value {}
impl LogicalWeight for serde_json::Value {
    fn logical_weight(&self) -> usize {
        json_dynamic_weight(self)
    }
}

fn json_dynamic_weight(value: &serde_json::Value) -> usize {
    use serde_json::Value;

    let mut total = 0usize;
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
            Value::String(value) => total = total.saturating_add(value.capacity()),
            Value::Array(values) => {
                total = total.saturating_add(values.capacity().saturating_mul(size_of::<Value>()));
                pending.extend(values);
            }
            Value::Object(values) => {
                for (key, value) in values {
                    total = total
                        .saturating_add(size_of::<String>())
                        .saturating_add(size_of::<Value>())
                        .saturating_add(JSON_MAP_NODE_ALLOWANCE)
                        .saturating_add(key.capacity());
                    pending.push(value);
                }
            }
        }
    }
    total
}

struct CacheRecord<K, V> {
    logical_key: K,
    value: V,
    expires_at: Instant,
}

/// Best-effort required-TTL cache backed by TinyLFU admission and S3-FIFO
/// eviction from the pinned, unchanged Pingora TinyUFO implementation.
///
/// Reads are lock-free at this layer. Inserts serialize because the pinned
/// TinyUFO has an unresolved concurrent same-key admission accounting case.
/// There is deliberately no per-key removal API.
pub struct TinyUfoTtlCache<K, V> {
    cache: TinyUfo<K, Arc<CacheRecord<K, V>>>,
    write_lock: Mutex<()>,
    limits: TinyUfoCacheLimits,
    total_weight_units: usize,
    min_entry_weight_units: usize,
}

pub fn validate_limits(limits: TinyUfoCacheLimits) -> Result<(), TinyUfoCacheConfigError> {
    validated_weight_limits(limits).map(|_| ())
}

fn validated_weight_limits(limits: TinyUfoCacheLimits) -> Result<(usize, usize), TinyUfoCacheConfigError> {
    let max_entries = limits.max_entries.get();
    if max_entries > MAX_CACHE_ENTRIES {
        return Err(TinyUfoCacheConfigError::TooManyEntries {
            configured: max_entries,
            maximum: MAX_CACHE_ENTRIES,
        });
    }
    max_entries
        .checked_mul(8)
        .ok_or(TinyUfoCacheConfigError::ArithmeticOverflow)?;

    let max_logical_bytes = limits.max_logical_bytes.get();
    let total_weight_units = max_logical_bytes / WEIGHT_UNIT_BYTES;
    if total_weight_units == 0 {
        return Err(TinyUfoCacheConfigError::LogicalLimitBelowWeightUnit {
            configured: max_logical_bytes,
            unit: WEIGHT_UNIT_BYTES,
        });
    }
    if total_weight_units > MAX_TOTAL_WEIGHT_UNITS {
        return Err(TinyUfoCacheConfigError::TooManyWeightUnits {
            configured: total_weight_units,
            maximum: MAX_TOTAL_WEIGHT_UNITS,
        });
    }

    let min_entry_weight_units = total_weight_units
        .checked_add(max_entries - 1)
        .ok_or(TinyUfoCacheConfigError::ArithmeticOverflow)?
        / max_entries;
    if min_entry_weight_units > u16::MAX as usize {
        return Err(TinyUfoCacheConfigError::EntryWeightTooLarge {
            configured: min_entry_weight_units,
            maximum: u16::MAX as usize,
        });
    }
    Ok((total_weight_units, min_entry_weight_units))
}

impl<K, V> TinyUfoTtlCache<K, V>
where
    K: Clone + Eq + Hash + LogicalWeight + Send + Sync + 'static,
    V: Clone + LogicalWeight + Send + Sync + 'static,
{
    pub fn validate_limits(limits: TinyUfoCacheLimits) -> Result<(), TinyUfoCacheConfigError> {
        validate_limits(limits)
    }

    pub fn new(limits: TinyUfoCacheLimits) -> Result<Self, TinyUfoCacheConfigError> {
        let (total_weight_units, min_entry_weight_units) = validated_weight_limits(limits)?;
        Ok(Self {
            cache: TinyUfo::new(total_weight_units, limits.max_entries.get()),
            write_lock: Mutex::new(()),
            limits,
            total_weight_units,
            min_entry_weight_units,
        })
    }

    pub fn limits(&self) -> TinyUfoCacheLimits {
        self.limits
    }

    pub fn get(&self, key: &K) -> (Option<V>, TinyUfoLookupStatus) {
        self.lookup_at(key, Instant::now())
    }

    pub fn insert(&self, key: K, value: V, ttl: Duration) -> TinyUfoInsertOutcome {
        self.insert_at(key, value, ttl, Instant::now())
    }

    fn insert_at(&self, key: K, value: V, ttl: Duration, now: Instant) -> TinyUfoInsertOutcome {
        if ttl.is_zero() {
            return TinyUfoInsertOutcome::RejectedInvalidTtl;
        }
        let Some(expires_at) = now.checked_add(ttl) else {
            return TinyUfoInsertOutcome::RejectedInvalidTtl;
        };
        let logical_bytes = record_logical_bytes(&key, &value);
        let Some(weight) = self.record_weight(logical_bytes) else {
            return TinyUfoInsertOutcome::RejectedOversized;
        };
        let record = Arc::new(CacheRecord {
            logical_key: key,
            value,
            expires_at,
        });

        let _guard = self.write_lock.lock();
        let evicted = self.cache.put(record.logical_key.clone(), Arc::clone(&record), weight);
        if evicted.iter().any(|entry| Arc::ptr_eq(&entry.data, &record)) {
            TinyUfoInsertOutcome::NotRetained
        } else {
            TinyUfoInsertOutcome::Stored
        }
    }

    fn lookup_at(&self, key: &K, now: Instant) -> (Option<V>, TinyUfoLookupStatus) {
        let Some(record) = self.cache.get(key) else {
            return (None, TinyUfoLookupStatus::Miss);
        };
        if record.logical_key != *key {
            return (None, TinyUfoLookupStatus::KeyCollision);
        }
        if now >= record.expires_at {
            return (None, TinyUfoLookupStatus::Expired);
        }
        (Some(record.value.clone()), TinyUfoLookupStatus::Hit)
    }

    fn record_weight(&self, logical_bytes: usize) -> Option<u16> {
        if logical_bytes > self.limits.max_logical_bytes.get() {
            return None;
        }
        let rounded_units = logical_bytes.checked_add(WEIGHT_UNIT_BYTES - 1)? / WEIGHT_UNIT_BYTES;
        let units = rounded_units.max(self.min_entry_weight_units);
        if units > self.total_weight_units || units > u16::MAX as usize {
            return None;
        }
        u16::try_from(units).ok()
    }
}

fn record_logical_bytes<K, V>(key: &K, value: &V) -> usize
where
    K: LogicalWeight,
    V: LogicalWeight,
{
    size_of::<CacheRecord<K, V>>()
        .saturating_add(key.logical_weight())
        .saturating_add(size_of::<V>())
        .saturating_add(value.logical_weight())
        .saturating_add(RECORD_ALLOCATION_ALLOWANCE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::lru::LocalLruCache;
    use std::hash::{Hash, Hasher};
    use std::hint::black_box;
    use std::thread;

    fn limits(entries: usize, bytes: usize) -> TinyUfoCacheLimits {
        TinyUfoCacheLimits::try_new(entries, bytes).expect("test limits are valid")
    }

    #[test]
    fn constructor_rejects_invalid_limits_before_allocation() {
        assert_eq!(
            TinyUfoCacheLimits::try_new(0, 64),
            Err(TinyUfoCacheConfigError::ZeroEntries)
        );
        assert!(TinyUfoTtlCache::<String, String>::new(limits(MAX_CACHE_ENTRIES + 1, 64)).is_err());
        assert!(TinyUfoTtlCache::<String, String>::new(limits(1, WEIGHT_UNIT_BYTES - 1)).is_err());
    }

    #[test]
    fn required_ttl_hit_and_exact_expiry_boundary() {
        let cache = TinyUfoTtlCache::new(limits(4, 4096)).expect("cache must construct");
        let now = Instant::now();
        assert_eq!(
            cache.insert_at("key".to_string(), "value".to_string(), Duration::from_secs(5), now),
            TinyUfoInsertOutcome::Stored
        );
        assert_eq!(
            cache.lookup_at(&"key".to_string(), now + Duration::from_secs(4)),
            (Some("value".to_string()), TinyUfoLookupStatus::Hit)
        );
        assert_eq!(
            cache.lookup_at(&"key".to_string(), now + Duration::from_secs(5)),
            (None, TinyUfoLookupStatus::Expired)
        );
    }

    #[test]
    fn repeated_expired_reads_never_resurrect_a_record() {
        let cache = TinyUfoTtlCache::new(limits(4, 4096)).expect("cache must construct");
        let now = Instant::now();
        cache.insert_at("key".to_string(), "value".to_string(), Duration::from_secs(1), now);
        for _ in 0..100 {
            assert_eq!(
                cache.lookup_at(&"key".to_string(), now + Duration::from_secs(1)),
                (None, TinyUfoLookupStatus::Expired)
            );
        }
    }

    #[test]
    fn zero_ttl_does_not_replace_existing_value() {
        let cache = TinyUfoTtlCache::new(limits(4, 4096)).expect("cache must construct");
        cache.insert("key".to_string(), "old".to_string(), Duration::from_secs(60));
        assert_eq!(
            cache.insert("key".to_string(), "new".to_string(), Duration::ZERO),
            TinyUfoInsertOutcome::RejectedInvalidTtl
        );
        assert_eq!(cache.get(&"key".to_string()).0, Some("old".to_string()));
    }

    #[test]
    fn same_key_replacement_needs_no_remove() {
        let cache = TinyUfoTtlCache::new(limits(4, 4096)).expect("cache must construct");
        cache.insert("key".to_string(), "old".to_string(), Duration::from_secs(60));
        cache.insert("key".to_string(), "new".to_string(), Duration::from_secs(60));
        assert_eq!(cache.get(&"key".to_string()).0, Some("new".to_string()));
    }

    #[test]
    fn tiny_lfu_rejection_reports_not_retained_without_a_followup_get() {
        // A larger estimator makes random Count-Min hash collisions negligible,
        // while the three-unit weight budget still retains only one record.
        let cache = TinyUfoTtlCache::new(limits(64, 3 * WEIGHT_UNIT_BYTES)).expect("cache must construct");
        let hot_key = "hot".to_string();
        for value in 0..16 {
            assert_eq!(
                cache.insert(hot_key.clone(), value.to_string(), Duration::from_secs(60)),
                TinyUfoInsertOutcome::Stored
            );
        }

        let cold_key = "cold".to_string();
        assert_eq!(
            cache.insert(cold_key.clone(), "cold-value".to_string(), Duration::from_secs(60)),
            TinyUfoInsertOutcome::NotRetained
        );
        assert_eq!(cache.get(&cold_key).0, None);
        assert_eq!(cache.get(&hot_key).0, Some("15".to_string()));
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct CollidingKey(&'static str);

    impl Hash for CollidingKey {
        fn hash<H: Hasher>(&self, state: &mut H) {
            7_u8.hash(state);
        }
    }

    impl sealed::Sealed for CollidingKey {}
    impl LogicalWeight for CollidingKey {
        fn logical_weight(&self) -> usize {
            self.0.len()
        }
    }

    #[test]
    fn hash_collision_never_returns_the_other_value() {
        let cache = TinyUfoTtlCache::new(limits(4, 4096)).expect("cache must construct");
        cache.insert(CollidingKey("a"), "a-value".to_string(), Duration::from_secs(60));
        assert_eq!(cache.get(&CollidingKey("b")), (None, TinyUfoLookupStatus::KeyCollision));
        cache.insert(CollidingKey("b"), "b-value".to_string(), Duration::from_secs(60));
        assert_ne!(cache.get(&CollidingKey("a")).0, Some("b-value".to_string()));
    }

    #[test]
    fn oversized_record_is_not_retained() {
        let cache = TinyUfoTtlCache::new(limits(4, 512)).expect("cache must construct");
        assert_eq!(
            cache.insert("key".to_string(), "x".repeat(1024), Duration::from_secs(60)),
            TinyUfoInsertOutcome::RejectedOversized
        );
    }

    #[test]
    fn concurrent_reads_and_serialized_writes_remain_safe() {
        let cache = Arc::new(TinyUfoTtlCache::new(limits(64, 64 * 1024)).expect("cache must construct"));
        let mut handles = Vec::new();
        for thread_id in 0..8 {
            let cache = Arc::clone(&cache);
            handles.push(thread::spawn(move || {
                for round in 0..2_000 {
                    let key = format!("key-{}", round % 64);
                    if thread_id % 2 == 0 {
                        cache.insert(key, round.to_string(), Duration::from_secs(60));
                    } else {
                        let _ = cache.get(&key);
                    }
                }
            }));
        }
        for handle in handles {
            handle.join().expect("cache worker must not panic");
        }
    }

    fn benchmark_elapsed(label: &str, operations: usize, operation: impl Fn(usize)) {
        let started = Instant::now();
        for index in 0..operations {
            operation(black_box(index));
        }
        let elapsed = started.elapsed();
        eprintln!(
            "{label}: {operations} operations in {elapsed:?} ({:.1} ns/op)",
            elapsed.as_nanos() as f64 / operations as f64
        );
    }

    fn benchmark_concurrent(
        label: &str,
        threads: usize,
        operations_per_thread: usize,
        operation: &(impl Fn(usize, usize) + Sync),
    ) {
        let started = Instant::now();
        thread::scope(|scope| {
            for thread_index in 0..threads {
                scope.spawn(move || {
                    for index in 0..operations_per_thread {
                        operation(thread_index, black_box(index));
                    }
                });
            }
        });
        let operations = threads * operations_per_thread;
        let elapsed = started.elapsed();
        eprintln!(
            "{label}/{threads}t: {operations} operations in {elapsed:?} ({:.1} ns/op)",
            elapsed.as_nanos() as f64 / operations as f64
        );
    }

    /// Manual release-mode comparison using equivalent owned-`String` values.
    ///
    /// Run with:
    /// `cargo test -p edgion-gateway cache::tiny_ufo::tests::benchmark_three_cache_variants --release -- --ignored --nocapture`
    #[test]
    #[ignore = "manual performance benchmark"]
    fn benchmark_three_cache_variants() {
        const ENTRIES: usize = 4096;
        const OPERATIONS: usize = 1_000_000;
        const RAW_WEIGHT_UNITS: u16 = 8;
        const VALUE_BYTES: usize = 128;
        let ttl = Duration::from_secs(3600);
        let keys: Vec<String> = (0..ENTRIES).map(|index| format!("key-{index:08}")).collect();
        let values: Vec<String> = (0..ENTRIES)
            .map(|index| format!("{index:08}-{}", "v".repeat(VALUE_BYTES)))
            .collect();

        let lru = LocalLruCache::new(ENTRIES);
        let raw = TinyUfo::new(RAW_WEIGHT_UNITS as usize * ENTRIES, ENTRIES);
        let adapter = TinyUfoTtlCache::new(limits(ENTRIES, RAW_WEIGHT_UNITS as usize * WEIGHT_UNIT_BYTES * ENTRIES))
            .expect("benchmark adapter must construct");
        for index in 0..ENTRIES {
            lru.insert(keys[index].clone(), values[index].clone(), Some(ttl));
            raw.put(keys[index].clone(), values[index].clone(), RAW_WEIGHT_UNITS);
            assert_eq!(
                adapter.insert(keys[index].clone(), values[index].clone(), ttl),
                TinyUfoInsertOutcome::Stored
            );
        }

        benchmark_elapsed("lru/live-hit", OPERATIONS, |index| {
            black_box(lru.get(&keys[index % ENTRIES]));
        });
        benchmark_elapsed("raw-tinyufo/live-hit", OPERATIONS, |index| {
            black_box(raw.get(&keys[index % ENTRIES]));
        });
        benchmark_elapsed("adapter/live-hit-ttl", OPERATIONS, |index| {
            black_box(adapter.get(&keys[index % ENTRIES]));
        });

        let missing = "missing-key".to_string();
        benchmark_elapsed("lru/miss", OPERATIONS, |_| {
            black_box(lru.get(&missing));
        });
        benchmark_elapsed("raw-tinyufo/miss", OPERATIONS, |_| {
            black_box(raw.get(&missing));
        });
        benchmark_elapsed("adapter/miss", OPERATIONS, |_| {
            black_box(adapter.get(&missing));
        });

        let expired_adapter =
            TinyUfoTtlCache::new(limits(ENTRIES, RAW_WEIGHT_UNITS as usize * WEIGHT_UNIT_BYTES * ENTRIES))
                .expect("expired benchmark adapter must construct");
        for index in 0..ENTRIES {
            expired_adapter.insert(keys[index].clone(), values[index].clone(), Duration::from_nanos(1));
        }
        thread::sleep(Duration::from_millis(1));
        benchmark_elapsed("adapter/expired-hit-storm", OPERATIONS, |index| {
            black_box(expired_adapter.get(&keys[index % ENTRIES]));
        });

        benchmark_elapsed("lru/replace", OPERATIONS / 10, |index| {
            let slot = index % ENTRIES;
            lru.insert(keys[slot].clone(), values[slot].clone(), Some(ttl));
        });
        benchmark_elapsed("raw-tinyufo/replace", OPERATIONS / 10, |index| {
            let slot = index % ENTRIES;
            raw.put(keys[slot].clone(), values[slot].clone(), RAW_WEIGHT_UNITS);
        });
        benchmark_elapsed("adapter/replace-ttl", OPERATIONS / 10, |index| {
            let slot = index % ENTRIES;
            black_box(adapter.insert(keys[slot].clone(), values[slot].clone(), ttl));
        });

        for threads in [1, 2, 4, 8] {
            let per_thread = OPERATIONS / threads;
            benchmark_concurrent("lru/95-read-5-write", threads, per_thread, &|thread_index, index| {
                let slot = (thread_index * per_thread + index) % ENTRIES;
                if index % 20 == 0 {
                    lru.insert(keys[slot].clone(), values[slot].clone(), Some(ttl));
                } else {
                    black_box(lru.get(&keys[slot]));
                }
            });
            benchmark_concurrent(
                "raw-tinyufo/95-read-5-write",
                threads,
                per_thread,
                &|thread_index, index| {
                    let slot = (thread_index * per_thread + index) % ENTRIES;
                    if index % 20 == 0 {
                        raw.put(keys[slot].clone(), values[slot].clone(), RAW_WEIGHT_UNITS);
                    } else {
                        black_box(raw.get(&keys[slot]));
                    }
                },
            );
            benchmark_concurrent(
                "adapter/95-read-5-write-ttl",
                threads,
                per_thread,
                &|thread_index, index| {
                    let slot = (thread_index * per_thread + index) % ENTRIES;
                    if index % 20 == 0 {
                        black_box(adapter.insert(keys[slot].clone(), values[slot].clone(), ttl));
                    } else {
                        black_box(adapter.get(&keys[slot]));
                    }
                },
            );
        }

        assert!(lru.len() <= ENTRIES);
        assert!(adapter.get(&keys[0]).0.is_some());
        assert!(raw.get(&keys[0]).is_some());
    }
}
