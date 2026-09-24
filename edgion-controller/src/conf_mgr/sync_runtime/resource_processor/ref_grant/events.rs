//! Event notification system for ReferenceGrant changes
//!
//! Provides event-driven revalidation when ReferenceGrants are modified

use parking_lot::RwLock;
use std::collections::HashSet;
use std::sync::Arc;

/// ReferenceGrant changed event
#[derive(Debug, Clone)]
pub struct ReferenceGrantChangedEvent {
    /// Affected namespaces (from and to namespaces)
    /// Empty set means all namespaces should be revalidated
    pub affected_namespaces: HashSet<String>,
}

/// Revalidation listener trait
#[async_trait::async_trait]
pub trait RevalidationListener: Send + Sync {
    /// Called when ReferenceGrant changes
    async fn on_reference_grant_changed(&self, event: &ReferenceGrantChangedEvent);
}

/// Global event dispatcher
pub struct ReferenceGrantEventDispatcher {
    listeners: RwLock<Vec<Arc<dyn RevalidationListener>>>,
}

impl ReferenceGrantEventDispatcher {
    pub fn new() -> Self {
        Self {
            listeners: RwLock::new(Vec::new()),
        }
    }

    /// Register a listener
    pub fn register_listener(&self, listener: Arc<dyn RevalidationListener>) {
        let mut listeners = self.listeners.write();
        listeners.push(listener);
        tracing::debug!(component = "ref_grant_events", "Registered revalidation listener");
    }

    /// Clear all registered listeners.
    ///
    /// **Do not call this on the normal reset path** -- listeners are
    /// registered once at process startup via `ConfMgr::create()` and must
    /// persist across resets. If this method is called inside reset,
    /// ReferenceGrant changes will never re-enqueue routes after the first
    /// restart (see the `ControllerRuntime::reset()` comments).
    ///
    /// For testing use only.
    pub fn clear_listeners(&self) {
        self.listeners.write().clear();
        tracing::debug!(component = "ref_grant_events", "Cleared all revalidation listeners");
    }

    /// Dispatch event to all listeners
    pub async fn dispatch(&self, event: &ReferenceGrantChangedEvent) {
        let listeners: Vec<_> = self.listeners.read().clone();
        tracing::info!(
            component = "ref_grant_events",
            affected_ns_count = event.affected_namespaces.len(),
            listener_count = listeners.len(),
            "Dispatching ReferenceGrantChanged event"
        );

        for listener in &listeners {
            listener.on_reference_grant_changed(event).await;
        }
    }
}

impl Default for ReferenceGrantEventDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/ref_grant/events_test.rs"]
mod tests;
