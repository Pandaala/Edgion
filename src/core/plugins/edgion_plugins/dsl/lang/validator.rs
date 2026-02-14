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
                    // 5. Regex syntax validation
                    if let Err(e) = regex::Regex::new(pattern) {
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

        // 6b. LoadConst / Matches index bounds check
        let const_count = script.constants.len();
        for (i, op) in script.code.iter().enumerate() {
            match op {
                OpCode::LoadConst(idx) => {
                    if *idx as usize >= const_count {
                        errors.push(ValidationError::new(format!(
                            "LoadConst at instruction {} references constant {} but pool size is {}",
                            i, idx, const_count
                        )));
                    }
                }
                OpCode::Matches(idx) => {
                    // Matches(idx) indexes into the constants array; the entry must be a Regex
                    if *idx as usize >= const_count {
                        errors.push(ValidationError::new(format!(
                            "Matches at instruction {} references constant {} but pool size is {}",
                            i, idx, const_count
                        )));
                    } else if !matches!(script.constants[*idx as usize], Constant::Regex(_)) {
                        errors.push(ValidationError::new(format!(
                            "Matches at instruction {} references constant {} which is not a Regex",
                            i, idx
                        )));
                    }
                }
                _ => {}
            }
        }

        // 6c. CallBuiltin argument count validation
        {
            use super::compiler::builtin_expected_argc;
            for (i, op) in script.code.iter().enumerate() {
                if let OpCode::CallBuiltin(builtin_id, argc) = op {
                    let expected = builtin_expected_argc(builtin_id);
                    if *argc as usize != expected {
                        errors.push(ValidationError::new(format!(
                            "CallBuiltin {:?} at instruction {} expects {} arg(s), bytecode says {}",
                            builtin_id, i, expected, argc
                        )));
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

        // 9. LoopInit / LoopEnd balance check
        {
            let mut loop_depth: i32 = 0;
            for (i, op) in script.code.iter().enumerate() {
                match op {
                    OpCode::LoopInit => {
                        loop_depth += 1;
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

        // 10. Loop nesting depth
        if script.max_loop_depth as usize > self.limits.max_loop_depth {
            errors.push(ValidationError::new(format!(
                "loop nesting too deep: {} (max {})",
                script.max_loop_depth, self.limits.max_loop_depth
            )));
        }

        errors
    }
}

/// Full pipeline: source → parse → compile → validate → serialize
///
/// Returns Ok(bytecode_base64) or Err(error messages)
pub fn compile_dsl_source(
    source: &str,
    validation_limits: &ValidationLimits,
) -> Result<String, Vec<String>> {
    // Step 1: Parse
    let program = parse_program(source).map_err(|e| vec![e.to_string()])?;

    // Step 2: Compile
    let script = Compiler::new()
        .compile(&program)
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
