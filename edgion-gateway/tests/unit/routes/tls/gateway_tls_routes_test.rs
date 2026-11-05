use super::*;

#[test]
fn hostname_build_failure_clears_all_tls_candidates() {
    let result: Result<RadixHostnameMatcher<Vec<TlsRouteCandidate>>, &str> = Err("injected capacity failure");
    let table = TlsRouteTable::from_hostname_result(result, 1, 1);
    assert!(table.hosts.is_empty());
}
use crate::runtime::store::listener_gateway_info::ListenerEntry;
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
    crate::test_current_status::processed(TLSRoute {
        metadata: kube::api::ObjectMeta {
            namespace: Some(namespace.to_string()),
            name: Some(name.to_string()),
            creation_timestamp,
            ..Default::default()
        },
        spec: TLSRouteSpec {
            current_status: None,
            resolved_status_controller: Some("edgion.io/gateway-controller".into()),
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
    })
}

fn key() -> ListenerKey {
    ListenerKey::new("0.0.0.0:443".parse().unwrap(), "TLS")
}

fn scoped_unit(name: &str, hostname: &str, attachments: &[(&str, &str, &str)]) -> Arc<TlsRouteUnit> {
    let parents: Vec<_> = attachments
        .iter()
        .map(|(gateway, listener, _)| {
            serde_json::json!({
                "name": gateway, "sectionName": listener,
            })
        })
        .collect();
    let resolved: Vec<_> = attachments
        .iter()
        .map(|(gateway, listener, hostname)| {
            serde_json::json!({
                "listenerKey": "0.0.0.0:443/TLS", "gatewayNamespace": "default",
                "gatewayName": gateway, "listenerName": listener, "hostnames": [hostname],
            })
        })
        .collect();
    let route = crate::test_current_status::from_value(serde_json::json!({
            "metadata": {"namespace": "default", "name": name},
            "spec": {"hostnames": [hostname], "resolvedStatusController": "edgion.io/gateway-controller", "parentRefs": parents, "resolvedListeners": resolved}
        }))
        .unwrap();
    Arc::new(TlsRouteUnit::from_route(Arc::new(route)).unwrap())
}

fn owner_index(entries: &[(&str, &str, Option<&str>)]) -> GatewayInfoIndex {
    GatewayInfoIndex::from_entries_for_test(
        entries
            .iter()
            .map(|(gateway, listener, hostname)| {
                (
                    format!("default/{gateway}"),
                    ListenerEntry {
                        port: 443,
                        info: Arc::new(GatewayInfo::new(
                            Some("default".into()),
                            (*gateway).into(),
                            Some((*listener).into()),
                            None,
                            None,
                            None,
                            Vec::new(),
                            None,
                            None,
                        )),
                        gateway_proof: None,
                        hostname: hostname.map(str::to_owned),
                        allowed_routes: None,
                    },
                )
            })
            .collect(),
    )
}

#[test]
fn ineligible_oldest_route_does_not_hide_the_exact_listeners_candidate() {
    let old = scoped_unit("a-old", "api.example.com", &[("gw", "wild", "api.example.com")]);
    let valid = scoped_unit("b-valid", "api.example.com", &[("gw", "exact", "api.example.com")]);
    let table = TlsRouteTable::from_units(
        &key(),
        &HashMap::from([(old.key_name(), old), (valid.key_name(), valid.clone())]),
    );
    let index = owner_index(&[
        ("gw", "wild", Some("*.example.com")),
        ("gw", "exact", Some("api.example.com")),
    ]);
    let (selected, owner) = table.select_route("api.example.com", &index).unwrap();
    assert!(Arc::ptr_eq(&selected, &valid));
    assert_eq!(owner.listener_name.as_deref(), Some("exact"));
}

#[test]
fn attachment_hostname_and_logical_owner_cannot_be_combined_across_records() {
    let route = scoped_unit(
        "multi",
        "*.example.com",
        &[("a", "a", "a.example.com"), ("b", "b", "b.example.com")],
    );
    let table = TlsRouteTable::from_units(&key(), &HashMap::from([(route.key_name(), route)]));
    // Only B is currently admitted, and its Listener accepts arbitrary names.
    // A's effective hostname must not borrow that unrelated owner.
    let index = owner_index(&[("b", "b", None)]);
    assert!(table.select_route("a.example.com", &index).is_none());
    assert_eq!(
        table.select_route("b.example.com", &index).unwrap().1.gateway_name(),
        "b"
    );
}

#[test]
fn cross_gateway_empty_exact_listener_blocks_route_continuation() {
    let route = scoped_unit("broad", "*.example.com", &[("broad", "tls", "*.example.com")]);
    let table = TlsRouteTable::from_units(&key(), &HashMap::from([(route.key_name(), route)]));
    let index = owner_index(&[
        ("broad", "tls", Some("*.example.com")),
        ("exact", "tls", Some("api.example.com")),
    ]);
    assert!(table.select_route("api.example.com", &index).is_none());
    assert!(table.select_route("other.example.com", &index).is_some());
}

#[test]
fn route_updates_share_topology_and_unchanged_candidate_groups() {
    let a = scoped_unit("a", "a.test", &[("gw", "tls", "a.test")]);
    let b = scoped_unit("b", "b.test", &[("gw", "tls", "b.test")]);
    let old = TlsRouteTable::from_units(&key(), &HashMap::from([(a.key_name(), a), (b.key_name(), b.clone())]));
    let replacement = scoped_unit("a", "a.test", &[("gw", "tls", "a.test")]);
    let new = old.with_units(
        &key(),
        &HashMap::from([(replacement.key_name(), replacement.clone()), (b.key_name(), b.clone())]),
    );
    assert!(new.hosts.shares_hosts(&old.hosts));
    let b_pattern = RadixHostnamePattern::parse("b.test").unwrap();
    assert!(Arc::ptr_eq(
        old.hosts.get(&b_pattern).unwrap(),
        new.hosts.get(&b_pattern).unwrap()
    ));
    assert!(!Arc::ptr_eq(&old.match_route("a.test").unwrap(), &replacement));
    assert!(Arc::ptr_eq(&new.match_route("a.test").unwrap(), &replacement));
    let renamed = scoped_unit("a", "0.test", &[("gw", "tls", "0.test")]);
    let topology = new.with_units(
        &key(),
        &HashMap::from([(renamed.key_name(), renamed), (b.key_name(), b.clone())]),
    );
    assert!(!topology.hosts.shares_hosts(&new.hosts));
    assert!(topology.match_route("a.test").is_none());
    assert!(Arc::ptr_eq(&topology.match_route("b.test").unwrap(), &b));
    assert!(new.match_route("a.test").is_some());
}

fn unit(mut route: TLSRoute) -> Arc<TlsRouteUnit> {
    route.spec.resolved_listeners = Some(vec![ResolvedTlsListener {
        listener_key: key(),
        gateway_namespace: route.metadata.namespace.clone().unwrap(),
        gateway_name: "test-gateway".into(),
        listener_name: "tls".into(),
        hostnames: route.spec.hostnames.clone().unwrap_or_default(),
    }]);
    Arc::new(TlsRouteUnit::from_route(Arc::new(route)).unwrap())
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
    assert_eq!(table.candidate_entry_count(), 0);
}

#[test]
fn candidate_entry_count_preserves_hostname_group_multiplicity() {
    let mut multi = create_test_tls_route("default", "multi", "a.example.com");
    multi.spec.hostnames = Some(vec!["a.example.com".into(), "b.example.com".into()]);
    let multi = unit(multi);
    let competitor = unit(create_test_tls_route("default", "competitor", "a.example.com"));
    let table = TlsRouteTable::from_units(
        &key(),
        &HashMap::from([(multi.key_name(), multi), (competitor.key_name(), competitor)]),
    );

    let mut retained_by_hostname: Vec<_> = table
        .hosts
        .groups()
        .map(|(pattern, candidates)| (pattern.as_str(), candidates.len()))
        .collect();
    retained_by_hostname.sort_unstable();
    assert_eq!(retained_by_hostname, [("a.example.com", 2), ("b.example.com", 1)]);
    assert_eq!(table.candidate_entry_count(), 3);
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
    route.spec.current_status = None;
    route = crate::test_current_status::processed(route);
    let multi = Arc::new(TlsRouteUnit::from_route(Arc::new(route.clone())).unwrap());
    let control = unit(create_test_tls_route("default", "z-control", "b.example.com"));
    let units = HashMap::from([(multi.key_name(), multi.clone()), (control.key_name(), control.clone())]);
    let first = TlsRouteTable::from_units(&key(), &units);
    let second = TlsRouteTable::from_units(&other, &units);
    assert!(Arc::ptr_eq(&first.match_route("a.example.com").unwrap(), &multi));
    assert!(Arc::ptr_eq(&first.match_route("b.example.com").unwrap(), &control));
    assert!(Arc::ptr_eq(&second.match_route("b.example.com").unwrap(), &multi));
    assert!(second.match_route("a.example.com").is_none());
    assert_eq!(multi.accepted_attachments().len(), 2);
    assert_eq!(
        multi.accepted_attachments()[0].parent.section_name.as_deref(),
        Some("a-tls")
    );
    assert_eq!(multi.accepted_attachments()[1].parent.gateway_key, "default/b");

    // Rejecting one parent removes only its attachment from candidate selection.
    route.spec.current_status.as_mut().unwrap().parents[0].conditions[0].status = "False".into();
    let rejected = Arc::new(TlsRouteUnit::from_route(Arc::new(route)).unwrap());
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
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;

    let older = Some(Time("2020-01-01T00:00:00Z".parse().unwrap()));
    let newer = Some(Time("2024-01-01T00:00:00Z".parse().unwrap()));

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
            winner.route().metadata.name.as_deref(),
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
    assert!(Arc::ptr_eq(a.backend_generation(), b.backend_generation()));
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
            table.match_route(sni).unwrap().route().metadata.name.as_deref(),
            Some(expected)
        );
    }
    assert!(table.match_route("example.com").is_none());
    assert!(table.match_route("unrelated.test").is_none());
}

#[test]
fn test_collision_precedence_for_every_slot_kind() {
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;

    let older = Some(Time("2020-01-01T00:00:00Z".parse().unwrap()));
    let newer = Some(Time("2024-01-01T00:00:00Z".parse().unwrap()));
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
        let unit = Arc::new(TlsRouteUnit::from_route(Arc::new(route)).unwrap());
        let table = TlsRouteTable::from_units(&key(), &HashMap::from([(unit.key_name(), unit)]));
        assert!(table.match_route("raw.example.com").is_none());
        assert!(table.match_route("unrelated.test").is_none());
    }
}

#[test]
fn published_candidates_retain_runners_up_until_the_snapshot_is_released() {
    for host in [Some("shared.example.com"), Some("*.example.com")] {
        let winner = unit(slot_route("default", "a", host));
        let loser = unit(slot_route("default", "z", host));
        let winner_weak = Arc::downgrade(&winner);
        let loser_weak = Arc::downgrade(&loser);
        let units = HashMap::from([(winner.key_name(), winner), (loser.key_name(), loser)]);
        let table = TlsRouteTable::from_units(&key(), &units);
        // Runtime eligibility can choose a runner-up, so the table owns all
        // candidates. They must all be released with the final snapshot.
        drop(units);
        assert!(loser_weak.upgrade().is_some());
        assert!(Arc::ptr_eq(
            &table.match_route("shared.example.com").unwrap(),
            &winner_weak.upgrade().unwrap()
        ));
        drop(table);
        assert!(winner_weak.upgrade().is_none());
        assert!(loser_weak.upgrade().is_none());
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
