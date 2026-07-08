//! DslPlugin — implements RequestFilter and UpstreamResponseFilter for EdgionDSL scripts
//!
//! Created from DslConfig by compiling source or deserializing bytecode,
//! then instantiating a sandboxed VM. Execution is wrapped via execute_safe()
//! with catch_unwind for fault isolation.

use std::collections::HashMap;

use async_trait::async_trait;

use crate::plugins::runtime::traits::UpstreamResponseFilter;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;

use super::config::DslConfig;
use super::lang::bytecode::CompiledScript;
use super::lang::vm::{execute_safe, DslErrorPolicy, Vm, VmLimits};
use super::runtime::DslRuntime;

/// DslPlugin — runs a pre-compiled DSL script per request
pub struct DslPlugin {
    name: String,
    vm: Vm,
    error_policy: DslErrorPolicy,
    runtime: DslRuntime,
}

impl DslPlugin {
    /// Create a new DslPlugin from pre-compiled script
    pub fn new(
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

        let limits = VmLimits {
            max_steps: config.max_steps.unwrap_or(10_000),
            max_loop_iterations: config.max_loop_iterations.unwrap_or(100),
            max_call_count: config.max_call_count.unwrap_or(500),
            max_stack_depth: config.max_stack_depth.unwrap_or(128),
            max_string_len: config.max_string_len.unwrap_or(8192),
            max_list_len: config.max_list_len.unwrap_or(1024),
            max_map_size: config.max_map_size.unwrap_or(512),
        };
        let policy = config.error_policy.clone().unwrap_or_default();
        let redis_links: HashMap<String, String> = config
            .linksys_refs
            .iter()
            .map(|r| (r.name.clone(), format!("{}/{}", r.namespace, r.name)))
            .collect();
        let runtime = DslRuntime {
            max_timeout_ms: config.http_max_timeout_ms.unwrap_or(5_000),
            max_response_body_bytes: config.http_max_response_body_bytes.unwrap_or(1024 * 1024),
            block_private_ips: config.http_block_private_ips.unwrap_or(false),
            redis_links,
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

    /// Create from DslConfig (used by PluginRuntime factory for upstream response filter stage)
    pub fn from_config_response(config: &DslConfig) -> Option<Box<dyn UpstreamResponseFilter>> {
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
}

#[async_trait]
impl RequestFilter for DslPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        execute_safe(&self.vm, session, log, &self.error_policy, &self.runtime).await
    }
}

impl UpstreamResponseFilter for DslPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    /// Bridges the **sync** `UpstreamResponseFilter` hook to the async DSL VM.
    ///
    /// REQUIRES a multi-thread Tokio runtime. `block_in_place` panics with
    /// "Cannot use `block_in_place` inside a `current_thread` runtime". On a
    /// multi-thread runtime it is also *required*: a bare `Handle::block_on`
    /// from a worker thread would itself panic ("Cannot block the current
    /// thread from within a runtime"), so `block_in_place` is the only safe
    /// sync→async bridge from this worker thread.
    ///
    /// The shipped gateway always builds the Tokio default (multi-thread)
    /// runtime, so this is safe in production. Tests that drive this method
    /// must use `#[tokio::test(flavor = "multi_thread")]` — see
    /// `tests/response_builtins.rs::test_upstream_response_filter_phase`.
    ///
    /// `spawn_blocking` is not a usable alternative: `session` and `log` are
    /// non-`'static`, non-`Send` borrows, while `spawn_blocking` requires a
    /// `'static + Send` closure.
    fn run_upstream_response_filter(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(execute_safe(
                &self.vm,
                session,
                log,
                &self.error_policy,
                &self.runtime,
            ))
        })
    }
}
