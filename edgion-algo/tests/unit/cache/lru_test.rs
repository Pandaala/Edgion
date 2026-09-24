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
