//! EdgionDSL stack-based VM interpreter with safety sandbox
//!
//! Security model:
//!   - Step budget: every instruction costs 1 step
//!   - Call budget: every CallBuiltin costs 1 call
//!   - Loop budget: per-loop iteration counter
//!   - Stack depth limit
//!   - String length limit
//!   - catch_unwind for fault isolation

use std::borrow::Cow;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession};
use crate::types::filters::PluginRunningResult;

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

impl Default for VmLimits {
    fn default() -> Self {
        Self {
            max_steps: 10_000,
            max_loop_iterations: 100,
            max_call_count: 500,
            max_stack_depth: 128,
            max_string_len: 8192,
        }
    }
}

// ==================== Error Policy ====================

/// DSL error handling policy — user-configurable per plugin
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub enum DslErrorPolicy {
    /// Ignore error, continue to next plugin (GoodNext) — fail-open
    #[serde(alias = "ignore")]
    #[default]
    Ignore,
    /// Return error response to client (default 500)
    #[serde(alias = "deny")]
    Deny,
    /// Return a specific status code + body
    #[serde(alias = "denyWith")]
    DenyWith { status: u16, body: Option<String> },
}

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
    regex_cache: Vec<Option<regex::Regex>>,
    pub(crate) limits: VmLimits,
}

impl Vm {
    /// Create a new VM with pre-compiled regex cache
    pub fn new(script: CompiledScript, limits: VmLimits) -> Result<Self, RuntimeError> {
        let mut regex_cache = Vec::with_capacity(script.constants.len());
        for constant in &script.constants {
            match constant {
                Constant::Regex(pattern) => {
                    let re = regex::Regex::new(pattern).map_err(|e| RuntimeError::RegexError {
                        message: format!("failed to compile regex '{}': {}", pattern, e),
                    })?;
                    regex_cache.push(Some(re));
                }
                _ => {
                    regex_cache.push(None);
                }
            }
        }
        Ok(Self {
            script,
            regex_cache,
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

            let opcode = self.script.code[state.pc].clone();
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
                        Constant::Str(s) => Value::Str(s.clone()),
                        Constant::Int(n) => Value::Int(*n),
                        Constant::Regex(_) => Value::Nil, // regex itself is not a value
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
                            Value::Str(format!("{}{}", a, b))
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
                            Value::Str(format!("{}{}", a, b_str))
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
                            Value::Str(format!("{}{}", a_str, b))
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
                        (Value::Str(s), Value::Str(p)) => s.starts_with(p.as_str()),
                        _ => false, // Nil-friendly: returns false
                    };
                    state.push(Value::Bool(result), self.limits.max_stack_depth)?;
                }
                OpCode::EndsWith => {
                    let suffix = state.pop()?;
                    let s = state.pop()?;
                    let result = match (&s, &suffix) {
                        (Value::Str(s), Value::Str(p)) => s.ends_with(p.as_str()),
                        _ => false,
                    };
                    state.push(Value::Bool(result), self.limits.max_stack_depth)?;
                }
                OpCode::Contains => {
                    let substr = state.pop()?;
                    let s = state.pop()?;
                    let result = match (&s, &substr) {
                        (Value::Str(s), Value::Str(p)) => s.contains(p.as_str()),
                        _ => false,
                    };
                    state.push(Value::Bool(result), self.limits.max_stack_depth)?;
                }
                OpCode::Matches(regex_idx) => {
                    let s = state.pop()?;
                    let result = match &s {
                        Value::Str(s) => {
                            if let Some(Some(re)) = self.regex_cache.get(regex_idx as usize) {
                                re.is_match(s)
                            } else {
                                false
                            }
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
                            if let Some(Some(re)) = self.regex_cache.get(regex_idx as usize) {
                                if let Some(m) = re.find(&s) {
                                    let found = m.as_str();
                                    if found.len() > self.limits.max_string_len {
                                        return Err(RuntimeError::StringTooLong {
                                            len: found.len(),
                                            limit: self.limits.max_string_len,
                                        });
                                    }
                                    Value::Str(found.to_string())
                                } else {
                                    Value::Nil
                                }
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
                    let s = state.pop()?.into_string();
                    let result = if let Some(Some(re)) = self.regex_cache.get(regex_idx as usize) {
                        // Cow::Borrowed means nothing matched — reuse the already-owned String.
                        match re.replace_all(&s, replacement.as_str()) {
                            Cow::Borrowed(_) => s,
                            Cow::Owned(o) => o,
                        }
                    } else {
                        s
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
                                l.get(*i as usize).cloned().map(Value::Str).unwrap_or(Value::Nil)
                            }
                        }
                        (Value::Map(m), Value::Str(k)) => m.get(k.as_str()).cloned().unwrap_or(Value::Nil),
                        _ => Value::Nil,
                    };
                    state.push(val, self.limits.max_stack_depth)?;
                }

                // ===== Map Operations =====
                OpCode::MakeMap(n) => {
                    use indexmap::IndexMap;
                    let count = n as usize;
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
                    state.push(Value::Map(map), self.limits.max_stack_depth)?;
                }

                // ===== Builtin Calls =====
                OpCode::CallBuiltin(id, argc) => {
                    state.tick_call()?;
                    let result = if id == BuiltinId::HttpCall {
                        self.call_builtin_http(argc, &mut state, runtime).await?
                    } else if is_redis_builtin(id) {
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

/// Returns true for all `redis.*` builtin IDs that require async dispatch.
#[inline]
fn is_redis_builtin(id: BuiltinId) -> bool {
    matches!(
        id,
        BuiltinId::RedisGet
            | BuiltinId::RedisSet
            | BuiltinId::RedisDel
            | BuiltinId::RedisExists
            | BuiltinId::RedisTtl
            | BuiltinId::RedisExpire
            | BuiltinId::RedisIncr
            | BuiltinId::RedisIncrBy
            | BuiltinId::RedisHGet
            | BuiltinId::RedisHSet
            | BuiltinId::RedisHDel
            | BuiltinId::RedisHGetAll
            | BuiltinId::RedisHExists
    )
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
