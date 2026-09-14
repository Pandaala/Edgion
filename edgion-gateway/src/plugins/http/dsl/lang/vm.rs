//! EdgionDSL stack-based VM interpreter with safety sandbox
//!
//! Security model:
//!   - Step budget: every instruction costs 1 step
//!   - Call budget: every CallBuiltin costs 1 call
//!   - Loop budget: per-loop iteration counter
//!   - Stack depth limit
//!   - String length limit
//!   - catch_unwind for fault isolation
//!
//! Note on the step budget: `max_steps` counts opcode dispatches, not CPU time.
//! Regex opcodes (`Matches`, `RegexFind`, `RegexReplace`) and `CallBuiltin`
//! may each perform O(input_len) work per single step — regex evaluation runs
//! over strings up to `max_string_len` bytes, and builtins such as `json_get`,
//! `split`, `url_encode`, and `base64` iterate their input under one call /
//! step deduction. The realistic CPU upper bound is therefore approximately
//! `max_steps * max_string_len` for regex-heavy scripts and
//! `max_call_count * max_string_len` for builtin-heavy scripts, additionally
//! bounded by each regex's `dfa_size_limit` (1 MiB) and per-builtin output
//! caps. See `skills/01-architecture/02-gateway/dsl-engine-security-model.md`
//! for the canonical breakdown of every cap and where it is enforced.

use std::sync::Arc;

use bytes::Bytes;
use regex_automata::dfa::dense;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::plugins::http::common::{replace_all_bounded, BoundedRegexReplacementError};
use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession};
use crate::request_body::{BodyTextOperation, LossyTextScanner, TextBodyScanner};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::plugin_configs::dsl::{
    DSL_DEFAULT_MAX_CALL_COUNT, DSL_DEFAULT_MAX_LIST_LEN, DSL_DEFAULT_MAX_LOOP_ITERATIONS, DSL_DEFAULT_MAX_MAP_SIZE,
    DSL_DEFAULT_MAX_STACK_DEPTH, DSL_DEFAULT_MAX_STEPS, DSL_DEFAULT_MAX_STRING_LEN,
};

use super::super::runtime::DslRuntime;
use super::body_ops::{body_access_error, expect_body, map_body_error, DslBodyOps};
use super::body_scan_budget::BodyScanBudget;
use super::body_scanners::{JsonPathScanner, JsonScanValue, JsonSelectPlan, JsonSelectScanner, StreamingRegexScanner};
use super::bytecode::*;
use super::error::RuntimeError;
use super::value::Value;
use edgion_dsl::json_path::DefiniteJsonPath;

pub(crate) trait RuntimeString {
    fn runtime_string(self) -> Result<String, RuntimeError>;
}

impl RuntimeString for Value {
    fn runtime_string(self) -> Result<String, RuntimeError> {
        self.try_into_string().map_err(|error| RuntimeError::TypeError {
            expected: "a value implicitly convertible to Str",
            got: error.got(),
            operation: "implicit string conversion",
        })
    }
}

fn expect_string(value: Value, operation: &'static str) -> Result<String, RuntimeError> {
    match value {
        Value::Str(value) => Ok(value.to_string()),
        other => Err(RuntimeError::TypeError {
            expected: "Str",
            got: other.type_name(),
            operation,
        }),
    }
}

fn nonnegative_u64(value: Value, operation: &'static str) -> Result<u64, RuntimeError> {
    match value {
        Value::Int(value) if value >= 0 => Ok(value as u64),
        Value::Int(_) => Err(RuntimeError::TypeError {
            expected: "non-negative Int",
            got: "Int",
            operation,
        }),
        other => Err(RuntimeError::TypeError {
            expected: "non-negative Int",
            got: other.type_name(),
            operation,
        }),
    }
}

// ==================== VM Limits ====================

/// Safety limits for VM execution — unified fuel/budget model.
///
/// Every resource-consuming operation deducts from a budget counter.
/// When any budget hits 0, execution terminates immediately.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VmLimits {
    /// Max bytecode instructions per execution (default: 10,000)
    #[serde(default = "default_max_steps")]
    pub max_steps: u32,

    /// Max iterations per individual loop (default: 100)
    #[serde(default = "default_max_loop_iterations")]
    pub max_loop_iterations: u32,

    /// Max builtin function calls per execution (default: 500)
    #[serde(default = "default_max_call_count")]
    pub max_call_count: u32,

    /// Max value stack depth (default: 128)
    #[serde(default = "default_max_stack_depth")]
    pub max_stack_depth: usize,

    /// Max string length from concatenation (default: 8192)
    #[serde(default = "default_max_string_len")]
    pub max_string_len: usize,

    /// Max number of elements in a List value (default: 1024)
    #[serde(default = "default_max_list_len")]
    pub max_list_len: usize,

    /// Max number of entries in a Map value (default: 512)
    #[serde(default = "default_max_map_size")]
    pub max_map_size: usize,
}

fn default_max_steps() -> u32 {
    DSL_DEFAULT_MAX_STEPS
}
fn default_max_loop_iterations() -> u32 {
    DSL_DEFAULT_MAX_LOOP_ITERATIONS
}
fn default_max_call_count() -> u32 {
    DSL_DEFAULT_MAX_CALL_COUNT
}
fn default_max_stack_depth() -> usize {
    DSL_DEFAULT_MAX_STACK_DEPTH
}
fn default_max_string_len() -> usize {
    DSL_DEFAULT_MAX_STRING_LEN
}
fn default_max_list_len() -> usize {
    DSL_DEFAULT_MAX_LIST_LEN
}
fn default_max_map_size() -> usize {
    DSL_DEFAULT_MAX_MAP_SIZE
}

impl Default for VmLimits {
    fn default() -> Self {
        Self {
            max_steps: DSL_DEFAULT_MAX_STEPS,
            max_loop_iterations: DSL_DEFAULT_MAX_LOOP_ITERATIONS,
            max_call_count: DSL_DEFAULT_MAX_CALL_COUNT,
            max_stack_depth: DSL_DEFAULT_MAX_STACK_DEPTH,
            max_string_len: DSL_DEFAULT_MAX_STRING_LEN,
            max_list_len: DSL_DEFAULT_MAX_LIST_LEN,
            max_map_size: DSL_DEFAULT_MAX_MAP_SIZE,
        }
    }
}

// ==================== Error Policy ====================

// Re-exported from `edgion_resources` so callers can use `vm::DslErrorPolicy`;
// `apply_error_policy` below maps its variants onto `PluginRunningResult`.
pub use edgion_resources::resources::edgion_plugins::plugin_configs::dsl::DslErrorPolicy;

// ==================== VM State ====================

/// Internal execution state (per-request, not reusable)
pub(crate) struct VmState {
    pc: usize,
    stack: SmallVec<[Value; 16]>,
    locals: SmallVec<[Value; 16]>,
    loop_counters: SmallVec<[u32; 4]>,

    // Budget counters
    step_budget: u32,
    call_budget: u32,

    // Original limits (for error messages)
    max_steps: u32,
    max_call_count: u32,
    pub(crate) body_scan_budget: BodyScanBudget,
}

impl VmState {
    fn new(local_count: u16, limits: &VmLimits) -> Self {
        let mut locals = SmallVec::with_capacity(local_count as usize);
        for _ in 0..local_count {
            locals.push(Value::Nil);
        }
        Self {
            pc: 0,
            stack: SmallVec::new(),
            locals,
            loop_counters: SmallVec::new(),
            step_budget: limits.max_steps,
            call_budget: limits.max_call_count,
            max_steps: limits.max_steps,
            max_call_count: limits.max_call_count,
            body_scan_budget: BodyScanBudget::default(),
        }
    }

    pub(crate) fn push(&mut self, value: Value, max_depth: usize) -> Result<(), RuntimeError> {
        if self.stack.len() >= max_depth {
            return Err(RuntimeError::StackOverflow { limit: max_depth });
        }
        self.stack.push(value);
        Ok(())
    }

    pub(crate) fn pop(&mut self) -> Result<Value, RuntimeError> {
        self.stack.pop().ok_or_else(|| RuntimeError::Internal {
            message: "stack underflow".to_string(),
        })
    }

    fn tick_step(&mut self) -> Result<(), RuntimeError> {
        if self.step_budget == 0 {
            return Err(RuntimeError::StepLimitExceeded { limit: self.max_steps });
        }
        self.step_budget -= 1;
        Ok(())
    }

    fn tick_call(&mut self) -> Result<(), RuntimeError> {
        if self.call_budget == 0 {
            return Err(RuntimeError::CallLimitExceeded {
                limit: self.max_call_count,
            });
        }
        self.call_budget -= 1;
        Ok(())
    }
}

// ==================== VM ====================

/// VM instance — created per-script, reused across requests
pub struct Vm {
    script: CompiledScript,
    // Parallel to script.constants: Some for pre-compiled Regex entries, None for others.
    regex_cache: Vec<Option<regex::Regex>>,
    streaming_regex_cache: Vec<Option<Arc<dense::DFA<Vec<u32>>>>>,
    json_path_cache: Vec<Option<DefiniteJsonPath>>,
    json_select_cache: Vec<Option<Arc<JsonSelectPlan>>>,
    // Parallel to script.constants: Some for Str entries, None for Int/Regex.
    const_str_cache: Vec<Option<Arc<str>>>,
    pub(crate) limits: VmLimits,
}

impl Vm {
    /// Create a new VM with pre-compiled regex cache
    pub fn new(script: CompiledScript, limits: VmLimits) -> Result<Self, RuntimeError> {
        let mut regex_cache = Vec::with_capacity(script.constants.len());
        let mut streaming_regex_cache = Vec::with_capacity(script.constants.len());
        let mut json_path_cache = Vec::with_capacity(script.constants.len());
        let mut json_select_cache = Vec::with_capacity(script.constants.len());
        let mut const_str_cache = Vec::with_capacity(script.constants.len());
        for constant in &script.constants {
            match constant {
                Constant::Regex(pattern) => {
                    // Cap DFA memory per pattern to bound worst-case allocation when
                    // loading bytecode with up to max_constants regex entries.
                    let re = regex::RegexBuilder::new(pattern)
                        .size_limit(1 << 20)
                        .dfa_size_limit(1 << 20)
                        .build()
                        .map_err(|e| RuntimeError::RegexError {
                            message: format!("failed to compile regex '{}': {}", pattern, e),
                        })?;
                    regex_cache.push(Some(re));
                    streaming_regex_cache.push(None);
                    json_path_cache.push(None);
                    json_select_cache.push(None);
                    const_str_cache.push(None);
                }
                Constant::StreamingRegex(pattern) => {
                    let mut builder = dense::Builder::new();
                    builder.configure(dense::Config::new().dfa_size_limit(Some(1 << 20)));
                    let dfa = builder.build(pattern).map_err(|error| RuntimeError::RegexError {
                        message: format!("failed to compile streaming regex '{}': {}", pattern, error),
                    })?;
                    regex_cache.push(None);
                    streaming_regex_cache.push(Some(Arc::new(dfa)));
                    json_path_cache.push(None);
                    json_select_cache.push(None);
                    const_str_cache.push(None);
                }
                Constant::JsonPath(path) => {
                    let path = DefiniteJsonPath::parse(path).map_err(|error| RuntimeError::Internal {
                        message: format!("invalid validated JSON path: {error}"),
                    })?;
                    regex_cache.push(None);
                    streaming_regex_cache.push(None);
                    json_path_cache.push(Some(path));
                    json_select_cache.push(None);
                    const_str_cache.push(None);
                }
                Constant::JsonSelect(entries) => {
                    let entry_limit = JSON_SELECT_MAX_ENTRIES.min(limits.max_map_size);
                    if entries.is_empty() {
                        return Err(RuntimeError::Internal {
                            message: "validated JSON select plan is empty".to_string(),
                        });
                    }
                    if entries.len() > entry_limit {
                        return Err(RuntimeError::MapTooLarge {
                            size: entries.len(),
                            limit: entry_limit,
                        });
                    }
                    let mut names = std::collections::HashSet::with_capacity(entries.len());
                    let parsed = entries
                        .iter()
                        .map(|entry| {
                            if entry.name.len() > JSON_SELECT_MAX_NAME_BYTES {
                                return Err(RuntimeError::Internal {
                                    message: "validated JSON select output name exceeds limit".to_string(),
                                });
                            }
                            if !names.insert(entry.name.as_str()) {
                                return Err(RuntimeError::Internal {
                                    message: "validated JSON select output name is duplicated".to_string(),
                                });
                            }
                            let path =
                                DefiniteJsonPath::parse(&entry.path).map_err(|error| RuntimeError::Internal {
                                    message: format!("invalid validated JSON select path: {error}"),
                                })?;
                            Ok((entry.name.clone(), path))
                        })
                        .collect::<Result<Vec<_>, RuntimeError>>()?;
                    regex_cache.push(None);
                    streaming_regex_cache.push(None);
                    json_path_cache.push(None);
                    json_select_cache.push(Some(Arc::new(JsonSelectPlan::new(parsed))));
                    const_str_cache.push(None);
                }
                Constant::Str(s) => {
                    regex_cache.push(None);
                    streaming_regex_cache.push(None);
                    json_path_cache.push(None);
                    json_select_cache.push(None);
                    const_str_cache.push(Some(Arc::from(s.as_str())));
                }
                Constant::Int(_) => {
                    regex_cache.push(None);
                    streaming_regex_cache.push(None);
                    json_path_cache.push(None);
                    json_select_cache.push(None);
                    const_str_cache.push(None);
                }
            }
        }
        Ok(Self {
            script,
            regex_cache,
            streaming_regex_cache,
            json_path_cache,
            json_select_cache,
            const_str_cache,
            limits,
        })
    }

    /// Execute script with a PluginSession. Returns Ok(result) or Err(runtime error).
    pub async fn execute(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        runtime: &DslRuntime,
    ) -> Result<PluginRunningResult, RuntimeError> {
        let mut state = VmState::new(self.script.local_count, &self.limits);

        loop {
            state.tick_step()?;

            if state.pc >= self.script.code.len() {
                return Ok(PluginRunningResult::GoodNext);
            }

            let opcode = self.script.code[state.pc];
            state.pc += 1;

            match opcode {
                // ===== Stack Operations =====
                OpCode::LoadConst(idx) => {
                    let constant = self
                        .script
                        .constants
                        .get(idx as usize)
                        .ok_or_else(|| RuntimeError::Internal {
                            message: format!(
                                "constant index {} out of bounds (pool size: {})",
                                idx,
                                self.script.constants.len()
                            ),
                        })?;
                    let value = match constant {
                        Constant::Str(_) => Value::Str(Arc::clone(
                            self.const_str_cache[idx as usize]
                                .as_ref()
                                .expect("Vm::new fills const_str_cache for every Constant::Str index; outer bounds guard at ok_or_else validated idx"),
                        )),
                        Constant::Int(n) => Value::Int(*n),
                        Constant::Regex(_) => {
                            return Err(RuntimeError::Internal {
                                message: format!(
                                    "LoadConst at pc {} references constant {} (Regex) which cannot be used as a value",
                                    state.pc - 1,
                                    idx
                                ),
                            });
                        }
                        Constant::StreamingRegex(_) | Constant::JsonPath(_) | Constant::JsonSelect(_) => {
                            return Err(RuntimeError::Internal {
                                message: format!(
                                    "LoadConst at pc {} references non-value constant {}",
                                    state.pc - 1,
                                    idx
                                ),
                            });
                        }
                    };
                    state.push(value, self.limits.max_stack_depth)?;
                }
                OpCode::LoadNil => state.push(Value::Nil, self.limits.max_stack_depth)?,
                OpCode::LoadTrue => state.push(Value::Bool(true), self.limits.max_stack_depth)?,
                OpCode::LoadFalse => state.push(Value::Bool(false), self.limits.max_stack_depth)?,
                OpCode::Pop => {
                    state.pop()?;
                }

                // ===== Local Variables =====
                OpCode::GetLocal(slot) => {
                    let val = state.locals.get(slot as usize).cloned().unwrap_or(Value::Nil);
                    state.push(val, self.limits.max_stack_depth)?;
                }
                OpCode::SetLocal(slot) => {
                    let val = state.pop()?;
                    if (slot as usize) < state.locals.len() {
                        state.locals[slot as usize] = val;
                    } else {
                        return Err(RuntimeError::Internal {
                            message: format!(
                                "SetLocal slot {} out of bounds (locals size: {})",
                                slot,
                                state.locals.len()
                            ),
                        });
                    }
                }

                // ===== Arithmetic =====
                OpCode::Add => {
                    let b = state.pop()?;
                    let a = state.pop()?;
                    let result = match (&a, &b) {
                        (Value::Int(a), Value::Int(b)) => Value::Int(
                            a.checked_add(*b)
                                .ok_or(RuntimeError::IntegerOverflow { operation: "+" })?,
                        ),
                        (Value::Str(a), Value::Str(b)) => {
                            let new_len = a.len() + b.len();
                            if new_len > self.limits.max_string_len {
                                return Err(RuntimeError::StringTooLong {
                                    len: new_len,
                                    limit: self.limits.max_string_len,
                                });
                            }
                            let mut result = String::with_capacity(new_len);
                            result.push_str(a);
                            result.push_str(b);
                            Value::Str(result.into())
                        }
                        (Value::Str(a), _) => {
                            let b_str = b.runtime_string()?;
                            let new_len = a.len() + b_str.len();
                            if new_len > self.limits.max_string_len {
                                return Err(RuntimeError::StringTooLong {
                                    len: new_len,
                                    limit: self.limits.max_string_len,
                                });
                            }
                            let mut result = String::with_capacity(new_len);
                            result.push_str(a);
                            result.push_str(&b_str);
                            Value::Str(result.into())
                        }
                        (_, Value::Str(b)) => {
                            let a_str = a.runtime_string()?;
                            let new_len = a_str.len() + b.len();
                            if new_len > self.limits.max_string_len {
                                return Err(RuntimeError::StringTooLong {
                                    len: new_len,
                                    limit: self.limits.max_string_len,
                                });
                            }
                            let mut result = String::with_capacity(new_len);
                            result.push_str(&a_str);
                            result.push_str(b);
                            Value::Str(result.into())
                        }
                        _ => {
                            return Err(RuntimeError::TypeError {
                                expected: "Int or Str",
                                got: a.type_name(),
                                operation: "+",
                            });
                        }
                    };
                    state.push(result, self.limits.max_stack_depth)?;
                }
                OpCode::Sub => {
                    let b = state.pop()?;
                    let a = state.pop()?;
                    match (&a, &b) {
                        (Value::Int(a), Value::Int(b)) => {
                            let n = a
                                .checked_sub(*b)
                                .ok_or(RuntimeError::IntegerOverflow { operation: "-" })?;
                            state.push(Value::Int(n), self.limits.max_stack_depth)?;
                        }
                        _ => {
                            return Err(RuntimeError::TypeError {
                                expected: "Int",
                                got: a.type_name(),
                                operation: "-",
                            });
                        }
                    }
                }
                OpCode::Mul => {
                    let b = state.pop()?;
                    let a = state.pop()?;
                    match (&a, &b) {
                        (Value::Int(a), Value::Int(b)) => {
                            let n = a
                                .checked_mul(*b)
                                .ok_or(RuntimeError::IntegerOverflow { operation: "*" })?;
                            state.push(Value::Int(n), self.limits.max_stack_depth)?;
                        }
                        _ => {
                            return Err(RuntimeError::TypeError {
                                expected: "Int",
                                got: a.type_name(),
                                operation: "*",
                            });
                        }
                    }
                }
                OpCode::Div => {
                    let b = state.pop()?;
                    let a = state.pop()?;
                    match (&a, &b) {
                        (Value::Int(_), Value::Int(0)) => {
                            return Err(RuntimeError::DivisionByZero);
                        }
                        (Value::Int(a), Value::Int(b)) => {
                            // Protect against i64::MIN / -1 overflow
                            let result = a
                                .checked_div(*b)
                                .ok_or(RuntimeError::IntegerOverflow { operation: "/" })?;
                            state.push(Value::Int(result), self.limits.max_stack_depth)?;
                        }
                        _ => {
                            return Err(RuntimeError::TypeError {
                                expected: "Int",
                                got: a.type_name(),
                                operation: "/",
                            });
                        }
                    }
                }
                OpCode::Neg => {
                    let v = state.pop()?;
                    match v {
                        Value::Int(n) => {
                            let neg = n
                                .checked_neg()
                                .ok_or(RuntimeError::IntegerOverflow { operation: "unary -" })?;
                            state.push(Value::Int(neg), self.limits.max_stack_depth)?;
                        }
                        _ => {
                            return Err(RuntimeError::TypeError {
                                expected: "Int",
                                got: v.type_name(),
                                operation: "unary -",
                            });
                        }
                    }
                }

                // ===== Comparison =====
                OpCode::Equal => {
                    let b = state.pop()?;
                    let a = state.pop()?;
                    let equal = match (a, b) {
                        (Value::Body(_), Value::Str(pattern)) | (Value::Str(pattern), Value::Body(_)) => {
                            DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime)
                                .text_predicate(BodyTextOperation::Equals(pattern.to_string()))
                                .await?
                        }
                        (a, b) => a == b,
                    };
                    state.push(Value::Bool(equal), self.limits.max_stack_depth)?;
                }
                OpCode::NotEqual => {
                    let b = state.pop()?;
                    let a = state.pop()?;
                    let equal = match (a, b) {
                        (Value::Body(_), Value::Str(pattern)) | (Value::Str(pattern), Value::Body(_)) => {
                            DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime)
                                .text_predicate(BodyTextOperation::Equals(pattern.to_string()))
                                .await?
                        }
                        (a, b) => a == b,
                    };
                    state.push(Value::Bool(!equal), self.limits.max_stack_depth)?;
                }
                OpCode::Less => {
                    let b = state.pop()?;
                    let a = state.pop()?;
                    let result = match (&a, &b) {
                        (Value::Int(a), Value::Int(b)) => a < b,
                        (Value::Str(a), Value::Str(b)) => a < b,
                        _ => {
                            return Err(RuntimeError::TypeError {
                                expected: "matching comparable types (Int/Int or Str/Str)",
                                got: a.type_name(),
                                operation: "<",
                            });
                        }
                    };
                    state.push(Value::Bool(result), self.limits.max_stack_depth)?;
                }
                OpCode::Greater => {
                    let b = state.pop()?;
                    let a = state.pop()?;
                    let result = match (&a, &b) {
                        (Value::Int(a), Value::Int(b)) => a > b,
                        (Value::Str(a), Value::Str(b)) => a > b,
                        _ => {
                            return Err(RuntimeError::TypeError {
                                expected: "matching comparable types (Int/Int or Str/Str)",
                                got: a.type_name(),
                                operation: ">",
                            });
                        }
                    };
                    state.push(Value::Bool(result), self.limits.max_stack_depth)?;
                }
                OpCode::LessEqual => {
                    let b = state.pop()?;
                    let a = state.pop()?;
                    let result = match (&a, &b) {
                        (Value::Int(a), Value::Int(b)) => a <= b,
                        (Value::Str(a), Value::Str(b)) => a <= b,
                        _ => {
                            return Err(RuntimeError::TypeError {
                                expected: "matching comparable types (Int/Int or Str/Str)",
                                got: a.type_name(),
                                operation: "<=",
                            });
                        }
                    };
                    state.push(Value::Bool(result), self.limits.max_stack_depth)?;
                }
                OpCode::GreaterEqual => {
                    let b = state.pop()?;
                    let a = state.pop()?;
                    let result = match (&a, &b) {
                        (Value::Int(a), Value::Int(b)) => a >= b,
                        (Value::Str(a), Value::Str(b)) => a >= b,
                        _ => {
                            return Err(RuntimeError::TypeError {
                                expected: "matching comparable types (Int/Int or Str/Str)",
                                got: a.type_name(),
                                operation: ">=",
                            });
                        }
                    };
                    state.push(Value::Bool(result), self.limits.max_stack_depth)?;
                }

                // ===== Logical =====
                OpCode::Not => {
                    let v = state.pop()?;
                    state.push(Value::Bool(!v.is_truthy()), self.limits.max_stack_depth)?;
                }

                // ===== String Methods =====
                OpCode::StartsWith => {
                    let prefix = state.pop()?;
                    let s = state.pop()?;
                    let result = match (s, prefix) {
                        (Value::Str(s), Value::Str(p)) => s.starts_with(p.as_ref()),
                        (Value::Body(_), Value::Str(p)) => {
                            DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime)
                                .text_predicate(BodyTextOperation::StartsWith(p.to_string()))
                                .await?
                        }
                        _ => false, // Nil-friendly: returns false
                    };
                    state.push(Value::Bool(result), self.limits.max_stack_depth)?;
                }
                OpCode::EndsWith => {
                    let suffix = state.pop()?;
                    let s = state.pop()?;
                    let result = match (s, suffix) {
                        (Value::Str(s), Value::Str(p)) => s.ends_with(p.as_ref()),
                        (Value::Body(_), Value::Str(p)) => {
                            DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime)
                                .text_predicate(BodyTextOperation::EndsWith(p.to_string()))
                                .await?
                        }
                        _ => false,
                    };
                    state.push(Value::Bool(result), self.limits.max_stack_depth)?;
                }
                OpCode::Contains => {
                    let substr = state.pop()?;
                    let s = state.pop()?;
                    let result = match (s, substr) {
                        (Value::Str(s), Value::Str(p)) => s.contains(p.as_ref()),
                        (Value::Body(_), Value::Str(p)) => {
                            DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime)
                                .text_predicate(BodyTextOperation::Contains(p.to_string()))
                                .await?
                        }
                        _ => false,
                    };
                    state.push(Value::Bool(result), self.limits.max_stack_depth)?;
                }
                OpCode::Matches(regex_idx) => {
                    let s = state.pop()?;
                    let Some(Some(dfa)) = self.streaming_regex_cache.get(regex_idx as usize) else {
                        return Err(RuntimeError::Internal {
                            message: format!("streaming_regex_cache[{}] missing or invalid", regex_idx),
                        });
                    };
                    let result = match s {
                        Value::Str(s) => {
                            let mut scanner = StreamingRegexScanner::new(Arc::clone(dfa)).map_err(map_body_error)?;
                            scanner.push(&s).map_err(map_body_error)?;
                            scanner.finish().map_err(map_body_error)?
                        }
                        Value::Body(_) => {
                            DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime)
                                .scan_with(async {
                                    StreamingRegexScanner::new(Arc::clone(dfa)).map(LossyTextScanner::new)
                                })
                                .await?
                        }
                        _ => false,
                    };
                    state.push(Value::Bool(result), self.limits.max_stack_depth)?;
                }
                OpCode::TextLen => {
                    let value = state.pop()?;
                    let len = match value {
                        Value::Str(value) => value.chars().count() as u64,
                        Value::Body(_) => {
                            DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime)
                                .char_len()
                                .await?
                        }
                        other => {
                            return Err(RuntimeError::TypeError {
                                expected: "Str or Body",
                                got: other.type_name(),
                                operation: "len",
                            })
                        }
                    };
                    let len = i64::try_from(len).map_err(|_| body_access_error("request body length exceeds Int"))?;
                    state.push(Value::Int(len), self.limits.max_stack_depth)?;
                }
                OpCode::TextIsEmpty => {
                    let value = state.pop()?;
                    let empty = match value {
                        Value::Str(value) => value.is_empty(),
                        Value::Body(_) => {
                            DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime)
                                .text_predicate(BodyTextOperation::IsEmpty)
                                .await?
                        }
                        _ => false,
                    };
                    state.push(Value::Bool(empty), self.limits.max_stack_depth)?;
                }
                OpCode::BodyByteLen => {
                    expect_body(state.pop()?, "body.byte_len")?;
                    let len =
                        DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime).byte_len()?;
                    let len = i64::try_from(len).map_err(|_| body_access_error("request body length exceeds Int"))?;
                    state.push(Value::Int(len), self.limits.max_stack_depth)?;
                }
                OpCode::BodyRead => {
                    let max_bytes = nonnegative_u64(state.pop()?, "body.read")?;
                    let offset = nonnegative_u64(state.pop()?, "body.read")?;
                    expect_body(state.pop()?, "body.read")?;
                    let text = DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime)
                        .read_text_range(offset, max_bytes)
                        .await?;
                    state.push(Value::Str(text.into()), self.limits.max_stack_depth)?;
                }
                OpCode::BodySha256 => {
                    expect_body(state.pop()?, "body.sha256")?;
                    let digest = DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime)
                        .sha256()
                        .await?;
                    state.push(Value::Str(digest.into()), self.limits.max_stack_depth)?;
                }
                OpCode::BodyMd5 => {
                    expect_body(state.pop()?, "body.md5")?;
                    let digest = DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime)
                        .md5()
                        .await?;
                    state.push(Value::Str(digest.into()), self.limits.max_stack_depth)?;
                }
                OpCode::BodyHmacSha256 => {
                    let key = match state.pop()? {
                        Value::Str(key) => key,
                        other => {
                            return Err(RuntimeError::TypeError {
                                expected: "Str",
                                got: other.type_name(),
                                operation: "body.hmac_sha256",
                            })
                        }
                    };
                    expect_body(state.pop()?, "body.hmac_sha256")?;
                    let digest = DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime)
                        .hmac_sha256(key.as_bytes())
                        .await?;
                    state.push(Value::Str(digest.into()), self.limits.max_stack_depth)?;
                }
                OpCode::JsonGet(path_idx) => {
                    let input = state.pop()?;
                    let Some(Some(path)) = self.json_path_cache.get(path_idx as usize) else {
                        return Err(RuntimeError::Internal {
                            message: format!("json_path_cache[{}] missing or invalid", path_idx),
                        });
                    };
                    let result = match input {
                        Value::Str(text) => JsonPathScanner::scan_owned(text, path.clone(), self.limits.max_string_len)
                            .await
                            .map_err(map_body_error)?,
                        Value::Body(_) => {
                            DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime)
                                .scan_with(async {
                                    JsonPathScanner::new(path.clone(), self.limits.max_string_len).await
                                })
                                .await?
                        }
                        _ => JsonScanValue::Missing,
                    };
                    let value = match result {
                        JsonScanValue::Value(value) => value,
                        JsonScanValue::Missing => Value::Nil,
                        JsonScanValue::TooLong => {
                            return Err(RuntimeError::StringTooLong {
                                len: self.limits.max_string_len.saturating_add(1),
                                limit: self.limits.max_string_len,
                            })
                        }
                    };
                    state.push(value, self.limits.max_stack_depth)?;
                }
                OpCode::JsonSelect(plan_idx) => {
                    expect_body(state.pop()?, "body.json_select")?;
                    let Some(Some(plan)) = self.json_select_cache.get(plan_idx as usize) else {
                        return Err(RuntimeError::Internal {
                            message: format!("json_select_cache[{}] missing or invalid", plan_idx),
                        });
                    };
                    let values = DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime)
                        .scan_with(async { JsonSelectScanner::new(Arc::clone(plan), self.limits.max_string_len).await })
                        .await?;

                    let mut output = indexmap::IndexMap::with_capacity(plan.outputs().len());
                    for (name, capture) in plan.outputs() {
                        let value = match values.get(*capture) {
                            Some(JsonScanValue::Value(value)) => value.clone(),
                            Some(JsonScanValue::Missing) | None => Value::Nil,
                            Some(JsonScanValue::TooLong) => {
                                return Err(RuntimeError::StringTooLong {
                                    len: self.limits.max_string_len.saturating_add(1),
                                    limit: self.limits.max_string_len,
                                });
                            }
                        };
                        output.insert(name.to_string(), value);
                    }
                    state.push(Value::Map(Box::new(output)), self.limits.max_stack_depth)?;
                }
                OpCode::BodyClear => {
                    expect_body(state.pop()?, "body.clear")?;
                    DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime)
                        .clear()
                        .await?;
                    state.push(Value::Nil, self.limits.max_stack_depth)?;
                }
                OpCode::BodyAppend => {
                    let bytes = expect_string(state.pop()?, "body.append")?;
                    expect_body(state.pop()?, "body.append")?;
                    DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime)
                        .append(Bytes::from(bytes))
                        .await?;
                    state.push(Value::Nil, self.limits.max_stack_depth)?;
                }
                OpCode::BodyPrepend => {
                    let bytes = expect_string(state.pop()?, "body.prepend")?;
                    expect_body(state.pop()?, "body.prepend")?;
                    DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime)
                        .prepend(Bytes::from(bytes))
                        .await?;
                    state.push(Value::Nil, self.limits.max_stack_depth)?;
                }
                OpCode::BodyRemove => {
                    let end = nonnegative_u64(state.pop()?, "body.remove")?;
                    let start = nonnegative_u64(state.pop()?, "body.remove")?;
                    expect_body(state.pop()?, "body.remove")?;
                    DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime)
                        .remove(start..end)
                        .await?;
                    state.push(Value::Nil, self.limits.max_stack_depth)?;
                }
                OpCode::BodyReplace => {
                    let to = expect_string(state.pop()?, "body.replace")?;
                    let from = expect_string(state.pop()?, "body.replace")?;
                    expect_body(state.pop()?, "body.replace")?;
                    if from.is_empty() {
                        return Err(RuntimeError::TypeError {
                            expected: "non-empty Str",
                            got: "Str",
                            operation: "body.replace",
                        });
                    }
                    DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime)
                        .replace_literal(Bytes::from(from), Bytes::from(to))
                        .await?;
                    state.push(Value::Nil, self.limits.max_stack_depth)?;
                }
                OpCode::RegexFind(regex_idx) => {
                    let s = state.pop()?;
                    // Nil-friendly: non-Str input returns Nil, consistent with Matches.
                    let result = match s {
                        Value::Str(s) => {
                            let Some(Some(re)) = self.regex_cache.get(regex_idx as usize) else {
                                return Err(RuntimeError::Internal {
                                    message: format!("regex_cache[{}] missing or invalid", regex_idx),
                                });
                            };
                            if let Some(m) = re.find(&s) {
                                let found = m.as_str();
                                if found.len() > self.limits.max_string_len {
                                    return Err(RuntimeError::StringTooLong {
                                        len: found.len(),
                                        limit: self.limits.max_string_len,
                                    });
                                }
                                Value::Str(Arc::from(found))
                            } else {
                                Value::Nil
                            }
                        }
                        _ => Value::Nil,
                    };
                    state.push(result, self.limits.max_stack_depth)?;
                }
                OpCode::RegexReplace(regex_idx) => {
                    let replacement = state.pop()?.runtime_string()?;
                    let s = state.pop()?;
                    // Nil-friendly: non-Str input returns Nil, consistent with RegexFind.
                    let Value::Str(s) = s else {
                        state.push(Value::Nil, self.limits.max_stack_depth)?;
                        continue;
                    };
                    let Some(Some(re)) = self.regex_cache.get(regex_idx as usize) else {
                        return Err(RuntimeError::Internal {
                            message: format!("regex_cache[{}] missing or invalid", regex_idx),
                        });
                    };
                    let result: Arc<str> = match replace_all_bounded(re, &s, replacement, self.limits.max_string_len) {
                        Ok(None) => Arc::clone(&s),
                        Ok(Some(output)) => Arc::from(output),
                        Err(BoundedRegexReplacementError::OutputTooLong { len }) => {
                            return Err(RuntimeError::StringTooLong {
                                len,
                                limit: self.limits.max_string_len,
                            });
                        }
                        Err(BoundedRegexReplacementError::AllocationFailed) => {
                            return Err(RuntimeError::Internal {
                                message: "bounded regex replacement output allocation failed".to_string(),
                            });
                        }
                        Err(BoundedRegexReplacementError::InvariantViolation) => {
                            return Err(RuntimeError::Internal {
                                message: "bounded regex replacement invariant failed".to_string(),
                            });
                        }
                    };
                    if result.len() > self.limits.max_string_len {
                        return Err(RuntimeError::StringTooLong {
                            len: result.len(),
                            limit: self.limits.max_string_len,
                        });
                    }
                    state.push(Value::Str(result), self.limits.max_stack_depth)?;
                }

                // ===== Control Flow =====
                OpCode::Jump(offset) => {
                    let target = state.pc as i64 + offset as i64;
                    if target < 0 || target as usize > self.script.code.len() {
                        return Err(RuntimeError::Internal {
                            message: format!(
                                "Jump target {} out of bounds (code length: {})",
                                target,
                                self.script.code.len()
                            ),
                        });
                    }
                    state.pc = target as usize;
                }
                OpCode::JumpIfFalse(offset) => {
                    let v = state.pop()?;
                    if !v.is_truthy() {
                        let target = state.pc as i64 + offset as i64;
                        if target < 0 || target as usize > self.script.code.len() {
                            return Err(RuntimeError::Internal {
                                message: format!(
                                    "JumpIfFalse target {} out of bounds (code length: {})",
                                    target,
                                    self.script.code.len()
                                ),
                            });
                        }
                        state.pc = target as usize;
                    }
                }
                OpCode::LoopInit => {
                    state.loop_counters.push(0);
                }
                OpCode::LoopBack(offset) => {
                    if let Some(counter) = state.loop_counters.last_mut() {
                        *counter += 1;
                        if *counter > self.limits.max_loop_iterations {
                            return Err(RuntimeError::LoopLimitExceeded {
                                limit: self.limits.max_loop_iterations,
                            });
                        }
                    } else {
                        return Err(RuntimeError::Internal {
                            message: "LoopBack without matching LoopInit".into(),
                        });
                    }
                    let target = state.pc as i64 + offset as i64;
                    if target < 0 || target as usize > self.script.code.len() {
                        return Err(RuntimeError::Internal {
                            message: format!(
                                "LoopBack target {} out of bounds (code length: {})",
                                target,
                                self.script.code.len()
                            ),
                        });
                    }
                    state.pc = target as usize;
                }
                OpCode::LoopEnd => {
                    // Clean up the loop counter pushed by LoopInit
                    state.loop_counters.pop();
                }

                // ===== List Operations =====
                OpCode::ListLen => {
                    let list = state.pop()?;
                    let len = match &list {
                        Value::List(l) => l.len() as i64,
                        _ => 0,
                    };
                    state.push(Value::Int(len), self.limits.max_stack_depth)?;
                }
                OpCode::IndexGet => {
                    let idx = state.pop()?;
                    let container = state.pop()?;
                    let val = match (&container, &idx) {
                        (Value::List(l), Value::Int(i)) => {
                            if *i < 0 {
                                Value::Nil
                            } else {
                                l.get(*i as usize)
                                    .map(|s| Value::Str(Arc::clone(s)))
                                    .unwrap_or(Value::Nil)
                            }
                        }
                        (Value::Map(m), Value::Str(k)) => m.get(k.as_ref()).cloned().unwrap_or(Value::Nil),
                        _ => Value::Nil,
                    };
                    state.push(val, self.limits.max_stack_depth)?;
                }

                // ===== Map Operations =====
                OpCode::MakeMap(n) => {
                    use indexmap::IndexMap;
                    let count = n as usize;
                    // Defense-in-depth: the validator already rejects oversized MakeMap at
                    // compile time, but injected or hand-crafted bytecode bypasses that path.
                    if count > self.limits.max_map_size {
                        return Err(RuntimeError::MapTooLarge {
                            size: count,
                            limit: self.limits.max_map_size,
                        });
                    }
                    let mut map: IndexMap<String, Value> = IndexMap::with_capacity(count);
                    let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(count);
                    for _ in 0..count {
                        let val = state.pop()?;
                        let key = state.pop()?;
                        pairs.push((key, val));
                    }
                    for (k, v) in pairs.into_iter().rev() {
                        let key_str = k.runtime_string()?;
                        map.insert(key_str, v);
                    }
                    state.push(Value::Map(Box::new(map)), self.limits.max_stack_depth)?;
                }

                // ===== Builtin Calls =====
                OpCode::CallBuiltin(id, argc) => {
                    state.tick_call()?;
                    let result = if id == BuiltinId::HttpCall {
                        self.call_builtin_http(argc, &mut state, session, runtime).await?
                    } else if id.is_redis_builtin() {
                        self.call_builtin_redis(id, argc, &mut state, runtime, log).await?
                    } else if id.is_body_builtin() {
                        self.call_builtin_body(id, &mut state, session, log).await?
                    } else {
                        self.call_builtin(id, argc, &mut state, session, log)?
                    };
                    state.push(result, self.limits.max_stack_depth)?;
                }

                // ===== Termination =====
                OpCode::ReturnNext => {
                    return Ok(PluginRunningResult::GoodNext);
                }
                OpCode::ReturnDeny => {
                    let body = state.pop()?;
                    let status = state.pop()?;
                    let status_code = match &status {
                        Value::Int(n) => {
                            let code = *n;
                            u16::try_from(code)
                                .ok()
                                .filter(|status| edgion_resources::http_status::is_valid_final_http_status(*status))
                                .unwrap_or(500)
                        }
                        _ => 500,
                    };
                    let body_bytes = match body {
                        Value::Nil => None,
                        other => Some(Bytes::from(other.runtime_string()?)),
                    };
                    return Ok(PluginRunningResult::ErrResponse {
                        status: status_code,
                        body: body_bytes,
                    });
                }
                OpCode::ReturnTerminate => {
                    return Ok(PluginRunningResult::ErrResponse {
                        status: 502,
                        body: None,
                    });
                }
                OpCode::ReturnGrpcError => {
                    let message = state.pop()?;
                    let code = state.pop()?;
                    let grpc_code = match &code {
                        Value::Int(n) => {
                            let c = *n;
                            if !(0..=16).contains(&c) {
                                13u16
                            } else {
                                c as u16
                            }
                        }
                        _ => 13, // INTERNAL
                    };
                    let message_str = match message {
                        Value::Nil => None,
                        other => Some(other.runtime_string()?),
                    };
                    return Ok(PluginRunningResult::ErrGrpcError {
                        grpc_status: grpc_code,
                        message: message_str,
                    });
                }
            }
        }
    }
}

// ==================== Safe Execution Wrapper ====================

/// Execute DSL script with full safety guarantees.
///
/// Any error (runtime error, panic, etc.) is handled according to
/// the configured error policy.
pub async fn execute_safe(
    vm: &Vm,
    session: &mut dyn PluginSession,
    log: &mut PluginLog,
    error_policy: &DslErrorPolicy,
    runtime: &DslRuntime,
) -> PluginRunningResult {
    use futures::FutureExt;
    let result = std::panic::AssertUnwindSafe(vm.execute(session, log, runtime))
        .catch_unwind()
        .await;

    match result {
        Ok(Ok(running_result)) => {
            match &running_result {
                // The VM only returns GoodNext / ErrResponse / ErrGrpcError.
                // Keep the other shared plugin-result arms exhaustive.
                PluginRunningResult::GoodNext | PluginRunningResult::Nothing => log.push("OK"),
                PluginRunningResult::ErrResponse { status, .. } => log.push(&format!("Deny {status}")),
                PluginRunningResult::ErrTerminateRequest => log.push("Deny terminate"),
                PluginRunningResult::ErrGrpcError { grpc_status, .. } => log.push(&format!("Deny grpc {grpc_status}")),
            };
            running_result
        }
        Ok(Err(runtime_err)) => {
            if matches!(&runtime_err, RuntimeError::BodyAccess { .. }) {
                log.push_err("request body access error");
                session.record_outcome(DyeOutcome::FailClose, "request-body-access-error");
                return PluginRunningResult::ErrResponse {
                    status: 500,
                    body: Some(Bytes::from_static(b"Request body processing failed")),
                };
            }
            let kind: &'static str = match &runtime_err {
                RuntimeError::TypeError { .. } => "type error",
                RuntimeError::DivisionByZero => "division by zero",
                RuntimeError::IntegerOverflow { .. } => "integer overflow",
                RuntimeError::StepLimitExceeded { .. } => "step limit exceeded",
                RuntimeError::LoopLimitExceeded { .. } => "loop limit exceeded",
                RuntimeError::CallLimitExceeded { .. } => "call limit exceeded",
                RuntimeError::StackOverflow { .. } => "stack overflow",
                RuntimeError::StringTooLong { .. } => "string too long",
                RuntimeError::ListTooLong { .. } => "list too long",
                RuntimeError::MapTooLarge { .. } => "map too large",
                RuntimeError::ApiError { .. } => "api error",
                RuntimeError::BodyAccess { .. } => unreachable!("handled above"),
                RuntimeError::RegexError { .. } => "regex error",
                RuntimeError::Internal { .. } => "internal error",
            };
            log.push_err(kind);
            apply_error_policy(session, error_policy)
        }
        Err(_panic) => {
            log.push_err("vm panic");
            apply_error_policy(session, error_policy)
        }
    }
}

/// Single choke point for every DSL error-policy decision (a runtime error
/// of any `RuntimeError` variant, or a guest panic). Also the single choke
/// point for the dye suite's dependency-failure recording: `record_outcome`
/// is called here, before returning `PluginRunningResult`, so it is always
/// ordered before the runtime's own response write. The specific
/// `RuntimeError` variant is not distinguished at the dye layer (it is
/// already captured in the `PluginLog` `kind` tag above), so cause
/// `script-error` is the single most accurate token for every arm.
fn apply_error_policy(session: &mut dyn PluginSession, policy: &DslErrorPolicy) -> PluginRunningResult {
    match policy {
        DslErrorPolicy::Ignore => {
            session.record_outcome(DyeOutcome::FailOpen, "script-error");
            PluginRunningResult::GoodNext
        }
        DslErrorPolicy::Deny => {
            session.record_outcome(DyeOutcome::FailClose, "script-error");
            PluginRunningResult::ErrResponse {
                status: 500,
                body: Some(Bytes::from_static(b"DSL execution error")),
            }
        }
        DslErrorPolicy::DenyWith { status, body } => {
            session.record_outcome(DyeOutcome::FailClose, "script-error");
            PluginRunningResult::ErrResponse {
                status: if edgion_resources::http_status::is_valid_final_http_status(*status) {
                    *status
                } else {
                    500
                },
                body: body.clone().map(Bytes::from),
            }
        }
    }
}

// ==================== Test helpers (cfg(test) only) ====================

#[cfg(test)]
impl Vm {
    /// Expose the internal const_str_cache for alignment invariant tests.
    pub(crate) fn const_str_cache_ref(&self) -> &[Option<Arc<str>>] {
        &self.const_str_cache
    }
}

// ==================== VM unit tests ====================

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal CompiledScript with the given constants and no real code.
    fn script_with_constants(constants: Vec<Constant>) -> CompiledScript {
        CompiledScript {
            version: super::super::bytecode::BYTECODE_VERSION,
            code: vec![OpCode::ReturnNext],
            constants,
            local_count: 0,
            max_loop_depth: 0,
            source: None,
            source_map: None,
        }
    }

    // -----------------------------------------------------------------------
    // Group C — const_str_cache alignment invariant
    // -----------------------------------------------------------------------

    /// Empty constant pool: cache length is 0, no panics.
    #[test]
    fn test_const_str_cache_empty_pool() {
        let script = script_with_constants(vec![]);
        let vm = Vm::new(script, VmLimits::default()).expect("vm creation failed");
        assert_eq!(vm.const_str_cache_ref().len(), 0);
    }

    /// Mixed pool: cache is parallel to constants; only Str slots are Some.
    #[test]
    fn test_const_str_cache_alignment() {
        let constants = vec![
            Constant::Str("hello".to_string()),
            Constant::Int(42),
            Constant::Str("world".to_string()),
        ];
        let script = script_with_constants(constants.clone());
        let vm = Vm::new(script, VmLimits::default()).expect("vm creation failed");
        let cache = vm.const_str_cache_ref();

        // Length must match the constant pool exactly.
        assert_eq!(
            cache.len(),
            constants.len(),
            "cache length must equal constant pool size"
        );

        // Index 0: Str("hello") → Some("hello")
        assert!(cache[0].is_some(), "Str constant at index 0 should be Some");
        assert_eq!(
            cache[0].as_deref(),
            Some("hello"),
            "Str constant content mismatch at index 0"
        );

        // Index 1: Int(42) → None
        assert!(cache[1].is_none(), "Int constant at index 1 should be None");

        // Index 2: Str("world") → Some("world")
        assert!(cache[2].is_some(), "Str constant at index 2 should be Some");
        assert_eq!(
            cache[2].as_deref(),
            Some("world"),
            "Str constant content mismatch at index 2"
        );
    }

    /// Regex constants must produce None in the str cache and Some in regex_cache.
    #[test]
    fn test_const_str_cache_regex_is_none() {
        let constants = vec![
            Constant::Str("prefix".to_string()),
            Constant::Regex("\\d+".to_string()),
            Constant::Int(0),
            Constant::Str("suffix".to_string()),
        ];
        let script = script_with_constants(constants.clone());
        let vm = Vm::new(script, VmLimits::default()).expect("vm creation failed");
        let cache = vm.const_str_cache_ref();

        assert_eq!(cache.len(), constants.len());

        // Str slots
        assert!(cache[0].is_some());
        assert_eq!(cache[0].as_deref(), Some("prefix"));
        assert!(cache[3].is_some());
        assert_eq!(cache[3].as_deref(), Some("suffix"));

        // Regex slot → None in str cache
        assert!(cache[1].is_none(), "Regex constant should be None in const_str_cache");

        // Int slot → None
        assert!(cache[2].is_none(), "Int constant should be None in const_str_cache");
    }

    /// All-Str pool: every slot is Some with the correct content.
    #[test]
    fn test_const_str_cache_all_str() {
        let strings = ["alpha", "beta", "gamma", "delta"];
        let constants: Vec<Constant> = strings.iter().map(|s| Constant::Str(s.to_string())).collect();
        let script = script_with_constants(constants);
        let vm = Vm::new(script, VmLimits::default()).expect("vm creation failed");
        let cache = vm.const_str_cache_ref();

        assert_eq!(cache.len(), strings.len());
        for (i, expected) in strings.iter().enumerate() {
            assert!(cache[i].is_some(), "index {} should be Some", i);
            assert_eq!(cache[i].as_deref(), Some(*expected), "content mismatch at index {}", i);
        }
    }

    /// All-Int pool: every slot is None.
    #[test]
    fn test_const_str_cache_all_int() {
        let constants = vec![Constant::Int(1), Constant::Int(2), Constant::Int(3)];
        let script = script_with_constants(constants.clone());
        let vm = Vm::new(script, VmLimits::default()).expect("vm creation failed");
        let cache = vm.const_str_cache_ref();

        assert_eq!(cache.len(), constants.len());
        for (i, entry) in cache.iter().enumerate() {
            assert!(entry.is_none(), "Int constant at index {} should be None", i);
        }
    }

    /// Arc content equality: two Vm instances built from the same script
    /// must produce independently-owned but content-equal Arcs.
    #[test]
    fn test_const_str_cache_arc_content_equality() {
        let constants = vec![Constant::Str("shared".to_string())];
        let script1 = script_with_constants(constants.clone());
        let script2 = script_with_constants(constants);
        let vm1 = Vm::new(script1, VmLimits::default()).expect("vm1 creation failed");
        let vm2 = Vm::new(script2, VmLimits::default()).expect("vm2 creation failed");

        let arc1 = vm1.const_str_cache_ref()[0].as_ref().unwrap();
        let arc2 = vm2.const_str_cache_ref()[0].as_ref().unwrap();

        // Content must match even though the Arcs are independent allocations.
        assert_eq!(arc1.as_ref(), arc2.as_ref(), "Arc content should be equal");
        // Pointer identity is NOT asserted — they are independent Vms.
    }
}
