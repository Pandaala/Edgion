//! Edgion metrics definitions
//!
//! Centralized metrics for monitoring gateway performance.
//! Uses the `metrics` crate for thread-safe, high-performance counters.

use metrics::{counter, gauge, histogram, Counter, Gauge, Label};
use std::sync::{LazyLock, OnceLock};

use crate::plugins::StreamLayer;

/// Metric names as constants for consistency
pub mod names {
    pub const CTX_CREATED: &str = "edgion_ctx_created_total";
    pub const CTX_ACTIVE: &str = "edgion_ctx_active";
    pub const ACCESS_LOG_DROPPED: &str = "edgion_access_log_dropped_total";
    pub const SSL_LOG_DROPPED: &str = "edgion_ssl_log_dropped_total";
    pub const TCP_LOG_DROPPED: &str = "edgion_tcp_log_dropped_total";
    pub const TLS_LOG_DROPPED: &str = "edgion_tls_log_dropped_total";
    pub const UDP_LOG_DROPPED: &str = "edgion_udp_log_dropped_total";
    // Config sync metrics (client side)
    pub const CONFIG_RELOAD_SIGNALS: &str = "edgion_config_reload_signals_total";
    pub const CONFIG_RELIST_TOTAL: &str = "edgion_config_relist_total";
    pub const CONFIG_EVENT_PARSE_ERRORS_TOTAL: &str = "edgion_config_event_parse_errors_total";
    // Backend request metrics (for LB testing and monitoring)
    pub const BACKEND_REQUESTS_TOTAL: &str = "edgion_backend_requests_total";
    // Gateway traffic byte counters
    pub const GATEWAY_REQUEST_BYTES: &str = "edgion_gateway_request_bytes_total";
    pub const GATEWAY_RESPONSE_BYTES: &str = "edgion_gateway_response_bytes_total";
    // Optional GenAI token rate/trend counter. Fixed labels yield at most
    // 2 directions x 3 providers x 2 usage sources = 12 series.
    pub const GENAI_TOKENS_TOTAL: &str = "edgion_gateway_genai_tokens_total";
    // Request mirror metrics (counter only — per-request latency is captured
    // in the mirror access log `et` field; mirror is fire-and-forget and its
    // tail latency does not affect user traffic, so no histogram is carried).
    pub const MIRROR_REQUESTS_TOTAL: &str = "edgion_mirror_requests_total";
    // End-to-end request duration histogram
    pub const GATEWAY_REQUEST_DURATION_MS: &str = "edgion_gateway_request_duration_ms";
    // Circuit breaker metrics
    pub const CB_EJECTIONS_TOTAL: &str = "edgion_circuit_breaker_ejections_total";
    pub const CB_EJECTED_ENDPOINTS: &str = "edgion_circuit_breaker_ejected_endpoints";
    pub const CB_REJECTIONS_TOTAL: &str = "edgion_circuit_breaker_rejections_total";
    // Load-balancing panic mode: 1 = engaged, 0 = normal (per service)
    pub const LB_PANIC_MODE: &str = "edgion_lb_panic_mode";
    // ACME HTTP-01 challenge lookup metrics
    pub const ACME_CHALLENGE_LOOKUP_TOTAL: &str = "edgion_acme_challenge_lookup_total";
    // Stream plugin: GlobalConnectionIpRestriction decision events
    pub const CONN_IP_RESTRICTION_EVENTS_TOTAL: &str = "edgion_conn_ip_restriction_events_total";
    // IP restriction: inline deny CIDR entries skipped by partial-good compilation (blacklist hole)
    pub const IP_DENY_ENTRIES_SKIPPED_TOTAL: &str = "edgion_ip_deny_entries_skipped_total";
    // Config aggregation: a conflict silently fell back to a default (e.g. conflicting outbound CA defaults -> System)
    pub const CONFIG_CONFLICT_TOTAL: &str = "edgion_config_conflict_total";
    // Outbound: a caller-injected header was dropped for an invalid name/value (silent no-op)
    pub const OUTBOUND_INJECTED_HEADER_SKIPPED_TOTAL: &str = "edgion_outbound_injected_header_skipped_total";
    // HTTP plugin: RegionRoute overlay resolution events (overlay_missing / overlay_type_mismatch)
    pub const REGION_ROUTE_OVERLAY_EVENTS_TOTAL: &str = "edgion_region_route_overlay_events_total";
    // Stream plugin: ConnectionRateLimit rejections
    pub const CONNECTION_RATE_LIMIT_REJECTED_TOTAL: &str = "edgion_connection_rate_limit_rejected_total";
    // Stream plugin: ConnectionRateLimit Redis failure (fired on both fail-open and fail-close)
    pub const CONNECTION_RATE_LIMIT_REDIS_FAILURE_TOTAL: &str = "edgion_connection_rate_limit_redis_failure_total";
    // RoundRobin selector: backend count > u16::MAX truncation
    pub const RR_SELECTOR_TRUNCATED_TOTAL: &str = "edgion_rr_selector_truncated_total";
    // Elasticsearch bulk producer: increments each time the producer→bulk-loop
    // mpsc channel is full at submit time (signal to raise `bulk.channelSize`).
    pub const LINK_SYS_ES_BULK_CHANNEL_FULL_TOTAL: &str = "edgion_link_sys_es_bulk_channel_full_total";
    // Link-sys provider health gauge: 1=healthy, 0=unhealthy, per provider.
    pub const LINK_SYS_HEALTH: &str = "edgion_link_sys_health";
    // Link-sys health flip counter (monotonic): exposes sub-scrape flapping the gauge cannot.
    pub const LINK_SYS_HEALTH_FLIPS_TOTAL: &str = "edgion_link_sys_health_flips_total";
    // LDAP auth plugin: error counter by kind (unreachable / timeout / bind_fail / tls_san_mismatch)
    pub const LDAP_AUTH_ERRORS_TOTAL: &str = "edgion_ldap_auth_errors_total";
    // Outbound TLS SAN pinning rejection, by plugin. The alerting signal for a
    // misconfigured `validation.subjectAltNames` or a rotated peer certificate.
    pub const OUTBOUND_TLS_SAN_MISMATCH_TOTAL: &str = "edgion_gateway_outbound_tls_san_mismatch_total";
    // Webhook key resolver: per-resolve failure counter by reason
    pub const WEBHOOK_RESOLVE_ERRORS_TOTAL: &str = "edgion_webhook_resolve_errors_total";
    // Egress resolver: outbound address rejected by the SSRF egress policy, by reason
    pub const EGRESS_BLOCKED_TOTAL: &str = "edgion_outbound_egress_blocked_total";
    // Upstream authority render: request-time authority the gateway refused to
    // use, by reason ("render_empty" / "invalid_label").
    pub const UPSTREAM_AUTHORITY_REJECTED_TOTAL: &str = "edgion_gateway_upstream_authority_rejected_total";
    // Outbound call: terminal response accepted at the status layer but classified as
    // failure by a `success.body` predicate — the "200 OK but error in the body" case.
    pub const OUTBOUND_SUCCESS_PREDICATE_FAIL_TOTAL: &str = "edgion_outbound_success_predicate_fail_total";
    // Outbound call: a templated field (e.g. timeoutMsTemplate) failed to render to a
    // usable value and silently fell back to its static default. The data plane must
    // not log, so this counter is the only signal an operator gets.
    pub const OUTBOUND_TEMPLATE_FALLBACK_TOTAL: &str = "edgion_outbound_template_fallback_total";
    // Access-log backfill: docs re-ingested into ES from the fallback file,
    // and lines that failed to backfill (kept for retry). Cold/failure-path.
    pub const ACCESS_LOG_BACKFILL_INGESTED_TOTAL: &str = "edgion_access_log_backfill_ingested_total";
    pub const ACCESS_LOG_BACKFILL_FAILED_TOTAL: &str = "edgion_access_log_backfill_failed_total";
    // FIPS: 1 = the BoringSSL FIPS-validated crypto module is linked and enabled
    // in this binary, 0 = not present (non-FIPS build). Set once at startup.
    // This is the ONLY validated-module compliance signal.
    pub const FIPS_MODULE_ENABLED: &str = "edgion_gateway_fips_module_enabled";
    // 1 = the host kernel reports FIPS mode (/proc/sys/crypto/fips_enabled).
    // Left operand of the compliance alert:
    //   fips_host_enabled == 1 and fips_module_enabled == 0
    pub const FIPS_HOST_ENABLED: &str = "edgion_gateway_fips_host_enabled";
    // 1 = the host FIPS flag exists but could not be read (masked /proc). Both
    // the guardrail and the fips_policy=auto default are inert in this state.
    pub const FIPS_HOST_DETECT_ERROR: &str = "edgion_gateway_fips_host_detect_error";
    // 1 = the approved cipher/curve profile is in effect on downstream
    // listeners. Labeled `validated="true|false"` by whether the validated
    // module backs it. NOT a compliance signal on its own: validated="false"
    // means approved suites over an unvalidated module.
    pub const FIPS_POLICY_ACTIVE: &str = "edgion_gateway_fips_policy_active";
    // Wasm plugin host metrics. The `module` label is the human-readable
    // source descriptor (file path / url / "inline:<sha8>"), NOT the raw
    // cache key: bounded by the number of configured modules and readable.
    pub const WASM_MODULE_LOAD_TOTAL: &str = "edgion_wasm_module_load_total";
    pub const WASM_MODULE_COMPILE_DURATION_MS: &str = "edgion_wasm_module_compile_duration_ms";
    pub const WASM_MODULE_EVICTED_TOTAL: &str = "edgion_wasm_module_evicted_total";
    pub const WASM_INSTANCE_INSTANTIATE_TOTAL: &str = "edgion_wasm_instance_instantiate_total";
    pub const WASM_INSTANCE_DISCARD_TOTAL: &str = "edgion_wasm_instance_discard_total";
    pub const WASM_POOL_SIZE: &str = "edgion_wasm_pool_size";
    pub const WASM_HOOK_DURATION_MS: &str = "edgion_wasm_hook_duration_ms";
    pub const WASM_TRAP_TOTAL: &str = "edgion_wasm_trap_total";
    pub const WASM_FAIL_MODE_TOTAL: &str = "edgion_wasm_fail_mode_total";
    pub const WASM_CALLOUT_TOTAL: &str = "edgion_wasm_callout_total";
    // Capacity/sizing gauges (multi-tenant sizing observability).
    pub const WASM_MODULES_LOADED: &str = "edgion_wasm_modules_loaded";
    pub const WASM_SHARED_DATA_BYTES: &str = "edgion_wasm_shared_data_bytes";
    // A guest returned Action::Pause from the response-body hook, i.e. it expects
    // the standard proxy-wasm cumulative-buffer semantics this host does not
    // implement. Alert on this: such a guest silently observes only the current
    // chunk, not the whole body.
    pub const WASM_BODY_PAUSE_UNSUPPORTED_TOTAL: &str = "edgion_wasm_body_pause_unsupported_total";

    // Plugin policy enforcement (controller allowlist/denylist, spec
    // 2026-07-17). Labeled by qualified plugin name (bounded by policy size).
    pub const PLUGIN_POLICY_BLOCKED_TOTAL: &str = "edgion_gateway_plugin_policy_blocked_total";
    pub const PLUGIN_POLICY_BYPASSED_TOTAL: &str = "edgion_gateway_plugin_policy_bypassed_total";

    // Retry budget enforcement (`EdgionBackendTrafficPolicy.retryConstraint`,
    // spec 2026-07-21). No labels (D5): a per-service label would churn
    // unboundedly on Service creation/deletion. Backend attribution comes
    // from the per-request access log / `EdgionStatus`, already service-scoped.
    pub const RETRY_BUDGET_DENIED_TOTAL: &str = "edgion_gateway_retry_budget_denied_total";
    // Circuit-breaker admit-time CONCURRENCY-CAP shed (backends/circuit_breaker/mod.rs
    // `CbPermit::try_acquire` returning None). This is NOT the outlier-detection
    // ejection/rejection family above (`CB_EJECTIONS_TOTAL`, `CB_EJECTED_ENDPOINTS`,
    // `CB_REJECTIONS_TOTAL`) — those trip on backend health; this one trips on a
    // per-endpoint in-flight-count cap, independent of health. Labelled `service` only.
    pub const CB_SHED_TOTAL: &str = "edgion_gateway_circuit_breaker_shed_total";

    // GeoIP country restriction metrics.
    pub const GEOIP_DB_LOADED: &str = "edgion_geoip_db_loaded";
    pub const GEOIP_DB_BUILD_EPOCH: &str = "edgion_geoip_db_build_epoch_seconds";
    pub const GEOIP_UPDATE_FAILED: &str = "edgion_geoip_update_failed_total";
    pub const GEOIP_DENIED: &str = "edgion_geoip_denied_total";
    /// Distinct from `GEOIP_UPDATE_FAILED`: that metric is about the
    /// background updater task, never on the per-request hot path (see the
    /// comment on the GeoIP metrics block below). A lookup panic happens
    /// inside `GeoIpProvider::lookup`, which every geo-consuming request
    /// calls — recording it under the updater's counter would be both
    /// mislabeled (its name and reason set are about update *cycles*, not
    /// per-request lookups) and would put a per-request event under a
    /// nominally-cold metric.
    pub const GEOIP_LOOKUP_PANIC: &str = "edgion_geoip_lookup_panic_total";
}

/// Global metrics singleton
static GLOBAL_METRICS: LazyLock<GatewayMetrics> = LazyLock::new(GatewayMetrics::new);

/// Get the global metrics instance
pub fn global_metrics() -> &'static GatewayMetrics {
    &GLOBAL_METRICS
}

/// Whether a provider reported token counts or Edgion estimated them from
/// observed characters. Fixed enum values keep the metric label bounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenAiUsageSource {
    Reported,
    Estimated,
}

impl GenAiUsageSource {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Reported => "reported",
            Self::Estimated => "estimated",
        }
    }
}

/// Record one finalized AI request's input and output token counts.
///
/// The disabled branch returns before invoking `counter!`, so the family is
/// neither registered nor emitted by a default-configured Gateway. Provider
/// and usage source are enums, direction is hard-coded, and model is
/// deliberately absent: the complete family is bounded to twelve series.
#[inline]
pub fn record_genai_tokens(
    enabled: bool,
    provider: crate::ai::AiProviderLabel,
    usage_source: GenAiUsageSource,
    input_tokens: u64,
    output_tokens: u64,
) {
    if !enabled {
        return;
    }
    let provider = provider.as_str();
    let usage_source = usage_source.as_str();
    counter!(
        names::GENAI_TOKENS_TOTAL,
        "direction" => "input",
        "provider" => provider,
        "usage_source" => usage_source,
    )
    .increment(input_tokens);
    counter!(
        names::GENAI_TOKENS_TOTAL,
        "direction" => "output",
        "provider" => provider,
        "usage_source" => usage_source,
    )
    .increment(output_tokens);
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
    access_log_dropped: OnceLock<Counter>,
    ssl_log_dropped: OnceLock<Counter>,
    tcp_log_dropped: OnceLock<Counter>,
    tls_log_dropped: OnceLock<Counter>,
    udp_log_dropped: OnceLock<Counter>,
    access_log_backfill_ingested: OnceLock<Counter>,
    access_log_backfill_failed: OnceLock<Counter>,
    config_reload_signals: OnceLock<Counter>,
    config_relist_total: OnceLock<Counter>,
}

#[inline]
fn get_or_register_counter<'a>(slot: &'a OnceLock<Counter>, name: &'static str) -> &'a Counter {
    slot.get_or_init(|| counter!(name))
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
            access_log_dropped: OnceLock::new(),
            ssl_log_dropped: OnceLock::new(),
            tcp_log_dropped: OnceLock::new(),
            tls_log_dropped: OnceLock::new(),
            udp_log_dropped: OnceLock::new(),
            access_log_backfill_ingested: OnceLock::new(),
            access_log_backfill_failed: OnceLock::new(),
            config_reload_signals: OnceLock::new(),
            config_relist_total: OnceLock::new(),
        }
    }

    /// Record a new context creation
    #[inline]
    pub fn ctx_created(&self) {
        self.ctx_created.increment(1);
        self.ctx_active.increment(1.0);
    }

    /// Record a dropped access log entry
    #[inline]
    pub fn access_log_dropped(&self) {
        get_or_register_counter(&self.access_log_dropped, names::ACCESS_LOG_DROPPED).increment(1);
    }

    /// Count documents successfully re-ingested into ES during a backfill run.
    pub fn access_log_backfill_ingested(&self, n: u64) {
        get_or_register_counter(
            &self.access_log_backfill_ingested,
            names::ACCESS_LOG_BACKFILL_INGESTED_TOTAL,
        )
        .increment(n);
    }

    /// Count lines that failed to backfill (the file is kept for a later retry).
    pub fn access_log_backfill_failed(&self, n: u64) {
        get_or_register_counter(
            &self.access_log_backfill_failed,
            names::ACCESS_LOG_BACKFILL_FAILED_TOTAL,
        )
        .increment(n);
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
    cfg: &crate::cli::config::BackendRequestsLabels,
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
    // Build a `Vec<metrics::Label>` directly so the registry's `IntoLabels`
    // impl can move it in place; this avoids the per-call `Vec::collect()`
    // that the `(&str, String)` slice form would trigger inside `counter!`.
    let mut labels: Vec<Label> = Vec::with_capacity(10);
    if cfg.gateway {
        labels.push(Label::new("gateway_namespace", gateway_ns.to_string()));
        labels.push(Label::new("gateway_name", gateway_name.to_string()));
    }
    if cfg.route {
        labels.push(Label::new("route_namespace", route_ns.to_string()));
        labels.push(Label::new("route_name", route_name.to_string()));
    }
    if cfg.backend {
        labels.push(Label::new("backend_namespace", backend_ns.to_string()));
        labels.push(Label::new("backend_name", backend_name.to_string()));
    }
    if cfg.protocol {
        labels.push(Label::new("protocol", protocol.to_string()));
    }
    if cfg.status {
        labels.push(Label::new("status", status.to_string()));
    }
    if !test_key.is_empty() {
        labels.push(Label::new("test_key", test_key.to_string()));
    }
    if !test_data.is_empty() {
        labels.push(Label::new("test_data", test_data.to_string()));
    }
    counter!(names::BACKEND_REQUESTS_TOTAL, labels).increment(1);
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
/// No latency histogram: mirror is fire-and-forget so its tail latency does
/// not affect user traffic; the `body_abandoned` / `channel_full` / `timeout`
/// statuses on this counter are sharper failure signals than a percentile.
/// Per-request latency is in the mirror access log `et` field when
/// `mirror_log` is enabled.
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
    cfg: &crate::cli::config::RequestDurationConfig,
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

    // Build a `Vec<metrics::Label>` directly so the registry's `IntoLabels`
    // impl can move it in place; this avoids the per-call `Vec::collect()`
    // that the `(&str, String)` slice form would trigger inside `histogram!`.
    let mut label_vec: Vec<Label> = Vec::with_capacity(8);
    if labels.gateway_namespace {
        label_vec.push(Label::new("gateway_namespace", gateway_namespace.to_string()));
    }
    if labels.gateway_name {
        label_vec.push(Label::new("gateway_name", gateway_name.to_string()));
    }
    if labels.route_namespace {
        label_vec.push(Label::new("route_namespace", route_namespace.to_string()));
    }
    if labels.route_name {
        label_vec.push(Label::new("route_name", route_name.to_string()));
    }
    if labels.backend_namespace {
        label_vec.push(Label::new("backend_namespace", backend_namespace.to_string()));
    }
    if labels.backend_name {
        label_vec.push(Label::new("backend_name", backend_name.to_string()));
    }
    if labels.protocol {
        label_vec.push(Label::new("protocol", protocol.to_string()));
    }
    if labels.status {
        label_vec.push(Label::new("status", status.to_string()));
    }

    histogram!(names::GATEWAY_REQUEST_DURATION_MS, label_vec).record(duration_ms);
}

/// Record a circuit breaker ejection event. `class` is the bounded failure-class
/// label (`gateway` | `application` | `local_origin`) identifying which counter
/// tripped, so operators can split local-origin (data-plane connect/proxy) ejections
/// from upstream-origin 5xx ejections. The gauge stays unlabelled by class so its
/// +1/-1 balance with `record_cb_recovery` (which has no class context) holds.
pub fn record_cb_ejection(service_key: &str, class: &'static str) {
    counter!(names::CB_EJECTIONS_TOTAL, "service" => service_key.to_string(), "class" => class).increment(1);
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

/// Set the panic-mode gauge for a service: 1.0 when panic mode is engaged
/// (health filtering bypassed), 0.0 when selecting normally. Called from the
/// selection path; cheap (atomic gauge set), no logging — Iron Rule 3 safe.
pub fn record_lb_panic_mode(service_key: &str, engaged: bool) {
    gauge!(names::LB_PANIC_MODE, "service" => service_key.to_string()).set(if engaged { 1.0 } else { 0.0 });
}

/// Set the link-sys provider health gauge: 1.0 = healthy, 0.0 = unhealthy.
///
/// Called on a healthy<->unhealthy transition and once at init() to establish a
/// `=1` baseline. Free-function `gauge!` (like `record_lb_panic_mode`) keeps the
/// series absent from `/metrics` until first emit. `link` cardinality = provider count.
pub fn record_link_sys_health(name: &str, healthy: bool) {
    gauge!(names::LINK_SYS_HEALTH, "link" => name.to_string()).set(if healthy { 1.0 } else { 0.0 });
}

/// Increment the link-sys health flip counter, labelled by provider and direction.
///
/// A level gauge cannot represent sub-scrape flapping (it only shows the last value
/// at scrape time); this monotonic counter does. `direction` is a fixed 2-value set.
pub fn record_link_sys_health_flip(name: &str, to_healthy: bool) {
    counter!(
        names::LINK_SYS_HEALTH_FLIPS_TOTAL,
        "link" => name.to_string(),
        "direction" => if to_healthy { "recovered" } else { "degraded" }
    )
    .increment(1);
}

/// Record an ACME HTTP-01 challenge token lookup event.
///
/// `result` must be one of: `"hit"`, `"expired"`, `"domain_mismatch"`, `"not_found"`, `"empty_host"`.
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

/// Record a RegionRoute overlay resolution event.
///
/// `result` must be one of:
/// - `"overlay_missing"` — `overrideRef` set but the EdgionConfigData key is not found in the store;
///   base regions used (fail-to-base).
/// - `"overlay_disabled"` — EdgionConfigData found but `spec.enable=false` (envelope kill-switch);
///   base regions used without consulting the overlay variant (H1).
/// - `"overlay_type_mismatch"` — EdgionConfigData found but its variant is not `RegionRouteOverride`
///   (D6 type guard rejected it); base regions used.
/// - `"overlay_invalid"` — overlay applied but the merged effective region table failed
///   `validate_regions` (e.g. duplicate name, reversed hash range); base regions used (H2).
///
/// Low, fixed cardinality — safe as a label.
pub fn record_region_route_overlay_event(result: &'static str) {
    counter!(names::REGION_ROUTE_OVERLAY_EVENTS_TOTAL, "result" => result).increment(1);
}

/// Record a `GlobalConnectionIpRestriction` stream plugin decision event.
///
/// `result` must be one of:
/// - `"overlay_missing"` — `activeProfileRef` set but EdgionConfigData not found; base used (fail-to-base)
/// - `"overlay_disabled"` — EdgionConfigData found but `spec.enable=false` (envelope kill-switch);
///   base used without consulting the selector (H1)
/// - `"overlay_type_mismatch"` — EdgionConfigData present but not a `Selector` variant (D6); base used
/// - `"overlay_invalid"` — Selector names a profile not in the config's `profiles`; base used
/// - `"fail_closed_profile_missing"` — resolved active profile not in `profiles` (defensive guard)
/// - `"denied"` — client IP blocked by the active profile rules
///
/// Low, fixed cardinality — safe as a label.
pub fn record_conn_ip_restriction_event(result: &'static str) {
    counter!(names::CONN_IP_RESTRICTION_EVENTS_TOTAL, "result" => result).increment(1);
}

/// Record that one or more inline `deny` CIDR entries were skipped by partial-good
/// compilation. A skipped deny entry is a hole in a blacklist (under-block / fail-open
/// direction), so it gets a loud counter — not just a passive status condition.
/// `scope` is a low-cardinality `&'static str`: "http" | "stream_l4" | "tls_route".
pub fn record_ip_deny_entries_skipped(scope: &'static str) {
    counter!(names::IP_DENY_ENTRIES_SKIPPED_TOTAL, "scope" => scope).increment(1);
}

/// A config-aggregation conflict silently fell back to a default. `field` is a bounded
/// constant `&'static str` naming which aggregate conflicted (e.g. "outbound_ca").
/// Zero-event sentinel: it stays absent until an operator ships genuinely conflicting
/// configs, at which point the resolved value is order-independent but non-obvious.
pub fn record_config_conflict(field: &'static str) {
    counter!(names::CONFIG_CONFLICT_TOTAL, "field" => field).increment(1);
}

/// A caller-injected outbound header (e.g. X-Forwarded-* from ForwardAuth) was dropped
/// at request build because its name or value was invalid — the operator's configured
/// header silently no-ops. Label-free zero-event sentinel.
pub fn record_injected_header_skipped() {
    counter!(names::OUTBOUND_INJECTED_HEADER_SKIPPED_TOTAL).increment(1);
}

/// Record a connection rejected by the ConnectionRateLimit stream plugin.
///
/// `dimension` must be `"listener"` or `"source_ip"`.
/// `stage` must be `"local"` or `"redis"`.
/// Both are fixed, low-cardinality `&'static str` — safe as labels.
pub fn record_connection_rate_limit_reject(dimension: &'static str, stage: &'static str) {
    counter!(
        names::CONNECTION_RATE_LIMIT_REJECTED_TOTAL,
        "dimension" => dimension,
        "stage" => stage
    )
    .increment(1);
}

/// Record a Redis failure encountered by the ConnectionRateLimit stream plugin,
/// fired regardless of the resulting Allow/Deny outcome so that fail-open is
/// observable (unlike the rejection counter, which only fires on Deny).
///
/// `dimension` must be `"listener"` or `"source_ip"`.
/// `cause` must be `"unavailable"` (client missing / not healthy) or `"runtime"`
/// (command error or accept-path timeout). Both are fixed, low-cardinality
/// `&'static str` — safe as labels.
pub fn record_connection_rate_limit_redis_failure(dimension: &'static str, cause: &'static str) {
    counter!(
        names::CONNECTION_RATE_LIMIT_REDIS_FAILURE_TOTAL,
        "dimension" => dimension,
        "cause" => cause
    )
    .increment(1);
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
/// `kind` must be one of: `"unreachable"`, `"timeout"`, `"bind_fail"`,
/// `"tls_san_mismatch"`. Fixed cardinality — safe as a label.
pub fn record_ldap_auth_error(kind: &'static str) {
    counter!(names::LDAP_AUTH_ERRORS_TOTAL, "kind" => kind).increment(1);
}

/// Record an outbound TLS SAN-pinning rejection.
///
/// `plugin` must be one of: `"ldap_auth"`, `"openid_connect"`.
/// Fixed cardinality — safe as a label.
pub fn record_outbound_tls_san_mismatch(plugin: &'static str) {
    counter!(names::OUTBOUND_TLS_SAN_MISMATCH_TOTAL, "plugin" => plugin).increment(1);
}

/// Record a webhook key-resolver failure.
///
/// `reason` must be one of: `"not_found"`, `"unhealthy"`, `"rate_limited"`,
/// `"status_error"`, `"call_failed"`, `"authority_rejected"`. Fixed cardinality —
/// safe as a label.
pub fn record_webhook_resolve_error(reason: &'static str) {
    counter!(names::WEBHOOK_RESOLVE_ERRORS_TOTAL, "reason" => reason).increment(1);
}

/// Record an outbound address rejected by the egress policy.
///
/// `reason` is one of `"loopback"`, `"link_local"`, `"private"`, `"unspecified"`.
pub fn record_egress_blocked(reason: &'static str) {
    counter!(names::EGRESS_BLOCKED_TOTAL, "reason" => reason).increment(1);
}

/// Record an upstream authority the gateway refused to use. `reason` is
/// `"render_empty"` (the template produced nothing — usually a `${ctx:…}` written
/// by a plugin that runs after `upstream_peer`) or `"invalid_label"` (the rendered
/// label is not a valid DNS label — a broken template, or an attack).
pub fn record_authority_rejected(reason: &'static str) {
    counter!(names::UPSTREAM_AUTHORITY_REJECTED_TOTAL, "reason" => reason).increment(1);
}

/// Record a response whose status was accepted but whose `success.body` predicate
/// classified it as a failure — the "200 OK but error in the body" case, which is
/// otherwise invisible (the data plane must not log).
pub fn record_outbound_success_predicate_fail() {
    counter!(names::OUTBOUND_SUCCESS_PREDICATE_FAIL_TOTAL).increment(1);
}

/// Record a template field that failed to render to a usable value and fell back
/// to its static default. `field` is `"timeout_ms"`.
/// Fixed cardinality — safe as a label. The data plane must not log, so this
/// counter is the only signal an operator gets for a silently-falling-back template.
pub fn record_outbound_template_fallback(field: &'static str) {
    counter!(names::OUTBOUND_TEMPLATE_FALLBACK_TOTAL, "field" => field).increment(1);
}

// ============================================================
// Wasm plugin host metrics
// ============================================================
//
// All Wasm metrics carry a `module` label: the human-readable source
// descriptor produced by `WasmSourceSpec::metrics_label()` (file path, url,
// or `inline:<sha8>`). Cardinality is bounded by the number of distinct
// configured modules.

/// Record a Wasm module load attempt outcome. `result` is `"ok"` or `"error"`.
pub fn record_wasm_module_load(module: &str, result: &'static str) {
    counter!(names::WASM_MODULE_LOAD_TOTAL, "module" => module.to_string(), "result" => result).increment(1);
}

/// Record the wall-clock duration of one `Module::new` compilation.
pub fn record_wasm_compile_duration(module: &str, ms: f64) {
    histogram!(names::WASM_MODULE_COMPILE_DURATION_MS, "module" => module.to_string()).record(ms);
}

/// Record a module-cache entry eviction (refcount reached zero and the grace
/// period elapsed without re-registration).
pub fn record_wasm_module_evicted(module: &str) {
    counter!(names::WASM_MODULE_EVICTED_TOTAL, "module" => module.to_string()).increment(1);
}

/// Record a fresh VM instantiation (pool miss).
pub fn record_wasm_instance_instantiate(module: &str) {
    counter!(names::WASM_INSTANCE_INSTANTIATE_TOTAL, "module" => module.to_string()).increment(1);
}

/// Record a VM discarded instead of returned to the pool.
/// `reason` is `"poisoned"` or `"over_capacity"`.
pub fn record_wasm_instance_discard(module: &str, reason: &'static str) {
    counter!(names::WASM_INSTANCE_DISCARD_TOTAL, "module" => module.to_string(), "reason" => reason).increment(1);
}

/// Set the current pooled-instance count for a module.
pub fn set_wasm_pool_size(module: &str, size: usize) {
    gauge!(names::WASM_POOL_SIZE, "module" => module.to_string()).set(size as f64);
}

/// Record the duration of one guest hook invocation.
/// `phase` is one of `"request_headers"`, `"response_headers"`,
/// `"response_body"`, `"http_call_response"`, `"tick"`.
pub fn record_wasm_hook_duration(module: &str, phase: &'static str, ms: f64) {
    histogram!(names::WASM_HOOK_DURATION_MS, "module" => module.to_string(), "phase" => phase).record(ms);
}

/// Record a guest trap (or protocol violation that poisons the lease).
pub fn record_wasm_trap(module: &str, phase: &'static str) {
    counter!(names::WASM_TRAP_TOTAL, "module" => module.to_string(), "phase" => phase).increment(1);
}

/// Record a fail-mode decision taken by the Wasm plugin.
/// `reason` is one of `"load"`, `"acquire"`, `"trap"`, `"not loaded"`,
/// `"callout"`, `"no-resume"`; `mode` is `"open"` or `"close"`.
pub fn record_wasm_fail_mode(module: &str, reason: &str, mode: &'static str) {
    counter!(
        names::WASM_FAIL_MODE_TOTAL,
        "module" => module.to_string(),
        "reason" => reason.to_string(),
        "mode" => mode
    )
    .increment(1);
}

/// Record a guest-dispatched HTTP callout outcome. `result` is one of
/// `"ok"`, `"error"`, `"denied"`, `"over_limit"`, `"tick_config_conflict"`
/// (tick-phase dispatch left disabled: plugins sharing the module configure
/// different callout allowlists).
pub fn record_wasm_callout(module: &str, result: &'static str) {
    counter!(names::WASM_CALLOUT_TOTAL, "module" => module.to_string(), "result" => result).increment(1);
}

/// Set the number of compiled modules currently held in the global cache.
/// Multiplied by `instancePoolSize` this bounds pooling-allocator sizing.
pub fn set_wasm_modules_loaded(count: usize) {
    gauge!(names::WASM_MODULES_LOADED).set(count as f64);
}

/// Set the current shared-data byte total for a module.
pub fn set_wasm_shared_data_bytes(module: &str, bytes: usize) {
    gauge!(names::WASM_SHARED_DATA_BYTES, "module" => module.to_string()).set(bytes as f64);
}

/// Record a guest returning `Action::Pause` from the response-body hook.
///
/// This host implements the body hook as a per-chunk read-only *window*, not the
/// spec's cumulative buffer, so Pause cannot be honored. A guest that pauses is
/// written against the standard buffering idiom and will silently observe only
/// the current chunk instead of the whole body — alert on this counter.
pub fn record_wasm_body_pause_unsupported(module: &str) {
    counter!(names::WASM_BODY_PAUSE_UNSUPPORTED_TOTAL, "module" => module.to_string()).increment(1);
}

// ============================================================
// Plugin policy enforcement metrics
// ============================================================
//
// Emitted by `policy_filter::PolicyBlockFilter` / `PolicyBypassNote`
// (edgion-gateway/src/plugins/runtime/policy_filter.rs) when a request's
// chain contains a controller-stamped `PluginPolicyAction`. `plugin` is the
// qualified name (`http/<TypeName>`); cardinality is bounded by the number
// of distinct plugin types the operator's policy touches.

/// A request was rejected because its chain contains a Block-stamped plugin.
pub fn record_plugin_policy_blocked(plugin: &str) {
    counter!(names::PLUGIN_POLICY_BLOCKED_TOTAL, "plugin" => plugin.to_string()).increment(1);
}

/// A request passed through a chain with a Bypass-stamped plugin skipped.
pub fn record_plugin_policy_bypassed(plugin: &str) {
    counter!(names::PLUGIN_POLICY_BYPASSED_TOTAL, "plugin" => plugin.to_string()).increment(1);
}

// ============================================================
// Retry budget enforcement metrics
// ============================================================
//
// Emitted when a retry is refused because the target service's
// `retryConstraint` budget (percentage ceiling OR minimum-rate floor) is
// exhausted. Label-less by design (D5): a per-service label would mint an
// unbounded number of series as Services churn (create/delete), retained
// server-side for the whole retention window. Which backend was throttled
// is read from the per-request access log / `EdgionStatus`, both already
// service-scoped.

/// A retry attempt was refused by a `retryConstraint` budget.
pub fn record_retry_budget_denied() {
    counter!(names::RETRY_BUDGET_DENIED_TOTAL).increment(1);
}

// ============================================================
// Circuit breaker concurrency-cap shed metric
// ============================================================
//
// Distinct from the outlier-detection ejection/rejection family further
// above (`CB_EJECTIONS_TOTAL` / `CB_EJECTED_ENDPOINTS` / `CB_REJECTIONS_TOTAL`,
// which trip on backend health): this counter fires when
// `backends::circuit_breaker::CbPermit::try_acquire` finds the chosen
// endpoint already at its configured in-flight concurrency cap and the
// request is shed before the upstream connect is attempted.

/// A request was shed by the per-endpoint circuit-breaker concurrency cap.
/// Labelled by `service` only — never per-endpoint or per-request — to stay
/// low-cardinality.
pub fn record_cb_shed(service_key: &str) {
    counter!(names::CB_SHED_TOTAL, "service" => service_key.to_string()).increment(1);
}

// ============================================================
// GeoIP country restriction metrics
// ============================================================
//
// `set_geoip_db_loaded`, `set_geoip_db_build_epoch`, `record_geoip_update_failed`,
// and `record_geoip_denied` are not on the per-request hot path (the first two
// are process-lifecycle state, the latter two fire from the background updater
// and from an L4 connection-filter deny path respectively), so all four follow
// the cold free-function `gauge!`/`counter!` pattern (like `record_lb_panic_mode`
// / `record_link_sys_health` above): the series is absent from `/metrics` until
// the first call. `record_geoip_lookup_panic`, below, is the one exception in
// this block — it fires from `GeoIpProvider::lookup`, which every geo-consuming
// request calls; it uses the same cold-registration pattern anyway since a
// panic inside the MMDB reader is expected to be exceedingly rare in practice.

/// Set to 1 when a database is loaded, 0 otherwise.
/// Alert on `edgion_geoip_db_loaded == 0` while geo rules are configured: that
/// is the dangerous silent failure — the control is not running.
pub fn set_geoip_db_loaded(loaded: bool) {
    gauge!(names::GEOIP_DB_LOADED).set(if loaded { 1.0 } else { 0.0 });
}

/// The loaded database's build timestamp. Alert on staleness.
pub fn set_geoip_db_build_epoch(epoch: u64) {
    gauge!(names::GEOIP_DB_BUILD_EPOCH).set(epoch as f64);
}

/// `reason` is a bounded enum, never free text:
/// fetch | encoding | size_cap | stall | validation | rename | disk | deadline.
/// This is strictly about the background *update cycle*; a per-request MMDB
/// lookup panic is a distinct, per-request event recorded by
/// `record_geoip_lookup_panic` instead, not folded in here.
pub fn record_geoip_update_failed(reason: &'static str) {
    counter!(names::GEOIP_UPDATE_FAILED, "reason" => reason).increment(1);
}

/// A per-request MMDB lookup inside `GeoIpProvider::lookup` panicked (caught
/// via `catch_unwind`) and degraded to `GeoLookup::NotFound`. Label-free: this
/// is a data-integrity signal about the loaded database, not something to
/// slice by request attributes.
pub fn record_geoip_lookup_panic() {
    counter!(names::GEOIP_LOOKUP_PANIC).increment(1);
}

/// L4 geo denials, which are never logged (see the design's §6.6).
/// `layer` is stage1 | tcp | tls; `reason` is deny_country | allow_miss | db_unavailable.
/// Neither label carries a client address.
pub fn record_geoip_denied(layer: StreamLayer, reason: &'static str) {
    counter!(names::GEOIP_DENIED, "layer" => layer.as_metric_label(), "reason" => reason).increment(1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics_exporter_prometheus::PrometheusBuilder;

    #[test]
    fn config_conflict_counter_increments() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            record_config_conflict("outbound_ca");
            let rendered = handle.render();
            assert!(
                rendered.contains("edgion_config_conflict_total") && rendered.contains("field=\"outbound_ca\""),
                "config_conflict counter must render with field label, got:\n{rendered}"
            );
        });
    }

    #[test]
    fn injected_header_skipped_counter_increments() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            record_injected_header_skipped();
            let rendered = handle.render();
            assert!(
                rendered.contains("edgion_outbound_injected_header_skipped_total"),
                "injected_header_skipped counter must render, got:\n{rendered}"
            );
        });
    }

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

    #[test]
    fn record_backend_request_emits_expected_labels_under_default_cfg() {
        use crate::cli::config::BackendRequestsLabels;
        use metrics_util::debugging::DebuggingRecorder;

        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let cfg = BackendRequestsLabels::default(); // gateway + protocol + status
        metrics::with_local_recorder(&recorder, || {
            record_backend_request(
                &cfg,
                "gw_ns",
                "gw_a",
                "route_ns",
                "route_x",
                "backend_ns",
                "svc_x",
                "http",
                "2xx",
                "",
                "",
            );
        });
        let snap = snapshotter.snapshot().into_vec();
        let key = snap
            .iter()
            .find(|(k, _, _, _)| k.key().name() == names::BACKEND_REQUESTS_TOTAL)
            .expect("counter registered");
        let labels: Vec<(&str, &str)> = key.0.key().labels().map(|l| (l.key(), l.value())).collect();
        assert!(labels.contains(&("gateway_namespace", "gw_ns")));
        assert!(labels.contains(&("gateway_name", "gw_a")));
        assert!(labels.contains(&("protocol", "http")));
        assert!(labels.contains(&("status", "2xx")));
        // route_namespace / backend_namespace must NOT appear under default cfg
        assert!(!labels.iter().any(|(k, _)| *k == "route_namespace"));
        assert!(!labels.iter().any(|(k, _)| *k == "backend_namespace"));
    }

    #[test]
    fn geoip_metric_names_are_declared() {
        assert_eq!(names::GEOIP_DB_LOADED, "edgion_geoip_db_loaded");
        assert_eq!(names::GEOIP_DB_BUILD_EPOCH, "edgion_geoip_db_build_epoch_seconds");
        assert_eq!(names::GEOIP_UPDATE_FAILED, "edgion_geoip_update_failed_total");
        assert_eq!(names::GEOIP_DENIED, "edgion_geoip_denied_total");
        assert_eq!(names::GEOIP_LOOKUP_PANIC, "edgion_geoip_lookup_panic_total");
    }

    /// Regression test: a lookup panic must land on its own dedicated metric,
    /// never on `edgion_geoip_update_failed_total` — that counter's `reason`
    /// label set is about the background update cycle, and folding a
    /// per-request lookup panic into it would mislabel a per-request event as
    /// an updater-cycle failure.
    #[test]
    fn geoip_lookup_panic_uses_its_own_metric_not_update_failed() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            record_geoip_lookup_panic();
            let rendered = handle.render();
            assert!(
                rendered.contains("edgion_geoip_lookup_panic_total"),
                "lookup panic must render under its own metric, got:\n{rendered}"
            );
            assert!(
                !rendered.contains("edgion_geoip_update_failed_total"),
                "lookup panic must NOT touch edgion_geoip_update_failed_total, got:\n{rendered}"
            );
        });
    }

    #[test]
    fn fips_metric_names_are_stable() {
        // These names are load-bearing: the compliance alert
        //   fips_host_enabled == 1 and fips_module_enabled == 0
        // breaks silently if either is renamed.
        assert_eq!(names::FIPS_MODULE_ENABLED, "edgion_gateway_fips_module_enabled");
        assert_eq!(names::FIPS_HOST_ENABLED, "edgion_gateway_fips_host_enabled");
        assert_eq!(names::FIPS_HOST_DETECT_ERROR, "edgion_gateway_fips_host_detect_error");
        assert_eq!(names::FIPS_POLICY_ACTIVE, "edgion_gateway_fips_policy_active");
    }

    #[test]
    fn genai_metric_disabled_does_not_register_family() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            record_genai_tokens(
                false,
                crate::ai::AiProviderLabel::OpenAi,
                GenAiUsageSource::Reported,
                11,
                7,
            );
        });
        assert!(!handle.render().contains(names::GENAI_TOKENS_TOTAL));
    }

    #[test]
    fn genai_metric_has_exactly_twelve_fixed_series_and_no_model_label() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            for provider in [
                crate::ai::AiProviderLabel::OpenAi,
                crate::ai::AiProviderLabel::OpenAiCompatible,
                crate::ai::AiProviderLabel::Anthropic,
            ] {
                for source in [GenAiUsageSource::Reported, GenAiUsageSource::Estimated] {
                    record_genai_tokens(true, provider, source, 1, 2);
                }
            }
        });

        let rendered = handle.render();
        let samples: Vec<&str> = rendered
            .lines()
            .filter(|line| line.starts_with(names::GENAI_TOKENS_TOTAL))
            .collect();
        assert_eq!(
            samples.len(),
            12,
            "fixed label domains must yield twelve series:\n{rendered}"
        );
        assert!(
            !rendered.contains("model="),
            "model must never be a metric label:\n{rendered}"
        );
        for sample in samples {
            assert!(sample.contains("direction=\"input\"") || sample.contains("direction=\"output\""));
            assert!(
                sample.contains("provider=\"openai\"")
                    || sample.contains("provider=\"openai_compatible\"")
                    || sample.contains("provider=\"anthropic\"")
            );
            assert!(sample.contains("usage_source=\"reported\"") || sample.contains("usage_source=\"estimated\""));
            assert!(
                sample.ends_with(" 1") || sample.ends_with(" 2"),
                "counter must retain integer increments: {sample}"
            );
        }
    }
}

#[cfg(test)]
mod lb_panic_metric_tests {
    use super::*;

    #[test]
    fn record_lb_panic_mode_does_not_panic() {
        // Smoke test: the gauge set path is callable for engaged/cleared.
        record_lb_panic_mode("default/svc", true);
        record_lb_panic_mode("default/svc", false);
    }
}

#[cfg(test)]
mod link_sys_health_metric_tests {
    use super::*;
    use metrics_util::debugging::DebuggingRecorder;

    #[test]
    fn record_link_sys_health_sets_gauge_and_does_not_panic() {
        // Smoke: gauge set path is callable for both states (mirrors lb_panic smoke test).
        record_link_sys_health("default/kafka-main", true);
        record_link_sys_health("default/kafka-main", false);
    }

    #[test]
    fn record_link_sys_health_flip_emits_direction_label() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || {
            record_link_sys_health_flip("default/kafka-main", true);
            record_link_sys_health_flip("default/kafka-main", false);
        });
        let snap = snapshotter.snapshot().into_vec();
        let key = snap
            .iter()
            .find(|(k, _, _, _)| k.key().name() == names::LINK_SYS_HEALTH_FLIPS_TOTAL)
            .expect("flip counter registered");
        let labels: Vec<(&str, &str)> = key.0.key().labels().map(|l| (l.key(), l.value())).collect();
        assert!(labels.contains(&("link", "default/kafka-main")));
        assert!(labels.contains(&("direction", "recovered")));
        let degraded = snap.iter().any(|(k, _, _, _)| {
            k.key().name() == names::LINK_SYS_HEALTH_FLIPS_TOTAL
                && k.key()
                    .labels()
                    .any(|l| l.key() == "direction" && l.value() == "degraded")
        });
        assert!(degraded, "degraded flip counter must be registered");
    }
}
