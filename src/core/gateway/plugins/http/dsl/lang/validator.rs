//! Bytecode validator — Controller-side static analysis
//!
//! Runs after compilation, checks CompiledScript safety.
//! Rejects scripts that exceed any limit before they reach the Gateway.

use super::bytecode::*;
use super::compiler::Compiler;
use super::error::ValidationError;
use super::parser::parse_program;

const MAX_REGEX_PATTERN_LEN: usize = 1024;

/// Validation limits for compiled scripts
#[derive(Debug, Clone)]
pub struct ValidationLimits {
    /// Max bytecode instructions (default: 1000)
    pub max_instructions: usize,
    /// Max local variable slots (default: 64)
    pub max_locals: usize,
    /// Max constant pool entries (default: 256)
    pub max_constants: usize,
    /// Max single string constant length (default: 4096)
    pub max_string_const_len: usize,
    /// Max constant pool total bytes (default: 32768)
    pub max_const_pool_bytes: usize,
    /// Max loop nesting depth (default: 8)
    pub max_loop_depth: usize,
    /// Max MakeMap operand — mirrors VmLimits::max_map_size (default: 512)
    pub max_map_size: usize,
}

impl Default for ValidationLimits {
    fn default() -> Self {
        Self {
            max_instructions: 1_000,
            max_locals: 64,
            max_constants: 256,
            max_string_const_len: 4_096,
            max_const_pool_bytes: 32_768,
            max_loop_depth: 8,
            max_map_size: 512,
        }
    }
}

/// Bytecode validator
pub struct Validator {
    limits: ValidationLimits,
}

impl Validator {
    pub fn new(limits: ValidationLimits) -> Self {
        Self { limits }
    }

    /// Validate a compiled script. Returns a list of errors (empty = valid).
    pub fn validate(&self, script: &CompiledScript) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        // 1. Instruction count
        if script.code.len() > self.limits.max_instructions {
            errors.push(ValidationError::new(format!(
                "too many instructions: {} (max {})",
                script.code.len(),
                self.limits.max_instructions
            )));
        }

        // 2. Local variable count
        if script.local_count as usize > self.limits.max_locals {
            errors.push(ValidationError::new(format!(
                "too many local variables: {} (max {})",
                script.local_count, self.limits.max_locals
            )));
        }

        // 3. Constant pool size
        if script.constants.len() > self.limits.max_constants {
            errors.push(ValidationError::new(format!(
                "too many constants: {} (max {})",
                script.constants.len(),
                self.limits.max_constants
            )));
        }

        // 4. String constant lengths and pool total bytes
        let mut total_pool_bytes = 0usize;
        for (i, constant) in script.constants.iter().enumerate() {
            match constant {
                Constant::Str(s) => {
                    if s.len() > self.limits.max_string_const_len {
                        errors.push(ValidationError::new(format!(
                            "string constant[{}] too long: {} bytes (max {})",
                            i,
                            s.len(),
                            self.limits.max_string_const_len
                        )));
                    }
                    total_pool_bytes += s.len();
                }
                Constant::Int(_) => {
                    total_pool_bytes += 8;
                }
                Constant::Regex(pattern) => {
                    total_pool_bytes += pattern.len();
                    if pattern.len() > MAX_REGEX_PATTERN_LEN {
                        errors.push(ValidationError::new(format!(
                            "regex constant[{}] too long: {} bytes (max {})",
                            i,
                            pattern.len(),
                            MAX_REGEX_PATTERN_LEN
                        )));
                    }
                    // 5. Regex syntax validation — bound DFA memory so a malicious
                    // bytecode payload cannot exhaust memory at validation time.
                    if let Err(e) = regex::RegexBuilder::new(pattern)
                        .size_limit(1 << 20)
                        .dfa_size_limit(1 << 20)
                        .build()
                    {
                        errors.push(ValidationError::new(format!(
                            "invalid regex constant[{}] '{}': {}",
                            i, pattern, e
                        )));
                    }
                }
            }
        }

        // 6. Total constant pool bytes
        if total_pool_bytes > self.limits.max_const_pool_bytes {
            errors.push(ValidationError::new(format!(
                "constant pool too large: {} bytes (max {})",
                total_pool_bytes, self.limits.max_const_pool_bytes
            )));
        }

        // 6b. LoadConst / regex-opcode index bounds check
        let const_count = script.constants.len();
        for (i, op) in script.code.iter().enumerate() {
            match op {
                OpCode::LoadConst(idx) => {
                    if *idx as usize >= const_count {
                        errors.push(ValidationError::new(format!(
                            "LoadConst at instruction {} references constant {} but pool size is {}",
                            i, idx, const_count
                        )));
                    } else if matches!(script.constants[*idx as usize], Constant::Regex(_)) {
                        errors.push(ValidationError::new(format!(
                            "LoadConst at instruction {} references constant {} (Regex) which cannot be used as a value",
                            i, idx
                        )));
                    }
                }
                OpCode::Matches(idx) => {
                    check_regex_ref(&mut errors, "Matches", i, *idx, &script.constants, const_count)
                }
                OpCode::RegexFind(idx) => {
                    check_regex_ref(&mut errors, "RegexFind", i, *idx, &script.constants, const_count)
                }
                OpCode::RegexReplace(idx) => {
                    check_regex_ref(&mut errors, "RegexReplace", i, *idx, &script.constants, const_count)
                }
                _ => {}
            }
        }

        // 6c. CallBuiltin argument count validation
        {
            use super::compiler::builtin_expected_argc;
            for (i, op) in script.code.iter().enumerate() {
                if let OpCode::CallBuiltin(builtin_id, argc) = op {
                    if let Some(expected) = builtin_expected_argc(builtin_id) {
                        if *argc as usize != expected {
                            errors.push(ValidationError::new(format!(
                                "CallBuiltin {:?} at instruction {} expects {} arg(s), bytecode says {}",
                                builtin_id, i, expected, argc
                            )));
                        }
                    }
                }
            }
        }

        // 7. Jump target validation
        let code_len = script.code.len();
        for (i, op) in script.code.iter().enumerate() {
            match op {
                OpCode::Jump(offset) | OpCode::JumpIfFalse(offset) | OpCode::LoopBack(offset) => {
                    let target = i as i64 + 1 + *offset as i64;
                    if target < 0 || target as usize > code_len {
                        errors.push(ValidationError::new(format!(
                            "jump at instruction {} targets out of bounds: {} (code length: {})",
                            i, target, code_len
                        )));
                    }
                }
                _ => {}
            }
        }

        // 8. Local variable slot validation
        for (i, op) in script.code.iter().enumerate() {
            match op {
                OpCode::GetLocal(slot) | OpCode::SetLocal(slot) => {
                    if *slot >= script.local_count {
                        errors.push(ValidationError::new(format!(
                            "instruction {} references local slot {} but only {} locals declared",
                            i, slot, script.local_count
                        )));
                    }
                }
                _ => {}
            }
        }

        // 9. LoopInit / LoopEnd balance check + observed max depth
        //
        // Re-derive the maximum loop nesting depth directly from the opcode stream rather than
        // trusting `script.max_loop_depth`. The bytecode field is operator-supplied (via the
        // `bytecode:` config knob) and an adversary can declare `max_loop_depth: 0` while shipping
        // arbitrarily-nested LoopInit/LoopEnd pairs, which the runtime would then execute.
        let mut loop_depth: i32 = 0;
        let mut max_observed_depth: i32 = 0;
        {
            for (i, op) in script.code.iter().enumerate() {
                match op {
                    OpCode::LoopInit => {
                        loop_depth += 1;
                        if loop_depth > max_observed_depth {
                            max_observed_depth = loop_depth;
                        }
                    }
                    OpCode::LoopEnd => {
                        loop_depth -= 1;
                        if loop_depth < 0 {
                            errors.push(ValidationError::new(format!(
                                "LoopEnd at instruction {} without matching LoopInit",
                                i
                            )));
                        }
                    }
                    OpCode::LoopBack(_) => {
                        if loop_depth <= 0 {
                            errors.push(ValidationError::new(format!(
                                "LoopBack at instruction {} without active LoopInit",
                                i
                            )));
                        }
                    }
                    _ => {}
                }
            }
            if loop_depth != 0 {
                errors.push(ValidationError::new(format!(
                    "unbalanced LoopInit/LoopEnd: {} LoopInit(s) without matching LoopEnd",
                    loop_depth
                )));
            }
        }

        // 10. Loop nesting depth — check the depth observed in the opcode stream, not the
        // self-declared field. Also reject scripts whose declared `max_loop_depth` understates
        // the real depth, so dishonest hand-crafted payloads fail loudly.
        let observed_depth = max_observed_depth.max(0) as usize;
        if observed_depth > self.limits.max_loop_depth {
            errors.push(ValidationError::new(format!(
                "loop nesting too deep: {} (max {})",
                observed_depth, self.limits.max_loop_depth
            )));
        }
        if (script.max_loop_depth as usize) < observed_depth {
            errors.push(ValidationError::new(format!(
                "declared max_loop_depth {} is less than observed depth {}",
                script.max_loop_depth, observed_depth
            )));
        }

        // 11. MakeMap operand check — catches oversized map literals from source and
        // rejects injected bytecode payloads (e.g. MakeMap(65535)) before they reach
        // the Gateway runtime.
        for (i, op) in script.code.iter().enumerate() {
            if let OpCode::MakeMap(n) = op {
                if (*n as usize) > self.limits.max_map_size {
                    errors.push(ValidationError::new(format!(
                        "MakeMap at instruction {} requires {} entries but limit is {}",
                        i, n, self.limits.max_map_size
                    )));
                }
            }
        }

        errors
    }
}

/// Validate that an opcode's regex constant index is in-bounds and points to a `Constant::Regex`.
/// Used for `Matches`, `RegexFind`, and `RegexReplace` arms.
fn check_regex_ref(
    errors: &mut Vec<ValidationError>,
    op_name: &str,
    instruction: usize,
    idx: u16,
    constants: &[Constant],
    const_count: usize,
) {
    if idx as usize >= const_count {
        errors.push(ValidationError::new(format!(
            "{} at instruction {} references constant {} but pool size is {}",
            op_name, instruction, idx, const_count
        )));
    } else if !matches!(constants[idx as usize], Constant::Regex(_)) {
        errors.push(ValidationError::new(format!(
            "{} at instruction {} references constant {} which is not a Regex",
            op_name, instruction, idx
        )));
    }
}

/// Full pipeline: source → parse → compile → validate → serialize
///
/// Returns Ok(bytecode_base64) or Err(error messages)
pub fn compile_dsl_source(source: &str, validation_limits: &ValidationLimits) -> Result<String, Vec<String>> {
    // Step 1: Parse
    let program = parse_program(source).map_err(|e| vec![e.to_string()])?;

    // Step 2: Compile (embed source for PC → source correlation in runtime debug logs)
    let script = Compiler::new()
        .compile_with_source(&program, source)
        .map_err(|e| vec![e.to_string()])?;

    // Step 3: Validate
    let errors = Validator::new(validation_limits.clone()).validate(&script);
    if !errors.is_empty() {
        return Err(errors.iter().map(|e| e.to_string()).collect());
    }

    // Step 4: Serialize
    let bytecode = script.serialize_base64().map_err(|e| vec![e])?;
    Ok(bytecode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_simple_script() {
        let source = r#"let x = 42"#;
        let result = compile_dsl_source(source, &ValidationLimits::default());
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_rejects_too_many_instructions() {
        let limits = ValidationLimits {
            max_instructions: 3,
            ..Default::default()
        };
        let source = r#"
            let a = 1
            let b = 2
            let c = 3
            let d = 4
        "#;
        let result = compile_dsl_source(source, &limits);
        assert!(result.is_err());
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let source = r#"
            let x = req.header("X-Test")
            if x == nil {
                return deny(403, "blocked")
            }
        "#;
        let bytecode_b64 = compile_dsl_source(source, &ValidationLimits::default()).unwrap();
        let script = CompiledScript::deserialize_base64(&bytecode_b64).unwrap();
        assert!(!script.code.is_empty());
        assert_eq!(script.local_count, 1);
    }

    #[test]
    fn test_compile_with_source_embeds_original_text() {
        let source = r#"let x = 42"#;
        let program = parse_program(source).unwrap();
        let script = Compiler::new().compile_with_source(&program, source).unwrap();
        assert_eq!(script.source.as_deref(), Some(source));
    }

    #[test]
    fn test_compile_without_source_leaves_field_none() {
        // Existing call sites using Compiler::compile (e.g. compiler/* unit tests)
        // must continue to produce CompiledScript::source = None.
        let program = parse_program("let x = 42").unwrap();
        let script = Compiler::new().compile(&program).unwrap();
        assert!(script.source.is_none());
        // Verify `#[serde(skip_serializing_if = "Option::is_none")]` keeps the
        // field out of the JSON payload when source is absent.
        let json = serde_json::to_string(&script).unwrap();
        assert!(
            !json.contains("\"source\""),
            "expected `source` to be omitted from JSON when None, got: {}",
            json
        );
    }

    #[test]
    fn test_validate_rejects_dishonest_max_loop_depth() {
        // Hand-crafted bytecode declares max_loop_depth=0 but actually nests LoopInit pairs
        // beyond the configured limit. Mimics an operator-supplied `bytecode:` payload.
        let limits = ValidationLimits {
            max_loop_depth: 2,
            ..Default::default()
        };
        let mut code = Vec::new();
        for _ in 0..5 {
            code.push(OpCode::LoopInit);
        }
        for _ in 0..5 {
            code.push(OpCode::LoopEnd);
        }
        code.push(OpCode::ReturnNext);
        let script = CompiledScript {
            version: BYTECODE_VERSION,
            code,
            constants: vec![],
            local_count: 0,
            max_loop_depth: 0, // dishonest — real depth is 5
            source: None,
            source_map: None,
        };
        let errors = Validator::new(limits).validate(&script);
        let joined = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("|");
        assert!(
            joined.contains("loop nesting too deep"),
            "expected observed-depth limit error, got: {}",
            joined
        );
        assert!(
            joined.contains("declared max_loop_depth"),
            "expected declared/observed mismatch error, got: {}",
            joined
        );
    }

    #[test]
    fn test_validate_accepts_honest_loop_depth() {
        let limits = ValidationLimits::default();
        let script = CompiledScript {
            version: BYTECODE_VERSION,
            code: vec![
                OpCode::LoopInit,
                OpCode::LoopInit,
                OpCode::LoopEnd,
                OpCode::LoopEnd,
                OpCode::ReturnNext,
            ],
            constants: vec![],
            local_count: 0,
            max_loop_depth: 2,
            source: None,
            source_map: None,
        };
        let errors = Validator::new(limits).validate(&script);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    #[test]
    fn test_validate_rejects_loadconst_regex() {
        // Hand-crafted bytecode: LoadConst(0) where constants[0] is a Regex.
        // The validator must reject this because Regex constants cannot be used as values.
        let script = CompiledScript {
            version: BYTECODE_VERSION,
            code: vec![OpCode::LoadConst(0), OpCode::ReturnNext],
            constants: vec![Constant::Regex("\\d+".to_string())],
            local_count: 0,
            max_loop_depth: 0,
            source: None,
            source_map: None,
        };
        let errors = Validator::new(ValidationLimits::default()).validate(&script);
        let joined = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("|");
        assert!(
            joined.contains("(Regex) which cannot be used as a value"),
            "expected LoadConst-Regex error, got: {}",
            joined
        );
    }

    #[test]
    fn test_validate_rejects_regex_opcode_oob_index() {
        // All three regex opcodes must reject an out-of-bounds constant index.
        for code in [
            vec![OpCode::Matches(5), OpCode::ReturnNext],
            vec![OpCode::RegexFind(5), OpCode::ReturnNext],
            vec![OpCode::RegexReplace(5), OpCode::ReturnNext],
        ] {
            let script = CompiledScript {
                version: BYTECODE_VERSION,
                code,
                constants: vec![Constant::Regex("\\d+".to_string())],
                local_count: 0,
                max_loop_depth: 0,
                source: None,
                source_map: None,
            };
            let errors = Validator::new(ValidationLimits::default()).validate(&script);
            let joined = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("|");
            assert!(
                joined.contains("but pool size is"),
                "expected out-of-bounds error, got: {}",
                joined
            );
        }
    }

    #[test]
    fn test_validate_rejects_regex_opcode_non_regex_constant() {
        // All three regex opcodes must reject a valid index that points to a non-Regex constant.
        for code in [
            vec![OpCode::Matches(0), OpCode::ReturnNext],
            vec![OpCode::RegexFind(0), OpCode::ReturnNext],
            vec![OpCode::RegexReplace(0), OpCode::ReturnNext],
        ] {
            let script = CompiledScript {
                version: BYTECODE_VERSION,
                code,
                constants: vec![Constant::Str("not-a-regex".to_string())],
                local_count: 0,
                max_loop_depth: 0,
                source: None,
                source_map: None,
            };
            let errors = Validator::new(ValidationLimits::default()).validate(&script);
            let joined = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("|");
            assert!(
                joined.contains("which is not a Regex"),
                "expected non-Regex error, got: {}",
                joined
            );
        }
    }

    #[test]
    fn test_validate_rejects_loadconst_regex_non_zero_index() {
        // Regex at index 1 in a mixed pool — verifies the type-check branch fires
        // independently of the bounds-check branch (which only triggers when idx >= pool_size).
        // Guards against an accidental idx == 0 short-circuit regression.
        let script = CompiledScript {
            version: BYTECODE_VERSION,
            code: vec![OpCode::LoadConst(1), OpCode::ReturnNext],
            constants: vec![Constant::Str("normal".to_string()), Constant::Regex("\\w+".to_string())],
            local_count: 0,
            max_loop_depth: 0,
            source: None,
            source_map: None,
        };
        let errors = Validator::new(ValidationLimits::default()).validate(&script);
        let joined = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("|");
        assert!(
            joined.contains("references constant 1 (Regex) which cannot be used as a value"),
            "expected LoadConst-Regex error at non-zero index, got: {}",
            joined
        );
    }

    #[test]
    fn test_validate_rejects_oversized_makemap() {
        // Hand-crafted bytecode: MakeMap(n) where n exceeds max_map_size.
        // The validator must catch injected oversized map operands before they reach the runtime.
        let limits = ValidationLimits {
            max_map_size: 4,
            ..Default::default()
        };
        let script = CompiledScript {
            version: BYTECODE_VERSION,
            code: vec![OpCode::MakeMap(5), OpCode::ReturnNext],
            constants: vec![],
            local_count: 0,
            max_loop_depth: 0,
            source: None,
            source_map: None,
        };
        let errors = Validator::new(limits).validate(&script);
        let joined = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("|");
        assert!(
            joined.contains("MakeMap at instruction 0 requires 5 entries but limit is 4"),
            "expected MakeMap limit error, got: {}",
            joined
        );
    }

    #[test]
    fn test_full_pipeline() {
        let source = r#"
            let ua = req.header("User-Agent")
            if ua == nil {
                return deny(403, "Missing User-Agent")
            }
            if ua.contains("bot") {
                return deny(403, "Bot blocked")
            }
            req.set_header("X-Processed", "true")
        "#;
        let bytecode = compile_dsl_source(source, &ValidationLimits::default());
        assert!(bytecode.is_ok());
    }
}
