//! Gateway-specific wrappers around the shared `common::observe::metrics_api`.
//!
//! The generic recorder installation and `/metrics` handler live in
//! `edgion_common::observe::metrics_api`. This module wires Gateway's
//! `MetricsConfig` (custom histogram buckets, cold-series eviction) into
//! that primitive and exposes a standalone `serve`/`serve_with_shutdown`
//! suitable for Gateway's dedicated metrics port.

use axum::{extract::State, response::Response, routing::get, Router};
use edgion_common::conf_sync::cert_expiry_watch::ExpiryWatchState;
use metrics_exporter_prometheus::Matcher;
use parking_lot::Mutex;
use std::sync::Arc;

use crate::cli::config::MetricsConfig;
use crate::conf_sync::ConfigSyncClient;
use edgion_common::observe::metrics_api::{
    health_handler, install_global_recorder, recorder_installed, render_metrics, RecorderConfig,
};

/// Initialize the Prometheus metrics exporter with Gateway configuration.
///
/// Translates Gateway's `MetricsConfig` into a `RecorderConfig` understood
/// by the shared recorder installer. Custom buckets are only registered
/// when the request-duration histogram is explicitly enabled.
pub fn init_metrics_exporter(cfg: &MetricsConfig) -> Result<(), String> {
    let rd = &cfg.request_duration;

    let buckets: Vec<(Matcher, Vec<f64>)> = if rd.enabled && !rd.buckets.is_empty() {
        vec![(
            Matcher::Full(crate::observe::metrics::registry::names::GATEWAY_REQUEST_DURATION_MS.to_string()),
            rd.buckets.clone(),
        )]
    } else {
        Vec::new()
    };

    install_global_recorder(RecorderConfig {
        service: "edgion-gateway",
        idle_timeout_secs: rd.idle_timeout_secs,
        histogram_buckets: &buckets,
        cardinality_cap: cfg.cardinality_cap,
        cardinality_mode: cfg.cardinality_mode,
    })
}

struct MetricsState {
    sync_client: Arc<ConfigSyncClient>,
    expiry_watch: Mutex<ExpiryWatchState>,
}

impl MetricsState {
    fn record_cert_expiry(&self) {
        // Serialize sampling and severity transitions across simultaneous scrapes.
        // Read the installed generation only after taking this cold-path lock.
        let mut state = self.expiry_watch.lock();
        if let Some(validity) = self.sync_client.installed_cert_validity() {
            super::registry::GatewayMetrics::record_conf_sync_cert_expiry(
                validity,
                chrono::Utc::now().timestamp(),
                &mut state,
            );
        }
    }
}

/// Sample installed certificate expiry and published LinkSys health, and preserve live Service metric handles
/// before the shared exporter renders them. The synchronous section has no await.
async fn metrics_handler(State(state): State<Arc<MetricsState>>) -> Response {
    state.record_cert_expiry();
    crate::link_sys::runtime::store::record_published_link_sys_health();
    crate::observe::metrics::GatewayMetrics::with_live_lb_degrade_counters(|| {
        super::registry::with_live_wasm_metrics(render_metrics)
    })
}

/// Gateway's standalone metrics listener. All Gateway scrapes must use the
/// keepalive boundary rather than mounting the shared handler directly.
pub fn create_metrics_router(sync_client: Arc<ConfigSyncClient>) -> Router {
    Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/health", get(health_handler))
        .with_state(Arc::new(MetricsState {
            sync_client,
            expiry_watch: Mutex::new(ExpiryWatchState::default()),
        }))
}

/// Serve the metrics API with graceful shutdown support.
pub async fn serve_with_shutdown(
    cfg: &MetricsConfig,
    sync_client: Arc<ConfigSyncClient>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    if !recorder_installed() {
        init_metrics_exporter(cfg).map_err(|e| anyhow::anyhow!("Failed to initialize metrics exporter: {}", e))?;
    }

    let app = create_metrics_router(sync_client);
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
    axum::serve(listener, app).with_graceful_shutdown(shutdown).await?;

    tracing::info!(component = "metrics_api", "Metrics API server stopped");
    Ok(())
}
