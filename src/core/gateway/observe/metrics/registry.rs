//! Edgion metrics definitions
//!
//! Centralized metrics for monitoring gateway performance.
//! Uses the `metrics` crate for thread-safe, high-performance counters.

use metrics::{counter, gauge, histogram, Counter, Gauge};
use std::sync::{LazyLock, OnceLock};

/// Metric names as constants for consistency
pub mod names {
    pub const CTX_CREATED: &str = "edgion_ctx_created_total";
    pub const CTX_ACTIVE: &str = "edgion_ctx_active";
    pub const REQUESTS_TOTAL: &str = "edgion_requests_total";
    pub const REQUESTS_FAILED: &str = "edgion_requests_failed_total";
    pub const ACCESS_LOG_DROPPED: &str = "edgion_access_log_dropped_total";
    pub const SSL_LOG_DROPPED: &str = "edgion_ssl_log_dropped_total";
    pub const TCP_LOG_DROPPED: &str = "edgion_tcp_log_dropped_total";
    pub const TLS_LOG_DROPPED: &str = "edgion_tls_log_dropped_total";
    pub const UDP_LOG_DROPPED: &str = "edgion_udp_log_dropped_total";
    // K8s Status update metrics
    pub const STATUS_UPDATE_TOTAL: &str = "edgion_status_update_total";
    pub const STATUS_UPDATE_FAILED: &str = "edgion_status_update_failed_total";
    pub const STATUS_UPDATE_SKIPPED: &str = "edgion_status_update_skipped_total";
    // Config sync metrics (client side)
    pub const CONFIG_RELOAD_SIGNALS: &str = "edgion_config_reload_signals_total";
    pub const CONFIG_RELIST_TOTAL: &str = "edgion_config_relist_total";
    pub const CONFIG_EVENT_PARSE_ERRORS_TOTAL: &str = "edgion_config_event_parse_errors_total";
    // Backend request metrics (for LB testing and monitoring)
    pub const BACKEND_REQUESTS_TOTAL: &str = "edgion_backend_requests_total";
    // Gateway traffic byte counters
    pub const GATEWAY_REQUEST_BYTES: &str = "edgion_gateway_request_bytes_total";
    pub const GATEWAY_RESPONSE_BYTES: &str = "edgion_gateway_response_bytes_total";
    // Request mirror metrics (counter only — per-request latency is captured
    // in the mirror access log `et` field; mirror is fire-and-forget and its
    // tail latency does not affect user traffic, so no histogram is carried).
    pub const MIRROR_REQUESTS_TOTAL: &str = "edgion_mirror_requests_total";
    // End-to-end request duration histogram
    pub const GATEWAY_REQUEST_DURATION_MS: &str = "edgion_gateway_request_duration_ms";
    // Controller: number of gateway instances connected via gRPC long-poll
    pub const CONTROLLER_CONNECTED_GATEWAYS: &str = "edgion_controller_connected_gateways";
    // Controller: schema validation failures from Admin API (non-K8s mode)
    pub const SCHEMA_VALIDATION_ERRORS: &str = "edgion_controller_schema_validation_errors_total";
    // Circuit breaker metrics
    pub const CB_EJECTIONS_TOTAL: &str = "edgion_circuit_breaker_ejections_total";
    pub const CB_EJECTED_ENDPOINTS: &str = "edgion_circuit_breaker_ejected_endpoints";
    pub const CB_REJECTIONS_TOTAL: &str = "edgion_circuit_breaker_rejections_total";
    // ACME HTTP-01 challenge lookup metrics
    pub const ACME_CHALLENGE_LOOKUP_TOTAL: &str = "edgion_acme_challenge_lookup_total";
    // Stream plugin: GlobalConnectionIpRestriction decision events
    pub const CONN_IP_RESTRICTION_EVENTS_TOTAL: &str = "edgion_conn_ip_restriction_events_total";
    // RoundRobin selector: backend count > u16::MAX truncation
    pub const RR_SELECTOR_TRUNCATED_TOTAL: &str = "edgion_rr_selector_truncated_total";
    // Elasticsearch bulk producer: increments each time the producer→bulk-loop
    // mpsc channel is full at submit time (signal to raise `bulk.channelSize`).
    pub const LINK_SYS_ES_BULK_CHANNEL_FULL_TOTAL: &str = "edgion_link_sys_es_bulk_channel_full_total";
    // LDAP auth plugin: error counter by kind (unreachable / timeout / bind_fail)
    pub const LDAP_AUTH_ERRORS_TOTAL: &str = "edgion_ldap_auth_errors_total";
    // Webhook key resolver: per-resolve failure counter by reason
    pub const WEBHOOK_RESOLVE_ERRORS_TOTAL: &str = "edgion_webhook_resolve_errors_total";
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
///
/// Registration policy (hybrid):
/// - **Hot per-request** handles are eagerly registered in `new()` for zero
///   per-call overhead; they become non-zero within the first request so the
///   brief zero-value window is not observable in practice.
/// - **Cold / failure-path** handles are `OnceLock`-wrapped and registered on
///   first call, so metrics that are never touched do not appear in `/metrics`
///   as zero-value noise (cleaner scrape, smaller bandwidth).
pub struct GatewayMetrics {
    // --- Hot per-request handles (eager registration) ---
    /// Total contexts created (requests received)
    ctx_created: Counter,
    /// Currently active contexts
    ctx_active: Gauge,
    /// Total gateway request bytes proxied (downstream → upstream)
    gateway_request_bytes: Counter,
    /// Total gateway response bytes proxied (upstream → downstream)
    gateway_response_bytes: Counter,

    // --- Cold / rare-event handles (lazy registration via OnceLock) ---
    requests_total: OnceLock<Counter>,
    requests_failed: OnceLock<Counter>,
    access_log_dropped: OnceLock<Counter>,
    ssl_log_dropped: OnceLock<Counter>,
    tcp_log_dropped: OnceLock<Counter>,
    tls_log_dropped: OnceLock<Counter>,
    udp_log_dropped: OnceLock<Counter>,
    status_update_total: OnceLock<Counter>,
    status_update_failed: OnceLock<Counter>,
    status_update_skipped: OnceLock<Counter>,
    config_reload_signals: OnceLock<Counter>,
    config_relist_total: OnceLock<Counter>,
    controller_connected_gateways: OnceLock<Gauge>,
    schema_validation_errors: OnceLock<Counter>,
}

#[inline]
fn get_or_register_counter<'a>(slot: &'a OnceLock<Counter>, name: &'static str) -> &'a Counter {
    slot.get_or_init(|| counter!(name))
}

#[inline]
fn get_or_register_gauge<'a>(slot: &'a OnceLock<Gauge>, name: &'static str) -> &'a Gauge {
    slot.get_or_init(|| gauge!(name))
}

impl GatewayMetrics {
    /// Create a new GatewayMetrics instance.
    ///
    /// Hot per-request handles register immediately; cold handles stay
    /// uninitialized until their first `record_*` call.
    fn new() -> Self {
        Self {
            // Hot handles: eager
            ctx_created: counter!(names::CTX_CREATED),
            ctx_active: gauge!(names::CTX_ACTIVE),
            gateway_request_bytes: counter!(names::GATEWAY_REQUEST_BYTES),
            gateway_response_bytes: counter!(names::GATEWAY_RESPONSE_BYTES),
            // Cold handles: lazy
            requests_total: OnceLock::new(),
            requests_failed: OnceLock::new(),
            access_log_dropped: OnceLock::new(),
            ssl_log_dropped: OnceLock::new(),
            tcp_log_dropped: OnceLock::new(),
            tls_log_dropped: OnceLock::new(),
            udp_log_dropped: OnceLock::new(),
            status_update_total: OnceLock::new(),
            status_update_failed: OnceLock::new(),
            status_update_skipped: OnceLock::new(),
            config_reload_signals: OnceLock::new(),
            config_relist_total: OnceLock::new(),
            controller_connected_gateways: OnceLock::new(),
            schema_validation_errors: OnceLock::new(),
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
        get_or_register_counter(&self.requests_total, names::REQUESTS_TOTAL).increment(1);
    }

    /// Record a failed request
    #[inline]
    pub fn request_failed(&self) {
        get_or_register_counter(&self.requests_total, names::REQUESTS_TOTAL).increment(1);
        get_or_register_counter(&self.requests_failed, names::REQUESTS_FAILED).increment(1);
    }

    /// Record a dropped access log entry
    #[inline]
    pub fn access_log_dropped(&self) {
        get_or_register_counter(&self.access_log_dropped, names::ACCESS_LOG_DROPPED).increment(1);
    }

    /// Record a dropped SSL log entry
    #[inline]
    pub fn ssl_log_dropped(&self) {
        get_or_register_counter(&self.ssl_log_dropped, names::SSL_LOG_DROPPED).increment(1);
    }

    /// Record a dropped TCP log entry
    #[inline]
    pub fn tcp_log_dropped(&self) {
        get_or_register_counter(&self.tcp_log_dropped, names::TCP_LOG_DROPPED).increment(1);
    }

    /// Record a dropped TLS access log entry
    #[inline]
    pub fn tls_log_dropped(&self) {
        get_or_register_counter(&self.tls_log_dropped, names::TLS_LOG_DROPPED).increment(1);
    }

    /// Record a dropped UDP log entry
    #[inline]
    pub fn udp_log_dropped(&self) {
        get_or_register_counter(&self.udp_log_dropped, names::UDP_LOG_DROPPED).increment(1);
    }

    /// Record a successful K8s status update
    #[inline]
    pub fn status_update_success(&self) {
        get_or_register_counter(&self.status_update_total, names::STATUS_UPDATE_TOTAL).increment(1);
    }

    /// Record a failed K8s status update
    #[inline]
    pub fn status_update_failed(&self) {
        get_or_register_counter(&self.status_update_total, names::STATUS_UPDATE_TOTAL).increment(1);
        get_or_register_counter(&self.status_update_failed, names::STATUS_UPDATE_FAILED).increment(1);
    }

    /// Record a skipped K8s status update (no change needed)
    #[inline]
    pub fn status_update_skipped(&self) {
        get_or_register_counter(&self.status_update_skipped, names::STATUS_UPDATE_SKIPPED).increment(1);
    }

    /// Record a reload signal received from controller
    #[inline]
    pub fn config_reload_signal(&self) {
        get_or_register_counter(&self.config_reload_signals, names::CONFIG_RELOAD_SIGNALS).increment(1);
    }

    /// Record a config relist operation
    #[inline]
    pub fn config_relist(&self) {
        get_or_register_counter(&self.config_relist_total, names::CONFIG_RELIST_TOTAL).increment(1);
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
        get_or_register_gauge(
            &self.controller_connected_gateways,
            names::CONTROLLER_CONNECTED_GATEWAYS,
        )
        .increment(1.0);
    }

    /// Record a gateway instance disconnected from controller
    #[inline]
    pub fn gateway_disconnected(&self) {
        get_or_register_gauge(
            &self.controller_connected_gateways,
            names::CONTROLLER_CONNECTED_GATEWAYS,
        )
        .decrement(1.0);
    }

    /// Record a schema validation error from Admin API (non-K8s mode)
    #[inline]
    pub fn schema_validation_error(&self) {
        get_or_register_counter(&self.schema_validation_errors, names::SCHEMA_VALIDATION_ERRORS).increment(1);
    }
}

/// Record a backend request metric.
///
/// Labels are pushed conditionally based on `BackendRequestsLabels`, so
/// `/metrics` only carries the dimensions operators explicitly asked for.
/// `test_key` / `test_data` are also conditional on non-empty: they are
/// populated only when `integration-testing-mode` is on AND a Gateway
/// annotation opts in (see `pg_logging::record_request_metrics`), so in
/// production neither label appears in any time series.
///
/// # Arguments
/// * `cfg` - Per-label toggles from `MetricsConfig::backend_requests.labels`
/// * `gateway_ns` / `gateway_name` - Emitted when `cfg.gateway = true`
/// * `route_ns` / `route_name` - Emitted when `cfg.route = true`
/// * `backend_ns` / `backend_name` - Emitted when `cfg.backend = true`
/// * `protocol` - Protocol (http/grpc/websocket), emitted when `cfg.protocol = true`
/// * `status` - Status group (2xx/3xx/4xx/5xx/failed), emitted when `cfg.status = true`
/// * `test_key` / `test_data` - Emitted only when non-empty
#[inline]
#[allow(clippy::too_many_arguments)]
pub fn record_backend_request(
    cfg: &crate::core::gateway::cli::config::BackendRequestsLabels,
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
    // Pre-size: up to 6 resource labels + protocol + status + 2 test labels.
    let mut labels: Vec<(&'static str, String)> = Vec::with_capacity(10);
    if cfg.gateway {
        labels.push(("gateway_namespace", gateway_ns.to_string()));
        labels.push(("gateway_name", gateway_name.to_string()));
    }
    if cfg.route {
        labels.push(("route_namespace", route_ns.to_string()));
        labels.push(("route_name", route_name.to_string()));
    }
    if cfg.backend {
        labels.push(("backend_namespace", backend_ns.to_string()));
        labels.push(("backend_name", backend_name.to_string()));
    }
    if cfg.protocol {
        labels.push(("protocol", protocol.to_string()));
    }
    if cfg.status {
        labels.push(("status", status.to_string()));
    }
    if !test_key.is_empty() {
        labels.push(("test_key", test_key.to_string()));
    }
    if !test_data.is_empty() {
        labels.push(("test_data", test_data.to_string()));
    }
    counter!(names::BACKEND_REQUESTS_TOTAL, &labels[..]).increment(1);
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

/// Record a mirror request outcome.
///
/// Per-request latency used to be captured here as a `MIRROR_DURATION_MS`
/// histogram, but the histogram was broken since the initial commit
/// (record path recorded milliseconds into Prometheus's default
/// second-range buckets → every sample landed in `+Inf`). It has been
/// removed on the grounds that (a) mirror is fire-and-forget and its
/// tail latency does not affect user traffic; (b) the `channel_full`
/// and `timeout` statuses on this counter are a sharper "mirror
/// falling behind" signal than a latency percentile; (c) per-request
/// latency is still recorded in the mirror access log (`et` field) when
/// `mirror_log` is enabled, for any finer-grained investigation.
#[inline]
pub fn record_mirror_metric(status: &'static str) {
    counter!(
        names::MIRROR_REQUESTS_TOTAL,
        "status" => status,
    )
    .increment(1);
}

/// Record end-to-end request duration histogram.
///
/// Labels are conditionally included based on `RequestDurationConfig.labels`.
/// When a label is disabled, it is not written at all (not an empty string),
/// which truly reduces cardinality.
#[inline]
#[allow(clippy::too_many_arguments)]
pub fn record_request_duration(
    cfg: &crate::core::gateway::cli::config::RequestDurationConfig,
    gateway_namespace: &str,
    gateway_name: &str,
    route_namespace: &str,
    route_name: &str,
    backend_namespace: &str,
    backend_name: &str,
    protocol: &str,
    status: &str,
    duration_ms: f64,
) {
    if !cfg.enabled {
        return;
    }
    let labels = &cfg.labels;

    let mut label_vec: Vec<(&'static str, String)> = Vec::with_capacity(8);
    if labels.gateway_namespace {
        label_vec.push(("gateway_namespace", gateway_namespace.to_string()));
    }
    if labels.gateway_name {
        label_vec.push(("gateway_name", gateway_name.to_string()));
    }
    if labels.route_namespace {
        label_vec.push(("route_namespace", route_namespace.to_string()));
    }
    if labels.route_name {
        label_vec.push(("route_name", route_name.to_string()));
    }
    if labels.backend_namespace {
        label_vec.push(("backend_namespace", backend_namespace.to_string()));
    }
    if labels.backend_name {
        label_vec.push(("backend_name", backend_name.to_string()));
    }
    if labels.protocol {
        label_vec.push(("protocol", protocol.to_string()));
    }
    if labels.status {
        label_vec.push(("status", status.to_string()));
    }

    histogram!(names::GATEWAY_REQUEST_DURATION_MS, &label_vec).record(duration_ms);
}

/// Record a circuit breaker ejection event
pub fn record_cb_ejection(service_key: &str) {
    counter!(names::CB_EJECTIONS_TOTAL, "service" => service_key.to_string()).increment(1);
    gauge!(names::CB_EJECTED_ENDPOINTS, "service" => service_key.to_string()).increment(1.0);
}

/// Record a circuit breaker recovery event (ejection expired)
pub fn record_cb_recovery(service_key: &str) {
    gauge!(names::CB_EJECTED_ENDPOINTS, "service" => service_key.to_string()).decrement(1.0);
}

/// Record a circuit breaker rejection (request blocked by ejected backend)
pub fn record_cb_rejection(service_key: &str) {
    counter!(names::CB_REJECTIONS_TOTAL, "service" => service_key.to_string()).increment(1);
}

/// Record an ACME HTTP-01 challenge token lookup event.
///
/// `result` must be one of: `"hit"`, `"expired"`, `"domain_mismatch"`, `"not_found"`.
/// Low, fixed cardinality — safe as a label.
pub fn record_acme_challenge_lookup(result: &'static str) {
    counter!(names::ACME_CHALLENGE_LOOKUP_TOTAL, "result" => result).increment(1);
}

/// Record a watch-event JSON parse failure (per resource kind).
///
/// Triggered from `ClientCache::start_watch` when either the outer
/// `serde_json::from_str` (events array) or the per-event
/// `serde_json::from_value::<T>` deserialization fails. `kind` is sourced from
/// `T::kind_name()` which returns a `&'static str` from a closed set of
/// registered resource types, so label cardinality is bounded.
///
/// The control flow on parse failure remains "log and continue" — this counter
/// exists purely so operators can alert on schema drift between controller and
/// gateway (the most common real-world trigger) without relying on log scraping.
pub fn record_config_event_parse_error(kind: &'static str) {
    counter!(names::CONFIG_EVENT_PARSE_ERRORS_TOTAL, "kind" => kind).increment(1);
}

/// Record a `GlobalConnectionIpRestriction` stream plugin decision event.
///
/// `result` must be one of:
/// - `"fail_closed_pm_missing"` — referenced PluginMetadata not found; connection denied
/// - `"fail_closed_profile_missing"` — active profile not present in PM; connection denied
/// - `"denied"` — client IP blocked by active profile rules
///
/// Low, fixed cardinality — safe as a label.
pub fn record_conn_ip_restriction_event(result: &'static str) {
    counter!(names::CONN_IP_RESTRICTION_EVENTS_TOTAL, "result" => result).increment(1);
}

/// Record an Elasticsearch bulk channel "full" event.
///
/// Fires when the producer→bulk-loop mpsc channel is at capacity at submit
/// time. Current behaviour is to fall back to an awaiting send (so the access
/// log path backpressures rather than drops); the counter exists so operators
/// can detect that they should raise `bulk.channelSize` before the backpressure
/// becomes visible at the request layer.
pub fn record_link_sys_es_bulk_channel_full() {
    counter!(names::LINK_SYS_ES_BULK_CHANNEL_FULL_TOTAL).increment(1);
}

/// Record a `RoundRobinSelector` backend-count truncation.
///
/// Fires every time a selector is built with more than `u16::MAX` backends
/// (an extreme configuration anomaly). No labels — identifying the exact
/// route is best done via logs; the counter exists purely as an alarm signal
/// so operators can detect silent truncation.
pub fn record_rr_selector_truncated() {
    counter!(names::RR_SELECTOR_TRUNCATED_TOTAL).increment(1);
}

/// Record a LDAP auth plugin error event.
///
/// `kind` must be one of: `"unreachable"`, `"timeout"`, `"bind_fail"`.
/// Fixed cardinality — safe as a label.
pub fn record_ldap_auth_error(kind: &'static str) {
    counter!(names::LDAP_AUTH_ERRORS_TOTAL, "kind" => kind).increment(1);
}

/// Record a webhook key-resolver failure.
///
/// `reason` must be one of: `"not_found"`, `"unhealthy"`, `"rate_limited"`,
/// `"status_error"`, `"call_failed"`. Fixed cardinality — safe as a label.
pub fn record_webhook_resolve_error(reason: &'static str) {
    counter!(names::WEBHOOK_RESOLVE_ERRORS_TOTAL, "reason" => reason).increment(1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics_exporter_prometheus::PrometheusBuilder;

    /// Verifies the hybrid registration policy:
    /// - Hot per-request handles register in `new()` and appear immediately (with value 0).
    /// - Cold / failure-path handles stay absent until their record method is called.
    #[test]
    fn gateway_metrics_registration_policy() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();

        metrics::with_local_recorder(&recorder, || {
            // `new()` must run inside with_local_recorder so eager counter!/gauge!
            // macros bind to the prometheus test recorder (not the noop global).
            let m = GatewayMetrics::new();

            // --- Before any record call ---
            let rendered = handle.render();
            // Hot handles: eagerly registered, should be visible (value 0).
            assert!(
                rendered.contains("edgion_ctx_created_total"),
                "hot counter ctx_created must register eagerly, got:\n{rendered}"
            );
            assert!(
                rendered.contains("edgion_ctx_active"),
                "hot gauge ctx_active must register eagerly, got:\n{rendered}"
            );
            assert!(
                rendered.contains("edgion_gateway_request_bytes_total"),
                "hot counter request_bytes must register eagerly, got:\n{rendered}"
            );
            assert!(
                rendered.contains("edgion_gateway_response_bytes_total"),
                "hot counter response_bytes must register eagerly, got:\n{rendered}"
            );
            // Cold handles: absent until called.
            assert!(
                !rendered.contains("edgion_access_log_dropped_total"),
                "cold counter access_log_dropped must not appear before first call, got:\n{rendered}"
            );
            assert!(
                !rendered.contains("edgion_ssl_log_dropped_total"),
                "cold counter ssl_log_dropped must not appear before first call, got:\n{rendered}"
            );
            assert!(
                !rendered.contains("edgion_controller_schema_validation_errors_total"),
                "cold counter schema_validation_errors must not appear before first call, got:\n{rendered}"
            );

            // --- After touching one cold counter ---
            m.access_log_dropped();
            let rendered = handle.render();
            assert!(
                rendered.contains("edgion_access_log_dropped_total"),
                "touched cold counter must appear, got:\n{rendered}"
            );
            assert!(
                !rendered.contains("edgion_ssl_log_dropped_total"),
                "still-untouched cold counter must remain absent, got:\n{rendered}"
            );
        });
    }
}
