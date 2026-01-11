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
pub fn extract_version(metadata: &ObjectMeta) -> u64 {
    metadata
        .resource_version
        .as_ref()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
}

/// Helper function to generate key_name from ObjectMeta
/// For namespaced resources: "namespace/name"
/// For cluster-scoped resources: "name"
pub fn key_name_from_metadata(metadata: &ObjectMeta) -> String {
    if let Some(namespace) = &metadata.namespace {
        format!("{}/{}", namespace, metadata.name.as_deref().unwrap_or(""))
    } else {
        metadata.name.as_deref().unwrap_or("").to_string()
    }
}

/// Macro to simplify ResourceMeta trait implementation
/// 
/// # Basic usage (namespaced resource, no pre_parse):
/// ```ignore
/// impl_resource_meta!(Gateway, Gateway, "Gateway");
/// ```
/// 
/// # With pre_parse:
/// ```ignore
/// impl_resource_meta!(HTTPRoute, HTTPRoute, "HTTPRoute", |r| {
///     r.preparse();
///     r.parse_timeouts();
/// });
/// ```
/// 
/// # Cluster-scoped resource (no namespace in key_name):
/// ```ignore
/// impl_resource_meta!(GatewayClass, GatewayClass, "GatewayClass", cluster_scoped);
/// ```
#[macro_export]
macro_rules! impl_resource_meta {
    // Basic: namespaced resource without pre_parse
    ($type:ty, $kind:ident, $kind_name:literal) => {
        impl $crate::types::ResourceMeta for $type {
            fn get_version(&self) -> u64 {
                $crate::types::resource_meta_traits::extract_version(&self.metadata)
            }

            fn resource_kind() -> $crate::types::ResourceKind {
                $crate::types::ResourceKind::$kind
            }

            fn kind_name() -> &'static str {
                $kind_name
            }

            fn key_name(&self) -> String {
                $crate::types::resource_meta_traits::key_name_from_metadata(&self.metadata)
            }
        }
    };

    // Cluster-scoped resource (no namespace in key_name)
    ($type:ty, $kind:ident, $kind_name:literal, cluster_scoped) => {
        impl $crate::types::ResourceMeta for $type {
            fn get_version(&self) -> u64 {
                $crate::types::resource_meta_traits::extract_version(&self.metadata)
            }

            fn resource_kind() -> $crate::types::ResourceKind {
                $crate::types::ResourceKind::$kind
            }

            fn kind_name() -> &'static str {
                $kind_name
            }

            fn key_name(&self) -> String {
                // Cluster-scoped: no namespace
                self.metadata.name.as_deref().unwrap_or("").to_string()
            }
        }
    };

    // With pre_parse closure
    ($type:ty, $kind:ident, $kind_name:literal, |$self:ident| $pre_parse:block) => {
        impl $crate::types::ResourceMeta for $type {
            fn get_version(&self) -> u64 {
                $crate::types::resource_meta_traits::extract_version(&self.metadata)
            }

            fn resource_kind() -> $crate::types::ResourceKind {
                $crate::types::ResourceKind::$kind
            }

            fn kind_name() -> &'static str {
                $kind_name
            }

            fn key_name(&self) -> String {
                $crate::types::resource_meta_traits::key_name_from_metadata(&self.metadata)
            }

            fn pre_parse(&mut $self) $pre_parse
        }
    };

    // Cluster-scoped with pre_parse
    ($type:ty, $kind:ident, $kind_name:literal, cluster_scoped, |$self:ident| $pre_parse:block) => {
        impl $crate::types::ResourceMeta for $type {
            fn get_version(&self) -> u64 {
                $crate::types::resource_meta_traits::extract_version(&self.metadata)
            }

            fn resource_kind() -> $crate::types::ResourceKind {
                $crate::types::ResourceKind::$kind
            }

            fn kind_name() -> &'static str {
                $kind_name
            }

            fn key_name(&self) -> String {
                // Cluster-scoped: no namespace
                self.metadata.name.as_deref().unwrap_or("").to_string()
            }

            fn pre_parse(&mut $self) $pre_parse
        }
    };
}
