use crate::routes::tls::route_unit::TlsRouteUnit;
use edgion_resources::matcher::HashHost;
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
    ///
    /// Entries are gateway-side [`TlsRouteUnit`]s. A route serving multiple SNI
    /// hostnames shares ONE `Arc<TlsRouteUnit>` across all its buckets so the
    /// per-route WRR cursor is not split per hostname.
    host_map: HashHost<Arc<TlsRouteUnit>>,

    /// Selected fallback route with no effective hostname restriction.
    catch_all_route: Option<Arc<TlsRouteUnit>>,
}

/// Deterministic precedence for TLSRoutes colliding on the same hostname.
///
/// Implements the Gateway API conflict-resolution rule (oldest `creationTimestamp`
/// wins, then namespace/name), matching `BackendTLSPolicyStore` precedence. The
/// winner is `routes.first()` after this sort, so equal-precedence overlapping
/// routes select deterministically instead of by HashMap iteration order.
fn tls_route_precedence(a: &Arc<TlsRouteUnit>, b: &Arc<TlsRouteUnit>) -> std::cmp::Ordering {
    a.route
        .metadata
        .creation_timestamp
        .cmp(&b.route.metadata.creation_timestamp)
        .then_with(|| a.route.metadata.namespace.cmp(&b.route.metadata.namespace))
        .then_with(|| a.route.metadata.name.cmp(&b.route.metadata.name))
}

/// Warn (route-rebuild path only — never `match_route`) when several TLSRoutes share
/// one hostname bucket. `routes` must already be sorted by [`tls_route_precedence`],
/// so `first()` is the selected route.
fn warn_tls_route_collision(hostname: &str, routes: &[Arc<TlsRouteUnit>]) {
    let fmt = |r: &Arc<TlsRouteUnit>| {
        format!(
            "{}/{}",
            r.route.metadata.namespace.as_deref().unwrap_or(""),
            r.route.metadata.name.as_deref().unwrap_or("")
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
            catch_all_route: None,
        }
    }

    /// Build a TlsRouteTable from a flat set of routes belonging to one port.
    ///
    /// When the controller has set `resolved_listeners`, we use
    /// `effective_hostnames` (the current route-global projection) for bucketing.
    /// Otherwise we use raw `spec.hostnames` for unprocessed inputs.
    /// A processed route with explicit hostnames but an empty intersection must
    /// not become catch-all.
    pub fn from_units(units: &HashMap<String, Arc<TlsRouteUnit>>) -> Self {
        let mut buckets: HashMap<String, Vec<Arc<TlsRouteUnit>>> = HashMap::new();
        let mut catch_all: Vec<Arc<TlsRouteUnit>> = Vec::new();

        for unit in units.values() {
            // The unit is built ONCE per route (in the conf-handler) and cached.
            // Here we only Arc-clone the SAME unit into every hostname bucket, so
            // the per-route WRR cursor is shared across SNI hostnames AND
            // preserved across unrelated route-table rebuilds.
            let route = &unit.route;

            let hostnames = if route.spec.resolved_listeners.is_some() {
                // Controller has processed this route; use its computed projection.
                route.spec.effective_hostnames.as_deref().unwrap_or_default()
            } else {
                route.spec.hostnames.as_deref().unwrap_or_default()
            };

            if hostnames.is_empty() {
                if route.spec.resolved_listeners.is_some()
                    && route.spec.hostnames.as_ref().is_some_and(|hs| !hs.is_empty())
                {
                    continue;
                }
                catch_all.push(unit.clone());
                continue;
            }

            for hostname in hostnames {
                let lower = hostname.to_ascii_lowercase();
                buckets.entry(lower).or_default().push(unit.clone());
            }
        }

        let mut host_map = HashHost::new();
        for (hostname, mut route_vec) in buckets {
            if route_vec.len() > 1 {
                // Several TLSRoutes claim the same hostname on this listener. We select
                // the first, so without a stable sort the chosen route would depend
                // on HashMap iteration order. Resolve by Gateway API precedence and warn.
                route_vec.sort_by(tls_route_precedence);
                warn_tls_route_collision(&hostname, &route_vec);
            }
            // Keep candidates only while resolving precedence and warning. The canonical
            // unit cache retains configured runners-up for the next listener rebuild.
            if let Some(winner) = route_vec.into_iter().next() {
                host_map.insert(&hostname, winner);
            }
        }

        if catch_all.len() > 1 {
            catch_all.sort_by(tls_route_precedence);
            warn_tls_route_collision("(catch-all)", &catch_all);
        }

        Self {
            host_map,
            catch_all_route: catch_all.into_iter().next(),
        }
    }

    /// Match a TLSRoute by SNI hostname.
    ///
    /// Priority: exact hostname > most-specific wildcard > catch-all.
    /// `HashHost` handles exact-vs-wildcard priority and most-specific-wildcard
    /// ordering internally.
    pub fn match_route(&self, sni_hostname: &str) -> Option<Arc<TlsRouteUnit>> {
        // Case normalization is handled inside HashHost::get() — no need to
        // pre-lowercase here. Callers may pass raw SNI strings.
        self.host_map
            .get(sni_hostname)
            .or(self.catch_all_route.as_ref())
            .cloned()
    }

    pub fn has_catch_all(&self) -> bool {
        self.catch_all_route.is_some()
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
    use edgion_resources::resources::common::ParentReference;
    use edgion_resources::resources::tls_route::*;

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

    fn unit(route: TLSRoute) -> Arc<TlsRouteUnit> {
        Arc::new(TlsRouteUnit::from_route(Arc::new(route)))
    }

    #[test]
    fn test_exact_match() {
        let route1 = unit(create_test_tls_route("default", "route1", "test.example.com"));
        let route2 = unit(create_test_tls_route("default", "route2", "api.example.com"));

        let mut units = HashMap::new();
        units.insert(route1.key_name(), route1);
        units.insert(route2.key_name(), route2);

        let table = TlsRouteTable::from_units(&units);

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
        let incumbent = unit(create_test_tls_route_ts("team-z", "zzz", "shared.example.com", older));
        let newcomer = unit(create_test_tls_route_ts("team-a", "aaa", "shared.example.com", newer));

        for (a, b) in [
            (incumbent.clone(), newcomer.clone()),
            (newcomer.clone(), incumbent.clone()),
        ] {
            let mut units = HashMap::new();
            units.insert(a.key_name(), a);
            units.insert(b.key_name(), b);

            let table = TlsRouteTable::from_units(&units);
            let winner = table.match_route("shared.example.com").unwrap();
            assert_eq!(
                winner.route.metadata.name.as_deref(),
                Some("zzz"),
                "older TLSRoute must win regardless of name or input order"
            );
        }
    }

    /// Per-route sharing invariant: one TLSRoute serving multiple SNI hostnames
    /// must share ONE `TlsRouteUnit` Arc across all its hostname buckets, so the
    /// per-route `backend_finder` WRR cursor is not split per SNI. Two lookups
    /// for different hostnames of the same route must return the same `Arc`.
    #[test]
    fn test_same_unit_shared_across_sni_hostnames() {
        let mut route = create_test_tls_route("default", "multi", "a.example.com");
        route.spec.hostnames = Some(vec!["a.example.com".to_string(), "b.example.com".to_string()]);
        let u = unit(route);

        let mut units = HashMap::new();
        units.insert(u.key_name(), u);
        let table = TlsRouteTable::from_units(&units);

        let a = table.match_route("a.example.com").unwrap();
        let b = table.match_route("b.example.com").unwrap();
        assert!(
            Arc::ptr_eq(&a, &b),
            "both SNI hostname buckets must reference the same unit Arc (shared WRR cursor)"
        );
        assert!(Arc::ptr_eq(&a.backend_finder, &b.backend_finder));
    }

    fn slot_route(namespace: &str, name: &str, hostname: Option<&str>) -> TLSRoute {
        let mut route = create_test_tls_route(namespace, name, "unused.example.com");
        route.spec.hostnames = hostname.map(|host| vec![host.to_string()]);
        route
    }

    #[test]
    fn test_exact_longest_wildcard_and_catch_all_priority() {
        let units = [
            ("exact", Some("API.DEEP.EXAMPLE.COM")),
            ("specific", Some("*.deep.example.com")),
            ("broad", Some("*.example.com")),
            ("fallback", None),
        ]
        .into_iter()
        .map(|(name, host)| {
            let unit = unit(slot_route("default", name, host));
            (unit.key_name(), unit)
        })
        .collect();
        let table = TlsRouteTable::from_units(&units);
        for (sni, expected) in [
            ("api.deep.example.com", "exact"),
            ("A.B.DEEP.EXAMPLE.COM", "specific"),
            ("deep.example.com", "broad"),
            ("a.b.example.com", "broad"),
            ("example.com", "fallback"),
            ("unrelated.test", "fallback"),
        ] {
            assert_eq!(
                table.match_route(sni).unwrap().route.metadata.name.as_deref(),
                Some(expected)
            );
        }
        assert!(table.has_catch_all());
    }

    #[test]
    fn test_collision_precedence_for_every_slot_kind() {
        use chrono::{TimeZone, Utc};
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;

        let older = Some(Time(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()));
        let newer = Some(Time(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap()));
        for host in [Some("shared.example.com"), Some("*.example.com"), None] {
            for (winner_ns, winner_name, winner_ts, loser_ns, loser_name, loser_ts) in [
                ("z", "z", older.clone(), "a", "a", newer.clone()),
                ("z", "z", None, "a", "a", older.clone()),
                ("a", "z", None, "b", "a", None),
                ("a", "a", older.clone(), "a", "b", older.clone()),
            ] {
                let mut winner = slot_route(winner_ns, winner_name, host);
                winner.metadata.creation_timestamp = winner_ts;
                let winner = unit(winner);
                let mut loser = slot_route(loser_ns, loser_name, host);
                loser.metadata.creation_timestamp = loser_ts;
                let loser = unit(loser);
                for ordered in [[winner.clone(), loser.clone()], [loser.clone(), winner.clone()]] {
                    let units = ordered.into_iter().map(|unit| (unit.key_name(), unit)).collect();
                    let table = TlsRouteTable::from_units(&units);
                    assert!(Arc::ptr_eq(&table.match_route("shared.example.com").unwrap(), &winner));
                }
            }
        }
    }

    #[test]
    fn test_raw_and_effective_hostname_inputs() {
        // This checks consumption of the existing route-global projection, not
        // attachment-specific correctness (tracked separately).
        for processed in [false, true] {
            for raw in [None, Some(vec![]), Some(vec!["raw.example.com".to_string()])] {
                for effective in [None, Some(vec![]), Some(vec!["effective.example.com".to_string()])] {
                    let mut route = slot_route("default", "route", None);
                    route.spec.hostnames = raw.clone();
                    route.spec.effective_hostnames = effective.clone();
                    route.spec.resolved_listeners = processed.then(Vec::new);
                    let unit = unit(route);
                    let units = HashMap::from([(unit.key_name(), unit)]);
                    let table = TlsRouteTable::from_units(&units);
                    let has_raw = raw.as_ref().is_some_and(|hosts| !hosts.is_empty());
                    let has_effective = effective.as_ref().is_some_and(|hosts| !hosts.is_empty());
                    let catch_all = !has_raw && (!processed || !has_effective);
                    assert_eq!(table.has_catch_all(), catch_all);
                    assert_eq!(table.match_route("unrelated.test").is_some(), catch_all);
                    assert_eq!(
                        table.match_route("raw.example.com").is_some(),
                        catch_all || (!processed && has_raw)
                    );
                    assert_eq!(
                        table.match_route("effective.example.com").is_some(),
                        catch_all || (processed && has_effective)
                    );
                }
            }
        }
    }

    #[test]
    fn test_published_slots_do_not_retain_losers() {
        for host in [Some("shared.example.com"), Some("*.example.com"), None] {
            let winner = unit(slot_route("default", "a", host));
            let loser = unit(slot_route("default", "z", host));
            let winner_weak = Arc::downgrade(&winner);
            let loser_weak = Arc::downgrade(&loser);
            let units = HashMap::from([(winner.key_name(), winner), (loser.key_name(), loser)]);
            let table = TlsRouteTable::from_units(&units);
            // Configured losers remain owned by the canonical cache in production.
            // Remove the input owners here to isolate the published table's ownership.
            drop(units);
            assert!(loser_weak.upgrade().is_none());
            assert!(Arc::ptr_eq(
                &table.match_route("shared.example.com").unwrap(),
                &winner_weak.upgrade().unwrap()
            ));
            drop(table);
            assert!(winner_weak.upgrade().is_none());
        }
    }

    #[test]
    fn test_rebuild_promotes_runner_up_without_mutating_old_snapshot() {
        use crate::routes::tls::routes_mgr::TlsRouteManager;

        for host in [Some("shared.example.com"), Some("*.example.com"), None] {
            let manager = TlsRouteManager::new();
            let winner = unit(slot_route("default", "a", host));
            let loser = unit(slot_route("default", "z", host));
            let winner_weak = Arc::downgrade(&winner);
            let loser_weak = Arc::downgrade(&loser);
            let winner_key = winner.key_name();
            let mut units = HashMap::from([(winner_key.clone(), winner), (loser.key_name(), loser)]);
            manager.rebuild(&units);
            let old = Arc::clone(&manager.load_route_table());
            units.remove(&winner_key);
            assert_eq!(
                manager
                    .load_route_table()
                    .match_route("shared.example.com")
                    .unwrap()
                    .key_name(),
                "default/a"
            );
            manager.rebuild(&units);
            let new = Arc::clone(&manager.load_route_table());
            assert_eq!(old.match_route("shared.example.com").unwrap().key_name(), "default/a");
            assert!(Arc::ptr_eq(
                &new.match_route("shared.example.com").unwrap(),
                &loser_weak.upgrade().unwrap()
            ));
            units.clear();
            manager.rebuild(&units);
            assert!(manager.load_route_table().match_route("shared.example.com").is_none());
            assert!(winner_weak.upgrade().is_some());
            assert!(loser_weak.upgrade().is_some());
            drop(old);
            assert!(winner_weak.upgrade().is_none());
            assert!(loser_weak.upgrade().is_some());
            drop(new);
            assert!(loser_weak.upgrade().is_none());
        }
    }
}
