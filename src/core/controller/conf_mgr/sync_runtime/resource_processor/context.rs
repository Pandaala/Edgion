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
use crate::core::controller::conf_mgr::ControllerRuntime;

/// Context for handler methods
///
/// This struct provides handlers with access to:
/// - Secret management utilities (via runtime.secret_ref_manager)
/// - Cross-resource requeue mechanism (with cycle detection)
/// - Processing configuration
pub struct HandlerContext {
    /// Metadata filter configuration
    metadata_filter: Option<Arc<MetadataFilterConfig>>,

    /// Namespace filter (if set, only process resources in these namespaces)
    namespace_filter: Option<Arc<Vec<String>>>,

    /// Trigger chain up to and including the current resource being processed.
    /// Used for cycle detection when calling `requeue`.
    trigger_chain: TriggerChain,

    /// Maximum times a (kind, key) pair may repeat in a trigger chain
    max_trigger_cycles: usize,

    /// Maximum total depth of a trigger chain (safety net for cross-kind loops)
    max_trigger_depth: usize,

    /// Controller runtime for shared stores (non-pub, access via methods)
    runtime: Arc<ControllerRuntime>,
}

impl HandlerContext {
    /// Create a new HandlerContext
    pub fn new(
        metadata_filter: Option<Arc<MetadataFilterConfig>>,
        namespace_filter: Option<Arc<Vec<String>>>,
        trigger_chain: TriggerChain,
        max_trigger_cycles: usize,
        max_trigger_depth: usize,
        runtime: Arc<ControllerRuntime>,
    ) -> Self {
        Self {
            metadata_filter,
            namespace_filter,
            trigger_chain,
            max_trigger_cycles,
            max_trigger_depth,
            runtime,
        }
    }

    /// Get SecretRefManager reference
    pub fn secret_ref_manager(&self) -> &super::SecretRefManager {
        &self.runtime.secret_ref_manager
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

    /// Access the controller runtime for shared stores.
    pub fn runtime(&self) -> &ControllerRuntime {
        &self.runtime
    }

    /// Cross-resource requeue with cascade cycle detection.
    ///
    /// Checks the trigger chain for cycles before enqueueing. If the target
    /// (kind, key) pair has already appeared `max_trigger_cycles` times in
    /// the chain, the requeue is dropped and an error is logged.
    ///
    /// Uses the delay subsystem (`requeue_with_chain`) for coalescing.
    pub async fn requeue(&self, kind: &str, key: String) {
        if self.trigger_chain.depth() >= self.max_trigger_depth {
            tracing::error!(
                target_kind = kind,
                target_key = %key,
                chain = %self.trigger_chain,
                depth = self.trigger_chain.depth(),
                max_depth = self.max_trigger_depth,
                "Trigger chain depth limit reached, dropping requeue"
            );
            return;
        }
        if self
            .trigger_chain
            .would_exceed_cycle_limit(kind, &key, self.max_trigger_cycles)
        {
            tracing::error!(
                target_kind = kind,
                target_key = %key,
                chain = %self.trigger_chain,
                max_cycles = self.max_trigger_cycles,
                "Trigger cycle limit reached, dropping requeue"
            );
            return;
        }
        self.runtime
            .processor_registry
            .requeue_with_chain(kind, key, self.trigger_chain.clone())
            .await;
    }

    /// Clean metadata using the configured filter
    pub fn clean_metadata<K>(&self, obj: &mut K)
    where
        K: kube::Resource,
    {
        if let Some(filter) = &self.metadata_filter {
            edgion_common::utils::clean_metadata(obj, filter);
        }
    }
}
