use async_trait::async_trait;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pingora_core::apps::ServerApp;
use pingora_core::connectors::TransportConnector;
use pingora_core::protocols::Stream;
use pingora_core::server::ShutdownWatch;
use pingora_core::upstreams::peer::BasicPeer;

use crate::backends::{
    select_service_endpoint, validate_endpoint_backend_addr, L4BackendRejection, L4ResolveError, ServiceDegradePolicy,
};
use crate::config::ResolvedGatewayConfig;
use crate::observe::{log_tcp, TcpLogEntry};
use crate::plugins::publication::global_publisher;
use crate::plugins::stream::connection_filter_bridge::resolve_route_owner_scope;
use crate::plugins::stream::CompiledStreamPlugins;
use crate::plugins::{StreamContext, StreamLayer, StreamPluginResult};
use crate::routes::common::{
    L4_BACKEND_REF_DENIED_LOG, L4_BACKEND_REF_DENIED_STATUS, L4_BACKEND_UNSUPPORTED_KIND_LOG,
    L4_BACKEND_UNSUPPORTED_KIND_STATUS, UNSAFE_UPSTREAM_ADDRESS_LOG,
};
use crate::routes::l4_duplex::{bidirectional_relay, DuplexError, DuplexHooks};
use crate::routes::tcp::route_unit::TcpRouteUnit;
use crate::routes::tcp::TcpPortRouteManager;
use edgion_resources::ListenerKey;

/// TCP connection context
pub struct TcpContext {
    pub listener_port: u16,
    pub direct_peer_ip: String,
    pub direct_peer_port: u16,
    pub remote_ip: String,
    pub remote_port: u16,
    pub upstream_addr: Option<String>,
    pub start_time: Instant,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub status: TcpStatus,
    pub connection_established: bool,
    /// Bounded reason for a fail-close / policy denial, surfaced into the TCP
    /// access log. Stays `&'static str` (no dynamic content) so it never
    /// carries client-controlled data onto the logging path.
    pub deny_reason: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub enum TcpStatus {
    Success,
    NoRouteMatched,
    NoRuleAvailable,
    PluginDenied,
    NoBackendSelected,
    /// Selected backendRef is Controller-stamped `ref_denied` (no matching
    /// ReferenceGrant). Fail closed before Service lookup or dial.
    RefDenied,
    /// Selected backendRef is not a core Service. Fail closed and keep the
    /// declared WRR share instead of dropping the slot at selector build.
    UnsupportedBackend,
    NoBackendResolved,
    UnsafeUpstreamAddress,
    UpstreamConnectionFailed,
    UpstreamReadError,
    UpstreamWriteError,
    DownstreamReadError,
    DownstreamWriteError,
    /// The bidirectional relay was closed because it stayed idle past the
    /// configured `tcpTimeout.idleTimeout` window.
    IdleTimeout,
}

impl TcpStatus {
    /// Stable string label for this status, used as the TCP access log
    /// `status` field. Decouples the log schema from the `Debug` impl so
    /// renaming a variant for code clarity cannot silently break log
    /// consumers, and avoids a per-connection `format!` allocation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "Success",
            Self::NoRouteMatched => "NoRouteMatched",
            Self::NoRuleAvailable => "NoRuleAvailable",
            Self::PluginDenied => "PluginDenied",
            Self::NoBackendSelected => "NoBackendSelected",
            Self::RefDenied => L4_BACKEND_REF_DENIED_STATUS,
            Self::UnsupportedBackend => L4_BACKEND_UNSUPPORTED_KIND_STATUS,
            Self::NoBackendResolved => "NoBackendResolved",
            Self::UnsafeUpstreamAddress => "UnsafeUpstreamAddress",
            Self::UpstreamConnectionFailed => "UpstreamConnectionFailed",
            Self::UpstreamReadError => "UpstreamReadError",
            Self::UpstreamWriteError => "UpstreamWriteError",
            Self::DownstreamReadError => "DownstreamReadError",
            Self::DownstreamWriteError => "DownstreamWriteError",
            Self::IdleTimeout => "IdleTimeout",
        }
    }
}

/// TCP proxy service for Gateway TCP listeners.
pub struct EdgionTcpProxy {
    pub listener_port: u16,
    pub listener_key: ListenerKey,
    pub tcp_route_manager: Arc<TcpPortRouteManager>,
    pub egc: Arc<crate::config::GatewayConfigHandle>,
    pub connector: TransportConnector,
}

#[async_trait]
impl ServerApp for EdgionTcpProxy {
    async fn process_new(self: &Arc<Self>, downstream: Stream, shutdown: &ShutdownWatch) -> Option<Stream> {
        let gateway_config = self.egc.load();
        if *shutdown.borrow() {
            return None;
        }

        let digest = downstream.get_socket_digest();
        let addrs = crate::util::resolve_transport_client_addrs(
            digest.as_deref().and_then(|d| d.peer_addr()),
            digest.as_deref().and_then(|d| d.raw_peer_addr()),
            "unknown",
        );

        let mut ctx = TcpContext {
            listener_port: self.listener_port,
            direct_peer_ip: addrs.direct_peer_ip,
            direct_peer_port: addrs.direct_peer_port,
            remote_ip: addrs.remote_ip,
            remote_port: addrs.remote_port,
            upstream_addr: None,
            start_time: Instant::now(),
            bytes_sent: 0,
            bytes_received: 0,
            status: TcpStatus::Success,
            connection_established: false,
            deny_reason: None,
        };

        self.handle_connection(downstream, &mut ctx, &gateway_config).await;

        self.log_connection(&ctx).await;

        None
    }
}

impl EdgionTcpProxy {
    async fn handle_connection(
        &self,
        downstream: Stream,
        ctx: &mut TcpContext,
        gateway_config: &ResolvedGatewayConfig,
    ) {
        // 1. Match TCPRoute unit from per-port route table
        let Some(unit) = select_route_unit(&self.tcp_route_manager) else {
            ctx.status = TcpStatus::NoRouteMatched;
            return;
        };
        let tcp_route = unit.route();

        // 2. Require at least one rule. The compiled runtime (backend_finder,
        // stream-plugin store key, keepalive) lives on the unit and is built
        // from the first rule, so a route with no rules cannot serve traffic.
        if tcp_route.spec.rules.as_ref().and_then(|rules| rules.first()).is_none() {
            ctx.status = TcpStatus::NoRuleAvailable;
            return;
        }

        let publication = global_publisher().snapshot();
        let Some((owner, scope)) = resolve_route_owner_scope(
            &publication,
            &self.listener_key,
            unit.resolved_parent_refs(),
            "",
            tcp_route.metadata.namespace.as_deref().unwrap_or("default"),
            "TCPRoute",
        ) else {
            ctx.status = TcpStatus::PluginDenied;
            ctx.deny_reason = Some("plugin policy unavailable");
            return;
        };
        let resource = unit.stream_plugin_store_key().and_then(|key| scope.stream(key));
        // Admission pins only the selected executable view across the await.
        drop(scope);
        drop(publication);
        if let StreamPluginCheck::Deny(reason) = check_stream_plugins(
            unit.stream_plugin_store_key(),
            resource.as_deref(),
            &ctx.direct_peer_ip,
            self.listener_port,
        )
        .await
        {
            ctx.status = TcpStatus::PluginDenied;
            ctx.deny_reason = Some(reason);
            return;
        }

        drop(resource);

        // 4. Select backend
        let Ok(selected) = unit.select_backend() else {
            ctx.status = TcpStatus::NoBackendSelected;
            return;
        };
        let backend =
            match crate::backends::policy::resolve_l4_policy_backend(selected, Some(owner.gateway_key()), |target| {
                select_service_endpoint(
                    target.key(),
                    target.section().clone(),
                    &None,
                    ServiceDegradePolicy::Live,
                    None,
                    "",
                    None,
                )
            }) {
                Ok((_, endpoint)) => endpoint.backend,
                Err(L4ResolveError::Rejected(L4BackendRejection::Denied)) => {
                    ctx.status = TcpStatus::RefDenied;
                    ctx.deny_reason = Some(L4_BACKEND_REF_DENIED_LOG);
                    return;
                }
                Err(L4ResolveError::Rejected(L4BackendRejection::Invalid)) => {
                    ctx.status = TcpStatus::UnsupportedBackend;
                    ctx.deny_reason = Some(L4_BACKEND_UNSUPPORTED_KIND_LOG);
                    return;
                }
                Err(L4ResolveError::Unavailable(_)) => {
                    ctx.status = TcpStatus::NoBackendResolved;
                    return;
                }
            };
        // 6. Build upstream address
        let upstream_addr = &backend.inner().addr;
        let upstream_addr_str = upstream_addr.to_string();
        let outlier_store = crate::backends::get_outlier_detection_store();
        let outlier_token = outlier_store.capture_completion_token(backend.as_ref());
        let upstream_addr_str = ctx.upstream_addr.insert(upstream_addr_str);
        if validate_endpoint_backend_addr(upstream_addr, gateway_config.allow_loopback_upstream).is_err() {
            ctx.status = TcpStatus::UnsafeUpstreamAddress;
            // Mirror the TLS/UDP fail-close access-log fields: surface the
            // shared SSRF-guard literal into `deny_reason` so operator log
            // scraping for "unsafe upstream address" matches on the TCP path
            // too (TCP's `status` enum debug repr is PascalCase and does not
            // contain this string).
            ctx.deny_reason = Some(UNSAFE_UPSTREAM_ADDRESS_LOG);
            return;
        }

        // 7. Connect to upstream
        let mut peer = BasicPeer::new(upstream_addr_str);
        peer.options.tcp_keepalive = unit
            .tcp_keepalive()
            .map(crate::routes::common::to_pingora_tcp_keepalive);
        // Peer options and the selected backend no longer borrow the route generation.
        drop(unit);
        // Bound the connect (TCP handshake) so an unreachable backend fails fast
        // instead of stranding the downstream connection until the OS TCP
        // retransmit timeout; mirrors the HTTP path's connection_timeout.
        peer.options.connection_timeout = gateway_config.l4_timeouts.connect_timeout;
        let Ok(upstream) = self.connector.new_stream(&peer).await else {
            ctx.status = TcpStatus::UpstreamConnectionFailed;
            // L4 upstream connect failure — local-origin failure.
            if let Some(token) = outlier_token.as_ref() {
                outlier_store.record_failure_for_backend_with_token(
                    backend.as_ref(),
                    token,
                    crate::backends::FailureClass::LocalOrigin,
                );
            }
            return;
        };

        ctx.connection_established = true;
        // A successful connect is the L4 "backend reachable" success signal; it
        // resets the endpoint's outlier-detection failure counters.
        if let Some(token) = outlier_token.as_ref() {
            outlier_store.record_success_for_backend_with_token(backend.as_ref(), token);
        }

        // 8. Bidirectional data forwarding
        self.duplex(downstream, upstream, ctx, gateway_config.l4_timeouts.idle_timeout)
            .await;
    }

    async fn duplex(&self, downstream: Stream, upstream: Stream, ctx: &mut TcpContext, idle_timeout: Option<Duration>) {
        bidirectional_relay(downstream, upstream, TcpDuplexHooks { ctx }, idle_timeout).await;
    }

    async fn log_connection(&self, ctx: &TcpContext) {
        let log_entry = TcpLogEntry::from_context(ctx);
        log_tcp(&log_entry).await;
    }
}

/// Retain only the selected unit through admission and preparation, not the full table.
fn select_route_unit(manager: &TcpPortRouteManager) -> Option<Arc<TcpRouteUnit>> {
    manager.load_route_table().match_route()
}

/// [`DuplexHooks`] for the plain TCP proxy.
///
/// - Byte counters increment `TcpContext::bytes_sent` / `bytes_received` directly
///   (TCP has no per-attempt upstream split, unlike TLS).
/// - Errors are recorded as `TcpStatus` enum variants on the context. The
///   `DownstreamFlush` variant intentionally maps to `DownstreamWriteError`,
///   per `DuplexError`'s "callers that do not distinguish flush from write"
///   guidance.
struct TcpDuplexHooks<'a> {
    ctx: &'a mut TcpContext,
}

impl DuplexHooks for TcpDuplexHooks<'_> {
    fn record_sent(&mut self, n: usize) {
        self.ctx.bytes_sent += n as u64;
    }

    fn record_received(&mut self, n: usize) {
        self.ctx.bytes_received += n as u64;
    }

    fn on_error(&mut self, err: DuplexError) {
        self.ctx.status = match err {
            DuplexError::UpstreamWrite => TcpStatus::UpstreamWriteError,
            DuplexError::DownstreamRead => TcpStatus::DownstreamReadError,
            DuplexError::DownstreamWrite => TcpStatus::DownstreamWriteError,
            // DownstreamFlush is a client-side write failure; collapse into DownstreamWriteError.
            DuplexError::DownstreamFlush => TcpStatus::DownstreamWriteError,
            DuplexError::IdleTimeout => TcpStatus::IdleTimeout,
            DuplexError::UpstreamRead => TcpStatus::UpstreamReadError,
        };
    }
}

/// Outcome of section-3 stream plugin enforcement on a TCP connection.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StreamPluginCheck {
    /// Connection may proceed to backend selection.
    Allow,
    /// Connection must be rejected. The caller sets `TcpStatus::PluginDenied`
    /// and records the bounded `&'static str` reason into the access log.
    Deny(&'static str),
}

/// Execute the scoped Stage-1 view after logical Gateway admission.
/// A missing attachment is a no-op; a declared reference without a selected
/// executable view fails closed. The caller owns the availability gate even
/// when no plugin is attached.
pub(crate) async fn check_stream_plugins(
    store_key: Option<&str>,
    resource: Option<&CompiledStreamPlugins>,
    direct_peer_ip: &str,
    listener_port: u16,
) -> StreamPluginCheck {
    let Some(_) = store_key else {
        return StreamPluginCheck::Allow;
    };

    // Fail-close: data plane must not emit tracing on the per-connection hot
    // path. The bounded reason flows into the TCP access log via the caller,
    // mirroring the TLS path's `ctx.err_log` (routes/tls/mod.rs).
    let Ok(direct_peer_ip) = direct_peer_ip.parse() else {
        return StreamPluginCheck::Deny("direct peer IP unparseable");
    };

    let Some(resource) = resource else {
        return StreamPluginCheck::Deny("stream plugin resource not found");
    };

    let runtime = &resource.stream_plugin_runtime;
    if runtime.is_empty() {
        // Empty runtime is operator intent — pass through silently.
        return StreamPluginCheck::Allow;
    }

    let stream_ctx = StreamContext::new(direct_peer_ip, listener_port, StreamLayer::Tcp);
    match runtime.run(&stream_ctx).await {
        StreamPluginResult::Allow => StreamPluginCheck::Allow,
        StreamPluginResult::Deny(_) => StreamPluginCheck::Deny("stream plugin policy denied"),
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/routes/tcp/edgion_tcp_test.rs"]
mod tests;
