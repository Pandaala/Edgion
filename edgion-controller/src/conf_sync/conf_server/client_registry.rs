//! Lightweight registry for tracking connected gateway instances.
//!
//! Thread-safe: RwLock for client map, Notify for change signaling.
//! Designed for low write frequency (only on gateway connect/disconnect).

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Notify;

pub struct ClientRegistry {
    /// client_id -> number of live metadata registrations
    clients: RwLock<HashMap<String, usize>>,
    /// Notify watchers when count changes
    notify: Notify,
}

/// One metadata stream's ownership of a gateway presence entry.
#[must_use = "hold the registration for the metadata stream lifetime"]
pub(super) struct ClientRegistration {
    registry: Arc<ClientRegistry>,
    client_id: String,
}

impl Drop for ClientRegistration {
    fn drop(&mut self) {
        self.registry.release(&self.client_id);
    }
}

impl ClientRegistry {
    pub fn new() -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            notify: Notify::new(),
        }
    }

    /// Acquire one registration; overlapping streams share one counted gateway.
    pub(super) fn register(self: &Arc<Self>, client_id: String, client_name: String) -> ClientRegistration {
        let mut clients = self.clients.write();
        let registrations = clients.entry(client_id.clone()).or_default();
        *registrations += 1;
        let is_new = *registrations == 1;
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
        ClientRegistration {
            registry: self.clone(),
            client_id,
        }
    }

    /// Release only this stream's registration; the last owner removes presence.
    fn release(&self, client_id: &str) {
        let mut clients = self.clients.write();
        let Some(registrations) = clients.get_mut(client_id) else {
            return;
        };
        *registrations -= 1;
        if *registrations == 0 {
            clients.remove(client_id);
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
