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

use crate::core::gateway::cli::config::MetricsConfig;

/// Global Prometheus metrics handle
static PROMETHEUS_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Initialize the Prometheus metrics exporter with configuration
pub fn init_metrics_exporter(cfg: &MetricsConfig) -> Result<(), String> {
    let rd = &cfg.request_duration;

    let mut builder = PrometheusBuilder::new().add_global_label("service", "edgion-gateway");

    // idle_timeout: clean up cold time-series
    if rd.idle_timeout_secs > 0 {
        builder = builder.idle_timeout(MetricKindMask::ALL, Some(Duration::from_secs(rd.idle_timeout_secs)));
    }

    // Custom bucket boundaries for request duration histogram
    if rd.enabled && !rd.buckets.is_empty() {
        builder = builder
            .set_buckets_for_metric(
                Matcher::Full(
                    crate::core::gateway::observe::metrics::registry::names::GATEWAY_REQUEST_DURATION_MS.to_string(),
                ),
                &rd.buckets,
            )
            .map_err(|e| format!("Failed to set buckets: {}", e))?;
    }

    // Build and install the recorder
    let handle = builder
        .install_recorder()
        .map_err(|e| format!("Failed to install Prometheus recorder: {}", e))?;

    PROMETHEUS_HANDLE
        .set(handle)
        .map_err(|_| "Prometheus handle already initialized".to_string())?;

    tracing::info!(
        component = "metrics",
        event = "exporter_initialized",
        idle_timeout_secs = rd.idle_timeout_secs,
        histogram_enabled = rd.enabled,
        "Prometheus metrics exporter initialized"
    );

    Ok(())
}

/// Get the Prometheus handle
fn get_prometheus_handle() -> Option<&'static PrometheusHandle> {
    PROMETHEUS_HANDLE.get()
}

/// Metrics handler - returns Prometheus formatted metrics
async fn metrics_handler() -> Response {
    match get_prometheus_handle() {
        Some(handle) => {
            let metrics = handle.render();
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
                metrics,
            )
                .into_response()
        }
        None => (StatusCode::INTERNAL_SERVER_ERROR, "Metrics exporter not initialized").into_response(),
    }
}

/// Health check for metrics endpoint
async fn health_handler() -> &'static str {
    "OK"
}

/// Create the metrics API router
pub fn create_metrics_router() -> Router {
    Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/health", get(health_handler))
}

/// Serve the metrics API on the specified port
pub async fn serve(cfg: &MetricsConfig) -> anyhow::Result<()> {
    // Initialize metrics exporter if not already done
    if PROMETHEUS_HANDLE.get().is_none() {
        init_metrics_exporter(cfg).map_err(|e| anyhow::anyhow!("Failed to initialize metrics exporter: {}", e))?;
    }

    let app = create_metrics_router();
    let addr: std::net::SocketAddr = cfg
        .listen
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid metrics listen address '{}': {}", cfg.listen, e))?;

    tracing::info!(
        component = "metrics_api",
        event = "server_starting",
        addr = %addr,
        "Metrics API server listening"
    );

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Serve the metrics API on the specified port with graceful shutdown support
pub async fn serve_with_shutdown(
    cfg: &MetricsConfig,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    if PROMETHEUS_HANDLE.get().is_none() {
        init_metrics_exporter(cfg).map_err(|e| anyhow::anyhow!("Failed to initialize metrics exporter: {}", e))?;
    }

    let app = create_metrics_router();
    let addr: std::net::SocketAddr = cfg
        .listen
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid metrics listen address '{}': {}", cfg.listen, e))?;

    tracing::info!(
        component = "metrics_api",
        event = "server_starting",
        addr = %addr,
        "Metrics API server listening"
    );

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;

    tracing::info!(component = "metrics_api", "Metrics API server stopped");
    Ok(())
}
