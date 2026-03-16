use crate::core::common::matcher::HashHost;
use crate::types::resources::TLSRoute;
use std::collections::HashMap;
use std::sync::Arc;

/// Per-port TLS route table — immutable snapshot shared via ArcSwap.
///
/// Routes are indexed directly by hostname for O(1) lookup. The gateway_key
/// dimension has been lifted to the outer `GlobalTlsRouteManagers` layer that
/// assigns routes to port-specific managers, so this table only needs to care
/// about hostname matching within a single port.
///
/// A new snapshot is built and atomically swapped on every route change,
/// ensuring all readers (`EdgionTlsTcpProxy` instances) always see consistent
/// data without caching stale Arc references.
pub struct TlsRouteTable {
    /// Hostname matcher (exact + wildcard) using `HashHost`.
    ///
    /// `HashHost` supports both exact and wildcard (`*.example.com`) lookups
    /// with O(W) wildcard matching where W = number of distinct wildcard suffix
    /// lengths, and automatically returns the most-specific wildcard match.
    host_map: HashHost<Vec<Arc<TLSRoute>>>,

    /// Routes with no hostname specified.
    /// Reserved for future use; not populated in phase 1.
    catch_all_routes: Option<Vec<Arc<TLSRoute>>>,
}

impl TlsRouteTable {
    pub fn new() -> Self {
        Self {
            host_map: HashHost::new(),
            catch_all_routes: None,
        }
    }

    /// Build a TlsRouteTable from a flat set of routes belonging to one port.
    ///
    /// Each route's `spec.hostnames` are used to bucket into the `HashHost`
    /// matcher. Routes with no hostnames go to `catch_all_routes`.
    pub fn from_routes(routes: &HashMap<String, Arc<TLSRoute>>) -> Self {
        let mut buckets: HashMap<String, Vec<Arc<TLSRoute>>> = HashMap::new();
        let mut catch_all: Vec<Arc<TLSRoute>> = Vec::new();

        for route in routes.values() {
            let hostnames: Vec<String> = route
                .spec
                .hostnames
                .as_ref()
                .map(|h| h.to_vec())
                .unwrap_or_default();

            if hostnames.is_empty() {
                catch_all.push(route.clone());
                continue;
            }

            for hostname in &hostnames {
                let lower = hostname.to_ascii_lowercase();
                buckets.entry(lower).or_default().push(route.clone());
            }
        }

        let mut host_map: HashHost<Vec<Arc<TLSRoute>>> = HashHost::new();
        for (hostname, route_vec) in buckets {
            host_map.insert(&hostname, route_vec);
        }

        Self {
            host_map,
            catch_all_routes: if catch_all.is_empty() {
                None
            } else {
                Some(catch_all)
            },
        }
    }

    /// Match a TLSRoute by SNI hostname.
    ///
    /// Priority: exact hostname > most-specific wildcard > catch-all.
    /// `HashHost` handles exact-vs-wildcard priority and most-specific-wildcard
    /// ordering internally.
    pub fn match_route(&self, sni_hostname: &str) -> Option<Arc<TLSRoute>> {
        let lower_sni = sni_hostname.to_ascii_lowercase();

        if let Some(routes) = self.host_map.get(&lower_sni) {
            if let Some(route) = routes.first() {
                return Some(route.clone());
            }
        }

        if let Some(routes) = &self.catch_all_routes {
            if let Some(route) = routes.first() {
                return Some(route.clone());
            }
        }

        None
    }

    pub fn has_catch_all(&self) -> bool {
        self.catch_all_routes.is_some()
    }
}

impl Default for TlsRouteTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::common::ParentReference;
    use crate::types::resources::tls_route::*;
    use crate::types::ResourceMeta;

    fn create_test_tls_route(namespace: &str, name: &str, hostname: &str) -> TLSRoute {
        TLSRoute {
            metadata: kube::api::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: TLSRouteSpec {
                parent_refs: Some(vec![ParentReference {
                    group: Some("gateway.networking.k8s.io".to_string()),
                    kind: Some("Gateway".to_string()),
                    namespace: Some(namespace.to_string()),
                    name: "test-gateway".to_string(),
                    section_name: None,
                    port: None,
                }]),
                hostnames: Some(vec![hostname.to_string()]),
                rules: Some(vec![]),
                resolved_ports: None,
            },
            status: None,
        }
    }

    fn create_test_tls_route_no_hostname(namespace: &str, name: &str) -> TLSRoute {
        TLSRoute {
            metadata: kube::api::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: TLSRouteSpec {
                parent_refs: Some(vec![ParentReference {
                    group: Some("gateway.networking.k8s.io".to_string()),
                    kind: Some("Gateway".to_string()),
                    namespace: Some(namespace.to_string()),
                    name: "test-gateway".to_string(),
                    section_name: None,
                    port: None,
                }]),
                hostnames: None,
                rules: Some(vec![]),
                resolved_ports: None,
            },
            status: None,
        }
    }

    #[test]
    fn test_exact_match() {
        let route1 = Arc::new(create_test_tls_route("default", "route1", "test.example.com"));
        let route2 = Arc::new(create_test_tls_route("default", "route2", "api.example.com"));

        let mut routes = HashMap::new();
        routes.insert(route1.key_name(), route1);
        routes.insert(route2.key_name(), route2);

        let table = TlsRouteTable::from_routes(&routes);

        assert!(table.match_route("test.example.com").is_some());
        assert!(table.match_route("api.example.com").is_some());
        assert!(table.match_route("other.example.com").is_none());
    }
    
    #[test]
    fn test_empty_table() {
        let table = TlsRouteTable::new();
        assert!(table.match_route("test.example.com").is_none());
        assert!(!table.has_catch_all());
    }
}
