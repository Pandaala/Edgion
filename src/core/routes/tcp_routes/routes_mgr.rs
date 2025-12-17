use std::sync::Arc;
use dashmap::DashMap;
use once_cell::sync::OnceCell;

use crate::types::resources::TCPRoute;

/// TCP 路由管理器 - 按监听端口索引 TCPRoute
pub struct TcpRouteManager {
    /// port -> TCPRoute mapping
    routes_by_port: Arc<DashMap<u16, Vec<Arc<TCPRoute>>>>,
}

impl TcpRouteManager {
    pub fn new() -> Self {
        Self {
            routes_by_port: Arc::new(DashMap::new()),
        }
    }
    
    /// 添加 TCPRoute
    pub fn add_route(&self, port: u16, route: Arc<TCPRoute>) {
        self.routes_by_port
            .entry(port)
            .or_insert_with(Vec::new)
            .push(route);
    }
    
    /// 根据端口匹配 TCPRoute
    pub fn match_route(&self, port: u16) -> Option<Arc<TCPRoute>> {
        self.routes_by_port
            .get(&port)
            .and_then(|routes| routes.first().cloned())
    }
    
    /// 移除指定端口的所有路由
    pub fn remove_routes(&self, port: u16) {
        self.routes_by_port.remove(&port);
    }
}

/// 全局 TCP 路由管理器
static GLOBAL_TCP_ROUTE_MANAGER: OnceCell<TcpRouteManager> = OnceCell::new();

pub fn get_global_tcp_route_manager() -> &'static TcpRouteManager {
    GLOBAL_TCP_ROUTE_MANAGER.get_or_init(|| TcpRouteManager::new())
}

