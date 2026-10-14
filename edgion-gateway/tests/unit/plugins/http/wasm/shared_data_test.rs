use std::collections::BTreeSet;
use std::sync::{Arc, Barrier};

use super::*;

fn assert_gauge(handle: &edgion_common::observe::metrics_api::TestRecorder, module: &str, bytes: usize) {
    let rendered = handle.render().replace("{service=\"test\",", "{");
    let sample = format!("edgion_wasm_shared_data_bytes{{module=\"{module}\"}} {bytes}");
    assert!(
        rendered.lines().any(|line| line == sample),
        "expected {sample}:\n{rendered}"
    );
}

#[test]
fn gauge_aggregates_generations_and_withdraws_only_the_dropped_store() {
    for old_first in [true, false] {
        let recorder = lifetime_recorder();
        let handle = &recorder;
        recorder.with(|| {
            let module = format!("generation-{old_first}");
            let old = SharedData::new(module.clone());
            old.set("k", Some(vec![0; 100]), 0).unwrap();
            let new = SharedData::new(module.clone());
            assert_gauge(handle, &module, 229);
            new.set("k", Some(vec![0; 40]), 0).unwrap();
            assert_gauge(handle, &module, 398);
            old.set("k", Some(vec![0; 140]), 0).unwrap();
            new.set("k", Some(vec![0; 10]), 0).unwrap();
            assert_gauge(handle, &module, 408);
            assert_eq!(new.set("missing", Some(vec![0]), 1), Err(SharedDataError::CasMismatch));
            assert_eq!(
                new.set("large", Some(vec![0; MAX_SHARED_VALUE_BYTES + 1]), 0),
                Err(SharedDataError::TooLarge)
            );
            assert_eq!(
                new.set_with_total_limit("extra", Some(vec![0]), 0, 139),
                Err(SharedDataError::TooLarge)
            );
            new.set("missing", None, 0).unwrap();
            assert_gauge(handle, &module, 408);
            if old_first {
                drop(old);
                assert_gauge(handle, &module, 139);
                new.set("k", None, 0).unwrap();
                assert_gauge(handle, &module, 0);
                new.set("k", Some(vec![0; 10]), 0).unwrap();
                drop(new);
            } else {
                drop(new);
                assert_gauge(handle, &module, 269);
                drop(old);
            }
            assert_gauge(handle, &module, 0);
            let recreated = SharedData::new(module.clone());
            recreated.set("k", Some(vec![0]), 0).unwrap();
            assert_gauge(handle, &module, 130);
            drop(recreated);
            assert_gauge(handle, &module, 0);
            assert_eq!(recorder.stats().gauge_owners, 0);
        });
    }
}

#[test]
fn gauge_concurrent_mutations_registration_and_drop_balance() {
    let recorder = lifetime_recorder();
    let handle = &recorder;
    let module = "concurrent-shared-data";
    let retained = recorder.with(|| Arc::new(SharedData::new(module.into())));
    retained.set("retained", Some(vec![0; 20]), 0).unwrap();
    let start = Barrier::new(8);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let recorder = &recorder;
            let start = &start;
            let retained = &retained;
            scope.spawn(move || {
                start.wait();
                recorder.with(|| {
                    for _ in 0..100 {
                        let temporary = SharedData::new(module.into());
                        temporary.set("tmp", Some(vec![0; 50]), 0).unwrap();
                        retained.set("contended", Some(vec![0; 30]), 0).unwrap();
                        retained.set("contended", Some(vec![0; 5]), 0).unwrap();
                        retained.set("contended", None, 0).unwrap();
                    }
                });
            });
        }
    });
    assert_gauge(handle, module, accounted_map_bytes(&retained));
    // A request-held Arc keeps the contribution after its cache owner drops.
    let request = retained.clone();
    drop(retained);
    assert_gauge(handle, module, 156);
    drop(request);
    assert_gauge(handle, module, 0);
}

#[test]
fn gauge_live_store_survives_idle_eviction_and_releases_its_handle() {
    let recorder = lifetime_recorder();
    let handle = &recorder;
    let module = "idle-shared-data";
    let store = recorder.with(|| SharedData::new(module.into()));
    store.set("k", Some(vec![0; 10]), 0).unwrap();
    assert_gauge(handle, module, 139);
    recorder.advance(std::time::Duration::from_secs(2));
    assert!(!recorder.render().contains("edgion_wasm_shared_data_bytes{"));
    store.set("k", Some(vec![0; 11]), 0).unwrap();
    assert_gauge(handle, module, 140);
    let overlap = recorder.with(|| SharedData::new(module.into()));
    overlap.set("k", Some(vec![0; 20]), 0).unwrap();
    drop(store);
    assert_gauge(handle, module, 149);
    drop(overlap);
    assert_gauge(handle, module, 0);
    recorder.advance(std::time::Duration::from_secs(2));
    assert!(!handle.render().contains("edgion_wasm_shared_data_bytes{"));
    assert_eq!(recorder.stats().gauge_owners, 0);
}

#[test]
fn gauge_last_drop_racing_registration_preserves_the_new_store() {
    let recorder = lifetime_recorder();
    let handle = &recorder;
    let module = "drop-register-shared-data";
    for _ in 0..32 {
        let old = recorder.with(|| SharedData::new(module.into()));
        old.set("k", Some(vec![0; 100]), 0).unwrap();
        let start = Barrier::new(2);
        let new = std::thread::scope(|scope| {
            let start_ref = &start;
            scope.spawn(move || {
                start_ref.wait();
                drop(old);
            });
            start.wait();
            let new = recorder.with(|| SharedData::new(module.into()));
            new.set("k", Some(vec![0; 40]), 0).unwrap();
            new
        });
        assert_gauge(handle, module, 169);
        drop(new);
        assert_gauge(handle, module, 0);
    }
}

fn accounted_map_bytes(sd: &SharedData) -> usize {
    sd.map
        .iter()
        .map(|entry| {
            let (value, _) = entry.value();
            ENTRY_OVERHEAD_BYTES + entry.key().len() + value.len()
        })
        .sum()
}

#[test]
fn set_get_roundtrip_and_cas_increments() {
    let sd = SharedData::default();
    assert!(sd.get("k").is_none());
    sd.set("k", Some(b"v1".to_vec()), 0).unwrap();
    let (v, cas1) = sd.get("k").unwrap();
    assert_eq!(v, b"v1");
    assert_eq!(cas1, 1);
    sd.set("k", Some(b"v2".to_vec()), cas1).unwrap();
    let (v, cas2) = sd.get("k").unwrap();
    assert_eq!(v, b"v2");
    assert!(cas2 > cas1);
}

#[test]
fn cas_mismatch_rejected() {
    let sd = SharedData::default();
    sd.set("k", Some(b"v".to_vec()), 0).unwrap();
    assert_eq!(sd.set("k", Some(b"x".to_vec()), 999), Err(SharedDataError::CasMismatch));
    // Nonzero CAS on a missing key can never match.
    assert_eq!(
        sd.set("missing", Some(b"x".to_vec()), 1),
        Err(SharedDataError::CasMismatch)
    );
}

#[test]
fn delete_recreate_rejects_stale_cas() {
    let sd = SharedData::default();
    sd.set("k", Some(b"old".to_vec()), 0).unwrap();
    let (_, stale_cas) = sd.get("k").unwrap();

    sd.set("k", None, stale_cas).unwrap();
    sd.set("k", Some(b"new".to_vec()), 0).unwrap();
    let (new_value, new_cas) = sd.get("k").unwrap();
    assert_eq!(new_value, b"new");
    assert_ne!(new_cas, stale_cas);

    let total_before = sd.total_bytes();
    assert_eq!(
        sd.set("k", Some(b"stale overwrite".to_vec()), stale_cas),
        Err(SharedDataError::CasMismatch)
    );
    assert_eq!(sd.get("k"), Some((b"new".to_vec(), new_cas)));
    assert_eq!(sd.total_bytes(), total_before);
}

#[test]
fn tokens_wrap_from_max_to_one_then_two() {
    let sd = SharedData::default();
    sd.last_cas_token.store(u32::MAX - 1, Ordering::Relaxed);

    sd.set("max", Some(Vec::new()), 0).unwrap();
    sd.set("one", Some(Vec::new()), 0).unwrap();
    sd.set("two", Some(Vec::new()), 0).unwrap();

    assert_eq!(sd.get("max").unwrap().1, u32::MAX);
    assert_eq!(sd.get("one").unwrap().1, 1);
    assert_eq!(sd.get("two").unwrap().1, 2);
}

#[test]
fn concurrent_tokens_skip_zero_and_do_not_duplicate_across_wrap() {
    const WRITERS: usize = 8;
    let sd = Arc::new(SharedData::default());
    sd.last_cas_token.store(u32::MAX - 4, Ordering::Relaxed);
    let start = Arc::new(Barrier::new(WRITERS + 1));
    let mut handles = Vec::new();

    for index in 0..WRITERS {
        let sd = sd.clone();
        let start = start.clone();
        handles.push(std::thread::spawn(move || {
            let key = format!("wrap-{index}");
            start.wait();
            sd.set(&key, Some(vec![index as u8]), 0).unwrap();
            sd.get(&key).unwrap().1
        }));
    }
    start.wait();

    let tokens: BTreeSet<_> = handles.into_iter().map(|handle| handle.join().unwrap()).collect();
    let expected = BTreeSet::from([u32::MAX - 3, u32::MAX - 2, u32::MAX - 1, u32::MAX, 1, 2, 3, 4]);
    assert_eq!(tokens, expected);
    assert!(!tokens.contains(&0));
}

#[test]
fn failures_deletes_and_noops_do_not_consume_tokens() {
    let sd = SharedData::default();
    sd.set("k", Some(b"value".to_vec()), 0).unwrap();
    let (_, cas) = sd.get("k").unwrap();
    assert_eq!(sd.last_cas_token.load(Ordering::Relaxed), cas);

    assert_eq!(
        sd.set("k", Some(b"stale".to_vec()), cas.wrapping_add(1)),
        Err(SharedDataError::CasMismatch)
    );
    assert_eq!(
        sd.set("k", Some(vec![0; MAX_SHARED_VALUE_BYTES + 1]), cas),
        Err(SharedDataError::TooLarge)
    );
    assert_eq!(
        sd.set_with_total_limit("other", Some(Vec::new()), 0, sd.total_bytes()),
        Err(SharedDataError::TooLarge)
    );
    sd.set("k", None, cas).unwrap();
    sd.set("missing", None, 0).unwrap();
    assert_eq!(sd.set("missing", None, cas), Err(SharedDataError::CasMismatch));
    assert_eq!(sd.last_cas_token.load(Ordering::Relaxed), cas);

    sd.set("next", Some(Vec::new()), 0).unwrap();
    assert_eq!(sd.get("next").unwrap().1, cas.wrapping_add(1).max(1));
}

#[test]
fn reservation_single_slot_is_atomic_and_rolls_back() {
    const SLOT_BYTES: usize = 17;
    let sd = Arc::new(SharedData::default());
    let phase = Arc::new(Barrier::new(2));
    let worker = {
        let sd = sd.clone();
        let phase = phase.clone();
        std::thread::spawn(move || {
            let reservation = sd.try_reserve_bytes_up_to(SLOT_BYTES, SLOT_BYTES).unwrap();
            phase.wait();
            phase.wait();
            drop(reservation);
        })
    };

    phase.wait();
    assert_eq!(sd.total_bytes(), SLOT_BYTES);
    assert!(matches!(
        sd.try_reserve_bytes_up_to(SLOT_BYTES, SLOT_BYTES),
        Err(SharedDataError::TooLarge)
    ));
    phase.wait();
    worker.join().unwrap();
    assert_eq!(sd.total_bytes(), 0);

    let reservation = sd.try_reserve_bytes_up_to(SLOT_BYTES, SLOT_BYTES).unwrap();
    assert_eq!(sd.total_bytes(), SLOT_BYTES);
    drop(reservation);
    assert_eq!(sd.total_bytes(), 0);

    let one_byte = sd.try_reserve_bytes_up_to(1, usize::MAX).unwrap();
    assert!(matches!(
        sd.try_reserve_bytes_up_to(usize::MAX, usize::MAX),
        Err(SharedDataError::TooLarge)
    ));
    drop(one_byte);
    assert_eq!(sd.total_bytes(), 0);
}

#[test]
fn delete_frees_bytes() {
    let sd = SharedData::default();
    sd.set("key", Some(vec![0u8; 100]), 0).unwrap();
    let before = sd.total_bytes();
    assert!(before >= 100);
    sd.set("key", None, 0).unwrap();
    assert!(sd.get("key").is_none());
    assert!(sd.total_bytes() < before);
    // Deleting a missing key is a no-op.
    sd.set("key", None, 0).unwrap();
}

#[test]
fn accounting_tracks_value_shrink_and_delete() {
    let sd = SharedData::default();
    sd.set("k", Some(vec![0u8; 1000]), 0).unwrap();
    let after_big = sd.total_bytes();
    assert_eq!(after_big, ENTRY_OVERHEAD_BYTES + 1000 + "k".len());
    // Shrink the value: the counter MUST drop, not stay at the old size
    // (the old `if new_total >= old_len` guard dropped this decrement).
    sd.set("k", Some(vec![0u8; 10]), sd.get("k").unwrap().1).unwrap();
    assert_eq!(sd.total_bytes(), ENTRY_OVERHEAD_BYTES + 10 + "k".len());
    // Growing the same entry reserves only the positive value delta.
    let shrink_cas = sd.get("k").unwrap().1;
    sd.set("k", Some(vec![0u8; 100]), shrink_cas).unwrap();
    assert_eq!(sd.get("k").unwrap().1, shrink_cas + 1);
    assert_eq!(sd.total_bytes(), ENTRY_OVERHEAD_BYTES + 100 + "k".len());
    // Delete brings it to zero exactly — no phantom residue, no wrap.
    sd.set("k", None, 0).unwrap();
    assert_eq!(sd.total_bytes(), 0);
}

#[test]
fn entry_overhead_bounds_tiny_key_count() {
    // Pathological load: many tiny keys with empty values. Without the
    // per-entry surcharge the logical accounting admits ~4M such entries
    // (hundreds of MB of real memory); with it the entry count is bounded
    // to cap/(overhead+key) and the cap stays honest about RSS.
    let sd = SharedData::default();
    let per_entry = ENTRY_OVERHEAD_BYTES + 8;
    let expected_max = MAX_SHARED_TOTAL_BYTES / per_entry;
    let mut inserted = 0usize;
    loop {
        match sd.set(&format!("{inserted:08}"), Some(Vec::new()), 0) {
            Ok(()) => inserted += 1,
            Err(SharedDataError::TooLarge) => break,
            Err(e) => panic!("unexpected error: {e:?}"),
        }
        assert!(
            inserted <= expected_max,
            "cap admitted more entries than the surcharge allows"
        );
    }
    assert_eq!(inserted, expected_max);
    assert_eq!(sd.total_bytes(), expected_max * per_entry);
}

#[test]
fn concurrent_multikey_writes_keep_exact_total() {
    let sd = Arc::new(SharedData::default());
    let mut handles = Vec::new();
    for t in 0..8 {
        let sd = sd.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..200 {
                let key = format!("k{t}-{i}");
                sd.set(&key, Some(vec![0u8; 32]), 0).unwrap();
                // Shrink then delete: exercises every accounting branch.
                sd.set(&key, Some(vec![0u8; 8]), sd.get(&key).unwrap().1).unwrap();
                sd.set(&key, None, 0).unwrap();
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    // Every key was deleted; with exact atomic deltas the counter must
    // land back at exactly zero regardless of interleaving (a load/store
    // scheme would drift and could underflow-wrap to usize::MAX).
    assert_eq!(
        sd.total_bytes(),
        0,
        "atomic-delta accounting must be exact under concurrency"
    );
    assert_eq!(sd.total_bytes(), accounted_map_bytes(&sd));
}

#[test]
fn concurrent_public_writes_enforce_cap_and_exact_accounting() {
    const WRITERS: usize = 72;
    const KEY_LEN: usize = 8;
    let sd = Arc::new(SharedData::default());
    let start = Arc::new(Barrier::new(WRITERS + 1));
    let mut handles = Vec::new();

    for index in 0..WRITERS {
        let sd = sd.clone();
        let start = start.clone();
        handles.push(std::thread::spawn(move || {
            let key = format!("k{index:07}");
            start.wait();
            (key.clone(), sd.set(&key, Some(vec![0; MAX_SHARED_VALUE_BYTES]), 0))
        }));
    }
    start.wait();

    let mut successes = 0;
    for handle in handles {
        let (key, result) = handle.join().unwrap();
        match result {
            Ok(()) => {
                successes += 1;
                assert!(sd.get(&key).is_some());
            }
            Err(SharedDataError::TooLarge) => assert!(sd.get(&key).is_none()),
            Err(error) => panic!("unexpected concurrent write error: {error:?}"),
        }
    }

    let charge = ENTRY_OVERHEAD_BYTES + KEY_LEN + MAX_SHARED_VALUE_BYTES;
    assert_eq!(successes, MAX_SHARED_TOTAL_BYTES / charge);
    assert!(sd.total_bytes() <= MAX_SHARED_TOTAL_BYTES);
    assert_eq!(sd.total_bytes(), successes * charge);
    assert_eq!(sd.total_bytes(), accounted_map_bytes(&sd));
}

#[test]
fn rejected_mutations_leave_value_token_and_accounting_unchanged() {
    let sd = SharedData::default();
    sd.set("k", Some(b"stable".to_vec()), 0).unwrap();
    let snapshot = sd.get("k").unwrap();
    let total = sd.total_bytes();
    let last_token = sd.last_cas_token.load(Ordering::Relaxed);

    let assert_unchanged = |sd: &SharedData| {
        assert_eq!(sd.get("k"), Some(snapshot.clone()));
        assert_eq!(sd.total_bytes(), total);
        assert_eq!(sd.total_bytes(), accounted_map_bytes(sd));
        assert_eq!(sd.last_cas_token.load(Ordering::Relaxed), last_token);
    };

    assert_eq!(
        sd.set("k", Some(b"stale".to_vec()), snapshot.1.wrapping_add(1)),
        Err(SharedDataError::CasMismatch)
    );
    assert_unchanged(&sd);

    assert_eq!(
        sd.set("k", Some(vec![0; MAX_SHARED_VALUE_BYTES + 1]), snapshot.1),
        Err(SharedDataError::TooLarge)
    );
    assert_unchanged(&sd);

    assert_eq!(
        sd.set_with_total_limit("k", Some(b"stable+".to_vec()), snapshot.1, total),
        Err(SharedDataError::TooLarge)
    );
    assert_unchanged(&sd);

    assert_eq!(
        sd.set_with_total_limit("k", Some(b"stable+".to_vec()), snapshot.1.wrapping_add(1), total),
        Err(SharedDataError::CasMismatch),
        "CAS mismatch remains authoritative over aggregate capacity"
    );
    assert_unchanged(&sd);
}

#[test]
fn size_limits_enforced() {
    let sd = SharedData::default();
    assert_eq!(
        sd.set("big", Some(vec![0u8; MAX_SHARED_VALUE_BYTES + 1]), 0),
        Err(SharedDataError::TooLarge)
    );
    // Fill close to the total cap with max-size values (leaving room
    // for key bytes), then overflow.
    let n = MAX_SHARED_TOTAL_BYTES / MAX_SHARED_VALUE_BYTES - 1;
    for i in 0..n {
        sd.set(&format!("k{i}"), Some(vec![0u8; MAX_SHARED_VALUE_BYTES]), 0)
            .unwrap();
    }
    assert_eq!(
        sd.set("overflow", Some(vec![0u8; MAX_SHARED_VALUE_BYTES]), 0),
        Err(SharedDataError::TooLarge)
    );
}

fn lifetime_recorder() -> edgion_common::observe::metrics_api::TestRecorder {
    use edgion_common::observe::{
        cardinality_guard::CardinalityMode,
        metrics_api::{RecorderConfig, TestRecorder},
    };
    TestRecorder::with_clock(RecorderConfig {
        service: "test",
        idle_timeout_secs: 1,
        histogram_buckets: &[],
        cardinality_cap: 10000,
        cardinality_mode: CardinalityMode::Enforce,
    })
    .unwrap()
}
