//! Metrics observability: registry, test data, and Prometheus API

pub mod api;
pub mod registry;
pub mod route_handles;
pub mod test_metrics;

pub use api::{create_metrics_router, serve_with_shutdown};
pub use registry::{
    adjust_request_body_storage_io_inflight, adjust_waf_generation_bytes, adjust_waf_runtime_bytes_active,
    global_metrics, record_acme_challenge_lookup, record_authority_rejected, record_backend_request,
    record_cb_ejection, record_cb_rejection, record_config_conflict, record_config_event_parse_error,
    record_conn_ip_restriction_event, record_connection_rate_limit_redis_failure, record_connection_rate_limit_reject,
    record_egress_blocked, record_genai_tokens, record_geoip_denied, record_geoip_lookup_panic,
    record_geoip_update_failed, record_injected_header_skipped, record_mirror_metric,
    record_outbound_success_predicate_fail, record_outbound_template_fallback, record_region_route_overlay_event,
    record_request_body_storage_io_failure, record_request_duration, record_retry_budget_denied,
    record_waf_body_bytes_inspected, record_waf_compile, record_waf_execution_micros, record_webhook_resolve_error,
    set_geoip_db_build_epoch, set_geoip_db_loaded, status_group, GatewayMetrics, GenAiUsageSource,
};
pub use test_metrics::{TestData, TestType};
