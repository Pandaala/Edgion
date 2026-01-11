//! Resource loader for initialization
//!
//! Unified single-pass loading that works for both file system and Kubernetes modes

use crate::core::conf_mgr::ConfStore;
use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::{CacheEventDispatch, ConfigServer, ServerCache};
use crate::types::prelude_resources::*;
use crate::types::{ResourceKind, ResourceMeta};
use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::{Resource, ResourceExt};
use serde::de::DeserializeOwned;
use std::sync::Arc;

/// Statistics for resource loading
struct LoadStats {
    loaded: usize,
    errors: usize,
}

impl LoadStats {
    fn new() -> Self {
        Self { loaded: 0, errors: 0 }
    }

    fn success(&mut self) {
        self.loaded += 1;
    }

    fn error(&mut self) {
        self.errors += 1;
    }
}

/// Load a simple resource (parse + apply, no validation)
fn load_simple<T>(
    content: &str,
    resource_name: &str,
    kind_name: &str,
    cache: &ServerCache<T>,
    stats: &mut LoadStats,
) where
    T: DeserializeOwned + Clone + Send + Sync + 'static + ResourceMeta + Resource,
{
    match serde_yaml::from_str::<T>(content) {
        Ok(resource) => {
            cache.apply_change(ResourceChange::InitAdd, resource);
            stats.success();
        }
        Err(e) => {
            stats.error();
            tracing::error!(
                component = "conf_store",
                kind = kind_name,
                name = %resource_name,
                error = %e,
                "Failed to parse resource"
            );
        }
    }
}

/// Load a route resource with cross-namespace validation
fn load_route_with_validation<T, F>(
    content: &str,
    resource_name: &str,
    kind_name: &str,
    cache: &ServerCache<T>,
    stats: &mut LoadStats,
    validator: F,
) where
    T: DeserializeOwned + Clone + Send + Sync + 'static + ResourceMeta + Resource + ResourceExt,
    F: FnOnce(&T) -> Vec<String>,
{
    match serde_yaml::from_str::<T>(content) {
        Ok(route) => {
            // Run validation and log warnings
            for err in validator(&route) {
                tracing::warn!(
                    component = "conf_store",
                    kind = kind_name,
                    route = route.name_any(),
                    warning = %err,
                    "Cross-namespace validation warning"
                );
            }
            cache.apply_change(ResourceChange::InitAdd, route);
            stats.success();
        }
        Err(e) => {
            stats.error();
            tracing::error!(
                component = "conf_store",
                kind = kind_name,
                name = %resource_name,
                error = %e,
                "Failed to parse resource"
            );
        }
    }
}

/// Load all resources from storage into ConfigServer
/// Unified single-pass loading suitable for both file system and Kubernetes modes
pub async fn load_all_resources_from_store(store: Arc<dyn ConfStore>, config_server: Arc<ConfigServer>) -> Result<()> {
    tracing::info!(
        component = "conf_store",
        event = "init_load_start",
        "Loading all resources from store..."
    );

    let all_resources = store
        .list_all()
        .await
        .context("Failed to list all resources from store")?;

    let mut stats = LoadStats::new();

    for resource in all_resources {
        let kind = ResourceKind::from_content(&resource.content);
        let name = &resource.name;
        let content = &resource.content;

        match kind {
            // === Base Configuration Resources ===
            Some(ResourceKind::GatewayClass) => {
                load_simple::<GatewayClass>(content, name, "GatewayClass", &config_server.gateway_classes, &mut stats);
            }
            Some(ResourceKind::Gateway) => {
                load_simple::<Gateway>(content, name, "Gateway", &config_server.gateways, &mut stats);
            }
            Some(ResourceKind::EdgionGatewayConfig) => {
                load_simple::<EdgionGatewayConfig>(content, name, "EdgionGatewayConfig", &config_server.edgion_gateway_configs, &mut stats);
            }

            // === Route Resources (with validation) ===
            Some(ResourceKind::HTTPRoute) => {
                load_route_with_validation::<HTTPRoute, _>(
                    content, name, "HTTPRoute", &config_server.routes, &mut stats,
                    |r| crate::core::ref_grant::validate_http_route_if_enabled(r),
                );
            }
            Some(ResourceKind::GRPCRoute) => {
                load_route_with_validation::<GRPCRoute, _>(
                    content, name, "GRPCRoute", &config_server.grpc_routes, &mut stats,
                    |r| crate::core::ref_grant::validate_grpc_route_if_enabled(r),
                );
            }
            Some(ResourceKind::TCPRoute) => {
                load_route_with_validation::<TCPRoute, _>(
                    content, name, "TCPRoute", &config_server.tcp_routes, &mut stats,
                    |r| crate::core::ref_grant::validate_tcp_route_if_enabled(r),
                );
            }
            Some(ResourceKind::UDPRoute) => {
                load_route_with_validation::<UDPRoute, _>(
                    content, name, "UDPRoute", &config_server.udp_routes, &mut stats,
                    |r| crate::core::ref_grant::validate_udp_route_if_enabled(r),
                );
            }
            Some(ResourceKind::TLSRoute) => {
                load_route_with_validation::<TLSRoute, _>(
                    content, name, "TLSRoute", &config_server.tls_routes, &mut stats,
                    |r| crate::core::ref_grant::validate_tls_route_if_enabled(r),
                );
            }

            // === Backend Resources ===
            Some(ResourceKind::Service) => {
                load_simple::<Service>(content, name, "Service", &config_server.services, &mut stats);
            }
            Some(ResourceKind::Endpoint) => {
                load_simple::<Endpoints>(content, name, "Endpoints", &config_server.endpoints, &mut stats);
            }
            Some(ResourceKind::EndpointSlice) => {
                load_simple::<EndpointSlice>(content, name, "EndpointSlice", &config_server.endpoint_slices, &mut stats);
            }

            // === Security and Policy Resources ===
            Some(ResourceKind::ReferenceGrant) => {
                load_simple::<ReferenceGrant>(content, name, "ReferenceGrant", &config_server.reference_grants, &mut stats);
            }
            Some(ResourceKind::BackendTLSPolicy) => {
                load_simple::<BackendTLSPolicy>(content, name, "BackendTLSPolicy", &config_server.backend_tls_policies, &mut stats);
            }
            Some(ResourceKind::EdgionTls) => {
                // EdgionTls has special handling for secret refs
                match serde_yaml::from_str::<EdgionTls>(content) {
                    Ok(tls) => {
                        config_server.apply_edgion_tls_change(ResourceChange::InitAdd, tls);
                        stats.success();
                    }
                    Err(e) => {
                        stats.error();
                        tracing::error!(component = "conf_store", kind = "EdgionTls", name = %name, error = %e, "Failed to parse resource");
                    }
                }
            }
            Some(ResourceKind::Secret) => {
                // Secret has special handling for cascading updates
                match serde_yaml::from_str::<Secret>(content) {
                    Ok(secret) => {
                        config_server.apply_secret_change(ResourceChange::InitAdd, secret);
                        stats.success();
                    }
                    Err(e) => {
                        stats.error();
                        tracing::error!(component = "conf_store", kind = "Secret", name = %name, error = %e, "Failed to parse resource");
                    }
                }
            }

            // === Plugin and Extension Resources ===
            Some(ResourceKind::EdgionPlugins) => {
                load_simple::<EdgionPlugins>(content, name, "EdgionPlugins", &config_server.edgion_plugins, &mut stats);
            }
            Some(ResourceKind::EdgionStreamPlugins) => {
                load_simple::<EdgionStreamPlugins>(content, name, "EdgionStreamPlugins", &config_server.edgion_stream_plugins, &mut stats);
            }
            Some(ResourceKind::PluginMetaData) => {
                load_simple::<PluginMetaData>(content, name, "PluginMetaData", &config_server.plugin_metadata, &mut stats);
            }

            // === Infrastructure Resources ===
            Some(ResourceKind::LinkSys) => {
                load_simple::<LinkSys>(content, name, "LinkSys", &config_server.link_sys, &mut stats);
            }

            // === Unknown ===
            Some(_) | None => {
                tracing::debug!(component = "conf_store", kind = ?kind, "Skipping unknown resource type");
            }
        }
    }

    tracing::info!(
        component = "conf_store",
        event = "init_load_complete",
        loaded = stats.loaded,
        errors = stats.errors,
        "Resource initialization complete"
    );

    Ok(())
}
