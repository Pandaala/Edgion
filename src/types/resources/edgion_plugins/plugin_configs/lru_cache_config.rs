//! Shared LRU cache configuration for plugins that use `ShardedLruCache`.
//!
//! Embed this struct in any plugin config that needs an LRU cache.
//! Per-plugin values of `0` fall back to the global defaults in `LruCacheGlobalConfig`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Reusable LRU cache configuration.
///
/// ## YAML example (inside a plugin config)
/// ```yaml
/// lruCache:
///   capacity: 2048
///   shards: 8
/// ```
///
/// Both fields default to `0`, which means "use the global
/// `lru_cache.default_capacity` / `lru_cache.default_shards`".
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct LruCacheConfig {
    /// LRU cache capacity (number of entries).
    /// Default: 0 (uses global `lru_cache.default_capacity`, typically 1024).
    #[serde(default)]
    pub capacity: usize,

    /// Number of cache shards to reduce lock contention under high concurrency.
    /// Default: 0 (uses global `lru_cache.default_shards`; 0 = single shard, no sharding).
    #[serde(default)]
    pub shards: usize,
}

impl Default for LruCacheConfig {
    fn default() -> Self {
        Self { capacity: 0, shards: 0 }
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
}
