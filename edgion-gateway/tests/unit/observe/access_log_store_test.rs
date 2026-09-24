use super::*;
use std::sync::{Arc, Barrier};

#[test]
fn test_store_and_get() {
    let store = AccessLogStore::new();
    store
        .store("trace-001".to_string(), r#"{"test": true}"#.to_string())
        .unwrap();

    let result = store.get("trace-001");
    assert_eq!(result, Some(r#"{"test": true}"#.to_string()));
}

#[test]
fn test_get_nonexistent() {
    let store = AccessLogStore::new();
    assert_eq!(store.get("nonexistent"), None);
}

#[test]
fn test_delete() {
    let store = AccessLogStore::new();
    store
        .store("trace-001".to_string(), r#"{"test": true}"#.to_string())
        .unwrap();

    assert!(store.delete("trace-001"));
    assert_eq!(store.get("trace-001"), None);
}

#[test]
fn test_list() {
    let store = AccessLogStore::new();
    store.store("trace-001".to_string(), "{}".to_string()).unwrap();
    store.store("trace-002".to_string(), "{}".to_string()).unwrap();

    let list = store.list();
    assert_eq!(list.len(), 2);
    assert!(list.iter().all(|entry| entry.store_count == 1));
}

#[test]
fn repeated_trace_counts_logging_boundary_writes() {
    let store = AccessLogStore::new();
    store
        .store("trace-001".to_string(), r#"{"value":1}"#.to_string())
        .unwrap();
    store
        .store("trace-001".to_string(), r#"{"value":2}"#.to_string())
        .unwrap();

    assert_eq!(store.get("trace-001"), Some(r#"{"value":2}"#.to_string()));
    let list = store.list();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].store_count, 2);
}

#[test]
fn repeated_trace_crosses_cleanup_cadence_without_self_deadlock() {
    const WRITES: usize = 101;
    let store = AccessLogStore::new();
    for write in 0..WRITES {
        store
            .store("trace-001".to_string(), format!(r#"{{"write":{write}}}"#))
            .unwrap();
    }

    let list = store.list();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].store_count, WRITES as u64);
    assert_eq!(store.status().total_stored, WRITES as u64);
}

#[test]
fn concurrent_first_writes_are_all_counted() {
    const WRITERS: usize = 32;
    let store = AccessLogStore::new();
    let barrier = Arc::new(Barrier::new(WRITERS));

    std::thread::scope(|scope| {
        for writer in 0..WRITERS {
            let barrier = Arc::clone(&barrier);
            let store = &store;
            scope.spawn(move || {
                barrier.wait();
                store
                    .store("shared-trace".to_string(), format!(r#"{{"writer":{writer}}}"#))
                    .unwrap();
            });
        }
    });

    let list = store.list();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].store_count, WRITERS as u64);
}

#[test]
fn concurrent_distinct_writes_cannot_exceed_capacity() {
    const WRITERS: usize = 32;
    let store = AccessLogStore::with_limits(DEFAULT_TTL, 1);
    let barrier = Arc::new(Barrier::new(WRITERS));

    let accepted = std::thread::scope(|scope| {
        let handles = (0..WRITERS)
            .map(|writer| {
                let barrier = Arc::clone(&barrier);
                let store = &store;
                scope.spawn(move || {
                    barrier.wait();
                    store.store(format!("trace-{writer}"), "{}".to_string()).is_ok()
                })
            })
            .collect::<Vec<_>>();

        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|accepted| *accepted)
            .count()
    });

    assert_eq!(accepted, 1);
    let status = store.status();
    assert_eq!(status.entry_count, 1);
    assert_eq!(status.total_stored, 1);
}

#[test]
fn rejected_write_does_not_increment_total_stored() {
    let store = AccessLogStore::with_limits(DEFAULT_TTL, 1);
    store.store("trace-1".to_string(), "{}".to_string()).unwrap();
    assert!(store.store("trace-2".to_string(), "{}".to_string()).is_err());

    assert_eq!(store.status().total_stored, 1);
}

#[test]
fn read_paths_remove_expired_entries() {
    let ttl = Duration::from_secs(1);
    let store = AccessLogStore::with_limits(ttl, 2);

    store.store("trace-1".to_string(), "{}".to_string()).unwrap();
    store.entries.get_mut("trace-1").unwrap().stored_at = Instant::now().checked_sub(ttl).unwrap();
    assert!(store.list().is_empty());

    store.store("trace-2".to_string(), "{}".to_string()).unwrap();
    store.entries.get_mut("trace-2").unwrap().stored_at = Instant::now().checked_sub(ttl).unwrap();
    assert_eq!(store.status().entry_count, 0);
}

#[test]
fn test_clear() {
    let store = AccessLogStore::new();
    store.store("trace-001".to_string(), "{}".to_string()).unwrap();
    store.store("trace-002".to_string(), "{}".to_string()).unwrap();

    store.clear();
    assert_eq!(store.list().len(), 0);
}

#[test]
fn test_status() {
    let store = AccessLogStore::new();
    store.store("trace-001".to_string(), "{}".to_string()).unwrap();

    let status = store.status();
    assert!(status.enabled);
    assert_eq!(status.entry_count, 1);
    assert_eq!(status.total_stored, 1);
    assert_eq!(status.max_capacity, DEFAULT_MAX_CAPACITY);
}
