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
