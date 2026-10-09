use crate::routes::common::filter_accepted_parent_refs;
use crate::runtime::matching::route::ResolvedParentRef;
use crate::runtime::store::listener_gateway_info::format_gateway_key;
use edgion_algo::matcher::radix_hostname::RadixHostnamePatternRef;
use edgion_resources::resources::refs::plugin_attachment_fields::StreamPluginAttachment;
use std::sync::Arc;

use super::{cap_tls_connect_retries, DEFAULT_TLS_CONNECT_RETRIES, MAX_TLS_CONNECT_RETRIES};
use crate::backends::{CompiledL4Backends, SelectedL4Backend};
use crate::routes::common::{resolve_stream_plugin_store_key, resolve_tcp_keepalive};
use edgion_resources::resources::common::TcpKeepaliveConfig;
use edgion_resources::resources::refs::ReferenceError;
use edgion_resources::resources::TLSRoute;
use edgion_resources::ResourceMeta;

pub(crate) struct AcceptedTlsAttachment {
    pub schema_index: usize,
    pub parent: ResolvedParentRef,
}

/// Gateway-side compiled companion for a TLSRoute.
///
/// Holds the route schema plus the gateway-side compiled runtime (BackendSelector,
/// proxy-protocol version, connect-retry budget, keepalive config,
/// stream-plugin store key). Built ONCE per route and shared (Arc-cloned) across
/// every SNI hostname bucket the route serves, so the WRR selector cursor and
/// per-rule state are not split per hostname.
///
/// Per-protocol note: TLS routes fan out across SNI hostnames but a single
/// route shares one rule set, so the compiled runtime is built from the first
/// rule (matching the data-plane read path `rules.first()`).
/// The schema and compiled selection cannot be replaced independently.
/// ```compile_fail
/// use edgion_gateway::routes::tls::TlsRouteUnit;
/// fn replace(unit: &mut TlsRouteUnit) { unit.route = unit.route().clone(); }
/// ```
pub struct TlsRouteUnit {
    /// Each effective hostname list stays paired with its exact logical owner.
    accepted_attachments: Vec<AcceptedTlsAttachment>,
    route: Arc<TLSRoute>,
    /// WRR selector over the first rule's backends (one per route, shared
    /// across all SNI hostnames that map to this route).
    backend_finder: Arc<CompiledL4Backends>,
    /// Store key for dynamic stream-plugin lookup (from route annotations).
    stream_plugin_store_key: Option<String>,
    /// Proxy Protocol version to send upstream (None = disabled, Some(2) = PP2).
    proxy_protocol_version: Option<u8>,
    /// Max upstream connect attempts (1 = no retry).
    max_connect_retries: u32,
    /// Resolved upstream TCP keepalive config (from route annotations).
    tcp_keepalive: Option<TcpKeepaliveConfig>,
    /// Operator log labels resolved once from the route metadata (annotations /
    /// labels) at build time, attached to `rsc.labels` in the TLS access log.
    /// Resolving here keeps the per-connection match path off the metadata scan.
    log_labels: Option<crate::LogLabels>,
}

impl TlsRouteUnit {
    /// Build a unit from a cached route.
    ///
    /// Resolves the proxy-protocol version, stream-plugin store key, connect
    /// retry budget and keepalive config from the route's metadata/annotations
    /// (single source of truth shared with the HTTP/TCP conf-handlers) and
    /// builds the WRR selector from the first rule's backend refs.
    pub fn from_route(route: Arc<TLSRoute>) -> Result<Self, ReferenceError> {
        let backend_finder = Arc::new(CompiledL4Backends::from_tls(&route)?);
        let namespace = route.metadata.namespace.as_deref().unwrap_or("default");
        let accepted = filter_accepted_parent_refs(
            route.spec.parent_refs.as_deref(),
            route
                .spec
                .current_status
                .as_ref()
                .map(|status| status.parents.as_slice()),
            Some(namespace),
            route.spec.resolved_status_controller.as_deref(),
        )
        .unwrap_or_default();
        let mut accepted_attachments: Vec<AcceptedTlsAttachment> = Vec::new();
        if route.spec.validate_hostnames().is_ok() {
            // Preserve authored parent order: admission selects the first owner.
            for parent in &accepted {
                for (i, attachment) in route
                    .spec
                    .resolved_listeners
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                {
                    let port = attachment
                        .listener_key
                        .0
                        .split_once('/')
                        .and_then(|(addr, _)| addr.parse::<std::net::SocketAddr>().ok())
                        .map(|addr| i32::from(addr.port()));
                    if attachment.listener_key.protocol() != "TLS"
                        || attachment.hostnames.is_empty()
                        || attachment
                            .hostnames
                            .iter()
                            .any(|hostname| RadixHostnamePatternRef::parse(hostname).is_err())
                        || parent.namespace.as_deref().unwrap_or(namespace) != attachment.gateway_namespace
                        || parent.name != attachment.gateway_name
                        || parent
                            .section_name
                            .as_deref()
                            .is_some_and(|name| name != attachment.listener_name)
                        || parent.port.is_some_and(|required| Some(required) != port)
                        || accepted_attachments.iter().any(|accepted| accepted.schema_index == i)
                    {
                        continue;
                    }
                    accepted_attachments.push(AcceptedTlsAttachment {
                        schema_index: i,
                        parent: ResolvedParentRef {
                            port: parent.port,
                            gateway_key: format_gateway_key(&attachment.gateway_namespace, &attachment.gateway_name),
                            parent_ns: attachment.gateway_namespace.clone(),
                            section_name: Some(attachment.listener_name.clone()),
                        },
                    });
                }
            }
        }

        let route_key = route.key_name();
        let annotations = route.metadata.annotations.as_ref();

        let proxy_protocol_version = annotations
            .and_then(|a| a.get(edgion_resources::constants::annotations::edgion::PROXY_PROTOCOL))
            .and_then(|v| match v.trim() {
                "v2" => Some(2u8),
                _ => None,
            });

        let stream_plugin_store_key = resolve_stream_plugin_store_key(StreamPluginAttachment::Tls(&route));

        let configured_max_connect_retries = annotations
            .and_then(|a| a.get(edgion_resources::constants::annotations::edgion::MAX_CONNECT_RETRIES))
            .and_then(|v| v.trim().parse::<u32>().ok())
            .map(|v| v.max(1))
            .unwrap_or(DEFAULT_TLS_CONNECT_RETRIES);
        let max_connect_retries = cap_tls_connect_retries(configured_max_connect_retries);
        if configured_max_connect_retries > MAX_TLS_CONNECT_RETRIES {
            tracing::warn!(
                route = %route_key,
                configured_max_connect_retries,
                max_connect_retries,
                "TLSRoute max_connect_retries capped"
            );
        }

        let tcp_keepalive = resolve_tcp_keepalive(annotations, &route_key);
        let log_labels = crate::extract_log_labels(&route.metadata);

        Ok(TlsRouteUnit {
            route,
            accepted_attachments,
            backend_finder,
            stream_plugin_store_key,
            proxy_protocol_version,
            max_connect_retries,
            tcp_keepalive,
            log_labels,
        })
    }

    pub fn route(&self) -> &Arc<TLSRoute> {
        &self.route
    }

    pub(crate) fn select_backend(&self) -> Result<SelectedL4Backend<'_>, u32> {
        self.backend_finder.select()
    }

    pub(crate) fn stream_plugin_store_key(&self) -> Option<&str> {
        self.stream_plugin_store_key.as_deref()
    }

    pub(crate) fn accepted_attachments(&self) -> &[AcceptedTlsAttachment] {
        &self.accepted_attachments
    }
    pub(crate) fn proxy_protocol_version(&self) -> Option<u8> {
        self.proxy_protocol_version
    }
    pub(crate) fn max_connect_retries(&self) -> u32 {
        self.max_connect_retries
    }
    pub(crate) fn log_labels(&self) -> Option<&crate::LogLabels> {
        self.log_labels.as_ref()
    }

    pub(crate) fn tcp_keepalive(&self) -> Option<&TcpKeepaliveConfig> {
        self.tcp_keepalive.as_ref()
    }

    /// Delegate to the inner route's resource key (e.g. `"default/my-route"`).
    pub fn key_name(&self) -> String {
        self.route.key_name()
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/routes/tls/route_unit_test.rs"]
mod tests;
