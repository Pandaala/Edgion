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
use crate::core::routes::tcp_routes::GatewayTcpRoutes;
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfig;
use crate::core::backends::endpoint_slice::get_roundrobin_store;

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
    pub gateway_namespace: Option<String>,
    pub listener_port: u16,
    pub gateway_tcp_routes: Arc<GatewayTcpRoutes>,
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

        // 处理连接（无论成功或失败都会更新 ctx）
        self.handle_connection(downstream, &mut ctx).await;
        
        // 统一记录访问日志
        self.log_connection(&ctx).await;
        
        None
    }
}

impl EdgionTcp {
    /// 处理 TCP 连接的核心逻辑
    async fn handle_connection(&self, downstream: Stream, ctx: &mut TcpContext) {
        // 1. 匹配 TCPRoute
        let tcp_route = match self.gateway_tcp_routes.match_route(self.listener_port) {
            Some(route) => route,
            None => {
                ctx.status = TcpStatus::UpstreamConnectionFailed;
                return;
            }
        };
        
        // 2. 选择后端
        let backend_ref = match tcp_route.spec.rules.as_ref()
            .and_then(|rules| rules.first())
        {
            Some(rule) => {
                match rule.backend_finder.select() {
                    Ok(backend) => backend,
                    Err(_) => {
                        ctx.status = TcpStatus::UpstreamConnectionFailed;
                        return;
                    }
                }
            }
            None => {
                ctx.status = TcpStatus::UpstreamConnectionFailed;
                return;
            }
        };
        
        // 3. 通过 EndpointSlice 解析后端地址
        let namespace = backend_ref.namespace.as_deref()
            .or_else(|| tcp_route.metadata.namespace.as_deref())
            .unwrap_or("default");
        let service_key = format!("{}/{}", namespace, &backend_ref.name);
        
        // 从 EndpointSlice store 选择后端
        let ep_store = get_roundrobin_store();
        let backend = match ep_store.select_peer(&service_key, b"", 256) {
            Some(backend) => backend,
            None => {
                ctx.status = TcpStatus::UpstreamConnectionFailed;
                return;
            }
        };
        
        // 4. 构建上游地址（使用实际的 IP 地址）
        let mut upstream_addr = backend.addr;
        if let Some(port) = backend_ref.port {
            upstream_addr.set_port(port as u16);
        }
        let upstream_addr_str = upstream_addr.to_string();
        ctx.upstream_addr = Some(upstream_addr_str.clone());
        
        // 5. 连接上游
        let peer = BasicPeer::new(&upstream_addr_str);
        let upstream = match self.connector.new_stream(&peer).await {
            Ok(stream) => stream,
            Err(_) => {
                ctx.status = TcpStatus::UpstreamConnectionFailed;
                return;
            }
        };
        
        // 6. 双向数据转发
        self.duplex(downstream, upstream, ctx).await;
    }
    
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
                        break;
                    }
                    Ok(n) => {
                        ctx.bytes_sent += n as u64;
                        if let Err(_) = upstream.write_all(&upstream_buf[0..n]).await {
                            ctx.status = TcpStatus::UpstreamWriteError;
                            break;
                        }
                        if let Err(_) = upstream.flush().await {
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
            // Upstream → Client
            result = upstream.read(&mut downstream_buf) => {
                match result {
                    Ok(0) => {
                        break;
                    }
                    Ok(n) => {
                        ctx.bytes_received += n as u64;
                        if let Err(_) = downstream.write_all(&downstream_buf[0..n]).await {
                            ctx.status = TcpStatus::DownstreamWriteError;
                            break;
                        }
                        if let Err(_) = downstream.flush().await {
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

