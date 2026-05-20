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

/// Deterministic precedence for TLSRoutes colliding on the same hostname.
///
/// Implements the Gateway API conflict-resolution rule (oldest `creationTimestamp`
/// wins, then namespace/name), matching `BackendTLSPolicyStore` precedence. The
/// winner is `routes.first()` after this sort, so equal-precedence overlapping
/// routes select deterministically instead of by HashMap iteration order.
fn tls_route_precedence(a: &Arc<TLSRoute>, b: &Arc<TLSRoute>) -> std::cmp::Ordering {
    a.metadata
        .creation_timestamp
        .cmp(&b.metadata.creation_timestamp)
        .then_with(|| a.metadata.namespace.cmp(&b.metadata.namespace))
        .then_with(|| a.metadata.name.cmp(&b.metadata.name))
}

/// Warn (route-rebuild path only — never `match_route`) when several TLSRoutes share
/// one hostname bucket. `routes` must already be sorted by [`tls_route_precedence`],
/// so `first()` is the selected route.
fn warn_tls_route_collision(hostname: &str, routes: &[Arc<TLSRoute>]) {
    let fmt = |r: &Arc<TLSRoute>| {
        format!(
            "{}/{}",
            r.metadata.namespace.as_deref().unwrap_or(""),
            r.metadata.name.as_deref().unwrap_or("")
        )
    };
    let participants = routes.iter().map(fmt).collect::<Vec<_>>().join(", ");
    let selected = routes.first().map(fmt).unwrap_or_default();
    tracing::warn!(
        component = "tls_route_table",
        hostname = %hostname,
        participants = %participants,
        selected = %selected,
        "Multiple TLSRoutes serve the same hostname on one listener; \
         selecting the route deterministically by (creationTimestamp, namespace, name)"
    );
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
    /// When the controller has set `resolved_listeners`, we use
    /// `effective_hostnames` (the route×listener intersection) for bucketing.
    /// Otherwise we fall back to raw `spec.hostnames` for backward compat.
    /// A processed route with explicit hostnames but an empty intersection must
    /// not become catch-all.
    pub fn from_routes(routes: &HashMap<String, Arc<TLSRoute>>) -> Self {
        let mut buckets: HashMap<String, Vec<Arc<TLSRoute>>> = HashMap::new();
        let mut catch_all: Vec<Arc<TLSRoute>> = Vec::new();

        for route in routes.values() {
            let hostnames: Vec<String> = if route.spec.resolved_listeners.is_some() {
                // Controller has processed this route; use the computed intersection.
                route.spec.effective_hostnames.as_ref().cloned().unwrap_or_default()
            } else {
                // Legacy path: route not yet processed by the new controller.
                route.spec.hostnames.as_ref().cloned().unwrap_or_default()
            };

            if hostnames.is_empty() {
                if route.spec.resolved_listeners.is_some()
                    && route.spec.hostnames.as_ref().is_some_and(|hs| !hs.is_empty())
                {
                    continue;
                }
                catch_all.push(route.clone());
                continue;
            }

            for hostname in &hostnames {
                let lower = hostname.to_ascii_lowercase();
                buckets.entry(lower).or_default().push(route.clone());
            }
        }

        let mut host_map: HashHost<Vec<Arc<TLSRoute>>> = HashHost::new();
        for (hostname, mut route_vec) in buckets {
            if route_vec.len() > 1 {
                // Several TLSRoutes claim the same hostname on this listener. `match_route`
                // returns the first, so without a stable sort the chosen route would depend
                // on HashMap iteration order. Resolve by Gateway API precedence and warn.
                route_vec.sort_by(tls_route_precedence);
                warn_tls_route_collision(&hostname, &route_vec);
            }
            host_map.insert(&hostname, route_vec);
        }

        if catch_all.len() > 1 {
            catch_all.sort_by(tls_route_precedence);
            warn_tls_route_collision("(catch-all)", &catch_all);
        }

        Self {
            host_map,
            catch_all_routes: if catch_all.is_empty() { None } else { Some(catch_all) },
        }
    }

    /// Match a TLSRoute by SNI hostname.
    ///
    /// Priority: exact hostname > most-specific wildcard > catch-all.
    /// `HashHost` handles exact-vs-wildcard priority and most-specific-wildcard
    /// ordering internally.
    pub fn match_route(&self, sni_hostname: &str) -> Option<Arc<TLSRoute>> {
        // Case normalization is handled inside HashHost::get() — no need to
        // pre-lowercase here. Callers may pass raw SNI strings.
        if let Some(routes) = self.host_map.get(sni_hostname) {
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
        create_test_tls_route_ts(namespace, name, hostname, None)
    }

    fn create_test_tls_route_ts(
        namespace: &str,
        name: &str,
        hostname: &str,
        creation_timestamp: Option<k8s_openapi::apimachinery::pkg::apis::meta::v1::Time>,
    ) -> TLSRoute {
        TLSRoute {
            metadata: kube::api::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                creation_timestamp,
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
                resolved_listeners: None,
                effective_hostnames: None,
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

    #[test]
    fn test_same_hostname_collision_prefers_oldest_deterministically() {
        use chrono::{TimeZone, Utc};
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;

        let older = Some(Time(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()));
        let newer = Some(Time(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap()));

        // incumbent is lexically larger (zzz) but older; must win regardless of input order.
        let incumbent = Arc::new(create_test_tls_route_ts("team-z", "zzz", "shared.example.com", older));
        let newcomer = Arc::new(create_test_tls_route_ts("team-a", "aaa", "shared.example.com", newer));

        for (a, b) in [(incumbent.clone(), newcomer.clone()), (newcomer.clone(), incumbent.clone())] {
            let mut routes = HashMap::new();
            routes.insert(a.key_name(), a);
            routes.insert(b.key_name(), b);

            let table = TlsRouteTable::from_routes(&routes);
            let winner = table.match_route("shared.example.com").unwrap();
            assert_eq!(
                winner.metadata.name.as_deref(),
                Some("zzz"),
                "older TLSRoute must win regardless of name or input order"
            );
        }
    }
}
