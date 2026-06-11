use crate::core::gateway::conf_sync::conf_client::ConfigClient;
use crate::core::gateway::observe::access_log::get_access_logger_unchecked;
use crate::core::gateway::observe::AccessLogger;
use crate::core::gateway::runtime::server::listener_builder;
use crate::core::gateway::runtime::store::listener_gateway_info::is_listener_conflicted;
use anyhow::{anyhow, Result};
use edgion_resources::listener_key::{resolve_listener_addr, ListenerKey};
use edgion_resources::resources::edgion_gateway_config::{EdgionGatewayConfig, EdgionGatewayConfigSpec};
use edgion_resources::{Gateway, ResourceMeta};
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
        .and_then(|annotations| annotations.get(edgion_resources::constants::annotations::edgion::ENABLE_HTTP2))
        .map(|value| !matches!(value.to_lowercase().as_str(), "false" | "0" | "no" | "off"))
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

    /// Configure listeners on Pingora server for all Gateway resources.
    ///
    /// This method:
    /// 1. For each Gateway, fetches its GatewayClass and EdgionGatewayConfig.
    /// 2. For each listener in the Gateway, adds it to the Pingora server.
    ///
    /// # Startup invariant
    ///
    /// Callers must ensure `GatewayHandler::full_set` has already populated
    /// `GatewayStore` and `ListenerGatewayInfoStore` before invoking this
    /// method. This is enforced by `cli/mod.rs` step 18 (`wait_for_ready`),
    /// which blocks until the gateway cache's `reset()` has synchronously
    /// driven `full_set` (`cache_data.rs::CacheData::reset`).
    pub fn configure_listeners(
        &self,
        pingora_server: &mut Server,
        gateways: Vec<edgion_resources::Gateway>,
    ) -> Result<()> {
        if gateways.is_empty() {
            // The cache is marked ready as soon as the first gRPC list response arrives,
            // regardless of item count, so reaching this branch means the controller pushed
            // zero Gateways. Surface the sibling cache counts so the operator can tell
            // "controller never synced anything" from "controller synced but no Gateway CRs
            // matched its class/namespace scope".
            let gateway_class_count = self.config_client.list_gateway_classes().data.len();
            let edgion_gateway_config_count = self.config_client.list_edgion_gateway_configs().data.len();
            return Err(anyhow!(
                "No Gateway resources synced from controller (gateways=0, gateway_classes={}, edgion_gateway_configs={}); \
                 check the controller's gateway_class_name, watched namespace, and RBAC scope, \
                 then verify Gateway CRs exist in the cluster",
                gateway_class_count,
                edgion_gateway_config_count
            ));
        }

        tracing::info!("Configuring {} Gateway resources", gateways.len());

        // Get ServerConf Arc from pingora_server for listener creation.
        // GatewayStore and ListenerGatewayInfoStore are already populated by
        // GatewayHandler::full_set (see the method-level startup invariant).
        let server_conf_arc = pingora_server.configuration.clone();

        // Track bound listeners to avoid duplicate physical bindings
        let mut bound_listeners: HashSet<ListenerKey> = HashSet::new();

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
            let edgion_gateway_config = match gateway_class.spec.parameters_ref.as_ref() {
                Some(params_ref) => match self.config_client.get_edgion_gateway_config(&params_ref.name) {
                    Some(egwc) => egwc,
                    None => {
                        tracing::warn!(
                            gateway = %gateway.key_name(),
                            edgion_gateway_config = %params_ref.name,
                            "EdgionGatewayConfig not found, skipping Gateway"
                        );
                        continue;
                    }
                },
                None => {
                    tracing::info!(
                        gateway = %gateway.key_name(),
                        gateway_class = %gateway_class_name,
                        "GatewayClass has no parametersRef, using built-in defaults"
                    );
                    EdgionGatewayConfig::new("default", EdgionGatewayConfigSpec::default())
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
                    let addr = resolve_listener_addr(gateway.spec.addresses.as_ref(), listener.port);
                    let listener_key = ListenerKey::new(addr, &listener.protocol);

                    if bound_listeners.contains(&listener_key) {
                        tracing::info!(
                            gateway = %gateway.key_name(),
                            listener = %listener.name,
                            key = %listener_key,
                            "Listener skipped - already bound (routes served via global table)"
                        );
                        continue;
                    }

                    if is_listener_conflicted(gateway, &listener.name) {
                        tracing::warn!(
                            gateway = %gateway.key_name(),
                            listener = %listener.name,
                            key = %listener_key,
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
                        listener_key: listener_key.clone(),
                        access_logger: self.get_access_logger(),
                        edgion_gateway_config: Arc::new(edgion_gateway_config.clone()),
                        server_conf: server_conf_arc.clone(),
                        enable_http2,
                        gateway_annotations: gateway_annotations.clone(),
                    };

                    // Add listener to pingora_server
                    listener_builder::add_listener(pingora_server, context)?;
                    bound_listeners.insert(listener_key);
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
