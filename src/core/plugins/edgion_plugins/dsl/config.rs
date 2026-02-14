//! Configuration for EdgionDSL plugin

use std::sync::Mutex;

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
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
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

    /// Max value stack depth (default: 128)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_stack_depth: Option<usize>,

    /// Max string length from concatenation (default: 8192)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_string_len: Option<usize>,

    /// Error handling policy (default: ignore = fail-open)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_policy: Option<DslErrorPolicy>,

    // === Validation cache (runtime only) ===
    #[serde(skip)]
    #[schemars(skip)]
    pub validation_error: Option<String>,

    /// Cached compiled bytecode (base64) from validation phase.
    /// Avoids double compilation: validate_source_compilation() compiles once,
    /// plugin.rs::resolve_script() reuses this cache.
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) compiled_bytecode_cache: Mutex<Option<String>>,
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
            max_stack_depth: None,
            max_string_len: None,
            error_policy: None,
            validation_error: None,
            compiled_bytecode_cache: Mutex::new(None),
        }
    }
}

impl Clone for DslConfig {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            source: self.source.clone(),
            bytecode: self.bytecode.clone(),
            max_steps: self.max_steps,
            max_loop_iterations: self.max_loop_iterations,
            max_call_count: self.max_call_count,
            max_stack_depth: self.max_stack_depth,
            max_string_len: self.max_string_len,
            error_policy: self.error_policy.clone(),
            validation_error: self.validation_error.clone(),
            // Cache is not cloned — re-compilation will happen if needed
            compiled_bytecode_cache: Mutex::new(None),
        }
    }
}

impl DslConfig {
    /// Return validation error if config is invalid (structural checks only).
    /// For other plugins that call this with `Option<&str>` convention.
    pub fn get_validation_error(&self) -> Option<&str> {
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
        if let Some(depth) = self.max_stack_depth {
            if depth == 0 || depth > 1024 {
                return Some("maxStackDepth must be between 1 and 1024");
            }
        }
        if let Some(len) = self.max_string_len {
            if len == 0 || len > 1_048_576 {
                return Some("maxStringLen must be between 1 and 1048576");
            }
        }
        // Validate DenyWith status range
        if let Some(DslErrorPolicy::DenyWith { status, .. }) = &self.error_policy {
            if *status < 100 || *status > 599 {
                return Some("errorPolicy denyWith status must be between 100 and 599");
            }
        }
        None
    }

    /// Return validation error as an owned String.
    /// This performs structural validation AND source compilation validation.
    /// Called from runtime.rs for DSL plugins specifically.
    pub fn get_validation_error_owned(&self) -> Option<String> {
        // Structural checks first (reuse the static method)
        if let Some(err) = self.get_validation_error() {
            return Some(err.to_string());
        }
        // Validate source compilation (dynamic error)
        if let Some(err) = self.validate_source_compilation() {
            return Some(err);
        }
        None
    }

    /// Validate that the source code compiles successfully.
    /// On success, caches compiled bytecode for later use.
    /// Returns error description or None if valid.
    pub fn validate_source_compilation(&self) -> Option<String> {
        if let Some(source) = &self.source {
            match compile_dsl_source(source, &ValidationLimits::default()) {
                Err(errs) => {
                    return Some(format!("DSL compile errors: {}", errs.join("; ")));
                }
                Ok(bytecode_b64) => {
                    // Cache compiled bytecode to avoid double compilation
                    if let Ok(mut cache) = self.compiled_bytecode_cache.lock() {
                        *cache = Some(bytecode_b64);
                    }
                }
            }
        }
        None
    }

    /// Read the cached compiled bytecode (if any) without consuming it.
    pub(crate) fn get_compiled_bytecode(&self) -> Option<String> {
        self.compiled_bytecode_cache.lock().ok()?.clone()
    }
}
