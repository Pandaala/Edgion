use super::*;
use crate::cli::config::{BackendRequestsLabels, RequestDurationConfig};
use metrics_util::debugging::{DebuggingRecorder, Snapshotter};

#[test]
fn ps_index_packs_protocol_and_status() {
    assert_eq!(ps_index(ProtocolIdx::Http, StatusIdx::S2xx), 0);
    assert_eq!(ps_index(ProtocolIdx::Http, StatusIdx::Failed), 4);
    assert_eq!(ps_index(ProtocolIdx::Grpc, StatusIdx::S2xx), 5);
    assert_eq!(ps_index(ProtocolIdx::Websocket, StatusIdx::Failed), 14);
}

#[test]
fn protocol_to_idx_accepts_known_values() {
    assert_eq!(protocol_to_idx("http"), Some(ProtocolIdx::Http));
    assert_eq!(protocol_to_idx("grpc"), Some(ProtocolIdx::Grpc));
    assert_eq!(protocol_to_idx("websocket"), Some(ProtocolIdx::Websocket));
}

#[test]
fn protocol_to_idx_rejects_unknown_values() {
    assert_eq!(protocol_to_idx(""), None);
    assert_eq!(protocol_to_idx("HTTP"), None);
    assert_eq!(protocol_to_idx("kafka"), None);
}

#[test]
fn status_to_idx_groups_codes() {
    assert_eq!(status_to_idx(Some(200)), StatusIdx::S2xx);
    assert_eq!(status_to_idx(Some(204)), StatusIdx::S2xx);
    assert_eq!(status_to_idx(Some(301)), StatusIdx::S3xx);
    assert_eq!(status_to_idx(Some(404)), StatusIdx::S4xx);
    assert_eq!(status_to_idx(Some(503)), StatusIdx::S5xx);
    assert_eq!(status_to_idx(Some(199)), StatusIdx::Failed);
    assert_eq!(status_to_idx(Some(600)), StatusIdx::Failed);
    assert_eq!(status_to_idx(None), StatusIdx::Failed);
}

#[test]
fn lookup_misses_on_empty_handles() {
    let h = RouteMetricHandles::empty();
    assert!(h.lookup("ns", "gw").is_none());
}

fn run_with_recorder<F: FnOnce() -> R, R>(f: F) -> (Snapshotter, R) {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let r = metrics::with_local_recorder(&recorder, f);
    (snapshotter, r)
}

#[test]
fn build_registers_one_counter_and_histogram_per_cell() {
    let gws = vec![("gw_ns".to_string(), "gw_a".to_string())];
    let backends = vec![
        ("ns1".to_string(), "svc_a".to_string()),
        ("ns1".to_string(), "svc_b".to_string()),
    ];
    // Enable backend labels so each backend produces a distinct label set;
    // without them both backends collapse to the same N_PS keys.
    let backend_cfg = BackendRequestsLabels {
        backend: true,
        ..BackendRequestsLabels::default()
    };
    let duration_cfg = RequestDurationConfig::default();

    let (snap, handles) = run_with_recorder(|| {
        RouteMetricHandles::build_with(&gws, "rns", "route_a", &backends, &backend_cfg, &duration_cfg)
    });

    // Sanity: lookup hits.
    let entry = handles.lookup("gw_ns", "gw_a").expect("gateway entry registered");
    assert_eq!(entry.counters.len(), 2);
    assert_eq!(entry.counters[0].as_ref().unwrap().len(), N_PS);

    // Cardinality: 1 gw x 2 backends x 15 ps = 30 unique counter keys.
    // DebuggingRecorder deduplicates by key, so the snapshot size reflects unique registrations.
    let snapshot = snap.snapshot();
    let names: Vec<String> = snapshot
        .into_vec()
        .into_iter()
        .map(|(key, _, _, _)| key.key().name().to_string())
        .collect();
    let n_counter = names.iter().filter(|n| n.contains("backend_requests_total")).count();
    let n_hist = names.iter().filter(|n| n.contains("request_duration_ms")).count();
    let expected = 2 * N_PS;
    assert_eq!(n_counter, expected, "expected {} counters, got {}", expected, n_counter);
    let expected_hist = if duration_cfg.enabled { expected } else { 0 };
    assert_eq!(
        n_hist, expected_hist,
        "expected {} histograms, got {}",
        expected_hist, n_hist
    );
}

#[test]
fn build_registers_histograms_when_duration_enabled() {
    let gws = vec![("gw_ns".to_string(), "gw_a".to_string())];
    let backends = vec![("ns1".to_string(), "svc_a".to_string())];
    let backend_cfg = BackendRequestsLabels {
        backend: true,
        ..Default::default()
    };
    let duration_cfg = RequestDurationConfig {
        enabled: true,
        ..Default::default()
    };

    let (snap, handles) = run_with_recorder(|| {
        RouteMetricHandles::build_with(&gws, "rns", "route_a", &backends, &backend_cfg, &duration_cfg)
    });

    let entry = handles.lookup("gw_ns", "gw_a").unwrap();
    assert!(entry.histograms.is_some());
    assert_eq!(entry.histograms.as_ref().unwrap().len(), 1);

    let snapshot = snap.snapshot();
    let names: Vec<String> = snapshot
        .into_vec()
        .into_iter()
        .map(|(key, _, _, _)| key.key().name().to_string())
        .collect();
    let n_hist = names.iter().filter(|n| n.contains("request_duration_ms")).count();
    assert_eq!(n_hist, N_PS);
}

#[test]
fn record_succeeds_for_in_bounds_backend_index() {
    let gws = vec![("gw_ns".to_string(), "gw_a".to_string())];
    let backends = vec![("ns1".to_string(), "svc_a".to_string())];
    let backend_cfg = BackendRequestsLabels {
        backend: true,
        ..Default::default()
    };
    let duration_cfg = RequestDurationConfig::default();

    let (_, ok) = run_with_recorder(|| {
        let handles = RouteMetricHandles::build_with(&gws, "rns", "route_a", &backends, &backend_cfg, &duration_cfg);
        let entry = handles.lookup("gw_ns", "gw_a").unwrap();
        entry.record(0, ps_index(ProtocolIdx::Http, StatusIdx::S2xx), 1.0)
    });
    assert!(ok, "in-bounds backend index must record successfully");
}

#[test]
fn record_falls_back_on_out_of_bounds_backend_index() {
    let gws = vec![("gw_ns".to_string(), "gw_a".to_string())];
    // A single backend → only index 0 is valid; index 1 is out of bounds.
    let backends = vec![("ns1".to_string(), "svc_a".to_string())];
    let backend_cfg = BackendRequestsLabels {
        backend: true,
        ..Default::default()
    };
    let duration_cfg = RequestDurationConfig::default();

    let (_, ok) = run_with_recorder(|| {
        let handles = RouteMetricHandles::build_with(&gws, "rns", "route_a", &backends, &backend_cfg, &duration_cfg);
        let entry = handles.lookup("gw_ns", "gw_a").unwrap();
        // Out-of-bounds index must NOT panic and must return false.
        entry.record(1, ps_index(ProtocolIdx::Http, StatusIdx::S2xx), 1.0)
    });

    assert!(!ok, "out-of-bounds backend index must return false (graceful fallback)");
}

#[test]
fn record_falls_back_on_out_of_bounds_protocol_status_index() {
    let gws = vec![("gw_ns".to_string(), "gw_a".to_string())];
    let backends = vec![("ns1".to_string(), "svc_a".to_string())];
    let backend_cfg = BackendRequestsLabels::default();
    let duration_cfg = RequestDurationConfig::default();

    let (_, ok) = run_with_recorder(|| {
        let handles = RouteMetricHandles::build_with(&gws, "rns", "route_a", &backends, &backend_cfg, &duration_cfg);
        let entry = handles.lookup("gw_ns", "gw_a").unwrap();
        entry.record(0, N_PS, 1.0)
    });

    assert!(!ok, "out-of-bounds protocol/status index must return false");
}

#[test]
fn record_validates_histogram_slot_before_incrementing_counter() {
    let gws = vec![("gw_ns".to_string(), "gw_a".to_string())];
    let backends = vec![("ns1".to_string(), "svc_a".to_string())];
    let backend_cfg = BackendRequestsLabels::default();
    let duration_cfg = RequestDurationConfig {
        enabled: true,
        ..Default::default()
    };
    let cell = ps_index(ProtocolIdx::Http, StatusIdx::S2xx);

    let (snap, ok) = run_with_recorder(|| {
        let mut handles =
            RouteMetricHandles::build_with(&gws, "rns", "route_a", &backends, &backend_cfg, &duration_cfg);
        let entry = handles.matrices.first_mut().unwrap();
        entry.histograms.as_mut().unwrap().clear();
        entry.record(0, cell, 1.0)
    });

    assert!(!ok, "an incomplete histogram matrix must use the dynamic fallback");
    let counter_value = snap
        .snapshot()
        .into_vec()
        .into_iter()
        .find_map(|(key, _, _, value)| {
            let is_target = key.key().name() == names::BACKEND_REQUESTS_TOTAL
                && key
                    .key()
                    .labels()
                    .any(|label| label.key() == "protocol" && label.value() == "http")
                && key
                    .key()
                    .labels()
                    .any(|label| label.key() == "status" && label.value() == "2xx");
            is_target.then_some(value)
        })
        .expect("target counter registered");
    assert!(
        matches!(counter_value, metrics_util::debugging::DebugValue::Counter(0)),
        "counter must remain untouched before dynamic fallback: {counter_value:?}"
    );
}

#[test]
fn sparse_build_preserves_raw_slots_without_registering_non_service_metrics() {
    let gws = vec![("gw_ns".to_string(), "gw_a".to_string())];
    let backends = vec![None, Some(("ns1".to_string(), "svc_a".to_string()))];
    let backend_cfg = BackendRequestsLabels {
        backend: true,
        ..Default::default()
    };
    let duration_cfg = RequestDurationConfig::default();

    let (snap, (invalid_slot, service_slot)) = run_with_recorder(|| {
        let handles =
            RouteMetricHandles::build_sparse_with(&gws, "rns", "route_a", &backends, &backend_cfg, &duration_cfg);
        let entry = handles.lookup("gw_ns", "gw_a").unwrap();
        (
            entry.record(0, ps_index(ProtocolIdx::Http, StatusIdx::S2xx), 1.0),
            entry.record(1, ps_index(ProtocolIdx::Http, StatusIdx::S2xx), 1.0),
        )
    });

    assert!(!invalid_slot);
    assert!(service_slot);
    let registered = snap
        .snapshot()
        .into_vec()
        .into_iter()
        .filter(|(key, _, _, _)| key.key().name().contains("backend_requests_total"))
        .count();
    assert_eq!(registered, N_PS);
}

#[test]
fn handle_path_emits_same_labels_as_fallback() {
    use crate::cli::config::{BackendRequestsLabels, RequestDurationConfig};
    use crate::observe::metrics::registry::record_backend_request;

    let gws = vec![("gw_ns".to_string(), "gw_a".to_string())];
    let backends = vec![("ns1".to_string(), "svc_a".to_string())];
    // Need backend label on so two emitters produce identical full label sets
    // (same shape as build_registers_one_counter_and_histogram_per_cell).
    let backend_cfg = BackendRequestsLabels {
        backend: true,
        ..Default::default()
    };
    let duration_cfg = RequestDurationConfig::default();

    let (snap, _) = run_with_recorder(|| {
        let handles = RouteMetricHandles::build_with(&gws, "rns", "route_a", &backends, &backend_cfg, &duration_cfg);
        // Drive both paths once for the same (gw, route, backend, protocol, status):
        let entry = handles.lookup("gw_ns", "gw_a").unwrap();
        entry.counters[0].as_ref().unwrap()[ps_index(ProtocolIdx::Http, StatusIdx::S2xx)].increment(1);
        // Fallback emits with the same input shape:
        record_backend_request(
            &backend_cfg,
            "gw_ns",
            "gw_a",
            "rns",
            "route_a",
            "ns1",
            "svc_a",
            "http",
            "2xx",
            "",
            "",
        );
    });

    // Both should land in the SAME time series — collect by key.
    // Only count non-zero entries: build_with pre-registers all N_PS cells in the
    // recorder (value = 0); we only want the cells that were actually incremented.
    let snapshot = snap.snapshot().into_vec();
    let counter_entries: Vec<_> = snapshot
        .iter()
        .filter(|(k, _, _, v)| {
            k.key().name() == crate::observe::metrics::registry::names::BACKEND_REQUESTS_TOTAL
                && matches!(v, metrics_util::debugging::DebugValue::Counter(n) if *n > 0)
        })
        .collect();
    // Exactly one unique key with the same label set; value = 2.
    assert_eq!(
        counter_entries.len(),
        1,
        "handle path and fallback registered different keys: {:?}",
        counter_entries
            .iter()
            .map(|(k, _, _, _)| k.key().labels().collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );
}

#[test]
fn unchanged_matrix_renews_after_idle_without_spending_more_keys() {
    use edgion_common::observe::{
        cardinality_guard::CardinalityMode,
        metrics_api::{RecorderConfig, TestRecorder},
    };
    let buckets = [(
        metrics_exporter_prometheus::Matcher::Full("edgion_gateway_request_duration_ms".into()),
        vec![1., 10., 100.],
    )];
    let recorder = TestRecorder::with_clock(RecorderConfig {
        service: "test",
        idle_timeout_secs: 1,
        histogram_buckets: &buckets,
        cardinality_cap: 100,
        cardinality_mode: CardinalityMode::Enforce,
    })
    .unwrap();
    let handles = recorder.with(|| {
        RouteMetricHandles::build_with(
            &[("ns".into(), "gw".into())],
            "ns",
            "route",
            &[("ns".into(), "backend".into())],
            &BackendRequestsLabels::default(),
            &RequestDurationConfig {
                enabled: true,
                ..Default::default()
            },
        )
    });
    assert_eq!(recorder.stats().admission_count, 2);
    assert_eq!(recorder.stats().live_epochs, 0);
    assert!(
        !recorder.render().contains("edgion_gateway_request_duration_ms"),
        "preparation must not create synthetic histogram samples"
    );
    let entry = handles.lookup("ns", "gw").unwrap();
    assert!(entry.record(0, ps_index(ProtocolIdx::Http, StatusIdx::S2xx), 5.));
    let admitted = recorder.stats().admission_count;
    assert_eq!(recorder.stats().live_epochs, 2);
    assert!(entry.record(0, ps_index(ProtocolIdx::Http, StatusIdx::S2xx), 50.));
    let before = recorder.render();
    assert!(before
        .lines()
        .any(|line| line.starts_with("edgion_gateway_request_duration_ms_count{") && line.ends_with(" 2")));
    assert!(before
        .lines()
        .any(|line| line.starts_with("edgion_gateway_request_duration_ms_sum{") && line.ends_with(" 55")));
    recorder.advance(std::time::Duration::from_secs(2));
    assert!(!recorder.render().contains("edgion_backend_requests_total"));
    assert_eq!(recorder.stats().live_epochs, 0);
    assert!(!recorder.render().contains("edgion_gateway_request_duration_ms"));
    assert!(entry.record(0, ps_index(ProtocolIdx::Http, StatusIdx::S2xx), 5.));
    assert_eq!(recorder.stats().admission_count, admitted);
    assert_eq!(recorder.stats().live_epochs, 2);
    assert!(recorder
        .render()
        .lines()
        .any(|line| line.starts_with("edgion_backend_requests_total{") && line.ends_with(" 1")));
    let renewed = recorder.render();
    assert!(renewed
        .lines()
        .any(|line| line.starts_with("edgion_gateway_request_duration_ms_count{") && line.ends_with(" 1")));
    assert!(renewed
        .lines()
        .any(|line| line.starts_with("edgion_gateway_request_duration_ms_sum{") && line.ends_with(" 5")));
    for (bound, count) in [("1", 0), ("10", 1), ("100", 1), ("+Inf", 1)] {
        assert!(
            renewed
                .lines()
                .any(|line| line.starts_with("edgion_gateway_request_duration_ms_bucket{")
                    && line.contains(&format!("le=\"{bound}\""))
                    && line.ends_with(&format!(" {count}"))),
            "fresh epoch must contain exactly the one real sample: {renewed}"
        );
    }
}
