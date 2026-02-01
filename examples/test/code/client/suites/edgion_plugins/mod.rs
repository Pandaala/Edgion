// EdgionPlugins Test Suite
//
// Tests for EdgionPlugins functionality (independent from Gateway tests)
// Corresponds to conf/EdgionPlugins/ directory structure

mod debug_access_log;
mod jwt_auth;
mod plugin_condition;
mod proxy_rewrite;

pub use debug_access_log::PluginLogsTestSuite;
pub use jwt_auth::JwtAuthTestSuite;
pub use plugin_condition::{AllConditionsTestSuite, PluginConditionTestSuite};
pub use proxy_rewrite::ProxyRewriteTestSuite;
