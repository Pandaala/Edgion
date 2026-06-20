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
    BackendTlsSniHostOverrideTestSuite, CircuitBreakerTestSuite, DanglingExtensionRefTestSuite,
    DnsResolutionFailureTestSuite, FilterConflictTestSuite, GrpcHealthCheckTestSuite, H2MtlsPersistenceTestSuite,
    H2TrailerMalformedTestSuite, H2cTestSuite, HeaderModifierTestSuite, HealthCheckTestSuite,
    HealthCheckTransitionTestSuite, HttpMatchTestSuite, HttpRedirectTestSuite, HttpSecurityTestSuite, HttpTestSuite,
    LBConsistentHashTestSuite, LBEwmaRetryOnStatusTestSuite, LBEwmaTestSuite, LBLeastConnRetryOnStatusTestSuite,
    LBLeastConnTestSuite, LBRoundRobinTestSuite, MultiPortTestSuite, PanicThresholdTestSuite,
    PartiallyInvalidTestSuite, PerTryTimeoutTestSuite, PercentMirrorTestSuite, RetryOnStatusTestSuite,
    SessionPersistenceTestSuite, SpChangeFinalTestSuite, SpChangeInitialTestSuite, TimeoutTestSuite,
    WebSocketTestSuite, WeightedBackendTestSuite,
};

pub use grpc_route::{
    GrpcMatchTestSuite, GrpcRateLimitTestSuite, GrpcTestSuite, GrpcUnmatchedRouteTestSuite, GrpcWebTestSuite,
};

pub use tcp_route::{
    GcirEnforcementTestSuite, TcpEspReloadAllowTestSuite, TcpEspReloadDenyTestSuite, TcpIdleTimeoutTestSuite,
    TcpKeepaliveSmokeTestSuite, TcpStreamPluginsTestSuite, TcpTestSuite,
};

pub use tls_route::{
    TlsBothAbsentParentRefTestSuite, TlsMultiSniTestSuite, TlsPassthroughTestSuite, TlsProxyProtocolTestSuite,
    TlsRouteTestSuite, TlsStreamPluginsTestSuite, TlsUnknownKindBackendTestSuite,
};

pub use udp_route::{UdpStreamPluginsTestSuite, UdpTestSuite};

// EdgionTls tests
pub use edgion_tls::{
    CipherTestSuite, EdgionTlsBothAbsentParentRefTestSuite, FallbackSniTestSuite, GrpcTlsTestSuite, HttpsTestSuite,
    LastKnownGoodFallbackTestSuite, LastKnownGoodInitialTestSuite, MtlsFailcloseTestSuite, MtlsTestSuite,
    PortOnlyEdgionTlsTestSuite, SniCollisionTestSuite, WildcardSingleLabelTestSuite,
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
    AllConditionsTestSuite, AllEndpointStatusTestSuite, BandwidthLimitTestSuite,
    BasicAuthDynamicGroupedInitialTestSuite, BasicAuthDynamicGroupedUpdateTestSuite, BasicAuthDynamicInitialTestSuite,
    BasicAuthDynamicUpdateTestSuite, BasicAuthTestSuite, CorsPluginTestSuite, CsrfTestSuite, CtxSetTestSuite,
    DirectEndpointTestSuite, DslDynamicInitialTestSuite, DslDynamicUpdateTestSuite, DslGrpcErrorTestSuite,
    DslTestSuite, DynamicExternalUpstreamTestSuite, DynamicInternalUpstreamTestSuite,
    EdgionConfigDataFlapInitialTestSuite, EdgionConfigDataFlapInvalidTestSuite, EdgionConfigDataFlapValidTestSuite,
    EdgionConfigDataManagedTestSuite, EdgionConfigDataStatusTestSuite, ExtProcTestSuite, FaultInjectionTestSuite,
    ForwardAuthTestSuite, GlobalAccessControlTestSuite, GlobalConnectionIpRestrictionTestSuite,
    GlobalPluginsSnapshotTestSuite, HeaderCertAuthTestSuite, HmacAuthTestSuite,
    IpRestrictionPartialGoodPluginTestSuite, IpRestrictionPluginTestSuite, IpRestrictionRefsPluginTestSuite,
    JweDecryptTestSuite, JwtAuthTestSuite, KeyAuthTestSuite, LdapAuthTestSuite, OpenidConnectTestSuite,
    PathNormalizationForwardViewTestSuite, PathNormalizationPreserveParamTestSuite, PathNormalizationTestSuite,
    PluginConditionTestSuite, PluginLogsTestSuite, PluginValidationStatusTestSuite, ProxyRewriteTestSuite,
    RateLimitDynamicInitialTestSuite, RateLimitDynamicUpdateTestSuite, RateLimitRedisTestSuite, RateLimitTestSuite,
    RealIpPluginTestSuite, RegionRouteTestSuite, RequestMirrorTestSuite, RequestRestrictionTestSuite,
    ResolvedRefsTestSuite, ResponseRewriteTestSuite, TraceContextPluginTestSuite, WebhookKeyGetTestSuite,
};

// LinkSys tests
pub use link_sys::LinkSysStatusTestSuite;

// Status tests (ReferenceGrant, BackendTLSPolicy, etc.)
pub use status::BackendTlsStatusTestSuite;
pub use status::BtlspFlapInitialTestSuite;
pub use status::BtlspFlapInvalidTestSuite;
pub use status::BtlspFlapValidTestSuite;
pub use status::EdgionConfigDataRefGrantTestSuite;
pub use status::GatewayFrontendValidationStatusTestSuite;
pub use status::GcParamsFlapMissingTestSuite;
pub use status::GcParamsFlapPresentTestSuite;
pub use status::GrpcRouteStatusTestSuite;
pub use status::HighFreqFinalTestSuite;
pub use status::HighFreqInitialTestSuite;
pub use status::HostnameIntersectionStatusTestSuite;
pub use status::ObservedGenerationStatusTestSuite;
pub use status::RefGrantStatusTestSuite;
pub use status::TcpRouteCrossNsSvcTestSuite;
pub use status::TlsListenerModeStatusTestSuite;
pub use status::TlsRouteCrossNsSvcTestSuite;
pub use status::TlsRouteStatusTestSuite;

// Services tests (ACME, etc.)
pub use services::AcmeTestSuite;
