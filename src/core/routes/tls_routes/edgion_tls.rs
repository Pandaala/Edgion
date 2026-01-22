use async_trait::async_trait;
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

use crate::core::backends::select_roundrobin_backend;
use crate::core::observe::AccessLogger;
use crate::core::routes::tls_routes::GatewayTlsRoutes;
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfig;

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
}

/// TLS proxy service that terminates TLS and forwards to TCP backend
pub struct EdgionTls {
    pub gateway_name: String,
    pub gateway_namespace: Option<String>,
    pub listener_port: u16,
    pub gateway_tls_routes: Arc<GatewayTlsRoutes>,
    pub access_logger: Arc<AccessLogger>,
    pub edgion_gateway_config: Arc<EdgionGatewayConfig>,
    pub connector: TransportConnector,
}

#[async_trait]
impl ServerApp for EdgionTls {
    async fn process_new(self: &Arc<Self>, mut downstream: Stream, _shutdown: &ShutdownWatch) -> Option<Stream> {
        // Create context
        let mut ctx = TlsContext {
            listener_port: self.listener_port,
            client_addr: "unknown".to_string(), // TODO: Extract from Stream
            client_port: 0,
            sni_hostname: None,
            upstream_addr: None,
            start_time: Instant::now(),
            bytes_sent: 0,
            bytes_received: 0,
            status: TlsStatus::Success,
            connection_established: false,
        };

        // Extract SNI from TLS stream
        let sni_hostname = match Self::extract_sni(&mut downstream) {
            Some(sni) => {
                ctx.sni_hostname = Some(sni.clone());
                sni
            }
            None => {
                ctx.status = TlsStatus::NoSniProvided;
                self.log_connection(&ctx).await;
                return None;
            }
        };

        // Handle connection (context will be updated regardless of success or failure)
        self.handle_connection(downstream, &mut ctx, &sni_hostname).await;

        // Only log if connection was actually established
        if ctx.connection_established {
            self.log_connection(&ctx).await;
        }

        None
    }
}

impl EdgionTls {
    /// Extract SNI hostname from TLS stream
    ///
    /// For Pingora's Stream type, if it's already TLS-terminated,
    /// we can access the SSL context to get the SNI.
    fn extract_sni(#[allow(unused_variables)] stream: &mut Stream) -> Option<String> {
        #[cfg(any(feature = "boringssl", feature = "openssl"))]
        {
            // Try to get SSL reference from the stream
            if let Some(ssl_ref) = stream.get_ssl() {
                // Get the SNI (Server Name Indication) from SSL context
                if let Some(sni) = ssl_ref.servername(NameType::HOST_NAME) {
                    return Some(sni.to_string());
                }
            }
        }
        None
    }

    /// Core logic for handling TLS-terminated connections
    async fn handle_connection(&self, downstream: Stream, ctx: &mut TlsContext, sni_hostname: &str) {
        // 1. Match TLSRoute based on SNI
        let tls_route = match self.gateway_tls_routes.match_route(sni_hostname) {
            Some(route) => route,
            None => {
                ctx.status = TlsStatus::NoMatchingRoute;
                tracing::warn!(
                    sni = %sni_hostname,
                    "No matching TLSRoute found"
                );
                return;
            }
        };

        // 2. Select backend
        let backend_ref = match tls_route.spec.rules.as_ref().and_then(|rules| rules.first()) {
            Some(rule) => match rule.backend_finder.select() {
                Ok(backend) => backend,
                Err(_) => {
                    ctx.status = TlsStatus::UpstreamConnectionFailed;
                    return;
                }
            },
            None => {
                ctx.status = TlsStatus::UpstreamConnectionFailed;
                return;
            }
        };

        // 3. Resolve backend address via EndpointSlice
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

        // 4. Build upstream address (using actual IP address)
        let mut upstream_addr = backend.addr;
        if let Some(port) = backend_ref.port {
            upstream_addr.set_port(port as u16);
        }
        let upstream_addr_str = upstream_addr.to_string();
        ctx.upstream_addr = Some(upstream_addr_str.clone());

        // 5. Connect to upstream TCP backend
        let peer = BasicPeer::new(&upstream_addr_str);
        let upstream = match self.connector.new_stream(&peer).await {
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

        // Mark connection as established
        ctx.connection_established = true;

        tracing::debug!(
            sni = %sni_hostname,
            upstream = %upstream_addr_str,
            "TLS terminated, forwarding to TCP backend"
        );

        // 6. Bidirectional data forwarding
        self.duplex(downstream, upstream, ctx).await;

        // Note: TLS routes currently use RoundRobin only
        // When LeastConnection support is added, increment/decrement should be called here
        // based on the selected LB policy
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

    /// Log access record
    async fn log_connection(&self, ctx: &TlsContext) {
        let duration_ms = ctx.start_time.elapsed().as_millis() as u64;

        let log_entry = serde_json::json!({
            "ts": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
            "protocol": "TLS-TCP",
            "listener_port": ctx.listener_port,
            "client_addr": &ctx.client_addr,
            "client_port": ctx.client_port,
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
