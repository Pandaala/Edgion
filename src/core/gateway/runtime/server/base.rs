use crate::core::gateway::conf_sync::conf_client::ConfigClient;
use crate::core::gateway::observe::access_log::get_access_logger_unchecked;
use crate::core::gateway::observe::AccessLogger;
use crate::core::gateway::runtime::server::listener_builder;
use crate::core::gateway::runtime::store::{get_global_gateway_store, rebuild_port_gateway_infos};
use crate::types::{Gateway, ResourceMeta};
use anyhow::{anyhow, Result};
use kube::ResourceExt;
use pingora_core::server::Server;
use std::collections::HashSet;
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
        .map(|value| !matches!(value.to_lowercase().as_str(), "false" | "0" | "no" | "off"))
        .unwrap_or(true) // Default to true if annotation is not present
}

/// Check if a Listener is marked as Conflicted in Gateway status
///
/// Returns true if the Listener has a Conflicted condition with status "True".
/// This is set by the Controller when port conflicts are detected.
fn is_listener_conflicted(gateway: &Gateway, listener_name: &str) -> bool {
    gateway
        .status
        .as_ref()
        .and_then(|s| s.listeners.as_ref())
        .and_then(|listeners| listeners.iter().find(|ls| ls.name == listener_name))
        .map(|ls| {
            ls.conditions
                .iter()
                .any(|c| c.type_ == "Conflicted" && c.status == "True")
        })
        .unwrap_or(false)
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

        // 1. Ensure all gateways are in the global gateway store
        // Use update_gateway (idempotent) instead of add_gateway to avoid warnings
        // when GatewayHandler has already added them via full_set/partial_update
        let gateway_store = get_global_gateway_store();
        {
            let mut store_guard = gateway_store.write().unwrap_or_else(|e| e.into_inner());
            for gateway in gateways.iter() {
                store_guard.update_gateway(gateway.clone());
            }
        }

        // 2. Get ServerConf Arc from pingora_server for listener creation
        let server_conf_arc = pingora_server.configuration.clone();

        // 2.5. Populate global PortGatewayInfoStore so that route matching can
        // dynamically look up which Gateways share each port.
        rebuild_port_gateway_infos(&gateways);

        // Track bound ports to avoid duplicate physical bindings
        let mut bound_ports: HashSet<u16> = HashSet::new();

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
                let enable_http2 = parse_enable_http2_annotation(gateway);

                if !enable_http2 {
                    tracing::info!(
                        gateway = %gateway.key_name(),
                        "HTTP/2 disabled via annotation"
                    );
                }

                for listener in listeners {
                    let port = listener.port as u16;

                    if bound_ports.contains(&port) {
                        tracing::info!(
                            gateway = %gateway.key_name(),
                            listener = %listener.name,
                            port = port,
                            "Listener skipped - port already bound by another Gateway (routes served via global table)"
                        );
                        continue;
                    }

                    if is_listener_conflicted(gateway, &listener.name) {
                        tracing::warn!(
                            gateway = %gateway.key_name(),
                            listener = %listener.name,
                            port = port,
                            "Listener skipped - marked as Conflicted by Controller (port conflict)"
                        );
                        continue;
                    }

                    tracing::info!(
                        "  Listener: name={}, protocol={}, port={}",
                        listener.name,
                        listener.protocol,
                        listener.port
                    );

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
                    bound_ports.insert(port);
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
