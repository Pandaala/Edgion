// 测试套件模块

mod http_suite;
mod http_match_suite;
mod http_security_suite;
mod https_suite;
mod grpc_suite;
mod grpc_tls_suite;
mod websocket_suite;
mod tcp_suite;
mod udp_suite;
pub mod real_ip_suite;
pub mod security_suite;
pub mod mtls_suite;
mod plugin_logs_suite;
mod lb_policy_suite;
mod timeout_suite;
mod weighted_backend_suite;

pub use http_suite::HttpTestSuite;
pub use http_match_suite::HttpMatchTestSuite;
pub use http_security_suite::HttpSecurityTestSuite;
pub use https_suite::HttpsTestSuite;
pub use grpc_suite::GrpcTestSuite;
pub use grpc_tls_suite::GrpcTlsTestSuite;
pub use websocket_suite::WebSocketTestSuite;
pub use tcp_suite::TcpTestSuite;
pub use udp_suite::UdpTestSuite;
pub use real_ip_suite::RealIpTestSuite;
pub use security_suite::SecurityTestSuite;
pub use mtls_suite::MtlsTestSuite;
pub use plugin_logs_suite::PluginLogsTestSuite;
pub use lb_policy_suite::LBPolicyTestSuite;
pub use timeout_suite::TimeoutTestSuite;
pub use weighted_backend_suite::WeightedBackendTestSuite;

