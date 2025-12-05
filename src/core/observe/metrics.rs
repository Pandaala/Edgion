//! Edgion metrics definitions
//!
//! Centralized metrics for monitoring gateway performance.
//! Uses the `metrics` crate for thread-safe, high-performance counters.

use metrics::{counter, gauge, Counter, Gauge};

/// Metric names as constants for consistency
pub mod names {
    pub const CTX_CREATED: &str = "edgion_ctx_created_total";
    pub const CTX_ACTIVE: &str = "edgion_ctx_active";
    pub const REQUESTS_TOTAL: &str = "edgion_requests_total";
    pub const REQUESTS_FAILED: &str = "edgion_requests_failed_total";
}

/// Gateway metrics collection
/// 
/// Provides high-performance, thread-safe metrics using sharded counters.
/// All metrics are automatically exported via the metrics facade.
pub struct GatewayMetrics {
    /// Total contexts created (requests received)
    pub ctx_created: Counter,
    /// Currently active contexts
    pub ctx_active: Gauge,
    /// Total requests processed
    pub requests_total: Counter,
    /// Total failed requests
    pub requests_failed: Counter,
}

impl GatewayMetrics {
    /// Create a new GatewayMetrics instance
    /// 
    /// Metrics are registered with the global metrics registry.
    pub fn new() -> Self {
        Self {
            ctx_created: counter!(names::CTX_CREATED),
            ctx_active: gauge!(names::CTX_ACTIVE),
            requests_total: counter!(names::REQUESTS_TOTAL),
            requests_failed: counter!(names::REQUESTS_FAILED),
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
}

impl Default for GatewayMetrics {
    fn default() -> Self {
        Self::new()
    }
}

