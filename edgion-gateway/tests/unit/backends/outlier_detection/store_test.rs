use super::super::state::ejection_duration;
use super::*;
use crate::backends::discovery::ServiceDiscoveryCoordinator;
use k8s_openapi::api::core::v1::{Service, ServicePort, ServiceSpec};
use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions, EndpointPort, EndpointSlice};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::ObjectMeta;
use std::collections::{BTreeMap, HashMap};

// The identifiers below select real published backends; they are never hashed
// or used as state keys. All transitions delegate to the production owner.
struct Fixture {
    store: OutlierDetectionStore,
    coordinator: ServiceDiscoveryCoordinator,
    slices: HashMap<String, Arc<EndpointSlice>>,
}

impl std::ops::Deref for Fixture {
    type Target = OutlierDetectionStore;
    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

impl Fixture {
    fn new() -> Self {
        let mut fixture = Self {
            store: OutlierDetectionStore::new(),
            coordinator: ServiceDiscoveryCoordinator::new(),
            slices: HashMap::new(),
        };
        let mut services = HashMap::new();
        let mut slices = HashMap::new();
        for name in ["svc", "svc1", "svc2"] {
            let key = format!("default/{name}");
            services.insert(
                key.clone(),
                Arc::new(Service {
                    metadata: ObjectMeta {
                        name: Some(name.into()),
                        namespace: Some("default".into()),
                        uid: Some(format!("service-{name}")),
                        ..Default::default()
                    },
                    spec: Some(ServiceSpec {
                        ports: Some(vec![ServicePort {
                            name: Some("http".into()),
                            port: 80,
                            target_port: Some(IntOrString::Int(8080)),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            );
            slices.insert(
                key,
                Arc::new(EndpointSlice {
                    metadata: ObjectMeta {
                        name: Some(name.into()),
                        namespace: Some("default".into()),
                        uid: Some(format!("slice-{name}")),
                        labels: Some(BTreeMap::from([("kubernetes.io/service-name".into(), name.into())])),
                        ..Default::default()
                    },
                    address_type: "IPv4".into(),
                    endpoints: (1..=4)
                        .map(|n| Endpoint {
                            addresses: vec![format!("10.0.0.{n}")],
                            conditions: Some(EndpointConditions {
                                ready: Some(true),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .collect(),
                    ports: Some(vec![EndpointPort {
                        name: Some("http".into()),
                        port: Some(8080),
                        protocol: Some("TCP".into()),
                        ..Default::default()
                    }]),
                }),
            );
        }
        fixture.coordinator.replace_services(&services);
        fixture.coordinator.replace_endpoint_slices(&slices);
        fixture.slices = slices;
        fixture
    }

    fn backend(&self, key: &str, index: usize) -> Arc<LbBackend> {
        Arc::clone(
            &self
                .coordinator
                .load_generation()
                .available(key)
                .unwrap()
                .canonical_probe_backends()[index],
        )
    }

    fn expire_runtime(&self, key: &str, index: usize) {
        let backend = self.backend(key, index);
        self.coordinator.runtime_cell(key).unwrap().active.load_full().entries
            [backend.runtime_identity().unwrap().as_ref()]
        .expire_outlier_deadlines_for_test();
    }

    fn endpoint(&self, key: &str, index: usize) -> Arc<PassiveEjectionState> {
        let backend = self.backend(key, index);
        Arc::clone(
            self.store
                .services
                .get(key)
                .unwrap()
                .identity_endpoints
                .get(backend.runtime_identity().unwrap().as_ref())
                .unwrap()
                .value(),
        )
    }

    fn set_service_config(&self, key: &str, config: Option<OutlierDetectionResolved>) {
        self.store.set_service_config_in(&self.coordinator, key, config);
    }

    fn record_failure(&self, key: &str, index: usize, class: FailureClass) {
        let backend = self.backend(key, index);
        if let Some(token) = self.store.capture_completion_token(&backend) {
            self.store
                .record_failure_for_backend_in(&self.coordinator, &backend, &token, class);
        }
    }

    fn record_success(&self, key: &str, index: usize) {
        let backend = self.backend(key, index);
        if let Some(token) = self.store.capture_completion_token(&backend) {
            self.store
                .record_success_for_backend_in(&self.coordinator, &backend, &token);
        }
    }

    fn is_ejected(&self, key: &str, index: usize) -> bool {
        self.coordinator.runtime_cell(key).unwrap().active.load_full().entries
            [self.backend(key, index).runtime_identity().unwrap().as_ref()]
        .outlier_ejected()
    }
    fn remove_service(&self, key: &str) {
        self.set_service_config(key, None);
    }
}

fn make_config(errors: u32, gateway: u32, seconds: u64, max_secs: u64, max_pct: u32) -> OutlierDetectionResolved {
    // Local-origin threshold inherits the gateway threshold, matching the
    // config `normalize` default, so existing tests need no extra argument.
    OutlierDetectionResolved {
        consecutive_errors: errors,
        consecutive_gateway_errors: gateway,
        consecutive_local_origin_failures: gateway,
        ejection_seconds: seconds,
        max_ejection_seconds: max_secs,
        max_ejection_percent: max_pct,
    }
}

/// Build a config with an explicit local-origin threshold distinct from the
/// gateway threshold, for the local-origin split tests.
fn make_config_lo(gateway: u32, local_origin: u32) -> OutlierDetectionResolved {
    OutlierDetectionResolved {
        consecutive_errors: 100,
        consecutive_gateway_errors: gateway,
        consecutive_local_origin_failures: local_origin,
        ejection_seconds: 30,
        max_ejection_seconds: 300,
        max_ejection_percent: 0,
    }
}

#[test]
fn test_no_config_means_not_ejected() {
    let store = Fixture::new();
    assert!(!store.is_ejected("default/svc1", 0));
}

#[test]
fn test_record_failure_ejects_after_threshold() {
    let store = Fixture::new();
    store.set_service_config("default/svc1", Some(make_config(3, 3, 30, 300, 0)));

    for _ in 0..2 {
        store.record_failure("default/svc1", 0, FailureClass::Gateway);
        assert!(!store.is_ejected("default/svc1", 0));
    }

    store.record_failure("default/svc1", 0, FailureClass::Gateway);
    assert!(store.is_ejected("default/svc1", 0));
}

#[test]
fn test_success_resets_failures() {
    let store = Fixture::new();
    store.set_service_config("default/svc1", Some(make_config(3, 3, 30, 300, 0)));

    store.record_failure("default/svc1", 0, FailureClass::Gateway);
    store.record_failure("default/svc1", 0, FailureClass::Gateway);
    store.record_success("default/svc1", 0);
    store.record_failure("default/svc1", 0, FailureClass::Gateway);
    store.record_failure("default/svc1", 0, FailureClass::Gateway);

    assert!(!store.is_ejected("default/svc1", 0));
}

#[test]
fn test_auto_recovery_after_cooldown() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        let store = Fixture::new();
        store.set_service_config("default/svc1", Some(make_config(1, 1, 1, 300, 0)));
        store.record_failure("default/svc1", 0, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc1", 0));
        assert_gauge(&handle, "default/svc1", 1);
        store.expire_runtime("default/svc1", 0);
        assert!(!store.is_ejected("default/svc1", 0));
        assert_gauge(&handle, "default/svc1", 0);
        assert!(!store.is_ejected("default/svc1", 0));
        assert_gauge(&handle, "default/svc1", 0);
        assert!(!store.endpoint("default/svc1", 0).is_open());
        assert_eq!(store.endpoint("default/svc1", 0).num_ejections(), 1);
    });
}

#[test]
fn test_max_ejection_percent() {
    let store = Fixture::new();
    store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 25)));

    store.record_failure("default/svc1", 0, FailureClass::Gateway);
    assert!(store.is_ejected("default/svc1", 0));

    store.record_failure("default/svc1", 1, FailureClass::Gateway);
    assert!(!store.is_ejected("default/svc1", 1));
}

#[test]
fn percent_arbitration_runs_only_for_ejection_candidates() {
    let store = Fixture::new();
    store.set_service_config("default/svc1", Some(make_config(3, 3, 30, 300, 50)));
    let service = Arc::clone(store.services.get("default/svc1").unwrap().value());

    store.record_failure("default/svc1", 0, FailureClass::Gateway);
    store.record_failure("default/svc1", 0, FailureClass::Gateway);
    assert_eq!(service.percent_arbitrations.load(Ordering::Relaxed), 0);

    store.record_failure("default/svc1", 0, FailureClass::Gateway);
    assert_eq!(service.percent_arbitrations.load(Ordering::Relaxed), 1);
    assert!(store.is_ejected("default/svc1", 0));

    store.record_failure("default/svc1", 0, FailureClass::Gateway);
    assert_eq!(service.percent_arbitrations.load(Ordering::Relaxed), 1);

    store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 0)));
    store.record_failure("default/svc1", 0, FailureClass::Gateway);
    assert!(store.is_ejected("default/svc1", 0));
    assert_eq!(
        store
            .services
            .get("default/svc1")
            .unwrap()
            .percent_arbitrations
            .load(Ordering::Relaxed),
        0
    );
}

#[test]
fn test_max_ejection_percent_concurrent_no_over_eject() {
    use std::sync::{Arc, Barrier};

    // 4 backends, 50% limit → at most 2 may be ejected regardless of concurrency.
    let store = Arc::new(Fixture::new());
    store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 50)));

    let barrier = Arc::new(Barrier::new(4));
    let indices = [0usize, 1, 2, 3];

    let handles: Vec<_> = indices
        .iter()
        .copied()
        .map(|index| {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                store.record_failure("default/svc1", index, FailureClass::Gateway);
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    let ejected = indices.iter().filter(|&&h| store.is_ejected("default/svc1", h)).count();
    assert!(
        ejected == 2,
        "over-ejected: {ejected} backends ejected, limit is 2 (50% of 4)"
    );
}

#[test]
fn test_remove_service_clears_state() {
    let store = Fixture::new();
    store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 0)));
    store.record_failure("default/svc1", 0, FailureClass::Gateway);
    assert!(store.is_ejected("default/svc1", 0));

    store.remove_service("default/svc1");
    assert!(!store.is_ejected("default/svc1", 0));
}

#[test]
fn test_record_success_on_ejected_does_not_leak_gauge() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        let store = Fixture::new();
        store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 0)));
        store.record_failure("default/svc1", 0, FailureClass::Gateway);
        assert!(store.is_ejected("default/svc1", 0));
        assert_gauge(&handle, "default/svc1", 1);
        store.record_success("default/svc1", 0);
        assert!(!store.is_ejected("default/svc1", 0));
        assert_eq!(store.endpoint("default/svc1", 0).num_ejections(), 1);
        assert_gauge(&handle, "default/svc1", 0);
        store.record_success("default/svc1", 0);
        assert_gauge(&handle, "default/svc1", 0);
    });
}

#[test]
fn test_percent_cap_applies_concurrently_when_total_endpoints_is_zero() {
    // Late concurrent completions against empty membership cannot recreate state.
    let store = Arc::new(Fixture::new());
    store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 50)));
    let old: Vec<_> = (0..4).map(|index| store.backend("default/svc1", index)).collect();
    let token = store.capture_completion_token(&old[0]).unwrap();
    let slices = store.slices.clone();
    store.coordinator.replace_endpoint_slices(&HashMap::new());
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let handles: Vec<_> = old
        .iter()
        .map(|backend| {
            let store = Arc::clone(&store);
            let backend = Arc::clone(backend);
            let token = token.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                store
                    .store
                    .record_failure_for_backend_in(&store.coordinator, &backend, &token, FailureClass::Gateway);
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert!(old
        .iter()
        .all(|backend| !store.has_runtime_identity(backend.runtime_identity().unwrap())));
    store.coordinator.replace_endpoint_slices(&slices);
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let handles: Vec<_> = (0..4)
        .map(|index| {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                store.record_failure("default/svc1", index, FailureClass::Gateway);
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(
        (0..4).filter(|&index| store.is_ejected("default/svc1", index)).count(),
        2
    );
}

#[test]
fn test_percent_cap_applies_when_total_endpoints_is_zero() {
    // The obsolete caller-zero fallback is replaced by membership rejection.
    let store = Fixture::new();
    store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 50)));
    let old = store.backend("default/svc1", 0);
    let token = store.capture_completion_token(&old).unwrap();
    let slices = store.slices.clone();
    store.coordinator.replace_endpoint_slices(&HashMap::new());
    store
        .store
        .record_failure_for_backend_in(&store.coordinator, &old, &token, FailureClass::Gateway);
    assert!(!store.has_runtime_identity(old.runtime_identity().unwrap()));
    // Restore actual membership; coordinator count, never caller count, caps opens.
    store.coordinator.replace_endpoint_slices(&slices);
    for index in 0..4 {
        store.record_failure("default/svc1", index, FailureClass::Gateway);
    }
    assert_eq!(
        (0..4).filter(|&index| store.is_ejected("default/svc1", index)).count(),
        2
    );
}

#[test]
fn test_config_change_resets_state_unchanged_preserves() {
    // Serialized config publication must preserve these semantics: a *changed* config
    // rebuilds the service (clearing ejection state), while an *unchanged* config
    // is a no-op that keeps the existing ejection state.
    let store = Fixture::new();
    store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 0)));
    store.record_failure("default/svc1", 0, FailureClass::Gateway);
    assert!(store.is_ejected("default/svc1", 0));

    // Re-applying the identical config must not wipe the ejection state.
    store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 0)));
    assert!(store.is_ejected("default/svc1", 0));

    // Applying a different config rebuilds the service with empty endpoints.
    store.set_service_config("default/svc1", Some(make_config(5, 5, 30, 300, 0)));
    assert!(!store.is_ejected("default/svc1", 0));
}

#[test]
fn test_concurrent_config_update_no_panic_and_key_present() {
    // Concurrent set_service_config calls for the same key must never panic and
    // must never leave the key absent. After the storm, one config must be installed.
    use std::sync::{Arc, Barrier};

    let store = Arc::new(Fixture::new());
    store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 0)));

    let barrier = Arc::new(Barrier::new(4));
    let handles: Vec<_> = (0..4u32)
        .map(|i| {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                // Each writer applies a distinct config to force the changed-path swap.
                store.set_service_config("default/svc1", Some(make_config(i + 2, i + 2, 30, 300, 0)));
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    assert!(store.has_service("default/svc1"));
}

#[test]
fn test_different_services_isolated() {
    let store = Fixture::new();
    store.set_service_config("default/svc1", Some(make_config(1, 1, 30, 300, 0)));
    store.set_service_config("default/svc2", Some(make_config(1, 1, 30, 300, 0)));

    store.record_failure("default/svc1", 0, FailureClass::Gateway);
    assert!(store.is_ejected("default/svc1", 0));
    assert!(!store.is_ejected("default/svc2", 0));
}

#[test]
fn app_and_gateway_counters_are_independent() {
    let store = Fixture::new();
    // app threshold 3, gateway threshold 10.
    store.set_service_config("default/svc", Some(make_config(3, 10, 30, 300, 0)));

    // 3 application failures eject via counter A even though gateway is far from 10.
    for _ in 0..3 {
        store.record_failure("default/svc", 0, FailureClass::Application);
    }
    assert!(store.is_ejected("default/svc", 0));
}

#[test]
fn gateway_counter_does_not_trip_app_threshold() {
    let store = Fixture::new();
    // app threshold 3, gateway threshold 10.
    store.set_service_config("default/svc", Some(make_config(3, 10, 30, 300, 0)));
    // 3 gateway failures must NOT eject (gateway threshold is 10).
    for _ in 0..3 {
        store.record_failure("default/svc", 0, FailureClass::Gateway);
    }
    assert!(!store.is_ejected("default/svc", 0));
}

#[test]
fn success_resets_both_counters() {
    let store = Fixture::new();
    store.set_service_config("default/svc", Some(make_config(3, 3, 30, 300, 0)));
    store.record_failure("default/svc", 0, FailureClass::Application);
    store.record_failure("default/svc", 0, FailureClass::Gateway);
    store.record_success("default/svc", 0);
    // After reset, two more app failures still below threshold 3.
    store.record_failure("default/svc", 0, FailureClass::Application);
    store.record_failure("default/svc", 0, FailureClass::Application);
    assert!(!store.is_ejected("default/svc", 0));
}

#[test]
fn local_origin_counter_is_independent_of_gateway() {
    let store = Fixture::new();
    // gateway threshold 10, local-origin threshold 3.
    store.set_service_config("default/svc", Some(make_config_lo(10, 3)));

    // 3 local-origin failures eject via the local-origin counter even though the
    // gateway counter is far from 10.
    for _ in 0..3 {
        store.record_failure("default/svc", 0, FailureClass::LocalOrigin);
    }
    assert!(store.is_ejected("default/svc", 0));
}

#[test]
fn gateway_failures_do_not_trip_local_origin_threshold() {
    let store = Fixture::new();
    // gateway threshold 10, local-origin threshold 2: 2 gateway failures must NOT
    // eject through the (lower) local-origin counter — the buckets are separate.
    store.set_service_config("default/svc", Some(make_config_lo(10, 2)));
    for _ in 0..2 {
        store.record_failure("default/svc", 0, FailureClass::Gateway);
    }
    assert!(!store.is_ejected("default/svc", 0));
}

#[test]
fn success_resets_local_origin_counter() {
    let store = Fixture::new();
    store.set_service_config("default/svc", Some(make_config_lo(10, 3)));
    store.record_failure("default/svc", 0, FailureClass::LocalOrigin);
    store.record_failure("default/svc", 0, FailureClass::LocalOrigin);
    store.record_success("default/svc", 0);
    // After reset, two more local-origin failures still below threshold 3.
    store.record_failure("default/svc", 0, FailureClass::LocalOrigin);
    store.record_failure("default/svc", 0, FailureClass::LocalOrigin);
    assert!(!store.is_ejected("default/svc", 0));
}

#[test]
fn failure_class_metric_labels_are_bounded() {
    assert_eq!(FailureClass::Gateway.metric_label(), "gateway");
    assert_eq!(FailureClass::Application.metric_label(), "application");
    assert_eq!(FailureClass::LocalOrigin.metric_label(), "local_origin");
}

#[test]
fn backoff_doubles_second_ejection_and_persists_num_ejections() {
    let store = Fixture::new();
    // base 1s, consecutive 1 so a single failure ejects.
    store.set_service_config("default/svc", Some(make_config(1, 1, 1, 300, 0)));

    store.record_failure("default/svc", 0, FailureClass::Gateway);
    assert!(store.is_ejected("default/svc", 0));

    // Expiring the first window recovers lazily and retains num_ejections = 1.
    let service = store.services.get("default/svc").expect("service state must exist");
    let endpoint = store.endpoint("default/svc", 0);
    assert_eq!(endpoint.num_ejections(), 1);
    store.expire_runtime("default/svc", 0);
    drop(endpoint);
    drop(service);
    assert!(!store.is_ejected("default/svc", 0)); // triggers reset-but-retain

    // Eject again -> num_ejections becomes 2 -> duration ~2s.
    store.record_failure("default/svc", 0, FailureClass::Gateway);
    assert!(store.is_ejected("default/svc", 0));
    let service = store.services.get("default/svc").expect("service state must exist");
    let endpoint = store.endpoint("default/svc", 0);
    assert_eq!(endpoint.num_ejections(), 2);
    assert_eq!(
        ejection_duration(
            endpoint.config().expect("endpoint config must exist"),
            endpoint.num_ejections(),
        ),
        std::time::Duration::from_secs(2)
    );
    drop(endpoint);
    drop(service);
    assert!(store.is_ejected("default/svc", 0));
}

#[test]
fn max_ejection_seconds_caps_backoff() {
    // Direct test of the backoff duration helper: linear base*num_ejections,
    // clamped at max_ejection_seconds. base=10s, cap=25s.
    let cfg = make_config(1, 1, 10, 25, 0);
    assert_eq!(ejection_duration(&cfg, 1), std::time::Duration::from_secs(10));
    assert_eq!(ejection_duration(&cfg, 2), std::time::Duration::from_secs(20));
    // 3*10=30 exceeds the 25s cap -> clamped to 25.
    assert_eq!(ejection_duration(&cfg, 3), std::time::Duration::from_secs(25));
    // Far past the cap stays clamped (and does not overflow).
    assert_eq!(ejection_duration(&cfg, u32::MAX), std::time::Duration::from_secs(25));
}

#[test]
fn change_detection_rebuilds_on_gateway_threshold_change() {
    let store = Fixture::new();
    store.set_service_config("default/svc", Some(make_config(5, 5, 30, 300, 0)));
    store.record_failure("default/svc", 0, FailureClass::Gateway); // G=1
                                                                   // Changing ONLY the gateway threshold must rebuild (clearing the counter).
    store.set_service_config("default/svc", Some(make_config(5, 2, 30, 300, 0)));
    // Counter was cleared by rebuild: needs 2 fresh gateway failures to eject.
    store.record_failure("default/svc", 0, FailureClass::Gateway);
    assert!(!store.is_ejected("default/svc", 0));
    store.record_failure("default/svc", 0, FailureClass::Gateway);
    assert!(store.is_ejected("default/svc", 0));
}

#[test]
fn concurrent_same_backend_failures_eject_once() {
    use std::sync::{Arc, Barrier};

    // consecutive 1 so a single failure trips the threshold; percent cap off
    // (0) so the percent guard cannot mask the idempotency check.
    let store = Arc::new(Fixture::new());
    store.set_service_config("default/svc", Some(make_config(1, 1, 30, 300, 0)));

    let barrier = Arc::new(Barrier::new(4));
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                store.record_failure("default/svc", 0, FailureClass::Gateway);
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    assert!(store.is_ejected("default/svc", 0));
    // Despite up to 4 concurrent failures crossing the threshold before any
    // ejection lands, the backend is ejected exactly once: num_ejections must
    // be 1 (regression for the step-3 idempotency guard — without it the race
    // would inflate num_ejections and leak the ejected-endpoints gauge).
    let ep = store.endpoint("default/svc", 0);
    assert_eq!(
        ep.num_ejections(),
        1,
        "concurrent same-backend ejection must count once"
    );
}

fn assert_gauge(handle: &metrics_exporter_prometheus::PrometheusHandle, service: &str, value: u32) {
    let expected = format!("edgion_circuit_breaker_ejected_endpoints{{service=\"{service}\"}} {value}");
    assert!(
        handle.render().lines().any(|line| line == expected),
        "{}",
        handle.render()
    );
}

#[test]
fn completion_expiry_clears_runtime_before_new_candidate() {
    let store = Fixture::new();
    store.set_service_config("default/svc1", Some(make_config(2, 2, 30, 300, 50)));
    for _ in 0..2 {
        store.record_failure("default/svc1", 0, FailureClass::Gateway);
    }
    assert!(store.is_ejected("default/svc1", 0));
    store.endpoint("default/svc1", 0).expire_deadlines_for_test();
    // Completion expiry resets the old counters; this is only failure one.
    store.record_failure("default/svc1", 0, FailureClass::Gateway);
    assert!(!store.is_ejected("default/svc1", 0));
    assert_eq!(store.endpoint("default/svc1", 0).num_ejections(), 1);
    store.record_failure("default/svc1", 0, FailureClass::Gateway);
    assert!(store.is_ejected("default/svc1", 0));
    assert_eq!(store.endpoint("default/svc1", 0).num_ejections(), 2);
}

#[test]
fn policy_replacement_and_retirement_settle_gauge_once_and_reject_late_completion() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        let store = Fixture::new();
        let config = make_config(1, 1, 30, 300, 50);
        store.set_service_config("default/svc1", Some(config.clone()));
        let backend = store.backend("default/svc1", 0);
        let token = store.capture_completion_token(&backend).unwrap();
        store.record_failure("default/svc1", 0, FailureClass::Gateway);
        assert_gauge(&handle, "default/svc1", 1);
        store.set_service_config("default/svc1", Some(config));
        assert_gauge(&handle, "default/svc1", 1);
        store.set_service_config("default/svc1", Some(make_config(2, 2, 30, 300, 50)));
        assert_gauge(&handle, "default/svc1", 0);
        store
            .store
            .record_failure_for_backend_in(&store.coordinator, &backend, &token, FailureClass::Gateway);
        store
            .store
            .record_success_for_backend_in(&store.coordinator, &backend, &token);
        assert!(!store.has_runtime_identity(backend.runtime_identity().unwrap()));
        assert_gauge(&handle, "default/svc1", 0);
        for _ in 0..2 {
            store.record_failure("default/svc1", 0, FailureClass::Gateway);
        }
        let current_token = store.capture_completion_token(&backend).unwrap();
        let detached = Arc::clone(
            &store
                .coordinator
                .runtime_cell("default/svc1")
                .unwrap()
                .active
                .load_full()
                .entries[backend.runtime_identity().unwrap().as_ref()],
        );
        assert_gauge(&handle, "default/svc1", 1);
        let mut replacement = store.slices.clone();
        let slice = Arc::make_mut(replacement.get_mut("default/svc1").unwrap());
        slice.metadata.uid = Some("replacement-slice".into());
        store.coordinator.replace_endpoint_slices(&replacement);
        // Activation's production cleanup targets the global store. Mirror only
        // that removal into this isolated store, after real runtime retirement.
        store.remove_runtime_identity(backend.runtime_identity().unwrap());
        assert_gauge(&handle, "default/svc1", 0);
        store
            .store
            .record_failure_for_backend_in(&store.coordinator, &backend, &current_token, FailureClass::Gateway);
        store
            .store
            .record_success_for_backend_in(&store.coordinator, &backend, &current_token);
        assert!(!store.has_runtime_identity(backend.runtime_identity().unwrap()));
        assert!(detached.outlier_ejected());
        assert!(!store.is_ejected("default/svc1", 0));
        store.set_service_config("default/svc1", None);
        store.set_service_config("default/svc1", None);
        assert_gauge(&handle, "default/svc1", 0);
    });
}
