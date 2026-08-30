//! Config-independent script resolution and static redis link-name check.
//!
//! These are the compile/validate entry points shared by the control plane
//! (`edgion-resources`' `DslConfig::compile_and_validate`) and the data plane
//! (`edgion-gateway`'s `DslPlugin`). They take plain values rather than the
//! `DslConfig` schema type so this crate stays a leaf with no
//! workspace-internal dependencies.

use std::collections::HashSet;

use crate::bytecode::{CompiledScript, Constant, OpCode};
use crate::validator::{compile_dsl_source, ValidationLimits, Validator};

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

/// Compile source code to CompiledScript, or deserialize from pre-compiled bytecode.
///
/// Priority: bytecode (pre-compiled) > source (compile on-the-fly).
/// This allows YAML users to write inline `source` code directly, while
/// pre-compiled `bytecode` remains supported for production use.
pub fn resolve_script(
    source: Option<&str>,
    bytecode: Option<&str>,
    val_limits: &ValidationLimits,
) -> Result<CompiledScript, String> {
    // 1. Pre-compiled bytecode takes priority
    if let Some(bytecode_str) = bytecode {
        let script = CompiledScript::deserialize_base64(bytecode_str)
            .map_err(|e| format!("bytecode deserialization failed: {}", e))?;

        // Validate pre-compiled bytecode to prevent malicious payloads
        let validator = Validator::new(val_limits.clone());
        let errors = validator.validate(&script);
        if !errors.is_empty() {
            let msgs: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
            return Err(format!("bytecode validation failed: {}", msgs.join("; ")));
        }

        return Ok(script);
    }

    // 2. Compile from source code
    if let Some(source) = source {
        let bytecode_b64 = compile_dsl_source(source, val_limits)
            .map_err(|errs| format!("DSL compile errors: {}", errs.join("; ")))?;
        return CompiledScript::deserialize_base64(&bytecode_b64)
            .map_err(|e| format!("compiled bytecode deserialization failed: {}", e));
    }

    Err("either source or bytecode must be provided".to_string())
}

/// Best-effort, fail-open compile-time check that every `redis.*` builtin's
/// link name appears in `valid_names` (the config's `linkSysRefs` aliases).
/// It only rejects when the bytecode layout unambiguously identifies the
/// link-name argument as a string literal; ambiguous layouts are skipped
/// (never rejected). See the inline notes for the layout guarantee and the
/// remaining false-negative case.
pub fn check_redis_link_names(script: &CompiledScript, valid_names: &HashSet<&str>) -> Result<(), String> {
    // Scan for redis.* CallBuiltin at index i. The first argument (link name)
    // lies at i-argc only when every preceding argument is a single-instruction
    // push — i.e. all argc opcodes in [i-argc, i) are zero-input single pushes.
    // A stack-consuming opcode in that window means some argument spanned
    // multiple instructions, so i-argc is unreliable and the check is skipped.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(source: &str) -> CompiledScript {
        resolve_script(Some(source), None, &ValidationLimits::default()).expect("source must compile")
    }

    #[test]
    fn resolve_script_rejects_uncompilable_source() {
        let err = resolve_script(Some("this is not valid dsl @@@"), None, &ValidationLimits::default())
            .expect_err("uncompilable source must fail");
        assert!(err.contains("DSL compile errors"), "got: {err}");
    }

    #[test]
    fn legacy_client_ip_builtin_is_rejected() {
        let err = resolve_script(Some("let ip = req.client_ip"), None, &ValidationLimits::default())
            .expect_err("the legacy client_ip builtin must not remain as an alias");
        assert!(err.contains("DSL compile errors"), "got: {err}");

        compile("let direct = req.direct_peer_ip\nlet remote = req.remote_ip");
    }

    #[test]
    fn resolve_script_requires_source_or_bytecode() {
        let err = resolve_script(None, None, &ValidationLimits::default()).expect_err("empty input must fail");
        assert_eq!(err, "either source or bytecode must be provided");
    }

    #[test]
    fn resolve_script_bytecode_takes_priority_and_roundtrips() {
        let script = compile("let a = 1");
        let b64 = script.serialize_base64().expect("serialize");
        // bytecode wins even when source is also present (and uncompilable)
        let resolved = resolve_script(Some("not valid @@@"), Some(&b64), &ValidationLimits::default())
            .expect("bytecode path must succeed");
        assert_eq!(resolved.code.len(), script.code.len());
    }

    #[test]
    fn check_rejects_unknown_redis_link() {
        let script = compile("let r = redis.get(\"mylink\", \"mykey\")");
        let err = check_redis_link_names(&script, &HashSet::new())
            .expect_err("redis call referencing an undeclared link must fail");
        assert!(err.contains("linkSysRefs"), "got: {err}");
    }

    #[test]
    fn check_accepts_declared_link_with_multi_instruction_sibling_arg() {
        // A non-literal (multi-instruction) argument after the link name makes
        // the i-argc positional guess unreliable; the link-name check must be
        // skipped (not rejected). The link "cache" is correctly declared.
        let script = compile("let r = redis.get(\"cache\", req.header(\"X-Key\"))");
        let names: HashSet<&str> = ["cache"].into_iter().collect();
        assert!(check_redis_link_names(&script, &names).is_ok());
    }

    #[test]
    fn check_fails_open_when_layout_is_ambiguous() {
        // Documented fail-open behavior: when a sibling argument is a
        // multi-instruction sub-expression, the positional guess is unreliable,
        // so the link-name check is skipped rather than risk a false rejection.
        // "typo" is undeclared, but the config still passes static validation
        // (the undeclared link surfaces at runtime via ok=false, not as a 500).
        let script = compile("let r = redis.get(\"typo\", req.header(\"X-Key\"))");
        assert!(check_redis_link_names(&script, &HashSet::new()).is_ok());
    }
}
