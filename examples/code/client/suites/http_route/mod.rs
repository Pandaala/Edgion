// HTTP Route Test suite
// Note: HTTPS moved to EdgionTls module

// basic tests
mod basic;
mod multi_port;

// Sub-modules - by function
mod backend;
mod filters;
mod r#match;
mod protocol;

// basic tests
pub use basic::HttpTestSuite;
pub use multi_port::MultiPortTestSuite;

// Export sub-module tests
pub use backend::{
    CircuitBreakerTestSuite, GrpcHealthCheckTestSuite, HealthCheckTestSuite, HealthCheckTransitionTestSuite,
    LBConsistentHashTestSuite, LBRoundRobinTestSuite, PartiallyInvalidTestSuite, RetryOnStatusTestSuite,
    SessionPersistenceTestSuite, TimeoutTestSuite, WeightedBackendTestSuite,
};
pub use filters::{
    FilterConflictTestSuite, HeaderModifierTestSuite, HttpRedirectTestSuite, HttpSecurityTestSuite,
    PercentMirrorTestSuite,
};
pub use protocol::{H2cTestSuite, WebSocketTestSuite};
pub use r#match::HttpMatchTestSuite;
