use crate::conf_sync::conf_client::ConfigClient;
use crate::observe::access_log::get_access_logger_unchecked;
use crate::observe::AccessLogger;
use crate::runtime::server::listener_builder;
use crate::runtime::server::proxy_protocol_policy::{
    get_global_proxy_protocol_policy_runtime, PreparedProxyProtocolPolicy,
};
use crate::runtime::store::listener_gateway_info::is_listener_conflicted;
use anyhow::{Context, Result};
use edgion_resources::listener_key::{resolve_listener_addr, ListenerKey};
use edgion_resources::resources::proxy_protocol::ResolvedInboundProxyProtocol;
use edgion_resources::{Gateway, ResourceMeta};
use kube::ResourceExt;
use pingora_core::server::Server;
use std::collections::HashSet;
use std::sync::Arc;

pub struct GatewayBase {
    config_client: Arc<ConfigClient>,
}

/// Parse HTTP/2 enable flag from Gateway annotation.
/// Returns `true` if the annotation is absent or any value other than "false", "0", "no", "off".
/// Returns `false` for those falsy values (case-insensitive).
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
        // Validate every Gateway and shared physical listener before adding any
        // service to Pingora. This also freezes the process-lifetime policy map
        // used by runtime update enforcement.
        let prepared_proxy_policies = get_global_proxy_protocol_policy_runtime()
            .preflight_and_freeze(gateways.iter())
            .context("inbound PROXY protocol startup preflight failed")?;

        if gateways.is_empty() {
            // The cache is marked ready as soon as the first gRPC list response arrives,
            // regardless of item count, so reaching this branch means the controller pushed
            // zero Gateways. Surface the sibling cache counts so the operator can tell
            // "controller never synced anything" from "controller synced but no Gateway CRs
            // matched its class/namespace scope".
            let gateway_class_count = self.config_client.list_gateway_classes().data.len();
            let edgion_gateway_config_count = self.config_client.list_edgion_gateway_configs().data.len();
            tracing::warn!(
                gateway_classes = gateway_class_count,
                edgion_gateway_configs = edgion_gateway_config_count,
                "No Gateway resources synced from controller; starting without data-plane listeners"
            );
            return Ok(());
        }

        tracing::info!("Configuring {} Gateway resources", gateways.len());

        // Get ServerConf Arc from pingora_server for listener creation.
        // GatewayStore and ListenerGatewayInfoStore are already populated by
        // GatewayHandler::full_set (see the method-level startup invariant).
        let server_conf_arc = pingora_server.configuration.clone();

        // Track bound listeners to avoid duplicate physical bindings
        let mut bound_listeners: HashSet<ListenerKey> = HashSet::new();

        // Process each Gateway and configure listeners
        for gateway in gateways.iter() {
            // Get GatewayClass for this Gateway
            let gateway_class_name = &gateway.spec.gateway_class_name;
            let Some(gateway_class) = self.config_client.get_gateway_class(gateway_class_name) else {
                tracing::warn!(
                    gateway = %gateway.key_name(),
                    gateway_class = %gateway_class_name,
                    "GatewayClass not found, skipping Gateway"
                );
                continue;
            };

            // Get EdgionGatewayConfig for this Gateway
            let edgion_gateway_config_name = match gateway_class.spec.parameters_ref.as_ref() {
                Some(params_ref) => match self.config_client.get_edgion_gateway_config(&params_ref.name) {
                    Some(_) => Some(params_ref.name.clone()),
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
                    None
                }
            };

            let egc = crate::config::bind_gateway_config(edgion_gateway_config_name.as_deref());

            // Process listeners for this Gateway
            if let Some(listeners) = &gateway.spec.listeners {
                tracing::info!(
                    "Processing gateway {} with {} listeners",
                    gateway.name_any(),
                    listeners.len()
                );

                // Prepare gateway metadata
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
                        gateway_namespace: gateway_namespace.clone(),
                        gateway_name: gateway_name.clone(),
                        gateway_key: gateway.key_name(),
                        listener: listener.clone(),
                        listener_key: listener_key.clone(),
                        access_logger: self.get_access_logger(),
                        egc: Arc::clone(&egc),
                        server_conf: server_conf_arc.clone(),
                        enable_http2,
                        gateway_annotations: gateway_annotations.clone(),
                        proxy_protocol_policy: if matches!(
                            listener.protocol.to_ascii_uppercase().as_str(),
                            "HTTP" | "HTTPS" | "TCP" | "TLS"
                        ) {
                            prepared_proxy_policies.get(&listener_key).cloned().ok_or_else(|| {
                                anyhow::anyhow!(
                                    "Gateway '{}' listener '{}' has no prepared inbound PROXY protocol policy",
                                    gateway.key_name(),
                                    listener.name
                                )
                            })?
                        } else {
                            PreparedProxyProtocolPolicy::Disabled
                        },
                        proxy_protocol_source: match &gateway.spec.resolved_inbound_proxy_protocol {
                            ResolvedInboundProxyProtocol::Ready {
                                listener_names, source, ..
                            } if listener_names.contains(&listener.name) => Some(*source),
                            _ => None,
                        },
                    };

                    // Add listener to pingora_server
                    listener_builder::add_listener(pingora_server, context)?;
                    bound_listeners.insert(listener_key);
                }
            }
        }

        // Print summary
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
