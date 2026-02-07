// Test suite module

pub mod edgion_plugins;
pub mod edgion_tls;
pub mod gateway;
pub mod grpc_route;
pub mod http_route;
pub mod ref_grant_status;
pub mod tcp_route;
pub mod udp_route;

// Re-export all test suites for convenience
pub use http_route::{
    HeaderModifierTestSuite, HttpMatchTestSuite, HttpRedirectTestSuite, HttpSecurityTestSuite, HttpTestSuite,
    LBConsistentHashTestSuite, LBRoundRobinTestSuite, TimeoutTestSuite, WebSocketTestSuite, WeightedBackendTestSuite,
};

pub use grpc_route::{GrpcMatchTestSuite, GrpcTestSuite};

pub use tcp_route::TcpTestSuite;

pub use udp_route::UdpTestSuite;

// EdgionTls tests
pub use edgion_tls::{CipherTestSuite, GrpcTlsTestSuite, HttpsTestSuite, MtlsTestSuite};

// Gateway tests
pub use gateway::{
    AllowedRoutesAllNamespacesTestSuite, AllowedRoutesKindsTestSuite, AllowedRoutesSameNamespaceTestSuite,
    BackendTlsTestSuite, CombinedScenariosTestSuite, GatewayTlsTestSuite, InitialPhaseTestSuite,
    ListenerHostnameTestSuite, PortConflictTestSuite, RealIpTestSuite, SecurityTestSuite, StreamPluginsTestSuite,
    UpdatePhaseTestSuite,
};

// EdgionPlugins tests
pub use edgion_plugins::{
    AllConditionsTestSuite, CtxSetTestSuite, JwtAuthTestSuite, KeyAuthTestSuite, PluginConditionTestSuite,
    PluginLogsTestSuite, ProxyRewriteTestSuite, RateLimitTestSuite, RealIpPluginTestSuite, RequestRestrictionTestSuite,
    ResponseRewriteTestSuite,
};

// ReferenceGrant Status tests
pub use ref_grant_status::RefGrantStatusTestSuite;
