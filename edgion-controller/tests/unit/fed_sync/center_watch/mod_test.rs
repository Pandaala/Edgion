use super::*;
use projection::{Identity, Setting, SourceKind};

fn row(name: &str, rv: &str) -> Record {
    Record::Setting(Setting {
        identity: Identity {
            key: SourceKey::new(SourceKind::EdgionConfigData, "apps", name).unwrap(),
            uid: None,
            resource_version: Some(rv.into()),
        },
        enabled: true,
        cluster_name: Some("fleet".into()),
        region_name: Some("east".into()),
        invalid_identity: false,
    })
}

#[test]
fn snapshot_delta_boundary_preserves_updates_and_deletes_without_tombstones() {
    let store = CenterWatchStore::default();
    let generation = store.generation();
    store.upsert(&generation, row("one", "1")).unwrap();
    let list = store.snapshot().unwrap();
    store.upsert(&generation, row("one", "1")).unwrap();
    assert_eq!(store.snapshot().unwrap().version, list.version);
    store.upsert(&generation, row("one", "2")).unwrap();
    store.upsert(&generation, row("two", "1")).unwrap();
    store.delete(&generation, row("one", "").key()).unwrap();
    let (version, events) = store.events_since(&list.generation, list.version).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(version, 4);
    assert_eq!(*list.records[0], row("one", "1"));
    assert!(events.last().unwrap().record.is_none());
    assert_eq!(store.snapshot().unwrap().records.len(), 1);
    assert_eq!(store.state.lock().rows.len(), 1);
    assert_eq!(
        store.events_since(&generation, version + 1).unwrap_err(),
        StoreError::FutureVersion
    );
}

#[test]
fn history_expiry_requires_relist_and_is_bounded() {
    let store = CenterWatchStore::default();
    let generation = store.generation();
    for n in 0..MAX_EVENTS + 5 {
        store.upsert(&generation, row("one", &n.to_string())).unwrap();
    }
    assert_eq!(store.events_since(&generation, 0).unwrap_err(), StoreError::TooOld);
    let state = store.state.lock();
    assert!(state.history.len() <= MAX_EVENTS);
    assert!(state.history_bytes <= MAX_HISTORY_BYTES);
    assert_eq!(state.rows.len(), 1);
    let floor = state.history_floor;
    drop(state);
    assert!(store.events_since(&generation, floor).is_ok());
}

#[test]
fn reset_fences_late_producers_and_faults_block_all_reads() {
    let store = CenterWatchStore::default();
    let old = store.generation();
    let mut notifications = store.subscribe();
    assert_eq!(store.fail(&old, StoreError::Projection), Err(StoreError::Projection));
    assert!(notifications.has_changed().unwrap());
    notifications.borrow_and_update();
    assert_eq!(store.snapshot().unwrap_err(), StoreError::Projection);
    assert_eq!(store.events_since(&old, 0).unwrap_err(), StoreError::Projection);
    store.reset();
    assert!(notifications.has_changed().unwrap());
    assert_ne!(store.generation(), old);
    assert_eq!(store.upsert(&old, row("one", "1")), Err(StoreError::StaleGeneration));
    assert_eq!(
        store.delete(&old, row("one", "1").key()),
        Err(StoreError::StaleGeneration)
    );
    assert_eq!(store.fail(&old, StoreError::Capacity), Err(StoreError::StaleGeneration));
    assert!(store.snapshot().unwrap().records.is_empty());
}

#[test]
fn live_overflow_never_silently_evicts_or_returns_partial_success() {
    let store = CenterWatchStore::default();
    let generation = store.generation();
    let mut failed = false;
    for n in 0..=MAX_ROWS {
        if store.upsert(&generation, row(&format!("item-{n}"), "1")) == Err(StoreError::Capacity) {
            failed = true;
            break;
        }
    }
    assert!(failed);
    assert_eq!(store.snapshot().unwrap_err(), StoreError::Capacity);
    assert_eq!(store.events_since(&generation, 0).unwrap_err(), StoreError::Capacity);
    let state = store.state.lock();
    assert!(state.rows.len() <= MAX_ROWS && state.bytes <= MAX_LIVE_BYTES);
    assert!(state.history_bytes <= MAX_HISTORY_BYTES);
}

#[test]
fn concurrent_kind_writers_allocate_monotonic_versions_inside_store_lock() {
    let store = Arc::new(CenterWatchStore::default());
    let threads: Vec<_> = (0..4)
        .map(|writer| {
            let store = store.clone();
            std::thread::spawn(move || {
                let generation = store.generation();
                for n in 0..40 {
                    store
                        .upsert(&generation, row(&format!("item-{writer}-{n}"), "1"))
                        .unwrap();
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let list = store.snapshot().unwrap();
    let (version, events) = store.events_since(&list.generation, 0).unwrap();
    assert_eq!(version, 160);
    assert_eq!(
        events.iter().map(|event| event.version).collect::<Vec<_>>(),
        (1..=160).collect::<Vec<_>>()
    );
}

#[test]
fn runtime_reset_owns_center_store_generation() {
    let runtime = crate::conf_mgr::ControllerRuntime::new();
    let old = runtime.center_watch.generation();
    runtime.center_watch.upsert(&old, row("one", "1")).unwrap();
    runtime.reset();
    assert_ne!(runtime.center_watch.generation(), old);
    assert!(runtime.center_watch.snapshot().unwrap().records.is_empty());
}

#[test]
fn notification_does_not_hold_store_lock_while_receiver_has_a_borrow() {
    let store = Arc::new(CenterWatchStore::default());
    let receiver = store.subscribe();
    let held = receiver.borrow();
    let writer_store = store.clone();
    let writer = std::thread::spawn(move || {
        writer_store
            .upsert(&writer_store.generation(), row("one", "1"))
            .unwrap();
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut observed = false;
    while std::time::Instant::now() < deadline {
        if store.state.try_lock().is_some_and(|state| state.version == 1) {
            observed = true;
            break;
        }
        std::thread::yield_now();
    }
    // Always release the receiver and join before asserting, including failures.
    drop(held);
    writer.join().unwrap();
    assert!(observed, "notification held the publication lock");
}

#[test]
fn accounting_includes_reserved_string_capacity() {
    let store = CenterWatchStore::default();
    let mut record = row("one", "1");
    let Record::Setting(setting) = &mut record else {
        panic!("setting");
    };
    let mut reserved = String::with_capacity(MAX_RECORD_BYTES * 2);
    reserved.push_str("fleet");
    setting.cluster_name = Some(reserved);
    assert_eq!(store.upsert(&store.generation(), record), Err(StoreError::Capacity));
    assert_eq!(store.state.lock().rows.len(), 0);
}

#[test]
fn capacity_failure_recovers_after_space_is_freed_and_failed_key_is_reprocessed() {
    let store = CenterWatchStore::default();
    let generation = store.generation();
    let mut rejected = None;
    for n in 0..=MAX_ROWS {
        let record = row(&format!("item-{n}"), "1");
        if store.upsert(&generation, record.clone()).is_err() {
            rejected = Some(record);
            break;
        }
    }
    let rejected = rejected.expect("bounded live capacity");
    assert_eq!(store.failed_keys(&generation).unwrap(), vec![rejected.key().clone()]);
    store.delete(&generation, row("item-0", "1").key()).unwrap();
    store.delete(&generation, row("item-1", "1").key()).unwrap();
    assert!(store.snapshot().is_err());
    store.upsert(&generation, rejected).unwrap();
    assert!(store.failed_keys(&generation).unwrap().is_empty());
    assert!(store.snapshot().is_ok());
}

#[test]
fn source_fault_tracking_is_bounded_and_late_faults_are_fenced() {
    let store = CenterWatchStore::default();
    let generation = store.generation();
    for n in 0..MAX_ROWS + 1 {
        let key = row(&format!("bad-{n}"), "1").key().clone();
        assert_eq!(
            store.fail_source(&generation, key, StoreError::Projection),
            Err(StoreError::Projection)
        );
    }
    let state = store.state.lock();
    assert!(state.failure_bytes <= MAX_FAILURE_BYTES);
    assert!(state.failures.len() <= MAX_ROWS);
    assert_eq!(state.fault, Some(StoreError::Capacity));
    drop(state);
    store.reset();
    assert_eq!(
        store.fail_source(&generation, row("late", "1").key().clone(), StoreError::Projection),
        Err(StoreError::StaleGeneration)
    );
    assert!(store.snapshot().is_ok());
}
