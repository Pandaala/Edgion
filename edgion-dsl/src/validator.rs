//! Bytecode validator — Controller-side static analysis
//!
//! Runs after compilation, checks CompiledScript safety.
//! Rejects scripts that exceed any limit before they reach the Gateway.

use super::bytecode::*;
use super::compiler::{validate_streaming_regex, Compiler};
use super::error::ValidationError;
use super::json_path::DefiniteJsonPath;
use super::parser::parse_program;

mod stack;

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

        // 0. Bytecode schema version. Direct callers can construct a script
        // without passing through `deserialize_base64`, so repeat the check.
        if script.version != BYTECODE_VERSION {
            errors.push(ValidationError::new(format!(
                "unsupported bytecode version: {} (supported: {})",
                script.version, BYTECODE_VERSION
            )));
        }

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
                    total_pool_bytes = total_pool_bytes.saturating_add(s.len());
                }
                Constant::Int(_) => {
                    total_pool_bytes = total_pool_bytes.saturating_add(8);
                }
                Constant::Regex(pattern) => {
                    total_pool_bytes = total_pool_bytes.saturating_add(pattern.len());
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
                Constant::StreamingRegex(pattern) => {
                    total_pool_bytes = total_pool_bytes.saturating_add(pattern.len());
                    if pattern.len() > MAX_REGEX_PATTERN_LEN {
                        errors.push(ValidationError::new(format!(
                            "streaming regex constant[{}] too long: {} bytes (max {})",
                            i,
                            pattern.len(),
                            MAX_REGEX_PATTERN_LEN
                        )));
                    } else if let Err(error) = validate_streaming_regex(pattern) {
                        errors.push(ValidationError::new(format!(
                            "invalid streaming regex constant[{}]: {}",
                            i, error
                        )));
                    }
                }
                Constant::JsonPath(path) => {
                    total_pool_bytes = total_pool_bytes.saturating_add(path.len());
                    if let Err(error) = DefiniteJsonPath::parse(path) {
                        errors.push(ValidationError::new(format!(
                            "invalid JSON path constant[{}]: {}",
                            i, error
                        )));
                    }
                }
                Constant::JsonSelect(entries) => {
                    let max_entries = JSON_SELECT_MAX_ENTRIES.min(self.limits.max_map_size);
                    if entries.is_empty() || entries.len() > max_entries {
                        errors.push(ValidationError::new(format!(
                            "JSON select constant[{}] has {} entries (allowed 1..={})",
                            i,
                            entries.len(),
                            max_entries
                        )));
                    }
                    let mut names =
                        std::collections::HashSet::with_capacity(entries.len().min(JSON_SELECT_MAX_ENTRIES));
                    for (entry_index, entry) in entries.iter().enumerate() {
                        total_pool_bytes = total_pool_bytes
                            .saturating_add(entry.name.len())
                            .saturating_add(entry.path.len());
                        if entry.name.len() > JSON_SELECT_MAX_NAME_BYTES {
                            errors.push(ValidationError::new(format!(
                                "JSON select constant[{}] entry[{}] name is too long: {} bytes (max {})",
                                i,
                                entry_index,
                                entry.name.len(),
                                JSON_SELECT_MAX_NAME_BYTES
                            )));
                        }
                        if entry.name.len() > self.limits.max_string_const_len {
                            errors.push(ValidationError::new(format!(
                                "JSON select constant[{}] entry[{}] name exceeds string constant limit: {} bytes (max {})",
                                i,
                                entry_index,
                                entry.name.len(),
                                self.limits.max_string_const_len
                            )));
                        }
                        if entry.path.len() > self.limits.max_string_const_len {
                            errors.push(ValidationError::new(format!(
                                "JSON select constant[{}] entry[{}] path exceeds string constant limit: {} bytes (max {})",
                                i,
                                entry_index,
                                entry.path.len(),
                                self.limits.max_string_const_len
                            )));
                        }
                        if !names.insert(entry.name.as_str()) {
                            errors.push(ValidationError::new(format!(
                                "JSON select constant[{}] has duplicate output name '{}'",
                                i, entry.name
                            )));
                        }
                        if let Err(error) = DefiniteJsonPath::parse(&entry.path) {
                            errors.push(ValidationError::new(format!(
                                "invalid JSON select path constant[{}] entry[{}]: {}",
                                i, entry_index, error
                            )));
                        }
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
                    } else if !matches!(script.constants[*idx as usize], Constant::Str(_) | Constant::Int(_)) {
                        let kind = constant_kind(&script.constants[*idx as usize]);
                        errors.push(ValidationError::new(format!(
                            "LoadConst at instruction {} references constant {} ({}) which cannot be used as a value",
                            i, idx, kind
                        )));
                    }
                }
                OpCode::Matches(idx) => check_constant_ref(
                    &mut errors,
                    "Matches",
                    i,
                    *idx,
                    &script.constants,
                    const_count,
                    ConstantRefKind::StreamingRegex,
                ),
                OpCode::RegexFind(idx) => check_constant_ref(
                    &mut errors,
                    "RegexFind",
                    i,
                    *idx,
                    &script.constants,
                    const_count,
                    ConstantRefKind::Regex,
                ),
                OpCode::RegexReplace(idx) => check_constant_ref(
                    &mut errors,
                    "RegexReplace",
                    i,
                    *idx,
                    &script.constants,
                    const_count,
                    ConstantRefKind::Regex,
                ),
                OpCode::JsonGet(idx) => check_constant_ref(
                    &mut errors,
                    "JsonGet",
                    i,
                    *idx,
                    &script.constants,
                    const_count,
                    ConstantRefKind::JsonPath,
                ),
                OpCode::JsonSelect(idx) => check_constant_ref(
                    &mut errors,
                    "JsonSelect",
                    i,
                    *idx,
                    &script.constants,
                    const_count,
                    ConstantRefKind::JsonSelect,
                ),
                _ => {}
            }
        }

        // 6c. CallBuiltin argument count validation
        {
            use super::compiler::builtin_arity;
            for (i, op) in script.code.iter().enumerate() {
                if let OpCode::CallBuiltin(builtin_id, argc) = op {
                    let arity = builtin_arity(builtin_id);
                    if !arity.accepts(*argc as usize) {
                        errors.push(ValidationError::new(format!(
                            "CallBuiltin {:?} at instruction {} expects {} arg(s), bytecode says {}",
                            builtin_id, i, arity, argc
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
                OpCode::GetLocal(slot) | OpCode::SetLocal(slot) if *slot >= script.local_count => {
                    errors.push(ValidationError::new(format!(
                        "instruction {} references local slot {} but only {} locals declared",
                        i, slot, script.local_count
                    )));
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
        let mut active_loops = Vec::new();
        let mut loop_owners = Vec::with_capacity(code_len + 1);
        {
            for (i, op) in script.code.iter().enumerate() {
                // A LoopInit index uniquely identifies its lexical parent chain.
                loop_owners.push(active_loops.last().copied());
                match op {
                    OpCode::LoopInit => {
                        active_loops.push(i);
                        loop_depth += 1;
                        if loop_depth > max_observed_depth {
                            max_observed_depth = loop_depth;
                        }
                    }
                    OpCode::LoopEnd => {
                        active_loops.pop();
                        loop_depth -= 1;
                        if loop_depth < 0 {
                            errors.push(ValidationError::new(format!(
                                "LoopEnd at instruction {} without matching LoopInit",
                                i
                            )));
                        }
                    }
                    OpCode::LoopBack(_) if loop_depth <= 0 => {
                        errors.push(ValidationError::new(format!(
                            "LoopBack at instruction {} without active LoopInit",
                            i
                        )));
                    }
                    _ => {}
                }
            }
            loop_owners.push(active_loops.last().copied());
            if loop_depth != 0 {
                errors.push(ValidationError::new(format!(
                    "unbalanced LoopInit/LoopEnd: {} LoopInit(s) without matching LoopEnd",
                    loop_depth
                )));
            }
        }

        // A jump must preserve lexical loop ownership, not only nesting
        // depth. LoopEnd is still inside its loop at entry, so
        // breaks and natural exits may target its cleanup instruction.
        for (i, op) in script.code.iter().enumerate() {
            let (offset, is_loop_back) = match op {
                OpCode::Jump(offset) | OpCode::JumpIfFalse(offset) => (*offset, false),
                OpCode::LoopBack(offset) => (*offset, true),
                _ => continue,
            };
            let target = i as i64 + 1 + offset as i64;
            if target < 0 || target as usize > code_len {
                continue; // Already reported by the target-bound check.
            }
            let target = target as usize;
            if is_loop_back {
                if target >= i || loop_owners[i].is_none() || loop_owners[i] != loop_owners[target] {
                    errors.push(ValidationError::new(format!(
                        "LoopBack at instruction {} must target backward within the same innermost loop",
                        i
                    )));
                }
            } else if target <= i || loop_owners[i] != loop_owners[target] {
                errors.push(ValidationError::new(format!(
                    "jump at instruction {} must target forward within the same active loops",
                    i
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

        // Only analyze structurally valid, instruction-bounded bytecode. This
        // checks actual operand flow rather than trusting declared call counts.
        if errors.is_empty() {
            if let Err(error) = stack::validate(&script.code, &script.constants) {
                errors.push(error);
            }
        }

        errors
    }
}

#[derive(Clone, Copy)]
enum ConstantRefKind {
    Regex,
    StreamingRegex,
    JsonPath,
    JsonSelect,
}

impl ConstantRefKind {
    fn name(self) -> &'static str {
        match self {
            Self::Regex => "Regex",
            Self::StreamingRegex => "StreamingRegex",
            Self::JsonPath => "JsonPath",
            Self::JsonSelect => "JsonSelect",
        }
    }

    fn accepts(self, constant: &Constant) -> bool {
        matches!(
            (self, constant),
            (Self::Regex, Constant::Regex(_))
                | (Self::StreamingRegex, Constant::StreamingRegex(_))
                | (Self::JsonPath, Constant::JsonPath(_))
                | (Self::JsonSelect, Constant::JsonSelect(_))
        )
    }
}

fn constant_kind(constant: &Constant) -> &'static str {
    match constant {
        Constant::Str(_) => "Str",
        Constant::Int(_) => "Int",
        Constant::Regex(_) => "Regex",
        Constant::StreamingRegex(_) => "StreamingRegex",
        Constant::JsonPath(_) => "JsonPath",
        Constant::JsonSelect(_) => "JsonSelect",
    }
}

/// Validate that an opcode constant index is in bounds and has the required
/// non-value constant kind.
fn check_constant_ref(
    errors: &mut Vec<ValidationError>,
    op_name: &str,
    instruction: usize,
    idx: u16,
    constants: &[Constant],
    const_count: usize,
    expected: ConstantRefKind,
) {
    if idx as usize >= const_count {
        errors.push(ValidationError::new(format!(
            "{} at instruction {} references constant {} but pool size is {}",
            op_name, instruction, idx, const_count
        )));
    } else if !expected.accepts(&constants[idx as usize]) {
        errors.push(ValidationError::new(format!(
            "{} at instruction {} references constant {} which is not a {}",
            op_name,
            instruction,
            idx,
            expected.name()
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
        // Compiler::compile must not embed source text; source must be None.
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
    fn test_validate_enforces_ranged_builtin_arities() {
        for (builtin, argc) in [
            (BuiltinId::HttpCall, 2),
            (BuiltinId::HttpCall, 6),
            (BuiltinId::RedisSet, 2),
            (BuiltinId::RedisSet, 5),
        ] {
            let script = CompiledScript {
                version: BYTECODE_VERSION,
                code: vec![OpCode::CallBuiltin(builtin, argc), OpCode::ReturnNext],
                constants: vec![],
                local_count: 0,
                max_loop_depth: 0,
                source: None,
                source_map: None,
            };
            let joined = Validator::new(ValidationLimits::default())
                .validate(&script)
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("|");
            assert!(
                joined.contains("expects") && joined.contains(&format!("bytecode says {argc}")),
                "expected argument-count error for {builtin:?}/{argc}, got: {joined}"
            );
        }

        for (builtin, argc) in [
            (BuiltinId::HttpCall, 3),
            (BuiltinId::HttpCall, 5),
            (BuiltinId::RedisSet, 3),
            (BuiltinId::RedisSet, 4),
        ] {
            let mut code = vec![OpCode::LoadNil; usize::from(argc)];
            code.extend([OpCode::CallBuiltin(builtin, argc), OpCode::Pop, OpCode::ReturnNext]);
            let script = CompiledScript {
                version: BYTECODE_VERSION,
                code,
                constants: vec![],
                local_count: 0,
                max_loop_depth: 0,
                source: None,
                source_map: None,
            };
            let errors = Validator::new(ValidationLimits::default()).validate(&script);
            assert!(
                errors.is_empty(),
                "valid boundary rejected for {builtin:?}/{argc}: {errors:?}"
            );
        }
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
                joined.contains("which is not a"),
                "expected wrong constant-kind error, got: {}",
                joined
            );
        }
    }

    #[test]
    fn test_validate_requires_streaming_regex_for_matches() {
        let script = CompiledScript {
            version: BYTECODE_VERSION,
            code: vec![OpCode::Matches(0), OpCode::ReturnNext],
            constants: vec![Constant::Regex("word".to_string())],
            local_count: 0,
            max_loop_depth: 0,
            source: None,
            source_map: None,
        };
        let joined = Validator::new(ValidationLimits::default())
            .validate(&script)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("|");
        assert!(joined.contains("which is not a StreamingRegex"));
    }

    #[test]
    fn test_validate_rejects_unicode_word_boundary_in_injected_streaming_regex() {
        let script = CompiledScript {
            version: BYTECODE_VERSION,
            code: vec![OpCode::Matches(0), OpCode::ReturnNext],
            constants: vec![Constant::StreamingRegex(r"\bword\b".to_string())],
            local_count: 0,
            max_loop_depth: 0,
            source: None,
            source_map: None,
        };
        let joined = Validator::new(ValidationLimits::default())
            .validate(&script)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("|");
        assert!(joined.contains("invalid streaming regex constant"));
    }

    #[test]
    fn test_validate_rejects_invalid_or_mistyped_json_path_constant() {
        for constants in [
            vec![Constant::JsonPath("$.*".to_string())],
            vec![Constant::Str("$.ok".to_string())],
        ] {
            let script = CompiledScript {
                version: BYTECODE_VERSION,
                code: vec![OpCode::JsonGet(0), OpCode::ReturnNext],
                constants,
                local_count: 0,
                max_loop_depth: 0,
                source: None,
                source_map: None,
            };
            let errors = Validator::new(ValidationLimits::default()).validate(&script);
            assert!(!errors.is_empty());
        }
    }

    #[test]
    fn test_validate_rejects_old_bytecode_version_for_direct_callers() {
        let script = CompiledScript {
            version: 1,
            code: vec![OpCode::ReturnNext],
            constants: vec![],
            local_count: 0,
            max_loop_depth: 0,
            source: None,
            source_map: None,
        };
        let joined = Validator::new(ValidationLimits::default())
            .validate(&script)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("|");
        assert!(joined.contains("unsupported bytecode version"));
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

    fn json_select_script(entries: Vec<JsonSelectEntry>) -> CompiledScript {
        CompiledScript {
            version: BYTECODE_VERSION,
            code: vec![
                OpCode::CallBuiltin(BuiltinId::ReqBody, 0),
                OpCode::JsonSelect(0),
                OpCode::Pop,
                OpCode::ReturnNext,
            ],
            constants: vec![Constant::JsonSelect(entries)],
            local_count: 0,
            max_loop_depth: 0,
            source: None,
            source_map: None,
        }
    }

    fn json_select_entry(name: impl Into<String>, path: impl Into<String>) -> JsonSelectEntry {
        JsonSelectEntry {
            name: name.into(),
            path: path.into(),
        }
    }

    #[test]
    fn test_validate_json_select_accepts_duplicate_paths() {
        let script = json_select_script(vec![
            json_select_entry("first", "$.user.id"),
            json_select_entry("second", "$.user.id"),
        ]);
        let errors = Validator::new(ValidationLimits::default()).validate(&script);
        assert!(errors.is_empty(), "duplicate paths must be valid: {errors:?}");
    }

    #[test]
    fn test_validate_rejects_json_select_opcode_reference_errors() {
        for script in [
            CompiledScript {
                version: BYTECODE_VERSION,
                code: vec![OpCode::JsonSelect(1), OpCode::ReturnNext],
                constants: vec![Constant::JsonSelect(vec![json_select_entry("id", "$.id")])],
                local_count: 0,
                max_loop_depth: 0,
                source: None,
                source_map: None,
            },
            CompiledScript {
                version: BYTECODE_VERSION,
                code: vec![OpCode::JsonSelect(0), OpCode::ReturnNext],
                constants: vec![Constant::JsonPath("$.id".to_string())],
                local_count: 0,
                max_loop_depth: 0,
                source: None,
                source_map: None,
            },
        ] {
            let errors = Validator::new(ValidationLimits::default()).validate(&script);
            assert!(!errors.is_empty(), "hostile opcode reference must fail");
        }
    }

    #[test]
    fn test_validate_rejects_json_select_entry_count_limits() {
        let empty = json_select_script(vec![]);
        let errors = Validator::new(ValidationLimits::default()).validate(&empty);
        assert!(errors.iter().any(|error| error.to_string().contains("allowed 1..=64")));

        let too_many = json_select_script(
            (0..65)
                .map(|index| json_select_entry(format!("key{index}"), "$.value"))
                .collect(),
        );
        let errors = Validator::new(ValidationLimits::default()).validate(&too_many);
        assert!(errors.iter().any(|error| error.to_string().contains("has 65 entries")));

        let smaller_map_limit = json_select_script(vec![
            json_select_entry("first", "$.a"),
            json_select_entry("second", "$.b"),
        ]);
        let errors = Validator::new(ValidationLimits {
            max_map_size: 1,
            ..Default::default()
        })
        .validate(&smaller_map_limit);
        assert!(errors.iter().any(|error| error.to_string().contains("allowed 1..=1")));
    }

    #[test]
    fn test_validate_rejects_json_select_name_path_and_pool_violations() {
        let invalid = json_select_script(vec![
            json_select_entry("duplicate", "$.ok"),
            json_select_entry("duplicate", "$.*"),
            json_select_entry("n".repeat(JSON_SELECT_MAX_NAME_BYTES + 1), "$.ok"),
        ]);
        let joined = Validator::new(ValidationLimits::default())
            .validate(&invalid)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("|");
        assert!(joined.contains("duplicate output name 'duplicate'"), "got: {joined}");
        assert!(joined.contains("invalid JSON select path"), "got: {joined}");
        assert!(joined.contains("name is too long: 257 bytes"), "got: {joined}");

        let script = json_select_script(vec![json_select_entry("name", "$.path")]);
        let joined = Validator::new(ValidationLimits {
            max_string_const_len: 4,
            max_const_pool_bytes: 5,
            ..Default::default()
        })
        .validate(&script)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("|");
        assert!(joined.contains("path exceeds string constant limit"), "got: {joined}");
        assert!(joined.contains("constant pool too large"), "got: {joined}");
    }

    #[test]
    fn test_validate_rejects_loading_json_select_as_value() {
        let script = CompiledScript {
            code: vec![OpCode::LoadConst(0), OpCode::ReturnNext],
            ..json_select_script(vec![json_select_entry("id", "$.id")])
        };
        let joined = Validator::new(ValidationLimits::default())
            .validate(&script)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("|");
        assert!(
            joined.contains("(JsonSelect) which cannot be used as a value"),
            "got: {joined}"
        );
    }

    #[test]
    fn test_full_pipeline_enforces_json_select_max_map_size() {
        let limits = ValidationLimits {
            max_map_size: 1,
            ..Default::default()
        };
        let result = compile_dsl_source(
            r#"let selected = req.body.json_select({"first": "$.a", "second": "$.b"})"#,
            &limits,
        );
        let joined = result.expect_err("configured maxMapSize must reject plan").join("|");
        assert!(joined.contains("allowed 1..=1"), "got: {joined}");
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
