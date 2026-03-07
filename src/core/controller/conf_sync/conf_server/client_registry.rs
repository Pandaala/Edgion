//! Lightweight registry for tracking connected gateway instances.
//!
//! Thread-safe: RwLock for client map, Notify for change signaling.
//! Designed for low write frequency (only on gateway connect/disconnect).

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::SystemTime;
use tokio::sync::Notify;

/// Lightweight registry for tracking connected gateway instances.
///
/// Thread-safe: RwLock for client map, Notify for change signaling.
/// Designed for low write frequency (only on gateway connect/disconnect).
pub struct ClientRegistry {
    /// client_id -> client metadata
    clients: RwLock<HashMap<String, ClientMeta>>,
    /// Notify watchers when count changes
    notify: Notify,
}

struct ClientMeta {
    #[allow(dead_code)]
    client_name: String,
    #[allow(dead_code)]
    connected_at: SystemTime,
}

impl ClientRegistry {
    pub fn new() -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            notify: Notify::new(),
        }
    }

    /// Register a gateway client. Notifies watchers if count changes.
    pub fn register(&self, client_id: String, client_name: String) {
        let mut clients = self.clients.write().unwrap();
        let is_new = !clients.contains_key(&client_id);
        clients.insert(
            client_id.clone(),
            ClientMeta {
                client_name: client_name.clone(),
                connected_at: SystemTime::now(),
            },
        );
        if is_new {
            tracing::info!(
                client_id = %client_id,
                client_name = %client_name,
                total = clients.len(),
                "Gateway client registered"
            );
            drop(clients); // Release lock before notify
            self.notify.notify_waiters();
        }
    }

    /// Unregister a gateway client. Notifies watchers if count changes.
    pub fn unregister(&self, client_id: &str) {
        let mut clients = self.clients.write().unwrap();
        if clients.remove(client_id).is_some() {
            tracing::info!(
                client_id = %client_id,
                total = clients.len(),
                "Gateway client unregistered"
            );
            drop(clients);
            self.notify.notify_waiters();
        }
    }

    /// Get current connected gateway count (at least 1)
    pub fn count(&self) -> u32 {
        self.clients.read().unwrap().len().max(1) as u32
    }

    /// Wait until the count changes
    pub async fn wait_for_change(&self) {
        self.notify.notified().await;
    }
}

impl Default for ClientRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
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
}
