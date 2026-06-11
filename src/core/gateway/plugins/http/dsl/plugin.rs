//! DslPlugin — implements RequestFilter and UpstreamResponseFilter for EdgionDSL scripts
//!
//! Created from DslConfig by compiling source or deserializing bytecode,
//! then instantiating a sandboxed VM. Execution is wrapped via execute_safe()
//! with catch_unwind for fault isolation.

use std::collections::HashMap;

use async_trait::async_trait;

use crate::core::gateway::plugins::runtime::traits::UpstreamResponseFilter;
use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;

use super::config::DslConfig;
use super::lang::bytecode::{CompiledScript, OpCode};
use super::lang::validator::{compile_dsl_source, ValidationLimits, Validator};
use super::lang::vm::{execute_safe, DslErrorPolicy, Vm, VmLimits};
use super::runtime::DslRuntime;

/// True for zero-input, single-value-push opcodes. When the `argc` opcodes
/// preceding a `CallBuiltin` are all of this kind, they map 1:1 to the argc
/// arguments (each argument is exactly one instruction), so the deepest push at
/// `i-argc` is provably the first argument. Any other opcode consumes stack,
/// meaning an argument spanned multiple instructions and the positional guess is
/// unreliable. This is an explicit allowlist: unknown / future opcodes default
/// to "not a single push", keeping `check_redis_link_names` conservative.
fn is_single_push_op(op: &OpCode) -> bool {
    matches!(
        op,
        OpCode::LoadConst(_) | OpCode::LoadNil | OpCode::LoadTrue | OpCode::LoadFalse | OpCode::GetLocal(_)
    )
}

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

    /// Compile source code to CompiledScript, or deserialize from pre-compiled bytecode.
    ///
    /// Priority: bytecode (pre-compiled) > source (compile on-the-fly).
    /// This allows YAML users to write inline `source` code directly, while
    /// the controller can also pre-compile to `bytecode` for production use.
    pub(crate) fn resolve_script(config: &DslConfig) -> Result<CompiledScript, String> {
        let val_limits = ValidationLimits {
            max_map_size: config.max_map_size.unwrap_or(512),
            ..ValidationLimits::default()
        };

        // 1. Pre-compiled bytecode takes priority
        if let Some(bytecode_str) = &config.bytecode {
            let script = CompiledScript::deserialize_base64(bytecode_str)
                .map_err(|e| format!("bytecode deserialization failed: {}", e))?;

            // Validate pre-compiled bytecode to prevent malicious payloads
            let validator = Validator::new(val_limits);
            let errors = validator.validate(&script);
            if !errors.is_empty() {
                let msgs: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
                return Err(format!("bytecode validation failed: {}", msgs.join("; ")));
            }

            return Ok(script);
        }

        // 2. Compile from source code
        if let Some(source) = &config.source {
            let bytecode_b64 = compile_dsl_source(source, &val_limits)
                .map_err(|errs| format!("DSL compile errors: {}", errs.join("; ")))?;
            return CompiledScript::deserialize_base64(&bytecode_b64)
                .map_err(|e| format!("compiled bytecode deserialization failed: {}", e));
        }

        Err("either source or bytecode must be provided".to_string())
    }

    /// Best-effort, fail-open compile-time check that every `redis.*` builtin's
    /// link name appears in `linksys_refs`. It only rejects when the bytecode
    /// layout unambiguously identifies the link-name argument as a string
    /// literal; ambiguous layouts are skipped (never rejected). See the inline
    /// notes for the layout guarantee and the remaining false-negative case.
    fn check_redis_link_names(config: &DslConfig, script: &CompiledScript) -> Result<(), String> {
        // Compile-time link-name check (best-effort, fail-open):
        // Scan bytecode for redis.* CallBuiltin at index i. The first argument
        // (link name) is pushed deepest, at index i-argc — but only when every
        // argument is a single-instruction push, so that the argc opcodes
        // preceding the call map 1:1 to the argc arguments.
        //
        // Before trusting that guess we verify the layout is unambiguous: all
        // argc opcodes in `[i-argc, i)` must be zero-input single pushes. This
        // is provably equivalent to "every argument is a single instruction"
        // (a stack-consuming opcode in the window means some argument spanned
        // multiple instructions and i-argc no longer points at the link name).
        // When the layout is ambiguous we skip the check (fail-open) rather than
        // reject: a positional miss must never fail-close a valid config — the
        // runtime ok=false path is the backstop for an undeclared link.
        //
        // Net effect: false negatives are possible (a typo'd link behind a
        // multi-instruction sibling argument is caught only at runtime), but a
        // valid config is never rejected. The check still catches the common
        // case of all-literal redis arguments and typos.
        use super::lang::bytecode::Constant;
        let valid_names: std::collections::HashSet<&str> =
            config.linksys_refs.iter().map(|r| r.name.as_str()).collect();
        for (i, op) in script.code.iter().enumerate() {
            if let OpCode::CallBuiltin(id, argc) = op {
                if id.is_redis_builtin() && *argc >= 1 {
                    let first_arg_pos = i.wrapping_sub(*argc as usize);
                    if first_arg_pos >= i {
                        continue;
                    }
                    // Only enforce when the positional guess is provably correct.
                    if !script.code[first_arg_pos..i].iter().all(is_single_push_op) {
                        continue;
                    }
                    if let Some(OpCode::LoadConst(k)) = script.code.get(first_arg_pos) {
                        if let Some(Constant::Str(link_name)) = script.constants.get(*k as usize) {
                            if !valid_names.contains(link_name.as_str()) {
                                return Err(format!("redis.* uses link '{}' which is not in linkSysRefs", link_name));
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Validate a `DslConfig` exactly as far as construction would: resolve the
    /// script (compile `source` / deserialize+validate `bytecode`), check the
    /// `linksys_refs` config, and run the redis link-name scan. Returns the
    /// validated `CompiledScript` on success so callers (notably `resolve_parts`)
    /// can reuse it without paying for a second deserialize / validate cycle.
    /// This is the single validation entry point used by
    /// `DslConfig::validation_error()` so the control plane sees the same
    /// failures the data-plane constructor would hit.
    ///
    /// Note: VM init (`Vm::new`) is intentionally NOT run here — it is the one
    /// failure that cannot be predicted statically and is covered by the
    /// data-plane seal backstop instead.
    pub(crate) fn validate_config(config: &DslConfig) -> Result<CompiledScript, String> {
        let script = Self::resolve_script(config)?;
        config.validate()?;
        Self::check_redis_link_names(config, &script)?;
        Ok(script)
    }

    /// Extract script, limits, error policy, and runtime from DslConfig.
    ///
    /// `validate_config` is the single source of truth for the three validation
    /// steps (resolve_script / config.validate / check_redis_link_names); this
    /// function reuses the `CompiledScript` it returns rather than running the
    /// same pipeline a second time.
    fn resolve_parts(config: &DslConfig) -> Result<(CompiledScript, VmLimits, DslErrorPolicy, DslRuntime), String> {
        let script = Self::validate_config(config)?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_config_rejects_uncompilable_source() {
        use super::super::config::DslConfig;
        let config = DslConfig {
            name: "bad".to_string(),
            source: Some("this is not valid dsl @@@".to_string()),
            bytecode: None,
            ..Default::default()
        };
        let err = DslPlugin::validate_config(&config).expect_err("uncompilable source must fail");
        assert!(!err.is_empty(), "error string must be non-empty: {err}");
    }

    #[test]
    fn validate_config_accepts_valid_source() {
        use super::super::config::DslConfig;
        let config = DslConfig {
            name: "ok".to_string(),
            source: Some("let a = 1".to_string()),
            bytecode: None,
            ..Default::default()
        };
        assert!(
            DslPlugin::validate_config(&config).is_ok(),
            "a trivial valid script must pass validation"
        );
    }

    #[test]
    fn validate_config_rejects_unknown_redis_link() {
        use super::super::config::DslConfig;
        // The script is syntactically valid DSL: `redis.get` takes a link alias
        // as its first (string-literal) argument. `linksys_refs` is empty, so
        // the link "mylink" is undeclared and the redis link-name scan must
        // reject the config after `resolve_script` succeeds.
        let config = DslConfig {
            name: "unknown-link".to_string(),
            source: Some("let r = redis.get(\"mylink\", \"mykey\")".to_string()),
            bytecode: None,
            ..Default::default()
        };
        let err = DslPlugin::validate_config(&config).expect_err("redis call referencing an undeclared link must fail");
        assert!(
            err.contains("linkSysRefs"),
            "error message must mention linkSysRefs, got: {err}"
        );
    }

    #[test]
    fn validate_config_accepts_redis_with_multi_instruction_sibling_arg() {
        use super::super::config::DslConfig;
        use edgion_resources::resources::edgion_plugins::plugin_configs::LinkSysRef;
        // Regression for F-015: a non-literal (multi-instruction) argument after
        // the link name used to shift the i-argc positional guess onto an
        // unrelated LoadConst ("X-Key"), wrongly rejecting this valid config and
        // fail-closing the whole route with a 500. The link name "cache" is
        // correctly declared; validation must pass.
        let config = DslConfig {
            name: "valid-link-dynamic-key".to_string(),
            source: Some("let r = redis.get(\"cache\", req.header(\"X-Key\"))".to_string()),
            bytecode: None,
            linksys_refs: vec![LinkSysRef {
                namespace: "default".to_string(),
                name: "cache".to_string(),
            }],
            ..Default::default()
        };
        assert!(
            DslPlugin::validate_config(&config).is_ok(),
            "valid redis link with a multi-instruction sibling argument must not be rejected"
        );
    }

    #[test]
    fn validate_config_skips_check_when_layout_is_ambiguous() {
        use super::super::config::DslConfig;
        // Documented fail-open behavior: when a sibling argument is a
        // multi-instruction sub-expression, the positional guess is unreliable,
        // so the link-name check is skipped rather than risk a false rejection.
        // Here the link "typo" is undeclared, but because the key argument is
        // multi-instruction the config still passes static validation (the
        // undeclared link surfaces at runtime via ok=false, not as a 500).
        let config = DslConfig {
            name: "ambiguous-layout".to_string(),
            source: Some("let r = redis.get(\"typo\", req.header(\"X-Key\"))".to_string()),
            bytecode: None,
            ..Default::default()
        };
        assert!(
            DslPlugin::validate_config(&config).is_ok(),
            "ambiguous bytecode layout must fail-open (skip the link-name check), not reject"
        );
    }
}
