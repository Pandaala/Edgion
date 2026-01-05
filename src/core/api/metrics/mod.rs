use axum::{
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use http::{header, StatusCode};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use std::sync::OnceLock;

/// Global Prometheus metrics handle
static PROMETHEUS_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Initialize the Prometheus metrics exporter
pub fn init_metrics_exporter() -> Result<(), String> {
    let builder = PrometheusBuilder::new();

    // Set prefix for all metrics
    let builder = builder.add_global_label("service", "edgion-gateway");

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
pub async fn serve(port: u16) -> anyhow::Result<()> {
    // Initialize metrics exporter if not already done
    if PROMETHEUS_HANDLE.get().is_none() {
        init_metrics_exporter().map_err(|e| anyhow::anyhow!("Failed to initialize metrics exporter: {}", e))?;
    }

    let app = create_metrics_router();
    let addr_str = format!("0.0.0.0:{}", port);
    let addr: std::net::SocketAddr = addr_str.parse()?;

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
