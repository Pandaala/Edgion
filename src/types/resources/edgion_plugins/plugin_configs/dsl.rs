//! DSL plugin configuration — re-exported from core::edgion_dsl::config
//!
//! This module re-exports the DslConfig from the core module to maintain
//! the plugin_configs pattern where all configs are accessible from types.

pub use crate::core::gateway::plugins::http::dsl::config::DslConfig;

use crate::core::gateway::plugins::http::dsl::lang::vm::DslErrorPolicy;
use crate::core::gateway::plugins::http::dsl::plugin::DslPlugin;
use crate::types::resources::edgion_plugins::PluginValidation;

impl PluginValidation for DslConfig {
    fn validation_error(&self) -> Option<String> {
        if self.name.is_empty() {
            return Some("dsl plugin name is required".to_string());
        }
        if self.source.is_none() && self.bytecode.is_none() {
            return Some("either source or bytecode must be provided".to_string());
        }
        if let Some(steps) = self.max_steps {
            if steps == 0 || steps > 1_000_000 {
                return Some("maxSteps must be between 1 and 1000000".to_string());
            }
        }
        if let Some(loop_iter) = self.max_loop_iterations {
            if loop_iter == 0 || loop_iter > 100_000 {
                return Some("maxLoopIterations must be between 1 and 100000".to_string());
            }
        }
        if let Some(calls) = self.max_call_count {
            if calls == 0 || calls > 100_000 {
                return Some("maxCallCount must be between 1 and 100000".to_string());
            }
        }
        if let Some(depth) = self.max_stack_depth {
            if depth == 0 || depth > 1024 {
                return Some("maxStackDepth must be between 1 and 1024".to_string());
            }
        }
        if let Some(len) = self.max_string_len {
            if len == 0 || len > 1_048_576 {
                return Some("maxStringLen must be between 1 and 1048576".to_string());
            }
        }
        if let Some(DslErrorPolicy::DenyWith { status, .. }) = &self.error_policy {
            if *status < 100 || *status > 599 {
                return Some("errorPolicy denyWith status must be between 100 and 599".to_string());
            }
        }
        // Run the full data-plane validation path (compile source / deserialize
        // bytecode / linksys_refs / redis link names) so the control plane
        // surfaces the same failures the constructor would hit.
        if let Err(e) = DslPlugin::validate_config(self) {
            return Some(e);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_error_catches_uncompilable_source() {
        let config = DslConfig {
            name: "bad".to_string(),
            source: Some("this is not valid dsl @@@".to_string()),
            bytecode: None,
            ..Default::default()
        };
        let err = config
            .validation_error()
            .expect("uncompilable source must surface an error");
        assert!(!err.is_empty(), "error must be non-empty: {err}");
    }
}
