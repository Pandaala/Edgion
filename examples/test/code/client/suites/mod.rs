// 测试套件模块

pub mod edgion_tls;
pub mod gateway;
pub mod grpc_route;
pub mod http_route;
pub mod tcp_route;
pub mod udp_route;

// Re-export all test suites for convenience
pub use http_route::{
    HttpMatchTestSuite, HttpRedirectTestSuite, HttpSecurityTestSuite, HttpTestSuite, LBPolicyTestSuite,
    TimeoutTestSuite, WebSocketTestSuite, WeightedBackendTestSuite,
};

pub use grpc_route::{GrpcMatchTestSuite, GrpcTestSuite};

pub use tcp_route::TcpTestSuite;

pub use udp_route::UdpTestSuite;

// EdgionTls 相关测试
pub use edgion_tls::{GrpcTlsTestSuite, HttpsTestSuite, MtlsTestSuite};

// Gateway 相关测试
pub use gateway::{BackendTlsTestSuite, PluginLogsTestSuite, RealIpTestSuite, SecurityTestSuite};
