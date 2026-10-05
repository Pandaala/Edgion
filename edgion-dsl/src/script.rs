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
/// preceding a `CallBuiltin` are all of this kind and control flow cannot enter
/// the middle of that window, they map 1:1 to the argc arguments. Any other
/// opcode means the positional guess is unreliable. This is an explicit
/// allowlist: unknown / future opcodes default to "not a single push".
fn is_single_push_op(op: &OpCode) -> bool {
    matches!(
        op,
        OpCode::LoadConst(_) | OpCode::LoadNil | OpCode::LoadTrue | OpCode::LoadFalse | OpCode::GetLocal(_)
    )
}

/// A jump into the middle of the push window can skip its apparent first
/// argument. Invalid targets are also ambiguous for direct callers; configured
/// bytecode is rejected separately by `Validator` before this check runs.
fn has_jump_into_window(code: &[OpCode], first_arg_pos: usize, call_pos: usize) -> bool {
    let (Ok(start), Ok(call), Ok(code_len)) = (
        i64::try_from(first_arg_pos),
        i64::try_from(call_pos),
        i64::try_from(code.len()),
    ) else {
        return true;
    };
    code.iter().enumerate().any(|(source, op)| {
        let offset = match op {
            OpCode::Jump(offset) | OpCode::JumpIfFalse(offset) | OpCode::LoopBack(offset) => *offset,
            _ => return false,
        };
        let Ok(source) = i64::try_from(source) else {
            return true;
        };
        let Some(target) = source
            .checked_add(1)
            .and_then(|next| next.checked_add(i64::from(offset)))
        else {
            return true;
        };
        target < 0 || target > code_len || (target > start && target <= call)
    })
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
    // lies at i-argc only when all argc opcodes in [i-argc, i) are zero-input
    // single pushes and no jump can enter after the first push. Otherwise the
    // positional guess is unreliable and the check is skipped.
    for (i, op) in script.code.iter().enumerate() {
        if let OpCode::CallBuiltin(id, argc) = op {
            if id.is_redis_builtin() && *argc >= 1 {
                let first_arg_pos = i.wrapping_sub(*argc as usize);
                if first_arg_pos >= i {
                    continue;
                }
                // Only enforce when the positional guess is provably correct.
                if !script.code[first_arg_pos..i].iter().all(is_single_push_op)
                    || has_jump_into_window(&script.code, first_arg_pos, i)
                {
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
#[path = "../tests/unit/script_test.rs"]
mod tests;
