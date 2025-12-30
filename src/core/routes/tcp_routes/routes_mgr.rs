use std::sync::Arc;
use std::collections::{HashMap, HashSet};
use dashmap::DashMap;
use std::sync::OnceLock;

use crate::types::resources::TCPRoute;
use crate::types::ResourceMeta;
use crate::core::routes::tcp_routes::GatewayTcpRoutes;

/// TCP 路由管理器
pub struct TcpRouteManager {
    /// resource_key -> Arc<TCPRoute> mapping
    /// For quick lookup and updates
    routes_by_key: Arc<DashMap<String, Arc<TCPRoute>>>,
    
    /// gateway_key -> GatewayTcpRoutes mapping
    /// Each gateway has its own set of TCP routes
    gateway_tcp_routes_map: Arc<DashMap<String, Arc<GatewayTcpRoutes>>>,
}

impl TcpRouteManager {
    pub fn new() -> Self {
        Self {
            routes_by_key: Arc::new(DashMap::new()),
            gateway_tcp_routes_map: Arc::new(DashMap::new()),
        }
    }
    
    /// Get or create GatewayTcpRoutes for a specific gateway
    /// 
    /// This method returns a cached GatewayTcpRoutes for the given gateway.
    /// If it doesn't exist, creates a new empty one.
    pub fn get_or_create_gateway_tcp_routes(&self, namespace: &str, name: &str) -> Arc<GatewayTcpRoutes> {
        let gateway_key = format!("{}/{}", namespace, name);
        
        let entry = self.gateway_tcp_routes_map.entry(gateway_key.clone());
        let is_new = matches!(entry, dashmap::mapref::entry::Entry::Vacant(_));
        
        let gateway_routes = entry
            .or_insert_with(|| Arc::new(GatewayTcpRoutes::new()))
            .value()
            .clone();
        
        if is_new {
            tracing::debug!(
                gateway_key = %gateway_key,
                "Created new GatewayTcpRoutes"
            );
        }
        
        gateway_routes
    }
    
    /// Build affected gateway->ports mapping from changed route keys
    /// 
    /// Returns a map of gateway_key -> set of affected ports
    /// This helps identify which specific gateway/port combinations need rebuilding
    fn build_affected_gateway_ports(
        &self,
        changed_route_keys: &HashSet<String>,
        removed_route_keys: &HashSet<String>,
    ) -> HashMap<String, HashSet<u16>> {
        let mut affected: HashMap<String, HashSet<u16>> = HashMap::new();
        
        // Process changed routes (from current state)
        for route_key in changed_route_keys {
            if let Some(route) = self.routes_by_key.get(route_key) {
                let gateways = self.extract_gateway_keys_from_route(&route);
                let ports = self.extract_ports_from_route(&route);
                
                for gateway_key in gateways {
                    affected.entry(gateway_key)
                        .or_insert_with(HashSet::new)
                        .extend(ports.iter().copied());
                }
            }
        }
        
        // Process removed routes (need to lookup before removal)
        for route_key in removed_route_keys {
            if let Some(route) = self.routes_by_key.get(route_key) {
                let gateways = self.extract_gateway_keys_from_route(&route);
                let ports = self.extract_ports_from_route(&route);
                
                for gateway_key in gateways {
                    affected.entry(gateway_key)
                        .or_insert_with(HashSet::new)
                        .extend(ports.iter().copied());
                }
            }
        }
        
        affected
    }
    
    /// Rebuild specified ports for a gateway (incremental update)
    /// 
    /// Only rebuilds the affected ports, leaving other ports unchanged.
    /// This is much more efficient than rebuilding all ports.
    fn rebuild_gateway_ports_incremental(
        &self,
        gateway_key: &str,
        affected_ports: &HashSet<u16>,
        removed_route_keys: &HashSet<String>,
    ) {
        // Rebuild only affected ports for this gateway
        let mut port_routes: HashMap<u16, Vec<Arc<TCPRoute>>> = HashMap::new();
        
        // Collect routes for this gateway's affected ports
        for entry in self.routes_by_key.iter() {
            let route_key = entry.key();
            
            // Skip removed routes
            if removed_route_keys.contains(route_key.as_str()) {
                continue;
            }
            
            let route = entry.value();
            let route_gateways = self.extract_gateway_keys_from_route(route);
            let route_ports = self.extract_ports_from_route(route);
            
            // Check if this route applies to our gateway
            if !route_gateways.contains(gateway_key) {
                continue;
            }
            
            // Add to affected ports only
            for port in route_ports.iter().filter(|p| affected_ports.contains(p)) {
                port_routes
                    .entry(*port)
                    .or_insert_with(Vec::new)
                    .push(route.clone());
            }
        }
        
        // Update only affected ports in the gateway
        if let Some(gateway_tcp_routes) = self.gateway_tcp_routes_map.get(gateway_key) {
            gateway_tcp_routes.update_ports_incremental(port_routes);
            tracing::debug!(
                gateway_key = %gateway_key,
                ports = affected_ports.len(),
                "Incrementally updated TCPRoute ports"
            );
        }
    }
    
    /// Rebuild gateway routes maps after route changes (full rebuild, used for replace_all)
    /// 
    /// This method should be called after replace_all to do a full rebuild.
    /// For add_route and remove_route, use incremental update instead.
    fn rebuild_gateway_routes_map(&self) {
        // Group routes by gateway
        let mut gateway_routes: HashMap<String, HashMap<u16, Vec<Arc<TCPRoute>>>> = HashMap::new();
        
        for entry in self.routes_by_key.iter() {
            let route = entry.value();
            let gateway_keys = self.extract_gateway_keys_from_route(route);
            let ports = self.extract_ports_from_route(route);
            
            for gateway_key in gateway_keys {
                let gateway_map = gateway_routes
                    .entry(gateway_key)
                    .or_insert_with(HashMap::new);
                
                for port in &ports {
                    gateway_map
                        .entry(*port)
                        .or_insert_with(Vec::new)
                        .push(route.clone());
                }
            }
        }
        
        // Update all gateways in the map
        // First, update gateways that have routes
        for (gateway_key, port_routes) in &gateway_routes {
            let ports_count = port_routes.len();
            let gateway_tcp_routes = self.gateway_tcp_routes_map
                .entry(gateway_key.clone())
                .or_insert_with(|| Arc::new(GatewayTcpRoutes::new()))
                .clone();
            
            gateway_tcp_routes.update_routes(port_routes.clone());
            tracing::debug!(
                gateway_key = %gateway_key,
                ports = ports_count,
                "Updated GatewayTcpRoutes"
            );
        }
        
        // Clear routes for gateways that exist in map but have no routes
        for entry in self.gateway_tcp_routes_map.iter() {
            let gateway_key = entry.key();
            if !gateway_routes.contains_key(gateway_key.as_str()) {
                // This gateway has no routes, clear it
                entry.value().update_routes(HashMap::new());
                tracing::debug!(
                    gateway_key = %gateway_key,
                    "Cleared GatewayTcpRoutes (no routes)"
                );
            }
        }
    }
    
    /// Add or update a TCPRoute (uses incremental update)
    pub fn add_route(&self, route: Arc<TCPRoute>) {
        let resource_key = route.key_name();
        
        // Calculate affected gateways/ports BEFORE insertion
        let mut changed_keys = HashSet::new();
        changed_keys.insert(resource_key.clone());
        let affected = self.build_affected_gateway_ports(&changed_keys, &HashSet::new());
        
        // Store by resource key
        self.routes_by_key.insert(resource_key.clone(), route);
        
        let affected_count = affected.len();
        
        // Rebuild only affected gateway/ports (incremental)
        for (gateway_key, ports) in affected {
            self.rebuild_gateway_ports_incremental(&gateway_key, &ports, &HashSet::new());
        }
        
        tracing::info!(
            route_key = %resource_key,
            affected_gateways = affected_count,
            "Added/updated TCPRoute with incremental update"
        );
    }
    
    /// Remove a TCPRoute by resource key (uses incremental update)
    pub fn remove_route(&self, resource_key: &str) {
        // Calculate affected gateways/ports BEFORE removal
        let mut removed_keys = HashSet::new();
        removed_keys.insert(resource_key.to_string());
        let affected = self.build_affected_gateway_ports(&HashSet::new(), &removed_keys);
        
        let affected_count = affected.len();
        
        // Remove from routes_by_key
        self.routes_by_key.remove(resource_key);
        
        // Rebuild only affected gateway/ports (incremental)
        for (gateway_key, ports) in &affected {
            self.rebuild_gateway_ports_incremental(gateway_key, ports, &removed_keys);
        }
        
        tracing::info!(
            route_key = %resource_key,
            affected_gateways = affected_count,
            "Removed TCPRoute with incremental update"
        );
    }
    
    /// Replace all routes (used in full_set)
    pub fn replace_all(&self, routes: HashMap<String, Arc<TCPRoute>>) {
        // Clear and rebuild routes_by_key
        self.routes_by_key.clear();
        
        for (key, route) in routes {
            self.routes_by_key.insert(key, route);
        }
        
        // Rebuild gateway routes map
        self.rebuild_gateway_routes_map();
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
}

/// 全局 TCP 路由管理器
static GLOBAL_TCP_ROUTE_MANAGER: OnceLock<TcpRouteManager> = OnceLock::new();

pub fn get_global_tcp_route_manager() -> &'static TcpRouteManager {
    GLOBAL_TCP_ROUTE_MANAGER.get_or_init(|| TcpRouteManager::new())
}
