// EdgionPlugins Test Suite
//
// Tests for EdgionPlugins functionality (independent from Gateway tests)
// Corresponds to conf/EdgionPlugins/ directory structure

mod bandwidth_limit;
mod ctx_set;
mod debug_access_log;
mod forward_auth;
mod jwt_auth;
mod key_auth;
mod plugin_condition;
mod proxy_rewrite;
mod rate_limit;
mod real_ip;
mod request_restriction;
mod response_rewrite;

pub use bandwidth_limit::BandwidthLimitTestSuite;
pub use ctx_set::CtxSetTestSuite;
pub use debug_access_log::PluginLogsTestSuite;
pub use forward_auth::ForwardAuthTestSuite;
pub use jwt_auth::JwtAuthTestSuite;
pub use key_auth::KeyAuthTestSuite;
pub use plugin_condition::{AllConditionsTestSuite, PluginConditionTestSuite};
pub use proxy_rewrite::ProxyRewriteTestSuite;
pub use rate_limit::RateLimitTestSuite;
pub use real_ip::RealIpPluginTestSuite;
pub use request_restriction::RequestRestrictionTestSuite;
pub use response_rewrite::ResponseRewriteTestSuite;
