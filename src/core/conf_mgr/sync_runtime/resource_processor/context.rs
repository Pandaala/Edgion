//! HandlerContext - Context for ProcessorHandler methods
//!
//! Provides access to shared resources needed during processing:
//! - SecretRefManager for tracking Secret dependencies
//! - Requeue function for cross-resource notifications
//! - Process configuration (metadata filter, namespace filter)

use std::sync::Arc;

use crate::core::conf_mgr::conf_center::MetadataFilterConfig;
use crate::core::conf_mgr::PROCESSOR_REGISTRY;

use super::SecretRefManager;

/// Context for handler methods
///
/// This struct provides handlers with access to:
/// - Secret management utilities
/// - Cross-resource requeue mechanism
/// - Processing configuration
pub struct HandlerContext {
    /// Secret reference manager (tracks which resources depend on which secrets)
    pub secret_ref_manager: Arc<SecretRefManager>,

    /// Metadata filter configuration
    pub metadata_filter: Option<Arc<MetadataFilterConfig>>,

    /// Namespace filter (if set, only process resources in these namespaces)
    pub namespace_filter: Option<Arc<Vec<String>>>,
}

impl HandlerContext {
    /// Create a new HandlerContext
    pub fn new(
        secret_ref_manager: Arc<SecretRefManager>,
        metadata_filter: Option<Arc<MetadataFilterConfig>>,
        namespace_filter: Option<Arc<Vec<String>>>,
    ) -> Self {
        Self {
            secret_ref_manager,
            metadata_filter,
            namespace_filter,
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

    /// Cross-resource requeue
    ///
    /// Trigger reprocessing of a resource in another processor's queue.
    /// Used for cascading updates (e.g., Secret change triggers Gateway requeue).
    pub fn requeue(&self, kind: &str, key: String) {
        PROCESSOR_REGISTRY.requeue(kind, key);
    }

    /// Clean metadata using the configured filter
    pub fn clean_metadata<K>(&self, obj: &mut K)
    where
        K: kube::Resource,
    {
        if let Some(filter) = &self.metadata_filter {
            crate::core::utils::clean_metadata(obj, filter);
        }
    }
}
