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
#[path = "../tests/unit/validator_test.rs"]
mod tests;
