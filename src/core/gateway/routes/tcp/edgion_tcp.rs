use async_trait::async_trait;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::select;

use pingora_core::apps::ServerApp;
use pingora_core::connectors::TransportConnector;
use pingora_core::protocols::Stream;
use pingora_core::server::ShutdownWatch;
use pingora_core::upstreams::peer::BasicPeer;

use crate::core::gateway::backends::select_roundrobin_backend;
use crate::core::gateway::observe::{log_tcp, TcpLogEntry};
use crate::core::gateway::plugins::stream::get_global_stream_plugin_store;
use crate::core::gateway::plugins::{StreamContext, StreamPluginResult};
use crate::core::gateway::routes::tcp::TcpPortRouteManager;
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfig;

/// TCP connection context
pub struct TcpContext {
    pub listener_port: u16,
    pub client_addr: String,
    pub client_port: u16,
    pub upstream_addr: Option<String>,
    pub start_time: Instant,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub status: TcpStatus,
    pub connection_established: bool,
}

#[derive(Debug, Clone)]
pub enum TcpStatus {
    Success,
    NoRouteMatched,
    NoRuleAvailable,
    PluginDenied,
    NoBackendSelected,
    NoBackendResolved,
    UpstreamConnectionFailed,
    UpstreamReadError,
    UpstreamWriteError,
    DownstreamReadError,
    DownstreamWriteError,
}

/// TCP proxy service for Gateway TCP listeners.
pub struct EdgionTcpProxy {
    pub listener_port: u16,
    pub tcp_route_manager: Arc<TcpPortRouteManager>,
    pub edgion_gateway_config: Arc<EdgionGatewayConfig>,
    pub connector: TransportConnector,
}

#[async_trait]
impl ServerApp for EdgionTcpProxy {
    async fn process_new(self: &Arc<Self>, downstream: Stream, shutdown: &ShutdownWatch) -> Option<Stream> {
        if *shutdown.borrow() {
            return None;
        }

        let (client_addr, client_port) = downstream
            .get_socket_digest()
            .and_then(|d| d.peer_addr().cloned())
            .and_then(|addr| addr.as_inet().map(|inet| (inet.ip().to_string(), inet.port())))
            .unwrap_or_else(|| ("unknown".to_string(), 0));

        let mut ctx = TcpContext {
            listener_port: self.listener_port,
            client_addr,
            client_port,
            upstream_addr: None,
            start_time: Instant::now(),
            bytes_sent: 0,
            bytes_received: 0,
            status: TcpStatus::Success,
            connection_established: false,
        };

        self.handle_connection(downstream, &mut ctx).await;

        self.log_connection(&ctx).await;

        None
    }
}

impl EdgionTcpProxy {
    async fn handle_connection(&self, downstream: Stream, ctx: &mut TcpContext) {
        // 1. Match TCPRoute from per-port route table
        let route_table = self.tcp_route_manager.load_route_table();
        let tcp_route = match route_table.match_route() {
            Some(route) => route,
            None => {
                ctx.status = TcpStatus::NoRouteMatched;
                return;
            }
        };

        // 2. Get the first rule
        let rule = match tcp_route.spec.rules.as_ref().and_then(|rules| rules.first()) {
            Some(rule) => rule,
            None => {
                ctx.status = TcpStatus::NoRuleAvailable;
                return;
            }
        };

        // 3. Execute stream plugins (dynamic lookup from StreamPluginStore)
        if let Some(store_key) = &rule.stream_plugin_store_key {
            if let Ok(client_ip) = ctx.client_addr.parse() {
                let store = get_global_stream_plugin_store();
                if let Some(resource) = store.get(store_key) {
                    let runtime = &resource.spec.stream_plugin_runtime;
                    if !runtime.is_empty() {
                        let stream_ctx = StreamContext::new(client_ip, self.listener_port);
                        match runtime.run(&stream_ctx).await {
                            StreamPluginResult::Allow => {}
                            StreamPluginResult::Deny(_) => {
                                ctx.status = TcpStatus::PluginDenied;
                                return;
                            }
                        }
                    }
                }
            }
        }

        // 4. Select backend
        let backend_ref = match rule.backend_finder.select() {
            Ok(backend) => backend,
            Err(_) => {
                ctx.status = TcpStatus::NoBackendSelected;
                return;
            }
        };

        // 5. Resolve backend address via EndpointSlice
        let namespace = backend_ref
            .namespace
            .as_deref()
            .or_else(|| tcp_route.metadata.namespace.as_deref())
            .unwrap_or("default");
        let service_key = format!("{}/{}", namespace, &backend_ref.name);

        let backend = match select_roundrobin_backend(&service_key) {
            Some(backend) => backend,
            None => {
                ctx.status = TcpStatus::NoBackendResolved;
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

        // 7. Connect to upstream
        let peer = BasicPeer::new(&upstream_addr_str);
        let upstream = match self.connector.new_stream(&peer).await {
            Ok(stream) => stream,
            Err(_) => {
                ctx.status = TcpStatus::UpstreamConnectionFailed;
                return;
            }
        };

        ctx.connection_established = true;

        // 8. Bidirectional data forwarding
        self.duplex(downstream, upstream, ctx).await;
    }

    async fn duplex(&self, mut downstream: Stream, mut upstream: Stream, ctx: &mut TcpContext) {
        const BUFFER_SIZE: usize = 8192;
        let mut upstream_buf = vec![0u8; BUFFER_SIZE];
        let mut downstream_buf = vec![0u8; BUFFER_SIZE];

        loop {
            select! {
                result = downstream.read(&mut upstream_buf) => {
                    match result {
                        Ok(0) => {
                            break;
                        }
                        Ok(n) => {
                            ctx.bytes_sent += n as u64;
                            if (upstream.write_all(&upstream_buf[0..n]).await).is_err() {
                                ctx.status = TcpStatus::UpstreamWriteError;
                                break;
                            }
                            if (upstream.flush().await).is_err() {
                                ctx.status = TcpStatus::UpstreamWriteError;
                                break;
                            }
                        }
                        Err(_) => {
                            ctx.status = TcpStatus::DownstreamReadError;
                            break;
                        }
                    }
                }
                result = upstream.read(&mut downstream_buf) => {
                    match result {
                        Ok(0) => {
                            break;
                        }
                        Ok(n) => {
                            ctx.bytes_received += n as u64;
                            if (downstream.write_all(&downstream_buf[0..n]).await).is_err() {
                                ctx.status = TcpStatus::DownstreamWriteError;
                                break;
                            }
                            if (downstream.flush().await).is_err() {
                                ctx.status = TcpStatus::DownstreamWriteError;
                                break;
                            }
                        }
                        Err(_) => {
                            ctx.status = TcpStatus::UpstreamReadError;
                            break;
                        }
                    }
                }
            }
        }
    }

    async fn log_connection(&self, ctx: &TcpContext) {
        let log_entry = TcpLogEntry::from_context(ctx);
        log_tcp(&log_entry).await;
    }
}
