//! HandlerContext - Context for ProcessorHandler methods
//!
//! Provides access to shared resources needed during processing:
//! - SecretRefManager for tracking Secret dependencies
//! - Requeue function for cross-resource notifications
//! - Trigger chain for cascade tracking and cycle detection
//! - Process configuration (metadata filter, namespace filter)

use std::sync::Arc;

use crate::core::controller::conf_mgr::conf_center::MetadataFilterConfig;
use crate::core::controller::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::core::controller::conf_mgr::PROCESSOR_REGISTRY;

use super::SecretRefManager;

/// Context for handler methods
///
/// This struct provides handlers with access to:
/// - Secret management utilities
/// - Cross-resource requeue mechanism (with cycle detection)
/// - Processing configuration
pub struct HandlerContext {
    /// Secret reference manager (tracks which resources depend on which secrets)
    pub secret_ref_manager: Arc<SecretRefManager>,

    /// Metadata filter configuration
    pub metadata_filter: Option<Arc<MetadataFilterConfig>>,

    /// Namespace filter (if set, only process resources in these namespaces)
    pub namespace_filter: Option<Arc<Vec<String>>>,

    /// Trigger chain up to and including the current resource being processed.
    /// Used for cycle detection when calling `requeue`.
    trigger_chain: TriggerChain,

    /// Maximum times a (kind, key) pair may repeat in a trigger chain
    max_trigger_cycles: usize,
}

impl HandlerContext {
    /// Create a new HandlerContext
    pub fn new(
        secret_ref_manager: Arc<SecretRefManager>,
        metadata_filter: Option<Arc<MetadataFilterConfig>>,
        namespace_filter: Option<Arc<Vec<String>>>,
        trigger_chain: TriggerChain,
        max_trigger_cycles: usize,
    ) -> Self {
        Self {
            secret_ref_manager,
            metadata_filter,
            namespace_filter,
            trigger_chain,
            max_trigger_cycles,
        }
    }

    /// Get SecretRefManager reference
    pub fn secret_ref_manager(&self) -> &SecretRefManager {
        &self.secret_ref_manager
    }

    /// Get metadata filter configuration
    pub fn metadata_filter(&self) -> Option<&MetadataFilterConfig> {
        self.metadata_filter.as_ref().map(|arc| arc.as_ref())
    }

    /// Get namespace filter
    pub fn namespace_filter(&self) -> Option<&Vec<String>> {
        self.namespace_filter.as_ref().map(|arc| arc.as_ref())
    }

    /// Get the current trigger chain (for diagnostics / logging)
    pub fn trigger_chain(&self) -> &TriggerChain {
        &self.trigger_chain
    }

    /// Cross-resource requeue with cascade cycle detection.
    ///
    /// Checks the trigger chain for cycles before enqueueing. If the target
    /// (kind, key) pair has already appeared `max_trigger_cycles` times in
    /// the chain, the requeue is dropped and an error is logged.
    ///
    /// Uses the delay subsystem (`requeue_with_chain`) for coalescing.
    pub fn requeue(&self, kind: &str, key: String) {
        if self.trigger_chain.would_exceed_cycle_limit(kind, &key, self.max_trigger_cycles) {
            tracing::error!(
                target_kind = kind,
                target_key = %key,
                chain = %self.trigger_chain,
                max_cycles = self.max_trigger_cycles,
                "Trigger cycle limit reached, dropping requeue"
            );
            return;
        }
        PROCESSOR_REGISTRY.requeue_with_chain(kind, key, self.trigger_chain.clone());
    }

    /// Clean metadata using the configured filter
    pub fn clean_metadata<K>(&self, obj: &mut K)
    where
        K: kube::Resource,
    {
        if let Some(filter) = &self.metadata_filter {
            crate::core::common::utils::clean_metadata(obj, filter);
        }
    }
}
