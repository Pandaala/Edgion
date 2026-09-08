use super::*;

fn cache(bytes: u64, entries: usize) -> Arc<OciMemoryCache> {
    let cache = OciMemoryCache::new(OciMemoryCacheConfig {
        max_bytes: bytes,
        max_entries: entries,
        ttl: Duration::from_secs(10),
        purge_interval: Duration::from_secs(1),
    })
    .unwrap();
    cache.register_owner();
    cache
}

fn digest(letter: char) -> String {
    letter.to_string().repeat(64)
}

fn put(cache: &Arc<OciMemoryCache>, letter: char, bytes: Vec<u8>) -> ArtifactBytes {
    cache.access().store_verified(CacheKind::Blob, &digest(letter), bytes)
}

#[test]
fn empty_reports_zero_and_metadata_configuration_is_bounded() {
    let cache = cache(64, 2);
    let stats = cache.stats();
    assert_eq!(stats.entries, 0);
    assert_eq!(stats.retained_bytes, 0);
    assert!(stats.keys.is_empty());
    assert!(OciMemoryCache::new(OciMemoryCacheConfig {
        max_bytes: 1,
        max_entries: usize::MAX,
        ttl: Duration::ZERO,
        purge_interval: Duration::ZERO,
    })
    .is_err());
    let normalized = OciMemoryCache::new(OciMemoryCacheConfig {
        max_bytes: 0,
        max_entries: 0,
        ttl: Duration::ZERO,
        purge_interval: Duration::ZERO,
    })
    .unwrap();
    assert_eq!(normalized.cfg.ttl, MIN_INTERVAL);
    assert_eq!(normalized.cfg.purge_interval, MIN_INTERVAL);
}

#[test]
fn allocation_capacity_charged_and_hits_share_body() {
    let cache = cache(16, 4);
    let mut source = Vec::with_capacity(16);
    source.extend_from_slice(b"wasm");
    let ptr = source.as_ptr();
    let stored = put(&cache, 'a', source);
    assert_eq!(stored.as_ptr(), ptr);
    assert_eq!(cache.stats().retained_bytes, 16);
    let hit = cache
        .access()
        .load(CacheKind::Blob, &digest('a').to_ascii_uppercase())
        .unwrap();
    assert!(Arc::ptr_eq(&stored.0, &hit.0));
    assert!(cache.access().load(CacheKind::Manifest, &digest('a')).is_none());
    assert!(cache.access().load(CacheKind::Blob, "invalid").is_none());
    let duplicate = put(&cache, 'a', b"wasm".to_vec());
    assert!(Arc::ptr_eq(&stored.0, &duplicate.0));
    assert_eq!(cache.stats().retained_bytes, 16);
}

#[test]
fn byte_and_entry_pressure_evict_lru_and_oversized_bypasses() {
    let cache = cache(8, 2);
    drop(put(&cache, 'a', vec![1; 4]));
    drop(put(&cache, 'b', vec![2; 4]));
    drop(cache.access().load(CacheKind::Blob, &digest('a')).unwrap());
    drop(put(&cache, 'c', vec![3; 4]));
    assert!(cache.access().load(CacheKind::Blob, &digest('b')).is_none());
    assert_eq!(cache.stats().entries, 2);
    assert_eq!(cache.stats().retained_bytes, 8);
    let oversized = put(&cache, 'd', vec![4; 9]);
    assert_eq!(oversized.len(), 9);
    assert_eq!(cache.stats().entries, 2);
    assert_eq!(cache.stats().retained_bytes, 8);

    let count_limited = self::cache(100, 1);
    drop(put(&count_limited, 'a', vec![1; 4]));
    drop(put(&count_limited, 'b', vec![2; 4]));
    assert_eq!(count_limited.stats().entries, 1);
    assert_eq!(count_limited.stats().retained_bytes, 4);
}

#[test]
fn pinned_eviction_keeps_charge_until_final_reader_drops() {
    let cache = cache(8, 2);
    let pinned = put(&cache, 'a', vec![1; 8]);
    let reader = pinned.clone();
    let bypass = put(&cache, 'b', vec![2; 4]);
    assert_eq!(cache.stats().entries, 0);
    assert_eq!(cache.stats().retained_bytes, 8);
    assert_eq!(bypass.as_ref(), &[2; 4]);
    drop(pinned);
    assert_eq!(cache.stats().retained_bytes, 8);
    drop(reader);
    assert_eq!(cache.stats().retained_bytes, 0);
    drop(put(&cache, 'b', vec![2; 4]));
    assert_eq!(cache.stats().retained_bytes, 4);
}

#[test]
fn zero_limits_disable_retention_without_rejecting_valid_bytes() {
    for (bytes, entries) in [(0, 4), (64, 0), (0, 0)] {
        let cache = cache(bytes, entries);
        let value = put(&cache, 'a', vec![1; 8]);
        assert_eq!(value.as_ref(), &[1; 8]);
        assert_eq!(cache.stats().entries, 0);
        assert_eq!(cache.stats().retained_bytes, 0);
    }
}

#[test]
fn last_owner_clear_blocks_late_admission_even_after_reregistration() {
    let cache = cache(64, 4);
    cache.register_owner();
    let old_access = cache.access();
    let pinned = put(&cache, 'a', vec![1; 8]);
    cache.unregister_owner();
    assert_eq!(cache.stats().entries, 1);
    cache.unregister_owner();
    assert_eq!(cache.stats().entries, 0);
    assert_eq!(cache.stats().retained_bytes, 8);
    drop(old_access.store_verified(CacheKind::Blob, &digest('b'), vec![2; 8]));
    assert_eq!(cache.stats().entries, 0);
    cache.register_owner();
    drop(old_access.store_verified(CacheKind::Blob, &digest('b'), vec![2; 8]));
    assert_eq!(cache.stats().entries, 0);
    drop(put(&cache, 'b', vec![2; 8]));
    assert_eq!(cache.stats().entries, 1);
    assert_eq!(cache.stats().retained_bytes, 16);
    drop(pinned);
    assert_eq!(cache.stats().retained_bytes, 8);
}

#[test]
fn touch_extends_idle_ttl_and_sweep_preserves_lru() {
    let cache = cache(64, 4);
    drop(put(&cache, 'a', vec![1; 8]));
    drop(put(&cache, 'b', vec![2; 8]));
    let key = CacheKind::Blob.key(&digest('a')).unwrap();
    {
        let _policy = cache.policy.lock();
        let mut entry = cache.entries.get(&key).unwrap();
        entry.last_used = Instant::now() - Duration::from_secs(9);
        cache.entries.insert(key.clone(), entry, None);
    }
    drop(cache.access().load(CacheKind::Blob, &digest('a')).unwrap());
    let before = cache.entries.get_keys();
    {
        let _policy = cache.policy.lock();
        cache.sweep(Instant::now() + Duration::from_secs(2));
    }
    assert_eq!(cache.entries.get_keys(), before);
    assert_eq!(cache.stats().entries, 2);
    {
        let _policy = cache.policy.lock();
        cache.sweep(Instant::now() + Duration::from_secs(11));
    }
    assert_eq!(cache.stats().entries, 0);
    assert_eq!(cache.stats().retained_bytes, 0);
}

#[tokio::test]
async fn maintenance_physically_releases_idle_entries_without_lookups_and_drops() {
    let cache = cache(64, 4);
    drop(put(&cache, 'a', vec![1; 8]));
    let key = CacheKind::Blob.key(&digest('a')).unwrap();
    {
        let _policy = cache.policy.lock();
        let mut entry = cache.entries.get(&key).unwrap();
        entry.last_used = Instant::now() - Duration::from_secs(11);
        cache.entries.insert(key, entry, None);
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        while cache.budget.load(Ordering::Acquire) != 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(cache.entries.len(), 0);
    let task = cache.maintenance.lock().as_ref().unwrap().clone();
    let weak = Arc::downgrade(&cache);
    drop(cache);
    tokio::task::yield_now().await;
    assert!(weak.upgrade().is_none());
    assert!(task.is_finished());
}

#[tokio::test]
async fn cancelled_claim_owner_and_waiter_release_bounded_coordination() {
    let cache = cache(64, 4);
    let access = cache.access();
    let claim = access.claim(CacheKind::Blob, &digest('a')).await.unwrap();
    let waiting = tokio::spawn({
        let access = access.clone();
        async move { access.claim(CacheKind::Blob, &digest('a')).await }
    });
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished());
    waiting.abort();
    assert!(waiting.await.unwrap_err().is_cancelled());
    drop(claim);
    let claim = tokio::time::timeout(Duration::from_secs(1), access.claim(CacheKind::Blob, &digest('a')))
        .await
        .unwrap();
    assert!(claim.is_some());
    assert_eq!(cache.claims.len(), CLAIM_STRIPES);
}
