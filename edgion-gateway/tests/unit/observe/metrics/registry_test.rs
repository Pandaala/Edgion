use super::*;
use metrics_exporter_prometheus::PrometheusBuilder;

#[test]
fn dropped_log_counts_keep_zero_lazy_and_add_batches() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        let m = GatewayMetrics::new();
        type RecordDrops = fn(&GatewayMetrics, u64);
        let counters: [(RecordDrops, &str); 5] = [
            (GatewayMetrics::access_log_dropped, names::ACCESS_LOG_DROPPED),
            (GatewayMetrics::ssl_log_dropped, names::SSL_LOG_DROPPED),
            (GatewayMetrics::tcp_log_dropped, names::TCP_LOG_DROPPED),
            (GatewayMetrics::tls_log_dropped, names::TLS_LOG_DROPPED),
            (GatewayMetrics::udp_log_dropped, names::UDP_LOG_DROPPED),
        ];
        for (record, name) in counters {
            record(&m, 0);
            assert!(!handle.render().contains(name));
            record(&m, 1000);
            record(&m, 1);
            record(&m, 0);
            assert!(handle.render().lines().any(|line| line == format!("{name} 1001")));
        }
    });
}

pub(super) fn lifetime_recorder() -> edgion_common::observe::metrics_api::TestRecorder {
    use edgion_common::observe::{
        cardinality_guard::CardinalityMode,
        metrics_api::{RecorderConfig, TestRecorder},
    };
    TestRecorder::with_clock(RecorderConfig {
        service: "test",
        idle_timeout_secs: 1,
        histogram_buckets: &[],
        cardinality_cap: 10000,
        cardinality_mode: CardinalityMode::Enforce,
    })
    .unwrap()
}
fn lifetime_render(recorder: &edgion_common::observe::metrics_api::TestRecorder) -> String {
    recorder
        .render()
        .replace("{service=\"test\",", "{")
        .replace("{service=\"test\"}", "")
}
#[test]
fn wasm_module_metrics_follow_overlapping_generation_lifetimes() {
    let recorder = lifetime_recorder();
    let (old, replacement) = recorder.with(|| (wasm_module_metrics("test-module"), wasm_module_metrics("test-module")));
    assert!(lifetime_render(&recorder).contains("edgion_wasm_modules_loaded 2"));
    std::thread::spawn(move || drop(old)).join().unwrap();
    assert!(lifetime_render(&recorder).contains("edgion_wasm_modules_loaded 1"));
    drop(replacement);
    let output = lifetime_render(&recorder);
    assert!(output.contains("edgion_wasm_modules_loaded 0"));
    assert!(output.contains("edgion_wasm_module_released_total{module=\"test-module\"} 2"));
}
#[test]
fn wasm_module_metrics_survive_idle_eviction_across_sources_and_generations() {
    let recorder = lifetime_recorder();
    let (a, b) = recorder.with(|| {
        (
            wasm_module_metrics("idle-runtime-a"),
            wasm_module_metrics("idle-runtime-b"),
        )
    });
    recorder.advance(std::time::Duration::from_secs(2));
    let output = lifetime_render(&recorder);
    assert!(output.contains("edgion_wasm_modules_loaded 2"));
    assert!(!output.contains("edgion_wasm_module_released_total"));
    let replacement = recorder.with(|| wasm_module_metrics("idle-runtime-a"));
    drop(a);
    assert!(lifetime_render(&recorder).contains("edgion_wasm_module_released_total{module=\"idle-runtime-a\"} 1"));
    recorder.advance(std::time::Duration::from_secs(2));
    assert!(!lifetime_render(&recorder).contains("edgion_wasm_module_released_total"));
    drop(b);
    drop(replacement);
    let output = lifetime_render(&recorder);
    assert!(output.contains("edgion_wasm_modules_loaded 0"));
    assert!(output.contains("edgion_wasm_module_released_total{module=\"idle-runtime-a\"} 1"));
    assert_eq!(recorder.stats().gauge_owners, 0);
}

#[test]
fn config_conflict_counter_increments() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        record_config_conflict("outbound_ca");
        let rendered = handle.render();
        assert!(
            rendered.contains("edgion_config_conflict_total") && rendered.contains("field=\"outbound_ca\""),
            "config_conflict counter must render with field label, got:\n{rendered}"
        );
    });
}

#[test]
fn injected_header_skipped_counter_increments() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        record_injected_header_skipped();
        let rendered = handle.render();
        assert!(
            rendered.contains("edgion_outbound_injected_header_skipped_total"),
            "injected_header_skipped counter must render, got:\n{rendered}"
        );
    });
}

/// Verifies the hybrid registration policy:
/// - Hot per-request handles register in `new()` and appear immediately (with value 0).
/// - Cold / failure-path handles stay absent until their record method is called.
#[test]
fn gateway_metrics_registration_policy() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();

    metrics::with_local_recorder(&recorder, || {
        // `new()` must run inside with_local_recorder so eager counter!/gauge!
        // macros bind to the prometheus test recorder (not the noop global).
        let m = GatewayMetrics::new();

        // --- Before any record call ---
        let rendered = handle.render();
        // Hot handles: eagerly registered, should be visible (value 0).
        assert!(
            rendered.contains("edgion_ctx_created_total"),
            "hot counter ctx_created must register eagerly, got:\n{rendered}"
        );
        assert!(
            rendered.contains("edgion_ctx_active"),
            "hot gauge ctx_active must register eagerly, got:\n{rendered}"
        );
        assert!(
            rendered.contains("edgion_gateway_request_bytes_total"),
            "hot counter request_bytes must register eagerly, got:\n{rendered}"
        );
        assert!(
            rendered.contains("edgion_gateway_response_bytes_total"),
            "hot counter response_bytes must register eagerly, got:\n{rendered}"
        );
        // Cold handles: absent until called.
        assert!(
            !rendered.contains("edgion_access_log_dropped_total"),
            "cold counter access_log_dropped must not appear before first call, got:\n{rendered}"
        );
        assert!(
            !rendered.contains("edgion_ssl_log_dropped_total"),
            "cold counter ssl_log_dropped must not appear before first call, got:\n{rendered}"
        );
        assert!(
            !rendered.contains("edgion_controller_schema_validation_errors_total"),
            "cold counter schema_validation_errors must not appear before first call, got:\n{rendered}"
        );

        // --- After touching one cold counter ---
        m.access_log_dropped(1);
        let rendered = handle.render();
        assert!(
            rendered.contains("edgion_access_log_dropped_total"),
            "touched cold counter must appear, got:\n{rendered}"
        );
        assert!(
            !rendered.contains("edgion_ssl_log_dropped_total"),
            "still-untouched cold counter must remain absent, got:\n{rendered}"
        );
    });
}

#[test]
fn context_lifecycle_balances_the_active_gauge() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();

    metrics::with_local_recorder(&recorder, || {
        let metrics = GatewayMetrics::new();
        metrics.ctx_created();
        let release = metrics.active_context_release_guard();
        let rendered = handle.render();
        assert!(rendered.lines().any(|line| line == "edgion_ctx_created_total 1"));
        assert!(rendered.lines().any(|line| line == "edgion_ctx_active 1"));

        drop(release);
        let rendered = handle.render();
        assert!(rendered.lines().any(|line| line == "edgion_ctx_active 0"));
    });
}

#[test]
fn record_backend_request_emits_expected_labels_under_default_cfg() {
    use crate::cli::config::BackendRequestsLabels;
    use metrics_util::debugging::DebuggingRecorder;

    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let cfg = BackendRequestsLabels::default(); // gateway + protocol + status
    metrics::with_local_recorder(&recorder, || {
        record_backend_request(
            &cfg,
            "gw_ns",
            "gw_a",
            "route_ns",
            "route_x",
            "backend_ns",
            "svc_x",
            "http",
            "2xx",
            "",
            "",
        );
    });
    let snap = snapshotter.snapshot().into_vec();
    let key = snap
        .iter()
        .find(|(k, _, _, _)| k.key().name() == names::BACKEND_REQUESTS_TOTAL)
        .expect("counter registered");
    let labels: Vec<(&str, &str)> = key.0.key().labels().map(|l| (l.key(), l.value())).collect();
    assert!(labels.contains(&("gateway_namespace", "gw_ns")));
    assert!(labels.contains(&("gateway_name", "gw_a")));
    assert!(labels.contains(&("protocol", "http")));
    assert!(labels.contains(&("status", "2xx")));
    // route_namespace / backend_namespace must NOT appear under default cfg
    assert!(!labels.iter().any(|(k, _)| *k == "route_namespace"));
    assert!(!labels.iter().any(|(k, _)| *k == "backend_namespace"));
}

#[test]
fn geoip_metric_names_are_declared() {
    assert_eq!(names::GEOIP_DB_LOADED, "edgion_geoip_db_loaded");
    assert_eq!(names::GEOIP_DB_BUILD_EPOCH, "edgion_geoip_db_build_epoch_seconds");
    assert_eq!(names::GEOIP_UPDATE_FAILED, "edgion_geoip_update_failed_total");
    assert_eq!(names::GEOIP_DENIED, "edgion_geoip_denied_total");
    assert_eq!(names::GEOIP_LOOKUP_PANIC, "edgion_geoip_lookup_panic_total");
}

/// Regression test: a lookup panic must land on its own dedicated metric,
/// never on `edgion_geoip_update_failed_total` — that counter's `reason`
/// label set is about the background update cycle, and folding a
/// per-request lookup panic into it would mislabel a per-request event as
/// an updater-cycle failure.
#[test]
fn geoip_lookup_panic_uses_its_own_metric_not_update_failed() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        record_geoip_lookup_panic();
        let rendered = handle.render();
        assert!(
            rendered.contains("edgion_geoip_lookup_panic_total"),
            "lookup panic must render under its own metric, got:\n{rendered}"
        );
        assert!(
            !rendered.contains("edgion_geoip_update_failed_total"),
            "lookup panic must NOT touch edgion_geoip_update_failed_total, got:\n{rendered}"
        );
    });
}

#[test]
fn fips_metric_names_are_stable() {
    // These names are load-bearing: the compliance alert
    //   fips_host_enabled == 1 and fips_module_enabled == 0
    // breaks silently if either is renamed.
    assert_eq!(names::FIPS_MODULE_ENABLED, "edgion_gateway_fips_module_enabled");
    assert_eq!(names::FIPS_HOST_ENABLED, "edgion_gateway_fips_host_enabled");
    assert_eq!(names::FIPS_HOST_DETECT_ERROR, "edgion_gateway_fips_host_detect_error");
    assert_eq!(names::FIPS_POLICY_ACTIVE, "edgion_gateway_fips_policy_active");
}

#[test]
fn genai_metric_disabled_does_not_register_family() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        record_genai_tokens(
            false,
            crate::ai::AiProviderLabel::OpenAi,
            GenAiUsageSource::Reported,
            11,
            7,
        );
    });
    assert!(!handle.render().contains(names::GENAI_TOKENS_TOTAL));
}

#[test]
fn genai_metric_has_exactly_twelve_fixed_series_and_no_model_label() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        for provider in [
            crate::ai::AiProviderLabel::OpenAi,
            crate::ai::AiProviderLabel::OpenAiCompatible,
            crate::ai::AiProviderLabel::Anthropic,
        ] {
            for source in [GenAiUsageSource::Reported, GenAiUsageSource::Estimated] {
                record_genai_tokens(true, provider, source, 1, 2);
            }
        }
    });

    let rendered = handle.render();
    let samples: Vec<&str> = rendered
        .lines()
        .filter(|line| line.starts_with(names::GENAI_TOKENS_TOTAL))
        .collect();
    assert_eq!(
        samples.len(),
        12,
        "fixed label domains must yield twelve series:\n{rendered}"
    );
    assert!(
        !rendered.contains("model="),
        "model must never be a metric label:\n{rendered}"
    );
    for sample in samples {
        assert!(sample.contains("direction=\"input\"") || sample.contains("direction=\"output\""));
        assert!(
            sample.contains("provider=\"openai\"")
                || sample.contains("provider=\"openai_compatible\"")
                || sample.contains("provider=\"anthropic\"")
        );
        assert!(sample.contains("usage_source=\"reported\"") || sample.contains("usage_source=\"estimated\""));
        assert!(
            sample.ends_with(" 1") || sample.ends_with(" 2"),
            "counter must retain integer increments: {sample}"
        );
    }
}

#[test]
fn geo_reference_unavailable_uses_only_existing_bounded_metric_labels() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        record_geoip_denied(StreamLayer::Stage1, "ref_unavailable");
        record_geoip_denied(StreamLayer::Tcp, "ref_unavailable");
    });
    let rendered = handle.render();
    assert!(rendered.contains("edgion_geoip_denied_total{layer=\"stage1\",reason=\"ref_unavailable\"} 1"));
    assert!(rendered.contains("edgion_geoip_denied_total{layer=\"tcp\",reason=\"ref_unavailable\"} 1"));
    assert!(!rendered.contains("resource=") && !rendered.contains("address="));
}
