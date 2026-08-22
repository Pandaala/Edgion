//! Shared LRU cache configuration for plugins that use `ShardedLruCache`.
//!
//! Embed this struct in any plugin config that needs an LRU cache.
//! Per-plugin values of `0` fall back to the global defaults in `LruCacheGlobalConfig`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const MAX_LRU_CACHE_CAPACITY: usize = 65_536;
pub const MAX_LRU_CACHE_SHARDS: usize = 64;

/// Reusable LRU cache configuration.
///
/// ## YAML example (inside a plugin config)
/// ```yaml
/// lruCache:
///   capacity: 2048
///   shards: 8
/// ```
///
/// When both fields are zero, the plugin inherits the global LRU defaults. A
/// positive capacity with zero shards selects one shard. Shards cannot be
/// overridden without also setting capacity.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct LruCacheConfig {
    /// LRU cache capacity (number of entries).
    /// Default: 0 (uses global `lru_cache.default_capacity`, typically 1024).
    #[serde(default)]
    pub capacity: usize,

    /// Number of cache shards to reduce lock contention under high concurrency.
    /// Default: 0 (one shard when capacity is explicit; otherwise inherit).
    #[serde(default)]
    pub shards: usize,
}

impl LruCacheConfig {
    pub fn validation_error(&self) -> Option<&'static str> {
        if self.capacity == 0 && self.shards > 0 {
            return Some("lruCache.capacity is required when shards is configured");
        }
        if self.capacity > MAX_LRU_CACHE_CAPACITY {
            return Some("lruCache.capacity exceeds the safety ceiling");
        }
        if self.shards > MAX_LRU_CACHE_SHARDS {
            return Some("lruCache.shards exceeds the safety ceiling");
        }
        if self.capacity > 0 && self.shards > self.capacity {
            return Some("lruCache.shards must not exceed capacity");
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_zero() {
        let cfg = LruCacheConfig::default();
        assert_eq!(cfg.capacity, 0);
        assert_eq!(cfg.shards, 0);
    }

    #[test]
    fn test_deserialize_empty_object() {
        let cfg: LruCacheConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.capacity, 0);
        assert_eq!(cfg.shards, 0);
    }

    #[test]
    fn test_deserialize_partial() {
        let cfg: LruCacheConfig = serde_json::from_str(r#"{"capacity": 512}"#).unwrap();
        assert_eq!(cfg.capacity, 512);
        assert_eq!(cfg.shards, 0);
    }

    #[test]
    fn test_deserialize_full() {
        let cfg: LruCacheConfig = serde_json::from_str(r#"{"capacity": 2048, "shards": 4}"#).unwrap();
        assert_eq!(cfg.capacity, 2048);
        assert_eq!(cfg.shards, 4);
    }

    #[test]
    fn validation_rejects_unsafe_limits() {
        assert_eq!(
            LruCacheConfig {
                capacity: MAX_LRU_CACHE_CAPACITY + 1,
                shards: 0,
            }
            .validation_error(),
            Some("lruCache.capacity exceeds the safety ceiling")
        );
        assert_eq!(
            LruCacheConfig { capacity: 4, shards: 5 }.validation_error(),
            Some("lruCache.shards must not exceed capacity")
        );
    }
}
