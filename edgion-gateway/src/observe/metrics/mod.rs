//! Metrics observability: registry, test data, and Prometheus API

pub mod api;
pub mod registry;
pub mod route_handles;
pub mod test_metrics;

pub use api::{create_metrics_router, serve_with_shutdown};
pub use registry::{
    global_metrics, record_acme_challenge_lookup, record_authority_rejected, record_backend_request,
    record_cb_ejection, record_cb_recovery, record_cb_rejection, record_config_conflict,
    record_config_event_parse_error, record_conn_ip_restriction_event, record_connection_rate_limit_redis_failure,
    record_connection_rate_limit_reject, record_egress_blocked, record_injected_header_skipped,
    record_ip_deny_entries_skipped, record_lb_panic_mode, record_mirror_metric, record_outbound_success_predicate_fail,
    record_outbound_template_fallback, record_region_route_overlay_event, record_request_duration,
    record_webhook_resolve_error, status_group, GatewayMetrics,
};
pub use test_metrics::{TestData, TestType};
