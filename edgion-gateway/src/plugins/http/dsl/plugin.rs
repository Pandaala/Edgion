//! DslPlugin — implements RequestFilter and the async UpstreamResponse stage for EdgionDSL scripts
//!
//! Created from DslConfig by compiling source or deserializing bytecode,
//! then instantiating a sandboxed VM. Execution is wrapped via execute_safe()
//! with catch_unwind for fault isolation.

use std::collections::HashMap;

use async_trait::async_trait;

use crate::plugins::runtime::traits::UpstreamResponse;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;

use super::config::DslConfig;
use super::lang::bytecode::CompiledScript;
use super::lang::vm::{execute_safe, DslErrorPolicy, Vm, VmLimits};
use super::runtime::{DslRateLimits, DslRuntime};

/// DslPlugin — runs a pre-compiled DSL script per request
pub struct DslPlugin {
    name: String,
    vm: Vm,
    error_policy: DslErrorPolicy,
    runtime: DslRuntime,
}

impl DslPlugin {
    /// Create a new DslPlugin from pre-compiled script
    fn new(
        name: String,
        script: CompiledScript,
        limits: VmLimits,
        error_policy: DslErrorPolicy,
        runtime: DslRuntime,
    ) -> Result<Self, String> {
        let vm = Vm::new(script, limits).map_err(|e| format!("VM creation error: {}", e))?;
        Ok(Self {
            name,
            vm,
            error_policy,
            runtime,
        })
    }

    /// Extract script, limits, error policy, and runtime from DslConfig.
    ///
    /// `DslConfig::compile_and_validate` (edgion-resources, backed by the
    /// `edgion-dsl` crate) is the single source of truth for the static
    /// validation pipeline; this function reuses the `CompiledScript` it
    /// returns rather than running the same pipeline a second time.
    fn resolve_parts(config: &DslConfig) -> Result<(CompiledScript, VmLimits, DslErrorPolicy, DslRuntime), String> {
        let script = config.compile_and_validate()?;
        let resolved = config.resolve_runtime_limits()?;

        let limits = VmLimits {
            max_steps: resolved.max_steps,
            max_loop_iterations: resolved.max_loop_iterations,
            max_call_count: resolved.max_call_count,
            max_stack_depth: resolved.max_stack_depth,
            max_string_len: resolved.max_string_len,
            max_list_len: resolved.max_list_len,
            max_map_size: resolved.max_map_size,
        };
        let policy = config.error_policy.clone().unwrap_or_default();
        let redis_links: HashMap<String, String> = config
            .linksys_refs
            .iter()
            .map(|r| (r.name.clone(), format!("{}/{}", r.namespace, r.name)))
            .collect();
        let runtime = DslRuntime {
            max_timeout_ms: resolved.http_max_timeout_ms,
            max_response_body_bytes: resolved.http_max_response_body_bytes,
            max_request_body_bytes: resolved.http_max_request_body_bytes,
            block_private_ips: config.http_block_private_ips.unwrap_or(false),
            redis_links,
            rate_limits: DslRateLimits::from_config(config)?,
            max_body_scan_passes: resolved.max_body_scan_passes,
        };
        Ok((script, limits, policy, runtime))
    }

    /// Create from DslConfig (used by PluginRuntime factory for request stage)
    pub fn from_config(config: &DslConfig) -> Option<Box<dyn RequestFilter>> {
        let (script, limits, policy, runtime) = match Self::resolve_parts(config) {
            Ok(parts) => parts,
            Err(e) => {
                tracing::error!("DSL plugin '{}' failed to load: {}", config.name, e);
                return None;
            }
        };
        match DslPlugin::new(config.name.clone(), script, limits, policy, runtime) {
            Ok(p) => Some(Box::new(p)),
            Err(e) => {
                tracing::error!("DSL plugin '{}' VM init failed: {}", config.name, e);
                None
            }
        }
    }

    /// Create from DslConfig (used by PluginRuntime factory for the async upstream response stage)
    pub fn from_config_response(config: &DslConfig) -> Option<Box<dyn UpstreamResponse>> {
        let (script, limits, policy, runtime) = match Self::resolve_parts(config) {
            Ok(parts) => parts,
            Err(e) => {
                tracing::error!("DSL plugin '{}' failed to load: {}", config.name, e);
                return None;
            }
        };
        if script.references_request_body_mutation() {
            tracing::error!(
                "DSL plugin '{}' uses request-body mutation outside the request stage",
                config.name
            );
            return None;
        }
        match DslPlugin::new(config.name.clone(), script, limits, policy, runtime) {
            Ok(p) => Some(Box::new(p)),
            Err(e) => {
                tracing::error!("DSL plugin '{}' VM init failed: {}", config.name, e);
                None
            }
        }
    }
}

#[async_trait]
impl RequestFilter for DslPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn requires_materialized_request_body(&self) -> bool {
        false
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        execute_safe(&self.vm, session, log, &self.error_policy, &self.runtime).await
    }
}

/// The response-header DSL stage runs in Pingora's async `response_filter`
/// hook, so the VM is awaited directly. It must never be bridged into the
/// synchronous `UpstreamResponseFilter` stage: blocking on `http.call` or
/// `redis.*` there would pin a worker thread and panic on the current-thread
/// runtimes Pingora builds when `work_stealing` is disabled.
#[async_trait]
impl UpstreamResponse for DslPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_upstream_response(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        execute_safe(&self.vm, session, log, &self.error_policy, &self.runtime).await
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/dsl/plugin_test.rs"]
mod tests;
