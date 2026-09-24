use super::*;
use kube::api::ObjectMeta;
use std::collections::HashMap;

#[test]
fn unavailable_source_counts_in_inventory_but_is_not_exposed_as_routable_raw() {
    let coordinator = Arc::new(ServiceDiscoveryCoordinator::new());
    coordinator.set_next_epoch_for_test(u64::MAX);
    coordinator.replace_services(&HashMap::from([
        (
            "default/backend".to_string(),
            Arc::new(Service {
                metadata: ObjectMeta {
                    namespace: Some("default".to_string()),
                    name: Some("backend".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
        ),
        (
            "default/other".to_string(),
            Arc::new(Service {
                metadata: ObjectMeta {
                    namespace: Some("default".to_string()),
                    name: Some("other".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
        ),
    ]));
    let store = ServiceStore::new(coordinator);
    assert_eq!(store.count(), 2);
    assert!(!store.contains("default/backend"));
    assert!(store.get("default/backend").is_none());
    assert!(store.keys().is_empty());
}
