//! Resource loader for initialization
//!
//! Unified single-pass loading that works for both file system and Kubernetes modes

use anyhow::{Context, Result};
use std::sync::Arc;
use crate::core::conf_sync::{ConfigServer, CacheEventDispatch};
use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_mgr::ConfStore;
use crate::types::ResourceKind;
use crate::types::prelude_resources::*;
use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::ResourceExt;

/// Load all resources from storage into ConfigServer
/// Unified single-pass loading suitable for both file system and Kubernetes modes
pub async fn load_all_resources_from_store(
    store: Arc<dyn ConfStore>,
    config_server: Arc<ConfigServer>,
) -> Result<()> {
    tracing::info!(
        component = "conf_store",
        event = "init_load_start",
        "Loading all resources from store (unified single-pass)..."
    );
    
    // Check if ReferenceGrant validation is enabled
    let enable_validation = {
        let base_conf = config_server.base_conf.read().unwrap();
        base_conf.edgion_gateway_config().spec.enable_reference_grant_validation
    };
    
    let validator = if enable_validation {
        Some(crate::core::ref_grant::CrossNamespaceValidator::new())
    } else {
        None
    };
    
    tracing::info!(
        component = "conf_store",
        enable_reference_grant_validation = enable_validation,
        "ReferenceGrant validation status"
    );
    
    // Load all resources from store
    let all_resources = store
        .list_all()
        .await
        .context("Failed to list all resources from store")?;
    
    let mut loaded_count = 0;
    let mut error_count = 0;
    
    // Single pass: process all resources
    for resource in all_resources {
        let kind = ResourceKind::from_content(&resource.content);
        
        match kind {
            Some(ResourceKind::ReferenceGrant) => {
                // ReferenceGrant: load through ConfHandler (triggers events & updates global store)
                if enable_validation {
                    match serde_yaml::from_str::<ReferenceGrant>(&resource.content) {
                        Ok(grant) => {
                            config_server.reference_grants.apply_change(
                                ResourceChange::InitAdd,
                                grant
                            );
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
                    }
                }
            }
            Some(ResourceKind::HTTPRoute) => {
                match serde_yaml::from_str::<HTTPRoute>(&resource.content) {
                    Ok(route) => {
                        // Validate if validator is present
                        if let Some(ref validator) = validator {
                            let errors = validate_http_route(validator, &route);
                            for err in errors {
                                tracing::warn!(
                                    component = "conf_store",
                                    route = route.name_any(),
                                    warning = %err,
                                    "Cross-namespace validation warning"
                                );
                            }
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
            Some(ResourceKind::GRPCRoute) => {
                match serde_yaml::from_str::<GRPCRoute>(&resource.content) {
                    Ok(route) => {
                        if let Some(ref validator) = validator {
                            let errors = validate_grpc_route(validator, &route);
                            for err in errors {
                                tracing::warn!(
                                    component = "conf_store",
                                    route = route.name_any(),
                                    warning = %err,
                                    "Cross-namespace validation warning"
                                );
                            }
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
                }
            }
            Some(ResourceKind::TCPRoute) => {
                match serde_yaml::from_str::<TCPRoute>(&resource.content) {
                    Ok(route) => {
                        if let Some(ref validator) = validator {
                            let errors = validate_tcp_route(validator, &route);
                            for err in errors {
                                tracing::warn!(
                                    component = "conf_store",
                                    route = route.name_any(),
                                    warning = %err,
                                    "Cross-namespace validation warning"
                                );
                            }
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
                }
            }
            Some(ResourceKind::UDPRoute) => {
                match serde_yaml::from_str::<UDPRoute>(&resource.content) {
                    Ok(route) => {
                        if let Some(ref validator) = validator {
                            let errors = validate_udp_route(validator, &route);
                            for err in errors {
                                tracing::warn!(
                                    component = "conf_store",
                                    route = route.name_any(),
                                    warning = %err,
                                    "Cross-namespace validation warning"
                                );
                            }
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
                }
            }
            Some(ResourceKind::TLSRoute) => {
                match serde_yaml::from_str::<TLSRoute>(&resource.content) {
                    Ok(route) => {
                        if let Some(ref validator) = validator {
                            let errors = validate_tls_route(validator, &route);
                            for err in errors {
                                tracing::warn!(
                                    component = "conf_store",
                                    route = route.name_any(),
                                    warning = %err,
                                    "Cross-namespace validation warning"
                                );
                            }
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
                }
            }
            Some(ResourceKind::Service) => {
                match serde_yaml::from_str::<Service>(&resource.content) {
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
                }
            }
            Some(ResourceKind::Endpoint) => {
                match serde_yaml::from_str::<Endpoints>(&resource.content) {
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
                }
            }
            Some(ResourceKind::EndpointSlice) => {
                match serde_yaml::from_str::<EndpointSlice>(&resource.content) {
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
                }
            }
            Some(ResourceKind::EdgionPlugins) => {
                match serde_yaml::from_str::<EdgionPlugins>(&resource.content) {
                    Ok(plugins) => {
                        config_server.edgion_plugins.apply_change(ResourceChange::InitAdd, plugins);
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
                }
            }
            Some(ResourceKind::EdgionStreamPlugins) => {
                match serde_yaml::from_str::<EdgionStreamPlugins>(&resource.content) {
                    Ok(plugins) => {
                        config_server.edgion_stream_plugins.apply_change(ResourceChange::InitAdd, plugins);
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
            Some(ResourceKind::EdgionTls) => {
                match serde_yaml::from_str::<EdgionTls>(&resource.content) {
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
                }
            }
            Some(ResourceKind::BackendTLSPolicy) => {
                match serde_yaml::from_str::<BackendTLSPolicy>(&resource.content) {
                    Ok(policy) => {
                        config_server.backend_tls_policies.apply_change(ResourceChange::InitAdd, policy);
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
                }
            }
            Some(ResourceKind::Secret) => {
                match serde_yaml::from_str::<Secret>(&resource.content) {
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
                }
            }
            Some(ResourceKind::LinkSys) => {
                match serde_yaml::from_str::<LinkSys>(&resource.content) {
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
                }
            }
            Some(ResourceKind::PluginMetaData) => {
                match serde_yaml::from_str::<PluginMetaData>(&resource.content) {
                    Ok(metadata) => {
                        config_server.plugin_metadata.apply_change(ResourceChange::InitAdd, metadata);
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
                }
            }
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

// Helper validation functions
// Note: Validation logic simplified for K8s controller initial implementation
// Full cross-namespace validation will be implemented in future updates

fn validate_http_route(
    _validator: &crate::core::ref_grant::CrossNamespaceValidator,
    _route: &HTTPRoute,
) -> Vec<String> {
    // TODO: Implement full cross-namespace validation
    Vec::new()
}

fn validate_grpc_route(
    _validator: &crate::core::ref_grant::CrossNamespaceValidator,
    _route: &GRPCRoute,
) -> Vec<String> {
    // TODO: Implement full cross-namespace validation
    Vec::new()
}

fn validate_tcp_route(
    _validator: &crate::core::ref_grant::CrossNamespaceValidator,
    _route: &TCPRoute,
) -> Vec<String> {
    // TODO: Implement full cross-namespace validation
    Vec::new()
}

fn validate_udp_route(
    _validator: &crate::core::ref_grant::CrossNamespaceValidator,
    _route: &UDPRoute,
) -> Vec<String> {
    // TODO: Implement full cross-namespace validation
    Vec::new()
}

fn validate_tls_route(
    _validator: &crate::core::ref_grant::CrossNamespaceValidator,
    _route: &TLSRoute,
) -> Vec<String> {
    // TODO: Implement full cross-namespace validation
    Vec::new()
}
