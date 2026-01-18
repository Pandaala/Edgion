//! Kubernetes Controller metrics
//!
//! Metrics for monitoring controller performance:
//! - Reconciliation counts and errors
//! - Reconciliation latency
//! - Initial sync duration
//! - Active controllers count

use metrics::{counter, gauge, histogram, Counter, Gauge, Histogram};
use std::sync::LazyLock;
use std::time::Instant;

/// Metric names as constants
pub mod names {
    /// Total reconciliations performed (with labels: kind, result)
    pub const RECONCILE_TOTAL: &str = "edgion_controller_reconcile_total";
    /// Reconciliation duration in seconds (with labels: kind)
    pub const RECONCILE_DURATION: &str = "edgion_controller_reconcile_duration_seconds";
    /// Initial sync duration in seconds (with labels: kind)
    pub const INIT_SYNC_DURATION: &str = "edgion_controller_init_sync_duration_seconds";
    /// Number of active resource controllers
    pub const ACTIVE_CONTROLLERS: &str = "edgion_controller_active";
    /// Total resources watched (with labels: kind)
    pub const RESOURCES_WATCHED: &str = "edgion_controller_resources_watched";
    /// Controller restarts (with labels: kind)
    pub const CONTROLLER_RESTARTS: &str = "edgion_controller_restarts_total";
    /// Leader election status (1 = leader, 0 = standby)
    pub const LEADER_STATUS: &str = "edgion_controller_leader";
}

/// Global controller metrics singleton
static CONTROLLER_METRICS: LazyLock<ControllerMetrics> = LazyLock::new(ControllerMetrics::new);

/// Get the global controller metrics instance
pub fn controller_metrics() -> &'static ControllerMetrics {
    &CONTROLLER_METRICS
}

/// Controller metrics collection
pub struct ControllerMetrics {
    /// Number of active controllers
    active_controllers: Gauge,
    /// Leader status
    leader_status: Gauge,
}

impl ControllerMetrics {
    fn new() -> Self {
        Self {
            active_controllers: gauge!(names::ACTIVE_CONTROLLERS),
            leader_status: gauge!(names::LEADER_STATUS),
        }
    }

    /// Record a controller starting
    pub fn controller_started(&self) {
        self.active_controllers.increment(1.0);
    }

    /// Record a controller stopping
    pub fn controller_stopped(&self) {
        self.active_controllers.decrement(1.0);
    }

    /// Set leader status (1.0 = leader, 0.0 = standby)
    pub fn set_leader(&self, is_leader: bool) {
        self.leader_status.set(if is_leader { 1.0 } else { 0.0 });
    }
}

/// Per-resource-kind metrics
pub struct ResourceMetrics {
    kind: &'static str,
    reconcile_success: Counter,
    reconcile_error: Counter,
    reconcile_duration: Histogram,
    init_sync_duration: Histogram,
    resources_watched: Gauge,
    restarts: Counter,
}

impl ResourceMetrics {
    /// Create metrics for a specific resource kind
    pub fn new(kind: &'static str) -> Self {
        Self {
            kind,
            reconcile_success: counter!(names::RECONCILE_TOTAL, "kind" => kind, "result" => "success"),
            reconcile_error: counter!(names::RECONCILE_TOTAL, "kind" => kind, "result" => "error"),
            reconcile_duration: histogram!(names::RECONCILE_DURATION, "kind" => kind),
            init_sync_duration: histogram!(names::INIT_SYNC_DURATION, "kind" => kind),
            resources_watched: gauge!(names::RESOURCES_WATCHED, "kind" => kind),
            restarts: counter!(names::CONTROLLER_RESTARTS, "kind" => kind),
        }
    }

    /// Record a successful reconciliation with duration
    #[inline]
    pub fn reconcile_success(&self, duration_secs: f64) {
        self.reconcile_success.increment(1);
        self.reconcile_duration.record(duration_secs);
    }

    /// Record a failed reconciliation with duration
    #[inline]
    pub fn reconcile_error(&self, duration_secs: f64) {
        self.reconcile_error.increment(1);
        self.reconcile_duration.record(duration_secs);
    }

    /// Record initial sync duration
    #[inline]
    pub fn init_sync_complete(&self, duration_secs: f64) {
        self.init_sync_duration.record(duration_secs);
    }

    /// Update the count of watched resources
    #[inline]
    pub fn set_resources_watched(&self, count: usize) {
        self.resources_watched.set(count as f64);
    }

    /// Record a controller restart
    #[inline]
    pub fn controller_restart(&self) {
        self.restarts.increment(1);
    }

    /// Get the kind this metrics is for
    #[allow(dead_code)]
    pub fn kind(&self) -> &'static str {
        self.kind
    }
}

/// RAII guard for measuring reconcile duration
pub struct ReconcileTimer {
    start: Instant,
    metrics: ResourceMetrics,
}

impl ReconcileTimer {
    /// Start a new reconcile timer
    pub fn start(kind: &'static str) -> Self {
        Self {
            start: Instant::now(),
            metrics: ResourceMetrics::new(kind),
        }
    }

    /// Record success and return duration
    pub fn success(self) -> f64 {
        let duration = self.start.elapsed().as_secs_f64();
        self.metrics.reconcile_success(duration);
        duration
    }

    /// Record error and return duration
    pub fn error(self) -> f64 {
        let duration = self.start.elapsed().as_secs_f64();
        self.metrics.reconcile_error(duration);
        duration
    }
}

/// RAII guard for measuring init sync duration
pub struct InitSyncTimer {
    start: Instant,
    metrics: ResourceMetrics,
}

impl InitSyncTimer {
    /// Start a new init sync timer
    pub fn start(kind: &'static str) -> Self {
        Self {
            start: Instant::now(),
            metrics: ResourceMetrics::new(kind),
        }
    }

    /// Complete and record duration, also set resources watched count
    pub fn complete(self, resources_count: usize) -> f64 {
        let duration = self.start.elapsed().as_secs_f64();
        self.metrics.init_sync_complete(duration);
        self.metrics.set_resources_watched(resources_count);
        duration
    }
}
