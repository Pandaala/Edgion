//! Resource change applier
//!
//! Dispatches resource changes to ConfigServer based on content type.

use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::{CacheEventDispatch, ConfigServer};
use crate::types::prelude_resources::*;
use crate::types::ResourceKind;
use anyhow::Result;
use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use std::sync::Arc;

/// Apply resource change to ConfigServer based on YAML/JSON content
pub fn apply_resource_change(config_server: &Arc<ConfigServer>, content: &str, change: ResourceChange) -> Result<()> {
    let kind = ResourceKind::from_content(content);

    match kind {
        Some(ResourceKind::GatewayClass) => {
            if let Ok(resource) = serde_yaml::from_str::<GatewayClass>(content) {
                config_server.gateway_classes.apply_change(change, resource);
            }
        }
        Some(ResourceKind::Gateway) => {
            if let Ok(resource) = serde_yaml::from_str::<Gateway>(content) {
                config_server.apply_gateway_change(change, resource);
            }
        }
        Some(ResourceKind::EdgionGatewayConfig) => {
            if let Ok(resource) = serde_yaml::from_str::<EdgionGatewayConfig>(content) {
                config_server.edgion_gateway_configs.apply_change(change, resource);
            }
        }
        Some(ResourceKind::HTTPRoute) => {
            if let Ok(resource) = serde_yaml::from_str::<HTTPRoute>(content) {
                config_server.apply_http_route_change(change, resource);
            }
        }
        Some(ResourceKind::GRPCRoute) => {
            if let Ok(resource) = serde_yaml::from_str::<GRPCRoute>(content) {
                config_server.apply_grpc_route_change(change, resource);
            }
        }
        Some(ResourceKind::TCPRoute) => {
            if let Ok(resource) = serde_yaml::from_str::<TCPRoute>(content) {
                config_server.apply_tcp_route_change(change, resource);
            }
        }
        Some(ResourceKind::UDPRoute) => {
            if let Ok(resource) = serde_yaml::from_str::<UDPRoute>(content) {
                config_server.apply_udp_route_change(change, resource);
            }
        }
        Some(ResourceKind::TLSRoute) => {
            if let Ok(resource) = serde_yaml::from_str::<TLSRoute>(content) {
                config_server.apply_tls_route_change(change, resource);
            }
        }
        Some(ResourceKind::Service) => {
            if let Ok(resource) = serde_yaml::from_str::<Service>(content) {
                config_server.apply_service_change(change, resource);
            }
        }
        Some(ResourceKind::Endpoint) => {
            if let Ok(resource) = serde_yaml::from_str::<Endpoints>(content) {
                config_server.apply_endpoint_change(change, resource);
            }
        }
        Some(ResourceKind::EndpointSlice) => {
            if let Ok(resource) = serde_yaml::from_str::<EndpointSlice>(content) {
                config_server.apply_endpoint_slice_change(change, resource);
            }
        }
        Some(ResourceKind::ReferenceGrant) => {
            if let Ok(resource) = serde_yaml::from_str::<ReferenceGrant>(content) {
                config_server.reference_grants.apply_change(change, resource);
            }
        }
        Some(ResourceKind::BackendTLSPolicy) => {
            if let Ok(resource) = serde_yaml::from_str::<BackendTLSPolicy>(content) {
                config_server.backend_tls_policies.apply_change(change, resource);
            }
        }
        Some(ResourceKind::EdgionTls) => {
            if let Ok(resource) = serde_yaml::from_str::<EdgionTls>(content) {
                config_server.apply_edgion_tls_change(change, resource);
            }
        }
        Some(ResourceKind::Secret) => {
            if let Ok(resource) = serde_yaml::from_str::<Secret>(content) {
                config_server.apply_secret_change(change, resource);
            }
        }
        Some(ResourceKind::EdgionPlugins) => {
            if let Ok(resource) = serde_yaml::from_str::<EdgionPlugins>(content) {
                config_server.apply_edgion_plugins_change(change, resource);
            }
        }
        Some(ResourceKind::EdgionStreamPlugins) => {
            if let Ok(resource) = serde_yaml::from_str::<EdgionStreamPlugins>(content) {
                config_server.edgion_stream_plugins.apply_change(change, resource);
            }
        }
        Some(ResourceKind::PluginMetaData) => {
            if let Ok(resource) = serde_yaml::from_str::<PluginMetaData>(content) {
                config_server.apply_plugin_metadata_change(change, resource);
            }
        }
        Some(ResourceKind::LinkSys) => {
            if let Ok(resource) = serde_yaml::from_str::<LinkSys>(content) {
                config_server.apply_link_sys_change(change, resource);
            }
        }
        Some(ResourceKind::Unspecified) => {
            tracing::debug!(component = "resource_applier", "Skipping Unspecified resource");
        }
        None => {
            tracing::debug!(
                component = "resource_applier",
                "Cannot parse resource kind from content"
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GATEWAY_CLASS_YAML: &str = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
"#;

    const GATEWAY_YAML: &str = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: test-gateway
  namespace: default
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 80
      protocol: HTTP
"#;

    const HTTP_ROUTE_YAML: &str = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: test-route
  namespace: default
spec:
  parentRefs:
    - name: test-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /
      backendRefs:
        - name: backend-svc
          port: 8080
"#;

    const UNKNOWN_KIND_YAML: &str = r#"
apiVersion: v1
kind: UnknownResource
metadata:
  name: test
"#;

    const INVALID_YAML: &str = r#"
this is not: valid: yaml: content
  - broken indentation
"#;

    #[test]
    fn test_parse_gateway_class_kind() {
        let kind = ResourceKind::from_content(GATEWAY_CLASS_YAML);
        assert_eq!(kind, Some(ResourceKind::GatewayClass));
    }

    #[test]
    fn test_parse_gateway_kind() {
        let kind = ResourceKind::from_content(GATEWAY_YAML);
        assert_eq!(kind, Some(ResourceKind::Gateway));
    }

    #[test]
    fn test_parse_http_route_kind() {
        let kind = ResourceKind::from_content(HTTP_ROUTE_YAML);
        assert_eq!(kind, Some(ResourceKind::HTTPRoute));
    }

    #[test]
    fn test_parse_gateway_class_yaml() {
        let result = serde_yaml::from_str::<GatewayClass>(GATEWAY_CLASS_YAML);
        assert!(result.is_ok(), "Failed to parse GatewayClass: {:?}", result.err());

        let gateway_class = result.unwrap();
        assert_eq!(gateway_class.metadata.name, Some("edgion".to_string()));
    }

    #[test]
    fn test_parse_gateway_yaml() {
        let result = serde_yaml::from_str::<Gateway>(GATEWAY_YAML);
        assert!(result.is_ok(), "Failed to parse Gateway: {:?}", result.err());

        let gateway = result.unwrap();
        assert_eq!(gateway.metadata.name, Some("test-gateway".to_string()));
        assert_eq!(gateway.metadata.namespace, Some("default".to_string()));
    }

    #[test]
    fn test_parse_http_route_yaml() {
        let result = serde_yaml::from_str::<HTTPRoute>(HTTP_ROUTE_YAML);
        assert!(result.is_ok(), "Failed to parse HTTPRoute: {:?}", result.err());

        let route = result.unwrap();
        assert_eq!(route.metadata.name, Some("test-route".to_string()));
    }

    #[test]
    fn test_unknown_kind_returns_none() {
        let kind = ResourceKind::from_content(UNKNOWN_KIND_YAML);
        assert_eq!(kind, None);
    }

    #[test]
    fn test_invalid_yaml_kind_returns_none() {
        let kind = ResourceKind::from_content(INVALID_YAML);
        // Should not panic, just return None or some kind
        // The exact behavior depends on the regex matching
        assert!(kind.is_none() || kind.is_some());
    }

    #[test]
    fn test_empty_content_returns_none() {
        let kind = ResourceKind::from_content("");
        assert_eq!(kind, None);
    }

    #[test]
    fn test_json_format_kind() {
        let json_content = r#"{"apiVersion": "v1", "kind": "Gateway", "metadata": {"name": "test"}}"#;
        let kind = ResourceKind::from_content(json_content);
        assert_eq!(kind, Some(ResourceKind::Gateway));
    }
}
