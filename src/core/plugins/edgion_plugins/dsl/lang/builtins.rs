//! Built-in function dispatch — maps BuiltinId to PluginSession methods
//!
//! Arguments are popped from the VM stack, results pushed back.
//! Each builtin maps to exactly one PluginSession API call.

use crate::core::plugins::plugin_runtime::{PluginLog, PluginSession};

use super::bytecode::BuiltinId;
use super::error::RuntimeError;
use super::value::Value;
use super::vm::{Vm, VmState};

/// Check that a string result doesn't exceed the VM's max_string_len limit.
fn check_string_len(s: &str, limit: usize) -> Result<(), RuntimeError> {
    if s.len() > limit {
        Err(RuntimeError::StringTooLong {
            len: s.len(),
            limit,
        })
    } else {
        Ok(())
    }
}

fn checked_value_str(s: String, limit: usize) -> Result<Value, RuntimeError> {
    check_string_len(&s, limit)?;
    Ok(Value::Str(s))
}

fn checked_value_opt_str(s: Option<String>, limit: usize) -> Result<Value, RuntimeError> {
    match s {
        Some(v) => checked_value_str(v, limit),
        None => Ok(Value::Nil),
    }
}

fn api_error(function: &'static str, err: impl std::fmt::Display) -> RuntimeError {
    tracing::debug!("DSL builtin '{}' API detail: {}", function, err);
    RuntimeError::ApiError {
        function: function.into(),
        message: "plugin session operation failed".into(),
    }
}

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
                checked_value_opt_str(session.header_value(&name), self.limits.max_string_len)
            }
            BuiltinId::ReqMethod => {
                checked_value_str(session.get_method().to_string(), self.limits.max_string_len)
            }
            BuiltinId::ReqPath => {
                checked_value_str(session.get_path().to_string(), self.limits.max_string_len)
            }
            BuiltinId::ReqQuery => {
                let name = state.pop()?.into_string();
                checked_value_opt_str(session.get_query_param(&name), self.limits.max_string_len)
            }
            BuiltinId::ReqQueryString => {
                checked_value_opt_str(session.get_query(), self.limits.max_string_len)
            }
            BuiltinId::ReqCookie => {
                let name = state.pop()?.into_string();
                checked_value_opt_str(session.get_cookie(&name), self.limits.max_string_len)
            }
            BuiltinId::ReqClientIp => {
                checked_value_str(session.client_addr().to_string(), self.limits.max_string_len)
            }
            BuiltinId::ReqRemoteIp => {
                checked_value_str(session.remote_addr().to_string(), self.limits.max_string_len)
            }
            BuiltinId::ReqPathParam => {
                let name = state.pop()?.into_string();
                checked_value_opt_str(session.get_path_param(&name), self.limits.max_string_len)
            }
            BuiltinId::ReqHeaderNames => {
                let headers = session.request_headers();
                const MAX_HEADER_NAMES: usize = 256;
                let mut names: Vec<String> = Vec::new();
                for (k, _) in headers.into_iter().take(MAX_HEADER_NAMES) {
                    check_string_len(&k, self.limits.max_string_len)?;
                    names.push(k);
                }
                Ok(Value::List(names))
            }
            BuiltinId::ReqScheme => {
                // Detect scheme from common proxy headers, fallback to "http"
                let scheme = session
                    .header_value("X-Forwarded-Proto")
                    .or_else(|| session.header_value("X-Scheme"))
                    .unwrap_or_else(|| "http".to_string());
                checked_value_str(scheme, self.limits.max_string_len)
            }
            BuiltinId::ReqHost => {
                checked_value_opt_str(session.header_value("Host"), self.limits.max_string_len)
            }
            BuiltinId::ReqUri => {
                let path = session.get_path().to_string();
                match session.get_query() {
                    Some(q) if !q.is_empty() => {
                        let uri = format!("{}?{}", path, q);
                        check_string_len(&uri, self.limits.max_string_len)?;
                        Ok(Value::Str(uri))
                    }
                    _ => checked_value_str(path, self.limits.max_string_len),
                }
            }
            BuiltinId::ReqContentType => {
                checked_value_opt_str(session.header_value("Content-Type"), self.limits.max_string_len)
            }
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
                    .map_err(|e| api_error("req.set_header", e))?;
                Ok(Value::Nil)
            }
            BuiltinId::ReqAppendHeader => {
                let value = state.pop()?.into_string();
                let name = state.pop()?.into_string();
                session
                    .append_request_header(&name, &value)
                    .map_err(|e| api_error("req.append_header", e))?;
                Ok(Value::Nil)
            }
            BuiltinId::ReqRemoveHeader => {
                let name = state.pop()?.into_string();
                session
                    .remove_request_header(&name)
                    .map_err(|e| api_error("req.remove_header", e))?;
                Ok(Value::Nil)
            }
            BuiltinId::ReqSetUri => {
                let uri = state.pop()?.into_string();
                session
                    .set_upstream_uri(&uri)
                    .map_err(|e| api_error("req.set_uri", e))?;
                Ok(Value::Nil)
            }
            BuiltinId::ReqSetHost => {
                let host = state.pop()?.into_string();
                session
                    .set_upstream_host(&host)
                    .map_err(|e| api_error("req.set_host", e))?;
                Ok(Value::Nil)
            }
            BuiltinId::ReqSetMethod => {
                let method = state.pop()?.into_string();
                session
                    .set_upstream_method(&method)
                    .map_err(|e| api_error("req.set_method", e))?;
                Ok(Value::Nil)
            }

            // ===== resp.* =====
            BuiltinId::RespSetHeader => {
                let value = state.pop()?.into_string();
                let name = state.pop()?.into_string();
                session
                    .set_response_header(&name, &value)
                    .map_err(|e| api_error("resp.set_header", e))?;
                Ok(Value::Nil)
            }
            BuiltinId::RespAppendHeader => {
                let value = state.pop()?.into_string();
                let name = state.pop()?.into_string();
                session
                    .append_response_header(&name, &value)
                    .map_err(|e| api_error("resp.append_header", e))?;
                Ok(Value::Nil)
            }
            BuiltinId::RespRemoveHeader => {
                let name = state.pop()?.into_string();
                session
                    .remove_response_header(&name)
                    .map_err(|e| api_error("resp.remove_header", e))?;
                Ok(Value::Nil)
            }

            // ===== ctx.* =====
            BuiltinId::CtxGet => {
                let key = state.pop()?.into_string();
                checked_value_opt_str(session.get_ctx_var(&key), self.limits.max_string_len)
            }
            BuiltinId::CtxSet => {
                let value = state.pop()?.into_string();
                let key = state.pop()?.into_string();
                session
                    .set_ctx_var(&key, &value)
                    .map_err(|e| api_error("ctx.set", e))?;
                Ok(Value::Nil)
            }
            BuiltinId::CtxRemove => {
                let key = state.pop()?.into_string();
                session
                    .remove_ctx_var(&key)
                    .map_err(|e| api_error("ctx.remove", e))?;
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
                    Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
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
                let end = state.pop()?.as_int().unwrap_or(0).max(0) as usize;
                let start = state.pop()?.as_int().unwrap_or(0).max(0) as usize;
                let s = state.pop()?;
                match &s {
                    Value::Str(s) => {
                        // Use char_indices for UTF-8 safe slicing
                        let char_count = s.chars().count();
                        let start = start.min(char_count);
                        let end = end.min(char_count);
                        if start <= end {
                            let byte_start = s.char_indices().nth(start).map(|(i, _)| i).unwrap_or(s.len());
                            let byte_end = s.char_indices().nth(end).map(|(i, _)| i).unwrap_or(s.len());
                            let out = s[byte_start..byte_end].to_string();
                            check_string_len(&out, self.limits.max_string_len)?;
                            Ok(Value::Str(out))
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
                let s = v.into_string();
                check_string_len(&s, self.limits.max_string_len)?;
                Ok(Value::Str(s))
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
                let result = s.to_uppercase();
                check_string_len(&result, self.limits.max_string_len)?;
                Ok(Value::Str(result))
            }
            BuiltinId::ToLower => {
                let s = state.pop()?.into_string();
                let result = s.to_lowercase();
                check_string_len(&result, self.limits.max_string_len)?;
                Ok(Value::Str(result))
            }
            BuiltinId::Base64Encode => {
                use base64::Engine;
                let s = state.pop()?.into_string();
                let encoded = base64::engine::general_purpose::STANDARD.encode(s.as_bytes());
                check_string_len(&encoded, self.limits.max_string_len)?;
                Ok(Value::Str(encoded))
            }
            BuiltinId::Base64Decode => {
                use base64::Engine;
                let s = state.pop()?.into_string();
                match base64::engine::general_purpose::STANDARD.decode(s.as_bytes()) {
                    Ok(bytes) => {
                        let decoded = String::from_utf8_lossy(&bytes).to_string();
                        check_string_len(&decoded, self.limits.max_string_len)?;
                        Ok(Value::Str(decoded))
                    }
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
            BuiltinId::UrlEncode => {
                let s = state.pop()?.into_string();
                let encoded: String = s.chars().map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
                        c.to_string()
                    } else {
                        let mut buf = [0u8; 4];
                        let bytes = c.encode_utf8(&mut buf);
                        bytes.as_bytes().iter().map(|b| format!("%{:02X}", b)).collect()
                    }
                }).collect();
                check_string_len(&encoded, self.limits.max_string_len)?;
                Ok(Value::Str(encoded))
            }
            BuiltinId::UrlDecode => {
                let s = state.pop()?.into_string();
                let mut result = Vec::new();
                let bytes = s.as_bytes();
                let mut i = 0;
                while i < bytes.len() {
                    if bytes[i] == b'%' && i + 2 < bytes.len() {
                        // Safe: only attempt hex parse on ASCII bytes
                        let hi = bytes[i + 1];
                        let lo = bytes[i + 2];
                        if hi.is_ascii_hexdigit() && lo.is_ascii_hexdigit() {
                            // Both bytes are ASCII, safe to use from_str_radix on the str slice
                            if let Ok(byte) = u8::from_str_radix(
                                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""),
                                16,
                            ) {
                                result.push(byte);
                                i += 3;
                                continue;
                            }
                        }
                    } else if bytes[i] == b'+' {
                        // application/x-www-form-urlencoded: '+' → space
                        result.push(b' ');
                        i += 1;
                        continue;
                    }
                    result.push(bytes[i]);
                    i += 1;
                }
                let decoded = String::from_utf8_lossy(&result).to_string();
                check_string_len(&decoded, self.limits.max_string_len)?;
                Ok(Value::Str(decoded))
            }
            BuiltinId::Sha256 => {
                // Not implemented — return error so users know
                Err(RuntimeError::ApiError {
                    function: "sha256".into(),
                    message: "sha256() is not yet implemented".into(),
                })
            }
            BuiltinId::Md5 => {
                Err(RuntimeError::ApiError {
                    function: "md5".into(),
                    message: "md5() is not yet implemented".into(),
                })
            }
            BuiltinId::RegexFind => {
                Err(RuntimeError::ApiError {
                    function: "regex_find".into(),
                    message: "regex_find() is not yet implemented".into(),
                })
            }
            BuiltinId::RegexReplace => {
                Err(RuntimeError::ApiError {
                    function: "regex_replace".into(),
                    message: "regex_replace() is not yet implemented".into(),
                })
            }
            BuiltinId::Range => {
                Err(RuntimeError::ApiError {
                    function: "range".into(),
                    message: "range() is not yet implemented".into(),
                })
            }
        }
    }
}

