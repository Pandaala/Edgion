// Test suite module

pub mod edgion_plugins;
pub mod edgion_tls;
pub mod gateway;
pub mod grpc_route;
pub mod http_route;
pub mod link_sys;
pub mod services;
pub mod status;
pub mod tcp_route;
pub mod tls_route;
pub mod udp_route;

// Re-export all test suites for convenience
pub use http_route::{
    CircuitBreakerTestSuite, FilterConflictTestSuite, GrpcHealthCheckTestSuite, H2cTestSuite, HeaderModifierTestSuite,
    HealthCheckTestSuite, HealthCheckTransitionTestSuite, HttpMatchTestSuite, HttpRedirectTestSuite,
    HttpSecurityTestSuite, HttpTestSuite, LBConsistentHashTestSuite, LBRoundRobinTestSuite, MultiPortTestSuite,
    PartiallyInvalidTestSuite, PercentMirrorTestSuite, RetryOnStatusTestSuite, SessionPersistenceTestSuite,
    TimeoutTestSuite, WebSocketTestSuite, WeightedBackendTestSuite,
};

pub use grpc_route::{GrpcMatchTestSuite, GrpcTestSuite, GrpcUnmatchedRouteTestSuite};

pub use tcp_route::{GcirEnforcementTestSuite, TcpStreamPluginsTestSuite, TcpTestSuite};

pub use tls_route::{
    TlsBothAbsentParentRefTestSuite, TlsMultiSniTestSuite, TlsPassthroughTestSuite, TlsProxyProtocolTestSuite,
    TlsRouteTestSuite, TlsStreamPluginsTestSuite, TlsUnknownKindBackendTestSuite,
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
    MisdirectedRequestTestSuite, PortConflictTestSuite, RealIpTestSuite, SecurityTestSuite, StreamPluginsTestSuite,
    UpdatePhaseTestSuite,
};

// EdgionPlugins tests
pub use edgion_plugins::{
    AllConditionsTestSuite, AllEndpointStatusTestSuite, BandwidthLimitTestSuite, BasicAuthTestSuite,
    CorsPluginTestSuite, CsrfTestSuite, CtxSetTestSuite, DirectEndpointTestSuite, DslDynamicInitialTestSuite,
    DslDynamicUpdateTestSuite, DslGrpcErrorTestSuite, DslTestSuite, DynamicExternalUpstreamTestSuite,
    DynamicInternalUpstreamTestSuite, ForwardAuthTestSuite, GlobalConnectionIpRestrictionTestSuite,
    HeaderCertAuthTestSuite, HmacAuthTestSuite, JweDecryptTestSuite, JwtAuthTestSuite, KeyAuthTestSuite,
    LdapAuthTestSuite, OpenidConnectTestSuite, PluginConditionTestSuite, PluginLogsTestSuite,
    PluginMetaDataManagedTestSuite, PluginMetaDataStatusTestSuite, ProxyRewriteTestSuite, RateLimitTestSuite,
    RealIpPluginTestSuite, RegionRouteTestSuite, RequestMirrorTestSuite, RequestRestrictionTestSuite,
    ResolvedRefsTestSuite, ResponseRewriteTestSuite, WebhookKeyGetTestSuite,
};

// LinkSys tests
pub use link_sys::LinkSysStatusTestSuite;

// Status tests (ReferenceGrant, BackendTLSPolicy, etc.)
pub use status::BackendTlsStatusTestSuite;
pub use status::GatewayFrontendValidationStatusTestSuite;
pub use status::GrpcRouteStatusTestSuite;
pub use status::HostnameIntersectionStatusTestSuite;
pub use status::ObservedGenerationStatusTestSuite;
pub use status::RefGrantStatusTestSuite;
pub use status::TlsListenerModeStatusTestSuite;
pub use status::TlsRouteStatusTestSuite;

// Services tests (ACME, etc.)
pub use services::AcmeTestSuite;
