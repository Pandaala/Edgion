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
use crate::core::gateway::observe::{log_tls, TlsLogEntry};
use crate::core::gateway::plugins::stream::get_global_stream_plugin_store;
use crate::core::gateway::plugins::{StreamContext, StreamPluginResult};
use crate::core::gateway::routes::tls::get_global_tls_route_manager;
use crate::types::ctx::ClientCertInfo;
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfig;
use crate::types::TlsConnMeta;

/// TLS connection context
pub struct TlsContext {
    pub listener_port: u16,
    pub client_addr: String,
    pub client_port: u16,
    pub sni_hostname: Option<String>,
    pub upstream_addr: Option<String>,
    pub start_time: Instant,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub status: TlsStatus,
    pub connection_established: bool,
    pub proxy_protocol_sent: bool,
    pub upstream_protocol: String,
    pub route_name: Option<String>,
    pub gateway_key: Option<String>,
    /// Correlation ID from ssl.log (set via TlsConnMeta from handshake)
    pub tls_id: Option<String>,
    /// Client certificate info from mTLS handshake
    pub client_cert_info: Option<ClientCertInfo>,
}

#[derive(Debug, Clone)]
pub enum TlsStatus {
    Success,
    NoSniProvided,
    NoMatchingRoute,
    UpstreamConnectionFailed,
    UpstreamReadError,
    UpstreamWriteError,
    DownstreamReadError,
    DownstreamWriteError,
    TlsHandshakeError,
    DeniedByPlugin,
}

/// TLS proxy service that terminates TLS and forwards to TCP backend.
///
/// Route lookup uses the global `TlsRouteManager` per-connection (via
/// `load_route_table()`) instead of caching an Arc at startup. This
/// eliminates the stale-Arc problem where rebuild could orphan the
/// reference held by a long-lived EdgionTls instance.
pub struct EdgionTls {
    pub gateway_name: String,
    pub gateway_namespace: Option<String>,
    pub gateway_key: String,
    pub listener_port: u16,
    pub access_logger: Arc<AccessLogger>,
    pub edgion_gateway_config: Arc<EdgionGatewayConfig>,
    pub connector: TransportConnector,
}

#[async_trait]
impl ServerApp for EdgionTls {
    async fn process_new(self: &Arc<Self>, mut downstream: Stream, shutdown: &ShutdownWatch) -> Option<Stream> {
        if *shutdown.borrow() {
            tracing::info!(
                listener_port = self.listener_port,
                "Rejecting new TLS connection during shutdown"
            );
            return None;
        }

        let (client_addr, client_port) = downstream
            .get_socket_digest()
            .and_then(|d| d.peer_addr().cloned())
            .and_then(|addr| addr.as_inet().map(|inet| (inet.ip().to_string(), inet.port())))
            .unwrap_or_else(|| ("unknown".to_string(), 0));

        let gateway_key = self
            .gateway_namespace
            .as_ref()
            .map(|ns| format!("{}/{}", ns, self.gateway_name));

        let mut ctx = TlsContext {
            listener_port: self.listener_port,
            client_addr,
            client_port,
            sni_hostname: None,
            upstream_addr: None,
            start_time: Instant::now(),
            bytes_sent: 0,
            bytes_received: 0,
            status: TlsStatus::Success,
            connection_established: false,
            proxy_protocol_sent: false,
            upstream_protocol: "TCP".to_string(),
            route_name: None,
            gateway_key,
            tls_id: None,
            client_cert_info: None,
        };

        // Read TlsConnMeta from SslDigest extension (set by handshake_complete_callback)
        let tls_meta: Option<TlsConnMeta> = downstream
            .get_ssl_digest()
            .and_then(|d| d.extension.get::<TlsConnMeta>().cloned());

        let (sni_hostname, tls_id, client_cert_info) = match tls_meta {
            Some(meta) => (meta.sni, meta.tls_id, meta.client_cert_info),
            None => {
                // Defensive fallback (should not happen in normal operation)
                (Self::extract_sni(&mut downstream), None, None)
            }
        };

        let sni_hostname = match sni_hostname {
            Some(sni) => sni,
            None => {
                ctx.status = TlsStatus::NoSniProvided;
                self.log_disconnect(&ctx).await;
                return None;
            }
        };

        ctx.sni_hostname = Some(sni_hostname.clone());
        ctx.tls_id = tls_id;
        ctx.client_cert_info = client_cert_info;

        self.handle_connection(downstream, &mut ctx, &sni_hostname).await;

        self.log_disconnect(&ctx).await;

        None
    }
}

impl EdgionTls {
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
    async fn handle_connection(&self, downstream: Stream, ctx: &mut TlsContext, sni_hostname: &str) {
        // 1. Match TLSRoute based on SNI — load fresh snapshot per-connection
        let route_table = get_global_tls_route_manager().load_route_table();
        let tls_route = match route_table.match_route(&self.gateway_key, sni_hostname) {
            Some(route) => route,
            None => {
                ctx.status = TlsStatus::NoMatchingRoute;
                tracing::warn!(
                    sni = %sni_hostname,
                    gateway_key = %self.gateway_key,
                    "No matching TLSRoute found"
                );
                return;
            }
        };

        // Record route name for logging
        ctx.route_name = tls_route
            .metadata
            .namespace
            .as_ref()
            .zip(tls_route.metadata.name.as_ref())
            .map(|(ns, name)| format!("{}/{}", ns, name));

        // 2. Get the first rule
        let rule = match tls_route.spec.rules.as_ref().and_then(|rules| rules.first()) {
            Some(rule) => rule,
            None => {
                ctx.status = TlsStatus::UpstreamConnectionFailed;
                return;
            }
        };

        // 3. Execute stream plugins (same pattern as EdgionTcp)
        if let Some(store_key) = &rule.stream_plugin_store_key {
            if let Ok(client_ip) = ctx.client_addr.parse() {
                let store = get_global_stream_plugin_store();
                if let Some(resource) = store.get(store_key) {
                    let runtime = &resource.spec.stream_plugin_runtime;
                    if !runtime.is_empty() {
                        let stream_ctx = StreamContext::new(client_ip, self.listener_port);
                        match runtime.run(&stream_ctx).await {
                            StreamPluginResult::Allow => {
                                tracing::debug!(
                                    store_key = %store_key,
                                    "Stream plugins allowed TLS connection"
                                );
                            }
                            StreamPluginResult::Deny(reason) => {
                                ctx.status = TlsStatus::DeniedByPlugin;
                                tracing::info!(
                                    sni = %sni_hostname,
                                    store_key = %store_key,
                                    reason = %reason,
                                    "TLS connection denied by stream plugin"
                                );
                                return;
                            }
                        }
                    }
                } else {
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
                ctx.status = TlsStatus::UpstreamConnectionFailed;
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
                ctx.status = TlsStatus::UpstreamConnectionFailed;
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
        ctx.upstream_addr = Some(upstream_addr_str.clone());

        // 7. Connect to upstream (TCP only; upstream TLS deferred)
        ctx.upstream_protocol = "TCP".to_string();
        let peer = BasicPeer::new(&upstream_addr_str);
        let mut upstream = match self.connector.new_stream(&peer).await {
            Ok(stream) => stream,
            Err(e) => {
                ctx.status = TlsStatus::UpstreamConnectionFailed;
                tracing::warn!(
                    upstream = %upstream_addr_str,
                    error = %e,
                    "Failed to connect to upstream"
                );
                return;
            }
        };

        ctx.connection_established = true;

        // 8. Send Proxy Protocol v2 header if configured
        if let Some(2) = rule.proxy_protocol_version {
            if let Ok(src_ip) = ctx.client_addr.parse::<IpAddr>() {
                let src_addr = std::net::SocketAddr::new(src_ip, ctx.client_port);
                let dst_addr: std::net::SocketAddr = upstream_addr_str.parse().unwrap_or_else(|_| {
                    std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0)
                });
                let mut builder = ProxyProtocolV2Builder::new(src_addr, dst_addr);
                builder.add_authority(sni_hostname);
                let pp2_header = builder.build();
                if let Err(e) = upstream.write_all(&pp2_header).await {
                    ctx.status = TlsStatus::UpstreamWriteError;
                    tracing::warn!(
                        upstream = %upstream_addr_str,
                        error = %e,
                        "Failed to send PP2 header to upstream"
                    );
                    return;
                }
                if upstream.flush().await.is_err() {
                    ctx.status = TlsStatus::UpstreamWriteError;
                    return;
                }
                ctx.proxy_protocol_sent = true;
            }
        }

        // 9. Log connection establishment
        self.log_connect(ctx).await;

        tracing::debug!(
            sni = %sni_hostname,
            upstream = %upstream_addr_str,
            pp2 = ctx.proxy_protocol_sent,
            "TLS terminated, forwarding to {} backend",
            ctx.upstream_protocol
        );

        // 10. Bidirectional data forwarding
        self.duplex(downstream, upstream, ctx).await;
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
                            ctx.bytes_sent += n as u64;
                            if (upstream.write_all(&upstream_buf[0..n]).await).is_err() {
                                ctx.status = TlsStatus::UpstreamWriteError;
                                break;
                            }
                            if (upstream.flush().await).is_err() {
                                ctx.status = TlsStatus::UpstreamWriteError;
                                break;
                            }
                        }
                        Err(_) => {
                            ctx.status = TlsStatus::DownstreamReadError;
                            break;
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
                            ctx.bytes_received += n as u64;
                            if (downstream.write_all(&downstream_buf[0..n]).await).is_err() {
                                ctx.status = TlsStatus::DownstreamWriteError;
                                break;
                            }
                            if (downstream.flush().await).is_err() {
                                ctx.status = TlsStatus::DownstreamWriteError;
                                break;
                            }
                        }
                        Err(_) => {
                            ctx.status = TlsStatus::UpstreamReadError;
                            break;
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
    async fn log_connect(&self, ctx: &TlsContext) {
        if !self.is_tls_proxy_log_enabled() {
            return;
        }
        let protocol = format!("TLS-{}", ctx.upstream_protocol);
        let entry = TlsLogEntry {
            ts: chrono::Utc::now().timestamp_millis(),
            event: "connect".to_string(),
            protocol,
            listener_port: ctx.listener_port,
            client_addr: ctx.client_addr.clone(),
            client_port: ctx.client_port,
            tls_id: ctx.tls_id.clone(),
            sni_hostname: ctx.sni_hostname.clone(),
            upstream_addr: ctx.upstream_addr.clone(),
            duration_ms: None,
            bytes_sent: None,
            bytes_received: None,
            status: format!("{:?}", ctx.status),
            connection_established: ctx.connection_established,
            proxy_protocol: if ctx.proxy_protocol_sent {
                Some("v2".to_string())
            } else {
                None
            },
            route_name: ctx.route_name.clone(),
            gateway_name: ctx.gateway_key.clone(),
        };
        log_tls(&entry).await;
    }

    /// Log connection disconnection event
    async fn log_disconnect(&self, ctx: &TlsContext) {
        if !self.is_tls_proxy_log_enabled() {
            return;
        }
        let duration_ms = ctx.start_time.elapsed().as_millis() as u64;
        let protocol = format!("TLS-{}", ctx.upstream_protocol);
        let entry = TlsLogEntry {
            ts: chrono::Utc::now().timestamp_millis(),
            event: "disconnect".to_string(),
            protocol,
            listener_port: ctx.listener_port,
            client_addr: ctx.client_addr.clone(),
            client_port: ctx.client_port,
            tls_id: ctx.tls_id.clone(),
            sni_hostname: ctx.sni_hostname.clone(),
            upstream_addr: ctx.upstream_addr.clone(),
            duration_ms: Some(duration_ms),
            bytes_sent: Some(ctx.bytes_sent),
            bytes_received: Some(ctx.bytes_received),
            status: format!("{:?}", ctx.status),
            connection_established: ctx.connection_established,
            proxy_protocol: if ctx.proxy_protocol_sent {
                Some("v2".to_string())
            } else {
                None
            },
            route_name: ctx.route_name.clone(),
            gateway_name: ctx.gateway_key.clone(),
        };
        log_tls(&entry).await;

        // Also send to the per-listener access logger for backward compatibility
        let log_entry = serde_json::json!({
            "ts": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
            "protocol": format!("TLS-{}", ctx.upstream_protocol),
            "listener_port": ctx.listener_port,
            "client_addr": &ctx.client_addr,
            "client_port": ctx.client_port,
            "tls_id": &ctx.tls_id,
            "sni_hostname": &ctx.sni_hostname,
            "upstream_addr": &ctx.upstream_addr,
            "duration_ms": duration_ms,
            "bytes_sent": ctx.bytes_sent,
            "bytes_received": ctx.bytes_received,
            "status": format!("{:?}", ctx.status),
        });
        self.access_logger.send(log_entry.to_string()).await;
    }
}
