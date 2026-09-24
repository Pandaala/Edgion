use super::*;

#[test]
fn test_register_and_count() {
    let registry = ClientRegistry::new();
    assert_eq!(registry.count(), 1); // At least 1

    registry.register("client-1".to_string(), "gw-1".to_string());
    assert_eq!(registry.count(), 1);

    registry.register("client-2".to_string(), "gw-2".to_string());
    assert_eq!(registry.count(), 2);

    // Re-register same client_id should not change count
    registry.register("client-1".to_string(), "gw-1-updated".to_string());
    assert_eq!(registry.count(), 2);
}

#[test]
fn test_unregister() {
    let registry = ClientRegistry::new();
    registry.register("client-1".to_string(), "gw-1".to_string());
    registry.register("client-2".to_string(), "gw-2".to_string());
    assert_eq!(registry.count(), 2);

    registry.unregister("client-1");
    assert_eq!(registry.count(), 1);

    registry.unregister("client-2");
    assert_eq!(registry.count(), 1); // At least 1

    // Unregister non-existent should be no-op
    registry.unregister("client-999");
    assert_eq!(registry.count(), 1);
}

#[tokio::test]
async fn test_wait_for_change_notifies() {
    let registry = std::sync::Arc::new(ClientRegistry::new());

    let registry_clone = registry.clone();
    let handle = tokio::spawn(async move {
        // Small delay then register
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        registry_clone.register("client-1".to_string(), "gw-1".to_string());
    });

    // Should unblock when register happens
    tokio::time::timeout(std::time::Duration::from_secs(2), registry.wait_for_change())
        .await
        .expect("wait_for_change should complete within timeout");

    handle.await.unwrap();
}
