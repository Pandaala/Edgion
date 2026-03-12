use async_trait::async_trait;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::select;

use pingora_core::apps::ServerApp;
use pingora_core::connectors::TransportConnector;
use pingora_core::protocols::Stream;
use pingora_core::server::ShutdownWatch;
#[cfg(any(feature = "boringssl", feature = "openssl"))]
use pingora_core::tls::ssl::NameType;
use pingora_core::upstreams::peer::BasicPeer;

use crate::core::common::utils::proxy_protocol::ProxyProtocolV2Builder;
use crate::core::gateway::backends::select_roundrobin_backend;
use crate::core::gateway::observe::AccessLogger;
use crate::core::gateway::observe::log_tls;
use crate::core::gateway::observe::logs::LogBuffer;
use crate::core::gateway::plugins::stream::get_global_stream_plugin_store;
use crate::core::gateway::plugins::{StreamContext, StreamPluginResult};
use crate::core::gateway::runtime::store::get_port_gateway_info_store;
use crate::core::gateway::routes::tls::get_global_tls_route_manager;
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfig;
use crate::types::{MatchedInfo, TlsConnMeta};
use serde::Serialize;

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

#[derive(Debug, Clone, Serialize)]
pub struct TlsUpstreamInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub addr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<MatchedInfo>,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub bytes_sent: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub bytes_received: u64,
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
    pub event: String,
    pub listener_port: u16,
    pub client_addr: String,
    pub client_port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<MatchedInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub upstream_info: Vec<TlsUpstreamInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<LogBuffer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub err_log: Option<String>,
}

impl TlsContext {
    fn new(listener_port: u16, client_addr: String, client_port: u16) -> Self {
        Self {
            ts: chrono::Utc::now().timestamp_millis(),
            start_at: Instant::now(),
            event: "connect".to_string(),
            listener_port,
            client_addr,
            client_port,
            tls_id: None,
            sni: None,
            matched: None,
            duration_ms: None,
            upstream_info: Vec::new(),
            log: None,
            err_log: None,
        }
    }

    fn push_log(&mut self, message: &str) {
        let _ = self.log.get_or_insert_with(LogBuffer::new).push(message);
    }

    fn current_upstream_mut(&mut self) -> Option<&mut TlsUpstreamInfo> {
        self.upstream_info.last_mut()
    }
}

/// TLS-to-TCP proxy service for Gateway TLS listeners.
///
/// Route lookup uses the global `TlsRouteManager` per-connection (via
/// `load_route_table()`) instead of caching an Arc at startup. This
/// eliminates the stale-Arc problem where rebuild could orphan the
/// reference held by a long-lived TLS proxy instance.
pub struct EdgionTlsTcpProxy {
    pub listener_port: u16,
    pub access_logger: Arc<AccessLogger>,
    pub edgion_gateway_config: Arc<EdgionGatewayConfig>,
    pub connector: TransportConnector,
}

#[async_trait]
impl ServerApp for EdgionTlsTcpProxy {
    async fn process_new(self: &Arc<Self>, mut downstream: Stream, shutdown: &ShutdownWatch) -> Option<Stream> {
        let (client_addr, client_port) = downstream
            .get_socket_digest()
            .and_then(|d| d.peer_addr().cloned())
            .and_then(|addr| addr.as_inet().map(|inet| (inet.ip().to_string(), inet.port())))
            .unwrap_or_else(|| ("unknown".to_string(), 0));

        let mut ctx = TlsContext::new(self.listener_port, client_addr, client_port);

        if *shutdown.borrow() {
            let msg = "Rejecting new TLS connection during shutdown";
            ctx.event = "reject".to_string();
            ctx.err_log = Some(msg.to_string());
            self.log_disconnect(&mut ctx).await;
            return None;
        }

        // Read TlsConnMeta from SslDigest extension (set by handshake_complete_callback)
        let tls_meta = downstream
            .get_ssl_digest()
            .and_then(|d| d.extension.get::<TlsConnMeta>().cloned());

        if let Some(meta) = tls_meta.as_ref() {
            ctx.tls_id = meta.tls_id.clone();
            ctx.sni = meta.sni.clone();
            ctx.matched = meta.matched.clone();
        }
        if ctx.sni.is_none() {
            ctx.sni = Self::extract_sni(&mut downstream);
        }

        let sni = match ctx.sni.as_deref() {
            Some(sni) => sni.to_string(),
            None => {
                ctx.err_log = Some("No SNI provided".to_string());
                self.log_disconnect(&mut ctx).await;
                return None;
            }
        };

        self.handle_connection(downstream, &mut ctx, &sni).await;
        self.log_disconnect(&mut ctx).await;

        None
    }
}

impl EdgionTlsTcpProxy {
    /// Extract SNI hostname from TLS stream
    fn extract_sni(#[allow(unused_variables)] stream: &mut Stream) -> Option<String> {
        #[cfg(any(feature = "boringssl", feature = "openssl"))]
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
    async fn handle_connection(&self, downstream: Stream, ctx: &mut TlsContext, sni: &str) {
        // 1. Match TLSRoute based on SNI — load fresh snapshot per-connection
        let route_table = get_global_tls_route_manager().load_route_table();
        let gateway_infos = get_port_gateway_info_store().get(self.listener_port);
        let matched = gateway_infos
            .iter()
            .find_map(|gateway_info| {
                let gateway_key = gateway_info.gateway_key();
                route_table.match_route(&gateway_key, sni)
            });

        let tls_route = match matched {
            Some(matched) => matched,
            None => {
                ctx.err_log = Some("No matching TLSRoute found".to_string());
                tracing::warn!(
                    sni = %sni,
                    listener_port = self.listener_port,
                    "No matching TLSRoute found"
                );
                return;
            }
        };
        ctx.matched = Some(MatchedInfo {
            kind: "TLSRoute".to_string(),
            ns: tls_route.metadata.namespace.clone().unwrap_or_else(|| "default".to_string()),
            name: tls_route.metadata.name.clone().unwrap_or_else(|| "-".to_string()),
            section: None,
        });

        // 2. Get the first rule
        let rule = match tls_route.spec.rules.as_ref().and_then(|rules| rules.first()) {
            Some(rule) => rule,
            None => {
                ctx.err_log = Some("TLSRoute has no rules".to_string());
                return;
            }
        };

        // 3. Execute stream plugins (same pattern as EdgionTcpProxy)
        if let Some(store_key) = &rule.stream_plugin_store_key {
            if let Ok(client_ip) = ctx.client_addr.parse() {
                let store = get_global_stream_plugin_store();
                if let Some(resource) = store.get(store_key) {
                    let runtime = &resource.spec.stream_plugin_runtime;
                    if !runtime.is_empty() {
                        let stream_ctx = StreamContext::new(client_ip, self.listener_port);
                        match runtime.run(&stream_ctx).await {
                            StreamPluginResult::Allow => {
                                ctx.push_log("Stream plugins allowed TLS connection");
                                tracing::debug!(
                                    store_key = %store_key,
                                    "Stream plugins allowed TLS connection"
                                );
                            }
                            StreamPluginResult::Deny(reason) => {
                                ctx.err_log = Some(format!("TLS connection denied by stream plugin: {reason}"));
                                tracing::info!(
                                    sni = %sni,
                                    store_key = %store_key,
                                    reason = %reason,
                                    "TLS connection denied by stream plugin"
                                );
                                return;
                            }
                        }
                    }
                } else {
                    ctx.push_log("EdgionStreamPlugins resource not found in store, allowing connection");
                    tracing::warn!(
                        store_key = %store_key,
                        "EdgionStreamPlugins resource not found in store, allowing connection"
                    );
                }
            }
        }

        // 4. Select backend
        let backend_ref = match rule.backend_finder.select() {
            Ok(backend) => backend,
            Err(_) => {
                ctx.err_log = Some("Failed to select backend".to_string());
                return;
            }
        };

        // 5. Resolve backend address via EndpointSlice
        let namespace = backend_ref
            .namespace
            .as_deref()
            .or_else(|| tls_route.metadata.namespace.as_deref())
            .unwrap_or("default");
        let service_key = format!("{}/{}", namespace, &backend_ref.name);

        let backend = match select_roundrobin_backend(&service_key) {
            Some(backend) => backend,
            None => {
                ctx.err_log = Some(format!("No healthy backend endpoint found for {service_key}"));
                tracing::warn!(
                    service = %service_key,
                    "No healthy backend endpoint found"
                );
                return;
            }
        };

        // 6. Build upstream address
        let mut upstream_addr = backend.addr;
        if let Some(port) = backend_ref.port {
            upstream_addr.set_port(port as u16);
        }
        let upstream_addr_str = upstream_addr.to_string();
        let parsed_upstream_addr: Option<std::net::SocketAddr> = upstream_addr_str.parse().ok();
        ctx.upstream_info.push(TlsUpstreamInfo {
            addr: parsed_upstream_addr.as_ref().map(|addr| addr.ip().to_string()),
            port: parsed_upstream_addr.as_ref().map(|addr| addr.port()),
            backend: Some(MatchedInfo {
                kind: backend_ref.kind.clone().unwrap_or_else(|| "Service".to_string()),
                ns: namespace.to_string(),
                name: backend_ref.name.clone(),
                section: None,
            }),
            bytes_sent: 0,
            bytes_received: 0,
            connection_established: false,
            proxy_protocol_sent: false,
            upstream_protocol: Some("TCP".to_string()),
        });

        // 7. Connect to upstream (TCP only; upstream TLS deferred)
        let peer = BasicPeer::new(&upstream_addr_str);
        let mut upstream = match self.connector.new_stream(&peer).await {
            Ok(stream) => stream,
            Err(e) => {
                ctx.err_log = Some(format!("Failed to connect to upstream: {e}"));
                tracing::warn!(
                    upstream = %upstream_addr_str,
                    error = %e,
                    "Failed to connect to upstream"
                );
                return;
            }
        };

        if let Some(upstream_info) = ctx.current_upstream_mut() {
            upstream_info.connection_established = true;
        }

        // 8. Send Proxy Protocol v2 header if configured
        if let Some(2) = rule.proxy_protocol_version {
            if let Ok(src_ip) = ctx.client_addr.parse::<IpAddr>() {
                let src_addr = std::net::SocketAddr::new(src_ip, ctx.client_port);
                let dst_addr: std::net::SocketAddr = upstream_addr_str.parse().unwrap_or_else(|_| {
                    std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0)
                });
                let mut builder = ProxyProtocolV2Builder::new(src_addr, dst_addr);
                builder.add_authority(sni);
                let pp2_header = builder.build();
                if let Err(e) = upstream.write_all(&pp2_header).await {
                    ctx.err_log = Some(format!("Failed to send PP2 header to upstream: {e}"));
                    tracing::warn!(
                        upstream = %upstream_addr_str,
                        error = %e,
                        "Failed to send PP2 header to upstream"
                    );
                    return;
                }
                if upstream.flush().await.is_err() {
                    ctx.err_log = Some("Failed to flush PP2 header to upstream".to_string());
                    return;
                }
                if let Some(upstream_info) = ctx.current_upstream_mut() {
                    upstream_info.proxy_protocol_sent = true;
                }
                ctx.push_log("Sent Proxy Protocol v2 header to upstream");
            }
        }

        // 9. Log connection establishment
        self.log_connect(ctx).await;

        tracing::debug!(
            sni = %sni,
            upstream = %upstream_addr_str,
            pp2 = ctx.current_upstream_mut().map(|u| u.proxy_protocol_sent).unwrap_or(false),
            "TLS terminated, forwarding to {} backend",
            "TCP"
        );

        // 10. Bidirectional data forwarding
        self.duplex(downstream, upstream, ctx).await
    }

    /// Bidirectional data transfer between downstream (TLS-terminated) and upstream (TCP)
    async fn duplex(&self, mut downstream: Stream, mut upstream: Stream, ctx: &mut TlsContext) {
        const BUFFER_SIZE: usize = 8192;
        let mut upstream_buf = vec![0u8; BUFFER_SIZE];
        let mut downstream_buf = vec![0u8; BUFFER_SIZE];

        loop {
            select! {
                // Client → Upstream
                result = downstream.read(&mut upstream_buf) => {
                    match result {
                        Ok(0) => {
                            break;
                        }
                        Ok(n) => {
                            if let Some(upstream_info) = ctx.current_upstream_mut() {
                                upstream_info.bytes_sent += n as u64;
                            }
                            if (upstream.write_all(&upstream_buf[0..n]).await).is_err() {
                                ctx.err_log = Some("Failed writing downstream data to upstream".to_string());
                                return;
                            }
                            if (upstream.flush().await).is_err() {
                                ctx.err_log = Some("Failed flushing downstream data to upstream".to_string());
                                return;
                            }
                        }
                        Err(_) => {
                            ctx.err_log = Some("Failed reading from downstream".to_string());
                            return;
                        }
                    }
                }
                // Upstream → Client
                result = upstream.read(&mut downstream_buf) => {
                    match result {
                        Ok(0) => {
                            break;
                        }
                        Ok(n) => {
                            if let Some(upstream_info) = ctx.current_upstream_mut() {
                                upstream_info.bytes_received += n as u64;
                            }
                            if (downstream.write_all(&downstream_buf[0..n]).await).is_err() {
                                ctx.err_log = Some("Failed writing upstream data to downstream".to_string());
                                return;
                            }
                            if (downstream.flush().await).is_err() {
                                ctx.err_log = Some("Failed flushing upstream data to downstream".to_string());
                                return;
                            }
                        }
                        Err(_) => {
                            ctx.err_log = Some("Failed reading from upstream".to_string());
                            return;
                        }
                    }
                }
            }
        }
    }

    fn is_tls_proxy_log_enabled(&self) -> bool {
        self.edgion_gateway_config
            .spec
            .security_protect
            .as_ref()
            .is_none_or(|sp| sp.tls_proxy_log_record)
    }

    /// Log connection establishment event
    async fn log_connect(&self, ctx: &mut TlsContext) {
        if !self.is_tls_proxy_log_enabled() {
            return;
        }
        ctx.event = "connect".to_string();
        ctx.ts = chrono::Utc::now().timestamp_millis();
        ctx.duration_ms = None;
        log_tls(ctx).await;
    }

    /// Log connection disconnection event
    async fn log_disconnect(&self, ctx: &mut TlsContext) {
        if !self.is_tls_proxy_log_enabled() {
            return;
        }
        ctx.event = "disconnect".to_string();
        ctx.ts = chrono::Utc::now().timestamp_millis();
        ctx.duration_ms = Some(ctx.start_at.elapsed().as_millis() as u64);
        log_tls(ctx).await;

        // Also send to the per-listener access logger for backward compatibility
        let log_entry = serde_json::json!({
            "ts": ctx.ts,
            "event": &ctx.event,
            "listener_port": ctx.listener_port,
            "client_addr": &ctx.client_addr,
            "client_port": ctx.client_port,
            "tls_id": &ctx.tls_id,
            "sni": &ctx.sni,
            "duration_ms": ctx.duration_ms,
            "upstream_info": &ctx.upstream_info,
            "log": &ctx.log,
            "err_log": &ctx.err_log,
        });
        self.access_logger.send(log_entry.to_string()).await;
    }
}
