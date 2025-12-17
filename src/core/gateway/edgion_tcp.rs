use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::select;
use async_trait::async_trait;

use pingora_core::apps::ServerApp;
use pingora_core::connectors::TransportConnector;
use pingora_core::protocols::Stream;
use pingora_core::server::ShutdownWatch;
use pingora_core::upstreams::peer::BasicPeer;

use crate::core::observe::AccessLogger;
use crate::core::routes::tcp_routes::TcpRouteManager;
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfig;

/// TCP 连接上下文
pub struct TcpContext {
    pub listener_port: u16,
    pub client_addr: String,
    pub upstream_addr: Option<String>,
    pub start_time: Instant,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub status: TcpStatus,
}

#[derive(Debug, Clone)]
pub enum TcpStatus {
    Success,
    UpstreamConnectionFailed,
    UpstreamReadError,
    UpstreamWriteError,
    DownstreamReadError,
    DownstreamWriteError,
}

/// TCP 代理服务
pub struct EdgionTcp {
    pub gateway_name: String,
    pub listener_port: u16,
    pub tcp_route_manager: &'static TcpRouteManager,
    pub access_logger: Arc<AccessLogger>,
    pub edgion_gateway_config: Arc<EdgionGatewayConfig>,
    pub connector: TransportConnector,
}

#[async_trait]
impl ServerApp for EdgionTcp {
    async fn process_new(
        self: &Arc<Self>,
        downstream: Stream,
        _shutdown: &ShutdownWatch,
    ) -> Option<Stream> {
        // 创建上下文
        let mut ctx = TcpContext {
            listener_port: self.listener_port,
            client_addr: "unknown".to_string(), // TODO: 从 Stream 提取
            upstream_addr: None,
            start_time: Instant::now(),
            bytes_sent: 0,
            bytes_received: 0,
            status: TcpStatus::Success,
        };
        
        // 匹配路由
        let tcp_route = match self.tcp_route_manager.match_route(self.listener_port) {
            Some(route) => route,
            None => {
                tracing::warn!(
                    port = self.listener_port,
                    "No TCPRoute found for port"
                );
                return None;
            }
        };
        
        // 选择后端
        let backend_ref = match tcp_route.spec.rules.as_ref()
            .and_then(|rules| rules.first())
        {
            Some(rule) => {
                match rule.backend_finder.select() {
                    Ok(backend) => backend,
                    Err(e) => {
                        tracing::error!(
                            error_code = e,
                            "Failed to select backend"
                        );
                        ctx.status = TcpStatus::UpstreamConnectionFailed;
                        self.log_connection(&ctx).await;
                        return None;
                    }
                }
            }
            None => {
                tracing::error!("No TCPRoute rules found");
                ctx.status = TcpStatus::UpstreamConnectionFailed;
                self.log_connection(&ctx).await;
                return None;
            }
        };
        
        // 构建上游地址
        let upstream_addr = format!("{}:{}", 
            backend_ref.name,
            backend_ref.port.unwrap_or(self.listener_port as i32)
        );
        ctx.upstream_addr = Some(upstream_addr.clone());
        
        // 连接上游
        let peer = BasicPeer::new(&upstream_addr);
        let upstream = match self.connector.new_stream(&peer).await {
            Ok(stream) => stream,
            Err(e) => {
                tracing::error!(
                    upstream = %upstream_addr,
                    error = %e,
                    "Failed to connect to upstream"
                );
                ctx.status = TcpStatus::UpstreamConnectionFailed;
                self.log_connection(&ctx).await;
                return None;
            }
        };
        
        tracing::info!(
            port = self.listener_port,
            upstream = %upstream_addr,
            "TCP connection established"
        );
        
        // 双工转发
        self.duplex(downstream, upstream, &mut ctx).await;
        
        // 记录访问日志
        self.log_connection(&ctx).await;
        
        None
    }
}

impl EdgionTcp {
    /// 双向数据转发
    async fn duplex(
        &self,
        mut downstream: Stream,
        mut upstream: Stream,
        ctx: &mut TcpContext,
    ) {
        const BUFFER_SIZE: usize = 8192;
        let mut upstream_buf = vec![0u8; BUFFER_SIZE];
        let mut downstream_buf = vec![0u8; BUFFER_SIZE];
        
        loop {
            select! {
                // Client → Upstream
                result = downstream.read(&mut upstream_buf) => {
                    match result {
                        Ok(0) => {
                            tracing::debug!("Client closed connection");
                            break;
                        }
                        Ok(n) => {
                            ctx.bytes_sent += n as u64;
                            if let Err(e) = upstream.write_all(&upstream_buf[0..n]).await {
                                tracing::error!("Failed to write to upstream: {}", e);
                                ctx.status = TcpStatus::UpstreamWriteError;
                                break;
                            }
                            if let Err(e) = upstream.flush().await {
                                tracing::error!("Failed to flush upstream: {}", e);
                                ctx.status = TcpStatus::UpstreamWriteError;
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::error!("Failed to read from client: {}", e);
                            ctx.status = TcpStatus::DownstreamReadError;
                            break;
                        }
                    }
                }
                // Upstream → Client
                result = upstream.read(&mut downstream_buf) => {
                    match result {
                        Ok(0) => {
                            tracing::debug!("Upstream closed connection");
                            break;
                        }
                        Ok(n) => {
                            ctx.bytes_received += n as u64;
                            if let Err(e) = downstream.write_all(&downstream_buf[0..n]).await {
                                tracing::error!("Failed to write to client: {}", e);
                                ctx.status = TcpStatus::DownstreamWriteError;
                                break;
                            }
                            if let Err(e) = downstream.flush().await {
                                tracing::error!("Failed to flush downstream: {}", e);
                                ctx.status = TcpStatus::DownstreamWriteError;
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::error!("Failed to read from upstream: {}", e);
                            ctx.status = TcpStatus::UpstreamReadError;
                            break;
                        }
                    }
                }
            }
        }
    }
    
    /// 记录访问日志
    async fn log_connection(&self, ctx: &TcpContext) {
        let duration_ms = ctx.start_time.elapsed().as_millis() as u64;
        
        let log_entry = serde_json::json!({
            "ts": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
            "protocol": "TCP",
            "listener_port": ctx.listener_port,
            "client_addr": &ctx.client_addr,
            "upstream_addr": &ctx.upstream_addr,
            "duration_ms": duration_ms,
            "bytes_sent": ctx.bytes_sent,
            "bytes_received": ctx.bytes_received,
            "status": format!("{:?}", ctx.status),
        });
        
        self.access_logger.send(log_entry.to_string()).await;
    }
}

