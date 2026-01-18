//! Kubernetes Controller metrics
//!
//! Metrics for monitoring controller performance:
//! - Initial sync duration
//! - Active controllers count
//! - Leader election status

use metrics::{gauge, histogram, Gauge, Histogram};
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
