use crate::backends::discovery::{
    get_service_discovery_coordinator, AvailableServiceProjection, ResolvedServiceSection, ServiceRuntimeSnapshot,
    ServiceSectionKey,
};
use crate::lb::{compiled_selector, LbBackend, PolicyKind};
use edgion_resources::resources::lb_policy::{ConsistentHashOn, ParsedLBPolicy};
use edgion_resources::uri_utils;
use edgion_resources::EdgionErrorCode;
use pingora_proxy::Session;
use std::net::SocketAddr;
use std::num::NonZeroU16;
use std::sync::Arc;

pub struct ResolvedServiceEndpoint {
    pub backend: Arc<LbBackend>,
    pub section: Arc<ResolvedServiceSection>,
    pub serving_port: NonZeroU16,
    pub ready: bool,
    pub slot_proof: [u8; 32],
}

/// Configuration boundary for one Service selection. HTTP/gRPC preserve their
/// captured Service override, including absence; L4 and mirrors use live policy.
#[derive(Clone, Copy)]
pub enum ServiceDegradePolicy {
    /// `None` uses the independently updated global default, never a live
    /// Service override. `Some(0)` explicitly disables degrade mode.
    CapturedOverride(Option<u8>),
    Live,
}

impl ServiceDegradePolicy {
    fn threshold(self, active: &ServiceRuntimeSnapshot) -> u8 {
        match self {
            Self::CapturedOverride(value) => value.unwrap_or_else(super::degrade_threshold::get_lb_degrade_default),
            Self::Live => active.degrade_threshold,
        }
    }
}

pub fn service_section_key_from_port(port: Option<i32>) -> Result<ServiceSectionKey, EdgionErrorCode> {
    port.and_then(|value| u16::try_from(value).ok())
        .and_then(NonZeroU16::new)
        .map(ServiceSectionKey::Number)
        .ok_or(EdgionErrorCode::BackendPortResolutionFailed)
}

fn load_active_projection(service_key: &str) -> Option<(Arc<AvailableServiceProjection>, Arc<ServiceRuntimeSnapshot>)> {
    let coordinator = get_service_discovery_coordinator();
    let generation = coordinator.load_generation();
    let available = Arc::clone(generation.available(service_key)?);
    let active = available.runtime_cell.active.load_full();
    (active.membership_identity == Some(available.membership_identity)).then_some((available, active))
}

#[inline]
fn runtime_eligible(active: &ServiceRuntimeSnapshot, backend: &LbBackend, record_rejection: bool) -> bool {
    let Some(identity) = backend.runtime_identity() else {
        return false;
    };
    let Some(state) = active.entries.get(identity.as_ref()) else {
        return false;
    };
    if !state.health_eligible() {
        return false;
    }
    if state.outlier_ejected() {
        if record_rejection {
            crate::observe::metrics::record_cb_rejection(identity.service_key.as_ref());
        }
        return false;
    }
    true
}

/// Decide whether degrade mode should engage for this selection. Gated so the
/// O(n) survivor count only runs for services that actually have health-check
/// or outlier-detection state (otherwise every backend passes and degrade can
/// never trigger). The caller records an event only when selection takes the
/// degrade branch, using the handle captured in this same runtime snapshot.
fn degrade_engaged(
    selector: &compiled_selector::CompiledSelector,
    active: &ServiceRuntimeSnapshot,
    threshold: u8,
) -> bool {
    if !active.health_policy_present && !active.outlier_policy_present {
        return false;
    }
    if threshold == 0 {
        return false;
    }
    let total = selector.backend_count();
    if total == 0 {
        return false;
    }
    let floor = crate::backends::degrade_threshold::degrade_floor(threshold, total);
    selector.degrade_should_engage(floor, |b| runtime_eligible(active, b, false))
}

/// Map a parsed LB policy to its eager-selector discriminator.
#[inline]
fn policy_to_kind(lb_policy: &Option<ParsedLBPolicy>) -> PolicyKind {
    match lb_policy {
        None => PolicyKind::RoundRobin,
        Some(ParsedLBPolicy::ConsistentHash(_)) => PolicyKind::ConsistentHash,
        Some(ParsedLBPolicy::LeastConn) => PolicyKind::LeastConn,
        Some(ParsedLBPolicy::Ewma) => PolicyKind::Ewma,
    }
}

/// Extract hash key from request based on LB policy configuration.
///
/// `remote_ip` is the RealIp-normalized client source IP (`ctx.request_info.remote_ip`),
/// used only by the `SourceIp` mode; the other modes read from the request headers/URI.
pub(crate) fn extract_hash_key<'a>(
    session: &'a Session,
    lb_policy: &Option<ParsedLBPolicy>,
    remote_ip: &'a str,
    query: Option<&'a str>,
) -> &'a [u8] {
    let Some(ParsedLBPolicy::ConsistentHash(hash_on)) = lb_policy else {
        return &[];
    };

    let req_header = session.req_header();

    match hash_on {
        ConsistentHashOn::Header(header_name) => req_header
            .headers
            .get(header_name.as_str())
            .and_then(|v| v.to_str().ok())
            .map(str::as_bytes)
            .unwrap_or_default(),
        ConsistentHashOn::Cookie(cookie_name) => req_header
            .headers
            .get("cookie")
            .and_then(|v| v.to_str().ok())
            .and_then(|cookies| {
                cookies.split(';').map(|s| s.trim()).find_map(|cookie| {
                    let (name, value) = cookie.split_once('=')?;
                    if name == cookie_name {
                        Some(value.as_bytes())
                    } else {
                        None
                    }
                })
            })
            .unwrap_or_default(),
        ConsistentHashOn::Arg(arg_name) => query
            .and_then(|query| {
                query.split('&').find_map(|param| {
                    let (name, value) = param.split_once('=')?;
                    // Fast path: most query param names are plain identifiers
                    // that match arg_name byte-for-byte, so the common case stays
                    // allocation-free. Only percent/plus-encoded names need
                    // decoding before comparison (e.g. `session%20id` / `foo+bar`
                    // matching an arg_name of `session id` / `foo bar`). The value
                    // is intentionally left raw — decoding it would change the hash
                    // key of existing deployments.
                    let matched =
                        name == arg_name || (name.contains(['%', '+']) && uri_utils::url_decode(name) == *arg_name);
                    matched.then_some(value.as_bytes())
                })
            })
            .unwrap_or_default(),
        // SourceIp hashes on the normalized client IP; an empty IP yields an
        // empty key, driving the internal RR fallback like the other modes when
        // their value is missing.
        ConsistentHashOn::SourceIp => remote_ip.as_bytes(),
    }
}

/// Errors raised when the cell yields no backend, broken out per LB policy
/// so the caller maps to the historically-correct `EdgionErrorCode`.
struct NotFoundMap {
    /// Empty backend list (no policy applies).
    empty: EdgionErrorCode,
    /// Round-robin selection found nothing healthy.
    rr: EdgionErrorCode,
    /// Consistent hash selected nothing (non-empty key path).
    ch: EdgionErrorCode,
    /// LeastConn found nothing.
    lc: EdgionErrorCode,
    /// EWMA found nothing.
    ewma: EdgionErrorCode,
}

impl NotFoundMap {
    fn for_kind(&self, kind: PolicyKind, ch_used_hash_key: bool) -> EdgionErrorCode {
        match kind {
            PolicyKind::RoundRobin => self.rr,
            PolicyKind::ConsistentHash if ch_used_hash_key => self.ch,
            // Empty hash key falls back to RR-style selection internally, so
            // surface the RR not-found code to match historical behaviour.
            PolicyKind::ConsistentHash => self.rr,
            PolicyKind::LeastConn => self.lc,
            PolicyKind::Ewma => self.ewma,
        }
    }
}

fn endpoint_slice_not_found() -> NotFoundMap {
    NotFoundMap {
        empty: EdgionErrorCode::BackendEndpointSliceNotFoundByRoundRobinDefault,
        rr: EdgionErrorCode::BackendEndpointSliceNotFoundByRoundRobin,
        ch: EdgionErrorCode::BackendEndpointSliceNotFoundByConsistent,
        lc: EdgionErrorCode::BackendEndpointSliceNotFoundByLeastConn,
        ewma: EdgionErrorCode::BackendEndpointSliceNotFoundByEwma,
    }
}

fn slot_for_backend<'a>(
    section: &'a ResolvedServiceSection,
    backend: &Arc<LbBackend>,
) -> Option<&'a crate::backends::discovery::ResolvedBackendSlot> {
    section
        .all_slots
        .iter()
        .find(|slot| Arc::ptr_eq(&slot.backend, backend))
        .map(Arc::as_ref)
}

pub fn select_service_endpoint(
    service_key: &str,
    section: ServiceSectionKey,
    lb_policy: &Option<ParsedLBPolicy>,
    degrade_policy: ServiceDegradePolicy,
    session: Option<&Session>,
    remote_ip: &str,
    query: Option<&str>,
) -> Result<ResolvedServiceEndpoint, EdgionErrorCode> {
    let not_found = endpoint_slice_not_found();
    let (available, active) = load_active_projection(service_key).ok_or(not_found.empty)?;
    let section = available
        .resolve_section(&section)
        .map_err(|_| EdgionErrorCode::BackendPortResolutionFailed)?;

    let kind = policy_to_kind(lb_policy);
    let selector = section.selectors.get(kind);
    let (hash_key, hash_key_used) = match (kind, session) {
        (PolicyKind::ConsistentHash, Some(session)) => {
            let key = extract_hash_key(session, lb_policy, remote_ip, query);
            let used = !key.is_empty();
            (Some(key), used)
        }
        (PolicyKind::ConsistentHash, None) => (None, false),
        _ => (None, false),
    };

    let now = crate::lb::ewma::score::now_units();
    let result = if degrade_engaged(selector, &active, degrade_policy.threshold(&active)) {
        if let Some(counter) = &active.degrade_selections {
            counter.record_selection();
        }
        selector.select(hash_key, |_| true, now)
    } else {
        selector.select(hash_key, |b| runtime_eligible(&active, b, true), now)
    };
    let backend = result.ok_or(not_found.for_kind(kind, hash_key_used))?;
    let (ready, slot_proof) = slot_for_backend(&section, &backend)
        .map(|slot| (slot.ready, slot.proof))
        .ok_or(EdgionErrorCode::BackendPortResolutionFailed)?;
    let serving_port = backend
        .addr
        .as_inet()
        .and_then(|addr| NonZeroU16::new(addr.port()))
        .ok_or(EdgionErrorCode::BackendPortResolutionFailed)?;
    Ok(ResolvedServiceEndpoint {
        backend,
        section,
        serving_port,
        ready,
        slot_proof,
    })
}

/// Current backends for one proven Service section. Used by health-adjacent
/// session-affinity construction for that section only.
pub fn backends_for_service_section(service_key: &str, section: &ServiceSectionKey) -> Arc<[Arc<LbBackend>]> {
    let Some((available, _)) = load_active_projection(service_key) else {
        return Arc::from(Vec::new());
    };
    match available.resolve_section(section) {
        Ok(section) => section
            .all_slots
            .iter()
            .map(|slot| Arc::clone(&slot.backend))
            .collect::<Vec<_>>()
            .into(),
        Err(_) => Arc::from(Vec::new()),
    }
}

pub(crate) fn resolved_endpoint_for_addr(
    service_key: &str,
    section: &ServiceSectionKey,
    target: SocketAddr,
) -> Option<(ResolvedServiceEndpoint, bool)> {
    let (available, active) = load_active_projection(service_key)?;
    let section = available.resolve_section(section).ok()?;
    let slot = section
        .all_slots
        .iter()
        .find(|slot| slot.backend.addr.as_inet().is_some_and(|addr| *addr == target))?;
    let backend = Arc::clone(&slot.backend);
    let ready = slot.ready;
    let slot_proof = slot.proof;
    let eligible = runtime_eligible(&active, &backend, true);
    let serving_port = NonZeroU16::new(target.port())?;
    Some((
        ResolvedServiceEndpoint {
            backend,
            section,
            serving_port,
            ready,
            slot_proof,
        },
        eligible,
    ))
}

pub(crate) fn prove_service_section(
    service_key: &str,
    section: &ServiceSectionKey,
) -> Result<Arc<ResolvedServiceSection>, EdgionErrorCode> {
    let coordinator = get_service_discovery_coordinator();
    let generation = coordinator.load_generation();
    let available = generation
        .available(service_key)
        .ok_or(EdgionErrorCode::BackendEndpointSliceNotFoundByRoundRobinDefault)?;
    available
        .resolve_section(section)
        .map_err(|_| EdgionErrorCode::BackendPortResolutionFailed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::discovery::get_service_discovery_coordinator;
    use crate::backends::peer_builder::parse_backend_app_protocol;
    use edgion_resources::constants::labels::k8s::SERVICE_NAME;
    use k8s_openapi::api::core::v1::{Service, ServicePort, ServiceSpec};
    use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions, EndpointPort, EndpointSlice};
    use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
    use kube::api::ObjectMeta;
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::net::SocketAddr as StdSocketAddr;

    struct PublishedService {
        service_key: String,
        slice_key: String,
    }

    impl Drop for PublishedService {
        fn drop(&mut self) {
            let coordinator = get_service_discovery_coordinator();
            coordinator.set_health_policy(&self.service_key, None);
            coordinator.set_outlier_policy(&self.service_key, None);
            coordinator.update_services(
                HashMap::new(),
                HashMap::new(),
                HashSet::from([self.service_key.clone()]),
            );
            coordinator.update_endpoint_slices(HashMap::new(), HashMap::new(), HashSet::from([self.slice_key.clone()]));
        }
    }

    fn section_number(port: u16) -> ServiceSectionKey {
        ServiceSectionKey::Number(NonZeroU16::new(port).unwrap())
    }

    fn publish_service(
        namespace: &str,
        name: &str,
        ports: Vec<ServicePort>,
        slice_ports: Vec<(&str, i32)>,
        address: &str,
    ) -> PublishedService {
        let service_key = format!("{namespace}/{name}");
        let slice_key = format!("{namespace}/{name}-slice");
        let coordinator = get_service_discovery_coordinator();
        coordinator.update_services(
            HashMap::from([(
                service_key.clone(),
                Arc::new(Service {
                    metadata: ObjectMeta {
                        namespace: Some(namespace.to_string()),
                        name: Some(name.to_string()),
                        uid: Some(format!("{name}-uid")),
                        ..Default::default()
                    },
                    spec: Some(ServiceSpec {
                        ports: Some(ports),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            )]),
            HashMap::new(),
            HashSet::new(),
        );
        coordinator.update_endpoint_slices(
            HashMap::from([(
                slice_key.clone(),
                Arc::new(EndpointSlice {
                    metadata: ObjectMeta {
                        namespace: Some(namespace.to_string()),
                        name: Some(format!("{name}-slice")),
                        uid: Some(format!("{name}-slice-uid")),
                        labels: Some(BTreeMap::from([(SERVICE_NAME.to_string(), name.to_string())])),
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
                    ports: Some(
                        slice_ports
                            .into_iter()
                            .map(|(port_name, port)| EndpointPort {
                                name: (!port_name.is_empty()).then(|| port_name.to_string()),
                                port: Some(port),
                                protocol: Some("TCP".to_string()),
                                ..Default::default()
                            })
                            .collect(),
                    ),
                }),
            )]),
            HashMap::new(),
            HashSet::new(),
        );
        PublishedService { service_key, slice_key }
    }

    fn service_port(name: &str, number: i32, target: IntOrString, app_protocol: Option<&str>) -> ServicePort {
        ServicePort {
            name: Some(name.to_string()),
            port: number,
            protocol: Some("TCP".to_string()),
            app_protocol: app_protocol.map(str::to_string),
            target_port: Some(target),
            ..Default::default()
        }
    }

    fn publish_degrade_service(name: &str) -> PublishedService {
        publish_service(
            "sel-degrade",
            name,
            vec![
                service_port("http", 80, IntOrString::Int(8080), None),
                service_port("metrics", 81, IntOrString::Int(8081), None),
            ],
            vec![("http", 8080), ("metrics", 8081)],
            "10.8.1.1",
        )
    }

    fn assert_degrade_count(handle: &metrics_exporter_prometheus::PrometheusHandle, key: &str, count: u64) {
        let rendered = crate::observe::metrics::GatewayMetrics::with_live_lb_degrade_counters(|| handle.render());
        let expected = format!("edgion_lb_degrade_selections_total{{service=\"{key}\"}} {count}");
        assert!(rendered.lines().any(|line| line == expected), "{rendered}");
        assert!(!rendered.contains("edgion_lb_degrade_mode"));
    }

    #[test]
    fn captured_degrade_threshold_uses_live_health_and_a_cold_counter_at_zero_live_threshold() {
        use crate::backends::degrade_threshold::{configure_lb_degrade_default, GLOBAL_STORE_TEST_LOCK};
        use crate::backends::discovery::HealthPolicyIdentity;

        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        configure_lb_degrade_default(0);
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            let published = publish_degrade_service("captured-zero-live");
            let key = &published.service_key;
            let coordinator = get_service_discovery_coordinator();
            let policy = Arc::new(HealthPolicyIdentity { generation: 1 });
            coordinator.set_health_policy(key, Some(Arc::clone(&policy)));
            let (available, active) = load_active_projection(key).unwrap();
            assert_eq!(active.degrade_threshold, 0);
            assert_degrade_count(&handle, key, 0);
            let section = available.resolve_section(&section_number(80)).unwrap();
            let identity = section.all_slots[0].backend.runtime_identity().unwrap();
            let select = |degrade| select_service_endpoint(key, section_number(80), &None, degrade, None, "", None);
            let captured = ServiceDegradePolicy::CapturedOverride(Some(100));
            assert!(select(captured).is_ok());
            assert_degrade_count(&handle, key, 0);
            assert!(coordinator.record_health_completion(
                &policy,
                available.membership_identity,
                identity,
                false,
                1,
                1
            ));

            // A different recorder catches any accidental hot-path registration.
            let other = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
            metrics::with_local_recorder(&other, || {
                assert!(select(ServiceDegradePolicy::Live).is_err());
                assert!(select(captured).is_ok());
            });
            assert_degrade_count(&handle, key, 1);
            assert!(!other.handle().render().contains("edgion_lb_degrade_selections_total"));

            // No captured override uses only the global default. It must not
            // adopt a subsequently installed live Service override.
            coordinator.set_degrade_threshold(key, 100);
            assert!(select(ServiceDegradePolicy::CapturedOverride(None)).is_err());
            assert!(select(ServiceDegradePolicy::CapturedOverride(Some(0))).is_err());
            assert!(select(ServiceDegradePolicy::Live).is_ok());
            assert_degrade_count(&handle, key, 2);
            configure_lb_degrade_default(100);
            assert!(select(ServiceDegradePolicy::CapturedOverride(None)).is_ok());
            assert!(select(ServiceDegradePolicy::CapturedOverride(Some(0))).is_err());
            assert_degrade_count(&handle, key, 3);

            assert!(coordinator.record_health_completion(&policy, available.membership_identity, identity, true, 1, 1));
            assert!(select(captured).is_ok());
            assert_degrade_count(&handle, key, 3);
            coordinator.set_health_policy(key, None);
            assert!(select(captured).is_ok());
            assert_degrade_count(&handle, key, 3);
        });
    }

    #[test]
    fn degrade_selections_survive_idle_scrapes_until_the_last_snapshot_is_released() {
        use crate::backends::degrade_threshold::{configure_lb_degrade_default, GLOBAL_STORE_TEST_LOCK};
        use crate::backends::discovery::HealthPolicyIdentity;
        use crate::observe::metrics::GatewayMetrics;
        use std::time::Duration;

        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        configure_lb_degrade_default(50);
        // Same eviction mask as the production recorder, with a short timeout.
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new()
            .idle_timeout(metrics_util::MetricKindMask::ALL, Some(Duration::from_millis(1)))
            .build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            let published = publish_degrade_service("idle");
            let key = published.service_key.clone();
            let coordinator = get_service_discovery_coordinator();
            let policy = Arc::new(HealthPolicyIdentity { generation: 1 });
            coordinator.set_health_policy(&key, Some(Arc::clone(&policy)));
            let (available, captured) = load_active_projection(&key).unwrap();
            let section = available.resolve_section(&section_number(80)).unwrap();
            assert!(coordinator.record_health_completion(
                &policy,
                available.membership_identity,
                section.all_slots[0].backend.runtime_identity().unwrap(),
                false,
                1,
                1,
            ));
            drop(available);
            let weak = Arc::downgrade(captured.degrade_selections.as_ref().unwrap());
            metrics::counter!("test_idle_control_total").increment(1);
            select_service_endpoint(
                &key,
                section_number(80),
                &None,
                ServiceDegradePolicy::Live,
                None,
                "",
                None,
            )
            .unwrap();
            assert_degrade_count(&handle, &key, 1);

            std::thread::sleep(Duration::from_millis(20));
            assert_degrade_count(&handle, &key, 1);
            let rendered = GatewayMetrics::with_live_lb_degrade_counters(|| handle.render());
            assert!(
                !rendered.contains("test_idle_control_total"),
                "idle eviction must be enabled"
            );
            select_service_endpoint(
                &key,
                section_number(80),
                &None,
                ServiceDegradePolicy::Live,
                None,
                "",
                None,
            )
            .unwrap();
            assert_degrade_count(&handle, &key, 2);

            drop(published);
            assert!(coordinator.runtime_cell(&key).is_none());
            std::thread::sleep(Duration::from_millis(20));
            assert_degrade_count(&handle, &key, 2);
            captured.degrade_selections.as_ref().unwrap().record_selection();
            assert_degrade_count(&handle, &key, 3);
            drop(captured);
            assert!(
                weak.upgrade().is_none(),
                "the metric registry must not retain deleted snapshots"
            );
            std::thread::sleep(Duration::from_millis(20));
            let rendered = GatewayMetrics::with_live_lb_degrade_counters(|| handle.render());
            assert!(!rendered.contains("edgion_lb_degrade_selections_total"), "{rendered}");
        });
    }

    #[test]
    fn degrade_selections_cache_handle_and_sum_sections_across_policy_updates() {
        use crate::backends::degrade_threshold::{configure_lb_degrade_default, GLOBAL_STORE_TEST_LOCK};
        use crate::backends::discovery::HealthPolicyIdentity;

        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        configure_lb_degrade_default(0);
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let coordinator = get_service_discovery_coordinator();
        let policy = Arc::new(HealthPolicyIdentity { generation: 1 });
        let published = metrics::with_local_recorder(&recorder, || {
            let published = publish_degrade_service("health");
            for _ in 0..3 {
                select_service_endpoint(
                    &published.service_key,
                    section_number(80),
                    &None,
                    ServiceDegradePolicy::Live,
                    None,
                    "",
                    None,
                )
                .unwrap();
            }
            assert!(!handle.render().contains("edgion_lb_degrade_selections_total"));
            configure_lb_degrade_default(50);
            assert!(!handle.render().contains("edgion_lb_degrade_selections_total"));
            coordinator.set_health_policy(&published.service_key, Some(Arc::clone(&policy)));
            assert_degrade_count(&handle, &published.service_key, 0);
            published
        });
        let key = &published.service_key;
        let (available, active) = load_active_projection(key).unwrap();
        let http = available.resolve_section(&section_number(80)).unwrap();
        let backend = &http.all_slots[0].backend;
        assert!(coordinator.record_health_completion(
            &policy,
            available.membership_identity,
            backend.runtime_identity().unwrap(),
            false,
            1,
            1,
        ));

        // Changing the thread-local recorder detects any request-time registry
        // lookup: all events must still reach the publication-time handle.
        let other_recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        metrics::with_local_recorder(&other_recorder, || {
            for _ in 0..3 {
                select_service_endpoint(
                    key,
                    section_number(80),
                    &None,
                    ServiceDegradePolicy::Live,
                    None,
                    "",
                    None,
                )
                .unwrap();
                select_service_endpoint(
                    key,
                    section_number(81),
                    &None,
                    ServiceDegradePolicy::Live,
                    None,
                    "",
                    None,
                )
                .unwrap();
            }
            assert_degrade_count(&handle, key, 3);
            configure_lb_degrade_default(0);
            assert!(select_service_endpoint(
                key,
                section_number(80),
                &None,
                ServiceDegradePolicy::Live,
                None,
                "",
                None
            )
            .is_err());
            assert_degrade_count(&handle, key, 3);
            configure_lb_degrade_default(50);
            let metrics_section = available.resolve_section(&section_number(81)).unwrap();
            assert!(coordinator.record_health_completion(
                &policy,
                available.membership_identity,
                metrics_section.all_slots[0].backend.runtime_identity().unwrap(),
                false,
                1,
                1,
            ));
            select_service_endpoint(
                key,
                section_number(80),
                &None,
                ServiceDegradePolicy::Live,
                None,
                "",
                None,
            )
            .unwrap();
            select_service_endpoint(
                key,
                section_number(81),
                &None,
                ServiceDegradePolicy::Live,
                None,
                "",
                None,
            )
            .unwrap();
            assert_degrade_count(&handle, key, 5);

            coordinator.set_health_policy(key, None);
            select_service_endpoint(
                key,
                section_number(80),
                &None,
                ServiceDegradePolicy::Live,
                None,
                "",
                None,
            )
            .unwrap();
            assert_degrade_count(&handle, key, 5);
            assert!(!other_recorder
                .handle()
                .render()
                .contains("edgion_lb_degrade_selections_total"));
        });
        // Captured snapshots keep the same event handle after a policy update.
        assert!(active.degrade_selections.is_some());
    }

    #[test]
    fn degrade_selections_outlier_removal_empty_membership_and_delete_preserve_history() {
        use crate::backends::degrade_threshold::{configure_lb_degrade_default, GLOBAL_STORE_TEST_LOCK};
        use crate::backends::outlier_detection::{FailureClass, OutlierDetectionStore};
        use edgion_resources::resources::outlier_detection::OutlierDetectionResolved;

        let _guard = GLOBAL_STORE_TEST_LOCK.lock();
        configure_lb_degrade_default(50);
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            let published = publish_degrade_service("outlier");
            let key = &published.service_key;
            let coordinator = get_service_discovery_coordinator();
            let outliers = OutlierDetectionStore::new();
            let config = OutlierDetectionResolved {
                consecutive_errors: 1,
                consecutive_gateway_errors: 1,
                consecutive_local_origin_failures: 1,
                ejection_seconds: 3600,
                max_ejection_seconds: 3600,
                max_ejection_percent: 0,
            };
            outliers.set_service_config_in(&coordinator, key, Some(config.clone()));
            let endpoint = select_service_endpoint(
                key,
                section_number(80),
                &None,
                ServiceDegradePolicy::Live,
                None,
                "",
                None,
            )
            .unwrap();
            let token = outliers.capture_completion_token(&endpoint.backend).unwrap();
            outliers.record_failure_for_backend_in(&coordinator, &endpoint.backend, &token, FailureClass::Gateway);
            select_service_endpoint(
                key,
                section_number(80),
                &None,
                ServiceDegradePolicy::Live,
                None,
                "",
                None,
            )
            .unwrap();
            assert_degrade_count(&handle, key, 1);
            outliers.set_service_config_in(&coordinator, key, None);
            select_service_endpoint(
                key,
                section_number(80),
                &None,
                ServiceDegradePolicy::Live,
                None,
                "",
                None,
            )
            .unwrap();
            assert_degrade_count(&handle, key, 1);

            outliers.set_service_config_in(&coordinator, key, Some(config));
            let token = outliers.capture_completion_token(&endpoint.backend).unwrap();
            outliers.record_failure_for_backend_in(&coordinator, &endpoint.backend, &token, FailureClass::Gateway);
            select_service_endpoint(
                key,
                section_number(80),
                &None,
                ServiceDegradePolicy::Live,
                None,
                "",
                None,
            )
            .unwrap();
            assert_degrade_count(&handle, key, 2);
            let (_, captured) = load_active_projection(key).unwrap();
            coordinator.update_endpoint_slices(
                HashMap::new(),
                HashMap::new(),
                HashSet::from([published.slice_key.clone()]),
            );
            let (empty, active) = load_active_projection(key).unwrap();
            let empty_section = empty.resolve_section(&section_number(80)).unwrap();
            assert_eq!(empty_section.selectors.get(PolicyKind::RoundRobin).backend_count(), 0);
            assert!(!degrade_engaged(
                empty_section.selectors.get(PolicyKind::RoundRobin),
                &active,
                active.degrade_threshold
            ));
            assert!(select_service_endpoint(
                key,
                section_number(80),
                &None,
                ServiceDegradePolicy::Live,
                None,
                "",
                None
            )
            .is_err());
            assert_degrade_count(&handle, key, 2);
            coordinator.update_services(HashMap::new(), HashMap::new(), HashSet::from([key.clone()]));
            assert!(load_active_projection(key).is_none());
            assert!(coordinator.runtime_cell(key).is_none());
            assert!(select_service_endpoint(
                key,
                section_number(80),
                &None,
                ServiceDegradePolicy::Live,
                None,
                "",
                None
            )
            .is_err());
            assert_degrade_count(&handle, key, 2);

            // A worker already holding an old event handle can finish after
            // deletion. This adds history; it cannot overwrite a current state.
            std::thread::spawn(move || captured.degrade_selections.as_ref().unwrap().record_selection())
                .join()
                .unwrap();
            assert_degrade_count(&handle, key, 3);
        });
    }

    /// HTTP, TCP, TLS, UDP, and request-mirror all call this helper before
    /// `select_service_endpoint` / `prove_service_section`. A missing or
    /// non-`NonZeroU16` port is fail-closed and never becomes a first-port dial.
    #[test]
    fn missing_or_invalid_backend_ref_port_is_fail_closed() {
        for port in [None, Some(0), Some(-1), Some(i32::MAX)] {
            assert_eq!(
                service_section_key_from_port(port),
                Err(EdgionErrorCode::BackendPortResolutionFailed),
                "port={port:?}"
            );
        }
        assert_eq!(service_section_key_from_port(Some(80)).unwrap(), section_number(80));
    }

    #[test]
    fn select_requested_port_cannot_return_another_section_serving_address() {
        let published = publish_service(
            "sel-multiport",
            "backend",
            vec![
                service_port("http", 80, IntOrString::String("web".to_string()), None),
                service_port("metrics", 81, IntOrString::String("web".to_string()), None),
            ],
            vec![("http", 8080), ("metrics", 8081)],
            "10.8.0.1",
        );
        let http = select_service_endpoint(
            &published.service_key,
            section_number(80),
            &None,
            ServiceDegradePolicy::Live,
            None,
            "",
            None,
        )
        .unwrap();
        let metrics = select_service_endpoint(
            &published.service_key,
            section_number(81),
            &None,
            ServiceDegradePolicy::Live,
            None,
            "",
            None,
        )
        .unwrap();
        assert_eq!(http.serving_port.get(), 8080);
        assert_eq!(metrics.serving_port.get(), 8081);
        assert_eq!(
            http.backend.addr.as_inet().unwrap(),
            &"10.8.0.1:8080".parse::<StdSocketAddr>().unwrap()
        );
        assert_eq!(
            metrics.backend.addr.as_inet().unwrap(),
            &"10.8.0.1:8081".parse::<StdSocketAddr>().unwrap()
        );
        assert!(matches!(
            select_service_endpoint(
                &published.service_key,
                section_number(82),
                &None,
                ServiceDegradePolicy::Live,
                None,
                "",
                None
            ),
            Err(EdgionErrorCode::BackendPortResolutionFailed)
        ));
    }

    #[test]
    fn named_target_port_still_dials_slice_endpoint_port() {
        let published = publish_service(
            "sel-named-target",
            "backend",
            vec![service_port(
                "http-web",
                80,
                IntOrString::String("http".to_string()),
                None,
            )],
            vec![("http-web", 9090)],
            "10.8.0.2",
        );
        let endpoint = select_service_endpoint(
            &published.service_key,
            section_number(80),
            &None,
            ServiceDegradePolicy::Live,
            None,
            "",
            None,
        )
        .unwrap();
        assert_eq!(endpoint.serving_port.get(), 9090);
        assert_eq!(endpoint.section.identity.number.get(), 80);
        assert_eq!(endpoint.section.identity.name.as_deref(), Some("http-web"));
    }

    #[test]
    fn missing_service_and_invalid_section_fail_closed() {
        assert!(matches!(
            select_service_endpoint(
                "sel-absent/backend",
                section_number(80),
                &None,
                ServiceDegradePolicy::Live,
                None,
                "",
                None
            ),
            Err(EdgionErrorCode::BackendEndpointSliceNotFoundByRoundRobinDefault)
        ));
        let published = publish_service(
            "sel-invalid-section",
            "backend",
            vec![service_port("http", 80, IntOrString::Int(8080), None)],
            vec![("http", 8080)],
            "10.8.0.3",
        );
        assert!(matches!(
            prove_service_section(&published.service_key, &section_number(81)),
            Err(EdgionErrorCode::BackendPortResolutionFailed)
        ));
        assert!(matches!(
            select_service_endpoint(
                &published.service_key,
                section_number(81),
                &None,
                ServiceDegradePolicy::Live,
                None,
                "",
                None
            ),
            Err(EdgionErrorCode::BackendPortResolutionFailed)
        ));
    }

    #[test]
    fn app_protocol_is_read_from_published_section_identity() {
        let published = publish_service(
            "sel-app-protocol",
            "backend",
            vec![service_port(
                "http",
                80,
                IntOrString::Int(8080),
                Some("kubernetes.io/h2c"),
            )],
            vec![("http", 8080)],
            "10.8.0.4",
        );
        let endpoint = select_service_endpoint(
            &published.service_key,
            section_number(80),
            &None,
            ServiceDegradePolicy::Live,
            None,
            "",
            None,
        )
        .unwrap();
        assert_eq!(
            endpoint.section.identity.app_protocol.as_deref(),
            Some("kubernetes.io/h2c")
        );
        assert_eq!(
            parse_backend_app_protocol(endpoint.section.identity.app_protocol.as_deref().unwrap()),
            Some(crate::backends::peer_builder::BackendAppProtocol::H2c)
        );
    }

    #[test]
    fn sticky_address_from_another_section_is_a_miss() {
        let published = publish_service(
            "sel-sticky-section",
            "backend",
            vec![
                service_port("http", 80, IntOrString::String("web".to_string()), None),
                service_port("metrics", 81, IntOrString::String("web".to_string()), None),
            ],
            vec![("http", 8080), ("metrics", 8081)],
            "10.8.0.5",
        );
        let metrics_addr: StdSocketAddr = "10.8.0.5:8081".parse().unwrap();
        assert!(
            resolved_endpoint_for_addr(&published.service_key, &section_number(80), metrics_addr).is_none(),
            "a port-81 address must not resolve inside the port-80 section"
        );
        let http_addr: StdSocketAddr = "10.8.0.5:8080".parse().unwrap();
        let (endpoint, eligible) =
            resolved_endpoint_for_addr(&published.service_key, &section_number(80), http_addr).unwrap();
        assert!(eligible);
        assert_eq!(endpoint.serving_port.get(), 8080);
    }
}
