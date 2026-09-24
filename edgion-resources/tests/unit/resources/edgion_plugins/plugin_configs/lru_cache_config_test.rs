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
