// HTTP Route Test suite
// Note: HTTPS moved to EdgionTls module

// basic tests
mod basic;

// Sub-modules - by function
mod backend;
mod filters;
mod r#match;
mod protocol;

// 导出basic tests
pub use basic::HttpTestSuite;

// Export sub-module tests
pub use backend::{LBConsistentHashTestSuite, LBRoundRobinTestSuite, TimeoutTestSuite, WeightedBackendTestSuite};
pub use filters::{HeaderModifierTestSuite, HttpRedirectTestSuite, HttpSecurityTestSuite};
pub use protocol::WebSocketTestSuite;
pub use r#match::HttpMatchTestSuite;
