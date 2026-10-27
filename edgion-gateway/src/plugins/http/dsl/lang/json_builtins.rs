//! Explicit bounded JSON strings and canonical request-body scanning.

use super::super::runtime::DslRuntime;
use super::body_ops::{body_access_error, map_body_read_error, DslBodyOps};
use super::bytecode::BuiltinId;
use super::error::RuntimeError;
use super::value::{Value, ValueWorkBudget};
use super::vm::{Vm, VmState};
use crate::plugins::runtime::PluginSession;
use crate::request_body::{BodyOperationError, RawBodyScanner, ScanControl};
use edgion_dsl::json::{self, JsonError, MemberValue, StrictJsonError, MAX_DSL_JSON_BYTES};
use std::sync::Arc;

fn json_error(error: JsonError) -> RuntimeError {
    match error {
        JsonError::Work(error) => error,
        JsonError::OutputLimit { limit } => RuntimeError::StringTooLong {
            len: limit.saturating_add(1),
            limit,
        },
        JsonError::Strict(StrictJsonError::DepthLimit) => RuntimeError::JsonLimitExceeded {
            kind: "depth",
            limit: json::DSL_JSON_DEPTH,
        },
        JsonError::Strict(StrictJsonError::NodeLimit) => RuntimeError::JsonLimitExceeded {
            kind: "nodes",
            limit: json::DSL_JSON_NODES as usize,
        },
        JsonError::Strict(StrictJsonError::TooLarge) => RuntimeError::JsonLimitExceeded {
            kind: "bytes",
            limit: MAX_DSL_JSON_BYTES,
        },
        JsonError::Strict(_) | JsonError::InvalidOperation => RuntimeError::ApiError {
            function: "json".into(),
            message: "invalid JSON operation".into(),
        },
        JsonError::Allocation => RuntimeError::ApiError {
            function: "json".into(),
            message: "bounded JSON allocation failed".into(),
        },
    }
}

fn malformed(error: &JsonError) -> bool {
    matches!(
        error,
        JsonError::Strict(StrictJsonError::Malformed | StrictJsonError::DuplicateKey)
    )
}

fn expect_string(value: Value, operation: &'static str) -> Result<Arc<str>, RuntimeError> {
    match value {
        Value::Str(text) => Ok(text),
        other => Err(RuntimeError::TypeError {
            expected: "Str",
            got: other.type_name(),
            operation,
        }),
    }
}

pub(crate) struct JsonScanOutcome {
    pub(crate) work: ValueWorkBudget,
    pub(crate) result: Result<Value, RuntimeError>,
}
pub(crate) struct JsonBodyScanner {
    source: Vec<u8>,
    work: ValueWorkBudget,
    limit: usize,
    error: Option<RuntimeError>,
}
impl JsonBodyScanner {
    pub(crate) fn new(work: ValueWorkBudget, limit: usize) -> Self {
        Self {
            source: Vec::new(),
            work,
            limit,
            error: None,
        }
    }
}
impl RawBodyScanner for JsonBodyScanner {
    type Output = JsonScanOutcome;
    fn requires_blocking_context(&self) -> bool {
        true
    }
    fn push(&mut self, chunk: &[u8]) -> Result<ScanControl, BodyOperationError> {
        let result = (|| -> Result<(), RuntimeError> {
            let next = self
                .source
                .len()
                .checked_add(chunk.len())
                .ok_or_else(|| body_access_error("JSON source limit exceeded"))?;
            if next > self.limit {
                return Err(body_access_error("JSON source limit exceeded"));
            }
            if next > self.source.capacity() {
                let target = next.max(self.source.capacity().saturating_mul(2)).min(self.limit);
                self.work.charge((target - self.source.capacity()) as u64 * 2)?;
                self.source
                    .try_reserve_exact(target - self.source.len())
                    .map_err(|_| body_access_error("bounded JSON allocation failed"))?;
            }
            self.work.charge(chunk.len() as u64 * 2)?;
            self.source.extend_from_slice(chunk);
            Ok(())
        })();
        match result {
            Ok(()) => Ok(ScanControl::Continue),
            Err(error) => {
                self.error = Some(body_access_error(error.to_string()));
                Ok(ScanControl::Stop)
            }
        }
    }
    fn finish(mut self) -> Result<Self::Output, BodyOperationError> {
        let result = if let Some(error) = self.error {
            Err(error)
        } else if let Ok(source) = std::str::from_utf8(&self.source) {
            match json::parse_raw(source, self.limit, &mut self.work) {
                Ok(_) => json::copy_raw(source, &mut self.work)
                    .map(Value::Str)
                    .map_err(|error| body_access_error(json_error(error).to_string())),
                Err(error) if malformed(&error) => Ok(Value::Nil),
                Err(error) => Err(body_access_error(json_error(error).to_string())),
            }
        } else {
            Ok(Value::Nil)
        };
        Ok(JsonScanOutcome {
            work: self.work,
            result,
        })
    }
}

impl Vm {
    pub(crate) async fn call_builtin_json(
        &self,
        id: BuiltinId,
        state: &mut VmState,
        session: &mut dyn PluginSession,
        runtime: &DslRuntime,
    ) -> Result<Value, RuntimeError> {
        let limit = self.limits.max_string_len.min(MAX_DSL_JSON_BYTES);
        if id == BuiltinId::JsonSetBody {
            if !session.ctx().request_info.in_request_body_stage {
                return Err(body_access_error(
                    "JSON body publication requires the request body stage",
                ));
            }
            let source =
                expect_string(state.pop()?, "json.set_body").map_err(|error| body_access_error(error.to_string()))?;
            let physical = session.ctx().request_body_config.max_body_size_bytes;
            if source.len() as u64 > physical {
                return Err(body_access_error("JSON body publication exceeds the physical cap"));
            }
            json::parse_raw(&source, limit, &mut state.value_work_budget)
                .map_err(|error| body_access_error(json_error(error).to_string()))?;
            state
                .value_work_budget
                .charge(source.len() as u64 * 2)
                .map_err(|error| body_access_error(error.to_string()))?;
            let mut outgoing = Vec::new();
            outgoing
                .try_reserve_exact(source.len())
                .map_err(|_| body_access_error("bounded JSON body allocation failed"))?;
            outgoing.extend_from_slice(source.as_bytes());
            drop(source);
            session
                .overwrite_request_body(bytes::Bytes::from(outgoing))
                .await
                .map_err(map_body_read_error)?;
            return Ok(Value::Bool(true));
        }
        if id == BuiltinId::JsonRead {
            match state.pop()? {
                Value::Str(source) => {
                    return match json::parse_raw(&source, limit, &mut state.value_work_budget) {
                        Ok(_) => Ok(Value::Str(source)),
                        Err(error) if malformed(&error) => Ok(Value::Nil),
                        Err(error) => Err(json_error(error)),
                    };
                }
                Value::Body(_) => {
                    let work = std::mem::replace(&mut state.value_work_budget, ValueWorkBudget::exhausted());
                    let outcome = DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime)
                        .read_json(work)
                        .await?;
                    state.value_work_budget = outcome.work;
                    return outcome.result;
                }
                other => {
                    return Err(RuntimeError::TypeError {
                        expected: "Str or Body",
                        got: other.type_name(),
                        operation: "json.read",
                    })
                }
            }
        }
        if matches!(id, BuiltinId::JsonSetRaw | BuiltinId::JsonSetString) {
            let value = expect_string(state.pop()?, "json.set")?;
            let key = expect_string(state.pop()?, "json.set")?;
            let source = expect_string(state.pop()?, "json.set")?;
            let member = if id == BuiltinId::JsonSetRaw {
                MemberValue::Raw(&value)
            } else {
                MemberValue::String(&value)
            };
            return json::set_member(&source, &key, member, limit, &mut state.value_work_budget)
                .map(Value::Str)
                .map_err(json_error);
        }
        let key = if id == BuiltinId::JsonKeys {
            None
        } else {
            Some(expect_string(state.pop()?, "json.get")?)
        };
        let source = expect_string(state.pop()?, "json.get")?;
        let document = match json::parse_raw(&source, limit, &mut state.value_work_budget) {
            Ok(document) => document,
            Err(error) if malformed(&error) => return Ok(Value::Nil),
            Err(error) => return Err(json_error(error)),
        };
        let Some(members) = document.members() else {
            return Ok(Value::Nil);
        };
        if id == BuiltinId::JsonKeys {
            if members.len() > self.limits.max_list_len {
                return Err(RuntimeError::ListTooLong {
                    len: members.len(),
                    limit: self.limits.max_list_len,
                });
            }
            state
                .value_work_budget
                .charge(members.len() as u64 * std::mem::size_of::<Arc<str>>() as u64 * 2)?;
            let mut keys = Vec::new();
            keys.try_reserve_exact(members.len())
                .map_err(|_| json_error(JsonError::Allocation))?;
            for (key, _) in members {
                keys.push(json::copy_raw(key, &mut state.value_work_budget).map_err(json_error)?);
            }
            return Ok(Value::List(keys));
        }
        let Some(raw) = document.get(key.as_deref().unwrap_or("")) else {
            return Ok(Value::Nil);
        };
        if id == BuiltinId::JsonGetString {
            json::decode_string(raw, &mut state.value_work_budget)
                .map(|text| text.map(Value::Str).unwrap_or(Value::Nil))
                .map_err(json_error)
        } else {
            json::copy_raw(raw, &mut state.value_work_budget)
                .map(Value::Str)
                .map_err(json_error)
        }
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/plugins/http/dsl/lang/json_builtins_test.rs"]
mod tests;
