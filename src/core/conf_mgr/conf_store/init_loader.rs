use anyhow::{Context, Result};
use std::sync::Arc;
use std::collections::HashMap;
use crate::core::conf_sync::{ConfigServer, CacheEventDispatch};
use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_mgr::ConfStore;
use crate::types::ResourceKind;
use crate::types::prelude_resources::*;
use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::ResourceExt;

/// Load all resources from storage into ConfigServer
/// Two-phase loading: ReferenceGrants first (if validation enabled), then validate and load other resources
pub async fn load_all_resources_from_store(
    store: Arc<dyn ConfStore>,
    config_server: Arc<ConfigServer>,
) -> Result<()> {
    tracing::info!(
        component = "conf_store",
        event = "init_load_start",
        "Loading all resources from store (two-phase)..."
    );
    
    // Check if ReferenceGrant validation is enabled
    let enable_validation = {
        let base_conf = config_server.base_conf.read().unwrap();
        base_conf.edgion_gateway_config().spec.enable_reference_grant_validation
    };
    
    // Phase 1: Load ReferenceGrants first (only if validation is enabled)
    if enable_validation {
        load_reference_grants_first(store.clone()).await?;
    } else {
        tracing::info!(
            component = "conf_store",
            "Skipping ReferenceGrant loading (validation disabled)"
        );
    }
    
    // Phase 2: Load and validate other resources
    load_and_validate_resources(store, config_server).await?;
    
    tracing::info!(
        component = "conf_store",
        event = "init_load_complete",
        "Resource initialization complete"
    );
    
    Ok(())
}

/// Phase 1: Load all ReferenceGrants directly into global store
async fn load_reference_grants_first(store: Arc<dyn ConfStore>) -> Result<()> {
    tracing::info!(
        component = "conf_store",
        event = "load_ref_grants_start",
        "Loading ReferenceGrants..."
    );
    
    let all_resources = store
        .list_all()
        .await
        .context("Failed to list all resources from store")?;
    
    let ref_grant_store = crate::core::ref_grant::get_global_reference_grant_store();
    let mut grants = HashMap::new();
    let mut error_count = 0;
    
    for resource in all_resources {
        if let Some(ResourceKind::ReferenceGrant) = ResourceKind::from_content(&resource.content) {
            match serde_yaml::from_str::<ReferenceGrant>(&resource.content) {
                Ok(grant) => {
                    let key = format!("{}/{}", 
                        grant.namespace().unwrap_or("default"), 
                        grant.name()
                    );
                    tracing::debug!(
                        component = "conf_store",
                        key = %key,
                        "Loaded ReferenceGrant"
                    );
                    grants.insert(key, Arc::new(grant));
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
    
    // Build index atomically
    ref_grant_store.replace_all(grants.clone());
    
    tracing::info!(
        component = "conf_store",
        event = "load_ref_grants_complete",
        loaded = grants.len(),
        errors = error_count,
        "ReferenceGrants loaded and indexed"
    );
    
    Ok(())
}

/// Phase 2: Load and validate other resources
async fn load_and_validate_resources(
    store: Arc<dyn ConfStore>,
    config_server: Arc<ConfigServer>,
) -> Result<()> {
    tracing::info!(
        component = "conf_store",
        event = "load_resources_start",
        "Loading and validating other resources..."
    );
    
    let all_resources = store
        .list_all()
        .await
        .context("Failed to list all resources from store")?;
    
    // Check if ReferenceGrant validation is enabled
    let enable_validation = {
        let base_conf = config_server.base_conf.read().unwrap();
        let enabled = base_conf.edgion_gateway_config().spec.enable_reference_grant_validation;
        tracing::info!(
            component = "conf_store",
            enable_reference_grant_validation = enabled,
            "ReferenceGrant validation status"
        );
        enabled
    };
    
    // Create validator for cross-namespace references (only if enabled)
    let validator = if enable_validation {
        Some(crate::core::ref_grant::CrossNamespaceValidator::new())
    } else {
        None
    };
    
    let mut loaded_count = 0;
    let mut error_count = 0;
    
    for resource in all_resources {
        let kind_str = resource.kind.clone();
        let name_str = resource.name.clone();
        let namespace_str = resource.namespace.clone();
        
        // Parse resource kind from content
        let kind = ResourceKind::from_content(&resource.content);
        
        let load_result = match kind {
            Some(ResourceKind::HTTPRoute) => {
                match serde_yaml::from_str::<HTTPRoute>(&resource.content) {
                    Ok(route) => {
                        // Validate cross-namespace references (only if enabled)
                        if let Some(ref validator) = validator {
                            let errors = validate_http_route(validator, &route);
                            if !errors.is_empty() {
                                for err in &errors {
                                    tracing::warn!(
                                        component = "conf_store",
                                        resource = "HTTPRoute",
                                        namespace = ?route.namespace(),
                                        name = ?route.name_any(),
                                        error = %err,
                                        "Cross-namespace reference validation failed"
                                    );
                                }
                            }
                        }
                        config_server.routes.apply_change(ResourceChange::InitAdd, route);
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Some(ResourceKind::GRPCRoute) => {
                match serde_yaml::from_str::<GRPCRoute>(&resource.content) {
                    Ok(route) => {
                        // Validate cross-namespace references (only if enabled)
                        if let Some(ref validator) = validator {
                            let errors = validate_grpc_route(validator, &route);
                            if !errors.is_empty() {
                                for err in &errors {
                                    tracing::warn!(
                                        component = "conf_store",
                                        resource = "GRPCRoute",
                                        namespace = ?route.namespace(),
                                        name = ?route.name_any(),
                                        error = %err,
                                        "Cross-namespace reference validation failed"
                                    );
                                }
                            }
                        }
                        config_server.grpc_routes.apply_change(ResourceChange::InitAdd, route);
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Some(ResourceKind::TCPRoute) => {
                match serde_yaml::from_str::<TCPRoute>(&resource.content) {
                    Ok(route) => {
                        // Validate cross-namespace references (only if enabled)
                        if let Some(ref validator) = validator {
                            let errors = validate_tcp_route(validator, &route);
                            if !errors.is_empty() {
                                for err in &errors {
                                    tracing::warn!(
                                        component = "conf_store",
                                        resource = "TCPRoute",
                                        namespace = ?route.namespace(),
                                        name = ?route.name_any(),
                                        error = %err,
                                        "Cross-namespace reference validation failed"
                                    );
                                }
                            }
                        }
                        config_server.tcp_routes.apply_change(ResourceChange::InitAdd, route);
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Some(ResourceKind::UDPRoute) => {
                match serde_yaml::from_str::<UDPRoute>(&resource.content) {
                    Ok(route) => {
                        // Validate cross-namespace references (only if enabled)
                        if let Some(ref validator) = validator {
                            let errors = validate_udp_route(validator, &route);
                            if !errors.is_empty() {
                                for err in &errors {
                                    tracing::warn!(
                                        component = "conf_store",
                                        resource = "UDPRoute",
                                        namespace = ?route.namespace(),
                                        name = ?route.name_any(),
                                        error = %err,
                                        "Cross-namespace reference validation failed"
                                    );
                                }
                            }
                        }
                        config_server.udp_routes.apply_change(ResourceChange::InitAdd, route);
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Some(ResourceKind::TLSRoute) => {
                match serde_yaml::from_str::<TLSRoute>(&resource.content) {
                    Ok(route) => {
                        // Validate cross-namespace references (only if enabled)
                        if let Some(ref validator) = validator {
                            let errors = validate_tls_route(validator, &route);
                            if !errors.is_empty() {
                                for err in &errors {
                                    tracing::warn!(
                                        component = "conf_store",
                                        resource = "TLSRoute",
                                        namespace = ?route.namespace(),
                                        name = ?route.name_any(),
                                        error = %err,
                                        "Cross-namespace reference validation failed"
                                    );
                                }
                            }
                        }
                        config_server.tls_routes.apply_change(ResourceChange::InitAdd, route);
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Some(ResourceKind::Service) => {
                match serde_yaml::from_str::<Service>(&resource.content) {
                    Ok(svc) => {
                        config_server.services.apply_change(ResourceChange::InitAdd, svc);
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Some(ResourceKind::EndpointSlice) => {
                match serde_yaml::from_str::<EndpointSlice>(&resource.content) {
                    Ok(eps) => {
                        config_server.endpoint_slices.apply_change(ResourceChange::InitAdd, eps);
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Some(ResourceKind::Endpoint) => {
                match serde_yaml::from_str::<Endpoints>(&resource.content) {
                    Ok(endpoint) => {
                        config_server.endpoints.apply_change(ResourceChange::InitAdd, endpoint);
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Some(ResourceKind::EdgionTls) => {
                match serde_yaml::from_str::<EdgionTls>(&resource.content) {
                    Ok(tls) => {
                        config_server.apply_edgion_tls_change(ResourceChange::InitAdd, tls);
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Some(ResourceKind::EdgionPlugins) => {
                match serde_yaml::from_str::<EdgionPlugins>(&resource.content) {
                    Ok(plugins) => {
                        config_server.edgion_plugins.apply_change(ResourceChange::InitAdd, plugins);
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Some(ResourceKind::EdgionStreamPlugins) => {
                match serde_yaml::from_str::<EdgionStreamPlugins>(&resource.content) {
                    Ok(stream_plugins) => {
                        config_server.edgion_stream_plugins.apply_change(ResourceChange::InitAdd, stream_plugins);
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Some(ResourceKind::ReferenceGrant) => {
                // Skip: already loaded in Phase 1
                tracing::debug!(
                    component = "conf_store",
                    kind = kind_str,
                    name = name_str,
                    "Skipping ReferenceGrant (loaded in Phase 1)"
                );
                continue;
            }
            Some(ResourceKind::BackendTLSPolicy) => {
                match serde_yaml::from_str::<BackendTLSPolicy>(&resource.content) {
                    Ok(policy) => {
                        config_server.backend_tls_policies.apply_change(ResourceChange::InitAdd, policy);
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Some(ResourceKind::PluginMetaData) => {
                match serde_yaml::from_str::<PluginMetaData>(&resource.content) {
                    Ok(metadata) => {
                        config_server.plugin_metadata.apply_change(ResourceChange::InitAdd, metadata);
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Some(ResourceKind::LinkSys) => {
                match serde_yaml::from_str::<LinkSys>(&resource.content) {
                    Ok(linksys) => {
                        config_server.link_sys.apply_change(ResourceChange::InitAdd, linksys);
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Some(ResourceKind::Secret) => {
                match serde_yaml::from_str::<Secret>(&resource.content) {
                    Ok(secret) => {
                        config_server.apply_secret_change(ResourceChange::InitAdd, secret);
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            // Skip base conf resources (loaded separately via load_base)
            Some(ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig | ResourceKind::Gateway) => {
                tracing::debug!(
                    component = "conf_store",
                    kind = kind_str,
                    name = name_str,
                    "Skipping base conf resource (loaded via load_base)"
                );
                continue;
            }
            _ => {
                tracing::warn!(
                    component = "conf_store",
                    kind = kind_str,
                    name = name_str,
                    "Unknown or unsupported resource kind"
                );
                error_count += 1;
                continue;
            }
        };
        
        match load_result {
            Ok(_) => {
                loaded_count += 1;
                tracing::debug!(
                    component = "conf_store",
                    kind = kind_str,
                    namespace = ?namespace_str,
                    name = name_str,
                    "Resource loaded successfully"
                );
            }
            Err(e) => {
                error_count += 1;
                tracing::error!(
                    component = "conf_store",
                    kind = kind_str,
                    namespace = ?namespace_str,
                    name = name_str,
                    error = %e,
                    "Failed to load resource"
                );
            }
        }
    }
    
    tracing::info!(
        component = "conf_store",
        event = "load_resources_complete",
        loaded = loaded_count,
        errors = error_count,
        "Other resources loaded"
    );
    
    Ok(())
}

// Validation helper functions

fn validate_http_route(
    validator: &crate::core::ref_grant::CrossNamespaceValidator,
    route: &HTTPRoute,
) -> Vec<String> {
    let mut errors = Vec::new();
    let route_ns = route.namespace().unwrap_or_else(|| "default".to_string());
    
    if let Some(rules) = &route.spec.rules {
        for rule in rules {
            if let Some(backend_refs) = &rule.backend_refs {
                for backend_ref in backend_refs {
                    if let Some(backend_ns) = &backend_ref.namespace {
                        if backend_ns != &route_ns {
                            let group = backend_ref.group.as_deref().unwrap_or("");
                            let kind = backend_ref.kind.as_deref().unwrap_or("Service");
                            
                            let allowed = validator.store.check_reference_allowed(
                                &route_ns,
                                "gateway.networking.k8s.io",
                                "HTTPRoute",
                                backend_ns,
                                group,
                                kind,
                                Some(&backend_ref.name),
                            );
                            if !allowed {
                                errors.push(format!(
                                    "Cross-namespace reference not allowed: HTTPRoute in namespace '{}' cannot reference {}/{} in namespace '{}' (no ReferenceGrant)",
                                    route_ns, kind, backend_ref.name, backend_ns
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    
    errors
}

fn validate_grpc_route(
    validator: &crate::core::ref_grant::CrossNamespaceValidator,
    route: &GRPCRoute,
) -> Vec<String> {
    let mut errors = Vec::new();
    let route_ns = route.namespace().unwrap_or_else(|| "default".to_string());
    
    if let Some(rules) = &route.spec.rules {
        for rule in rules {
            if let Some(backend_refs) = &rule.backend_refs {
                for backend_ref in backend_refs {
                    if let Some(backend_ns) = &backend_ref.namespace {
                        if backend_ns != &route_ns {
                            let allowed = validator.store.check_reference_allowed(
                                &route_ns,
                                "gateway.networking.k8s.io",
                                "GRPCRoute",
                                backend_ns,
                                "",
                                "Service",
                                Some(&backend_ref.name),
                            );
                            if !allowed {
                                errors.push(format!(
                                    "Cross-namespace reference not allowed: GRPCRoute in namespace '{}' cannot reference Service/{} in namespace '{}' (no ReferenceGrant)",
                                    route_ns, backend_ref.name, backend_ns
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    
    errors
}

fn validate_tcp_route(
    validator: &crate::core::ref_grant::CrossNamespaceValidator,
    route: &TCPRoute,
) -> Vec<String> {
    let mut errors = Vec::new();
    let route_ns = route.namespace().unwrap_or_else(|| "default".to_string());
    
    if let Some(rules) = &route.spec.rules {
        for rule in rules {
            if let Some(backend_refs) = &rule.backend_refs {
                for backend_ref in backend_refs {
                    if let Some(backend_ns) = &backend_ref.namespace {
                        if backend_ns != &route_ns {
                            let allowed = validator.store.check_reference_allowed(
                                &route_ns,
                                "gateway.networking.k8s.io",
                                "TCPRoute",
                                backend_ns,
                                "",
                                "Service",
                                Some(&backend_ref.name),
                            );
                            if !allowed {
                                errors.push(format!(
                                    "Cross-namespace reference not allowed: TCPRoute in namespace '{}' cannot reference Service/{} in namespace '{}' (no ReferenceGrant)",
                                    route_ns, backend_ref.name, backend_ns
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    
    errors
}

fn validate_udp_route(
    validator: &crate::core::ref_grant::CrossNamespaceValidator,
    route: &UDPRoute,
) -> Vec<String> {
    let mut errors = Vec::new();
    let route_ns = route.namespace().unwrap_or_else(|| "default".to_string());
    
    if let Some(rules) = &route.spec.rules {
        for rule in rules {
            if let Some(backend_refs) = &rule.backend_refs {
                for backend_ref in backend_refs {
                    if let Some(backend_ns) = &backend_ref.namespace {
                        if backend_ns != &route_ns {
                            let allowed = validator.store.check_reference_allowed(
                                &route_ns,
                                "gateway.networking.k8s.io",
                                "UDPRoute",
                                backend_ns,
                                "",
                                "Service",
                                Some(&backend_ref.name),
                            );
                            if !allowed {
                                errors.push(format!(
                                    "Cross-namespace reference not allowed: UDPRoute in namespace '{}' cannot reference Service/{} in namespace '{}' (no ReferenceGrant)",
                                    route_ns, backend_ref.name, backend_ns
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    
    errors
}

fn validate_tls_route(
    validator: &crate::core::ref_grant::CrossNamespaceValidator,
    route: &TLSRoute,
) -> Vec<String> {
    let mut errors = Vec::new();
    let route_ns = route.namespace().unwrap_or_else(|| "default".to_string());
    
    if let Some(rules) = &route.spec.rules {
        for rule in rules {
            if let Some(backend_refs) = &rule.backend_refs {
                for backend_ref in backend_refs {
                    if let Some(backend_ns) = &backend_ref.namespace {
                        if backend_ns != &route_ns {
                            let allowed = validator.store.check_reference_allowed(
                                &route_ns,
                                "gateway.networking.k8s.io",
                                "TLSRoute",
                                backend_ns,
                                "",
                                "Service",
                                Some(&backend_ref.name),
                            );
                            if !allowed {
                                errors.push(format!(
                                    "Cross-namespace reference not allowed: TLSRoute in namespace '{}' cannot reference Service/{} in namespace '{}' (no ReferenceGrant)",
                                    route_ns, backend_ref.name, backend_ns
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    
    errors
}

