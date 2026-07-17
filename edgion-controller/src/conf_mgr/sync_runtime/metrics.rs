//! Sync Runtime metrics
//!
//! Metrics for monitoring sync runtime performance:
//! - Initial sync duration
//! - Active controllers count
//! - Leader election status
//! - Reload operations

use metrics::{counter, gauge, Counter, Gauge};
use std::sync::LazyLock;
use std::time::Instant;

/// Metric names as constants
pub mod names {
    /// Last initial sync duration in seconds (with labels: kind)
    pub const INIT_SYNC_DURATION: &str = "edgion_controller_init_sync_duration_seconds_last";
    /// Number of active resource controllers
    pub const ACTIVE_CONTROLLERS: &str = "edgion_controller_active";
    /// Total resources watched (with labels: kind)
    pub const RESOURCES_WATCHED: &str = "edgion_controller_resources_watched";
    /// Leader election status (1 = leader, 0 = standby)
    pub const LEADER_STATUS: &str = "edgion_controller_leader";
    /// Total reload operations triggered
    pub const RELOAD_TOTAL: &str = "edgion_controller_reload_total";
    /// Last reload duration in seconds
    pub const RELOAD_DURATION: &str = "edgion_controller_reload_duration_seconds_last";
    /// Total client notifications sent for reload
    pub const RELOAD_CLIENT_NOTIFICATIONS: &str = "edgion_controller_reload_client_notifications_total";
    /// Total status writes (with labels: kind, backend, result)
    pub const STATUS_WRITES_TOTAL: &str = "edgion_controller_status_writes_total";
    /// Total status writes skipped by semantic diff (with labels: kind)
    pub const STATUS_WRITES_SKIPPED_TOTAL: &str = "edgion_controller_status_writes_skipped_total";
    /// Total Apply events received during init phase (unexpected; with labels: kind)
    ///
    /// In normal operation this counter should stay at zero. A non-zero value
    /// indicates the kube-rs watcher stream sent an Apply before InitDone, which
    /// is treated as an InitApply with a warning log. Monitor this in production
    /// to detect watcher stream ordering anomalies.
    pub const UNEXPECTED_APPLY_DURING_INIT_TOTAL: &str = "edgion_controller_unexpected_apply_during_init_total";
    /// Leader-job worker liveness (1 = running, 0 = stopped or dead;
    /// label: leader_job = acme | conf_sync_rotation | dns_resolver).
    /// The label is deliberately NOT `job`: Prometheus reserves `job` as a
    /// target label and would rename an exported one to `exported_job`,
    /// breaking any alert written against the documented name.
    pub const LEADER_JOB_ALIVE: &str = "edgion_controller_leader_job_alive";
}

/// Global controller metrics singleton
static CONTROLLER_METRICS: LazyLock<ControllerMetrics> = LazyLock::new(ControllerMetrics::new);

/// Get the global controller metrics instance
pub fn controller_metrics() -> &'static ControllerMetrics {
    &CONTROLLER_METRICS
}

/// Gauge handle for one leader job's liveness series.
///
/// Write discipline (see the wedge-recovery design spec): 1 is written by the
/// start path; 0 on orderly stop is written by each service's `stop_*` fn,
/// gated on a handle actually being taken; 0 on unexpected death is written by
/// the supervision observer only while holding the service's own lock with the
/// generation confirmed current. An ungated write can wedge the series for a
/// live worker (drain-based stops outlive the stop call) or mint a permanent
/// 0-series for a job that never started.
pub fn leader_job_gauge(job: &'static str) -> Gauge {
    gauge!(names::LEADER_JOB_ALIVE, "leader_job" => job)
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

/// Per-resource-kind metrics for init sync
pub struct ResourceMetrics {
    init_sync_duration: Gauge,
    resources_watched: Gauge,
}

impl ResourceMetrics {
    /// Create metrics for a specific resource kind
    pub fn new(kind: &'static str) -> Self {
        Self {
            init_sync_duration: gauge!(names::INIT_SYNC_DURATION, "kind" => kind),
            resources_watched: gauge!(names::RESOURCES_WATCHED, "kind" => kind),
        }
    }

    /// Record initial sync duration
    #[inline]
    pub fn init_sync_complete(&self, duration_secs: f64) {
        self.init_sync_duration.set(duration_secs);
    }

    /// Update the count of watched resources
    #[inline]
    pub fn set_resources_watched(&self, count: usize) {
        self.resources_watched.set(count as f64);
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
        let duration = Instant::now().saturating_duration_since(self.start).as_secs_f64();
        self.metrics.init_sync_complete(duration);
        self.metrics.set_resources_watched(resources_count);
        duration
    }
}

// ==================== Reload Metrics ====================

/// Global reload metrics singleton
static RELOAD_METRICS: LazyLock<ReloadMetrics> = LazyLock::new(ReloadMetrics::new);

/// Get the global reload metrics instance
pub fn reload_metrics() -> &'static ReloadMetrics {
    &RELOAD_METRICS
}

/// Reload operation metrics
pub struct ReloadMetrics {
    /// Total reload operations triggered
    reload_total: Counter,
    /// Last reload duration
    reload_duration: Gauge,
    /// Total client notifications sent
    client_notifications: Counter,
}

impl ReloadMetrics {
    fn new() -> Self {
        Self {
            reload_total: counter!(names::RELOAD_TOTAL),
            reload_duration: gauge!(names::RELOAD_DURATION),
            client_notifications: counter!(names::RELOAD_CLIENT_NOTIFICATIONS),
        }
    }

    /// Record a reload operation started
    #[inline]
    pub fn reload_started(&self) {
        self.reload_total.increment(1);
    }

    /// Record reload duration
    #[inline]
    pub fn reload_completed(&self, duration_secs: f64) {
        self.reload_duration.set(duration_secs);
    }

    /// Record a client notification sent
    #[inline]
    pub fn client_notified(&self) {
        self.client_notifications.increment(1);
    }
}

// ==================== Status Write Metrics ====================

/// Record a successful status write (K8s API or FileSystem).
///
/// Labels: `kind` (resource kind), `backend` ("kubernetes" or "filesystem").
#[inline]
pub fn record_status_write(kind: &str, backend: &str) {
    counter!(names::STATUS_WRITES_TOTAL, "kind" => kind.to_string(), "backend" => backend.to_string(), "result" => "success").increment(1);
}

/// Record a failed status write.
#[inline]
pub fn record_status_write_error(kind: &str, backend: &str) {
    counter!(names::STATUS_WRITES_TOTAL, "kind" => kind.to_string(), "backend" => backend.to_string(), "result" => "error").increment(1);
}

/// Record a status write that was skipped because semantic diff detected
/// no meaningful change.
#[inline]
pub fn record_status_write_skipped(kind: &str) {
    counter!(names::STATUS_WRITES_SKIPPED_TOTAL, "kind" => kind.to_string()).increment(1);
}
