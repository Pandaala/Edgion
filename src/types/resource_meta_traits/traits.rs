//! ResourceMeta trait definition

use kube::core::ObjectMeta;
use serde::de::DeserializeOwned;

use crate::types::resource_kind::ResourceKind;

/// Trait for Kubernetes resources with metadata and type information
///
/// This trait combines:
/// - Resource version tracking (for optimistic concurrency control)
/// - Resource kind identification (for routing and dispatching)
/// - Human-readable type names (for logging and debugging)
/// - Unique identifier generation (namespace/name format)
/// - Pre-parsing hook for runtime-only fields
pub trait ResourceMeta: DeserializeOwned + Send + Sync + 'static {
    /// Get the resource version as u64
    fn get_version(&self) -> u64;

    /// Get the ResourceKind enum variant for this type
    fn resource_kind() -> ResourceKind;

    /// Get a human-readable name for this resource type
    fn kind_name() -> &'static str;

    /// Get a unique key identifier for this resource (namespace/name format)
    /// Returns "namespace/name" for namespaced resources, or "name" for cluster-scoped resources
    fn key_name(&self) -> String;

    /// Pre-parse hook for populating runtime-only fields after deserialization
    ///
    /// This method is called after a resource is deserialized from YAML/JSON
    /// to populate any computed/runtime fields that are not part of the serialized data.
    ///
    /// Default implementation does nothing. Override for resources that need pre-processing.
    fn pre_parse(&mut self) {
        // Default: no-op
    }
}

/// Helper function to extract version from Kubernetes resource_version string
/// Returns 0 if resource_version is None or cannot be parsed
pub(super) fn extract_version(metadata: &ObjectMeta) -> u64 {
    metadata
        .resource_version
        .as_ref()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
}
