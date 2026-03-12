//! Metrics observability: registry, test data, and Prometheus API

pub mod api;
pub mod registry;
pub mod test_metrics;

pub use api::{create_metrics_router, init_metrics_exporter, serve};
pub use registry::{global_metrics, record_backend_request, record_mirror_metric, status_group, GatewayMetrics};
pub use test_metrics::{set_latency_test_data, set_lb_test_data, set_retry_test_data, TestData, TestType};
