use crate::types::resources::TLSRoute;
use std::collections::HashMap;
use std::sync::Arc;

/// Global TLS route table — immutable snapshot shared via ArcSwap.
///
/// Routes are indexed by (gateway_key, hostname) for O(1) lookup.
/// A new snapshot is built and atomically swapped on every route change,
/// ensuring all readers (EdgionTls instances) always see consistent data
/// without caching stale Arc references.
pub struct TlsRouteTable {
    /// gateway_key -> hostname -> Vec<Arc<TLSRoute>>
    gateway_routes: HashMap<String, HashMap<String, Vec<Arc<TLSRoute>>>>,
}

impl TlsRouteTable {
    pub fn new() -> Self {
        Self {
            gateway_routes: HashMap::new(),
        }
    }

    pub fn from_gateway_routes(
        gateway_routes: HashMap<String, HashMap<String, Vec<Arc<TLSRoute>>>>,
    ) -> Self {
        Self { gateway_routes }
    }

    /// Match a TLSRoute by gateway key and SNI hostname.
    ///
    /// Priority: exact hostname > wildcard hostname.
    pub fn match_route(&self, gateway_key: &str, sni_hostname: &str) -> Option<Arc<TLSRoute>> {
        let hostname_routes = self.gateway_routes.get(gateway_key)?;

        // Exact match first
        if let Some(routes) = hostname_routes.get(sni_hostname) {
            if let Some(route) = routes.first() {
                return Some(route.clone());
            }
        }

        // Wildcard matching: *.example.com matches test.example.com
        if let Some(dot_pos) = sni_hostname.find('.') {
            let wildcard = format!("*{}", &sni_hostname[dot_pos..]);
            if let Some(routes) = hostname_routes.get(&wildcard) {
                if let Some(route) = routes.first() {
                    return Some(route.clone());
                }
            }
        }

        None
    }

    pub fn gateway_count(&self) -> usize {
        self.gateway_routes.len()
    }

    pub fn total_hostname_count(&self) -> usize {
        self.gateway_routes.values().map(|h| h.len()).sum()
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
            },
            status: None,
        }
    }

    #[test]
    fn test_exact_match() {
        let route1 = Arc::new(create_test_tls_route("default", "route1", "test.example.com"));
        let route2 = Arc::new(create_test_tls_route("default", "route2", "api.example.com"));

        let mut hostname_routes = HashMap::new();
        hostname_routes.insert("test.example.com".to_string(), vec![route1]);
        hostname_routes.insert("api.example.com".to_string(), vec![route2]);

        let mut gateway_routes = HashMap::new();
        gateway_routes.insert("default/test-gateway".to_string(), hostname_routes);

        let table = TlsRouteTable::from_gateway_routes(gateway_routes);

        assert!(table.match_route("default/test-gateway", "test.example.com").is_some());
        assert!(table.match_route("default/test-gateway", "api.example.com").is_some());
        assert!(table.match_route("default/test-gateway", "other.example.com").is_none());
        assert!(table.match_route("default/other-gateway", "test.example.com").is_none());
    }

    #[test]
    fn test_wildcard_match() {
        let route = Arc::new(create_test_tls_route("default", "route1", "*.example.com"));

        let mut hostname_routes = HashMap::new();
        hostname_routes.insert("*.example.com".to_string(), vec![route]);

        let mut gateway_routes = HashMap::new();
        gateway_routes.insert("default/test-gateway".to_string(), hostname_routes);

        let table = TlsRouteTable::from_gateway_routes(gateway_routes);

        assert!(table.match_route("default/test-gateway", "test.example.com").is_some());
        assert!(table.match_route("default/test-gateway", "api.example.com").is_some());
        assert!(table.match_route("default/test-gateway", "example.com").is_none());
    }

    #[test]
    fn test_empty_table() {
        let table = TlsRouteTable::new();
        assert!(table.match_route("any/gateway", "test.example.com").is_none());
        assert_eq!(table.gateway_count(), 0);
        assert_eq!(table.total_hostname_count(), 0);
    }
}
