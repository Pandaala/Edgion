use crate::routes::tls::route_unit::TlsRouteUnit;
use edgion_resources::matcher::HashHost;
use edgion_resources::ListenerKey;
use std::collections::HashMap;
use std::sync::Arc;

/// Per-port TLS route table — immutable snapshot shared via ArcSwap.
///
/// Routes are indexed by their accepted attachments' hostnames on this physical
/// listener. Exact logical attachment identities remain on the compiled unit
/// for admission against the current Gateway publication.
/// A new immutable snapshot is published for each affected listener on updates.
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
        }
    }

    /// Build from accepted attachments on this physical listener only.
    /// Empty intersections and unresolved routes never become catch-all.
    pub fn from_units(listener_key: &ListenerKey, units: &HashMap<String, Arc<TlsRouteUnit>>) -> Self {
        let mut buckets: HashMap<String, Vec<Arc<TlsRouteUnit>>> = HashMap::new();

        for unit in units.values() {
            // The unit is built ONCE per route (in the conf-handler) and cached.
            // Here we only Arc-clone the SAME unit into every hostname bucket, so
            // the per-route WRR cursor is shared across SNI hostnames AND
            // preserved across unrelated route-table rebuilds.
            let route = &unit.route;

            let attachments = route.spec.resolved_listeners.as_deref().unwrap_or_default();
            for attachment in unit.accepted_attachments.iter().map(|i| &attachments[*i]) {
                if &attachment.listener_key != listener_key {
                    continue;
                }
                for hostname in &attachment.hostnames {
                    let candidates = buckets.entry(hostname.to_ascii_lowercase()).or_default();
                    // Duplicate parentRefs or overlapping attachments must not create
                    // a collision with the same route or split its selector state.
                    if !candidates.iter().any(|candidate| Arc::ptr_eq(candidate, unit)) {
                        candidates.push(unit.clone());
                    }
                }
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

        Self { host_map }
    }

    /// Match a TLSRoute by SNI hostname.
    ///
    /// Priority: exact hostname > most-specific wildcard.
    /// `HashHost` handles exact-vs-wildcard priority and most-specific-wildcard
    /// ordering internally.
    pub fn match_route(&self, sni_hostname: &str) -> Option<Arc<TlsRouteUnit>> {
        // Case normalization is handled inside HashHost::get() — no need to
        // pre-lowercase here. Callers may pass raw SNI strings.
        self.host_map.get(sni_hostname).cloned()
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
            },
            status: None,
        }
    }

    fn key() -> ListenerKey {
        ListenerKey::new("0.0.0.0:443".parse().unwrap(), "TLS")
    }

    fn unit(mut route: TLSRoute) -> Arc<TlsRouteUnit> {
        route.spec.resolved_listeners = Some(vec![ResolvedTlsListener {
            listener_key: key(),
            gateway_namespace: route.metadata.namespace.clone().unwrap(),
            gateway_name: "test-gateway".into(),
            listener_name: "tls".into(),
            hostnames: route.spec.hostnames.clone().unwrap_or_default(),
        }]);
        Arc::new(TlsRouteUnit::from_route(Arc::new(route)))
    }

    #[test]
    fn test_exact_match() {
        let route1 = unit(create_test_tls_route("default", "route1", "test.example.com"));
        let route2 = unit(create_test_tls_route("default", "route2", "api.example.com"));

        let mut units = HashMap::new();
        units.insert(route1.key_name(), route1);
        units.insert(route2.key_name(), route2);

        let table = TlsRouteTable::from_units(&key(), &units);

        assert!(table.match_route("test.example.com").is_some());
        assert!(table.match_route("api.example.com").is_some());
        assert!(table.match_route("other.example.com").is_none());
    }

    #[test]
    fn test_empty_table() {
        let table = TlsRouteTable::new();
        assert!(table.match_route("test.example.com").is_none());
    }

    #[test]
    fn attachment_hostnames_cannot_shadow_another_ports_valid_route() {
        let other = ListenerKey::new("0.0.0.0:9443".parse().unwrap(), "TLS");
        let mut route = create_test_tls_route("default", "a-multi", "a.example.com");
        route.spec.hostnames = Some(vec!["a.example.com".into(), "b.example.com".into()]);
        route.spec.parent_refs = Some(
            serde_json::from_value(serde_json::json!([
                {"name": "a", "port": 443}, {"name": "b", "port": 9443}
            ]))
            .unwrap(),
        );
        route.spec.resolved_listeners = Some(vec![
            ResolvedTlsListener {
                listener_key: key(),
                gateway_namespace: "default".into(),
                gateway_name: "a".into(),
                listener_name: "a-tls".into(),
                hostnames: vec!["a.example.com".into()],
            },
            ResolvedTlsListener {
                listener_key: other.clone(),
                gateway_namespace: "default".into(),
                gateway_name: "b".into(),
                listener_name: "b-tls".into(),
                hostnames: vec!["b.example.com".into()],
            },
        ]);
        let multi = Arc::new(TlsRouteUnit::from_route(Arc::new(route.clone())));
        let control = unit(create_test_tls_route("default", "z-control", "b.example.com"));
        let units = HashMap::from([(multi.key_name(), multi.clone()), (control.key_name(), control.clone())]);
        let first = TlsRouteTable::from_units(&key(), &units);
        let second = TlsRouteTable::from_units(&other, &units);
        assert!(Arc::ptr_eq(&first.match_route("a.example.com").unwrap(), &multi));
        assert!(Arc::ptr_eq(&first.match_route("b.example.com").unwrap(), &control));
        assert!(Arc::ptr_eq(&second.match_route("b.example.com").unwrap(), &multi));
        assert!(second.match_route("a.example.com").is_none());
        assert_eq!(multi.resolved_parent_refs[&key()].len(), 1);
        assert_eq!(
            multi.resolved_parent_refs[&key()][0].section_name.as_deref(),
            Some("a-tls")
        );
        assert_eq!(multi.resolved_parent_refs[&other][0].gateway_key, "default/b");

        // Rejecting one parent removes only its attachment from candidate selection.
        route.status = Some(
            serde_json::from_value(serde_json::json!({"parents": [{
                "parentRef": {"name": "a", "port": 443}, "controllerName": "edgion.io/gateway-controller",
                "conditions": [{"type": "Accepted", "status": "False", "reason": "NotAllowedByListeners",
                    "message": "denied", "lastTransitionTime": "2026-09-13T00:00:00Z"}]
            }]}))
            .unwrap(),
        );
        let rejected = Arc::new(TlsRouteUnit::from_route(Arc::new(route)));
        let units = HashMap::from([
            (rejected.key_name(), rejected.clone()),
            (control.key_name(), control.clone()),
        ]);
        assert!(TlsRouteTable::from_units(&key(), &units)
            .match_route("a.example.com")
            .is_none());
        assert!(TlsRouteTable::from_units(&other, &units)
            .match_route("b.example.com")
            .is_some());
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

            let table = TlsRouteTable::from_units(&key(), &units);
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
        let table = TlsRouteTable::from_units(&key(), &units);

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
    fn test_exact_longest_wildcard_and_invalid_catch_all() {
        let units = [
            ("exact", Some("api.deep.example.com")),
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
        let table = TlsRouteTable::from_units(&key(), &units);
        for (sni, expected) in [
            ("api.deep.example.com", "exact"),
            ("A.B.DEEP.EXAMPLE.COM", "specific"),
            ("deep.example.com", "broad"),
            ("a.b.example.com", "broad"),
        ] {
            assert_eq!(
                table.match_route(sni).unwrap().route.metadata.name.as_deref(),
                Some(expected)
            );
        }
        assert!(table.match_route("example.com").is_none());
        assert!(table.match_route("unrelated.test").is_none());
    }

    #[test]
    fn test_collision_precedence_for_every_slot_kind() {
        use chrono::{TimeZone, Utc};
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;

        let older = Some(Time(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()));
        let newer = Some(Time(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap()));
        for host in [Some("shared.example.com"), Some("*.example.com")] {
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
                    let table = TlsRouteTable::from_units(&key(), &units);
                    assert!(Arc::ptr_eq(&table.match_route("shared.example.com").unwrap(), &winner));
                }
            }
        }
    }

    #[test]
    fn unresolved_or_empty_routes_never_become_catch_all() {
        for hosts in [
            None,
            Some(vec![]),
            Some(vec!["*".into()]),
            Some(vec!["raw.example.com".into()]),
        ] {
            let mut route = create_test_tls_route("default", "route", "raw.example.com");
            route.spec.hostnames = hosts;
            let unit = Arc::new(TlsRouteUnit::from_route(Arc::new(route)));
            let table = TlsRouteTable::from_units(&key(), &HashMap::from([(unit.key_name(), unit)]));
            assert!(table.match_route("raw.example.com").is_none());
            assert!(table.match_route("unrelated.test").is_none());
        }
    }

    #[test]
    fn test_published_slots_do_not_retain_losers() {
        for host in [Some("shared.example.com"), Some("*.example.com")] {
            let winner = unit(slot_route("default", "a", host));
            let loser = unit(slot_route("default", "z", host));
            let winner_weak = Arc::downgrade(&winner);
            let loser_weak = Arc::downgrade(&loser);
            let units = HashMap::from([(winner.key_name(), winner), (loser.key_name(), loser)]);
            let table = TlsRouteTable::from_units(&key(), &units);
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

        for host in [Some("shared.example.com"), Some("*.example.com")] {
            let manager = TlsRouteManager::new(key());
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
