//! Edgion metrics definitions
//!
//! Centralized metrics for monitoring gateway performance.
//! Uses the `metrics` crate for thread-safe, high-performance counters.

use metrics::{counter, gauge, Counter, Gauge};
use std::sync::LazyLock;

/// Metric names as constants for consistency
pub mod names {
    pub const CTX_CREATED: &str = "edgion_ctx_created_total";
    pub const CTX_ACTIVE: &str = "edgion_ctx_active";
    pub const REQUESTS_TOTAL: &str = "edgion_requests_total";
    pub const REQUESTS_FAILED: &str = "edgion_requests_failed_total";
    pub const ACCESS_LOG_DROPPED: &str = "edgion_access_log_dropped_total";
}

/// Global metrics singleton
static GLOBAL_METRICS: LazyLock<GatewayMetrics> = LazyLock::new(|| GatewayMetrics::new());

/// Get the global metrics instance
pub fn global_metrics() -> &'static GatewayMetrics {
    &GLOBAL_METRICS
}

/// Gateway metrics collection
///
/// Provides high-performance, thread-safe metrics using sharded counters.
/// All metrics are automatically exported via the metrics facade.
pub struct GatewayMetrics {
    /// Total contexts created (requests received)
    ctx_created: Counter,
    /// Currently active contexts
    ctx_active: Gauge,
    /// Total requests processed
    requests_total: Counter,
    /// Total failed requests
    requests_failed: Counter,
    /// Total access logs dropped (channel full)
    access_log_dropped: Counter,
}

impl GatewayMetrics {
    /// Create a new GatewayMetrics instance
    fn new() -> Self {
        Self {
            ctx_created: counter!(names::CTX_CREATED),
            ctx_active: gauge!(names::CTX_ACTIVE),
            requests_total: counter!(names::REQUESTS_TOTAL),
            requests_failed: counter!(names::REQUESTS_FAILED),
            access_log_dropped: counter!(names::ACCESS_LOG_DROPPED),
        }
    }

    /// Record a new context creation
    #[inline]
    pub fn ctx_created(&self) {
        self.ctx_created.increment(1);
        self.ctx_active.increment(1.0);
    }

    /// Record a context destruction
    #[inline]
    pub fn ctx_destroyed(&self) {
        self.ctx_active.decrement(1.0);
    }

    /// Record a successful request
    #[inline]
    pub fn request_success(&self) {
        self.requests_total.increment(1);
    }

    /// Record a failed request
    #[inline]
    pub fn request_failed(&self) {
        self.requests_total.increment(1);
        self.requests_failed.increment(1);
    }

    /// Record a dropped access log entry
    #[inline]
    pub fn access_log_dropped(&self) {
        self.access_log_dropped.increment(1);
    }
}
