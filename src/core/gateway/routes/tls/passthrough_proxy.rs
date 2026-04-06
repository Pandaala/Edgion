//! TLS Passthrough proxy service.
//!
//! Forwards raw TLS traffic to backends without termination.
//! Extracts SNI from the ClientHello to perform route matching,
//! then transparently relays all bytes (including the original ClientHello)
//! to the selected upstream.

use async_trait::async_trait;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use pingora_core::apps::ServerApp;
use pingora_core::connectors::TransportConnector;
use pingora_core::protocols::Stream;
use pingora_core::server::ShutdownWatch;
use pingora_core::upstreams::peer::BasicPeer;

use super::proxy::{TlsContext, TlsUpstreamInfo};
use super::sni_parser;
use super::TlsRouteManager;
use crate::core::common::utils::proxy_protocol::ProxyProtocolV2Builder;
use crate::core::gateway::backends::{resolve_target_port, select_roundrobin_backend, PortLookupSource};
use crate::core::gateway::observe::log_tls;
use crate::core::gateway::observe::AccessLogger;
use crate::core::gateway::runtime::matching::route::hostname_matches_listener;
use crate::core::gateway::runtime::store::get_listener_gateway_info_store;
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfig;
use crate::types::ResourceMeta;
use crate::types::{ListenerKey, MatchedInfo, Rsc};

fn is_service_backend_ref(group: Option<&str>, kind: Option<&str>) -> bool {
    let group_ok = matches!(group, None | Some("") | Some("core"));
    let kind_ok = matches!(kind, None | Some("Service"));
    group_ok && kind_ok
}

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
        let (client_addr, client_port) = downstream
            .get_socket_digest()
            .and_then(|d| d.peer_addr().cloned())
            .and_then(|addr| addr.as_inet().map(|inet| (inet.ip().to_string(), inet.port())))
            .unwrap_or_else(|| ("unknown".to_string(), 0));

        let mut ctx = TlsContext::new(self.listener_port, client_addr, client_port);

        if *shutdown.borrow() {
            ctx.err_log = Some("shutdown".to_string());
            self.log_disconnect(&mut ctx).await;
            return None;
        }

        // 1. Read the beginning of the stream to extract SNI from ClientHello.
        //    We read into a buffer, parse SNI, then prepend the bytes back
        //    so upstream receives the complete TLS handshake.
        let mut peek_buf = vec![0u8; sni_parser::MAX_CLIENT_HELLO_LEN];
        let n = match downstream.read(&mut peek_buf).await {
            Ok(0) => {
                ctx.err_log = Some("connection closed before ClientHello".to_string());
                self.log_disconnect(&mut ctx).await;
                return None;
            }
            Ok(n) => n,
            Err(e) => {
                ctx.err_log = Some(format!("read ClientHello failed: {e}"));
                self.log_disconnect(&mut ctx).await;
                return None;
            }
        };
        let initial_bytes = &peek_buf[..n];

        // 2. Extract SNI
        let sni = match sni_parser::extract_sni(initial_bytes) {
            Some(sni) => sni,
            None => {
                ctx.err_log = Some("no SNI in ClientHello".to_string());
                self.log_disconnect(&mut ctx).await;
                return None;
            }
        };
        ctx.sni = Some(sni.clone());

        // 3. Handle connection: route match → connect upstream → relay
        self.handle_connection(downstream, &mut ctx, &sni, &peek_buf[..n]).await;
        self.log_disconnect(&mut ctx).await;

        None
    }
}

impl EdgionTlsPassthroughProxy {
    /// Route match, connect upstream, and relay traffic.
    ///
    /// `initial_bytes` contains the raw ClientHello bytes already read from
    /// downstream. These must be forwarded to upstream before starting the
    /// bidirectional relay.
    async fn handle_connection(&self, downstream: Stream, ctx: &mut TlsContext, sni: &str, initial_bytes: &[u8]) {
        // 0. Verify SNI is accepted by at least one listener on this port.
        //    Multiple Gateways share the same physical port, so the route table
        //    is a union of all their TLSRoutes. Without this check, an SNI that
        //    only matches a wildcard route from a different Gateway would be
        //    incorrectly forwarded.
        if !self.sni_accepted_by_any_listener(sni) {
            ctx.err_log = Some("no listener accepts SNI".to_string());
            return;
        }

        // 1. Match TLSRoute by SNI
        let route_table = self.tls_route_manager.load_route_table();
        let tls_route = match route_table.match_route(sni) {
            Some(route) => route,
            None => {
                ctx.err_log = Some("no match".to_string());
                return;
            }
        };

        ctx.matched = Some(MatchedInfo {
            rsc: Rsc {
                kind: "TLSRoute".to_string(),
                ns: tls_route.metadata.namespace.clone().unwrap_or_default(),
                name: tls_route.metadata.name.clone().unwrap_or_default(),
            },
            section: None,
            sv: tls_route.get_sync_version(),
        });

        // 2. Get the first rule
        let rule = match tls_route.spec.rules.as_ref().and_then(|rules| rules.first()) {
            Some(rule) => rule,
            None => {
                ctx.err_log = Some("no rules".to_string());
                return;
            }
        };

        // 3. Connect upstream with retry
        let max_attempts = rule.max_connect_retries;
        let mut upstream = None;
        let mut last_err = String::new();

        for attempt in 0..max_attempts {
            let backend_ref = match rule.backend_finder.select() {
                Ok(b) => b,
                Err(_) => {
                    ctx.err_log = Some("no backend".to_string());
                    return;
                }
            };
            if !is_service_backend_ref(backend_ref.group.as_deref(), backend_ref.kind.as_deref()) {
                last_err = format!(
                    "unsupported backend kind: group={:?}, kind={:?}, name={}",
                    backend_ref.group, backend_ref.kind, backend_ref.name
                );
                continue;
            }

            let namespace = backend_ref
                .namespace
                .as_deref()
                .or_else(|| tls_route.metadata.namespace.as_deref())
                .unwrap_or("default");
            let service_key = format!("{}/{}", namespace, &backend_ref.name);

            let backend = match select_roundrobin_backend(&service_key) {
                Some(b) => b,
                None => {
                    last_err = format!("no endpoint: {service_key}");
                    continue;
                }
            };

            let mut upstream_addr = backend.addr;
            resolve_target_port(
                &mut upstream_addr,
                backend_ref.port,
                &service_key,
                PortLookupSource::Auto,
            );
            let upstream_addr_str = upstream_addr.to_string();
            let parsed_upstream_addr: Option<std::net::SocketAddr> = upstream_addr_str.parse().ok();

            ctx.upstream_info.push(TlsUpstreamInfo {
                addr: parsed_upstream_addr.as_ref().map(|a| a.ip().to_string()),
                port: parsed_upstream_addr.as_ref().map(|a| a.port()),
                backend: Some(MatchedInfo {
                    rsc: Rsc {
                        kind: backend_ref.kind.clone().unwrap_or_else(|| "Service".to_string()),
                        ns: namespace.to_string(),
                        name: backend_ref.name.clone(),
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

            let peer = BasicPeer::new(&upstream_addr_str);
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
                    upstream = Some(stream);
                    break;
                }
                Err(e) => {
                    last_err = format!("connect failed: {upstream_addr_str}: {e}");
                    if attempt + 1 < max_attempts {
                        ctx.push_log(&format!("retry {}/{max_attempts}: {last_err}", attempt + 1));
                    }
                }
            }
        }

        let mut upstream_stream = match upstream {
            Some(s) => s,
            None => {
                ctx.err_log = Some(last_err);
                return;
            }
        };

        // 4. Send Proxy Protocol v2 if configured
        if let Some(2) = rule.proxy_protocol_version {
            if let Ok(src_ip) = ctx.client_addr.parse::<IpAddr>() {
                let upstream_addr_str = ctx
                    .upstream_info
                    .last()
                    .and_then(|i| {
                        let addr = i.addr.as_deref()?;
                        let port = i.port?;
                        Some(format!("{addr}:{port}"))
                    })
                    .unwrap_or_default();
                let src_addr = std::net::SocketAddr::new(src_ip, ctx.client_port);
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

        // 5. Forward the initial ClientHello bytes that we already read
        if let Err(e) = upstream_stream.write_all(initial_bytes).await {
            ctx.err_log = Some(format!("upstream write initial bytes failed: {e}"));
            return;
        }
        if upstream_stream.flush().await.is_err() {
            ctx.err_log = Some("upstream flush initial bytes failed".to_string());
            return;
        }
        if let Some(info) = ctx.current_upstream_mut() {
            info.bytes_sent += initial_bytes.len() as u64;
        }

        // 6. Bidirectional relay of raw TLS traffic
        self.duplex(downstream, upstream_stream, ctx).await;
    }

    /// Bidirectional data transfer between downstream and upstream.
    async fn duplex(&self, mut downstream: Stream, mut upstream: Stream, ctx: &mut TlsContext) {
        const BUFFER_SIZE: usize = 8192;
        let mut upstream_buf = vec![0u8; BUFFER_SIZE];
        let mut downstream_buf = vec![0u8; BUFFER_SIZE];

        loop {
            tokio::select! {
                result = downstream.read(&mut upstream_buf) => {
                    match result {
                        Ok(0) => break,
                        Ok(n) => {
                            if let Some(info) = ctx.current_upstream_mut() {
                                info.bytes_sent += n as u64;
                            }
                            if (upstream.write_all(&upstream_buf[..n]).await).is_err() {
                                ctx.err_log = Some("upstream write err".to_string());
                                return;
                            }
                            if (upstream.flush().await).is_err() {
                                ctx.err_log = Some("upstream flush err".to_string());
                                return;
                            }
                        }
                        Err(_) => {
                            ctx.err_log = Some("downstream read err".to_string());
                            return;
                        }
                    }
                }
                result = upstream.read(&mut downstream_buf) => {
                    match result {
                        Ok(0) => break,
                        Ok(n) => {
                            if let Some(info) = ctx.current_upstream_mut() {
                                info.bytes_received += n as u64;
                            }
                            if (downstream.write_all(&downstream_buf[..n]).await).is_err() {
                                ctx.err_log = Some("downstream write err".to_string());
                                return;
                            }
                            if (downstream.flush().await).is_err() {
                                ctx.err_log = Some("downstream flush err".to_string());
                                return;
                            }
                        }
                        Err(_) => {
                            ctx.err_log = Some("upstream read err".to_string());
                            return;
                        }
                    }
                }
            }
        }
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

        let log_entry = serde_json::json!({
            "ts": ctx.ts,
            "mode": "passthrough",
            "listener_port": ctx.listener_port,
            "client_addr": &ctx.client_addr,
            "client_port": ctx.client_port,
            "sni": &ctx.sni,
            "end_time": ctx.end_time,
            "upstream_info": &ctx.upstream_info,
            "log": &ctx.log,
            "err_log": &ctx.err_log,
        });
        self.access_logger.send(log_entry.to_string()).await;
    }
}
