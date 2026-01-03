//! Global Resource Type Registry
//!
//! This module provides a centralized registry for all resource types in the system.
//! It defines metadata for each resource type and provides utilities to access this information.

use std::sync::LazyLock;

/// Metadata for a resource type
#[derive(Debug, Clone)]
pub struct ResourceTypeMetadata {
    /// The name of the resource type (used for display and logging)
    pub name: &'static str,
    /// Description of the resource type (optional)
    pub description: Option<&'static str>,
    /// Whether this resource is a base configuration resource
    pub is_base_conf: bool,
}

impl ResourceTypeMetadata {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            description: None,
            is_base_conf: false,
        }
    }

    pub const fn with_description(mut self, description: &'static str) -> Self {
        self.description = Some(description);
        self
    }

    pub const fn base_conf(mut self) -> Self {
        self.is_base_conf = true;
        self
    }
}

/// Global registry of all resource types
/// This list defines all resources that are tracked by the system
pub static RESOURCE_TYPES: LazyLock<Vec<ResourceTypeMetadata>> = LazyLock::new(|| {
    vec![
        // Base configuration resources
        ResourceTypeMetadata::new("gateway_classes")
            .with_description("GatewayClass resources")
            .base_conf(),
        ResourceTypeMetadata::new("gateways")
            .with_description("Gateway resources")
            .base_conf(),
        ResourceTypeMetadata::new("edgion_gateway_configs")
            .with_description("EdgionGatewayConfig resources")
            .base_conf(),
        
        // Route resources
        ResourceTypeMetadata::new("routes")
            .with_description("HTTPRoute resources"),
        ResourceTypeMetadata::new("grpc_routes")
            .with_description("GRPCRoute resources"),
        ResourceTypeMetadata::new("tcp_routes")
            .with_description("TCPRoute resources"),
        ResourceTypeMetadata::new("udp_routes")
            .with_description("UDPRoute resources"),
        ResourceTypeMetadata::new("tls_routes")
            .with_description("TLSRoute resources"),
        
        // Backend resources
        ResourceTypeMetadata::new("services")
            .with_description("Kubernetes Service resources"),
        ResourceTypeMetadata::new("endpoint_slices")
            .with_description("Kubernetes EndpointSlice resources"),
        ResourceTypeMetadata::new("endpoints")
            .with_description("Kubernetes Endpoints resources"),
        
        // Security and policy resources
        ResourceTypeMetadata::new("edgion_tls")
            .with_description("EdgionTls resources for TLS configuration"),
        ResourceTypeMetadata::new("reference_grants")
            .with_description("ReferenceGrant resources for cross-namespace access"),
        ResourceTypeMetadata::new("backend_tls_policies")
            .with_description("BackendTLSPolicy resources"),
        
        // Plugin and extension resources
        ResourceTypeMetadata::new("edgion_plugins")
            .with_description("EdgionPlugins resources"),
        ResourceTypeMetadata::new("edgion_stream_plugins")
            .with_description("EdgionStreamPlugins resources"),
        ResourceTypeMetadata::new("plugin_metadata")
            .with_description("PluginMetaData resources"),
        
        // Infrastructure resources
        ResourceTypeMetadata::new("link_sys")
            .with_description("LinkSys resources for external system integration"),
        
        // Note: Secrets are not included as they follow related resources
    ]
});

/// Get the list of all resource type names
pub fn all_resource_type_names() -> Vec<&'static str> {
    RESOURCE_TYPES.iter().map(|r| r.name).collect()
}

/// Get the list of base configuration resource names
pub fn base_conf_resource_names() -> Vec<&'static str> {
    RESOURCE_TYPES
        .iter()
        .filter(|r| r.is_base_conf)
        .map(|r| r.name)
        .collect()
}

/// Get metadata for a specific resource type by name
pub fn get_resource_metadata(name: &str) -> Option<&ResourceTypeMetadata> {
    RESOURCE_TYPES.iter().find(|r| r.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_registry() {
        // Ensure we have resources registered
        assert!(!RESOURCE_TYPES.is_empty());
        
        // Check base conf resources
        let base_conf = base_conf_resource_names();
        assert!(base_conf.contains(&"gateway_classes"));
        assert!(base_conf.contains(&"gateways"));
        assert!(base_conf.contains(&"edgion_gateway_configs"));
        
        // Check non-base conf resources
        let all_names = all_resource_type_names();
        assert!(all_names.contains(&"routes"));
        assert!(all_names.contains(&"services"));
    }

    #[test]
    fn test_metadata_lookup() {
        let metadata = get_resource_metadata("gateway_classes");
        assert!(metadata.is_some());
        assert!(metadata.unwrap().is_base_conf);
        
        let metadata = get_resource_metadata("routes");
        assert!(metadata.is_some());
        assert!(!metadata.unwrap().is_base_conf);
    }
}

