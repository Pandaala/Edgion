// HTTP Route 测试套件

mod basic;
mod https;
mod lb_policy;
mod matches;
mod redirect;
mod security;
mod timeout;
mod websocket;
mod weighted_backend;

pub use basic::HttpTestSuite;
pub use https::HttpsTestSuite;
pub use lb_policy::LBPolicyTestSuite;
pub use matches::HttpMatchTestSuite;
pub use redirect::HttpRedirectTestSuite;
pub use security::HttpSecurityTestSuite;
pub use timeout::TimeoutTestSuite;
pub use websocket::WebSocketTestSuite;
pub use weighted_backend::WeightedBackendTestSuite;
