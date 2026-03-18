use async_trait::async_trait;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
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
use crate::core::gateway::observe::log_tls;
use crate::core::gateway::observe::logs::LogBuffer;
use crate::core::gateway::observe::AccessLogger;
use crate::core::gateway::plugins::stream::get_global_stream_plugin_store;
use crate::core::gateway::plugins::{StreamContext, StreamPluginResult, TlsRouteContext};
use crate::core::gateway::routes::tls::TlsRouteManager;
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfig;
use crate::types::{MatchedInfo, ResourceMeta, TlsConnMeta};
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
    pub client_addr: String,
    pub client_port: u16,
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
}

impl TlsContext {
    fn new(listener_port: u16, client_addr: String, client_port: u16) -> Self {
        Self {
            ts: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
            start_at: Instant::now(),
            listener_port,
            client_addr,
            client_port,
            tls_id: None,
            sni: None,
            matched: None,
            is_mtls: false,
            end_time: None,
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
/// Each instance holds an `Arc<TlsRouteManager>` for its listener port,
/// obtained at startup from `GlobalTlsRouteManagers`. Route updates swap
/// only the inner `ArcSwap<TlsRouteTable>`, so the Arc stays stable.
pub struct EdgionTlsTcpProxy {
    pub listener_port: u16,
    pub tls_route_manager: Arc<TlsRouteManager>,
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
            ctx.err_log = Some("shutdown".to_string());
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
            ctx.is_mtls = meta.is_mtls;
        }
        if ctx.sni.is_none() {
            ctx.sni = Self::extract_sni(&mut downstream);
        }

        let sni = match ctx.sni.as_deref() {
            Some(sni) => sni.to_string(),
            None => {
                ctx.err_log = Some("no SNI".to_string());
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
        // 1. Match TLSRoute by SNI from per-port route table
        let route_table = self.tls_route_manager.load_route_table();
        let tls_route = match route_table.match_route(sni) {
            Some(route) => route,
            None => {
                ctx.err_log = Some("no match".to_string());
                return;
            }
        };

        ctx.matched = Some(MatchedInfo {
            kind: "TLSRoute".to_string(),
            ns: tls_route.metadata.namespace.clone().unwrap_or_default(),
            name: tls_route.metadata.name.clone().unwrap_or_default(),
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

        // 3. Execute TLS route plugins (Stage 2: post-handshake, post-route-match)
        if !self.run_tls_route_plugins(rule, ctx, sni).await {
            return;
        }

        // 4. Connect upstream with retry
        let max_attempts = rule.max_connect_retries;
        let mut upstream = None;
        let mut last_err = String::new();

        for attempt in 0..max_attempts {
            // 4a. Select backend (re-select on each retry for load-balancing rotation)
            let backend_ref = match rule.backend_finder.select() {
                Ok(b) => b,
                Err(_) => {
                    ctx.err_log = Some("no backend".to_string());
                    return;
                }
            };

            // 4b. Resolve backend address via EndpointSlice
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
                    sv: 0,
                }),
                bytes_sent: 0,
                bytes_received: 0,
                connect_time: None,
                connection_established: false,
                proxy_protocol_sent: false,
                upstream_protocol: Some("TCP".to_string()),
            });

            // 4c. TCP connect
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
                    upstream = Some((stream, upstream_addr_str));
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

        let (mut upstream_stream, upstream_addr_str) = match upstream {
            Some(pair) => pair,
            None => {
                ctx.err_log = Some(last_err);
                return;
            }
        };

        // 5. Send Proxy Protocol v2 header if configured
        if !self
            .send_proxy_protocol_v2(rule, ctx, sni, &upstream_addr_str, &mut upstream_stream)
            .await
        {
            return;
        }

        // 6. Bidirectional data forwarding
        self.duplex(downstream, upstream_stream, ctx).await
    }

    /// Send Proxy Protocol v2 header to upstream if PP2 is enabled.
    /// Returns `false` if the write fails (caller should abort the connection).
    async fn send_proxy_protocol_v2(
        &self,
        rule: &crate::types::resources::tls_route::TLSRouteRule,
        ctx: &mut TlsContext,
        sni: &str,
        upstream_addr_str: &str,
        upstream: &mut Stream,
    ) -> bool {
        let Some(2) = rule.proxy_protocol_version else {
            return true;
        };
        let Ok(src_ip) = ctx.client_addr.parse::<IpAddr>() else {
            return true;
        };

        let src_addr = std::net::SocketAddr::new(src_ip, ctx.client_port);
        let dst_addr: std::net::SocketAddr = upstream_addr_str
            .parse()
            .unwrap_or_else(|_| std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0));
        let mut builder = ProxyProtocolV2Builder::new(src_addr, dst_addr);
        builder.add_authority(sni);
        let pp2_header = builder.build();

        if let Err(e) = upstream.write_all(&pp2_header).await {
            ctx.err_log = Some(format!("PP2 write failed: {e}"));
            return false;
        }
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

    /// Execute TLS route plugins (Stage 2).
    ///
    /// Tries `tls_route_plugin_runtime` first (new stage-aware path).
    /// Falls back to `stream_plugin_runtime` for backward compatibility
    /// when `tls_route_plugins` is absent but `plugins` is present on the resource.
    ///
    /// Returns `true` to proceed, `false` to reject the connection.
    async fn run_tls_route_plugins(
        &self,
        rule: &crate::types::resources::tls_route::TLSRouteRule,
        ctx: &mut TlsContext,
        sni: &str,
    ) -> bool {
        let Some(store_key) = &rule.stream_plugin_store_key else {
            return true;
        };
        let Ok(client_ip) = ctx.client_addr.parse() else {
            return true;
        };

        let store = get_global_stream_plugin_store();
        let Some(resource) = store.get(store_key) else {
            ctx.push_log("stream plugin resource not found, allowing");
            return true;
        };

        // Prefer Stage 2 (TlsRoute) runtime
        let tls_runtime = &resource.spec.tls_route_plugin_runtime;
        if !tls_runtime.is_empty() {
            let tls_ctx = TlsRouteContext {
                client_ip,
                listener_port: self.listener_port,
                sni: sni.to_string(),
                tls_id: ctx.tls_id.clone(),
                matched_route_ns: ctx.matched.as_ref().map(|m| m.ns.clone()).unwrap_or_default(),
                matched_route_name: ctx.matched.as_ref().map(|m| m.name.clone()).unwrap_or_default(),
                is_mtls: ctx.is_mtls,
            };
            return match tls_runtime.run(&tls_ctx).await {
                StreamPluginResult::Allow => {
                    ctx.push_log("tls route plugin allowed");
                    true
                }
                StreamPluginResult::Deny(reason) => {
                    ctx.err_log = Some(format!("tls route plugin denied: {reason}"));
                    false
                }
            };
        }

        // Backward compat: fall back to Stage 1 (ConnectionFilter) runtime
        // when the resource only has `plugins` but no `tlsRoutePlugins`.
        let stream_runtime = &resource.spec.stream_plugin_runtime;
        if !stream_runtime.is_empty() {
            let stream_ctx = StreamContext::new(client_ip, self.listener_port);
            return match stream_runtime.run(&stream_ctx).await {
                StreamPluginResult::Allow => {
                    ctx.push_log("stream plugin allowed (compat)");
                    true
                }
                StreamPluginResult::Deny(reason) => {
                    ctx.err_log = Some(format!("stream plugin denied (compat): {reason}"));
                    false
                }
            };
        }

        true
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

    fn is_tls_proxy_log_enabled(&self) -> bool {
        self.edgion_gateway_config
            .spec
            .security_protect
            .as_ref()
            .is_none_or(|sp| sp.tls_proxy_log_record)
    }

    /// Log connection disconnection event
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

        // Also send to the per-listener access logger for backward compatibility
        let log_entry = serde_json::json!({
            "ts": ctx.ts,
            "listener_port": ctx.listener_port,
            "client_addr": &ctx.client_addr,
            "client_port": ctx.client_port,
            "tls_id": &ctx.tls_id,
            "sni": &ctx.sni,
            "end_time": ctx.end_time,
            "upstream_info": &ctx.upstream_info,
            "log": &ctx.log,
            "err_log": &ctx.err_log,
        });
        self.access_logger.send(log_entry.to_string()).await;
    }
}
