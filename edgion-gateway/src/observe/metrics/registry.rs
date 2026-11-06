//! Edgion metrics definitions
//!
//! Centralized metrics for monitoring gateway performance.
//! Uses the `metrics` crate for thread-safe, high-performance counters.

use edgion_common::cert_util::LeafValidity;
use edgion_common::conf_sync::cert_expiry_watch::ExpiryWatchState;
use edgion_common::observe::metrics_facade::{
    dynamic_counter, dynamic_gauge, dynamic_histogram, fixed_counter, fixed_gauge, DynamicCounter,
    FixedCounter as Counter, FixedGauge as Gauge, GaugeContribution,
};
use metrics::Label;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, OnceLock, Weak};

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
    // Request-body spill/materialization I/O. Both labels have fixed enum-like
    // values; request, route, and resource identity deliberately stay out.
    pub const REQUEST_BODY_STORAGE_IO_FAILURES_TOTAL: &str = "edgion_request_body_storage_io_failures_total";
    pub const REQUEST_BODY_STORAGE_IO_INFLIGHT: &str = "edgion_request_body_storage_io_inflight";
    // Native WAF metrics. Every label is a closed enum-like set; resource
    // names and rule identifiers deliberately stay in diagnostics instead.
    pub const WAF_COMPILE_TOTAL: &str = "edgion_gateway_waf_compile_total";
    pub const WAF_COMPILE_MICROSECONDS_TOTAL: &str = "edgion_gateway_waf_compile_microseconds_total";
    pub const WAF_GENERATION_BYTES: &str = "edgion_gateway_waf_generation_bytes";
    pub const WAF_RUNTIME_BYTES_ACTIVE: &str = "edgion_gateway_waf_runtime_bytes_active";
    pub const WAF_BODY_BYTES_INSPECTED_TOTAL: &str = "edgion_gateway_waf_body_bytes_inspected_total";
    pub const WAF_EXECUTION_TOTAL: &str = "edgion_gateway_waf_execution_total";
    pub const WAF_EXECUTION_MICROSECONDS_TOTAL: &str = "edgion_gateway_waf_execution_microseconds_total";
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
    // Selection events that bypass health filtering (per Service, across sections).
    pub const LB_DEGRADE_SELECTIONS_TOTAL: &str = "edgion_lb_degrade_selections_total";
    // ACME HTTP-01 challenge lookup metrics
    pub const ACME_CHALLENGE_LOOKUP_TOTAL: &str = "edgion_acme_challenge_lookup_total";
    // Stream plugin: GlobalConnectionIpRestriction decision events
    pub const CONN_IP_RESTRICTION_EVENTS_TOTAL: &str = "edgion_conn_ip_restriction_events_total";
    // Config aggregation: a conflict silently fell back to a default (e.g. conflicting outbound CA defaults -> System)
    pub const CONFIG_CONFLICT_TOTAL: &str = "edgion_config_conflict_total";
    // Outbound: a caller-injected header was dropped for an invalid name/value (silent no-op)
    pub const OUTBOUND_INJECTED_HEADER_SKIPPED_TOTAL: &str = "edgion_outbound_injected_header_skipped_total";
    // HTTP plugin: RegionRoute configuration availability outcomes.
    pub const REGION_ROUTE_CONFIG_EVENTS_TOTAL: &str = "edgion_region_route_config_events_total";
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
    // Redis LinkSys connection coverage sampled at scrape time, by state (live/configured).
    pub const LINK_SYS_REDIS_CONNECTIONS: &str = "edgion_link_sys_redis_connections";
    // Redis LinkSys topology events from the published generation's recovery loop.
    pub const LINK_SYS_REDIS_TOPOLOGY_EVENTS_TOTAL: &str = "edgion_link_sys_redis_topology_events_total";
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
    // FIPS: 1 = BoringSSL reports FIPS mode in this binary, 0 = not present
    // (non-FIPS build). Set once at startup. This mode signal does not establish
    // CMVP certificate scope or build provenance.
    pub const FIPS_MODULE_ENABLED: &str = "edgion_gateway_fips_module_enabled";
    // 1 = the host kernel reports FIPS mode (/proc/sys/crypto/fips_enabled).
    // Left operand of the compliance alert:
    //   fips_host_enabled == 1 and fips_module_enabled == 0
    pub const FIPS_HOST_ENABLED: &str = "edgion_gateway_fips_host_enabled";
    // 1 = the host FIPS flag exists but could not be read (masked /proc). Both
    // the guardrail and the fips_policy=auto default are inert in this state.
    pub const FIPS_HOST_DETECT_ERROR: &str = "edgion_gateway_fips_host_detect_error";
    // 1 = the approved cipher/curve profile is in effect on downstream
    // listeners. Labeled `module_enabled="true|false"` by whether BoringSSL
    // FIPS mode backs it. This is not a certification or compliance signal.
    pub const FIPS_POLICY_ACTIVE: &str = "edgion_gateway_fips_policy_active";
    // Wasm plugin host metrics. The `module` label is the human-readable
    // source descriptor (file path / url / "inline:<sha8>"), NOT the raw
    // cache key: bounded by the number of configured modules and readable.
    pub const WASM_MODULE_LOAD_TOTAL: &str = "edgion_wasm_module_load_total";
    pub const WASM_MODULE_COMPILE_DURATION_MS: &str = "edgion_wasm_module_compile_duration_ms";
    pub const WASM_MODULE_RELEASED_TOTAL: &str = "edgion_wasm_module_released_total";
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

    // GatewayClass-scoped plugin policy enforcement. Labeled by qualified
    // plugin name (bounded by policy size).
    pub const PLUGIN_POLICY_BLOCKED_TOTAL: &str = "edgion_gateway_plugin_policy_blocked_total";
    pub const PLUGIN_POLICY_BYPASSED_TOTAL: &str = "edgion_gateway_plugin_policy_bypassed_total";

    // Retry budget enforcement (`EdgionBackendTrafficPolicy.retryConstraint`,
    // spec 2026-07-21). No labels (D5): a per-service label would churn
    // unboundedly on Service creation/deletion. Backend attribution comes
    // from the per-request access log / `EdgionErrorCode`, already service-scoped.
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

pub fn record_request_body_storage_io_failure(operation: &'static str, reason: &'static str) {
    fixed_counter!(names::REQUEST_BODY_STORAGE_IO_FAILURES_TOTAL,
        "operation" => operation,
        "reason" => reason
    )
    .increment(1);
}

pub fn adjust_request_body_storage_io_inflight(delta: f64) {
    let metric = fixed_gauge!(names::REQUEST_BODY_STORAGE_IO_INFLIGHT);
    if delta >= 0.0 {
        metric.increment(delta);
    } else {
        metric.decrement(-delta);
    }
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
    fixed_counter!(names::GENAI_TOKENS_TOTAL,
        "direction" => "input",
        "provider" => provider,
        "usage_source" => usage_source,
    )
    .increment(input_tokens);
    fixed_counter!(names::GENAI_TOKENS_TOTAL,
        "direction" => "output",
        "provider" => provider,
        "usage_source" => usage_source,
    )
    .increment(output_tokens);
}

/// Gateway metrics collection
///
/// Fixed writes use exporter atomics; dynamic descriptors use renewable Common slots.
/// All metrics are automatically exported via the metrics facade.
///
/// Registration policy (hybrid):
/// - **Hot per-request** handles are eagerly registered in `new()` for zero
///   warmed allocation cost; they become non-zero within the first request so the
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

/// Balances the active-context gauge when terminal request logging finishes.
pub(crate) struct ActiveContextReleaseGuard<'a> {
    metrics: &'a GatewayMetrics,
}

impl Drop for ActiveContextReleaseGuard<'_> {
    fn drop(&mut self) {
        self.metrics.ctx_destroyed();
    }
}

#[inline]
fn get_or_register_counter<'a>(slot: &'a OnceLock<Counter>, name: &'static str) -> &'a Counter {
    slot.get_or_init(|| fixed_counter!(name))
}

/// Weak ownership deduplicates renewable descriptors for current/in-flight
/// Service snapshots. It never pins export accumulation or participates in
/// scraping. Cold registration uses this lock; final-owner removal never waits.
type DegradeCounters = HashMap<Arc<str>, Weak<DegradeSelectionCounter>>;
static LB_DEGRADE_COUNTERS: LazyLock<Mutex<DegradeCounters>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static LB_DEGRADE_CLEANUP_PENDING: AtomicBool = AtomicBool::new(false);

fn lock_degrade_counters() -> parking_lot::MutexGuard<'static, DegradeCounters> {
    let mut counters = LB_DEGRADE_COUNTERS.lock();
    if LB_DEGRADE_CLEANUP_PENDING.swap(false, Ordering::AcqRel) {
        counters.retain(|_, entry| entry.strong_count() > 0);
    }
    counters
}

pub(crate) struct DegradeSelectionCounter {
    service_key: Arc<str>,
    counter: DynamicCounter,
}

impl DegradeSelectionCounter {
    #[inline]
    pub(crate) fn record_selection(&self) {
        self.counter.increment(1);
    }
}

impl Drop for DegradeSelectionCounter {
    fn drop(&mut self) {
        let Some(mut counters) = LB_DEGRADE_COUNTERS.try_lock() else {
            // An in-flight request can release the last snapshot. Defer cleanup
            // to the next registration, scrape, or stats call rather than wait.
            LB_DEGRADE_CLEANUP_PENDING.store(true, Ordering::Release);
            return;
        };
        // A new owner may already have replaced a weak entry whose last Arc
        // was being dropped. Never remove that replacement's registration.
        if counters
            .get(self.service_key.as_ref())
            .is_some_and(|entry| std::ptr::eq(entry.as_ptr(), self))
        {
            counters.remove(self.service_key.as_ref());
        }
    }
}

impl GatewayMetrics {
    /// The existing shared conf_sync metric is sampled from the installed
    /// transport, never from the mutable certificate files.
    pub(crate) fn record_conf_sync_cert_expiry(validity: LeafValidity, now: i64, state: &mut ExpiryWatchState) {
        use edgion_common::conf_sync::cert_expiry_watch::{report_validity, sides};
        report_validity(sides::CLIENT, validity, now, state);
    }

    /// Prepare a descriptor on the Service configuration path and retain it in
    /// snapshots. The first real selection admits an export epoch; later idle
    /// renewal uses the same identity. Warm writes construct no labels/lookups.
    pub(crate) fn lb_degrade_selections(service_key: &str) -> Arc<DegradeSelectionCounter> {
        let mut counters = lock_degrade_counters();
        if let Some(counter) = counters.get(service_key).and_then(Weak::upgrade) {
            return counter;
        }
        let service_key: Arc<str> = service_key.into();
        let counter = Arc::new(DegradeSelectionCounter {
            counter: dynamic_counter!(names::LB_DEGRADE_SELECTIONS_TOTAL, "service" => service_key.to_string()),
            service_key: Arc::clone(&service_key),
        });
        counters.insert(service_key, Arc::downgrade(&counter));
        counter
    }

    /// Bounded store-residue signal; includes handles retained by old snapshots.
    pub(crate) fn lb_degrade_metric_stats(key_limit: usize) -> (usize, Vec<String>) {
        let counters = lock_degrade_counters();
        let mut keys: Vec<_> = counters.keys().map(|key| key.to_string()).collect();
        keys.sort_unstable();
        keys.truncate(key_limit);
        (counters.len(), keys)
    }

    /// Create a new GatewayMetrics instance.
    ///
    /// Hot per-request handles register immediately; cold handles stay
    /// uninitialized until their first `record_*` call.
    fn new() -> Self {
        Self {
            // Hot handles: eager
            ctx_created: fixed_counter!(names::CTX_CREATED),
            ctx_active: fixed_gauge!(names::CTX_ACTIVE),
            gateway_request_bytes: fixed_counter!(names::GATEWAY_REQUEST_BYTES),
            gateway_response_bytes: fixed_counter!(names::GATEWAY_RESPONSE_BYTES),
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

    /// Record a new proxy context.
    #[inline]
    pub(crate) fn ctx_created(&self) {
        self.ctx_created.increment(1);
        self.ctx_active.increment(1.0);
    }

    /// Release one proxy context's active-gauge charge.
    #[inline]
    pub(crate) fn ctx_destroyed(&self) {
        self.ctx_active.decrement(1.0);
    }

    /// Hold through Pingora's terminal logging hook to balance `ctx_created`.
    #[inline]
    pub(crate) fn active_context_release_guard(&self) -> ActiveContextReleaseGuard<'_> {
        ActiveContextReleaseGuard { metrics: self }
    }

    /// Record a dropped access log entry
    #[inline]
    pub fn access_log_dropped(&self, count: u64) {
        if count == 0 {
            return;
        }
        get_or_register_counter(&self.access_log_dropped, names::ACCESS_LOG_DROPPED).increment(count);
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
    pub fn ssl_log_dropped(&self, count: u64) {
        if count == 0 {
            return;
        }
        get_or_register_counter(&self.ssl_log_dropped, names::SSL_LOG_DROPPED).increment(count);
    }

    /// Record a dropped TCP log entry
    #[inline]
    pub fn tcp_log_dropped(&self, count: u64) {
        if count == 0 {
            return;
        }
        get_or_register_counter(&self.tcp_log_dropped, names::TCP_LOG_DROPPED).increment(count);
    }

    /// Record a dropped TLS access log entry
    #[inline]
    pub fn tls_log_dropped(&self, count: u64) {
        if count == 0 {
            return;
        }
        get_or_register_counter(&self.tls_log_dropped, names::TLS_LOG_DROPPED).increment(count);
    }

    /// Record a dropped UDP log entry
    #[inline]
    pub fn udp_log_dropped(&self, count: u64) {
        if count == 0 {
            return;
        }
        get_or_register_counter(&self.udp_log_dropped, names::UDP_LOG_DROPPED).increment(count);
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
    status: &'static str,
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
        labels.push(Label::from_static_parts("status", status));
    }
    if !test_key.is_empty() {
        labels.push(Label::new("test_key", test_key.to_string()));
    }
    if !test_data.is_empty() {
        labels.push(Label::new("test_data", test_data.to_string()));
    }
    dynamic_counter!(names::BACKEND_REQUESTS_TOTAL, labels).increment(1);
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

/// Record one native WAF generation build and its wall-clock cost.
///
/// `result` is a closed set (`ok` / `error`). A cumulative microsecond
/// counter keeps latency observable without adding an unsanctioned histogram.
pub fn record_waf_compile(result: &'static str, elapsed_micros: u64) {
    fixed_counter!(names::WAF_COMPILE_TOTAL, "result" => result).increment(1);
    fixed_counter!(names::WAF_COMPILE_MICROSECONDS_TOTAL, "result" => result).increment(elapsed_micros);
}

/// Adjust retained bytes for live compiled WAF generations.
/// Callers enforce +N/-N symmetry with an RAII lease.
pub fn adjust_waf_generation_bytes(delta: i64) {
    let metric = fixed_gauge!(names::WAF_GENERATION_BYTES);
    if delta >= 0 {
        metric.increment(delta as f64);
    } else {
        metric.decrement(delta.unsigned_abs() as f64);
    }
}

/// Adjust logical WAF transaction/scratch charge and known provisional JSON
/// capacities with symmetric ownership. Excludes body storage and RSS.
pub fn adjust_waf_runtime_bytes_active(delta: i64) {
    let metric = fixed_gauge!(names::WAF_RUNTIME_BYTES_ACTIVE);
    if delta >= 0 {
        metric.increment(delta as f64);
    } else {
        metric.decrement(delta.unsigned_abs() as f64);
    }
}

/// Record request source admitted once by prefix, Full, or JSON lexer inspection.
pub fn record_waf_body_bytes_inspected(direction: &'static str, bytes: usize) {
    fixed_counter!(names::WAF_BODY_BYTES_INSPECTED_TOTAL, "direction" => direction)
        .increment(u64::try_from(bytes).unwrap_or(u64::MAX));
}

/// Record cumulative WAF execution time for a fixed phase name.
pub fn record_waf_execution_micros(phase: &'static str, elapsed_micros: u64) {
    fixed_counter!(names::WAF_EXECUTION_TOTAL, "phase" => phase).increment(1);
    fixed_counter!(names::WAF_EXECUTION_MICROSECONDS_TOTAL, "phase" => phase).increment(elapsed_micros);
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
    fixed_counter!(names::MIRROR_REQUESTS_TOTAL,
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
    status: &'static str,
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
        label_vec.push(Label::from_static_parts("status", status));
    }

    dynamic_histogram!(names::GATEWAY_REQUEST_DURATION_MS, label_vec).record(duration_ms);
}

/// Record a circuit breaker ejection event. `class` is the bounded failure-class
/// label (`gateway` | `application` | `local_origin`) identifying which counter
/// tripped, so operators can split local-origin (data-plane connect/proxy) ejections
/// from upstream-origin 5xx ejections. The gauge stays unlabelled by class so its
/// +1/-1 balance with `record_cb_recovery` (which has no class context) holds.
pub fn record_cb_ejection(service_key: &str, class: &'static str) {
    dynamic_counter!(names::CB_EJECTIONS_TOTAL, "service" => service_key.to_string(), "class" => class).increment(1);
}

/// One endpoint generation owns its ejected-state contribution.
pub(crate) fn cb_ejected_endpoint_contribution(service_key: &str) -> GaugeContribution {
    GaugeContribution::new(metrics::Key::from_parts(
        names::CB_EJECTED_ENDPOINTS,
        vec![Label::new("service", service_key.to_string())],
    ))
}

/// Record a circuit breaker rejection (request blocked by ejected backend)
pub fn record_cb_rejection(service_key: &str) {
    dynamic_counter!(names::CB_REJECTIONS_TOTAL, "service" => service_key.to_string()).increment(1);
}

/// Set the link-sys provider health gauge: 1.0 = healthy, 0.0 = unhealthy.
///
/// Called when `/metrics` samples the published LinkSys clients. A client that
/// has not been published cannot write the shared series. `link` cardinality
/// is bounded by the configured provider count.
pub fn record_link_sys_health(name: &str, healthy: bool) {
    dynamic_gauge!(names::LINK_SYS_HEALTH, "link" => name.to_string()).set(if healthy { 1.0 } else { 0.0 });
}

/// Increment the link-sys health flip counter, labelled by provider and direction.
///
/// A level gauge cannot represent sub-scrape flapping (it only shows the last value
/// at scrape time); this monotonic counter does. `direction` is a fixed 2-value set.
pub fn record_link_sys_health_flip(name: &str, to_healthy: bool) {
    dynamic_counter!(names::LINK_SYS_HEALTH_FLIPS_TOTAL,
        "link" => name.to_string(),
        "direction" => if to_healthy { "recovered" } else { "degraded" }
    )
    .increment(1);
}

/// Set the Redis LinkSys connection coverage gauges for one published client.
///
/// Standalone/Sentinel: live pool members and `pool.size`. Cluster: primaries
/// answering the probe and known primaries. `state` is a fixed 2-value set;
/// `link` cardinality is bounded by the configured provider count.
pub fn record_link_sys_redis_connections(name: &str, live: usize, configured: usize) {
    dynamic_gauge!(names::LINK_SYS_REDIS_CONNECTIONS, "link" => name.to_string(), "state" => "live").set(live as f64);
    dynamic_gauge!(names::LINK_SYS_REDIS_CONNECTIONS, "link" => name.to_string(), "state" => "configured")
        .set(configured as f64);
}

/// A Redis LinkSys topology event. Labels are a fixed set; no endpoint data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RedisTopologyEvent {
    /// A recovery attempt failed after the previous one succeeded.
    RecoveryFailed,
    /// A recovery attempt succeeded after a failed one.
    Recovered,
    /// A recovery replaced the published Sentinel primary with a different one.
    PrimaryChanged,
}

impl RedisTopologyEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RecoveryFailed => "recovery_failed",
            Self::Recovered => "recovered",
            Self::PrimaryChanged => "primary_changed",
        }
    }
}

/// Count one Redis topology event for a published client.
pub fn record_link_sys_redis_topology_event(name: &str, event: RedisTopologyEvent) {
    dynamic_counter!(names::LINK_SYS_REDIS_TOPOLOGY_EVENTS_TOTAL,
        "link" => name.to_string(),
        "event" => event.as_str()
    )
    .increment(1);
}

/// Record an ACME HTTP-01 challenge token lookup event.
///
/// `result` must be one of: `"hit"`, `"expired"`, `"domain_mismatch"`, `"not_found"`, `"empty_host"`.
/// Low, fixed cardinality — safe as a label.
pub fn record_acme_challenge_lookup(result: &'static str) {
    fixed_counter!(names::ACME_CHALLENGE_LOOKUP_TOTAL, "result" => result).increment(1);
}

/// Record a watch payload, event-envelope, or resource decode failure (per kind).
///
/// Counts the first generic decode failure that terminates each watch stream.
/// The unprocessed tail is not decoded or counted; a fresh List recovers it.
/// The backend-specific decoder also uses this counter. `kind` is sourced from
/// `T::kind_name()` which returns a `&'static str` from a closed set of
/// registered resource types, so label cardinality is bounded.
///
/// Generic watch failures emit one safe error and trigger a relist using the
/// existing retry loop. This counter does not imply successful cache recovery;
/// backend-specific withdrawal and relist behavior is separate.
pub fn record_config_event_parse_error(kind: &'static str) {
    fixed_counter!(names::CONFIG_EVENT_PARSE_ERRORS_TOTAL, "kind" => kind).increment(1);
}

/// Record one RegionRoute configuration failure. Fixed reasons are
/// setting_reference_unavailable, setting_missing, setting_disabled,
/// setting_type_mismatch, setting_invalid and hash_modulo_invalid.
/// setting_invalid includes consumer/table incompatibility; hash_modulo_invalid
/// identifies an invalid plugin hash domain before dependency selection.
pub fn record_region_route_config_event(result: &'static str) {
    fixed_counter!(names::REGION_ROUTE_CONFIG_EVENTS_TOTAL, "result" => result).increment(1);
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
    fixed_counter!(names::CONN_IP_RESTRICTION_EVENTS_TOTAL, "result" => result).increment(1);
}

/// A config-aggregation conflict silently fell back to a default. `field` is a bounded
/// constant `&'static str` naming which aggregate conflicted (for example,
/// "outbound_ca" or "outbound_client_identity").
/// Zero-event sentinel: it stays absent until an operator ships genuinely conflicting
/// configs, at which point the resolved value is order-independent but non-obvious.
pub fn record_config_conflict(field: &'static str) {
    fixed_counter!(names::CONFIG_CONFLICT_TOTAL, "field" => field).increment(1);
}

/// A caller-injected outbound header (e.g. X-Forwarded-* from ForwardAuth) was dropped
/// at request build because its name or value was invalid — the operator's configured
/// header silently no-ops. Label-free zero-event sentinel.
pub fn record_injected_header_skipped() {
    fixed_counter!(names::OUTBOUND_INJECTED_HEADER_SKIPPED_TOTAL).increment(1);
}

/// Record a connection rejected by the ConnectionRateLimit stream plugin.
///
/// `dimension` must be `"listener"` or `"source_ip"`.
/// `stage` must be `"local"` or `"redis"`.
/// Both are fixed, low-cardinality `&'static str` — safe as labels.
pub fn record_connection_rate_limit_reject(dimension: &'static str, stage: &'static str) {
    fixed_counter!(names::CONNECTION_RATE_LIMIT_REJECTED_TOTAL,
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
    fixed_counter!(names::CONNECTION_RATE_LIMIT_REDIS_FAILURE_TOTAL,
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
    fixed_counter!(names::LINK_SYS_ES_BULK_CHANNEL_FULL_TOTAL).increment(1);
}

/// Record a `RoundRobinSelector` backend-count truncation.
///
/// Fires every time a selector is built with more than `u16::MAX` backends
/// (an extreme configuration anomaly). No labels — identifying the exact
/// route is best done via logs; the counter exists purely as an alarm signal
/// so operators can detect silent truncation.
pub fn record_rr_selector_truncated() {
    fixed_counter!(names::RR_SELECTOR_TRUNCATED_TOTAL).increment(1);
}

/// Record a LDAP auth plugin error event.
///
/// `kind` must be one of: `"unreachable"`, `"timeout"`, `"bind_fail"`,
/// `"tls_san_mismatch"`. Fixed cardinality — safe as a label.
pub fn record_ldap_auth_error(kind: &'static str) {
    fixed_counter!(names::LDAP_AUTH_ERRORS_TOTAL, "kind" => kind).increment(1);
}

/// Record an outbound TLS SAN-pinning rejection.
///
/// `plugin` must be one of: `"ldap_auth"`, `"openid_connect"`.
/// Fixed cardinality — safe as a label.
pub fn record_outbound_tls_san_mismatch(plugin: &'static str) {
    fixed_counter!(names::OUTBOUND_TLS_SAN_MISMATCH_TOTAL, "plugin" => plugin).increment(1);
}

/// Record a webhook key-resolver failure.
///
/// `reason` must be one of: `"not_found"`, `"unhealthy"`, `"rate_limited"`,
/// `"status_error"`, `"call_failed"`, `"authority_rejected"`. Fixed cardinality —
/// safe as a label.
pub fn record_webhook_resolve_error(reason: &'static str) {
    fixed_counter!(names::WEBHOOK_RESOLVE_ERRORS_TOTAL, "reason" => reason).increment(1);
}

/// Record an outbound address rejected by the egress policy.
///
/// `reason` is one of `"loopback"`, `"link_local"`, `"private"`, `"unspecified"`.
pub fn record_egress_blocked(reason: &'static str) {
    fixed_counter!(names::EGRESS_BLOCKED_TOTAL, "reason" => reason).increment(1);
}

/// Record an upstream authority the gateway refused to use. `reason` is
/// `"render_empty"` (the template produced nothing — usually a `${ctx:…}` written
/// by a plugin that runs after `upstream_peer`) or `"invalid_label"` (the rendered
/// label is not a valid DNS label — a broken template, or an attack).
pub fn record_authority_rejected(reason: &'static str) {
    fixed_counter!(names::UPSTREAM_AUTHORITY_REJECTED_TOTAL, "reason" => reason).increment(1);
}

/// Record a response whose status was accepted but whose `success.body` predicate
/// classified it as a failure — the "200 OK but error in the body" case, which is
/// otherwise invisible (the data plane must not log).
pub fn record_outbound_success_predicate_fail() {
    fixed_counter!(names::OUTBOUND_SUCCESS_PREDICATE_FAIL_TOTAL).increment(1);
}

/// Record a template field that failed to render to a usable value and fell back
/// to its static default. `field` is `"timeout_ms"`.
/// Fixed cardinality — safe as a label. The data plane must not log, so this
/// counter is the only signal an operator gets for a silently-falling-back template.
pub fn record_outbound_template_fallback(field: &'static str) {
    fixed_counter!(names::OUTBOUND_TEMPLATE_FALLBACK_TOTAL, "field" => field).increment(1);
}

// ============================================================
// Wasm plugin host metrics
// ============================================================
//
// Per-module Wasm metrics carry the source descriptor produced by
// `WasmSourceSpec::metrics_label()`. URL sources use a 96-byte-capped,
// credential-free `url:<canonical-origin>#<path-sha8>` identity that remains
// stable across signed-query rotations. Cardinality is bounded by the number
// of distinct configured source identities.

/// Record a Wasm module load attempt outcome. `result` is `"ok"` or `"error"`.
pub fn record_wasm_module_load(module: &str, result: &'static str) {
    dynamic_counter!(names::WASM_MODULE_LOAD_TOTAL, "module" => module.to_string(), "result" => result).increment(1);
}

/// Record the wall-clock duration of one `Module::new` compilation.
pub fn record_wasm_compile_duration(module: &str, ms: f64) {
    dynamic_histogram!(names::WASM_MODULE_COMPILE_DURATION_MS, "module" => module.to_string()).record(ms);
}

/// Accounts for one live compiled runtime generation, including initialization
/// and old generations retained by outstanding leases. Capturing the fixed handle
/// and private renewable release proxy
/// keeps destruction paired with the same recorder as construction.
#[cfg(feature = "wasm")]
pub(crate) struct WasmModuleMetrics {
    loaded: Gauge,
    released: DynamicCounter,
}

#[cfg(feature = "wasm")]
pub(crate) fn wasm_module_metrics(module: &str) -> WasmModuleMetrics {
    let loaded = fixed_gauge!(names::WASM_MODULES_LOADED);
    loaded.increment(1.);
    WasmModuleMetrics {
        loaded,
        released: dynamic_counter!(names::WASM_MODULE_RELEASED_TOTAL,"module"=>module.to_string()),
    }
}
#[cfg(feature = "wasm")]
impl Drop for WasmModuleMetrics {
    fn drop(&mut self) {
        self.loaded.decrement(1.);
        self.released.increment(1);
    }
}

/// Record a fresh VM instantiation (pool miss).
pub fn record_wasm_instance_instantiate(module: &str) {
    dynamic_counter!(names::WASM_INSTANCE_INSTANTIATE_TOTAL, "module" => module.to_string()).increment(1);
}

/// Record a VM discarded instead of returned to the pool.
/// `reason` is `"poisoned"` or `"over_capacity"`.
pub fn record_wasm_instance_discard(module: &str, reason: &'static str) {
    dynamic_counter!(names::WASM_INSTANCE_DISCARD_TOTAL, "module" => module.to_string(), "reason" => reason)
        .increment(1);
}

/// One module generation owns its pooled VM contribution.
#[cfg(feature = "wasm")]
pub(crate) fn wasm_pool_size_contribution(module: &str) -> GaugeContribution {
    GaugeContribution::new(metrics::Key::from_parts(
        names::WASM_POOL_SIZE,
        vec![Label::new("module", module.to_string())],
    ))
}

/// Record the duration of one guest hook invocation.
/// `phase` is one of `"request_headers"`, `"response_headers"`,
/// `"response_body"`, `"http_call_response"`, `"tick"`.
pub fn record_wasm_hook_duration(module: &str, phase: &'static str, ms: f64) {
    dynamic_histogram!(names::WASM_HOOK_DURATION_MS, "module" => module.to_string(), "phase" => phase).record(ms);
}

/// Record a guest trap (or protocol violation that poisons the lease).
pub fn record_wasm_trap(module: &str, phase: &'static str) {
    dynamic_counter!(names::WASM_TRAP_TOTAL, "module" => module.to_string(), "phase" => phase).increment(1);
}

/// Record a fail-mode decision taken by the Wasm plugin.
/// `reason` is one of `"load"`, `"acquire"`, `"trap"`, `"not loaded"`,
/// `"callout"`, `"no-resume"`; `mode` is `"open"` or `"close"`.
pub fn record_wasm_fail_mode(module: &str, reason: &str, mode: &'static str) {
    dynamic_counter!(names::WASM_FAIL_MODE_TOTAL,
        "module" => module.to_string(),
        "reason" => reason.to_string(),
        "mode" => mode
    )
    .increment(1);
}

/// Record a guest-dispatched HTTP callout outcome. `result` is one of
/// `"ok"`, `"error"`, `"denied"`, `"over_limit"`
pub fn record_wasm_callout(module: &str, result: &'static str) {
    dynamic_counter!(names::WASM_CALLOUT_TOTAL, "module" => module.to_string(), "result" => result).increment(1);
}

/// Each store owns a committed contribution; same-module generations aggregate.
#[cfg(feature = "wasm")]
pub(crate) fn wasm_shared_data_bytes_gauge(module: &str) -> GaugeContribution {
    GaugeContribution::new(metrics::Key::from_parts(
        names::WASM_SHARED_DATA_BYTES,
        vec![Label::new("module", module.to_string())],
    ))
}

/// Record a guest returning `Action::Pause` from the response-body hook.
///
/// This host implements the body hook as a per-chunk read-only *window*, not the
/// spec's cumulative buffer, so Pause cannot be honored. A guest that pauses is
/// written against the standard buffering idiom and will silently observe only
/// the current chunk instead of the whole body — alert on this counter.
pub fn record_wasm_body_pause_unsupported(module: &str) {
    dynamic_counter!(names::WASM_BODY_PAUSE_UNSUPPORTED_TOTAL, "module" => module.to_string()).increment(1);
}

// ============================================================
// Plugin policy enforcement metrics
// ============================================================
//
// Emitted by `policy_filter::PolicyBlockFilter` / `PolicyBypassNote`
// (edgion-gateway/src/plugins/runtime/policy_filter.rs) when the published
// GatewayClass-scoped policy for a request restricts an entry. `plugin` is the
// qualified name (`http/<TypeName>`); cardinality is bounded by the number
// of distinct plugin types the operator's policy touches.

/// A request was rejected because its scoped policy blocks a plugin.
pub fn record_plugin_policy_blocked(plugin: &str) {
    dynamic_counter!(names::PLUGIN_POLICY_BLOCKED_TOTAL, "plugin" => plugin.to_string()).increment(1);
}

/// A request passed through a chain with a plugin bypassed by its scoped policy.
pub fn record_plugin_policy_bypassed(plugin: &str) {
    dynamic_counter!(names::PLUGIN_POLICY_BYPASSED_TOTAL, "plugin" => plugin.to_string()).increment(1);
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
// is read from the per-request access log / `EdgionErrorCode`, both already
// service-scoped.

/// A retry attempt was refused by a `retryConstraint` budget.
pub fn record_retry_budget_denied() {
    fixed_counter!(names::RETRY_BUDGET_DENIED_TOTAL).increment(1);
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
    dynamic_counter!(names::CB_SHED_TOTAL, "service" => service_key.to_string()).increment(1);
}

// ============================================================
// GeoIP country restriction metrics
// ============================================================
//
// `set_geoip_db_loaded`, `set_geoip_db_build_epoch`, `record_geoip_update_failed`,
// and `record_geoip_denied` are not on the per-request hot path (the first two
// are process-lifecycle state, the latter two fire from the background updater
// and from an L4 connection-filter deny path respectively), so all four follow
// the cold free-function `gauge!`/`counter!` pattern (like `record_link_sys_health`
// above): the series is absent from `/metrics` until
// the first call. `record_geoip_lookup_panic`, below, is the one exception in
// this block — it fires from `GeoIpProvider::lookup`, which every geo-consuming
// request calls; it uses the same cold-registration pattern anyway since a
// panic inside the MMDB reader is expected to be exceedingly rare in practice.

/// Set to 1 when a database is loaded, 0 otherwise.
/// Alert on `edgion_geoip_db_loaded == 0` while geo rules are configured: that
/// is the dangerous silent failure — the control is not running.
pub fn set_geoip_db_loaded(loaded: bool) {
    fixed_gauge!(names::GEOIP_DB_LOADED).set(if loaded { 1.0 } else { 0.0 });
}

/// The loaded database's build timestamp. Alert on staleness.
pub fn set_geoip_db_build_epoch(epoch: u64) {
    fixed_gauge!(names::GEOIP_DB_BUILD_EPOCH).set(epoch as f64);
}

/// `reason` is a bounded enum, never free text:
/// fetch | encoding | size_cap | stall | validation | rename | disk | deadline.
/// This is strictly about the background *update cycle*; a per-request MMDB
/// lookup panic is a distinct, per-request event recorded by
/// `record_geoip_lookup_panic` instead, not folded in here.
pub fn record_geoip_update_failed(reason: &'static str) {
    fixed_counter!(names::GEOIP_UPDATE_FAILED, "reason" => reason).increment(1);
}

/// A per-request MMDB lookup inside `GeoIpProvider::lookup` panicked (caught
/// via `catch_unwind`) and was reported as `GeoLookup::Failed`. Label-free: this
/// is a data-integrity signal about the loaded database, not something to
/// slice by request attributes. Decode-without-panic failures share the
/// `Failed` outcome but do not increment this counter.
pub fn record_geoip_lookup_panic() {
    fixed_counter!(names::GEOIP_LOOKUP_PANIC).increment(1);
}

/// L4 geo denials, which are never logged (see the design's §6.6).
/// `layer` is stage1 | tcp; `reason` is deny_country | allow_miss |
/// db_unavailable | lookup_failed | ref_unavailable.
/// Neither label carries a client address.
pub fn record_geoip_denied(layer: StreamLayer, reason: &'static str) {
    fixed_counter!(names::GEOIP_DENIED, "layer" => layer.as_metric_label(), "reason" => reason).increment(1);
}

#[cfg(test)]
#[path = "../../../tests/unit/observe/metrics/registry_test/wasm_shared_data_metric_tests.rs"]
mod wasm_shared_data_metric_tests;

#[cfg(test)]
#[path = "../../../tests/unit/observe/metrics/registry_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/unit/observe/metrics/registry_test/lb_degrade_counter_tests.rs"]
mod lb_degrade_counter_tests;

#[cfg(test)]
#[path = "../../../tests/unit/observe/metrics/registry_test/link_sys_health_metric_tests.rs"]
mod link_sys_health_metric_tests;
