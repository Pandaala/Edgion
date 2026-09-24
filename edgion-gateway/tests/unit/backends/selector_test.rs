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
    publish_service_with_ready(namespace, name, ports, slice_ports, address, true)
}

fn publish_service_with_ready(
    namespace: &str,
    name: &str,
    ports: Vec<ServicePort>,
    slice_ports: Vec<(&str, i32)>,
    address: &str,
    ready: bool,
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
                        ready: Some(ready),
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
        assert!(coordinator.record_health_completion(&policy, available.membership_identity, identity, false, 1, 1));

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

#[test]
fn sticky_address_recognizes_unready_slot_but_marks_it_ineligible() {
    let unready = publish_service_with_ready(
        "sel-sticky-ready",
        "unready",
        vec![service_port("http", 80, IntOrString::Int(8080), None)],
        vec![("http", 8080)],
        "10.8.0.6",
        false,
    );
    assert!(select_service_endpoint(
        &unready.service_key,
        section_number(80),
        &None,
        ServiceDegradePolicy::Live,
        None,
        "",
        None,
    )
    .is_err());
    let unready_addr: StdSocketAddr = "10.8.0.6:8080".parse().unwrap();
    let (endpoint, eligible) =
        resolved_endpoint_for_addr(&unready.service_key, &section_number(80), unready_addr).unwrap();
    assert!(!endpoint.ready);
    assert!(!eligible);

    let ready = publish_service(
        "sel-sticky-ready",
        "ready",
        vec![service_port("http", 80, IntOrString::Int(8080), None)],
        vec![("http", 8080)],
        "10.8.0.7",
    );
    assert!(select_service_endpoint(
        &ready.service_key,
        section_number(80),
        &None,
        ServiceDegradePolicy::Live,
        None,
        "",
        None,
    )
    .is_ok());
    let ready_addr: StdSocketAddr = "10.8.0.7:8080".parse().unwrap();
    let (endpoint, eligible) = resolved_endpoint_for_addr(&ready.service_key, &section_number(80), ready_addr).unwrap();
    assert!(endpoint.ready);
    assert!(eligible);
}
