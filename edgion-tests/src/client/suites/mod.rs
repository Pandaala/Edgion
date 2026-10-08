// Test suite module

pub mod edgion_backend;
pub mod edgion_backend_traffic_policy;
pub mod edgion_plugins;
pub mod edgion_tls;
pub mod gateway;
pub mod grpc_route;
pub mod http_route;
mod l4_duplex;
pub mod link_sys;
pub use l4_duplex::L4DuplexTestSuite;
pub mod plugin_policy;
pub mod services;
pub mod status;
pub mod tcp_route;
pub mod tls_route;
pub mod udp_route;

// Re-export all test suites for convenience
pub use http_route::{
    BackendTlsSniHostOverrideTestSuite, CircuitBreakerTestSuite, DanglingExtensionRefTestSuite,
    DegradeThresholdTestSuite, DelegationDynamicInitialTestSuite, DelegationDynamicUpdateTestSuite,
    DelegationTestSuite, DnsFlapBoundedTestSuite, DnsFlapFailingTestSuite, DnsFlapWorkingTestSuite,
    DnsResolutionFailureTestSuite, DnsResolutionTestSuite, FilterConflictTestSuite, GrpcHealthCheckTestSuite,
    H2MtlsPersistenceTestSuite, H2TrailerMalformedTestSuite, H2cTestSuite, HeaderModifierTestSuite,
    HealthCheckTestSuite, HealthCheckTransitionTestSuite, HttpMatchTestSuite, HttpRedirectTestSuite,
    HttpSecurityTestSuite, HttpTestSuite, InformationalRetryTestSuite, LBConsistentHashTestSuite,
    LBEwmaRetryOnStatusTestSuite, LBEwmaTestSuite, LBLeastConnRetryOnStatusTestSuite, LBLeastConnTestSuite,
    LBRoundRobinTestSuite, MultiPortTestSuite, PartiallyInvalidTestSuite, PerTryTimeoutTestSuite,
    PercentMirrorTestSuite, RetryOnStatusTestSuite, SessionPersistenceTestSuite, SpChangeFinalTestSuite,
    SpChangeInitialTestSuite, SpWeightedFinalTestSuite, SpWeightedInitialTestSuite, SpWeightedZeroWeightTestSuite,
    SseStreamingTestSuite, TimeoutTestSuite, TransportTestSuite, WebSocketTestSuite, WeightedBackendTestSuite,
};

pub use edgion_backend_traffic_policy::{
    ConnectTimeoutTestSuite, EdgionBackendTrafficPolicyTestSuite, RetryBudgetTestSuite, UpstreamAuthorityTestSuite,
};

pub use edgion_backend::{
    AiBackendSecretRotationInitialTestSuite, AiBackendSecretRotationUpdatedTestSuite, AnthropicProviderTestSuite,
    CredentialExhaustionTestSuite, ResilienceTestSuite, SingleProviderTestSuite,
};

pub use grpc_route::{
    GrpcDelegationDynamicInitialTestSuite, GrpcDelegationDynamicUpdateTestSuite, GrpcDelegationTestSuite,
    GrpcMatchTestSuite, GrpcRateLimitLocalTestSuite, GrpcSessionPersistenceTestSuite, GrpcSpWeightedTestSuite,
    GrpcTestSuite, GrpcTimeoutRewriteWireTestSuite, GrpcUnmatchedRouteTestSuite, GrpcWebTestSuite,
};

pub use tcp_route::{
    GcirEnforcementTestSuite, TcpDeniedBackendRefTestSuite, TcpEspReloadAllowTestSuite, TcpEspReloadDenyTestSuite,
    TcpGeoRestrictionTestSuite, TcpIdleTimeoutTestSuite, TcpKeepaliveSmokeTestSuite, TcpStreamPluginsTestSuite,
    TcpTestSuite,
};

pub use tls_route::{
    TlsBothAbsentParentRefTestSuite, TlsDeniedBackendRefTestSuite, TlsGeoRestrictionTestSuite, TlsMultiSniTestSuite,
    TlsPassthroughTestSuite, TlsProxyProtocolTestSuite, TlsRouteTestSuite, TlsStreamPluginsTestSuite,
    TlsUnknownKindBackendTestSuite,
};

pub use udp_route::{UdpDeniedBackendRefTestSuite, UdpStreamPluginsTestSuite, UdpTestSuite};

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
    capture_csrf_pre_rotation_token, AccessLogExternTestSuite, AiGuardTestSuite, AllConditionsTestSuite,
    AllEndpointStatusTestSuite, BandwidthLimitTestSuite, BasicAuthDynamicGroupedInitialTestSuite,
    BasicAuthDynamicGroupedUpdateTestSuite, BasicAuthDynamicInitialTestSuite, BasicAuthDynamicUpdateTestSuite,
    BasicAuthTestSuite, BodyDynamicCappedTestSuite, BodyDynamicInitialTestSuite, BodyDynamicRejectedTestSuite,
    BodyDynamicUpdateTestSuite, BodyPluginsTestSuite, CanaryHotSwapFinalTestSuite, CanaryHotSwapInitialTestSuite,
    CanaryTestSuite, ConditionRefsDynamicDeletedTestSuite, ConditionRefsDynamicInitialTestSuite,
    ConditionRefsDynamicUpdatedTestSuite, CorsPluginTestSuite, CredentialInjectorDynamicDeletedTestSuite,
    CredentialInjectorDynamicInitialTestSuite, CredentialInjectorDynamicUpdatedTestSuite, CredentialInjectorTestSuite,
    CsrfPreRotationTokenGuard, CsrfSecretRotationDeletedTestSuite, CsrfSecretRotationInitialTestSuite,
    CsrfSecretRotationUpdatedTestSuite, CsrfTestSuite, CtxSetTestSuite, DeniedNamedJumpTestSuite,
    DirectEndpointTestSuite, DslBodyTestSuite, DslDynamicInitialTestSuite, DslDynamicUpdateTestSuite,
    DslGrpcErrorTestSuite, DslTerminateDynamicInitialTestSuite, DslTerminateDynamicRestoredTestSuite,
    DslTerminateDynamicUpdatedTestSuite, DslTerminateTestSuite, DslTestSuite, DslXmlDynamicTestSuite,
    DynamicExternalUpstreamTestSuite, DynamicInternalUpstreamTestSuite, EdgionConfigDataFlapInitialTestSuite,
    EdgionConfigDataFlapInvalidTestSuite, EdgionConfigDataFlapValidTestSuite, EdgionConfigDataManagedTestSuite,
    EdgionConfigDataStatusTestSuite, ExtProcTestSuite, FaultInjectionTestSuite, FormJsonTransformDynamicFormTestSuite,
    FormJsonTransformDynamicJsonTestSuite, ForwardAuthTestSuite, GeoRestrictionTestSuite,
    GlobalConnectionIpRestrictionTestSuite, GlobalPluginsSnapshotTestSuite, GuardrailEncodingTestSuite,
    GuardrailTestSuite, HeaderCertAuthTestSuite, HmacAuthTestSuite, IpRestrictionPartialGoodPluginTestSuite,
    IpRestrictionPluginTestSuite, IpRestrictionRefsPluginTestSuite, JsonSchemaValidationDynamicInvalidTestSuite,
    JsonSchemaValidationDynamicV1TestSuite, JsonSchemaValidationDynamicV2TestSuite, JsonSchemaValidationTestSuite,
    JweDecryptTestSuite, JwtAuthTestSuite, KeyAuthTestSuite, LdapAuthTestSuite, LdapAuthTlsTestSuite,
    MatchedRouteMetaTestSuite, OpenidConnectTestSuite, PathNormalizationForwardViewTestSuite,
    PathNormalizationPreserveParamTestSuite, PathNormalizationTestSuite, PluginConditionTestSuite, PluginLogsTestSuite,
    PluginValidationStatusTestSuite, ProxyRewriteTestSuite, RateLimitLocalDynamicInitialTestSuite,
    RateLimitLocalDynamicUpdateTestSuite, RateLimitLocalTestSuite, RateLimitRedisTestSuite, RealIpPluginTestSuite,
    RegionRouteTestSuite, RequestAccessPolicyHmacDynamicDeletedTestSuite,
    RequestAccessPolicyHmacDynamicInitialTestSuite, RequestAccessPolicyHmacDynamicUpdatedTestSuite,
    RequestAccessPolicyTestSuite, RequestIdPluginTestSuite, RequestMirrorTestSuite, RequestRestrictionTestSuite,
    ResolvedRefsTestSuite, ResponseRewriteTestSuite, WafDynamicBundleV1TestSuite, WafDynamicBundleV2TestSuite,
    WafDynamicConfiguredPrefixTestSuite, WafDynamicControlTestSuite, WafDynamicDeletedTestSuite,
    WafDynamicDetectionTestSuite, WafDynamicInitialTestSuite, WafDynamicInjectionTestSuite,
    WafDynamicJsonDetectionTestSuite, WafDynamicJsonV1TestSuite, WafDynamicJsonV2TestSuite,
    WafDynamicJsonZeroSourceTestSuite, WafDynamicOriginalDetectionTestSuite, WafDynamicOriginalOnTestSuite,
    WafDynamicRecoveredTestSuite, WafDynamicSelectorHighTestSuite, WafDynamicStaticSelectionTestSuite,
    WafDynamicSupportedDetectionTestSuite, WafDynamicSupportedOnTestSuite, WafDynamicSupportedRetainedTestSuite,
    WafDynamicSupportedStandardTestSuite, WafDynamicUpdatedTestSuite, WafDynamicXmlDetectionTestSuite,
    WafDynamicXmlV1TestSuite, WafDynamicXmlV2TestSuite, WasmPluginTestSuite, WebhookKeyGetTestSuite,
};

// LinkSys tests
pub use link_sys::LinkSysStatusTestSuite;

// Status tests (ReferenceGrant, BackendTLSPolicy, etc.)
pub use status::BackendTlsStatusTestSuite;
pub use status::BtlspFlapInitialTestSuite;
pub use status::BtlspFlapInvalidTestSuite;
pub use status::BtlspFlapValidTestSuite;
pub use status::EbtpAncestorStatusTestSuite;
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
pub use services::{AcmeGatewayRefOwnershipPhase, AcmeGatewayRefOwnershipTestSuite, AcmeTestSuite};

pub use edgion_backend::{
    MessagesBindingAllowedSuite, MessagesBindingRejectedSuite, MessagesCredentialBSuite, MessagesTestSuite,
    ResponsesContinuationAllowedSuite, ResponsesContinuationRejectedSuite, ResponsesTestSuite,
};
pub use edgion_plugins::{GraphqlRateLimitDynamicSuite, GraphqlRateLimitTestSuite};
pub use edgion_plugins::{GrpcJsonTranscoderDynamicSuite, GrpcJsonTranscoderTestSuite};

pub use edgion_plugins::{GraphqlOperationRouterDynamicSuite, GraphqlOperationRouterTestSuite};
