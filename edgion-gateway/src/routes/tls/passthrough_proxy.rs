//! TLS Passthrough proxy service.
//!
//! Forwards raw TLS traffic to backends without termination.
//! Extracts SNI from the ClientHello to perform route matching,
//! then transparently relays all bytes (including the original ClientHello)
//! to the selected upstream.

use async_trait::async_trait;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};

use pingora_core::apps::ServerApp;
use pingora_core::connectors::TransportConnector;
use pingora_core::protocols::Stream;
use pingora_core::server::ShutdownWatch;
use pingora_core::upstreams::peer::BasicPeer;

use super::proxy::{TlsContext, TlsUpstreamInfo};
use super::route_unit::TlsRouteUnit;
use super::sni_parser;
use super::TlsDuplexHooks;
use super::TlsRouteManager;
use crate::backends::{
    select_service_endpoint, service_section_key_from_port, validate_endpoint_backend_addr, ServiceDegradePolicy,
};
use crate::observe::log_tls;
use crate::observe::AccessLogger;
use crate::routes::common::UNSAFE_UPSTREAM_ADDRESS_LOG;
use crate::routes::l4_duplex::bidirectional_relay;
use crate::runtime::matching::route::hostname_matches_listener;
use crate::runtime::store::get_listener_gateway_info_store;
use crate::util::proxy_protocol::ProxyProtocolV2Builder;
use crate::{MatchedInfo, Rsc};
use edgion_resources::resources::edgion_gateway_config::EdgionGatewayConfig;
use edgion_resources::ListenerKey;
use edgion_resources::ResourceMeta;

use super::tls_connect_retry_backoff;

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
    pub edgion_gateway_config: Arc<EdgionGatewayConfig>,
    pub connector: TransportConnector,
}

#[async_trait]
impl ServerApp for EdgionTlsPassthroughProxy {
    async fn process_new(self: &Arc<Self>, mut downstream: Stream, shutdown: &ShutdownWatch) -> Option<Stream> {
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
            self.log_disconnect(&mut ctx).await;
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
                self.log_disconnect(&mut ctx).await;
                return None;
            }
        };

        // 2. Extract SNI from the complete record.
        let sni = match sni_parser::extract_sni(&peek_buf[..record_end]) {
            Some(sni) => sni,
            None => {
                ctx.err_log = Some("no SNI in ClientHello".to_string());
                self.log_disconnect(&mut ctx).await;
                return None;
            }
        };
        ctx.sni = Some(sni.clone());

        // 3. Handle connection: route match → connect upstream → relay.
        //    Forward every byte we drained from downstream — not just the
        //    ClientHello record — so any trailing 0-RTT EarlyData reaches
        //    the backend.
        peek_buf.truncate(filled);
        self.handle_connection(downstream, &mut ctx, &sni, peek_buf).await;
        self.log_disconnect(&mut ctx).await;

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
    async fn handle_connection(&self, downstream: Stream, ctx: &mut TlsContext, sni: &str, initial_bytes: Vec<u8>) {
        // 0. Verify SNI is accepted by at least one listener on this port.
        //    Multiple Gateways share the same physical port, so the route table
        //    is a union of all their TLSRoutes. Without this check, an SNI that
        //    only matches a wildcard route from a different Gateway would be
        //    incorrectly forwarded.
        if !self.sni_accepted_by_any_listener(sni) {
            ctx.err_log = Some("no listener accepts SNI".to_string());
            return;
        }

        // 1. Match TLSRoute unit by SNI
        let unit = match select_route_unit(&self.tls_route_manager, sni) {
            Some(unit) => unit,
            None => {
                ctx.err_log = Some("no match".to_string());
                return;
            }
        };
        let tls_route = &unit.route;

        ctx.matched = Some(MatchedInfo {
            rsc: Rsc {
                kind: "TLSRoute".to_string(),
                ns: tls_route.metadata.namespace.clone().unwrap_or_default(),
                name: tls_route.metadata.name.clone().unwrap_or_default(),
                labels: unit.log_labels.clone(),
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

        // 3. Execute TLS route plugins (Stage 2: post-route-match)
        if !super::run_tls_route_plugins(
            unit.stream_plugin_store_key.as_deref(),
            ctx,
            sni,
            self.listener_port,
            super::TlsRouteMode::Passthrough,
        )
        .await
        {
            return;
        }

        // 4. Connect upstream with retry
        let max_attempts = unit.max_connect_retries;
        let mut upstream = None;
        let mut last_err = String::new();

        for attempt in 0..max_attempts {
            let backend_ref = match unit.backend_finder.select() {
                Ok(b) => b,
                Err(_) => {
                    ctx.err_log = Some("no backend".to_string());
                    return;
                }
            };
            if super::apply_tls_l4_post_select(
                ctx,
                backend_ref.ref_denied.as_ref(),
                backend_ref.group.as_deref(),
                backend_ref.kind.as_deref(),
            ) {
                // Authorization or kind miss is terminal: do not retry onto
                // another backend and do not look up or dial the Service.
                return;
            }

            let route_ns = tls_route.metadata.namespace.as_deref().unwrap_or("default");
            let backend_ns = backend_ref.namespace.as_deref().unwrap_or(route_ns);
            let service_key = format!("{}/{}", backend_ns, backend_ref.name);

            let section = match service_section_key_from_port(backend_ref.port) {
                Ok(section) => section,
                Err(_) => {
                    last_err = format!("no proven section: {}", service_key);
                    continue;
                }
            };
            let endpoint =
                match select_service_endpoint(&service_key, section, &None, ServiceDegradePolicy::Live, None, "", None)
                {
                    Ok(endpoint) => endpoint,
                    Err(_) => {
                        last_err = format!("no endpoint: {}", service_key);
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
                        kind: backend_ref.kind.clone().unwrap_or_else(|| "Service".to_string()),
                        ns: backend_ns.to_string(),
                        name: backend_ref.name.clone(),
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
            if validate_endpoint_backend_addr(upstream_addr, self.allow_loopback_upstream()).is_err() {
                ctx.err_log = Some(UNSAFE_UPSTREAM_ADDRESS_LOG.to_string());
                return;
            }

            let mut peer = BasicPeer::new(&upstream_addr_str);
            peer.options.tcp_keepalive = unit
                .tcp_keepalive
                .as_ref()
                .map(crate::routes::common::to_pingora_tcp_keepalive);
            // Bound the connect (TCP handshake; the TLS handshake is end-to-end
            // for passthrough) so an unreachable backend fails fast instead of
            // stranding the downstream connection until the OS retransmit timeout.
            peer.options.connection_timeout =
                edgion_resources::resources::edgion_gateway_config::resolve_tcp_connect_timeout(
                    self.edgion_gateway_config.spec.tcp_timeout.as_ref(),
                );
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
                    last_err = format!("connect failed: {upstream_addr_str}: {e}");
                    if attempt + 1 < max_attempts {
                        ctx.push_log(&format!("retry {}/{max_attempts}: {last_err}", attempt + 1));
                        tokio::time::sleep(tls_connect_retry_backoff(attempt)).await;
                    }
                }
            }
        }

        let (mut upstream_stream, upstream_addr_str) = match upstream {
            Some(pair) => pair,
            None => {
                ctx.err_log = Some(last_err);
                return;
            }
        };

        // 5. Send Proxy Protocol v2 if configured
        if let Some(2) = unit.proxy_protocol_version {
            if let Ok(src_ip) = ctx.remote_ip.parse::<IpAddr>() {
                // Use the already-bracketed upstream_addr_str (SocketAddr Display) for the PP2
                // destination. Re-deriving it from the bare-IP info.addr would drop the IPv6
                // brackets and fail to parse, silently advertising 0.0.0.0:0 — see proxy.rs,
                // which feeds the bracketed string straight into the PP2 builder.
                let src_addr = std::net::SocketAddr::new(src_ip, ctx.remote_port);
                let dst_addr: std::net::SocketAddr = upstream_addr_str.parse().unwrap_or_else(|_| {
                    std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0)
                });
                let mut builder = ProxyProtocolV2Builder::new(src_addr, dst_addr);
                builder.add_authority(sni);
                let pp2_header = builder.build();

                if let Err(e) = upstream_stream.write_all(&pp2_header).await {
                    ctx.err_log = Some(format!("PP2 write failed: {e}"));
                    return;
                }
                if upstream_stream.flush().await.is_err() {
                    ctx.err_log = Some("PP2 flush failed".to_string());
                    return;
                }
                if let Some(info) = ctx.current_upstream_mut() {
                    info.proxy_protocol_sent = true;
                }
                ctx.push_log("PP2 sent");
            }
        }

        // 6. Forward the initial ClientHello bytes already read from downstream
        if !replay_initial_bytes(&mut upstream_stream, ctx, initial_bytes).await {
            return;
        }

        // 7. Bidirectional relay of raw TLS traffic
        self.duplex(downstream, upstream_stream, ctx).await;
    }

    /// Bidirectional data transfer between downstream and upstream.
    async fn duplex(&self, downstream: Stream, upstream: Stream, ctx: &mut TlsContext) {
        let idle = edgion_resources::resources::edgion_gateway_config::resolve_tcp_idle_timeout(
            self.edgion_gateway_config.spec.tcp_timeout.as_ref(),
        );
        bidirectional_relay(downstream, upstream, TlsDuplexHooks { ctx }, idle).await;
    }

    /// Check if any listener on this port accepts the given SNI hostname.
    ///
    /// A listener with `hostname: None` (catch-all) accepts any SNI.
    /// A listener with a concrete or wildcard hostname must match via
    /// `hostname_matches_listener`.
    fn sni_accepted_by_any_listener(&self, sni: &str) -> bool {
        let store = get_listener_gateway_info_store();
        let index = store.get_index(&self.listener_key);
        if index.is_empty() {
            return true;
        }

        for (_, gw_listeners) in index.iter_gateways() {
            for entry in gw_listeners.iter() {
                match entry.hostname.as_deref() {
                    None => return true,
                    Some(h) if hostname_matches_listener(sni, h) => return true,
                    _ => {}
                }
            }
        }

        false
    }

    fn is_tls_proxy_log_enabled(&self) -> bool {
        self.edgion_gateway_config
            .spec
            .security_protect
            .as_ref()
            .is_none_or(|sp| sp.tls_proxy_log_record)
    }

    fn allow_loopback_upstream(&self) -> bool {
        self.edgion_gateway_config
            .spec
            .security_protect
            .as_ref()
            .map(|s| s.allow_loopback_upstream)
            .unwrap_or(false)
    }

    async fn log_disconnect(&self, ctx: &mut TlsContext) {
        if !self.is_tls_proxy_log_enabled() {
            return;
        }
        ctx.end_time = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
        );
        log_tls(ctx).await;

        let direct_peer_ip = edgion_resources::sensitive::apply_sensitive(&ctx.direct_peer_ip, str::to_owned);
        let remote_ip = edgion_resources::sensitive::apply_sensitive(&ctx.remote_ip, str::to_owned);
        let log_entry = serde_json::json!({
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
        });
        self.access_logger.send(log_entry.to_string()).await;
    }
}

/// Keep only the selected generation, never a table guard across connection I/O.
fn select_route_unit(manager: &TlsRouteManager, sni: &str) -> Option<Arc<TlsRouteUnit>> {
    manager.load_route_table().match_route(sni)
}

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
mod tests {
    use super::*;
    use edgion_resources::resources::tls_route::{TLSRoute, TLSRouteSpec};
    use kube::api::ObjectMeta;
    use std::collections::HashMap;
    use std::io;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    fn route_unit(name: &str, hostname: &str) -> Arc<TlsRouteUnit> {
        Arc::new(TlsRouteUnit::from_route(Arc::new(TLSRoute {
            metadata: ObjectMeta {
                namespace: Some("default".to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: TLSRouteSpec {
                parent_refs: None,
                hostnames: Some(vec![hostname.to_string()]),
                rules: None,
                resolved_listeners: None,
                effective_hostnames: None,
            },
            status: None,
        })))
    }

    #[test]
    fn selected_unit_survives_reload_without_retaining_old_table() {
        let manager = TlsRouteManager::new();
        assert!(select_route_unit(&manager, "a.example.com").is_none());
        let old_unit = route_unit("a", "a.example.com");
        let sibling = route_unit("b", "b.example.com");
        let old_unit_weak = Arc::downgrade(&old_unit);
        let selector_weak = Arc::downgrade(&old_unit.backend_finder);
        let sibling_weak = Arc::downgrade(&sibling);
        manager.rebuild(&HashMap::from([
            ("default/a".to_string(), old_unit),
            ("default/b".to_string(), sibling),
        ]));
        let old_table = Arc::downgrade(&manager.load_route_table());
        assert!(select_route_unit(&manager, "missing.example.com").is_none());
        let selected = select_route_unit(&manager, "A.EXAMPLE.COM").unwrap();
        assert!(Arc::ptr_eq(&selected, &old_unit_weak.upgrade().unwrap()));
        assert!(select_route_unit(&manager, "b.example.com").is_some());

        let new_unit = route_unit("a", "a.example.com");
        manager.rebuild(&HashMap::from([("default/a".to_string(), new_unit.clone())]));
        assert!(old_table.upgrade().is_none(), "selection must release the table guard");
        assert!(
            sibling_weak.upgrade().is_none(),
            "unselected hostname winner is released"
        );
        assert!(Arc::ptr_eq(&selected.backend_finder, &selector_weak.upgrade().unwrap()));
        assert!(Arc::ptr_eq(
            &select_route_unit(&manager, "a.example.com").unwrap(),
            &new_unit
        ));
        assert!(!Arc::ptr_eq(&selected, &new_unit));
        assert!(select_route_unit(&manager, "b.example.com").is_none());
        drop(selected);
        assert!(old_unit_weak.upgrade().is_none());
        assert!(selector_weak.upgrade().is_none());
    }

    fn context() -> TlsContext {
        let mut ctx = TlsContext::new(8443, "192.0.2.1".to_string(), 1234, "192.0.2.1".to_string(), 1234);
        ctx.upstream_info.push(TlsUpstreamInfo {
            addr: None,
            port: None,
            backend: None,
            bytes_sent: 7,
            bytes_received: 0,
            connect_time: None,
            connection_established: true,
            proxy_protocol_sent: false,
            upstream_protocol: None,
        });
        ctx
    }

    #[derive(Default)]
    struct ReplayWriter {
        bytes: Vec<u8>,
        fail_after: Option<usize>,
        fail_flush: bool,
        flushes: usize,
    }

    impl AsyncWrite for ReplayWriter {
        fn poll_write(mut self: Pin<&mut Self>, _: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
            let remaining = self.fail_after.unwrap_or(usize::MAX).saturating_sub(self.bytes.len());
            if remaining == 0 {
                return Poll::Ready(Err(io::Error::other("replay write failed")));
            }
            let count = bytes.len().min(3).min(remaining);
            self.bytes.extend_from_slice(&bytes[..count]);
            Poll::Ready(Ok(count))
        }

        fn poll_flush(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.flushes += 1;
            Poll::Ready(if self.fail_flush {
                Err(io::Error::other("replay flush failed"))
            } else {
                Ok(())
            })
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn replay_forwards_record_and_trailing_bytes_once_before_accounting() {
        // A complete handshake record plus a following application-data record.
        let bytes = [0x16, 3, 3, 0, 1, 1, 0x17, 3, 3, 0, 2, 42, 43];
        let mut initial = vec![0; sni_parser::MAX_CLIENT_HELLO_LEN];
        initial[..bytes.len()].copy_from_slice(&bytes);
        initial.truncate(bytes.len());
        assert!(matches!(
            sni_parser::client_hello_record_complete(&initial),
            sni_parser::RecordCompleteness::Complete { record_end: 6 }
        ));
        let mut writer = ReplayWriter::default();
        let mut ctx = context();
        assert!(replay_initial_bytes(&mut writer, &mut ctx, initial).await);
        assert_eq!(writer.bytes, bytes);
        assert_eq!(writer.flushes, 1);
        assert_eq!(ctx.upstream_info[0].bytes_sent, 7 + bytes.len() as u64);
        assert!(ctx.err_log.is_none());
    }

    #[tokio::test]
    async fn replay_write_failure_is_terminal_without_flush_or_accounting() {
        let mut writer = ReplayWriter {
            fail_after: Some(4),
            ..Default::default()
        };
        let mut ctx = context();
        assert!(!replay_initial_bytes(&mut writer, &mut ctx, b"initial bytes".to_vec()).await);
        assert_eq!(writer.bytes, b"init");
        assert_eq!(writer.flushes, 0);
        assert_eq!(ctx.upstream_info[0].bytes_sent, 7);
        assert_eq!(
            ctx.err_log.as_deref(),
            Some("upstream write initial bytes failed: replay write failed")
        );
    }

    #[tokio::test]
    async fn replay_flush_failure_is_terminal_without_accounting() {
        let mut writer = ReplayWriter {
            fail_flush: true,
            ..Default::default()
        };
        let mut ctx = context();
        assert!(!replay_initial_bytes(&mut writer, &mut ctx, b"initial bytes".to_vec()).await);
        assert_eq!(writer.bytes, b"initial bytes");
        assert_eq!(writer.flushes, 1);
        assert_eq!(ctx.upstream_info[0].bytes_sent, 7);
        assert_eq!(ctx.err_log.as_deref(), Some("upstream flush initial bytes failed"));
    }
}
