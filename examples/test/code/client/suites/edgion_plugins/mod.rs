// EdgionPlugins Test Suite
//
// Tests for EdgionPlugins functionality (independent from Gateway tests)
// Corresponds to conf/EdgionPlugins/ directory structure

mod debug_access_log;
mod plugin_condition;

pub use debug_access_log::PluginLogsTestSuite;
pub use plugin_condition::{AllConditionsTestSuite, PluginConditionTestSuite};
