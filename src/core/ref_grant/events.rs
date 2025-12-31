//! Event notification system for ReferenceGrant changes
//!
//! Provides event-driven revalidation when ReferenceGrants are modified

use std::sync::{Arc, RwLock, OnceLock};
use std::collections::HashSet;

/// ReferenceGrant changed event
#[derive(Debug, Clone)]
pub struct ReferenceGrantChangedEvent {
    /// Affected namespaces (from and to namespaces)
    /// Empty set means all namespaces should be revalidated
    pub affected_namespaces: HashSet<String>,
}

/// Revalidation listener trait
pub trait RevalidationListener: Send + Sync {
    /// Called when ReferenceGrant changes
    fn on_reference_grant_changed(&self, event: &ReferenceGrantChangedEvent);
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
        let mut listeners = self.listeners.write().unwrap();
        listeners.push(listener);
        tracing::debug!(
            component = "ref_grant_events",
            "Registered revalidation listener"
        );
    }
    
    /// Dispatch event to all listeners
    pub fn dispatch(&self, event: &ReferenceGrantChangedEvent) {
        let listeners = self.listeners.read().unwrap();
        tracing::info!(
            component = "ref_grant_events",
            affected_ns_count = event.affected_namespaces.len(),
            listener_count = listeners.len(),
            "Dispatching ReferenceGrantChanged event"
        );
        
        for listener in listeners.iter() {
            listener.on_reference_grant_changed(event);
        }
    }
}

impl Default for ReferenceGrantEventDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Global event dispatcher singleton
static GLOBAL_DISPATCHER: OnceLock<Arc<ReferenceGrantEventDispatcher>> = OnceLock::new();

pub fn get_global_dispatcher() -> Arc<ReferenceGrantEventDispatcher> {
    GLOBAL_DISPATCHER
        .get_or_init(|| Arc::new(ReferenceGrantEventDispatcher::new()))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TestListener {
        call_count: AtomicUsize,
    }

    impl RevalidationListener for TestListener {
        fn on_reference_grant_changed(&self, _event: &ReferenceGrantChangedEvent) {
            self.call_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn test_dispatcher_registers_and_dispatches() {
        let dispatcher = ReferenceGrantEventDispatcher::new();
        let listener = Arc::new(TestListener {
            call_count: AtomicUsize::new(0),
        });
        
        dispatcher.register_listener(listener.clone());
        
        let event = ReferenceGrantChangedEvent {
            affected_namespaces: HashSet::new(),
        };
        
        dispatcher.dispatch(&event);
        
        assert_eq!(listener.call_count.load(Ordering::SeqCst), 1);
    }
}

