//! ProcessorRegistry - Global registry for all ResourceProcessors
//!
//! Provides unified access to all processors via:
//! - Dynamic access by kind name: `get("HTTPRoute")`
//! - Cross-resource requeue: `requeue("Gateway", "default/my-gateway")`
//! - WatchObj collection for ConfigSyncServer

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

use super::sync_runtime::resource_processor::{get_listener_port_manager, get_service_ref_manager, ProcessorObj};
use super::sync_runtime::workqueue::TriggerChain;
use crate::core::controller::conf_sync::conf_server::WatchObj;

/// Global processor registry instance
pub static PROCESSOR_REGISTRY: LazyLock<ProcessorRegistry> = LazyLock::new(ProcessorRegistry::new);

/// Registry that manages all ResourceProcessor instances
///
/// This provides a central place to:
/// 1. Register processors during startup
/// 2. Access processors by kind name
/// 3. Collect WatchObjs for ConfigSyncServer
/// 4. Enable cross-resource requeue
pub struct ProcessorRegistry {
    /// All processors indexed by kind name
    processors: RwLock<HashMap<&'static str, Arc<dyn ProcessorObj>>>,
}

impl ProcessorRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            processors: RwLock::new(HashMap::new()),
        }
    }

    /// Register a processor
    ///
    /// Called during ResourceProcessor initialization
    pub fn register(&self, processor: Arc<dyn ProcessorObj>) {
        let kind = processor.kind();
        let mut map = self.processors.write().unwrap();

        tracing::info!(component = "processor_registry", kind = kind, "Registering processor");

        map.insert(kind, processor);
    }

    /// Get a processor by kind name (dynamic)
    pub fn get(&self, kind: &str) -> Option<Arc<dyn ProcessorObj>> {
        self.processors.read().unwrap().get(kind).cloned()
    }

    /// Get all registered kind names
    pub fn all_kinds(&self) -> Vec<&'static str> {
        self.processors.read().unwrap().keys().copied().collect()
    }

    /// Get count of registered processors
    pub fn count(&self) -> usize {
        self.processors.read().unwrap().len()
    }

    /// Check if all registered processors are ready
    ///
    /// Returns `false` if registry is empty (must have at least one processor)
    pub fn is_all_ready(&self) -> bool {
        let processors = self.processors.read().unwrap();
        !processors.is_empty() && processors.values().all(|p| p.is_ready())
    }

    /// Get list of kinds that are not ready
    pub fn not_ready_kinds(&self) -> Vec<&'static str> {
        self.processors
            .read()
            .unwrap()
            .iter()
            .filter(|(_, p)| !p.is_ready())
            .map(|(k, _)| *k)
            .collect()
    }

    /// Get all WatchObjs for ConfigSyncServer registration
    ///
    /// Returns a HashMap that can be passed to ConfigSyncServer::register_all()
    /// Filters out resources in no_sync_kinds (configurable, default: ReferenceGrant, Secret)
    ///
    /// # Arguments
    /// * `no_sync_kinds` - List of resource kind names that should not be synced to Gateway
    pub fn all_watch_objs(&self, no_sync_kinds: &[&str]) -> HashMap<String, Arc<dyn WatchObj>> {
        self.processors
            .read()
            .unwrap()
            .iter()
            .filter(|(k, _)| !no_sync_kinds.contains(k))
            .map(|(k, v)| (k.to_string(), v.as_watch_obj()))
            .collect()
    }

    /// Cross-resource requeue (immediate, no trigger chain).
    ///
    /// Enqueues a key to another resource's workqueue immediately.
    /// Used for init-time revalidation and scenarios without cascade context.
    ///
    /// For cascade-aware requeue with delay coalescing, use `requeue_with_chain`.
    pub fn requeue(&self, kind: &str, key: String) {
        if let Some(processor) = self.get(kind) {
            processor.requeue(key.clone());
            tracing::debug!(
                component = "processor_registry",
                target_kind = kind,
                key = %key,
                "Cross-resource requeue triggered"
            );
        } else {
            tracing::warn!(
                component = "processor_registry",
                target_kind = kind,
                key = %key,
                "Requeue failed: processor not found"
            );
        }
    }

    /// Cross-resource requeue with trigger chain (delayed, coalesced).
    ///
    /// Enqueues a key through the delay subsystem for coalescing.
    /// The trigger chain is propagated for cascade cycle detection.
    ///
    /// Called by `HandlerContext::requeue` after cycle detection passes.
    pub fn requeue_with_chain(&self, kind: &str, key: String, chain: TriggerChain) {
        if let Some(processor) = self.get(kind) {
            processor.requeue_with_chain(key.clone(), chain);
            tracing::debug!(
                component = "processor_registry",
                target_kind = kind,
                key = %key,
                "Cross-resource requeue with chain triggered"
            );
        } else {
            tracing::warn!(
                component = "processor_registry",
                target_kind = kind,
                key = %key,
                "Requeue with chain failed: processor not found"
            );
        }
    }

    /// Set all processors to not ready state
    pub fn set_all_not_ready(&self) {
        for processor in self.processors.read().unwrap().values() {
            processor.set_not_ready();
        }
    }

    /// Clear all processors' caches
    pub fn clear_all(&self) {
        for processor in self.processors.read().unwrap().values() {
            processor.clear();
        }
    }

    /// Requeue all resources across all kinds.
    /// Used on leader transition to trigger full status reconciliation.
    pub fn requeue_all(&self) {
        let processors = self.processors.read().unwrap();
        for (kind, processor) in processors.iter() {
            let count = processor.requeue_all_keys();
            tracing::info!(
                component = "processor_registry",
                kind = kind,
                count = count,
                "Requeued all resources for leader status reconciliation"
            );
        }
    }

    /// Clear all registered processors and related global state
    ///
    /// Used when:
    /// - Restarting controller after failure
    /// - Losing leadership and re-election
    /// - Testing cleanup
    ///
    /// This also clears:
    /// - ListenerPortManager: Global port tracking for conflict detection
    ///
    /// Note: This does NOT immediately notify watch clients. The gRPC layer detects
    /// server_id changes after the new ConfigSyncServer is ready, ensuring clients
    /// only relist when the new server is available.
    pub fn clear_registry(&self) {
        tracing::info!(component = "processor_registry", "Clearing all registered processors");
        self.processors.write().unwrap().clear();

        // Clear ListenerPortManager to avoid stale port conflict data
        get_listener_port_manager().clear();
        tracing::info!(component = "processor_registry", "Cleared ListenerPortManager");

        // Clear ServiceRefManager to avoid stale Service→Route references
        get_service_ref_manager().clear();
        tracing::info!(component = "processor_registry", "Cleared ServiceRefManager");
    }
}

impl Default for ProcessorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_basic() {
        let registry = ProcessorRegistry::new();
        assert_eq!(registry.count(), 0);
        // Empty registry is NOT ready - must have at least one processor
        assert!(!registry.is_all_ready());
    }
}
