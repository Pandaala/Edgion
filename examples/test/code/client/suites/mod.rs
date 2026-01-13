// Test suite module

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

// EdgionTls tests
pub use edgion_tls::{CipherTestSuite, GrpcTlsTestSuite, HttpsTestSuite, MtlsTestSuite};

// Gateway tests
pub use gateway::{
    AllowedRoutesAllNamespacesTestSuite, AllowedRoutesKindsTestSuite, AllowedRoutesSameNamespaceTestSuite,
    BackendTlsTestSuite, CombinedScenariosTestSuite, GatewayTlsTestSuite, ListenerHostnameTestSuite,
    PluginLogsTestSuite, RealIpTestSuite, SecurityTestSuite,
};
