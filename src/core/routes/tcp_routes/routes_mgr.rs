use std::sync::Arc;
use std::collections::{HashMap, HashSet};
use dashmap::DashMap;
use once_cell::sync::OnceCell;

use crate::types::resources::TCPRoute;
use crate::types::ResourceMeta;

/// TCP 路由管理器
pub struct TcpRouteManager {
    /// port -> Vec<Arc<TCPRoute>> mapping
    /// Multiple routes can listen on the same port (different gateways)
    routes_by_port: Arc<DashMap<u16, Vec<Arc<TCPRoute>>>>,
    
    /// gateway_key -> Set<port> mapping
    /// Tracks which ports each gateway is using
    gateway_ports: Arc<DashMap<String, HashSet<u16>>>,
    
    /// resource_key -> Arc<TCPRoute> mapping
    /// For quick lookup and updates
    routes_by_key: Arc<DashMap<String, Arc<TCPRoute>>>,
}

impl TcpRouteManager {
    pub fn new() -> Self {
        Self {
            routes_by_port: Arc::new(DashMap::new()),
            gateway_ports: Arc::new(DashMap::new()),
            routes_by_key: Arc::new(DashMap::new()),
        }
    }
    
    /// Add or update a TCPRoute
    pub fn add_route(&self, route: Arc<TCPRoute>) {
        let resource_key = route.key_name();
        
        // Extract ports from parent_refs
        let ports = self.extract_ports_from_route(&route);
        
        // Extract gateway keys from parent_refs
        let gateway_keys = self.extract_gateway_keys_from_route(&route);
        
        // Store by resource key
        self.routes_by_key.insert(resource_key.clone(), route.clone());
        
        // Index by port
        for port in &ports {
            self.routes_by_port
                .entry(*port)
                .or_insert_with(Vec::new)
                .push(route.clone());
        }
        
        // Track gateway -> port mapping
        for gateway_key in &gateway_keys {
            self.gateway_ports
                .entry(gateway_key.clone())
                .or_insert_with(HashSet::new)
                .extend(&ports);
        }
    }
    
    /// Remove a TCPRoute by resource key
    pub fn remove_route(&self, resource_key: &str) {
        if let Some((_, route)) = self.routes_by_key.remove(resource_key) {
            let ports = self.extract_ports_from_route(&route);
            let gateway_keys = self.extract_gateway_keys_from_route(&route);
            
            // Remove from port index
            for port in &ports {
                if let Some(mut routes) = self.routes_by_port.get_mut(port) {
                    routes.retain(|r| r.key_name() != resource_key);
                    if routes.is_empty() {
                        drop(routes);
                        self.routes_by_port.remove(port);
                    }
                }
            }
            
            // Clean up gateway -> port mapping
            for gateway_key in &gateway_keys {
                if let Some(mut port_set) = self.gateway_ports.get_mut(gateway_key.as_str()) {
                    for port in &ports {
                        port_set.remove(port);
                    }
                    if port_set.is_empty() {
                        drop(port_set);
                        self.gateway_ports.remove(gateway_key.as_str());
                    }
                }
            }
        }
    }
    
    /// Match a TCPRoute by listener port and gateway
    pub fn match_route(&self, port: u16, gateway_key: Option<&str>) -> Option<Arc<TCPRoute>> {
        self.routes_by_port.get(&port).and_then(|routes| {
            if let Some(gw_key) = gateway_key {
                // Find route matching this gateway
                routes.iter()
                    .find(|r| self.route_matches_gateway(r, gw_key))
                    .cloned()
            } else {
                // Return first available route
                routes.first().cloned()
            }
        })
    }
    
    /// Replace all routes (used in full_set)
    pub fn replace_all(&self, routes: HashMap<String, Arc<TCPRoute>>) {
        // Clear all existing data
        self.routes_by_key.clear();
        self.routes_by_port.clear();
        self.gateway_ports.clear();
        
        // Add all new routes
        for route in routes.values() {
            self.add_route(route.clone());
        }
    }
    
    /// Get all routes for a specific gateway
    pub fn get_routes_for_gateway(&self, gateway_key: &str) -> Vec<Arc<TCPRoute>> {
        if let Some(ports) = self.gateway_ports.get(gateway_key) {
            let mut routes = Vec::new();
            for port in ports.iter() {
                if let Some(port_routes) = self.routes_by_port.get(port) {
                    for route in port_routes.iter() {
                        if self.route_matches_gateway(route, gateway_key) {
                            routes.push(route.clone());
                        }
                    }
                }
            }
            routes
        } else {
            Vec::new()
        }
    }
    
    // Private helper methods
    
    fn extract_ports_from_route(&self, route: &TCPRoute) -> HashSet<u16> {
        let mut ports = HashSet::new();
        if let Some(parent_refs) = &route.spec.parent_refs {
            for parent_ref in parent_refs {
                if let Some(port) = parent_ref.port {
                    ports.insert(port as u16);
                }
            }
        }
        ports
    }
    
    fn extract_gateway_keys_from_route(&self, route: &TCPRoute) -> HashSet<String> {
        let mut gateway_keys = HashSet::new();
        if let Some(parent_refs) = &route.spec.parent_refs {
            for parent_ref in parent_refs {
                let namespace = parent_ref.namespace.as_deref()
                    .or_else(|| route.metadata.namespace.as_deref())
                    .unwrap_or("default");
                let gateway_key = format!("{}/{}", namespace, parent_ref.name);
                gateway_keys.insert(gateway_key);
            }
        }
        gateway_keys
    }
    
    fn route_matches_gateway(&self, route: &TCPRoute, gateway_key: &str) -> bool {
        let gateway_keys = self.extract_gateway_keys_from_route(route);
        gateway_keys.contains(gateway_key)
    }
}

/// 全局 TCP 路由管理器
static GLOBAL_TCP_ROUTE_MANAGER: OnceCell<TcpRouteManager> = OnceCell::new();

pub fn get_global_tcp_route_manager() -> &'static TcpRouteManager {
    GLOBAL_TCP_ROUTE_MANAGER.get_or_init(|| TcpRouteManager::new())
}
