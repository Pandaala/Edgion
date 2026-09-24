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
#[path = "../../tests/unit/cache/tiny_ufo_test.rs"]
mod tests;
