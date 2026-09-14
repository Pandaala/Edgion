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

    /// Create from DslConfig (used by PluginRuntime factory for upstream response filter stage)
    pub fn from_config_response(config: &DslConfig) -> Option<Box<dyn UpstreamResponseFilter>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::common::external_http::MAX_OUTBOUND_TIMEOUT_MS;
    use edgion_resources::resources::edgion_plugins::plugin_configs::dsl::{
        DSL_MAX_BODY_SCAN_PASSES, DSL_MAX_CALL_COUNT, DSL_MAX_HTTP_REQUEST_BODY_BYTES,
        DSL_MAX_HTTP_RESPONSE_BODY_BYTES, DSL_MAX_LIST_LEN, DSL_MAX_LOOP_ITERATIONS, DSL_MAX_MAP_SIZE,
        DSL_MAX_STACK_DEPTH, DSL_MAX_STEPS, DSL_MAX_STRING_LEN,
    };

    fn config(source: &str) -> DslConfig {
        DslConfig {
            name: "body-test".to_string(),
            source: Some(source.to_string()),
            ..Default::default()
        }
    }

    fn config_with_name(name: String) -> DslConfig {
        DslConfig {
            name,
            source: Some("return next()".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn standalone_factories_enforce_plugin_log_name_capacity() {
        assert_eq!(DslConfig::MAX_NAME_BYTES, crate::plugins::runtime::log::NAME_CAPACITY);

        let valid = config_with_name("a".repeat(DslConfig::MAX_NAME_BYTES));
        let request_filter = DslPlugin::from_config(&valid).expect("36-byte request plugin name must be accepted");
        assert_eq!(request_filter.name(), valid.name);
        let response_filter =
            DslPlugin::from_config_response(&valid).expect("36-byte response plugin name must be accepted");
        assert_eq!(response_filter.name(), valid.name);

        let overlong = config_with_name("a".repeat(DslConfig::MAX_NAME_BYTES + 1));
        assert!(DslPlugin::from_config(&overlong).is_none());
        assert!(DslPlugin::from_config_response(&overlong).is_none());
    }

    #[test]
    fn standalone_factories_reject_unbounded_runtime_limits() {
        for (field, limit_value) in [
            ("maxSteps", u64::from(DSL_MAX_STEPS) + 1),
            ("maxLoopIterations", u64::from(DSL_MAX_LOOP_ITERATIONS) + 1),
            ("maxCallCount", u64::from(DSL_MAX_CALL_COUNT) + 1),
            ("maxStackDepth", DSL_MAX_STACK_DEPTH as u64 + 1),
            ("maxStringLen", DSL_MAX_STRING_LEN as u64 + 1),
            ("maxListLen", DSL_MAX_LIST_LEN as u64 + 1),
            ("maxMapSize", DSL_MAX_MAP_SIZE as u64 + 1),
            ("maxBodyScanPasses", u64::from(DSL_MAX_BODY_SCAN_PASSES) + 1),
            ("httpMaxTimeoutMs", MAX_OUTBOUND_TIMEOUT_MS + 1),
            ("httpMaxResponseBodyBytes", DSL_MAX_HTTP_RESPONSE_BODY_BYTES as u64 + 1),
            ("httpMaxRequestBodyBytes", DSL_MAX_HTTP_REQUEST_BODY_BYTES as u64 + 1),
        ] {
            let mut config_value = serde_json::json!({
                "name": "factory-limits",
                "source": "return next()"
            });
            config_value[field] = serde_json::json!(limit_value);
            let config: DslConfig = serde_json::from_value(config_value).unwrap();
            assert!(
                DslPlugin::from_config(&config).is_none(),
                "request factory accepted invalid {field}"
            );
            assert!(
                DslPlugin::from_config_response(&config).is_none(),
                "response factory accepted invalid {field}"
            );
        }
    }

    #[test]
    fn dsl_request_filter_uses_capture_only_body_gate() {
        let filter = DslPlugin::from_config(&config("let found = req.body.contains(\"x\")")).unwrap();
        assert!(!filter.requires_materialized_request_body());
    }

    #[test]
    fn request_body_call_limit_resolves_default_and_explicit_values() {
        let (_, _, _, default_runtime) = DslPlugin::resolve_parts(&config("return next()")).unwrap();
        assert_eq!(default_runtime.max_request_body_bytes, 1024 * 1024);

        let mut explicit = config("return next()");
        explicit.http_max_request_body_bytes = Some(64 * 1024);
        let (_, _, _, explicit_runtime) = DslPlugin::resolve_parts(&explicit).unwrap();
        assert_eq!(explicit_runtime.max_request_body_bytes, 64 * 1024);
    }

    #[test]
    fn response_factory_rejects_request_body_mutation() {
        assert!(DslPlugin::from_config_response(&config("req.body.clear()")).is_none());
    }

    #[test]
    fn response_factory_allows_cached_request_body_reads() {
        assert!(DslPlugin::from_config_response(&config("let found = req.body.contains(\"x\")")).is_some());
    }
}
