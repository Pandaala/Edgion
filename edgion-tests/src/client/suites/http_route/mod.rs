// HTTP Route Test suite
// Note: HTTPS moved to EdgionTls module

// basic tests
mod basic;
mod delegation;
mod multi_port;

// Sub-modules - by function
mod backend;
mod dangling_extension_ref;
mod filters;
mod r#match;
mod protocol;
mod transport;

pub use basic::HttpTestSuite;
pub use dangling_extension_ref::DanglingExtensionRefTestSuite;
pub use delegation::{DelegationDynamicInitialTestSuite, DelegationDynamicUpdateTestSuite, DelegationTestSuite};
pub use multi_port::MultiPortTestSuite;

// Export sub-module tests
pub use backend::{
    BackendTlsSniHostOverrideTestSuite, CircuitBreakerTestSuite, DnsFlapFailingTestSuite, DnsFlapWorkingTestSuite,
    DnsResolutionFailureTestSuite, DnsResolutionTestSuite, GrpcHealthCheckTestSuite, H2MtlsPersistenceTestSuite,
    HealthCheckTestSuite, HealthCheckTransitionTestSuite, LBConsistentHashTestSuite, LBEwmaRetryOnStatusTestSuite,
    LBEwmaTestSuite, LBLeastConnRetryOnStatusTestSuite, LBLeastConnTestSuite, LBRoundRobinTestSuite,
    PanicThresholdTestSuite, PartiallyInvalidTestSuite, PerTryTimeoutTestSuite, RetryOnStatusTestSuite,
    SessionPersistenceTestSuite, SpChangeFinalTestSuite, SpChangeInitialTestSuite, SpWeightedFinalTestSuite,
    SpWeightedInitialTestSuite, SpWeightedZeroWeightTestSuite, TimeoutTestSuite, WeightedBackendTestSuite,
};
pub use filters::{
    FilterConflictTestSuite, HeaderModifierTestSuite, HttpRedirectTestSuite, HttpSecurityTestSuite,
    PercentMirrorTestSuite,
};
pub use protocol::{H2TrailerMalformedTestSuite, H2cTestSuite, WebSocketTestSuite};
pub use r#match::HttpMatchTestSuite;
pub use transport::TransportTestSuite;
