use crate::core::conf_sync::conf_client::ConfigClient;
use crate::core::gateway::gateway_store::get_global_gateway_store;
use crate::core::gateway::listener_builder;
use crate::core::observe::access_log::get_access_logger_unchecked;
use crate::core::observe::AccessLogger;
use crate::types::{Gateway, ResourceMeta};
use anyhow::{anyhow, Result};
use kube::ResourceExt;
use pingora_core::server::Server;
use std::sync::Arc;

pub struct GatewayBase {
    config_client: Arc<ConfigClient>,
}

/// Parse HTTP/2 enable flag from Gateway annotations
/// Returns true if annotation is not set or set to "true"
/// Returns false if annotation is set to "false"
fn parse_enable_http2_annotation(gateway: &Gateway) -> bool {
    gateway
        .metadata
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.get(listener_builder::ANNOTATION_ENABLE_HTTP2))
        .and_then(|value| match value.to_lowercase().as_str() {
            "false" | "0" | "no" | "off" => Some(false),
            _ => Some(true),
        })
        .unwrap_or(true) // Default to true if annotation is not present
}

impl GatewayBase {
    pub fn new(config_client: Arc<ConfigClient>) -> Self {
        Self { config_client }
    }

    /// Get the access logger
    fn get_access_logger(&self) -> Arc<AccessLogger> {
        get_access_logger_unchecked().clone()
    }

    /// Configure listeners on Pingora server for all Gateway resources
    ///
    /// This method:
    /// 1. Adds all gateways to the global gateway store
    /// 2. For each Gateway, fetches its GatewayClass and EdgionGatewayConfig
    /// 3. For each listener in the Gateway, adds it to the Pingora server
    pub fn configure_listeners(&self, pingora_server: &mut Server, gateways: Vec<crate::types::Gateway>) -> Result<()> {
        if gateways.is_empty() {
            return Err(anyhow!("No Gateway resources available"));
        }

        tracing::info!("Configuring {} Gateway resources", gateways.len());

        // 1. Add all gateways to the global gateway store
        let gateway_store = get_global_gateway_store();
        {
            let mut store_guard = gateway_store.write().unwrap();
            for gateway in gateways.iter() {
                if let Err(e) = store_guard.add_gateway(gateway.clone()) {
                    tracing::warn!("Failed to add gateway to store: {}", e);
                }
            }
        }

        // 2. Get ServerConf Arc from pingora_server for listener creation
        let server_conf_arc = pingora_server.configuration.clone();

        // 3. Process each Gateway and configure listeners
        for gateway in gateways.iter() {
            // 3.1 Get GatewayClass for this Gateway
            let gateway_class_name = &gateway.spec.gateway_class_name;
            let gateway_class = match self.config_client.get_gateway_class(gateway_class_name) {
                Some(gc) => gc,
                None => {
                    tracing::warn!(
                        gateway = %gateway.key_name(),
                        gateway_class = %gateway_class_name,
                        "GatewayClass not found, skipping Gateway"
                    );
                    continue;
                }
            };

            // 3.2 Get EdgionGatewayConfig for this Gateway
            let edgion_gateway_config_name = match gateway_class.spec.parameters_ref.as_ref() {
                Some(params_ref) => &params_ref.name,
                None => {
                    tracing::warn!(
                        gateway = %gateway.key_name(),
                        gateway_class = %gateway_class_name,
                        "GatewayClass has no parametersRef, skipping Gateway"
                    );
                    continue;
                }
            };

            let edgion_gateway_config = match self.config_client.get_edgion_gateway_config(edgion_gateway_config_name) {
                Some(egwc) => egwc,
                None => {
                    tracing::warn!(
                        gateway = %gateway.key_name(),
                        edgion_gateway_config = %edgion_gateway_config_name,
                        "EdgionGatewayConfig not found, skipping Gateway"
                    );
                    continue;
                }
            };

            // 3.3 Process listeners for this Gateway
            if let Some(listeners) = &gateway.spec.listeners {
                tracing::info!(
                    "Processing gateway {} with {} listeners",
                    gateway.name_any(),
                    listeners.len()
                );

                // Prepare gateway metadata
                let gateway_class_name_clone = gateway_class.metadata.name.clone();
                let gateway_namespace = gateway.metadata.namespace.clone();
                let gateway_name = gateway.name_any();

                // Extract gateway annotations (convert BTreeMap to HashMap)
                let gateway_annotations = gateway
                    .metadata
                    .annotations
                    .clone()
                    .map(|btree| btree.into_iter().collect::<std::collections::HashMap<_, _>>())
                    .unwrap_or_default();

                // Parse HTTP/2 setting from Gateway annotation
                let enable_http2 = parse_enable_http2_annotation(&gateway);

                if !enable_http2 {
                    tracing::info!(
                        gateway = %gateway.key_name(),
                        "HTTP/2 disabled via annotation"
                    );
                }

                // Add each listener
                for listener in listeners {
                    tracing::info!(
                        "  Listener: name={}, protocol={}, port={}",
                        listener.name,
                        listener.protocol,
                        listener.port
                    );

                    // Create listener context
                    let context = listener_builder::ListenerContext {
                        gateway_class_name: gateway_class_name_clone.clone(),
                        gateway_namespace: gateway_namespace.clone(),
                        gateway_name: gateway_name.clone(),
                        gateway_key: gateway.key_name(),
                        listener: listener.clone(),
                        access_logger: self.get_access_logger(),
                        edgion_gateway_config: Arc::new(edgion_gateway_config.clone()),
                        server_conf: server_conf_arc.clone(),
                        enable_http2,
                        gateway_annotations: gateway_annotations.clone(),
                    };

                    // Add listener to pingora_server
                    listener_builder::add_listener(pingora_server, context)?;
                }
            }
        }

        // 4. Print summary
        tracing::info!("Gateway listeners configured. Summary:");
        for gateway in gateways.iter() {
            if let Some(listeners) = &gateway.spec.listeners {
                for listener in listeners {
                    let host = listener.hostname.as_deref().unwrap_or("0.0.0.0");
                    let addr = format!("{}:{}", host, listener.port);
                    let protocol = &listener.protocol;
                    tracing::info!(
                        gateway=%gateway.key_name(),
                        listener=%listener.name,
                        addr=%addr,
                        protocol=%protocol,
                        "Listener configured"
                    );
                }
            }
        }

        Ok(())
    }
}
