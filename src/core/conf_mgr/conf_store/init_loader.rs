//! Resource loader for initialization
//!
//! Unified single-pass loading that works for both file system and Kubernetes modes

use crate::core::conf_mgr::ConfStore;
use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::{CacheEventDispatch, ConfigServer};
use crate::types::prelude_resources::*;
use crate::types::ResourceKind;
use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::ResourceExt;
use std::sync::Arc;

/// Load all resources from storage into ConfigServer
/// Unified single-pass loading suitable for both file system and Kubernetes modes
pub async fn load_all_resources_from_store(store: Arc<dyn ConfStore>, config_server: Arc<ConfigServer>) -> Result<()> {
    tracing::info!(
        component = "conf_store",
        event = "init_load_start",
        "Loading all resources from store..."
    );

    // Load all resources from store
    let all_resources = store
        .list_all()
        .await
        .context("Failed to list all resources from store")?;

    let mut loaded_count = 0;
    let mut error_count = 0;

    // Single pass: process all resources uniformly
    for resource in all_resources {
        let kind = ResourceKind::from_content(&resource.content);

        match kind {
            Some(ResourceKind::GatewayClass) => match serde_yaml::from_str::<GatewayClass>(&resource.content) {
                Ok(gc) => {
                    config_server.gateway_classes.apply_change(ResourceChange::InitAdd, gc);
                    loaded_count += 1;
                }
                Err(e) => {
                    error_count += 1;
                    tracing::error!(
                        component = "conf_store",
                        name = %resource.name,
                        error = %e,
                        "Failed to parse GatewayClass"
                    );
                }
            },
            Some(ResourceKind::Gateway) => match serde_yaml::from_str::<Gateway>(&resource.content) {
                Ok(gateway) => {
                    config_server.gateways.apply_change(ResourceChange::InitAdd, gateway);
                    loaded_count += 1;
                }
                Err(e) => {
                    error_count += 1;
                    tracing::error!(
                        component = "conf_store",
                        name = %resource.name,
                        error = %e,
                        "Failed to parse Gateway"
                    );
                }
            },
            Some(ResourceKind::EdgionGatewayConfig) => {
                match serde_yaml::from_str::<EdgionGatewayConfig>(&resource.content) {
                    Ok(egwc) => {
                        config_server
                            .edgion_gateway_configs
                            .apply_change(ResourceChange::InitAdd, egwc);
                        loaded_count += 1;
                    }
                    Err(e) => {
                        error_count += 1;
                        tracing::error!(
                            component = "conf_store",
                            name = %resource.name,
                            error = %e,
                            "Failed to parse EdgionGatewayConfig"
                        );
                    }
                }
            }
            Some(ResourceKind::ReferenceGrant) => match serde_yaml::from_str::<ReferenceGrant>(&resource.content) {
                Ok(grant) => {
                    config_server
                        .reference_grants
                        .apply_change(ResourceChange::InitAdd, grant);
                    loaded_count += 1;
                }
                Err(e) => {
                    error_count += 1;
                    tracing::error!(
                        component = "conf_store",
                        name = %resource.name,
                        error = %e,
                        "Failed to parse ReferenceGrant"
                    );
                }
            },
            Some(ResourceKind::HTTPRoute) => {
                match serde_yaml::from_str::<HTTPRoute>(&resource.content) {
                    Ok(route) => {
                        // Validate using ref_grant module (checks enabled status internally)
                        let errors = crate::core::ref_grant::validate_http_route_if_enabled(&route);
                        for err in errors {
                            tracing::warn!(
                                component = "conf_store",
                                route = route.name_any(),
                                warning = %err,
                                "Cross-namespace validation warning"
                            );
                        }
                        config_server.routes.apply_change(ResourceChange::InitAdd, route);
                        loaded_count += 1;
                    }
                    Err(e) => {
                        error_count += 1;
                        tracing::error!(
                            component = "conf_store",
                            name = %resource.name,
                            error = %e,
                            "Failed to parse HTTPRoute"
                        );
                    }
                }
            }
            Some(ResourceKind::GRPCRoute) => match serde_yaml::from_str::<GRPCRoute>(&resource.content) {
                Ok(route) => {
                    let errors = crate::core::ref_grant::validate_grpc_route_if_enabled(&route);
                    for err in errors {
                        tracing::warn!(
                            component = "conf_store",
                            route = route.name_any(),
                            warning = %err,
                            "Cross-namespace validation warning"
                        );
                    }
                    config_server.grpc_routes.apply_change(ResourceChange::InitAdd, route);
                    loaded_count += 1;
                }
                Err(e) => {
                    error_count += 1;
                    tracing::error!(
                        component = "conf_store",
                        name = %resource.name,
                        error = %e,
                        "Failed to parse GRPCRoute"
                    );
                }
            },
            Some(ResourceKind::TCPRoute) => match serde_yaml::from_str::<TCPRoute>(&resource.content) {
                Ok(route) => {
                    let errors = crate::core::ref_grant::validate_tcp_route_if_enabled(&route);
                    for err in errors {
                        tracing::warn!(
                            component = "conf_store",
                            route = route.name_any(),
                            warning = %err,
                            "Cross-namespace validation warning"
                        );
                    }
                    config_server.tcp_routes.apply_change(ResourceChange::InitAdd, route);
                    loaded_count += 1;
                }
                Err(e) => {
                    error_count += 1;
                    tracing::error!(
                        component = "conf_store",
                        name = %resource.name,
                        error = %e,
                        "Failed to parse TCPRoute"
                    );
                }
            },
            Some(ResourceKind::UDPRoute) => match serde_yaml::from_str::<UDPRoute>(&resource.content) {
                Ok(route) => {
                    let errors = crate::core::ref_grant::validate_udp_route_if_enabled(&route);
                    for err in errors {
                        tracing::warn!(
                            component = "conf_store",
                            route = route.name_any(),
                            warning = %err,
                            "Cross-namespace validation warning"
                        );
                    }
                    config_server.udp_routes.apply_change(ResourceChange::InitAdd, route);
                    loaded_count += 1;
                }
                Err(e) => {
                    error_count += 1;
                    tracing::error!(
                        component = "conf_store",
                        name = %resource.name,
                        error = %e,
                        "Failed to parse UDPRoute"
                    );
                }
            },
            Some(ResourceKind::TLSRoute) => match serde_yaml::from_str::<TLSRoute>(&resource.content) {
                Ok(route) => {
                    let errors = crate::core::ref_grant::validate_tls_route_if_enabled(&route);
                    for err in errors {
                        tracing::warn!(
                            component = "conf_store",
                            route = route.name_any(),
                            warning = %err,
                            "Cross-namespace validation warning"
                        );
                    }
                    config_server.tls_routes.apply_change(ResourceChange::InitAdd, route);
                    loaded_count += 1;
                }
                Err(e) => {
                    error_count += 1;
                    tracing::error!(
                        component = "conf_store",
                        name = %resource.name,
                        error = %e,
                        "Failed to parse TLSRoute"
                    );
                }
            },
            Some(ResourceKind::Service) => match serde_yaml::from_str::<Service>(&resource.content) {
                Ok(svc) => {
                    config_server.services.apply_change(ResourceChange::InitAdd, svc);
                    loaded_count += 1;
                }
                Err(e) => {
                    error_count += 1;
                    tracing::error!(
                        component = "conf_store",
                        name = %resource.name,
                        error = %e,
                        "Failed to parse Service"
                    );
                }
            },
            Some(ResourceKind::Endpoint) => match serde_yaml::from_str::<Endpoints>(&resource.content) {
                Ok(ep) => {
                    config_server.endpoints.apply_change(ResourceChange::InitAdd, ep);
                    loaded_count += 1;
                }
                Err(e) => {
                    error_count += 1;
                    tracing::error!(
                        component = "conf_store",
                        name = %resource.name,
                        error = %e,
                        "Failed to parse Endpoints"
                    );
                }
            },
            Some(ResourceKind::EndpointSlice) => match serde_yaml::from_str::<EndpointSlice>(&resource.content) {
                Ok(eps) => {
                    config_server.endpoint_slices.apply_change(ResourceChange::InitAdd, eps);
                    loaded_count += 1;
                }
                Err(e) => {
                    error_count += 1;
                    tracing::error!(
                        component = "conf_store",
                        name = %resource.name,
                        error = %e,
                        "Failed to parse EndpointSlice"
                    );
                }
            },
            Some(ResourceKind::EdgionPlugins) => match serde_yaml::from_str::<EdgionPlugins>(&resource.content) {
                Ok(plugins) => {
                    config_server
                        .edgion_plugins
                        .apply_change(ResourceChange::InitAdd, plugins);
                    loaded_count += 1;
                }
                Err(e) => {
                    error_count += 1;
                    tracing::error!(
                        component = "conf_store",
                        name = %resource.name,
                        error = %e,
                        "Failed to parse EdgionPlugins"
                    );
                }
            },
            Some(ResourceKind::EdgionStreamPlugins) => {
                match serde_yaml::from_str::<EdgionStreamPlugins>(&resource.content) {
                    Ok(plugins) => {
                        config_server
                            .edgion_stream_plugins
                            .apply_change(ResourceChange::InitAdd, plugins);
                        loaded_count += 1;
                    }
                    Err(e) => {
                        error_count += 1;
                        tracing::error!(
                            component = "conf_store",
                            name = %resource.name,
                            error = %e,
                            "Failed to parse EdgionStreamPlugins"
                        );
                    }
                }
            }
            Some(ResourceKind::EdgionTls) => match serde_yaml::from_str::<EdgionTls>(&resource.content) {
                Ok(tls) => {
                    config_server.apply_edgion_tls_change(ResourceChange::InitAdd, tls);
                    loaded_count += 1;
                }
                Err(e) => {
                    error_count += 1;
                    tracing::error!(
                        component = "conf_store",
                        name = %resource.name,
                        error = %e,
                        "Failed to parse EdgionTls"
                    );
                }
            },
            Some(ResourceKind::BackendTLSPolicy) => match serde_yaml::from_str::<BackendTLSPolicy>(&resource.content) {
                Ok(policy) => {
                    config_server
                        .backend_tls_policies
                        .apply_change(ResourceChange::InitAdd, policy);
                    loaded_count += 1;
                }
                Err(e) => {
                    error_count += 1;
                    tracing::error!(
                        component = "conf_store",
                        name = %resource.name,
                        error = %e,
                        "Failed to parse BackendTLSPolicy"
                    );
                }
            },
            Some(ResourceKind::Secret) => match serde_yaml::from_str::<Secret>(&resource.content) {
                Ok(secret) => {
                    config_server.apply_secret_change(ResourceChange::InitAdd, secret);
                    loaded_count += 1;
                }
                Err(e) => {
                    error_count += 1;
                    tracing::error!(
                        component = "conf_store",
                        name = %resource.name,
                        error = %e,
                        "Failed to parse Secret"
                    );
                }
            },
            Some(ResourceKind::LinkSys) => match serde_yaml::from_str::<LinkSys>(&resource.content) {
                Ok(link_sys) => {
                    config_server.link_sys.apply_change(ResourceChange::InitAdd, link_sys);
                    loaded_count += 1;
                }
                Err(e) => {
                    error_count += 1;
                    tracing::error!(
                        component = "conf_store",
                        name = %resource.name,
                        error = %e,
                        "Failed to parse LinkSys"
                    );
                }
            },
            Some(ResourceKind::PluginMetaData) => match serde_yaml::from_str::<PluginMetaData>(&resource.content) {
                Ok(metadata) => {
                    config_server
                        .plugin_metadata
                        .apply_change(ResourceChange::InitAdd, metadata);
                    loaded_count += 1;
                }
                Err(e) => {
                    error_count += 1;
                    tracing::error!(
                        component = "conf_store",
                        name = %resource.name,
                        error = %e,
                        "Failed to parse PluginMetaData"
                    );
                }
            },
            Some(_) | None => {
                // Skip unknown or unsupported resource types
                tracing::debug!(
                    component = "conf_store",
                    kind = ?kind,
                    "Skipping unknown or unsupported resource type"
                );
            }
        }
    }

    tracing::info!(
        component = "conf_store",
        event = "init_load_complete",
        loaded = loaded_count,
        errors = error_count,
        "Resource initialization complete"
    );

    Ok(())
}
