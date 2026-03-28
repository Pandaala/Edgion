// Test suite module

pub mod edgion_plugins;
pub mod edgion_tls;
pub mod gateway;
pub mod grpc_route;
pub mod http_route;
pub mod services;
pub mod status;
pub mod tcp_route;
pub mod tls_route;
pub mod udp_route;

// Re-export all test suites for convenience
pub use http_route::{
    CircuitBreakerTestSuite, FilterConflictTestSuite, GrpcHealthCheckTestSuite, HeaderModifierTestSuite,
    HealthCheckTestSuite, HealthCheckTransitionTestSuite, HttpMatchTestSuite, HttpRedirectTestSuite,
    HttpSecurityTestSuite, HttpTestSuite, LBConsistentHashTestSuite, LBRoundRobinTestSuite, MultiPortTestSuite,
    PartiallyInvalidTestSuite, PercentMirrorTestSuite, RetryOnStatusTestSuite, SessionPersistenceTestSuite,
    TimeoutTestSuite, WebSocketTestSuite, WeightedBackendTestSuite,
};

pub use grpc_route::{GrpcMatchTestSuite, GrpcTestSuite};

pub use tcp_route::{TcpStreamPluginsTestSuite, TcpTestSuite};

pub use tls_route::{
    TlsBothAbsentParentRefTestSuite, TlsMultiSniTestSuite, TlsPassthroughTestSuite, TlsProxyProtocolTestSuite,
    TlsRouteTestSuite, TlsStreamPluginsTestSuite,
};

pub use udp_route::UdpTestSuite;

// EdgionTls tests
pub use edgion_tls::{
    CipherTestSuite, EdgionTlsBothAbsentParentRefTestSuite, GrpcTlsTestSuite, HttpsTestSuite, MtlsTestSuite,
    PortOnlyEdgionTlsTestSuite,
};

// Gateway tests
pub use gateway::{
    AllowedRoutesAllNamespacesTestSuite, AllowedRoutesKindsTestSuite, AllowedRoutesSameNamespaceTestSuite,
    AllowedRoutesSelectorNamespaceTestSuite, BackendTlsGatewayCertTestSuite, BackendTlsSanTestSuite,
    BackendTlsTestSuite, CombinedScenariosTestSuite, FrontendValidationTestSuite,
    GatewayTlsNoHostnameListenerTestSuite, GatewayTlsTestSuite, InitialPhaseTestSuite, ListenerHostnameTestSuite,
    PortConflictTestSuite, RealIpTestSuite, SecurityTestSuite, StreamPluginsTestSuite, UpdatePhaseTestSuite,
};

// EdgionPlugins tests
pub use edgion_plugins::{
    AllConditionsTestSuite, AllEndpointStatusTestSuite, BandwidthLimitTestSuite, BasicAuthTestSuite, CtxSetTestSuite,
    DirectEndpointTestSuite, DslTestSuite, DynamicExternalUpstreamTestSuite, DynamicInternalUpstreamTestSuite,
    ForwardAuthTestSuite, HeaderCertAuthTestSuite, HmacAuthTestSuite, JweDecryptTestSuite, JwtAuthTestSuite,
    KeyAuthTestSuite, LdapAuthTestSuite, OpenidConnectTestSuite, PluginConditionTestSuite, PluginLogsTestSuite,
    ProxyRewriteTestSuite, RateLimitTestSuite, RealIpPluginTestSuite, RequestMirrorTestSuite,
    RequestRestrictionTestSuite, ResponseRewriteTestSuite, WebhookKeyGetTestSuite,
};

// Status tests (ReferenceGrant, BackendTLSPolicy, etc.)
pub use status::BackendTlsStatusTestSuite;
pub use status::GrpcRouteStatusTestSuite;
pub use status::HostnameIntersectionStatusTestSuite;
pub use status::RefGrantStatusTestSuite;
pub use status::ObservedGenerationStatusTestSuite;
pub use status::TlsListenerModeStatusTestSuite;

// Services tests (ACME, etc.)
pub use services::AcmeTestSuite;
