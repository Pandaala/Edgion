//! Lightweight registry for tracking connected gateway instances.
//!
//! Thread-safe: RwLock for client map, Notify for change signaling.
//! Designed for low write frequency (only on gateway connect/disconnect).

use parking_lot::RwLock;
use std::collections::HashMap;
use tokio::sync::Notify;

pub struct ClientRegistry {
    /// client_id -> client metadata
    clients: RwLock<HashMap<String, ClientMeta>>,
    /// Notify watchers when count changes
    notify: Notify,
}

struct ClientMeta {}

impl ClientRegistry {
    pub fn new() -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            notify: Notify::new(),
        }
    }

    /// Register a gateway client. Notifies watchers if count changes.
    pub fn register(&self, client_id: String, client_name: String) {
        let mut clients = self.clients.write();
        let is_new = !clients.contains_key(&client_id);
        clients.insert(client_id.clone(), ClientMeta {});
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
        let mut clients = self.clients.write();
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
        self.clients.read().len().max(1) as u32
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
#[path = "../../../tests/unit/conf_sync/conf_server/client_registry_test.rs"]
mod tests;
