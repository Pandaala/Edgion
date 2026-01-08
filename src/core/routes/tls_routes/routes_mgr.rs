use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::OnceLock;

use crate::core::routes::tls_routes::GatewayTlsRoutes;
use crate::types::resources::TLSRoute;
use crate::types::ResourceMeta;

/// TLS route manager
pub struct TlsRouteManager {
    /// resource_key -> Arc<TLSRoute> mapping
    /// For quick lookup and updates
    routes_by_key: Arc<DashMap<String, Arc<TLSRoute>>>,

    /// gateway_key -> GatewayTlsRoutes mapping
    /// Each gateway has its own set of TLS routes
    gateway_tls_routes_map: Arc<DashMap<String, Arc<GatewayTlsRoutes>>>,
}

impl Default for TlsRouteManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TlsRouteManager {
    pub fn new() -> Self {
        Self {
            routes_by_key: Arc::new(DashMap::new()),
            gateway_tls_routes_map: Arc::new(DashMap::new()),
        }
    }

    /// Get or create GatewayTlsRoutes for a specific gateway
    ///
    /// This method returns a cached GatewayTlsRoutes for the given gateway.
    /// If it doesn't exist, creates a new empty one.
    pub fn get_or_create_gateway_tls_routes(&self, namespace: &str, name: &str) -> Arc<GatewayTlsRoutes> {
        let gateway_key = format!("{}/{}", namespace, name);

        let entry = self.gateway_tls_routes_map.entry(gateway_key.clone());
        let is_new = matches!(entry, dashmap::mapref::entry::Entry::Vacant(_));

        let gateway_routes = entry
            .or_insert_with(|| Arc::new(GatewayTlsRoutes::new()))
            .value()
            .clone();

        if is_new {
            tracing::debug!(
                gateway_key = %gateway_key,
                "Created new GatewayTlsRoutes"
            );
        }

        gateway_routes
    }

    /// Rebuild gateway routes maps after route changes
    ///
    /// This method should be called after add_route, remove_route, or replace_all
    /// to update the GatewayTlsRoutes for affected gateways.
    fn rebuild_gateway_routes_map(&self) {
        // Group routes by gateway and hostname
        let mut gateway_routes: HashMap<String, HashMap<String, Vec<Arc<TLSRoute>>>> = HashMap::new();

        for entry in self.routes_by_key.iter() {
            let route = entry.value();
            let gateway_keys = self.extract_gateway_keys_from_route(route);
            let hostnames = self.extract_hostnames_from_route(route);

            for gateway_key in gateway_keys {
                let gateway_map = gateway_routes.entry(gateway_key).or_default();

                for hostname in &hostnames {
                    gateway_map
                        .entry(hostname.clone())
                        .or_default()
                        .push(route.clone());
                }
            }
        }

        // Update all gateways in the map
        // First, update gateways that have routes
        for (gateway_key, hostname_routes) in &gateway_routes {
            let hostnames_count = hostname_routes.len();
            let gateway_tls_routes = self
                .gateway_tls_routes_map
                .entry(gateway_key.clone())
                .or_insert_with(|| Arc::new(GatewayTlsRoutes::new()))
                .clone();

            gateway_tls_routes.update_routes(hostname_routes.clone());
            tracing::debug!(
                gateway_key = %gateway_key,
                hostnames = hostnames_count,
                "Updated GatewayTlsRoutes"
            );
        }

        // Clear routes for gateways that exist in map but have no routes
        for entry in self.gateway_tls_routes_map.iter() {
            let gateway_key = entry.key();
            if !gateway_routes.contains_key(gateway_key.as_str()) {
                // This gateway has no routes, clear it
                entry.value().update_routes(HashMap::new());
                tracing::debug!(
                    gateway_key = %gateway_key,
                    "Cleared GatewayTlsRoutes (no routes)"
                );
            }
        }
    }

    /// Add or update a TLSRoute
    pub fn add_route(&self, route: Arc<TLSRoute>) {
        let resource_key = route.key_name();

        // Store by resource key
        self.routes_by_key.insert(resource_key, route);

        // Rebuild gateway routes map
        self.rebuild_gateway_routes_map();
    }

    /// Remove a TLSRoute by resource key
    pub fn remove_route(&self, resource_key: &str) {
        // Remove from routes_by_key
        self.routes_by_key.remove(resource_key);

        // Rebuild gateway routes map
        self.rebuild_gateway_routes_map();
    }

    /// Replace all routes (used in full_set)
    pub fn replace_all(&self, routes: HashMap<String, Arc<TLSRoute>>) {
        // Clear and rebuild routes_by_key
        self.routes_by_key.clear();

        for (key, route) in routes {
            self.routes_by_key.insert(key, route);
        }

        // Rebuild gateway routes map
        self.rebuild_gateway_routes_map();
    }

    // Private helper methods

    fn extract_hostnames_from_route(&self, route: &TLSRoute) -> HashSet<String> {
        let mut hostnames = HashSet::new();
        if let Some(route_hostnames) = &route.spec.hostnames {
            for hostname in route_hostnames {
                hostnames.insert(hostname.clone());
            }
        }
        hostnames
    }

    fn extract_gateway_keys_from_route(&self, route: &TLSRoute) -> HashSet<String> {
        let mut gateway_keys = HashSet::new();
        if let Some(parent_refs) = &route.spec.parent_refs {
            for parent_ref in parent_refs {
                let namespace = parent_ref
                    .namespace
                    .as_deref()
                    .or(route.metadata.namespace.as_deref())
                    .unwrap_or("default");
                let gateway_key = format!("{}/{}", namespace, parent_ref.name);
                gateway_keys.insert(gateway_key);
            }
        }
        gateway_keys
    }
}

/// Global TLS route manager
static GLOBAL_TLS_ROUTE_MANAGER: OnceLock<TlsRouteManager> = OnceLock::new();

pub fn get_global_tls_route_manager() -> &'static TlsRouteManager {
    GLOBAL_TLS_ROUTE_MANAGER.get_or_init(TlsRouteManager::new)
}
