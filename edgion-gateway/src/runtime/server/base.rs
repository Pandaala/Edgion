use crate::conf_sync::conf_client::ConfigClient;
use crate::observe::access_log::get_access_logger_unchecked;
use crate::observe::AccessLogger;
use crate::runtime::server::downstream_h2::DownstreamH2Policy;
use crate::runtime::server::listener_builder;
use crate::runtime::server::proxy_protocol_policy::{
    get_global_proxy_protocol_policy_runtime, PreparedProxyProtocolPolicy,
};
use crate::runtime::store::listener_gateway_info::is_listener_conflicted;
use anyhow::{Context, Result};
use edgion_resources::listener_key::{resolve_listener_addr, ListenerKey};
use edgion_resources::resources::proxy_protocol::ResolvedInboundProxyProtocol;
use edgion_resources::resources::refs::gateway_class_fields::GatewayClassParametersRef;
use edgion_resources::{Gateway, GatewayClass, ResourceMeta};
use kube::ResourceExt;
use pingora_core::server::Server;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

pub struct GatewayBase {
    config_client: Arc<ConfigClient>,
    bind_ip: IpAddr,
}

/// Parse HTTP/2 enable flag from Gateway annotation.
/// Returns `true` if the annotation is absent or any value other than "false", "0", "no", "off".
/// Returns `false` for those falsy values (case-insensitive).
fn parse_enable_http2_annotation(gateway: &Gateway) -> bool {
    matches!(
        edgion_resources::resources::gateway::HttpServiceDeclaration::from_gateway(gateway, "HTTPS"),
        Some(edgion_resources::resources::gateway::HttpServiceDeclaration::Https { enable_http2: true })
    )
}

/// Resolve the class-owned target before querying startup configuration.
fn resolve_gateway_config_name(class: &GatewayClass, mut exists: impl FnMut(&str) -> bool) -> Result<Option<String>> {
    let Some(reference) = GatewayClassParametersRef::from_class(class)? else {
        return Ok(None);
    };
    let name = reference.target().name();
    if !exists(name) {
        anyhow::bail!("referenced EdgionGatewayConfig not found");
    }
    Ok(Some(name.to_owned()))
}

impl GatewayBase {
    pub fn new(config_client: Arc<ConfigClient>, bind_ip: Option<IpAddr>) -> Self {
        Self {
            config_client,
            bind_ip: bind_ip.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
        }
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
        downstream_h2_policy: &DownstreamH2Policy,
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
        let mut offload_budget = listener_builder::TlsOffloadBudget::new(&server_conf_arc)?;

        // Collect only constructible candidates. Admitted declarations take
        // precedence over native readiness failures, whose sockets still permit
        // certificate recovery when no admitted service owns the key.
        let mut candidates = Vec::new();

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
            let edgion_gateway_config_name = match resolve_gateway_config_name(&gateway_class, |name| {
                self.config_client.get_edgion_gateway_config(name).is_some()
            }) {
                Ok(Some(name)) => Some(name),
                Ok(None) => {
                    tracing::info!(
                        gateway = %gateway.key_name(),
                        gateway_class = %gateway_class_name,
                        "GatewayClass has no parametersRef, using built-in defaults"
                    );
                    None
                }
                Err(error) => {
                    tracing::warn!(
                        gateway = %gateway.key_name(),
                        gateway_class = %gateway_class_name,
                        error = %error,
                        "GatewayClass parametersRef unusable, skipping Gateway"
                    );
                    continue;
                }
            };

            let egc = crate::config::bind_gateway_config(edgion_gateway_config_name.as_deref());

            // Process listeners for this Gateway
            if let Some(listeners) = &gateway.spec.listeners {
                let gateway_bind_ip = edgion_resources::listener_key::gateway_local_bind_ip(gateway)
                    .with_context(|| {
                        format!(
                            "Gateway '{}' has an invalid edgion.io/local-bind-ip annotation",
                            gateway.key_name()
                        )
                    })?
                    .unwrap_or(self.bind_ip);
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
                    let Some(native_fallback) = startup_listener_priority(gateway, &listener.name) else {
                        continue;
                    };
                    let addr = resolve_listener_addr(gateway, listener.port);
                    let listener_key = ListenerKey::new(addr, &listener.protocol);
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
                        bind_ip: gateway_bind_ip,
                        access_logger: self.get_access_logger(),
                        egc: Arc::clone(&egc),
                        server_conf: server_conf_arc.clone(),
                        downstream_h2_policy: downstream_h2_policy.clone(),
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

                    candidates.push((native_fallback, context));
                }
            }
        }

        for context in select_startup_listener_contexts(candidates)? {
            listener_builder::add_listener(pingora_server, context, &mut offload_budget)?;
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

fn startup_listener_priority(gateway: &Gateway, listener_name: &str) -> Option<bool> {
    use edgion_resources::resources::gateway::{
        gateway_listener_has_shared_policy_conflict, gateway_listener_is_explicitly_ineligible,
    };
    if gateway_listener_has_shared_policy_conflict(gateway, listener_name)
        || is_listener_conflicted(gateway, listener_name)
    {
        return None;
    }
    Some(gateway_listener_is_explicitly_ineligible(gateway, listener_name))
}

fn select_startup_listener_contexts(
    mut candidates: Vec<(bool, listener_builder::ListenerContext)>,
) -> Result<Vec<listener_builder::ListenerContext>> {
    // Stable ordering preserves input precedence among equally eligible owners.
    candidates.sort_by_key(|(native_fallback, _)| *native_fallback);
    let mut bound_listeners: HashMap<ListenerKey, (SocketAddr, bool)> = HashMap::new();
    let mut selected = Vec::new();
    for (_, context) in candidates {
        let socket_addr = SocketAddr::new(context.bind_ip, context.listener.port as u16);
        let is_udp = context.listener.protocol.eq_ignore_ascii_case("UDP");
        if let Some((previous_addr, _)) = bound_listeners.get(&context.listener_key) {
            anyhow::ensure!(
                *previous_addr == socket_addr,
                "Gateway '{}' listener '{}' shares logical listener '{}' but binds {} instead of {}",
                context.gateway_key,
                context.listener.name,
                context.listener_key,
                socket_addr,
                previous_addr
            );
            tracing::info!(gateway = %context.gateway_key, listener = %context.listener.name,
                key = %context.listener_key, "Listener skipped - already bound (routes served via global table)");
            continue;
        }
        if let Some((previous_key, (previous_addr, _))) =
            bound_listeners.iter().find(|(_, (previous_addr, previous_udp))| {
                socket_bindings_overlap(*previous_addr, *previous_udp, socket_addr, is_udp)
            })
        {
            anyhow::bail!(
                "Gateway '{}' listener '{}' binds {} which overlaps listener '{}' on {}",
                context.gateway_key,
                context.listener.name,
                socket_addr,
                previous_key,
                previous_addr
            );
        }
        bound_listeners.insert(context.listener_key.clone(), (socket_addr, is_udp));
        selected.push(context);
    }
    Ok(selected)
}

fn socket_bindings_overlap(previous: SocketAddr, previous_udp: bool, current: SocketAddr, current_udp: bool) -> bool {
    if previous_udp != current_udp || previous.port() != current.port() {
        return false;
    }
    let normalized = |ip: IpAddr| match ip {
        IpAddr::V6(ipv6) => ipv6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(ipv6)),
        other => other,
    };
    match (normalized(previous.ip()), normalized(current.ip())) {
        (IpAddr::V4(a), IpAddr::V4(b)) => a == b || a.is_unspecified() || b.is_unspecified(),
        (IpAddr::V6(a), IpAddr::V6(b)) => a == b || a.is_unspecified() || b.is_unspecified(),
        // Pingora leaves IPV6_V6ONLY at the OS default. An IPv6 wildcard may
        // also claim IPv4, so reject that pairing until socket options fix it.
        (IpAddr::V6(a), IpAddr::V4(_)) | (IpAddr::V4(_), IpAddr::V6(a)) => a.is_unspecified(),
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/runtime/server/base_test.rs"]
mod tests;
