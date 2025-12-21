// 测试套件模块

mod http_suite;
mod https_suite;
mod grpc_suite;
mod grpc_tls_suite;
mod websocket_suite;
mod tcp_suite;
mod udp_suite;
pub mod real_ip_suite;

pub use http_suite::HttpTestSuite;
pub use https_suite::HttpsTestSuite;
pub use grpc_suite::GrpcTestSuite;
pub use grpc_tls_suite::GrpcTlsTestSuite;
pub use websocket_suite::WebSocketTestSuite;
pub use tcp_suite::TcpTestSuite;
pub use udp_suite::UdpTestSuite;
pub use real_ip_suite::RealIpTestSuite;

