use crate::types::ResourceKind;
use axum::http::StatusCode;

/// Helper function to validate a resource against its schema
pub fn validate_resource<T: serde::Serialize>(
    validator: &crate::core::conf_mgr::SchemaValidator,
    kind: ResourceKind,
    resource: &T,
) -> Result<(), StatusCode> {
    let json_value = serde_json::to_value(resource).map_err(|e| {
        tracing::warn!(
            component = "unified_api",
            error = %e,
            "Failed to convert resource to JSON for validation"
        );
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    validator.validate(kind, &json_value).map_err(|e| {
        tracing::warn!(
            component = "unified_api",
            kind = ?kind,
            error = %e,
            "Schema validation failed"
        );
        StatusCode::BAD_REQUEST
    })?;

    Ok(())
}

/// Parse request body as either JSON or YAML
/// Tries JSON first, falls back to YAML if JSON parsing fails
pub fn parse_resource<T>(body: &str) -> Result<T, StatusCode>
where
    T: serde::de::DeserializeOwned,
{
    // Try JSON first (more common in API calls)
    if let Ok(resource) = serde_json::from_str::<T>(body) {
        return Ok(resource);
    }

    // Fall back to YAML
    serde_yaml::from_str::<T>(body).map_err(|e| {
        tracing::warn!(
            component = "unified_api",
            error = %e,
            "Failed to parse request body as JSON or YAML"
        );
        StatusCode::BAD_REQUEST
    })
}

/// Parse ResourceKind from string (case-insensitive)
pub fn parse_kind(kind_str: &str) -> Result<ResourceKind, String> {
    ResourceKind::from_kind_name(kind_str).ok_or_else(|| format!("Unknown resource kind: {}", kind_str))
}

/// Determine if a resource kind is cluster-scoped
pub fn is_cluster_scoped(kind: &ResourceKind) -> bool {
    matches!(kind, ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig)
}

/// Update resource_version for a resource in non-k8s mode
pub fn update_resource_version<T>(resource: &mut T)
where
    T: kube::ResourceExt,
{
    let version = crate::core::utils::next_resource_version();
    resource.meta_mut().resource_version = Some(version.to_string());
}

/// Parse resource and optionally update resource_version
///
/// If `update_version` is true, automatically updates the resource_version
/// by calling next_resource_version(). This should be true in non-k8s mode
/// for create/update operations.
pub fn parse_resource_and_update_version<T>(body: &str, update_version: bool) -> Result<T, StatusCode>
where
    T: serde::de::DeserializeOwned + kube::ResourceExt,
{
    let mut resource = parse_resource(body)?;

    if update_version {
        update_resource_version(&mut resource);
    }

    Ok(resource)
}

/// Helper macro to convert list data to JSON Value Vec
#[macro_export]
macro_rules! list_to_json {
    ($list_data:expr) => {{
        $list_data
            .into_iter()
            .map(|item| serde_json::to_value(item).unwrap_or(serde_json::Value::Null))
            .collect::<Vec<_>>()
    }};
}

/// Helper macro to list all resources of a kind from ConfigServer
#[macro_export]
macro_rules! list_all_resources {
    ($server:expr, $kind:expr) => {{
        use $crate::types::ResourceKind;
        match $kind {
            ResourceKind::HTTPRoute => list_to_json!($server.routes.list().data),
            ResourceKind::GRPCRoute => list_to_json!($server.grpc_routes.list().data),
            ResourceKind::TCPRoute => list_to_json!($server.tcp_routes.list().data),
            ResourceKind::UDPRoute => list_to_json!($server.udp_routes.list().data),
            ResourceKind::TLSRoute => list_to_json!($server.tls_routes.list().data),
            ResourceKind::Service => list_to_json!($server.services.list().data),
            ResourceKind::EndpointSlice => list_to_json!($server.endpoint_slices.list().data),
            ResourceKind::Endpoint => list_to_json!($server.endpoints.list().data),
            ResourceKind::EdgionTls => list_to_json!($server.edgion_tls.list().data),
            ResourceKind::EdgionPlugins => list_to_json!($server.edgion_plugins.list().data),
            ResourceKind::EdgionStreamPlugins => list_to_json!($server.edgion_stream_plugins.list().data),
            ResourceKind::ReferenceGrant => list_to_json!($server.reference_grants.list().data),
            ResourceKind::BackendTLSPolicy => list_to_json!($server.backend_tls_policies.list().data),
            ResourceKind::PluginMetaData => list_to_json!($server.plugin_metadata.list().data),
            ResourceKind::LinkSys => list_to_json!($server.link_sys.list().data),
            ResourceKind::Secret => list_to_json!($server.secrets.list().data),
            ResourceKind::GatewayClass => list_to_json!($server.gateway_classes.list().data),
            ResourceKind::Gateway => list_to_json!($server.gateways.list().data),
            ResourceKind::EdgionGatewayConfig => list_to_json!($server.edgion_gateway_configs.list().data),
            ResourceKind::Unspecified => vec![],
        }
    }};
}

/// Helper macro to list namespaced resources with namespace filter
#[macro_export]
macro_rules! list_namespaced_resources {
    ($server:expr, $kind:expr, $ns:expr) => {{
        use $crate::types::ResourceKind;
        use kube::ResourceExt;
        let ns_str = $ns.as_str();
        match $kind {
            ResourceKind::HTTPRoute => {
                let filtered: Vec<_> = $server
                    .routes
                    .list()
                    .data
                    .into_iter()
                    .filter(|r| r.namespace().as_deref() == Some(ns_str))
                    .collect();
                list_to_json!(filtered)
            }
            ResourceKind::GRPCRoute => {
                let filtered: Vec<_> = $server
                    .grpc_routes
                    .list()
                    .data
                    .into_iter()
                    .filter(|r| r.namespace().as_deref() == Some(ns_str))
                    .collect();
                list_to_json!(filtered)
            }
            ResourceKind::TCPRoute => {
                let filtered: Vec<_> = $server
                    .tcp_routes
                    .list()
                    .data
                    .into_iter()
                    .filter(|r| r.namespace().as_deref() == Some(ns_str))
                    .collect();
                list_to_json!(filtered)
            }
            ResourceKind::UDPRoute => {
                let filtered: Vec<_> = $server
                    .udp_routes
                    .list()
                    .data
                    .into_iter()
                    .filter(|r| r.namespace().as_deref() == Some(ns_str))
                    .collect();
                list_to_json!(filtered)
            }
            ResourceKind::TLSRoute => {
                let filtered: Vec<_> = $server
                    .tls_routes
                    .list()
                    .data
                    .into_iter()
                    .filter(|r| r.namespace().as_deref() == Some(ns_str))
                    .collect();
                list_to_json!(filtered)
            }
            ResourceKind::Service => {
                let filtered: Vec<_> = $server
                    .services
                    .list()
                    .data
                    .into_iter()
                    .filter(|r| r.namespace().as_deref() == Some(ns_str))
                    .collect();
                list_to_json!(filtered)
            }
            ResourceKind::EndpointSlice => {
                let filtered: Vec<_> = $server
                    .endpoint_slices
                    .list()
                    .data
                    .into_iter()
                    .filter(|r| r.namespace().as_deref() == Some(ns_str))
                    .collect();
                list_to_json!(filtered)
            }
            ResourceKind::Endpoint => {
                let filtered: Vec<_> = $server
                    .endpoints
                    .list()
                    .data
                    .into_iter()
                    .filter(|r| r.namespace().as_deref() == Some(ns_str))
                    .collect();
                list_to_json!(filtered)
            }
            ResourceKind::EdgionTls => {
                let filtered: Vec<_> = $server
                    .edgion_tls
                    .list()
                    .data
                    .into_iter()
                    .filter(|r| r.namespace().as_deref() == Some(ns_str))
                    .collect();
                list_to_json!(filtered)
            }
            ResourceKind::EdgionPlugins => {
                let filtered: Vec<_> = $server
                    .edgion_plugins
                    .list()
                    .data
                    .into_iter()
                    .filter(|r| r.namespace() == Some(ns_str))
                    .collect();
                list_to_json!(filtered)
            }
            ResourceKind::EdgionStreamPlugins => {
                let filtered: Vec<_> = $server
                    .edgion_stream_plugins
                    .list()
                    .data
                    .into_iter()
                    .filter(|r| r.namespace() == Some(ns_str))
                    .collect();
                list_to_json!(filtered)
            }
            ResourceKind::ReferenceGrant => {
                let filtered: Vec<_> = $server
                    .reference_grants
                    .list()
                    .data
                    .into_iter()
                    .filter(|r| r.namespace() == Some(ns_str))
                    .collect();
                list_to_json!(filtered)
            }
            ResourceKind::BackendTLSPolicy => {
                let filtered: Vec<_> = $server
                    .backend_tls_policies
                    .list()
                    .data
                    .into_iter()
                    .filter(|r| r.namespace() == Some(ns_str))
                    .collect();
                list_to_json!(filtered)
            }
            ResourceKind::PluginMetaData => {
                let filtered: Vec<_> = $server
                    .plugin_metadata
                    .list()
                    .data
                    .into_iter()
                    .filter(|r| r.namespace().as_deref() == Some(ns_str))
                    .collect();
                list_to_json!(filtered)
            }
            ResourceKind::LinkSys => {
                let filtered: Vec<_> = $server
                    .link_sys
                    .list()
                    .data
                    .into_iter()
                    .filter(|r| r.namespace().as_deref() == Some(ns_str))
                    .collect();
                list_to_json!(filtered)
            }
            ResourceKind::Secret => {
                let filtered: Vec<_> = $server
                    .secrets
                    .list()
                    .data
                    .into_iter()
                    .filter(|r| r.namespace().as_deref() == Some(ns_str))
                    .collect();
                list_to_json!(filtered)
            }
            _ => vec![],
        }
    }};
}

/// Helper macro to get a single namespaced resource
#[macro_export]
macro_rules! get_namespaced_resource {
    ($server:expr, $kind:expr, $ns:expr, $name:expr) => {{
        use $crate::types::ResourceKind;
        use kube::ResourceExt;
        let ns_str = $ns.as_str();
        let name_str = $name.as_str();
        match $kind {
            ResourceKind::HTTPRoute => $server
                .routes
                .list()
                .data
                .into_iter()
                .find(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str))
                .and_then(|r| serde_json::to_value(r).ok()),
            ResourceKind::GRPCRoute => $server
                .grpc_routes
                .list()
                .data
                .into_iter()
                .find(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str))
                .and_then(|r| serde_json::to_value(r).ok()),
            ResourceKind::TCPRoute => $server
                .tcp_routes
                .list()
                .data
                .into_iter()
                .find(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str))
                .and_then(|r| serde_json::to_value(r).ok()),
            ResourceKind::UDPRoute => $server
                .udp_routes
                .list()
                .data
                .into_iter()
                .find(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str))
                .and_then(|r| serde_json::to_value(r).ok()),
            ResourceKind::TLSRoute => $server
                .tls_routes
                .list()
                .data
                .into_iter()
                .find(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str))
                .and_then(|r| serde_json::to_value(r).ok()),
            ResourceKind::Service => $server
                .services
                .list()
                .data
                .into_iter()
                .find(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str))
                .and_then(|r| serde_json::to_value(r).ok()),
            ResourceKind::EndpointSlice => $server
                .endpoint_slices
                .list()
                .data
                .into_iter()
                .find(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str))
                .and_then(|r| serde_json::to_value(r).ok()),
            ResourceKind::Endpoint => $server
                .endpoints
                .list()
                .data
                .into_iter()
                .find(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str))
                .and_then(|r| serde_json::to_value(r).ok()),
            ResourceKind::EdgionTls => $server
                .edgion_tls
                .list()
                .data
                .into_iter()
                .find(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str))
                .and_then(|r| serde_json::to_value(r).ok()),
            ResourceKind::EdgionPlugins => $server
                .edgion_plugins
                .list()
                .data
                .into_iter()
                .find(|r| r.name_any() == name_str && r.namespace() == Some(ns_str))
                .and_then(|r| serde_json::to_value(r).ok()),
            ResourceKind::EdgionStreamPlugins => $server
                .edgion_stream_plugins
                .list()
                .data
                .into_iter()
                .find(|r| r.name_any() == name_str && r.namespace() == Some(ns_str))
                .and_then(|r| serde_json::to_value(r).ok()),
            ResourceKind::ReferenceGrant => $server
                .reference_grants
                .list()
                .data
                .into_iter()
                .find(|r| r.name_any() == name_str && r.namespace() == Some(ns_str))
                .and_then(|r| serde_json::to_value(r).ok()),
            ResourceKind::BackendTLSPolicy => $server
                .backend_tls_policies
                .list()
                .data
                .into_iter()
                .find(|r| r.name_any() == name_str && r.namespace() == Some(ns_str))
                .and_then(|r| serde_json::to_value(r).ok()),
            ResourceKind::PluginMetaData => $server
                .plugin_metadata
                .list()
                .data
                .into_iter()
                .find(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str))
                .and_then(|r| serde_json::to_value(r).ok()),
            ResourceKind::LinkSys => $server
                .link_sys
                .list()
                .data
                .into_iter()
                .find(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str))
                .and_then(|r| serde_json::to_value(r).ok()),
            ResourceKind::Secret => $server
                .secrets
                .list()
                .data
                .into_iter()
                .find(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str))
                .and_then(|r| serde_json::to_value(r).ok()),
            _ => None,
        }
    }};
}

/// Helper macro to check if a namespaced resource exists
#[macro_export]
macro_rules! resource_exists_namespaced {
    ($server:expr, $kind:expr, $ns:expr, $name:expr) => {{
        use $crate::types::ResourceKind;
        use kube::ResourceExt;
        let ns_str = $ns.as_str();
        let name_str = $name.as_str();
        match $kind {
            ResourceKind::HTTPRoute => $server
                .routes
                .list()
                .data
                .iter()
                .any(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str)),
            ResourceKind::GRPCRoute => $server
                .grpc_routes
                .list()
                .data
                .iter()
                .any(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str)),
            ResourceKind::TCPRoute => $server
                .tcp_routes
                .list()
                .data
                .iter()
                .any(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str)),
            ResourceKind::UDPRoute => $server
                .udp_routes
                .list()
                .data
                .iter()
                .any(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str)),
            ResourceKind::TLSRoute => $server
                .tls_routes
                .list()
                .data
                .iter()
                .any(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str)),
            ResourceKind::Service => $server
                .services
                .list()
                .data
                .iter()
                .any(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str)),
            ResourceKind::EndpointSlice => $server
                .endpoint_slices
                .list()
                .data
                .iter()
                .any(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str)),
            ResourceKind::Endpoint => $server
                .endpoints
                .list()
                .data
                .iter()
                .any(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str)),
            ResourceKind::EdgionTls => $server
                .edgion_tls
                .list()
                .data
                .iter()
                .any(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str)),
            ResourceKind::EdgionPlugins => $server
                .edgion_plugins
                .list()
                .data
                .iter()
                .any(|r| r.name_any() == name_str && r.namespace() == Some(ns_str)),
            ResourceKind::EdgionStreamPlugins => $server
                .edgion_stream_plugins
                .list()
                .data
                .iter()
                .any(|r| r.name_any() == name_str && r.namespace() == Some(ns_str)),
            ResourceKind::ReferenceGrant => $server
                .reference_grants
                .list()
                .data
                .iter()
                .any(|r| r.name_any() == name_str && r.namespace() == Some(ns_str)),
            ResourceKind::BackendTLSPolicy => $server
                .backend_tls_policies
                .list()
                .data
                .iter()
                .any(|r| r.name_any() == name_str && r.namespace() == Some(ns_str)),
            ResourceKind::PluginMetaData => $server
                .plugin_metadata
                .list()
                .data
                .iter()
                .any(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str)),
            ResourceKind::LinkSys => $server
                .link_sys
                .list()
                .data
                .iter()
                .any(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str)),
            ResourceKind::Secret => $server
                .secrets
                .list()
                .data
                .iter()
                .any(|r| r.name_any() == name_str && r.namespace().as_deref() == Some(ns_str)),
            _ => false,
        }
    }};
}
