use arc_swap::ArcSwap;
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::OnceLock;

use crate::core::gateway::routes::tls::gateway_tls_routes::TlsRouteTable;
use crate::types::resources::TLSRoute;
use crate::types::ResourceMeta;

/// TLS route manager — stores all TLSRoute resources and provides
/// an atomically-swappable global route table for lock-free lookups.
///
/// Design follows the same pattern as HTTP `RouteManager`:
/// - A single `ArcSwap<TlsRouteTable>` holds the current snapshot.
/// - Consumers (EdgionTls) call `load_route_table()` per-connection.
/// - Route updates build a new `TlsRouteTable` and swap atomically.
/// - No per-gateway Arc caching — eliminates the stale-Arc problem.
pub struct TlsRouteManager {
    /// resource_key -> Arc<TLSRoute> mapping for quick lookup and updates
    routes_by_key: Arc<DashMap<String, Arc<TLSRoute>>>,

    /// Global TLS route table — atomically swapped on every route change
    global_tls_routes: ArcSwap<TlsRouteTable>,
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
            global_tls_routes: ArcSwap::from_pointee(TlsRouteTable::new()),
        }
    }

    /// Load the current route table snapshot.
    ///
    /// Called by EdgionTls on every connection — returns an Arc guard
    /// that is always up-to-date (no stale references).
    pub fn load_route_table(&self) -> arc_swap::Guard<Arc<TlsRouteTable>> {
        self.global_tls_routes.load()
    }

    /// Rebuild and atomically swap the global route table.
    ///
    /// Groups all routes by gateway_key and hostname, then builds
    /// a new immutable `TlsRouteTable` snapshot.
    fn rebuild_route_table(&self) {
        let mut gateway_routes: HashMap<String, HashMap<String, Vec<Arc<TLSRoute>>>> =
            HashMap::new();

        for entry in self.routes_by_key.iter() {
            let route = entry.value();
            let gateway_keys = Self::extract_gateway_keys(route);
            let hostnames = Self::extract_hostnames(route);

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

        let gw_count = gateway_routes.len();
        let hostname_count: usize = gateway_routes.values().map(|h| h.len()).sum();

        let new_table = TlsRouteTable::from_gateway_routes(gateway_routes);
        self.global_tls_routes.store(Arc::new(new_table));

        tracing::debug!(
            component = "tls_route_manager",
            gateways = gw_count,
            hostnames = hostname_count,
            "Rebuilt global TLS route table"
        );
    }

    /// Add or update a TLSRoute
    pub fn add_route(&self, route: Arc<TLSRoute>) {
        let resource_key = route.key_name();
        self.routes_by_key.insert(resource_key, route);
        self.rebuild_route_table();
    }

    /// Remove a TLSRoute by resource key
    pub fn remove_route(&self, resource_key: &str) {
        self.routes_by_key.remove(resource_key);
        self.rebuild_route_table();
    }

    /// Replace all routes (used in full_set)
    pub fn replace_all(&self, routes: HashMap<String, Arc<TLSRoute>>) {
        self.routes_by_key.clear();
        for (key, route) in routes {
            self.routes_by_key.insert(key, route);
        }
        self.rebuild_route_table();
    }

    fn extract_hostnames(route: &TLSRoute) -> HashSet<String> {
        let mut hostnames = HashSet::new();
        if let Some(route_hostnames) = &route.spec.hostnames {
            for hostname in route_hostnames {
                hostnames.insert(hostname.clone());
            }
        }
        hostnames
    }

    fn extract_gateway_keys(route: &TLSRoute) -> HashSet<String> {
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
