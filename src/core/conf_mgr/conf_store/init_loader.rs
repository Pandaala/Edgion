use anyhow::{Context, Result};
use std::sync::Arc;
use crate::core::conf_sync::{ConfigServer, CacheEventDispatch};
use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_mgr::ConfStore;
use crate::types::ResourceKind;
use crate::types::prelude_resources::*;
use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;

/// Load all resources from storage into ConfigServer
pub async fn load_all_resources_from_store(
    store: Arc<dyn ConfStore>,
    config_server: Arc<ConfigServer>,
) -> Result<()> {
    tracing::info!(
        component = "conf_store",
        event = "init_load_start",
        "Loading all resources from store..."
    );
    
    let all_resources = store
        .list_all()
        .await
        .context("Failed to list all resources from store")?;
    
    tracing::info!(
        component = "conf_store",
        resource_count = all_resources.len(),
        "Found resources to load"
    );
    
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
                        config_server.routes.apply_change(ResourceChange::InitAdd, route);
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Some(ResourceKind::GRPCRoute) => {
                match serde_yaml::from_str::<GRPCRoute>(&resource.content) {
                    Ok(route) => {
                        config_server.grpc_routes.apply_change(ResourceChange::InitAdd, route);
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Some(ResourceKind::TCPRoute) => {
                match serde_yaml::from_str::<TCPRoute>(&resource.content) {
                    Ok(route) => {
                        config_server.tcp_routes.apply_change(ResourceChange::InitAdd, route);
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Some(ResourceKind::UDPRoute) => {
                match serde_yaml::from_str::<UDPRoute>(&resource.content) {
                    Ok(route) => {
                        config_server.udp_routes.apply_change(ResourceChange::InitAdd, route);
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Some(ResourceKind::TLSRoute) => {
                match serde_yaml::from_str::<TLSRoute>(&resource.content) {
                    Ok(route) => {
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
                match serde_yaml::from_str::<ReferenceGrant>(&resource.content) {
                    Ok(ref_grant) => {
                        config_server.reference_grants.apply_change(ResourceChange::InitAdd, ref_grant);
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(e) => Err(e.into()),
                }
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
        event = "init_load_complete",
        loaded = loaded_count,
        errors = error_count,
        "Resource initialization complete"
    );
    
    Ok(())
}

