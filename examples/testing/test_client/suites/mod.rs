// 测试套件模块

mod backend_tls_suite;
mod grpc_match_suite;
mod grpc_suite;
mod grpc_tls_suite;
mod http_match_suite;
mod http_security_suite;
mod http_suite;
mod https_suite;
mod lb_policy_suite;
pub mod mtls_suite;
mod plugin_logs_suite;
pub mod real_ip_suite;
pub mod security_suite;
mod tcp_suite;
mod timeout_suite;
mod udp_suite;
mod websocket_suite;
mod weighted_backend_suite;

pub use backend_tls_suite::BackendTlsTestSuite;
pub use grpc_match_suite::GrpcMatchTestSuite;
pub use grpc_suite::GrpcTestSuite;
pub use grpc_tls_suite::GrpcTlsTestSuite;
pub use http_match_suite::HttpMatchTestSuite;
pub use http_security_suite::HttpSecurityTestSuite;
pub use http_suite::HttpTestSuite;
pub use https_suite::HttpsTestSuite;
pub use lb_policy_suite::LBPolicyTestSuite;
pub use mtls_suite::MtlsTestSuite;
pub use plugin_logs_suite::PluginLogsTestSuite;
pub use real_ip_suite::RealIpTestSuite;
pub use security_suite::SecurityTestSuite;
pub use tcp_suite::TcpTestSuite;
pub use timeout_suite::TimeoutTestSuite;
pub use udp_suite::UdpTestSuite;
pub use websocket_suite::WebSocketTestSuite;
pub use weighted_backend_suite::WeightedBackendTestSuite;
