//! DSL plugin configuration — re-exported from core::edgion_dsl::config
//!
//! This module re-exports the DslConfig from the core module to maintain
//! the plugin_configs pattern where all configs are accessible from types.

pub use crate::core::gateway::plugins::http::dsl::config::DslConfig;

use crate::types::resources::edgion_plugins::PluginValidation;

impl PluginValidation for DslConfig {
    fn validation_error(&self) -> Option<String> {
        self.get_validation_error().map(str::to_owned)
    }
}
