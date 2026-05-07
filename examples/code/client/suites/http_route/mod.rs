// HTTP Route Test suite
// Note: HTTPS moved to EdgionTls module

// basic tests
mod basic;
mod multi_port;

// Sub-modules - by function
mod backend;
mod dangling_extension_ref;
mod filters;
mod r#match;
mod protocol;

// basic tests
pub use basic::HttpTestSuite;
pub use dangling_extension_ref::DanglingExtensionRefTestSuite;
pub use multi_port::MultiPortTestSuite;

// Export sub-module tests
pub use backend::{
    CircuitBreakerTestSuite, DnsResolutionFailureTestSuite, GrpcHealthCheckTestSuite, HealthCheckTestSuite,
    HealthCheckTransitionTestSuite, LBConsistentHashTestSuite, LBLeastConnRetryOnStatusTestSuite, LBLeastConnTestSuite,
    LBRoundRobinTestSuite, PartiallyInvalidTestSuite, RetryOnStatusTestSuite, SessionPersistenceTestSuite,
    SpChangeFinalTestSuite, SpChangeInitialTestSuite, TimeoutTestSuite, WeightedBackendTestSuite,
};
pub use filters::{
    FilterConflictTestSuite, HeaderModifierTestSuite, HttpRedirectTestSuite, HttpSecurityTestSuite,
    PercentMirrorTestSuite,
};
pub use protocol::{H2cTestSuite, WebSocketTestSuite};
pub use r#match::HttpMatchTestSuite;
