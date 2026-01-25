//! ProcessorRegistry - Global registry for all ResourceProcessors
//!
//! Provides unified access to all processors via:
//! - Dynamic access by kind name: `get("HTTPRoute")`
//! - Cross-resource requeue: `requeue("Gateway", "default/my-gateway")`
//! - WatchObj collection for ConfigSyncServer

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

use super::sync_runtime::resource_processor::ProcessorObj;
use crate::core::conf_sync::conf_server::WatchObj;

/// Global processor registry instance
pub static PROCESSOR_REGISTRY: LazyLock<ProcessorRegistry> = LazyLock::new(ProcessorRegistry::new);

/// Resource kinds that should NOT be synced to client (Gateway)
///
/// These resources are only processed on the Controller side.
/// The client-side stores will exist but remain empty.
const NO_SYNC_KINDS: &[&str] = &["ReferenceGrant", "Secret"];

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
    /// Filters out resources in NO_SYNC_KINDS (e.g., ReferenceGrant, Secret)
    pub fn all_watch_objs(&self) -> HashMap<String, Arc<dyn WatchObj>> {
        self.processors
            .read()
            .unwrap()
            .iter()
            .filter(|(k, _)| !NO_SYNC_KINDS.contains(k))
            .map(|(k, v)| (k.to_string(), v.as_watch_obj()))
            .collect()
    }

    /// Cross-resource requeue
    ///
    /// Enqueues a key to another resource's workqueue.
    /// Used by processors to trigger reprocessing of dependent resources.
    ///
    /// # Example
    /// ```ignore
    /// // When a Secret changes, requeue dependent Gateway
    /// PROCESSOR_REGISTRY.requeue("Gateway", "default/my-gateway".to_string());
    /// ```
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

    /// Clear all registered processors
    ///
    /// Used when:
    /// - Restarting controller after failure
    /// - Losing leadership and re-election
    /// - Testing cleanup
    pub fn clear_registry(&self) {
        tracing::info!(component = "processor_registry", "Clearing all registered processors");
        self.processors.write().unwrap().clear();
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
