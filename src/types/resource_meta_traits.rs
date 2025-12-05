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
/// - Unique identifier generation (namespace/name format)
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
}


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
    
    fn key_name(&self) -> String {
        // GatewayClass is cluster-scoped, so no namespace
        self.metadata.name.as_deref().unwrap_or("").to_string()
    }
}

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
    
    fn key_name(&self) -> String {
        // EdgionGatewayConfig is cluster-scoped, so no namespace
        self.metadata.name.as_deref().unwrap_or("").to_string()
    }
}

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
    
    fn key_name(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }
}

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
    
    fn key_name(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }
}

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
    
    fn key_name(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }
}

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
    
    fn key_name(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }
}

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
    
    fn key_name(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }
}

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
    
    fn key_name(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }
}

