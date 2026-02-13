//! Configuration for EdgionDSL plugin

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::lang::validator::{ValidationLimits, compile_dsl_source};
use super::lang::vm::DslErrorPolicy;

/// DSL plugin configuration — stored in EdgionPlugin enum
///
/// Users can provide inline `source` code in YAML, which is automatically
/// compiled to bytecode during preparse. Pre-compiled `bytecode` is also
/// supported for production deployments.
///
/// YAML example:
/// ```yaml
/// type: Dsl
/// config:
///   name: "ip-filter"
///   source: |
///     let ip = req.header("X-Real-IP")
///     if ip == nil {
///       return deny(403, "Missing IP")
///     }
///   maxSteps: 10000
///   errorPolicy: deny
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DslConfig {
    /// Script name (for logging and identification)
    pub name: String,

    /// EdgionDSL source code — inline script written by the user.
    /// Automatically compiled to bytecode during preparse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    /// Pre-compiled bytecode (base64-encoded JSON, populated by controller
    /// or via external tooling). Takes priority over `source` if both present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytecode: Option<String>,

    /// Max VM instructions per execution (default: 10000)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u32>,

    /// Max loop iterations per loop (default: 100)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_loop_iterations: Option<u32>,

    /// Max builtin API calls per execution (default: 500)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_call_count: Option<u32>,

    /// Error handling policy (default: ignore = fail-open)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_policy: Option<DslErrorPolicy>,

    // === Validation cache (runtime only) ===
    #[serde(skip)]
    #[schemars(skip)]
    pub validation_error: Option<String>,
}

impl Default for DslConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            source: None,
            bytecode: None,
            max_steps: None,
            max_loop_iterations: None,
            max_call_count: None,
            error_policy: None,
            validation_error: None,
        }
    }
}

impl DslConfig {
    /// Return validation error if config is invalid.
    /// Called during preparse for status reporting.
    ///
    /// This performs both structural validation (required fields, limits)
    /// and DSL compilation validation (parse + compile + bytecode checks).
    pub fn get_validation_error(&self) -> Option<&str> {
        // Check cached validation error first
        if let Some(ref err) = self.validation_error {
            return Some(err.as_str());
        }

        if self.name.is_empty() {
            return Some("dsl plugin name is required");
        }
        if self.source.is_none() && self.bytecode.is_none() {
            return Some("either source or bytecode must be provided");
        }
        if let Some(steps) = self.max_steps {
            if steps == 0 || steps > 1_000_000 {
                return Some("maxSteps must be between 1 and 1000000");
            }
        }
        if let Some(loop_iter) = self.max_loop_iterations {
            if loop_iter == 0 || loop_iter > 100_000 {
                return Some("maxLoopIterations must be between 1 and 100000");
            }
        }
        if let Some(calls) = self.max_call_count {
            if calls == 0 || calls > 100_000 {
                return Some("maxCallCount must be between 1 and 100000");
            }
        }
        None
    }

    /// Validate that the source code compiles successfully.
    /// Returns error description or None if valid.
    /// Used for deeper validation beyond structural checks.
    pub fn validate_source_compilation(&self) -> Option<String> {
        if let Some(source) = &self.source {
            if let Err(errs) = compile_dsl_source(source, &ValidationLimits::default()) {
                return Some(format!("DSL compile errors: {}", errs.join("; ")));
            }
        }
        None
    }
}
