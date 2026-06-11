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

use std::borrow::Cow;
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession};
use edgion_resources::filters::PluginRunningResult;

use super::super::runtime::DslRuntime;
use super::bytecode::*;
use super::error::RuntimeError;
use super::value::Value;

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
    10_000
}
fn default_max_loop_iterations() -> u32 {
    100
}
fn default_max_call_count() -> u32 {
    500
}
fn default_max_stack_depth() -> usize {
    128
}
fn default_max_string_len() -> usize {
    8192
}
fn default_max_list_len() -> usize {
    1024
}
fn default_max_map_size() -> usize {
    512
}

impl Default for VmLimits {
    fn default() -> Self {
        Self {
            max_steps: 10_000,
            max_loop_iterations: 100,
            max_call_count: 500,
            max_stack_depth: 128,
            max_string_len: 8192,
            max_list_len: 1024,
            max_map_size: 512,
        }
    }
}

// ==================== Error Policy ====================

// `DslErrorPolicy` is serializable schema and now lives in the `types` leaf
// layer. Re-export it here so existing `vm::DslErrorPolicy` references (config,
// plugin, tests) keep resolving; `apply_error_policy` below maps its variants
// onto runtime `PluginRunningResult`s.
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
    // Parallel to script.constants: Some for Str entries, None for Int/Regex.
    const_str_cache: Vec<Option<Arc<str>>>,
    pub(crate) limits: VmLimits,
}

impl Vm {
    /// Create a new VM with pre-compiled regex cache
    pub fn new(script: CompiledScript, limits: VmLimits) -> Result<Self, RuntimeError> {
        let mut regex_cache = Vec::with_capacity(script.constants.len());
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
                    const_str_cache.push(None);
                }
                Constant::Str(s) => {
                    regex_cache.push(None);
                    const_str_cache.push(Some(Arc::from(s.as_str())));
                }
                Constant::Int(_) => {
                    regex_cache.push(None);
                    const_str_cache.push(None);
                }
            }
        }
        Ok(Self {
            script,
            regex_cache,
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
                            let b_str = b.into_string();
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
                            let a_str = a.into_string();
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
                    state.push(Value::Bool(a == b), self.limits.max_stack_depth)?;
                }
                OpCode::NotEqual => {
                    let b = state.pop()?;
                    let a = state.pop()?;
                    state.push(Value::Bool(a != b), self.limits.max_stack_depth)?;
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
                    let result = match (&s, &prefix) {
                        (Value::Str(s), Value::Str(p)) => s.starts_with(p.as_ref()),
                        _ => false, // Nil-friendly: returns false
                    };
                    state.push(Value::Bool(result), self.limits.max_stack_depth)?;
                }
                OpCode::EndsWith => {
                    let suffix = state.pop()?;
                    let s = state.pop()?;
                    let result = match (&s, &suffix) {
                        (Value::Str(s), Value::Str(p)) => s.ends_with(p.as_ref()),
                        _ => false,
                    };
                    state.push(Value::Bool(result), self.limits.max_stack_depth)?;
                }
                OpCode::Contains => {
                    let substr = state.pop()?;
                    let s = state.pop()?;
                    let result = match (&s, &substr) {
                        (Value::Str(s), Value::Str(p)) => s.contains(p.as_ref()),
                        _ => false,
                    };
                    state.push(Value::Bool(result), self.limits.max_stack_depth)?;
                }
                OpCode::Matches(regex_idx) => {
                    let s = state.pop()?;
                    let result = match &s {
                        Value::Str(s) => {
                            let Some(Some(re)) = self.regex_cache.get(regex_idx as usize) else {
                                return Err(RuntimeError::Internal {
                                    message: format!("regex_cache[{}] missing or invalid", regex_idx),
                                });
                            };
                            re.is_match(s)
                        }
                        _ => false,
                    };
                    state.push(Value::Bool(result), self.limits.max_stack_depth)?;
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
                    let replacement = state.pop()?.into_string();
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
                    // Cow::Borrowed means nothing matched — clone the Arc to avoid allocation.
                    let result: Arc<str> = match re.replace_all(&s, replacement.as_str()) {
                        Cow::Borrowed(_) => Arc::clone(&s),
                        Cow::Owned(o) => Arc::from(o.as_str()),
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
                        let key_str = k.into_string();
                        map.insert(key_str, v);
                    }
                    state.push(Value::Map(Box::new(map)), self.limits.max_stack_depth)?;
                }

                // ===== Builtin Calls =====
                OpCode::CallBuiltin(id, argc) => {
                    state.tick_call()?;
                    let result = if id == BuiltinId::HttpCall {
                        self.call_builtin_http(argc, &mut state, runtime).await?
                    } else if id.is_redis_builtin() {
                        self.call_builtin_redis(id, argc, &mut state, runtime).await?
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
                            if !(100..=599).contains(&code) {
                                500u16
                            } else {
                                code as u16
                            }
                        }
                        _ => 500,
                    };
                    let body_str = match body {
                        Value::Nil => None,
                        other => Some(other.into_string()),
                    };
                    return Ok(PluginRunningResult::ErrResponse {
                        status: status_code,
                        body: body_str,
                    });
                }
                OpCode::ReturnTerminate => {
                    return Ok(PluginRunningResult::ErrTerminateRequest);
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
                        other => Some(other.into_string()),
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
                // `Nothing` is unreachable for DSL scripts (the VM only ever
                // returns GoodNext / ErrResponse / ErrTerminateRequest /
                // ErrGrpcError); folded into the allow case for completeness.
                PluginRunningResult::GoodNext | PluginRunningResult::Nothing => log.push("OK; "),
                PluginRunningResult::ErrResponse { status, .. } => log.push(&format!("Deny {status}; ")),
                PluginRunningResult::ErrTerminateRequest => log.push("Deny terminate; "),
                PluginRunningResult::ErrGrpcError { grpc_status, .. } => {
                    log.push(&format!("Deny grpc {grpc_status}; "))
                }
            };
            running_result
        }
        Ok(Err(runtime_err)) => {
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
                RuntimeError::UndefinedLocal { .. } => "undefined local",
                RuntimeError::ApiError { .. } => "api error",
                RuntimeError::RegexError { .. } => "regex error",
                RuntimeError::Internal { .. } => "internal error",
            };
            log.push_err(kind);
            apply_error_policy(error_policy)
        }
        Err(_panic) => {
            log.push_err("vm panic");
            apply_error_policy(error_policy)
        }
    }
}

fn apply_error_policy(policy: &DslErrorPolicy) -> PluginRunningResult {
    match policy {
        DslErrorPolicy::Ignore => PluginRunningResult::GoodNext,
        DslErrorPolicy::Deny => PluginRunningResult::ErrResponse {
            status: 500,
            body: Some("DSL execution error".to_string()),
        },
        DslErrorPolicy::DenyWith { status, body } => PluginRunningResult::ErrResponse {
            status: *status,
            body: body.clone(),
        },
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
