//! Service-scoped LB runtime state — SP affinity + backend lifecycle ledger.
//!
//! Two responsibilities:
//!   1. Session-persistence affinity map (token → endpoint SocketAddr).
//!   2. Backend lifecycle ledger: draining registry.
//!
//! Metric atomics (the packed Peak-EWMA `score`, `conn_count`) live on `LbBackend` itself.
//! `DRAINING_BACKENDS` holds the live `Arc<LbBackend>` so `BackendCleaner`
//! can poll `backend.conn()` without a side counter.

use crate::backends::discovery::ServiceSectionKey;
use dashmap::DashMap;
use pingora_core::protocols::l4::socket::SocketAddr;
use std::sync::{Arc, LazyLock};

use super::backend::LbBackend;

// ── SP affinity map ───────────────────────────────────────────────────────────

/// Max entries per service in the session affinity map.
const SP_AFFINITY_MAX_PER_SERVICE: usize = 1024;

/// Membership-tagged sticky snapshot owned by one Service runtime cell.
///
/// The coordinator publishes this only after revalidating both the joint
/// Service generation and the active runtime membership.
pub struct SpAffinitySnapshot {
    pub membership_identity: u64,
    pub entries: Arc<std::collections::HashMap<String, std::net::SocketAddr>>,
}

fn build_sp_affinity_map(
    backends: &[Arc<LbBackend>],
    scope: Option<&str>,
) -> Arc<std::collections::HashMap<String, std::net::SocketAddr>> {
    let mut map = std::collections::HashMap::with_capacity(backends.len().min(SP_AFFINITY_MAX_PER_SERVICE));
    for b in backends.iter().take(SP_AFFINITY_MAX_PER_SERVICE) {
        if let SocketAddr::Inet(addr) = b.addr {
            let addr_str = addr.to_string();
            // Threat model: this unkeyed hash of `IP:port` (no secret, no MAC)
            // is a stickiness hint, NOT a security boundary. A client that
            // knows a backend's `IP:port` can forge the matching token to pin
            // its own requests to that backend, but the map only ever holds the
            // service's real backends, so a forged token can only pick an
            // existing backend (never an arbitrary address) and only affects
            // the forging client's own traffic. See
            // `session_persistence::generate_session_token` (the single token
            // derivation — map keys and response-injected tokens must never
            // drift) for the full rationale. Use an HMAC over a secret if
            // affinity is ever promoted to a routing restriction.
            let token = crate::routes::http::session_persistence::generate_scoped_session_token(scope, &addr_str);
            map.insert(token, addr);
        }
    }
    Arc::new(map)
}

/// Get or lazily build the session affinity map. The `backends_fn` closure is
/// only invoked on a cache miss, avoiding unnecessary allocation on the hot path.
pub(crate) fn sp_affinity_get_or_build_key(
    service_key: &str,
    section: &ServiceSectionKey,
    _backends_fn: impl FnOnce() -> Arc<[Arc<LbBackend>]>,
) -> Arc<std::collections::HashMap<String, std::net::SocketAddr>> {
    let coordinator = crate::backends::discovery::get_service_discovery_coordinator();
    sp_affinity_get_or_build_from(&coordinator, service_key, section)
}

fn sp_affinity_get_or_build_from(
    coordinator: &crate::backends::discovery::ServiceDiscoveryCoordinator,
    service_key: &str,
    section: &ServiceSectionKey,
) -> Arc<std::collections::HashMap<String, std::net::SocketAddr>> {
    let generation = coordinator.load_generation();
    let Some(available) = generation.available(service_key) else {
        return Arc::new(std::collections::HashMap::new());
    };
    let membership = available.membership_identity;
    let cell = Arc::clone(&available.runtime_cell);
    let active = cell.active.load_full();
    if active.membership_identity != Some(membership) {
        return Arc::new(std::collections::HashMap::new());
    }
    if let Some(snapshot) = cell.sp.load_full().get(section) {
        if snapshot.membership_identity == membership {
            return Arc::clone(&snapshot.entries);
        }
    }

    let _build_guard = cell.sp_build_lock.lock();
    if let Some(snapshot) = cell.sp.load_full().get(section) {
        if snapshot.membership_identity == membership {
            return Arc::clone(&snapshot.entries);
        }
    }
    try_publish_sp_snapshot(
        coordinator,
        service_key,
        section,
        &generation,
        &active,
        available,
        &cell,
    )
    .unwrap_or_else(|| Arc::new(std::collections::HashMap::new()))
}

fn try_publish_sp_snapshot(
    coordinator: &crate::backends::discovery::ServiceDiscoveryCoordinator,
    _service_key: &str,
    section: &ServiceSectionKey,
    generation: &Arc<crate::backends::discovery::ServiceSelectorGeneration>,
    active: &Arc<crate::backends::discovery::ServiceRuntimeSnapshot>,
    available: &Arc<crate::backends::discovery::AvailableServiceProjection>,
    cell: &Arc<crate::backends::discovery::ServiceRuntimeCell>,
) -> Option<Arc<std::collections::HashMap<String, std::net::SocketAddr>>> {
    let membership = available.membership_identity;
    let current_generation = coordinator.load_generation();
    let current_active = cell.active.load_full();
    let valid = Arc::ptr_eq(generation, &current_generation)
        && Arc::ptr_eq(active, &current_active)
        && current_active.membership_identity == Some(membership);
    if !valid {
        return None;
    }
    let resolved = available.resolve_section(section).ok()?;
    let backends: Vec<Arc<LbBackend>> = resolved
        .all_slots
        .iter()
        .map(|slot| Arc::clone(&slot.backend))
        .collect();
    let entries = build_sp_affinity_map(&backends, None);
    let after_generation = coordinator.load_generation();
    let after_active = cell.active.load_full();
    if !Arc::ptr_eq(&current_generation, &after_generation)
        || !Arc::ptr_eq(&current_active, &after_active)
        || after_active.membership_identity != Some(membership)
    {
        return None;
    }
    let mut next = (*cell.sp.load_full()).clone();
    next.insert(
        section.clone(),
        Arc::new(SpAffinitySnapshot {
            membership_identity: membership,
            entries: Arc::clone(&entries),
        }),
    );
    cell.sp.store(Arc::new(next));
    Some(entries)
}

pub(crate) fn sp_affinity_get_or_build_scoped(
    service_key: &str,
    section: &ServiceSectionKey,
    scope: Option<&str>,
    backends_fn: impl FnOnce() -> Arc<[Arc<LbBackend>]>,
) -> Arc<std::collections::HashMap<String, std::net::SocketAddr>> {
    // Typed scopes may include a direct backend UID. Keeping those generations in
    // this process-global Service cache would let an in-flight request recreate
    // a retired scope after hot-reload cleanup. Build the bounded view from the
    // request-captured endpoint Arc instead; only ordinary Service affinity is
    // cached globally.
    match scope {
        Some(scope) => build_sp_affinity_map(&backends_fn(), Some(scope)),
        None => sp_affinity_get_or_build_key(service_key, section, backends_fn),
    }
}

// ── Backend lifecycle ledger ──────────────────────────────────────────────────

/// Draining backend registry: backends removed from EP/EPS that still have
/// in-flight connections. Holds the live `Arc<LbBackend>` so
/// `BackendCleaner` can poll the exact wrapper's generation-owned connection
/// count. Shared transport counters are never used as drain ownership.
/// Key: `"service_key\0addr_string\0incarnation\0proof"` for typed backends.
static DRAINING_BACKENDS: LazyLock<DashMap<String, Arc<LbBackend>>> = LazyLock::new(DashMap::new);

pub struct DrainingStats {
    pub backend_count: usize,
    pub service_keys: Vec<String>,
    pub service_keys_truncated: bool,
}

pub fn draining_stats(key_limit: usize) -> DrainingStats {
    let backend_count = DRAINING_BACKENDS.len();
    let mut service_keys = DRAINING_BACKENDS
        .iter()
        .filter_map(|entry| entry.key().split('\0').next().map(str::to_string))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let service_keys_truncated = service_keys.len() > key_limit;
    service_keys.truncate(key_limit);
    DrainingStats {
        backend_count,
        service_keys,
        service_keys_truncated,
    }
}

fn backend_key(service_key: &str, addr: &SocketAddr, backend: &LbBackend) -> String {
    match backend.runtime_identity() {
        Some(identity) => format!(
            "{}\0{}\0{}\0{}",
            service_key,
            addr,
            identity.incarnation,
            hex::encode(identity.proof_digest)
        ),
        None => format!("{}\0{}", service_key, addr),
    }
}

/// Register a backend as draining. Call when a backend is removed from EP/EPS
/// but still has in-flight connections. The cleaner polls `backend.conn()` and
/// removes it once it reaches zero.
pub fn begin_draining(service_key: &str, addr: &SocketAddr, backend: Arc<LbBackend>) {
    let key = backend_key(service_key, addr, &backend);
    DRAINING_BACKENDS.insert(key, backend);
    tracing::info!(
        service_key = %service_key,
        "Backend marked as draining"
    );
}

/// Remove lifecycle state for exactly one backend identity.
pub fn remove_backend_exact(service_key: &str, addr: &SocketAddr, backend: &LbBackend) {
    DRAINING_BACKENDS.remove(&backend_key(service_key, addr, backend));
}

pub fn finish_draining(key: &str) {
    DRAINING_BACKENDS.remove(key);
}

/// Return all backends currently in draining state, as `(key, Arc<LbBackend>)`.
/// Used by `BackendCleaner` to poll conn counts.
pub fn all_draining_backends() -> Vec<(String, Arc<LbBackend>)> {
    DRAINING_BACKENDS
        .iter()
        .map(|e| (e.key().clone(), e.value().clone()))
        .collect()
}

/// Remove the entire lifecycle state for a service (service deleted).
///
/// Concurrency window (documented, not fixed): `retain()` is a compound
/// multi-key scan-and-delete that can interleave with concurrent per-key
/// writers — `begin_draining` / `remove_backend` driven by the EndpointSlice
/// dispatch task, and `remove_backend` driven by the background
/// `BackendCleaner`. A `begin_draining` insert landing in a DashMap shard
/// that `retain` has already scanned can survive this sweep, leaving a
/// draining backend whose owning service has been removed.
///
/// Accepted because DRAINING_BACKENDS is self-healing runtime lifecycle
/// state — `BackendCleaner` (see `lb/leastconn/cleaner.rs`) removes any
/// backend whose `conn()` reaches zero. The orphan is bounded by connection
/// lifetime, not a permanent residue. See
/// `docs/superpowers/specs/2026-06-09-gateway-cross-kind-derived-state-race-audit-design.md`
/// for the full audit.
pub fn remove_service(service_key: &str) {
    let prefix = format!("{}\0", service_key);
    DRAINING_BACKENDS.retain(|k, _| !k.starts_with(&prefix));
    tracing::info!(
        service_key = %service_key,
        "Removed service runtime state"
    );
}

#[cfg(test)]
mod tests {
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
        let map = build_sp_affinity_map(&backends, None);
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
}
