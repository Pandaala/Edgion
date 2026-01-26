//! Sync Runtime metrics
//!
//! Metrics for monitoring sync runtime performance:
//! - Initial sync duration
//! - Active controllers count
//! - Leader election status
//! - Reload operations

use metrics::{counter, gauge, histogram, Counter, Gauge, Histogram};
use std::sync::LazyLock;
use std::time::Instant;

/// Metric names as constants
pub mod names {
    /// Initial sync duration in seconds (with labels: kind)
    pub const INIT_SYNC_DURATION: &str = "edgion_controller_init_sync_duration_seconds";
    /// Number of active resource controllers
    pub const ACTIVE_CONTROLLERS: &str = "edgion_controller_active";
    /// Total resources watched (with labels: kind)
    pub const RESOURCES_WATCHED: &str = "edgion_controller_resources_watched";
    /// Leader election status (1 = leader, 0 = standby)
    pub const LEADER_STATUS: &str = "edgion_controller_leader";
    /// Total reload operations triggered
    pub const RELOAD_TOTAL: &str = "edgion_controller_reload_total";
    /// Reload duration in seconds
    pub const RELOAD_DURATION: &str = "edgion_controller_reload_duration_seconds";
    /// Total client notifications sent for reload
    pub const RELOAD_CLIENT_NOTIFICATIONS: &str = "edgion_controller_reload_client_notifications_total";
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

/// Per-resource-kind metrics for init sync
pub struct ResourceMetrics {
    init_sync_duration: Histogram,
    resources_watched: Gauge,
}

impl ResourceMetrics {
    /// Create metrics for a specific resource kind
    pub fn new(kind: &'static str) -> Self {
        Self {
            init_sync_duration: histogram!(names::INIT_SYNC_DURATION, "kind" => kind),
            resources_watched: gauge!(names::RESOURCES_WATCHED, "kind" => kind),
        }
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
    /// Reload duration histogram
    reload_duration: Histogram,
    /// Total client notifications sent
    client_notifications: Counter,
}

impl ReloadMetrics {
    fn new() -> Self {
        Self {
            reload_total: counter!(names::RELOAD_TOTAL),
            reload_duration: histogram!(names::RELOAD_DURATION),
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
        self.reload_duration.record(duration_secs);
    }

    /// Record a client notification sent
    #[inline]
    pub fn client_notified(&self) {
        self.client_notifications.increment(1);
    }
}

/// RAII guard for measuring reload duration
pub struct ReloadTimer {
    start: Instant,
}

impl ReloadTimer {
    /// Start a new reload timer
    pub fn start() -> Self {
        reload_metrics().reload_started();
        Self {
            start: Instant::now(),
        }
    }

    /// Complete and record duration
    pub fn complete(self) -> f64 {
        let duration = self.start.elapsed().as_secs_f64();
        reload_metrics().reload_completed(duration);
        duration
    }
}
