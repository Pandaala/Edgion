//! DSL plugin configuration — re-exported from core::edgion_dsl::config
//!
//! This module re-exports the DslConfig from the core module to maintain
//! the plugin_configs pattern where all configs are accessible from types.

pub use crate::core::gateway::plugins::http::dsl::config::DslConfig;
