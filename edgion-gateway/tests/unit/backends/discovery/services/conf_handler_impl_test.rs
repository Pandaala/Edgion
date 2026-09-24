use super::*;
use crate::backends::discovery::ServiceStore;

fn create_test_service(namespace: &str, name: &str) -> Service {
    let json = serde_json::json!({
        "apiVersion": "v1",
        "kind": "Service",
        "metadata": {
            "namespace": namespace,
            "name": name
        },
        "spec": {
            "ports": [{
                "port": 80,
                "targetPort": 8080
            }],
            "selector": {
                "app": name
            }
        }
    });
    serde_json::from_value(json).expect("Failed to create Service")
}

fn seed_dns_cache(service_key: &str) {
    get_global_dns_cache().insert(
        service_key,
        "seed.example.test",
        "fp-test",
        vec!["10.20.0.1".parse().expect("valid IPv4")],
        std::time::Duration::from_secs(60),
        std::time::Instant::now(),
    );
}

fn dns_cache_has(service_key: &str) -> bool {
    get_global_dns_cache()
        .get(service_key, "seed.example.test", "fp-test", std::time::Instant::now())
        .is_some()
}

fn handler_and_store() -> (ServiceHandler, ServiceStore) {
    let coordinator = Arc::new(ServiceDiscoveryCoordinator::new());
    (
        ServiceHandler {
            coordinator: Arc::clone(&coordinator),
        },
        ServiceStore::new(coordinator),
    )
}

#[test]
fn partial_update_invalidates_dns_cache_for_removed_services() {
    let (store_handler, _store) = handler_and_store();
    let removed = "dnscache-partial-removed/svc";
    let kept = "dnscache-partial-kept/svc";
    seed_dns_cache(removed);
    seed_dns_cache(kept);

    store_handler.partial_update(HashMap::new(), HashMap::new(), HashSet::from([removed.to_string()]));

    assert!(!dns_cache_has(removed), "removed Service's entry must be invalidated");
    assert!(dns_cache_has(kept), "an unrelated Service's entry must survive");
}

#[test]
fn partial_update_does_not_invalidate_dns_cache_for_updated_services() {
    let (store_handler, _store) = handler_and_store();
    let updated = "dnscache-partial-updated/svc";
    seed_dns_cache(updated);

    let mut update = HashMap::new();
    update.insert(
        updated.to_string(),
        Arc::new(create_test_service("dnscache-partial-updated", "svc")),
    );
    store_handler.partial_update(HashMap::new(), update, HashSet::new());

    assert!(dns_cache_has(updated), "an updated Service must not be invalidated");
}

#[test]
fn full_set_invalidates_dns_cache_for_services_absent_from_the_new_set() {
    let (store_handler, _store) = handler_and_store();
    let dropped = "dnscache-fullset-dropped/svc";
    let retained = "dnscache-fullset-retained/svc";

    let mut initial = HashMap::new();
    initial.insert(
        dropped.to_string(),
        Arc::new(create_test_service("dnscache-fullset-dropped", "svc")),
    );
    store_handler.full_set(&initial);

    seed_dns_cache(dropped);
    seed_dns_cache(retained);

    store_handler.full_set(&HashMap::new());

    assert!(!dns_cache_has(dropped), "dropped Service's entry must be invalidated");
    assert!(
        dns_cache_has(retained),
        "a Service this store never held must not be swept"
    );
}

#[test]
fn test_full_set() {
    let (store_handler, store) = handler_and_store();

    let mut data = HashMap::new();
    data.insert(
        "default/svc1".to_string(),
        Arc::new(create_test_service("default", "svc1")),
    );
    data.insert(
        "default/svc2".to_string(),
        Arc::new(create_test_service("default", "svc2")),
    );

    store_handler.full_set(&data);

    assert!(store.contains("default/svc1"));
    assert!(store.contains("default/svc2"));
    assert!(!store.contains("default/svc3"));
}

#[test]
fn test_partial_update_add() {
    let (store_handler, store) = handler_and_store();

    let mut add = HashMap::new();
    add.insert(
        "default/svc1".to_string(),
        Arc::new(create_test_service("default", "svc1")),
    );

    store_handler.partial_update(add, HashMap::new(), HashSet::new());

    assert!(store.contains("default/svc1"));
}

#[test]
fn test_partial_update_update() {
    let (store_handler, store) = handler_and_store();

    let mut data = HashMap::new();
    data.insert(
        "default/svc1".to_string(),
        Arc::new(create_test_service("default", "svc1")),
    );
    store_handler.full_set(&data);

    let mut update = HashMap::new();
    update.insert(
        "default/svc1".to_string(),
        Arc::new(create_test_service("default", "svc1")),
    );

    store_handler.partial_update(HashMap::new(), update, HashSet::new());

    assert!(store.contains("default/svc1"));
}

#[test]
fn test_partial_update_remove() {
    let (store_handler, store) = handler_and_store();

    let mut data = HashMap::new();
    data.insert(
        "default/svc1".to_string(),
        Arc::new(create_test_service("default", "svc1")),
    );
    store_handler.full_set(&data);

    let mut remove = HashSet::new();
    remove.insert("default/svc1".to_string());
    store_handler.partial_update(HashMap::new(), HashMap::new(), remove);

    assert!(!store.contains("default/svc1"));
}
