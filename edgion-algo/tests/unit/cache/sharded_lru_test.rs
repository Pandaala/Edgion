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
