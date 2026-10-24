use crate::plugins::publication::global_publisher;
use async_trait::async_trait;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncWrite, AsyncWriteExt};

use pingora_core::apps::ServerApp;
use pingora_core::connectors::TransportConnector;
use pingora_core::protocols::Stream;
use pingora_core::server::ShutdownWatch;
#[cfg(feature = "boringssl")]
use pingora_core::tls::ssl::NameType;
use pingora_core::upstreams::peer::BasicPeer;

use crate::backends::{select_service_endpoint, validate_endpoint_backend_addr, ServiceDegradePolicy};
use crate::config::ResolvedGatewayConfig;
use crate::observe::log_tls;
use crate::observe::logs::LogBuffer;
use crate::observe::AccessLogger;
use crate::routes::common::UNSAFE_UPSTREAM_ADDRESS_LOG;
use crate::routes::l4_duplex::bidirectional_relay;
use crate::routes::tls::TlsRouteManager;
#[cfg(test)]
use crate::routes::tls::TlsRouteUnit;
use crate::util::proxy_protocol::ProxyProtocolV2Builder;
use crate::{MatchedInfo, Rsc, TlsConnMeta};
use edgion_resources::{ListenerKey, ResourceMeta};
use serde::Serialize;

use super::{tls_connect_retry_delay, TlsDuplexHooks};

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

#[derive(Debug, Clone, Serialize)]
pub struct TlsUpstreamInfo {
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "edgion_resources::sensitive::masked_opt::serialize"
    )]
    pub addr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<MatchedInfo>,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub bytes_sent: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub bytes_received: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_time: Option<i64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub connection_established: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub proxy_protocol_sent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_protocol: Option<String>,
}

/// TLS connection context
#[derive(Debug, Clone, Serialize)]
pub struct TlsContext {
    pub ts: i64,
    #[serde(skip_serializing)]
    pub start_at: Instant,
    pub listener_port: u16,
    #[serde(serialize_with = "edgion_resources::sensitive::masked::serialize")]
    pub direct_peer_ip: String,
    pub direct_peer_port: u16,
    #[serde(serialize_with = "edgion_resources::sensitive::masked::serialize")]
    pub remote_ip: String,
    pub remote_port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<MatchedInfo>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_mtls: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_time: Option<i64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub upstream_info: Vec<TlsUpstreamInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<LogBuffer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub err_log: Option<String>,
    /// Structured failure token (`RefDenied`, `UnsupportedBackend`, …).
    /// Complements the free-form `err_log` so operator pipelines can match
    /// TLS the same way they match TCP `status`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<&'static str>,
}

impl TlsContext {
    pub(super) fn new(
        listener_port: u16,
        direct_peer_ip: String,
        direct_peer_port: u16,
        remote_ip: String,
        remote_port: u16,
    ) -> Self {
        Self {
            ts: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
            start_at: Instant::now(),
            listener_port,
            direct_peer_ip,
            direct_peer_port,
            remote_ip,
            remote_port,
            tls_id: None,
            sni: None,
            matched: None,
            is_mtls: false,
            end_time: None,
            upstream_info: Vec::new(),
            log: None,
            err_log: None,
            status: None,
        }
    }

    pub(super) fn push_log(&mut self, message: &str) {
        let _ = self.log.get_or_insert_with(LogBuffer::new).push(message);
    }

    pub(super) fn current_upstream_mut(&mut self) -> Option<&mut TlsUpstreamInfo> {
        self.upstream_info.last_mut()
    }
}

/// Send a Proxy Protocol v2 header to an established upstream when enabled.
/// Returns `false` if the write or flush fails and the caller must abort.
pub(super) async fn send_proxy_protocol_v2<W: AsyncWrite + Unpin>(
    proxy_protocol_version: Option<u8>,
    ctx: &mut TlsContext,
    upstream_addr_str: &str,
    upstream: &mut W,
) -> bool {
    let Some(2) = proxy_protocol_version else {
        return true;
    };
    let Ok(src_ip) = ctx.remote_ip.parse::<IpAddr>() else {
        return true;
    };

    let src_addr = std::net::SocketAddr::new(src_ip, ctx.remote_port);
    let dst_addr: std::net::SocketAddr = upstream_addr_str
        .parse()
        .unwrap_or_else(|_| std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0));
    let pp2_header = {
        let mut builder = ProxyProtocolV2Builder::new(src_addr, dst_addr);
        builder.add_authority(ctx.sni.as_deref().unwrap_or_default());
        builder.build()
    };

    if let Err(e) = upstream.write_all(&pp2_header).await {
        ctx.err_log = Some(format!("PP2 write failed: {e}"));
        return false;
    }
    drop(pp2_header);
    if upstream.flush().await.is_err() {
        ctx.err_log = Some("PP2 flush failed".to_string());
        return false;
    }
    if let Some(info) = ctx.current_upstream_mut() {
        info.proxy_protocol_sent = true;
    }
    ctx.push_log("PP2 sent");
    true
}

/// TLS-to-TCP proxy service for Gateway TLS listeners.
///
/// Each instance holds an `Arc<TlsRouteManager>` for its listener,
/// obtained at startup from `GlobalTlsRouteManagers`. Route updates swap
/// only the inner `ArcSwap<TlsRouteTable>`, so the Arc stays stable.
pub struct EdgionTlsTcpProxy {
    pub listener_port: u16,
    pub listener_key: ListenerKey,
    pub tls_route_manager: Arc<TlsRouteManager>,
    pub access_logger: Arc<AccessLogger>,
    pub egc: Arc<crate::config::GatewayConfigHandle>,
    pub connector: TransportConnector,
}

#[async_trait]
impl ServerApp for EdgionTlsTcpProxy {
    async fn process_new(self: &Arc<Self>, mut downstream: Stream, shutdown: &ShutdownWatch) -> Option<Stream> {
        let gateway_config = self.egc.load();
        let digest = downstream.get_socket_digest();
        let addrs = crate::util::resolve_transport_client_addrs(
            digest.as_deref().and_then(|d| d.peer_addr()),
            digest.as_deref().and_then(|d| d.raw_peer_addr()),
            "unknown",
        );

        let mut ctx = TlsContext::new(
            self.listener_port,
            addrs.direct_peer_ip,
            addrs.direct_peer_port,
            addrs.remote_ip,
            addrs.remote_port,
        );

        if *shutdown.borrow() {
            ctx.err_log = Some("shutdown".to_string());
            self.log_disconnect(&mut ctx, gateway_config.tls_proxy_log_record).await;
            return None;
        }

        // Borrow handshake metadata only while installing the connection fields.
        {
            let ssl_digest = downstream.get_ssl_digest();
            install_tls_metadata(
                &mut ctx,
                ssl_digest.as_ref().and_then(|d| d.extension.get::<TlsConnMeta>()),
            );
        }
        if ctx.sni.is_none() {
            ctx.sni = Self::extract_sni(&mut downstream);
        }

        if ctx.sni.is_none() {
            ctx.err_log = Some("no SNI".to_string());
            self.log_disconnect(&mut ctx, gateway_config.tls_proxy_log_record).await;
            return None;
        }

        self.handle_connection(downstream, &mut ctx, &gateway_config).await;
        self.log_disconnect(&mut ctx, gateway_config.tls_proxy_log_record).await;

        None
    }
}

impl EdgionTlsTcpProxy {
    /// Extract SNI hostname from TLS stream
    fn extract_sni(#[allow(unused_variables)] stream: &mut Stream) -> Option<String> {
        #[cfg(feature = "boringssl")]
        {
            if let Some(ssl_ref) = stream.get_ssl() {
                if let Some(sni) = ssl_ref.servername(NameType::HOST_NAME) {
                    return Some(sni.to_string());
                }
            }
        }
        None
    }

    /// Core logic for handling TLS-terminated connections
    async fn handle_connection(
        &self,
        downstream: Stream,
        ctx: &mut TlsContext,
        gateway_config: &ResolvedGatewayConfig,
    ) {
        // 1. Match TLSRoute unit by SNI from the per-listener route table.
        let publication = global_publisher().snapshot();
        let Some(selected) = super::select_tls_route(
            &self.tls_route_manager,
            &publication,
            &self.listener_key,
            ctx.sni.as_deref().unwrap_or_default(),
        ) else {
            ctx.err_log = Some("no match".to_string());
            return;
        };
        drop(publication);
        let unit = selected.unit;
        let tls_route = unit.route();

        ctx.matched = Some(MatchedInfo {
            rsc: Rsc {
                kind: "TLSRoute".to_string(),
                ns: tls_route.metadata.namespace.clone().unwrap_or_default(),
                name: tls_route.metadata.name.clone().unwrap_or_default(),
                labels: unit.log_labels().cloned(),
            },
            section: None,
            sv: tls_route.get_sync_version(),
        });

        // 2. Require at least one rule. The compiled runtime (backend_finder,
        // retries, keepalive, proxy-protocol, stream-plugin key) lives on the
        // unit and is built from the first rule.
        if tls_route.spec.rules.as_ref().and_then(|rules| rules.first()).is_none() {
            ctx.err_log = Some("no rules".to_string());
            return;
        }

        let Some(scope) = selected.scope else {
            ctx.err_log = Some("plugin policy unavailable".to_string());
            return;
        };
        let resource = unit.stream_plugin_store_key().and_then(|key| scope.stream(key));
        drop(scope);
        if !super::run_tls_route_plugins(
            unit.stream_plugin_store_key(),
            resource.as_deref(),
            ctx,
            self.listener_port,
            super::TlsRouteMode::Terminated,
        )
        .await
        {
            return;
        }

        drop(resource);

        // 4. Connect upstream with retry
        let max_attempts = unit.max_connect_retries();
        let mut upstream = None;
        let mut last_err = String::new();

        for attempt in 0..max_attempts {
            // 4a. Select backend (re-select on each retry for load-balancing rotation)
            let (target, backend) =
                match super::resolve_tls_backend_attempt(&unit, ctx, Some(selected.owner.gateway_key()), |target| {
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
                    super::TlsBackendAttempt::Ready(target, endpoint) => (target, endpoint.backend),
                    super::TlsBackendAttempt::Terminal => return,
                    super::TlsBackendAttempt::Retryable(target) => {
                        last_err = format!("no endpoint: {}", target.key());
                        continue;
                    }
                };
            let upstream_addr = &backend.inner().addr;
            let upstream_addr_str = upstream_addr.to_string();
            let outlier_store = crate::backends::get_outlier_detection_store();
            let outlier_token = outlier_store.capture_completion_token(backend.as_ref());
            let parsed_upstream_addr: Option<std::net::SocketAddr> = upstream_addr_str.parse().ok();

            ctx.upstream_info.push(TlsUpstreamInfo {
                addr: parsed_upstream_addr.as_ref().map(|addr| addr.ip().to_string()),
                port: parsed_upstream_addr.as_ref().map(|addr| addr.port()),
                backend: Some(MatchedInfo {
                    rsc: Rsc {
                        kind: "Service".to_string(),
                        ns: target.namespace().to_string(),
                        name: target.name().to_string(),
                        labels: None,
                    },
                    section: None,
                    sv: 0,
                }),
                bytes_sent: 0,
                bytes_received: 0,
                connect_time: None,
                connection_established: false,
                proxy_protocol_sent: false,
                upstream_protocol: Some("TCP".to_string()),
            });
            if validate_endpoint_backend_addr(upstream_addr, gateway_config.allow_loopback_upstream).is_err() {
                ctx.err_log = Some(UNSAFE_UPSTREAM_ADDRESS_LOG.to_string());
                return;
            }

            // 4c. TCP connect
            let mut peer = BasicPeer::new(&upstream_addr_str);
            peer.options.tcp_keepalive = unit
                .tcp_keepalive()
                .map(crate::routes::common::to_pingora_tcp_keepalive);
            // Bound the upstream TCP connect so an unreachable backend fails
            // fast instead of stranding the downstream connection until the OS
            // TCP retransmit timeout; mirrors the HTTP path. The upstream connect
            // is plain TCP here (downstream TLS is terminated separately), so this
            // bounds the connect itself, not a TLS handshake.
            peer.options.connection_timeout = gateway_config.l4_timeouts.connect_timeout;
            match self.connector.new_stream(&peer).await {
                Ok(stream) => {
                    if let Some(info) = ctx.current_upstream_mut() {
                        info.connect_time = Some(
                            SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as i64,
                        );
                        info.connection_established = true;
                    }
                    // A successful connect is the L4 "backend reachable" success
                    // signal; it resets the endpoint's outlier-detection counters.
                    if let Some(token) = outlier_token.as_ref() {
                        outlier_store.record_success_for_backend_with_token(backend.as_ref(), token);
                    }
                    upstream = Some((stream, upstream_addr_str));
                    break;
                }
                Err(e) => {
                    // TLS upstream connect failure — local-origin failure.
                    if let Some(token) = outlier_token.as_ref() {
                        outlier_store.record_failure_for_backend_with_token(
                            backend.as_ref(),
                            token,
                            crate::backends::FailureClass::LocalOrigin,
                        );
                    }
                    last_err = super::format_connect_error(&upstream_addr_str, &e);
                    if let Some(delay) = tls_connect_retry_delay(attempt, max_attempts) {
                        ctx.push_log(&format!("retry {}/{max_attempts}: {last_err}", attempt + 1));
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        let Some((mut upstream_stream, upstream_addr_str)) = upstream else {
            ctx.err_log = Some(last_err);
            return;
        };
        drop(last_err);
        let proxy_protocol_version = unit.proxy_protocol_version();
        // Retries are complete; setup and relay no longer borrow the route generation.
        drop(unit);

        // 5. Send Proxy Protocol v2 header if configured
        if !send_proxy_protocol_v2(proxy_protocol_version, ctx, &upstream_addr_str, &mut upstream_stream).await {
            return;
        }

        drop(upstream_addr_str);

        // 6. Bidirectional data forwarding
        self.duplex(
            downstream,
            upstream_stream,
            ctx,
            gateway_config.l4_timeouts.idle_timeout,
        )
        .await
    }

    /// Bidirectional data transfer between downstream (TLS-terminated) and upstream (TCP)
    async fn duplex(&self, downstream: Stream, upstream: Stream, ctx: &mut TlsContext, idle_timeout: Option<Duration>) {
        bidirectional_relay(downstream, upstream, TlsDuplexHooks { ctx }, idle_timeout).await;
    }

    /// Log connection disconnection event
    async fn log_disconnect(&self, ctx: &mut TlsContext, enabled: bool) {
        if !enabled {
            return;
        }
        ctx.end_time = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
        );
        log_tls(ctx).await;

        // Also emit to the per-listener access logger
        if self.access_logger.sender_count() != 0 {
            self.access_logger.send(Self::listener_log_entry(ctx).to_string()).await;
        }
    }

    fn listener_log_entry(ctx: &TlsContext) -> serde_json::Value {
        let direct_peer_ip = edgion_resources::sensitive::apply_sensitive(&ctx.direct_peer_ip, str::to_owned);
        let remote_ip = edgion_resources::sensitive::apply_sensitive(&ctx.remote_ip, str::to_owned);
        serde_json::json!({
            "ts": ctx.ts,
            "listener_port": ctx.listener_port,
            "direct_peer_ip": direct_peer_ip,
            "direct_peer_port": ctx.direct_peer_port,
            "remote_ip": remote_ip,
            "remote_port": ctx.remote_port,
            "tls_id": &ctx.tls_id,
            "sni": &ctx.sni,
            "end_time": ctx.end_time,
            "upstream_info": &ctx.upstream_info,
            "log": &ctx.log,
            "err_log": &ctx.err_log,
            "status": ctx.status,
        })
    }
}

#[cfg(test)]
include!("../../../tests/support/routes/tls/proxy_test/514_function_item.rs");

/// Copy only the fields used by the TLS connection, not the full handshake record.
fn install_tls_metadata(ctx: &mut TlsContext, meta: Option<&TlsConnMeta>) {
    if let Some(meta) = meta {
        ctx.tls_id = meta.tls_id.clone();
        ctx.sni = meta.sni.clone();
        ctx.matched = meta.matched.clone();
        ctx.is_mtls = meta.is_mtls;
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/routes/tls/proxy_test.rs"]
mod tests;
