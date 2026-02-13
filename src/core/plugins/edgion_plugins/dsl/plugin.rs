//! DslPlugin — implements RequestFilter for EdgionDSL scripts
//!
//! Created from DslConfig by compiling source or deserializing bytecode,
//! then instantiating a sandboxed VM. Execution is wrapped via execute_safe()
//! with catch_unwind for fault isolation.

use async_trait::async_trait;

use crate::core::plugins::plugin_runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;

use super::config::DslConfig;
use super::lang::bytecode::CompiledScript;
use super::lang::validator::{ValidationLimits, compile_dsl_source};
use super::lang::vm::{DslErrorPolicy, Vm, VmLimits, execute_safe};

/// DslPlugin — runs a pre-compiled DSL script per request
pub struct DslPlugin {
    name: String,
    vm: Vm,
    error_policy: DslErrorPolicy,
}

impl DslPlugin {
    /// Create a new DslPlugin from pre-compiled script
    pub fn new(
        name: String,
        script: CompiledScript,
        limits: VmLimits,
        error_policy: DslErrorPolicy,
    ) -> Result<Self, String> {
        let vm = Vm::new(script, limits).map_err(|e| format!("VM creation error: {}", e))?;
        Ok(Self {
            name,
            vm,
            error_policy,
        })
    }

    /// Compile source code to CompiledScript, or deserialize from pre-compiled bytecode.
    ///
    /// Priority: bytecode (pre-compiled) > source (compile on-the-fly).
    /// This allows YAML users to write inline `source` code directly, while
    /// the controller can also pre-compile to `bytecode` for production use.
    fn resolve_script(config: &DslConfig) -> Result<CompiledScript, String> {
        // 1. Pre-compiled bytecode takes priority
        if let Some(bytecode_str) = &config.bytecode {
            return CompiledScript::deserialize_base64(bytecode_str)
                .map_err(|e| format!("bytecode deserialization failed: {}", e));
        }

        // 2. Compile from source code
        if let Some(source) = &config.source {
            let bytecode_b64 = compile_dsl_source(source, &ValidationLimits::default())
                .map_err(|errs| format!("DSL compile errors: {}", errs.join("; ")))?;
            return CompiledScript::deserialize_base64(&bytecode_b64)
                .map_err(|e| format!("compiled bytecode deserialization failed: {}", e));
        }

        Err("either source or bytecode must be provided".to_string())
    }

    /// Create from DslConfig (used by PluginRuntime factory)
    pub fn from_config(config: &DslConfig) -> Option<Box<dyn RequestFilter>> {
        let script = match Self::resolve_script(config) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("DSL plugin '{}' failed to load: {}", config.name, e);
                return None;
            }
        };

        let limits = VmLimits {
            max_steps: config.max_steps.unwrap_or(10_000),
            max_loop_iterations: config.max_loop_iterations.unwrap_or(100),
            max_call_count: config.max_call_count.unwrap_or(500),
            ..VmLimits::default()
        };

        let policy = config.error_policy.clone().unwrap_or_default();
        let plugin = DslPlugin::new(config.name.clone(), script, limits, policy).ok()?;
        Some(Box::new(plugin))
    }
}

#[async_trait]
impl RequestFilter for DslPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        execute_safe(&self.vm, session, log, &self.error_policy)
    }
}
