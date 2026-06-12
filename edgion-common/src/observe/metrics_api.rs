//! Shared Prometheus recorder installation and `/metrics` handler.
//!
//! Gateway, Controller, and Center all need to install a process-wide
//! `PrometheusRecorder` and render a `/metrics` page. They differ only in:
//!
//! - how they mount the handler (Gateway runs its own listener; Controller
//!   and Center hang the route off their existing admin router);
//! - whether they customize histogram buckets (Gateway tunes the
//!   `gateway_request_duration_ms` histogram; Controller/Center have no
//!   histograms in scope).
//!
//! This module exposes the recorder install as an idempotent operation
//! guarded by a `OnceLock<PrometheusHandle>`, so subsequent callers in the
//! same process see a `Result::Ok(())` instead of a panic.

use axum::{
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use http::{header, StatusCode};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use metrics_util::MetricKindMask;
use std::sync::OnceLock;
use std::time::Duration;

/// Global Prometheus handle. Set exactly once per process.
static PROMETHEUS_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Configuration for installing the global Prometheus recorder.
///
/// `service` is used as a Prometheus `service=""` global label so metrics
/// from Gateway/Controller/Center can be distinguished in a shared
/// backend. `idle_timeout_secs = 0` disables cold-series eviction.
///
/// `histogram_buckets` is an optional list of `(matcher, buckets)` pairs
/// for tuning individual histograms (Gateway uses this for
/// `gateway_request_duration_ms`). Pass `&[]` when no histograms need
/// custom buckets.
pub struct RecorderConfig<'a> {
    pub service: &'static str,
    pub idle_timeout_secs: u64,
    pub histogram_buckets: &'a [(Matcher, Vec<f64>)],
}

/// Install the global Prometheus recorder.
///
/// Idempotent: the first call does the install; subsequent calls in the
/// same process are no-ops that return `Ok(())` so combined-mode test
/// harnesses (and future in-process topologies) do not panic.
pub fn install_global_recorder(cfg: RecorderConfig<'_>) -> Result<(), String> {
    if PROMETHEUS_HANDLE.get().is_some() {
        return Ok(());
    }

    let mut builder = PrometheusBuilder::new().add_global_label("service", cfg.service);

    if cfg.idle_timeout_secs > 0 {
        builder = builder.idle_timeout(MetricKindMask::ALL, Some(Duration::from_secs(cfg.idle_timeout_secs)));
    }

    for (matcher, buckets) in cfg.histogram_buckets {
        builder = builder
            .set_buckets_for_metric(matcher.clone(), buckets)
            .map_err(|e| format!("Failed to set buckets: {}", e))?;
    }

    let handle = builder
        .install_recorder()
        .map_err(|e| format!("Failed to install Prometheus recorder: {}", e))?;

    // If another thread raced us between the `get()` check above and this
    // point, prefer the first winner and drop our handle silently. This is
    // exceedingly unlikely in the single-threaded bin/cli setup path but
    // keeps the function honestly idempotent.
    let _ = PROMETHEUS_HANDLE.set(handle);

    tracing::info!(
        component = "metrics",
        event = "exporter_initialized",
        service = cfg.service,
        idle_timeout_secs = cfg.idle_timeout_secs,
        "Prometheus metrics exporter initialized"
    );

    Ok(())
}

/// Whether the global Prometheus recorder has been installed.
pub fn recorder_installed() -> bool {
    PROMETHEUS_HANDLE.get().is_some()
}

/// Axum handler that renders the current recorder state as Prometheus
/// text. Returns 500 with a diagnostic body if the recorder was not
/// installed before the server started serving.
pub async fn metrics_handler() -> Response {
    match PROMETHEUS_HANDLE.get() {
        Some(handle) => {
            let body = handle.render();
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
                body,
            )
                .into_response()
        }
        None => (StatusCode::INTERNAL_SERVER_ERROR, "Metrics exporter not initialized").into_response(),
    }
}

/// Liveness probe paired with `/metrics` on Gateway's standalone metrics
/// listener. Controller and Center reuse their existing `/health` route.
pub async fn health_handler() -> &'static str {
    "OK"
}

/// Router that exposes `/metrics` and `/health` on a standalone listener.
/// Used by Gateway; Controller and Center instead mount `metrics_handler`
/// directly on their admin router.
pub fn create_metrics_router() -> Router {
    Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/health", get(health_handler))
}
