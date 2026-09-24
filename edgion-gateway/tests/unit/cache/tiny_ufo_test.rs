use super::*;
use edgion_algo::cache::LocalLruCache;
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
