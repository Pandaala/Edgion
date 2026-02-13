//! Built-in function dispatch — maps BuiltinId to PluginSession methods
//!
//! Arguments are popped from the VM stack, results pushed back.
//! Each builtin maps to exactly one PluginSession API call.

use crate::core::plugins::plugin_runtime::{PluginLog, PluginSession};

use super::bytecode::BuiltinId;
use super::error::RuntimeError;
use super::value::Value;
use super::vm::{Vm, VmState};

impl Vm {
    pub(crate) fn call_builtin(
        &self,
        id: BuiltinId,
        _argc: u8,
        state: &mut VmState,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> Result<Value, RuntimeError> {
        match id {
            // ===== req.* read =====
            BuiltinId::ReqHeader => {
                let name = state.pop()?.into_string();
                Ok(session.header_value(&name).into())
            }
            BuiltinId::ReqMethod => Ok(Value::Str(session.get_method().to_string())),
            BuiltinId::ReqPath => Ok(Value::Str(session.get_path().to_string())),
            BuiltinId::ReqQuery => {
                let name = state.pop()?.into_string();
                Ok(session.get_query_param(&name).into())
            }
            BuiltinId::ReqQueryString => Ok(session.get_query().into()),
            BuiltinId::ReqCookie => {
                let name = state.pop()?.into_string();
                Ok(session.get_cookie(&name).into())
            }
            BuiltinId::ReqClientIp => Ok(Value::Str(session.client_addr().to_string())),
            BuiltinId::ReqRemoteIp => Ok(Value::Str(session.remote_addr().to_string())),
            BuiltinId::ReqPathParam => {
                let name = state.pop()?.into_string();
                Ok(session.get_path_param(&name).into())
            }
            BuiltinId::ReqHeaderNames => {
                let headers = session.request_headers();
                let names: Vec<String> = headers.into_iter().map(|(k, _)| k).collect();
                Ok(Value::List(names))
            }
            BuiltinId::ReqScheme => {
                // Use header_value as a fallback for scheme
                Ok(Value::Str("http".to_string()))
            }
            BuiltinId::ReqHost => Ok(session.header_value("Host").into()),
            BuiltinId::ReqUri => Ok(Value::Str(session.get_path().to_string())),
            BuiltinId::ReqContentType => Ok(session.header_value("Content-Type").into()),
            BuiltinId::ReqHasHeader => {
                let name = state.pop()?.into_string();
                Ok(Value::Bool(session.header_value(&name).is_some()))
            }

            // ===== req.* mutation =====
            BuiltinId::ReqSetHeader => {
                let value = state.pop()?.into_string();
                let name = state.pop()?.into_string();
                session
                    .set_request_header(&name, &value)
                    .map_err(|e| RuntimeError::ApiError {
                        function: "req.set_header".into(),
                        message: e.to_string(),
                    })?;
                Ok(Value::Nil)
            }
            BuiltinId::ReqAppendHeader => {
                let value = state.pop()?.into_string();
                let name = state.pop()?.into_string();
                session
                    .append_request_header(&name, &value)
                    .map_err(|e| RuntimeError::ApiError {
                        function: "req.append_header".into(),
                        message: e.to_string(),
                    })?;
                Ok(Value::Nil)
            }
            BuiltinId::ReqRemoveHeader => {
                let name = state.pop()?.into_string();
                session
                    .remove_request_header(&name)
                    .map_err(|e| RuntimeError::ApiError {
                        function: "req.remove_header".into(),
                        message: e.to_string(),
                    })?;
                Ok(Value::Nil)
            }
            BuiltinId::ReqSetUri => {
                let uri = state.pop()?.into_string();
                session
                    .set_upstream_uri(&uri)
                    .map_err(|e| RuntimeError::ApiError {
                        function: "req.set_uri".into(),
                        message: e.to_string(),
                    })?;
                Ok(Value::Nil)
            }
            BuiltinId::ReqSetHost => {
                let host = state.pop()?.into_string();
                session
                    .set_upstream_host(&host)
                    .map_err(|e| RuntimeError::ApiError {
                        function: "req.set_host".into(),
                        message: e.to_string(),
                    })?;
                Ok(Value::Nil)
            }
            BuiltinId::ReqSetMethod => {
                let method = state.pop()?.into_string();
                session
                    .set_upstream_method(&method)
                    .map_err(|e| RuntimeError::ApiError {
                        function: "req.set_method".into(),
                        message: e.to_string(),
                    })?;
                Ok(Value::Nil)
            }

            // ===== resp.* =====
            BuiltinId::RespSetHeader => {
                let value = state.pop()?.into_string();
                let name = state.pop()?.into_string();
                session
                    .set_response_header(&name, &value)
                    .map_err(|e| RuntimeError::ApiError {
                        function: "resp.set_header".into(),
                        message: e.to_string(),
                    })?;
                Ok(Value::Nil)
            }
            BuiltinId::RespAppendHeader => {
                let value = state.pop()?.into_string();
                let name = state.pop()?.into_string();
                session
                    .append_response_header(&name, &value)
                    .map_err(|e| RuntimeError::ApiError {
                        function: "resp.append_header".into(),
                        message: e.to_string(),
                    })?;
                Ok(Value::Nil)
            }
            BuiltinId::RespRemoveHeader => {
                let name = state.pop()?.into_string();
                session
                    .remove_response_header(&name)
                    .map_err(|e| RuntimeError::ApiError {
                        function: "resp.remove_header".into(),
                        message: e.to_string(),
                    })?;
                Ok(Value::Nil)
            }

            // ===== ctx.* =====
            BuiltinId::CtxGet => {
                let key = state.pop()?.into_string();
                Ok(session.get_ctx_var(&key).into())
            }
            BuiltinId::CtxSet => {
                let value = state.pop()?.into_string();
                let key = state.pop()?.into_string();
                session
                    .set_ctx_var(&key, &value)
                    .map_err(|e| RuntimeError::ApiError {
                        function: "ctx.set".into(),
                        message: e.to_string(),
                    })?;
                Ok(Value::Nil)
            }
            BuiltinId::CtxRemove => {
                let key = state.pop()?.into_string();
                session
                    .remove_ctx_var(&key)
                    .map_err(|e| RuntimeError::ApiError {
                        function: "ctx.remove".into(),
                        message: e.to_string(),
                    })?;
                Ok(Value::Nil)
            }

            // ===== Utilities =====
            BuiltinId::Log => {
                let msg = state.pop()?.into_string();
                log.push(&msg);
                Ok(Value::Nil)
            }
            BuiltinId::Len => {
                let v = state.pop()?;
                match &v {
                    Value::Str(s) => Ok(Value::Int(s.len() as i64)),
                    Value::Nil => Ok(Value::Int(0)),
                    Value::List(l) => Ok(Value::Int(l.len() as i64)),
                    _ => Err(RuntimeError::TypeError {
                        expected: "Str",
                        got: v.type_name(),
                        operation: "len()",
                    }),
                }
            }
            BuiltinId::Substr => {
                let end = state.pop()?.as_int().unwrap_or(0) as usize;
                let start = state.pop()?.as_int().unwrap_or(0) as usize;
                let s = state.pop()?;
                match &s {
                    Value::Str(s) => {
                        let start = start.min(s.len());
                        let end = end.min(s.len());
                        if start <= end {
                            Ok(Value::Str(s[start..end].to_string()))
                        } else {
                            Ok(Value::Str(String::new()))
                        }
                    }
                    Value::Nil => Ok(Value::Str(String::new())),
                    _ => Err(RuntimeError::TypeError {
                        expected: "Str",
                        got: s.type_name(),
                        operation: "substr()",
                    }),
                }
            }
            BuiltinId::ToStr => {
                let v = state.pop()?;
                Ok(Value::Str(v.into_string()))
            }
            BuiltinId::ToInt => {
                let v = state.pop()?;
                match &v {
                    Value::Str(s) => Ok(s.parse::<i64>().map(Value::Int).unwrap_or(Value::Nil)),
                    Value::Int(n) => Ok(Value::Int(*n)),
                    _ => Ok(Value::Nil),
                }
            }
            BuiltinId::ToUpper => {
                let s = state.pop()?.into_string();
                Ok(Value::Str(s.to_uppercase()))
            }
            BuiltinId::ToLower => {
                let s = state.pop()?.into_string();
                Ok(Value::Str(s.to_lowercase()))
            }
            BuiltinId::Base64Encode => {
                use base64::Engine;
                let s = state.pop()?.into_string();
                let encoded = base64::engine::general_purpose::STANDARD.encode(s.as_bytes());
                Ok(Value::Str(encoded))
            }
            BuiltinId::Base64Decode => {
                use base64::Engine;
                let s = state.pop()?.into_string();
                match base64::engine::general_purpose::STANDARD.decode(s.as_bytes()) {
                    Ok(bytes) => Ok(Value::Str(String::from_utf8_lossy(&bytes).to_string())),
                    Err(_) => Ok(Value::Nil),
                }
            }
            BuiltinId::TimeNow => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                Ok(Value::Int(now))
            }
            // Remaining builtins — return Nil for now, implement as needed
            _ => Ok(Value::Nil),
        }
    }
}

