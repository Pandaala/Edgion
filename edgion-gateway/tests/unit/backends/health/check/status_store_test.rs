use super::*;
use std::sync::{Arc, Barrier};
use std::thread;

#[test]
fn test_register_unregister_service_cleanup() {
    let store = HealthStatusStore::new();
    let svc = "default/svc";
    store.register_service(svc);
    store.register_service(svc);
    assert!(store.has_service(svc));
    assert_eq!(store.tracked_service_count(), 1);
    assert_eq!(store.keys(), vec![svc.to_string()]);
    store.unregister_service(svc);
    store.unregister_service(svc);
    assert!(!store.has_service(svc));
    assert_eq!(store.tracked_service_count(), 0);
    assert!(store.keys().is_empty());
}

#[test]
fn test_concurrent_access() {
    let store = Arc::new(HealthStatusStore::new());
    let barrier = Arc::new(Barrier::new(8));
    let keys: Vec<String> = (0..8).map(|i| format!("default/svc-{i}")).collect();
    let mutate = |remove: bool| {
        let threads: Vec<_> = keys
            .iter()
            .cloned()
            .map(|key| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..100 {
                        if remove {
                            store.unregister_service(&key);
                        } else {
                            store.register_service(&key);
                        }
                        assert_eq!(store.has_service(&key), !remove);
                        let snapshot = store.keys();
                        assert_eq!(snapshot.contains(&key), !remove);
                    }
                })
            })
            .collect();
        for handle in threads {
            handle.join().expect("marker mutation thread");
        }
    };
    mutate(false);
    let mut actual = store.keys();
    actual.sort();
    assert_eq!(actual, keys);
    assert_eq!(store.tracked_service_count(), 8);
    mutate(true);
    assert_eq!(store.tracked_service_count(), 0);
    assert!(store.keys().is_empty());
}
