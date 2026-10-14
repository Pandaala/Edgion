//! Shared Prometheus recorder installation and `/metrics` handler.
//!
//! Gateway and Controller install the same classified recorder. Valid fixed
//! tuples use the unchanged Prometheus exporter without idle expiration. Dynamic
//! identities use renewable slots with bounded aggregation and idle eviction.
//! Both partitions share one lifetime admission soft cap; valid fixed tuples
//! always have priority. Two bounded Controller snapshot families stay external.
//!
//! Binary startup owns installation and logging. A scrape-only mutex bounds
//! retained snapshots through formatting; request operations never acquire it.
//! Serial repeated installation is idempotent, while concurrent installation
//! still reports the global recorder conflict.

use super::dynamic_metrics::DynamicStore;
use super::metrics_catalog::{definition, validate, MetricClass, MetricType};
use crate::observe::cardinality_guard::{CardinalityGuard, CardinalityMode};
use axum::{
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use http::{header, StatusCode};
use metrics::{Counter, Gauge, Histogram, Key, KeyName, Metadata, Recorder, SharedString, Unit};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle, PrometheusRecorder};
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::OnceLock;

/// Global Prometheus handle. Set exactly once per process.
struct Exposure {
    fixed: PrometheusHandle,
    dynamic: Arc<DynamicStore>,
    render: Mutex<()>,
}
static PROMETHEUS_HANDLE: OnceLock<Exposure> = OnceLock::new();
struct ClassifiedRecorder {
    fixed: PrometheusRecorder,
    dynamic: Arc<DynamicStore>,
}
impl ClassifiedRecorder {
    fn class(&self, key: &Key, kind: MetricType) -> Option<MetricClass> {
        let def = definition(key.name())?;
        validate(key, kind, def.class).then_some(def.class)
    }
}
impl Recorder for ClassifiedRecorder {
    fn describe_counter(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        match definition(key.as_str()).map(|def| def.class) {
            Some(MetricClass::Fixed) => self.fixed.describe_counter(key, unit, description),
            Some(MetricClass::Dynamic) => self.dynamic.describe(key.as_str(), description.to_string()),
            _ => {}
        }
    }
    fn describe_gauge(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        match definition(key.as_str()).map(|def| def.class) {
            Some(MetricClass::Fixed) => self.fixed.describe_gauge(key, unit, description),
            Some(MetricClass::Dynamic) => self.dynamic.describe(key.as_str(), description.to_string()),
            _ => {}
        }
    }
    fn describe_histogram(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        match definition(key.as_str()).map(|def| def.class) {
            Some(MetricClass::Fixed) => self.fixed.describe_histogram(key, unit, description),
            Some(MetricClass::Dynamic) => self.dynamic.describe(key.as_str(), description.to_string()),
            _ => {}
        }
    }
    fn register_counter(&self, key: &Key, metadata: &Metadata<'_>) -> Counter {
        match self.class(key, MetricType::Counter) {
            Some(MetricClass::Fixed) => self.fixed.register_counter(key, metadata),
            Some(MetricClass::Dynamic) => self.dynamic.counter(key),
            _ => Counter::noop(),
        }
    }
    fn register_gauge(&self, key: &Key, metadata: &Metadata<'_>) -> Gauge {
        match self.class(key, MetricType::Gauge) {
            Some(MetricClass::Fixed) => self.fixed.register_gauge(key, metadata),
            Some(MetricClass::Dynamic) => self.dynamic.gauge(key),
            _ => Gauge::noop(),
        }
    }
    fn register_histogram(&self, key: &Key, metadata: &Metadata<'_>) -> Histogram {
        match self.class(key, MetricType::Histogram) {
            Some(MetricClass::Fixed) => self.fixed.register_histogram(key, metadata),
            Some(MetricClass::Dynamic) => self.dynamic.histogram(key),
            _ => Histogram::noop(),
        }
    }
}

#[cfg(any(test, feature = "test-util"))]
thread_local! { static LOCAL_DYNAMIC: std::cell::RefCell<Option<Arc<DynamicStore>>> = const { std::cell::RefCell::new(None) }; }
pub(crate) fn local_recorder_active() -> bool {
    #[cfg(any(test, feature = "test-util"))]
    {
        LOCAL_DYNAMIC.with(|store| store.borrow().is_some())
    }
    #[cfg(not(any(test, feature = "test-util")))]
    {
        false
    }
}
pub(crate) fn dynamic_store() -> Option<Arc<DynamicStore>> {
    #[cfg(any(test, feature = "test-util"))]
    if let Some(store) = LOCAL_DYNAMIC.with(|store| store.borrow().clone()) {
        return Some(store);
    }
    PROMETHEUS_HANDLE.get().map(|handle| handle.dynamic.clone())
}

/// Configuration for installing the global Prometheus recorder.
///
/// `service` is used as a Prometheus `service=""` global label so metrics
/// from Gateway/Controller can be distinguished in a shared backend.
/// `idle_timeout_secs = 0` disables cold-series eviction.
///
/// `histogram_buckets` is an optional list of `(matcher, buckets)` pairs
/// for tuning individual histograms (Gateway uses this for
/// `gateway_request_duration_ms`). Pass `&[]` when no histograms need
/// custom buckets.
pub struct RecorderConfig<'a> {
    pub service: &'static str,
    pub idle_timeout_secs: u64,
    pub histogram_buckets: &'a [(Matcher, Vec<f64>)],
    /// Max distinct series before the guard acts. 0 disables the guard.
    pub cardinality_cap: usize,
    pub cardinality_mode: CardinalityMode,
}

/// Install the global Prometheus recorder.
///
/// Idempotent for serial startup calls: after successful installation,
/// subsequent calls return `Ok(())`. Concurrent first calls are not serialized;
/// the losing caller can receive the global recorder's installation error.
pub fn install_global_recorder(cfg: RecorderConfig<'_>) -> Result<(), String> {
    if !matches!(cfg.service, "edgion-gateway" | "edgion-controller") {
        return Err("unsupported production metric service role".to_string());
    }
    if PROMETHEUS_HANDLE.get().is_some() {
        return Ok(());
    }

    let mut builder = PrometheusBuilder::new().add_global_label("service", cfg.service);

    for (matcher, buckets) in cfg.histogram_buckets {
        builder = builder
            .set_buckets_for_metric(matcher.clone(), buckets)
            .map_err(|e| format!("Failed to set buckets: {}", e))?;
    }

    let fixed = builder.build_recorder();
    let handle = fixed.handle();
    let dynamic = Arc::new(DynamicStore::new(
        cfg.service,
        cfg.idle_timeout_secs,
        cfg.histogram_buckets,
    ));
    let recorder = ClassifiedRecorder {
        fixed,
        dynamic: dynamic.clone(),
    };
    let guarded = CardinalityGuard::new(recorder, cfg.cardinality_cap, cfg.cardinality_mode);
    dynamic.attach(guarded.admission.clone());
    metrics::set_global_recorder(guarded).map_err(|error| format!("set_global_recorder: {error:?}"))?;
    let _ = PROMETHEUS_HANDLE.set(Exposure {
        fixed: handle,
        dynamic,
        render: Mutex::new(()),
    });

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

pub use super::dynamic_metrics::DynamicMetricStats;
/// Metrics registry diagnostics. Lifetime admission keys and unexpired events
/// are legitimate persistence; overdue epochs and retired owners are not.
pub fn metric_store_stats() -> DynamicMetricStats {
    dynamic_store().map(|store| store.stats()).unwrap_or_default()
}

#[cfg(any(test, feature = "test-util"))]
pub struct TestRecorder {
    recorder: CardinalityGuard<ClassifiedRecorder>,
    fixed: PrometheusHandle,
    dynamic: Arc<DynamicStore>,
    scrape: Mutex<()>,
    clock: Option<Arc<std::sync::atomic::AtomicU64>>,
}
#[cfg(any(test, feature = "test-util"))]
impl TestRecorder {
    pub fn new(cfg: RecorderConfig<'_>) -> Result<Self, String> {
        Self::build(cfg, None)
    }
    pub fn with_clock(cfg: RecorderConfig<'_>) -> Result<Self, String> {
        Self::build(cfg, Some(Arc::new(std::sync::atomic::AtomicU64::new(0))))
    }
    fn build(cfg: RecorderConfig<'_>, clock: Option<Arc<std::sync::atomic::AtomicU64>>) -> Result<Self, String> {
        let mut builder = PrometheusBuilder::new().add_global_label("service", cfg.service);
        for (matcher, buckets) in cfg.histogram_buckets {
            builder = builder
                .set_buckets_for_metric(matcher.clone(), buckets)
                .map_err(|error| error.to_string())?;
        }
        let fixed = builder.build_recorder();
        let handle = fixed.handle();
        let mut dynamic = DynamicStore::new(cfg.service, cfg.idle_timeout_secs, cfg.histogram_buckets);
        dynamic.set_clock(clock.clone());
        let dynamic = Arc::new(dynamic);
        let recorder = CardinalityGuard::new(
            ClassifiedRecorder {
                fixed,
                dynamic: dynamic.clone(),
            },
            cfg.cardinality_cap,
            cfg.cardinality_mode,
        );
        dynamic.attach(recorder.admission.clone());
        Ok(Self {
            recorder,
            fixed: handle,
            dynamic,
            scrape: Mutex::new(()),
            clock,
        })
    }
    pub fn with<T>(&self, operation: impl FnOnce() -> T) -> T {
        struct Restore(Option<Arc<DynamicStore>>);
        impl Drop for Restore {
            fn drop(&mut self) {
                LOCAL_DYNAMIC.with(|slot| *slot.borrow_mut() = self.0.take());
            }
        }
        let previous = LOCAL_DYNAMIC.with(|slot| slot.replace(Some(self.dynamic.clone())));
        let _restore = Restore(previous);
        metrics::with_local_recorder(&self.recorder, operation)
    }
    pub fn render(&self) -> String {
        let _guard = self.scrape.lock();
        let mut out = self.fixed.render();
        out.push_str(&self.dynamic.render());
        out
    }
    pub fn advance(&self, duration: std::time::Duration) {
        if let Some(clock) = &self.clock {
            clock.fetch_add(
                duration.as_nanos().min(u64::MAX as u128) as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
        }
    }
    pub fn stats(&self) -> DynamicMetricStats {
        self.dynamic.stats()
    }
    /// Test-only production snapshot probe: dense three-window summary clone,
    /// including lock acquisition, excluding post-gate destruction/serialization.
    pub fn dense_summary_snapshot_probe(&self) -> (usize, u64) {
        self.dynamic.dense_summary_snapshot_probe()
    }
}

/// Axum handler that renders the current recorder state as Prometheus
/// text. Returns 500 with a diagnostic body if the recorder was not
/// installed before the server started serving.
pub async fn metrics_handler() -> Response {
    render_metrics()
}

/// Synchronous render boundary for callers that must coordinate live metric
/// handles with exporter idle eviction. Do not hold a non-Send guard across
/// the asynchronous handler; wrap this function instead.
pub fn render_metrics() -> Response {
    match PROMETHEUS_HANDLE.get() {
        Some(handle) => {
            let _scrape = handle.render.lock();
            let mut body = handle.fixed.render();
            body.push_str(&handle.dynamic.render());
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
/// listener. Controller reuses its existing `/health` route.
pub async fn health_handler() -> &'static str {
    "OK"
}

/// Router that exposes `/metrics` and `/health` on a standalone listener.
/// Used by Gateway; Controller instead mounts `metrics_handler` directly
/// on its admin router.
pub fn create_metrics_router() -> Router {
    Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/health", get(health_handler))
}

#[cfg(test)]
#[path = "../../tests/unit/observe/metrics_api_test.rs"]
mod tests;
