//! TLS Passthrough proxy service.
//!
//! Forwards raw TLS traffic to backends without termination.
//! Extracts SNI from the ClientHello to perform route matching,
//! then transparently relays all bytes (including the original ClientHello)
//! to the selected upstream.

use crate::config::ResolvedGatewayConfig;
use crate::plugins::publication::global_publisher;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};

use pingora_core::apps::ServerApp;
use pingora_core::connectors::TransportConnector;
use pingora_core::protocols::Stream;
use pingora_core::server::ShutdownWatch;
use pingora_core::upstreams::peer::BasicPeer;

use super::proxy::{send_proxy_protocol_v2, TlsContext, TlsUpstreamInfo};
#[cfg(test)]
use super::route_unit::TlsRouteUnit;
use super::sni_parser;
use super::TlsDuplexHooks;
use super::TlsRouteManager;
use crate::backends::{select_service_endpoint, validate_endpoint_backend_addr, ServiceDegradePolicy};
use crate::observe::log_tls;
use crate::observe::AccessLogger;
use crate::routes::common::UNSAFE_UPSTREAM_ADDRESS_LOG;
use crate::routes::l4_duplex::bidirectional_relay;
use crate::{MatchedInfo, Rsc};
use edgion_resources::ListenerKey;
use edgion_resources::ResourceMeta;

use super::tls_connect_retry_delay;

/// Wall-clock budget for accumulating a complete TLS ClientHello record from
/// the downstream. Aligned with Envoy's `listener_filters_timeout` default
/// (15 s). Using an absolute deadline (rather than per-read relative timeouts)
/// blocks slowloris-style dribble: an attacker feeding one byte at a time
/// cannot push the cumulative wait past this bound.
const SNI_READ_DEADLINE: Duration = Duration::from_secs(15);

/// TLS Passthrough proxy service.
///
/// Binds as a plain TCP listener. On each connection:
/// 1. Reads the first bytes to extract SNI from the TLS ClientHello
/// 2. Matches a TLSRoute by SNI hostname
/// 3. Connects to the selected backend over plain TCP
/// 4. Relays all bytes bidirectionally (the TLS session is between client and backend)
pub struct EdgionTlsPassthroughProxy {
    pub listener_port: u16,
    pub listener_key: ListenerKey,
    pub tls_route_manager: Arc<TlsRouteManager>,
    pub access_logger: Arc<AccessLogger>,
    pub egc: Arc<crate::config::GatewayConfigHandle>,
    pub connector: TransportConnector,
}

#[async_trait]
impl ServerApp for EdgionTlsPassthroughProxy {
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

        // 1. Read the beginning of the stream to extract SNI from ClientHello.
        //    We read into a bounded buffer with an absolute deadline. SNI is
        //    parsed from exactly the ClientHello record; all bytes consumed
        //    from downstream (which may include 0-RTT EarlyData appended by
        //    the client in the same TCP segment) are forwarded verbatim so
        //    upstream sees the original byte stream.
        let mut peek_buf = vec![0u8; sni_parser::MAX_CLIENT_HELLO_LEN];
        let (record_end, filled) = match self.read_client_hello(&mut downstream, &mut peek_buf).await {
            Ok(spans) => spans,
            Err(err) => {
                ctx.err_log = Some(err);
                self.log_disconnect(&mut ctx, gateway_config.tls_proxy_log_record).await;
                return None;
            }
        };

        // 2. Extract SNI from the complete record.
        let Some(sni) = sni_parser::extract_sni(&peek_buf[..record_end]) else {
            ctx.err_log = Some("no SNI in ClientHello".to_string());
            self.log_disconnect(&mut ctx, gateway_config.tls_proxy_log_record).await;
            return None;
        };
        ctx.sni = Some(sni.clone());

        // 3. Handle connection: route match → connect upstream → relay.
        //    Forward every byte we drained from downstream — not just the
        //    ClientHello record — so any trailing 0-RTT EarlyData reaches
        //    the backend.
        peek_buf.truncate(filled);
        self.handle_connection(downstream, &mut ctx, &sni, peek_buf, &gateway_config)
            .await;
        self.log_disconnect(&mut ctx, gateway_config.tls_proxy_log_record).await;

        None
    }
}

impl EdgionTlsPassthroughProxy {
    /// Accumulate bytes from `downstream` until a complete TLS Handshake
    /// record is buffered.
    ///
    /// Returns `(record_end, filled)`:
    /// - `record_end` is the offset just past the ClientHello record;
    ///   `buf[..record_end]` is the exact record handed to the SNI parser.
    /// - `filled` is the total bytes drained from downstream
    ///   (`filled >= record_end`); the caller forwards `buf[..filled]` so
    ///   any trailing bytes the client coalesced into the same TCP segment
    ///   (e.g., TLS 1.3 0-RTT EarlyData) reach upstream verbatim.
    ///
    /// Closes the connection (returning an `Err` log string) when:
    /// - the peer disconnects before a full record arrives,
    /// - the record would exceed `buf.len()` (`MAX_CLIENT_HELLO_LEN`),
    /// - the cumulative read time exceeds [`SNI_READ_DEADLINE`],
    /// - the first byte already disproves a TLS Handshake record.
    async fn read_client_hello(&self, downstream: &mut Stream, buf: &mut [u8]) -> Result<(usize, usize), String> {
        let deadline = tokio::time::Instant::now() + SNI_READ_DEADLINE;
        let mut filled: usize = 0;

        loop {
            match sni_parser::client_hello_record_complete(&buf[..filled]) {
                sni_parser::RecordCompleteness::Complete { record_end } => {
                    return Ok((record_end, filled));
                }
                sni_parser::RecordCompleteness::NotTlsHandshake => {
                    return Err("not a TLS handshake".to_string());
                }
                sni_parser::RecordCompleteness::NeedMore => {}
            }

            if filled >= buf.len() {
                return Err("ClientHello exceeds MAX_CLIENT_HELLO_LEN".to_string());
            }

            let read_fut = downstream.read(&mut buf[filled..]);
            let n = match tokio::time::timeout_at(deadline, read_fut).await {
                Ok(Ok(0)) => {
                    return Err(if filled == 0 {
                        "connection closed before ClientHello".to_string()
                    } else {
                        "connection closed mid-ClientHello".to_string()
                    });
                }
                Ok(Ok(n)) => n,
                Ok(Err(e)) => return Err(format!("read ClientHello failed: {e}")),
                Err(_) => return Err("ClientHello read timeout".to_string()),
            };
            filled += n;
        }
    }

    /// Route match, connect upstream, and relay traffic.
    ///
    /// `initial_bytes` contains the raw ClientHello bytes already read from
    /// downstream. These must be forwarded to upstream before starting the
    /// bidirectional relay.
    async fn handle_connection(
        &self,
        downstream: Stream,
        ctx: &mut TlsContext,
        sni: &str,
        initial_bytes: Vec<u8>,
        gateway_config: &ResolvedGatewayConfig,
    ) {
        // 1. Match TLSRoute unit by SNI
        let publication = global_publisher().snapshot();
        let Some(selected) = super::select_tls_route(&self.tls_route_manager, &publication, &self.listener_key, sni)
        else {
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

        // 2. Require at least one rule. The compiled runtime lives on the unit
        // and is built from the first rule.
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
            sni,
            self.listener_port,
            super::TlsRouteMode::Passthrough,
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
            let (target, endpoint) = match super::resolve_tls_backend_attempt(&unit, ctx, |target| {
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
                super::TlsBackendAttempt::Ready(target, endpoint) => (target, endpoint),
                super::TlsBackendAttempt::Terminal => return,
                super::TlsBackendAttempt::Retryable(target) => {
                    last_err = format!("no endpoint: {}", target.key());
                    continue;
                }
            };
            let backend = endpoint.backend;

            let upstream_addr = &backend.inner().addr;
            let upstream_addr_str = upstream_addr.to_string();
            let outlier_store = crate::backends::get_outlier_detection_store();
            let outlier_token = outlier_store.capture_completion_token(backend.as_ref());
            let parsed_upstream_addr: Option<std::net::SocketAddr> = upstream_addr_str.parse().ok();

            ctx.upstream_info.push(TlsUpstreamInfo {
                addr: parsed_upstream_addr.as_ref().map(|a| a.ip().to_string()),
                port: parsed_upstream_addr.as_ref().map(|a| a.port()),
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
                upstream_protocol: Some("TLS-Passthrough".to_string()),
            });
            if validate_endpoint_backend_addr(upstream_addr, gateway_config.allow_loopback_upstream).is_err() {
                ctx.err_log = Some(UNSAFE_UPSTREAM_ADDRESS_LOG.to_string());
                return;
            }

            let mut peer = BasicPeer::new(&upstream_addr_str);
            peer.options.tcp_keepalive = unit
                .tcp_keepalive()
                .map(crate::routes::common::to_pingora_tcp_keepalive);
            // Bound the connect (TCP handshake; the TLS handshake is end-to-end
            // for passthrough) so an unreachable backend fails fast instead of
            // stranding the downstream connection until the OS retransmit timeout.
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
                    // TLS passthrough upstream connect failure — local-origin failure.
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

        // 5. Send Proxy Protocol v2 if configured
        if !send_proxy_protocol_v2(
            unit.proxy_protocol_version(),
            ctx,
            sni,
            &upstream_addr_str,
            &mut upstream_stream,
        )
        .await
        {
            return;
        }

        // 6. Forward the initial ClientHello bytes already read from downstream
        if !replay_initial_bytes(&mut upstream_stream, ctx, initial_bytes).await {
            return;
        }

        // 7. Bidirectional relay of raw TLS traffic
        self.duplex(
            downstream,
            upstream_stream,
            ctx,
            gateway_config.l4_timeouts.idle_timeout,
        )
        .await;
    }

    /// Bidirectional data transfer between downstream and upstream.
    async fn duplex(&self, downstream: Stream, upstream: Stream, ctx: &mut TlsContext, idle_timeout: Option<Duration>) {
        bidirectional_relay(downstream, upstream, TlsDuplexHooks { ctx }, idle_timeout).await;
    }

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

        self.access_logger.send(Self::listener_log_entry(ctx).to_string()).await;
    }

    fn listener_log_entry(ctx: &TlsContext) -> serde_json::Value {
        let direct_peer_ip = edgion_resources::sensitive::apply_sensitive(&ctx.direct_peer_ip, str::to_owned);
        let remote_ip = edgion_resources::sensitive::apply_sensitive(&ctx.remote_ip, str::to_owned);
        serde_json::json!({
            "ts": ctx.ts,
            "mode": "passthrough",
            "listener_port": ctx.listener_port,
            "direct_peer_ip": direct_peer_ip,
            "direct_peer_port": ctx.direct_peer_port,
            "remote_ip": remote_ip,
            "remote_port": ctx.remote_port,
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
include!("../../../tests/support/routes/tls/passthrough_proxy_test/432_function_item.rs");

/// Consume the inspection buffer so its allocation is released before relay.
async fn replay_initial_bytes<W: AsyncWrite + Unpin>(
    upstream: &mut W,
    ctx: &mut TlsContext,
    initial_bytes: Vec<u8>,
) -> bool {
    if let Err(e) = upstream.write_all(&initial_bytes).await {
        ctx.err_log = Some(format!("upstream write initial bytes failed: {e}"));
        return false;
    }
    if upstream.flush().await.is_err() {
        ctx.err_log = Some("upstream flush initial bytes failed".to_string());
        return false;
    }
    if let Some(info) = ctx.current_upstream_mut() {
        info.bytes_sent += initial_bytes.len() as u64;
    }
    drop(initial_bytes);
    true
}

#[cfg(test)]
#[path = "../../../tests/unit/routes/tls/passthrough_proxy_test.rs"]
mod tests;
