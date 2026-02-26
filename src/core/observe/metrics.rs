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
    pub const SSL_LOG_DROPPED: &str = "edgion_ssl_log_dropped_total";
    pub const TCP_LOG_DROPPED: &str = "edgion_tcp_log_dropped_total";
    pub const UDP_LOG_DROPPED: &str = "edgion_udp_log_dropped_total";
    // K8s Status update metrics
    pub const STATUS_UPDATE_TOTAL: &str = "edgion_status_update_total";
    pub const STATUS_UPDATE_FAILED: &str = "edgion_status_update_failed_total";
    pub const STATUS_UPDATE_SKIPPED: &str = "edgion_status_update_skipped_total";
    // Config sync metrics (client side)
    pub const CONFIG_RELOAD_SIGNALS: &str = "edgion_config_reload_signals_total";
    pub const CONFIG_RELIST_TOTAL: &str = "edgion_config_relist_total";
    // Backend request metrics (for LB testing and monitoring)
    pub const BACKEND_REQUESTS_TOTAL: &str = "edgion_backend_requests_total";
    // Gateway traffic byte counters
    pub const GATEWAY_REQUEST_BYTES: &str = "edgion_gateway_request_bytes_total";
    pub const GATEWAY_RESPONSE_BYTES: &str = "edgion_gateway_response_bytes_total";
    // Controller: number of gateway instances connected via gRPC long-poll
    pub const CONTROLLER_CONNECTED_GATEWAYS: &str = "edgion_controller_connected_gateways";
    // Controller: schema validation failures from Admin API (non-K8s mode)
    pub const SCHEMA_VALIDATION_ERRORS: &str = "edgion_controller_schema_validation_errors_total";
}

/// Global metrics singleton
static GLOBAL_METRICS: LazyLock<GatewayMetrics> = LazyLock::new(GatewayMetrics::new);

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
    /// Total SSL logs dropped (channel full)
    ssl_log_dropped: Counter,
    /// Total TCP logs dropped (channel full)
    tcp_log_dropped: Counter,
    /// Total UDP logs dropped (channel full)
    udp_log_dropped: Counter,
    /// Total K8s status updates attempted
    status_update_total: Counter,
    /// Total K8s status update failures
    status_update_failed: Counter,
    /// Total K8s status updates skipped (no change)
    status_update_skipped: Counter,
    /// Total reload signals received from controller
    config_reload_signals: Counter,
    /// Total config relist operations
    config_relist_total: Counter,
    /// Total gateway request bytes proxied (downstream → upstream)
    gateway_request_bytes: Counter,
    /// Total gateway response bytes proxied (upstream → downstream)
    gateway_response_bytes: Counter,
    /// Currently connected gateway instances (gRPC WatchServerMeta)
    controller_connected_gateways: Gauge,
    /// Total schema validation errors from Admin API (non-K8s mode)
    schema_validation_errors: Counter,
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
            ssl_log_dropped: counter!(names::SSL_LOG_DROPPED),
            tcp_log_dropped: counter!(names::TCP_LOG_DROPPED),
            udp_log_dropped: counter!(names::UDP_LOG_DROPPED),
            status_update_total: counter!(names::STATUS_UPDATE_TOTAL),
            status_update_failed: counter!(names::STATUS_UPDATE_FAILED),
            status_update_skipped: counter!(names::STATUS_UPDATE_SKIPPED),
            config_reload_signals: counter!(names::CONFIG_RELOAD_SIGNALS),
            config_relist_total: counter!(names::CONFIG_RELIST_TOTAL),
            gateway_request_bytes: counter!(names::GATEWAY_REQUEST_BYTES),
            gateway_response_bytes: counter!(names::GATEWAY_RESPONSE_BYTES),
            controller_connected_gateways: gauge!(names::CONTROLLER_CONNECTED_GATEWAYS),
            schema_validation_errors: counter!(names::SCHEMA_VALIDATION_ERRORS),
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

    /// Record a dropped SSL log entry
    #[inline]
    pub fn ssl_log_dropped(&self) {
        self.ssl_log_dropped.increment(1);
    }

    /// Record a dropped TCP log entry
    #[inline]
    pub fn tcp_log_dropped(&self) {
        self.tcp_log_dropped.increment(1);
    }

    /// Record a dropped UDP log entry
    #[inline]
    pub fn udp_log_dropped(&self) {
        self.udp_log_dropped.increment(1);
    }

    /// Record a successful K8s status update
    #[inline]
    pub fn status_update_success(&self) {
        self.status_update_total.increment(1);
    }

    /// Record a failed K8s status update
    #[inline]
    pub fn status_update_failed(&self) {
        self.status_update_total.increment(1);
        self.status_update_failed.increment(1);
    }

    /// Record a skipped K8s status update (no change needed)
    #[inline]
    pub fn status_update_skipped(&self) {
        self.status_update_skipped.increment(1);
    }

    /// Record a reload signal received from controller
    #[inline]
    pub fn config_reload_signal(&self) {
        self.config_reload_signals.increment(1);
    }

    /// Record a config relist operation
    #[inline]
    pub fn config_relist(&self) {
        self.config_relist_total.increment(1);
    }

    /// Record proxied request bytes (downstream → upstream body)
    #[inline]
    pub fn add_request_bytes(&self, bytes: u64) {
        self.gateway_request_bytes.increment(bytes);
    }

    /// Record proxied response bytes (upstream → downstream body)
    #[inline]
    pub fn add_response_bytes(&self, bytes: u64) {
        self.gateway_response_bytes.increment(bytes);
    }

    /// Record a gateway instance connected to controller via gRPC
    #[inline]
    pub fn gateway_connected(&self) {
        self.controller_connected_gateways.increment(1.0);
    }

    /// Record a gateway instance disconnected from controller
    #[inline]
    pub fn gateway_disconnected(&self) {
        self.controller_connected_gateways.decrement(1.0);
    }

    /// Record a schema validation error from Admin API (non-K8s mode)
    #[inline]
    pub fn schema_validation_error(&self) {
        self.schema_validation_errors.increment(1);
    }
}

/// Record a backend request metric
///
/// This function records detailed metrics for each backend request,
/// useful for monitoring request distribution and verifying LB behavior.
///
/// # Arguments
/// * `gateway_ns` - Gateway namespace
/// * `gateway_name` - Gateway name
/// * `route_ns` - Matched route namespace
/// * `route_name` - Matched route name
/// * `backend_ns` - Backend service namespace
/// * `backend_name` - Backend service name
/// * `protocol` - Protocol (http/grpc/websocket, from discover_protocol)
/// * `status` - Status group (2xx/3xx/4xx/5xx/failed)
/// * `test_key` - Test identifier (from Gateway annotation, empty in production)
/// * `test_data` - JSON test data (from TestData::to_json(), empty in production)
#[inline]
pub fn record_backend_request(
    gateway_ns: &str,
    gateway_name: &str,
    route_ns: &str,
    route_name: &str,
    backend_ns: &str,
    backend_name: &str,
    protocol: &str,
    status: &str,
    test_key: &str,
    test_data: &str,
) {
    counter!(
        names::BACKEND_REQUESTS_TOTAL,
        "gateway_namespace" => gateway_ns.to_string(),
        "gateway_name" => gateway_name.to_string(),
        "route_namespace" => route_ns.to_string(),
        "route_name" => route_name.to_string(),
        "backend_namespace" => backend_ns.to_string(),
        "backend_name" => backend_name.to_string(),
        "protocol" => protocol.to_string(),
        "status" => status.to_string(),
        "test_key" => test_key.to_string(),
        "test_data" => test_data.to_string(),
    )
    .increment(1);
}

/// Convert HTTP status code to status group
///
/// Groups status codes into: 2xx, 3xx, 4xx, 5xx, or "failed" for errors
#[inline]
pub fn status_group(status: Option<u16>) -> &'static str {
    match status {
        Some(s) if (200..300).contains(&s) => "2xx",
        Some(s) if (300..400).contains(&s) => "3xx",
        Some(s) if (400..500).contains(&s) => "4xx",
        Some(s) if (500..600).contains(&s) => "5xx",
        _ => "failed",
    }
}
