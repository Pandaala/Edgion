// 测试套件模块

pub mod grpc_route;
pub mod http_route;
pub mod plugins;
pub mod security;
pub mod tcp_route;
pub mod tls;
pub mod udp_route;

// Re-export all test suites for convenience
pub use http_route::{
    HttpMatchTestSuite, HttpRedirectTestSuite, HttpSecurityTestSuite, HttpTestSuite, HttpsTestSuite, LBPolicyTestSuite,
    TimeoutTestSuite, WebSocketTestSuite, WeightedBackendTestSuite,
};

pub use grpc_route::{GrpcMatchTestSuite, GrpcTestSuite, GrpcTlsTestSuite};

pub use tcp_route::TcpTestSuite;
// StreamPluginsTestSuite 暂未使用
#[allow(unused_imports)]
pub use tcp_route::StreamPluginsTestSuite;

pub use udp_route::UdpTestSuite;

pub use tls::{BackendTlsTestSuite, MtlsTestSuite};

pub use plugins::PluginLogsTestSuite;

pub use security::{RealIpTestSuite, SecurityTestSuite};
