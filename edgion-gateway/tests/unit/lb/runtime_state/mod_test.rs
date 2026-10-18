use super::*;
use crate::backends::discovery::ServiceDiscoveryCoordinator;
use crate::routes::http::session_persistence::{generate_scoped_session_token, generate_session_token};
use edgion_resources::constants::labels::k8s::SERVICE_NAME;
use k8s_openapi::api::core::v1::Service;
use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions, EndpointPort, EndpointSlice};
use kube::api::ObjectMeta;
use pingora_load_balancing::Backend;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr as StdSocketAddr;
use std::num::NonZeroU16;

fn test_section() -> ServiceSectionKey {
    ServiceSectionKey::Number(NonZeroU16::new(8080).unwrap())
}

fn backend(addr: StdSocketAddr) -> Arc<LbBackend> {
    Arc::new(LbBackend::new(Backend {
        addr: SocketAddr::Inet(addr),
        weight: 1,
        ext: Default::default(),
    }))
}

/// Round-trip test: a session token derived from the backend cell address via
/// `SocketAddr::to_string()` must resolve back through the affinity map for
/// both IPv4 and IPv6 backends. Mis-formatted forms must NOT resolve.
#[test]
fn sp_affinity_round_trip_ipv4_and_ipv6() {
    let v4: StdSocketAddr = "10.0.0.1:8080".parse().unwrap();
    let v6: StdSocketAddr = "[::1]:30001".parse().unwrap();
    let svc = "ns/sp-round-trip";
    let backends = Arc::new(vec![backend(v4), backend(v6)]);
    let map = build_sp_affinity_map(backends.iter().map(Arc::as_ref), None);
    let lookup = |token: &str| map.get(token).copied();

    // Token format: backend cell address formatted with `SocketAddr::to_string()`.
    let v4_token = generate_session_token(&v4.to_string());
    let v6_token = generate_session_token(&v6.to_string());
    assert_eq!(lookup(&v4_token), Some(v4));
    assert_eq!(lookup(&v6_token), Some(v6));

    // Unbracketed IPv6 form (`::1:30001`, from `format!("{}:{}", ip, port)`)
    // must NOT resolve — it is not a valid `SocketAddr::to_string()` output.
    let v6_unbracketed = generate_session_token(&format!("{}:{}", v6.ip(), v6.port()));
    assert_eq!(lookup(&v6_unbracketed), None);

    // Token built from a rewritten peer port must NOT resolve: session tokens
    // key off the cell address, not the post-rewritten peer address.
    let v6_rewritten_port = generate_session_token("[::1]:8080");
    assert_eq!(lookup(&v6_rewritten_port), None);

    remove_service(svc);
}

#[test]
fn scoped_affinity_views_are_isolated_and_never_registered_globally() {
    let addr: StdSocketAddr = "10.0.0.8:8080".parse().unwrap();
    let svc = "ns/scoped-affinity";
    let backends: Arc<[Arc<LbBackend>]> = Arc::from(vec![backend(addr)]);
    let first_scope = "edgion.io/EdgionBackend/ns/first\0core/Service/ns/scoped-affinity";
    let second_scope = "edgion.io/EdgionBackend/ns/second\0core/Service/ns/scoped-affinity";
    let first = sp_affinity_get_or_build_scoped(svc, &test_section(), Some(first_scope), || backends.clone());
    let second = sp_affinity_get_or_build_scoped(svc, &test_section(), Some(second_scope), || backends.clone());

    let first_token = generate_scoped_session_token(Some(first_scope), &addr.to_string());
    let second_token = generate_scoped_session_token(Some(second_scope), &addr.to_string());
    assert_eq!(first.get(&first_token), Some(&addr));
    assert!(!first.contains_key(&second_token));
    assert_eq!(second.get(&second_token), Some(&addr));

    let rebuilt = sp_affinity_get_or_build_scoped(svc, &test_section(), Some(first_scope), || backends.clone());
    assert!(!Arc::ptr_eq(&first, &rebuilt));
    remove_service(svc);
}

#[test]
fn scoped_affinity_view_has_the_canonical_endpoint_cap() {
    let svc = "ns/scoped-bounded";
    let scope = "edgion.io/EdgionBackend/ns/agent/uid\0core/Service/ns/scoped-bounded";
    let backends = Arc::from(
        (0..=SP_AFFINITY_MAX_PER_SERVICE)
            .map(|index| {
                let third = ((index / 254) % 254 + 1) as u8;
                let fourth = (index % 254 + 1) as u8;
                backend(StdSocketAddr::from(([10, 1, third, fourth], 8080)))
            })
            .collect::<Vec<_>>(),
    );
    let view = sp_affinity_get_or_build_scoped(svc, &test_section(), Some(scope), || backends);
    assert_eq!(view.len(), SP_AFFINITY_MAX_PER_SERVICE);
}

fn publish_affinity_service(coordinator: &ServiceDiscoveryCoordinator, address: &str, uid: &str) {
    coordinator.replace_services(&HashMap::from([(
        "default/affinity".to_string(),
        Arc::new(Service {
            metadata: ObjectMeta {
                namespace: Some("default".to_string()),
                name: Some("affinity".to_string()),
                uid: Some("service-uid".to_string()),
                ..Default::default()
            },
            spec: Some(k8s_openapi::api::core::v1::ServiceSpec {
                ports: Some(vec![k8s_openapi::api::core::v1::ServicePort {
                    port: 8080,
                    protocol: Some("TCP".to_string()),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }),
    )]));
    coordinator.update_endpoint_slices(
        HashMap::new(),
        HashMap::from([(
            "default/affinity-slice".to_string(),
            Arc::new(EndpointSlice {
                metadata: ObjectMeta {
                    namespace: Some("default".to_string()),
                    name: Some("affinity-slice".to_string()),
                    uid: Some(uid.to_string()),
                    labels: Some(BTreeMap::from([(SERVICE_NAME.to_string(), "affinity".to_string())])),
                    ..Default::default()
                },
                address_type: "IPv4".to_string(),
                endpoints: vec![Endpoint {
                    addresses: vec![address.to_string()],
                    conditions: Some(EndpointConditions {
                        ready: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                ports: Some(vec![EndpointPort {
                    port: Some(8080),
                    ..Default::default()
                }]),
            }),
        )]),
        HashSet::new(),
    );
}

#[test]
fn stale_sp_builder_cannot_overwrite_new_membership_and_repair_rebuilds() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    publish_affinity_service(&coordinator, "10.4.0.1", "slice-a");
    let old_generation = coordinator.load_generation();
    let old_available = Arc::clone(old_generation.available("default/affinity").unwrap());
    let cell = coordinator.runtime_cell("default/affinity").unwrap();
    let old_active = cell.active.load_full();

    publish_affinity_service(&coordinator, "10.4.0.2", "slice-b");
    assert!(cell.sp.load_full().is_empty());
    assert!(try_publish_sp_snapshot(
        &coordinator,
        "default/affinity",
        &test_section(),
        &old_generation,
        &old_active,
        &old_available,
        &cell,
    )
    .is_none());
    assert!(cell.sp.load_full().is_empty());

    let repaired = sp_affinity_get_or_build_from(&coordinator, "default/affinity", &test_section());
    let new_addr: StdSocketAddr = "10.4.0.2:8080".parse().unwrap();
    let old_addr: StdSocketAddr = "10.4.0.1:8080".parse().unwrap();
    assert_eq!(
        repaired.get(&generate_session_token(&new_addr.to_string())),
        Some(&new_addr)
    );
    assert!(!repaired.contains_key(&generate_session_token(&old_addr.to_string())));
    let snapshot = cell
        .sp
        .load_full()
        .get(&test_section())
        .cloned()
        .expect("new membership cache published");
    assert_eq!(
        snapshot.membership_identity,
        cell.active.load().membership_identity.unwrap()
    );
}

#[test]
fn draining_removal_is_exact_across_same_socket_delete_recreate_aba() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    let service_key = "default/affinity";
    remove_service(service_key);
    publish_affinity_service(&coordinator, "10.4.0.9", "slice-first");
    let first_generation = coordinator.load_generation();
    let first = Arc::clone(
        &first_generation
            .available(service_key)
            .unwrap()
            .canonical_probe_backends()[0],
    );
    first.inc_conn();

    publish_affinity_service(&coordinator, "10.4.0.9", "slice-recreated");
    let second_generation = coordinator.load_generation();
    let second = Arc::clone(
        &second_generation
            .available(service_key)
            .unwrap()
            .canonical_probe_backends()[0],
    );
    assert_ne!(first.runtime_identity(), second.runtime_identity());
    second.inc_conn();
    coordinator.update_services(HashMap::new(), HashMap::new(), HashSet::from([service_key.to_string()]));

    let matching = all_draining_backends()
        .into_iter()
        .filter(|(_, backend)| {
            backend
                .runtime_identity()
                .is_some_and(|identity| identity.service_key.as_ref() == service_key)
        })
        .collect::<Vec<_>>();
    assert_eq!(matching.len(), 2);
    let addr = first.inner().addr.clone();
    remove_backend_exact(service_key, &addr, &first);
    let remaining = all_draining_backends()
        .into_iter()
        .filter(|(_, backend)| {
            backend
                .runtime_identity()
                .is_some_and(|identity| identity.service_key.as_ref() == service_key)
        })
        .collect::<Vec<_>>();
    assert_eq!(remaining.len(), 1);
    assert!(Arc::ptr_eq(&remaining[0].1, &second));
    remove_backend_exact(service_key, &addr, &second);
}

#[test]
fn sticky_token_from_another_section_is_a_miss() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    coordinator.replace_services(&HashMap::from([(
        "default/affinity".to_string(),
        Arc::new(Service {
            metadata: ObjectMeta {
                namespace: Some("default".to_string()),
                name: Some("affinity".to_string()),
                uid: Some("service-uid".to_string()),
                ..Default::default()
            },
            spec: Some(k8s_openapi::api::core::v1::ServiceSpec {
                ports: Some(vec![
                    k8s_openapi::api::core::v1::ServicePort {
                        name: Some("http".to_string()),
                        port: 80,
                        protocol: Some("TCP".to_string()),
                        ..Default::default()
                    },
                    k8s_openapi::api::core::v1::ServicePort {
                        name: Some("metrics".to_string()),
                        port: 81,
                        protocol: Some("TCP".to_string()),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            }),
            ..Default::default()
        }),
    )]));
    coordinator.update_endpoint_slices(
        HashMap::new(),
        HashMap::from([(
            "default/affinity-slice".to_string(),
            Arc::new(EndpointSlice {
                metadata: ObjectMeta {
                    namespace: Some("default".to_string()),
                    name: Some("affinity-slice".to_string()),
                    uid: Some("slice-dual".to_string()),
                    labels: Some(BTreeMap::from([(SERVICE_NAME.to_string(), "affinity".to_string())])),
                    ..Default::default()
                },
                address_type: "IPv4".to_string(),
                endpoints: vec![Endpoint {
                    addresses: vec!["10.4.0.8".to_string()],
                    conditions: Some(EndpointConditions {
                        ready: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                ports: Some(vec![
                    EndpointPort {
                        name: Some("http".to_string()),
                        port: Some(8080),
                        protocol: Some("TCP".to_string()),
                        ..Default::default()
                    },
                    EndpointPort {
                        name: Some("metrics".to_string()),
                        port: Some(8081),
                        protocol: Some("TCP".to_string()),
                        ..Default::default()
                    },
                ]),
            }),
        )]),
        HashSet::new(),
    );

    let http = ServiceSectionKey::Number(NonZeroU16::new(80).unwrap());
    let metrics = ServiceSectionKey::Number(NonZeroU16::new(81).unwrap());
    let http_map = sp_affinity_get_or_build_from(&coordinator, "default/affinity", &http);
    let http_addr: StdSocketAddr = "10.4.0.8:8080".parse().unwrap();
    let metrics_addr: StdSocketAddr = "10.4.0.8:8081".parse().unwrap();
    assert_eq!(
        http_map.get(&generate_session_token(&http_addr.to_string())),
        Some(&http_addr)
    );
    assert!(
        !http_map.contains_key(&generate_session_token(&metrics_addr.to_string())),
        "a token issued for port 81 must miss the port-80 sticky map"
    );
    let metrics_map = sp_affinity_get_or_build_from(&coordinator, "default/affinity", &metrics);
    assert_eq!(
        metrics_map.get(&generate_session_token(&metrics_addr.to_string())),
        Some(&metrics_addr)
    );
}

#[test]
fn affinity_builder_borrows_only_the_capped_input_prefix() {
    let backends: Vec<_> = (0..=SP_AFFINITY_MAX_PER_SERVICE)
        .map(|index| {
            backend(StdSocketAddr::from((
                [10, 2, (index / 256) as u8, (index % 256) as u8],
                8080,
            )))
        })
        .collect();
    let visited = std::cell::Cell::new(0);
    let entries = build_sp_affinity_map(
        backends.iter().map(|backend| {
            visited.set(visited.get() + 1);
            assert_eq!(Arc::strong_count(backend), 1);
            backend.as_ref()
        }),
        None,
    );
    assert_eq!(visited.get(), SP_AFFINITY_MAX_PER_SERVICE);
    assert_eq!(entries.len(), SP_AFFINITY_MAX_PER_SERVICE);
    for backend in &backends[..SP_AFFINITY_MAX_PER_SERVICE] {
        let addr = backend.addr.as_inet().unwrap();
        assert_eq!(entries.get(&generate_session_token(&addr.to_string())), Some(addr));
    }
    let excluded = backends[SP_AFFINITY_MAX_PER_SERVICE].addr.as_inet().unwrap();
    assert!(!entries.contains_key(&generate_session_token(&excluded.to_string())));
}
