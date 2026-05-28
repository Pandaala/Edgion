//! Metrics observability: registry, test data, and Prometheus API

pub mod api;
pub mod registry;
pub mod test_metrics;

pub use api::{create_metrics_router, serve, serve_with_shutdown};
pub use registry::{
    global_metrics, record_acme_challenge_lookup, record_backend_request, record_cb_ejection, record_cb_recovery,
    record_cb_rejection, record_config_event_parse_error, record_conn_ip_restriction_event, record_mirror_metric,
    record_request_duration, record_webhook_resolve_error, status_group, GatewayMetrics,
};
pub use test_metrics::{set_latency_test_data, set_lb_test_data, set_retry_test_data, TestData, TestType};
