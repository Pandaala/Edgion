use super::*;
use std::sync::Arc;
use std::thread;

#[test]
fn test_unknown_backend_defaults_healthy() {
    let store = HealthStatusStore::new();
    assert!(store.is_healthy("default/svc", 12345));
}

#[test]
fn test_consecutive_failures_trigger_unhealthy() {
    let store = HealthStatusStore::new();
    let svc = "default/svc";
    let hash = 1_u64;
    store.register_service(svc, vec![hash]);

    store.record_check(svc, hash, false, 2, 3);
    assert!(store.is_healthy(svc, hash));
    store.record_check(svc, hash, false, 2, 3);
    assert!(store.is_healthy(svc, hash));
    store.record_check(svc, hash, false, 2, 3);
    assert!(!store.is_healthy(svc, hash));
    assert_eq!(store.unhealthy_count(svc), 1);
}

#[test]
fn test_consecutive_successes_trigger_healthy() {
    let store = HealthStatusStore::new();
    let svc = "default/svc";
    let hash = 2_u64;
    store.register_service(svc, vec![hash]);

    store.record_check(svc, hash, false, 2, 1);
    assert!(!store.is_healthy(svc, hash));
    store.record_check(svc, hash, true, 2, 1);
    assert!(!store.is_healthy(svc, hash));
    store.record_check(svc, hash, true, 2, 1);
    assert!(store.is_healthy(svc, hash));
    assert_eq!(store.unhealthy_count(svc), 0);
}

#[test]
fn test_success_resets_failure_counter() {
    let store = HealthStatusStore::new();
    let svc = "default/svc";
    let hash = 3_u64;
    store.register_service(svc, vec![hash]);

    store.record_check(svc, hash, false, 2, 3);
    store.record_check(svc, hash, false, 2, 3);
    store.record_check(svc, hash, true, 2, 3);
    store.record_check(svc, hash, false, 2, 3);
    assert!(store.is_healthy(svc, hash));
}

#[test]
fn test_failure_resets_success_counter() {
    let store = HealthStatusStore::new();
    let svc = "default/svc";
    let hash = 4_u64;
    store.register_service(svc, vec![hash]);

    store.record_check(svc, hash, false, 2, 1);
    assert!(!store.is_healthy(svc, hash));
    store.record_check(svc, hash, true, 2, 1);
    store.record_check(svc, hash, false, 2, 1);
    assert!(!store.is_healthy(svc, hash));
}

#[test]
fn test_register_unregister_service_cleanup() {
    let store = HealthStatusStore::new();
    let svc = "default/svc";
    store.register_service(svc, vec![10, 20]);
    assert!(store.has_service(svc));
    assert!(store.is_healthy(svc, 10));

    store.unregister_service(svc);
    assert!(!store.has_service(svc));
    assert!(store.is_healthy(svc, 10));
    assert_eq!(store.service_count(), 0);
}

#[test]
fn test_cross_service_backend_isolation() {
    let store = HealthStatusStore::new();
    let shared = 88_u64;
    let svc_a = "default/svc-a";
    let svc_b = "default/svc-b";
    store.register_service(svc_a, vec![shared]);
    store.register_service(svc_b, vec![shared]);

    store.record_check(svc_a, shared, false, 1, 1);
    assert!(!store.is_healthy(svc_a, shared));
    assert!(store.is_healthy(svc_b, shared));
}

#[test]
fn test_concurrent_access() {
    let store = Arc::new(HealthStatusStore::new());
    let svc = "default/svc";
    let hash = 99_u64;
    store.register_service(svc, vec![hash]);

    let mut threads = Vec::new();
    for _ in 0..8 {
        let store_ref = store.clone();
        threads.push(thread::spawn(move || {
            for i in 0..1000 {
                store_ref.record_check(svc, hash, i % 2 == 0, 2, 3);
                let _ = store_ref.is_healthy(svc, hash);
            }
        }));
    }

    for handle in threads {
        handle.join().expect("join thread");
    }
}
