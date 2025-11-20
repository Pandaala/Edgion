//! ResourceMeta trait and implementations
//!
//! This module provides the ResourceMeta trait for Kubernetes resources,
//! combining version information, resource kind, and type metadata.

use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::core::ObjectMeta;
use serde::de::DeserializeOwned;

use super::resource_kind::ResourceKind;
use super::resources::*;

/// Trait for Kubernetes resources with metadata and type information
/// 
/// This trait combines:
/// - Resource version tracking (for optimistic concurrency control)
/// - Resource kind identification (for routing and dispatching)
/// - Human-readable type names (for logging and debugging)
pub trait ResourceMeta: DeserializeOwned + Send + Sync + 'static {
    /// Get the resource version as u64
    fn get_version(&self) -> u64;
    
    /// Get the ResourceKind enum variant for this type
    fn resource_kind() -> ResourceKind;
    
    /// Get a human-readable name for this resource type
    fn kind_name() -> &'static str;
}

/// Deprecated: Use ResourceMeta instead
/// 
/// This is kept for backward compatibility with existing code.
pub trait Versionable: ResourceMeta {}

/// Helper function to extract version from Kubernetes resource_version string
/// Returns 0 if resource_version is None or cannot be parsed
fn extract_version(metadata: &ObjectMeta) -> u64 {
    metadata
        .resource_version
        .as_ref()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
}

impl ResourceMeta for GatewayClass {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
    
    fn resource_kind() -> ResourceKind {
        ResourceKind::GatewayClass
    }
    
    fn kind_name() -> &'static str {
        "GatewayClass"
    }
}

impl Versionable for GatewayClass {}

impl ResourceMeta for EdgionGatewayConfig {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
    
    fn resource_kind() -> ResourceKind {
        ResourceKind::EdgionGatewayConfig
    }
    
    fn kind_name() -> &'static str {
        "EdgionGatewayConfig"
    }
}

impl Versionable for EdgionGatewayConfig {}

impl ResourceMeta for Gateway {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
    
    fn resource_kind() -> ResourceKind {
        ResourceKind::Gateway
    }
    
    fn kind_name() -> &'static str {
        "Gateway"
    }
}

impl Versionable for Gateway {}

impl ResourceMeta for HTTPRoute {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
    
    fn resource_kind() -> ResourceKind {
        ResourceKind::HTTPRoute
    }
    
    fn kind_name() -> &'static str {
        "HTTPRoute"
    }
}

impl Versionable for HTTPRoute {}

impl ResourceMeta for Service {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
    
    fn resource_kind() -> ResourceKind {
        ResourceKind::Service
    }
    
    fn kind_name() -> &'static str {
        "Service"
    }
}

impl Versionable for Service {}

impl ResourceMeta for EndpointSlice {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
    
    fn resource_kind() -> ResourceKind {
        ResourceKind::EndpointSlice
    }
    
    fn kind_name() -> &'static str {
        "EndpointSlice"
    }
}

impl Versionable for EndpointSlice {}

impl ResourceMeta for Secret {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
    
    fn resource_kind() -> ResourceKind {
        ResourceKind::Secret
    }
    
    fn kind_name() -> &'static str {
        "Secret"
    }
}

impl Versionable for Secret {}

impl ResourceMeta for EdgionTls {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
    
    fn resource_kind() -> ResourceKind {
        ResourceKind::EdgionTls
    }
    
    fn kind_name() -> &'static str {
        "EdgionTls"
    }
}

impl Versionable for EdgionTls {}

