//! HandlerContext - Context for ProcessorHandler methods
//!
//! Provides access to shared resources needed during processing:
//! - SecretRefManager for tracking Secret dependencies
//! - Requeue function for cross-resource notifications
//! - Process configuration (metadata filter, namespace filter)

use std::sync::Arc;

use k8s_openapi::api::core::v1::Secret;

use crate::core::conf_mgr::conf_center::MetadataFilterConfig;
use crate::core::conf_mgr::PROCESSOR_REGISTRY;
use crate::core::conf_sync::types::ListData;

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

    /// Function to get secrets list (injected to avoid circular dependency)
    secrets_list_fn: Option<Box<dyn Fn() -> ListData<Secret> + Send + Sync>>,
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
            secrets_list_fn: None,
        }
    }

    /// Set the secrets list function
    pub fn with_secrets_list_fn<F>(mut self, f: F) -> Self
    where
        F: Fn() -> ListData<Secret> + Send + Sync + 'static,
    {
        self.secrets_list_fn = Some(Box::new(f));
        self
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

    /// Get secrets list (if available)
    pub fn list_secrets(&self) -> Option<ListData<Secret>> {
        self.secrets_list_fn.as_ref().map(|f| f())
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
