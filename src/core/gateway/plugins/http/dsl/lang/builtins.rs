//! Built-in function dispatch — maps BuiltinId to PluginSession methods
//!
//! Arguments are popped from the VM stack, results pushed back.
//! Each builtin maps to exactly one PluginSession API call.

use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession};

use super::bytecode::BuiltinId;
use super::error::RuntimeError;
use super::value::Value;
use super::vm::{Vm, VmState};

/// Check that a string result doesn't exceed the VM's max_string_len limit.
fn check_string_len(s: &str, limit: usize) -> Result<(), RuntimeError> {
    if s.len() > limit {
        Err(RuntimeError::StringTooLong { len: s.len(), limit })
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

fn api_error(function: &'static str, _err: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::ApiError {
        function: function.into(),
        message: "plugin session operation failed".into(),
    }
}

impl Vm {
    pub(crate) fn call_builtin(
        &self,
        id: BuiltinId,
        argc: u8,
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
            BuiltinId::ReqMethod => checked_value_str(session.get_method().to_string(), self.limits.max_string_len),
            BuiltinId::ReqPath => checked_value_str(session.get_path().to_string(), self.limits.max_string_len),
            BuiltinId::ReqQuery => {
                let name = state.pop()?.into_string();
                checked_value_opt_str(session.get_query_param(&name), self.limits.max_string_len)
            }
            BuiltinId::ReqQueryString => checked_value_opt_str(session.get_query(), self.limits.max_string_len),
            BuiltinId::ReqCookie => {
                let name = state.pop()?.into_string();
                checked_value_opt_str(session.get_cookie(&name), self.limits.max_string_len)
            }
            BuiltinId::ReqClientIp => checked_value_str(session.client_addr().to_string(), self.limits.max_string_len),
            BuiltinId::ReqRemoteIp => checked_value_str(session.remote_addr().to_string(), self.limits.max_string_len),
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
            BuiltinId::ReqHost => checked_value_opt_str(session.header_value("Host"), self.limits.max_string_len),
            BuiltinId::ReqUri => checked_value_str(session.get_uri(), self.limits.max_string_len),
            BuiltinId::ReqRequestPath => {
                checked_value_str(session.get_request_path().to_string(), self.limits.max_string_len)
            }
            BuiltinId::ReqRequestUri => checked_value_str(session.get_request_uri(), self.limits.max_string_len),
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

            // ===== resp.* read =====
            BuiltinId::RespStatus => match session.get_response_status() {
                Some(s) => Ok(Value::Int(s as i64)),
                None => Ok(Value::Nil),
            },
            BuiltinId::RespHeader => {
                let name = state.pop()?.into_string();
                checked_value_opt_str(session.get_response_header(&name), self.limits.max_string_len)
            }
            BuiltinId::RespHasHeader => {
                let name = state.pop()?.into_string();
                Ok(Value::Bool(session.get_response_header(&name).is_some()))
            }

            // ===== req.* metadata (P1) =====
            BuiltinId::ReqProtocol => checked_value_str(session.discover_protocol(), self.limits.max_string_len),
            BuiltinId::ReqGrpcService => checked_value_opt_str(session.grpc_service(), self.limits.max_string_len),
            BuiltinId::ReqGrpcMethod => checked_value_opt_str(session.grpc_method(), self.limits.max_string_len),
            BuiltinId::ReqContentLength => match session.header_value("Content-Length") {
                Some(v) => match v.parse::<i64>() {
                    Ok(n) => Ok(Value::Int(n)),
                    Err(_) => Ok(Value::Nil),
                },
                None => Ok(Value::Nil),
            },
            BuiltinId::ReqClientCertCn => {
                let cn = session.client_cert_info().and_then(|c| c.cn.clone());
                checked_value_opt_str(cn, self.limits.max_string_len)
            }
            BuiltinId::ReqClientCertFingerprint => {
                let fp = session.client_cert_info().map(|c| c.fingerprint.clone());
                checked_value_opt_str(fp, self.limits.max_string_len)
            }

            // ===== resp.* read (P2) =====
            BuiltinId::RespHeaderNames => {
                // Collect response header names — requires response_header to be available
                // Uses get_response_header pattern: if response_header is None, returns empty list
                // We don't have a dedicated method, so we check known standard headers
                // Actually, we need a new PluginSession method for this.
                // For now, return empty list if response headers are not accessible.
                let names = session.response_header_names();
                const MAX_HEADER_NAMES: usize = 256;
                let mut result: Vec<String> = Vec::new();
                for name in names.into_iter().take(MAX_HEADER_NAMES) {
                    check_string_len(&name, self.limits.max_string_len)?;
                    result.push(name);
                }
                Ok(Value::List(result))
            }
            BuiltinId::RespContentType => {
                checked_value_opt_str(session.get_response_header("Content-Type"), self.limits.max_string_len)
            }

            // ===== resp.* set status =====
            BuiltinId::RespSetStatus => {
                let code_val = state.pop()?;
                let code = match &code_val {
                    Value::Int(n) => {
                        let c = *n;
                        if !(100..=599).contains(&c) {
                            return Err(RuntimeError::ApiError {
                                function: "resp.set_status".into(),
                                message: format!("invalid status code: {} (must be 100-599)", c),
                            });
                        }
                        c as u16
                    }
                    _ => {
                        return Err(RuntimeError::TypeError {
                            expected: "Int",
                            got: code_val.type_name(),
                            operation: "resp.set_status()",
                        });
                    }
                };
                session
                    .set_response_status(code)
                    .map_err(|e| api_error("resp.set_status", e))?;
                Ok(Value::Nil)
            }

            // ===== resp.* mutation =====
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
                session.set_ctx_var(&key, &value).map_err(|e| api_error("ctx.set", e))?;
                Ok(Value::Nil)
            }
            BuiltinId::CtxRemove => {
                let key = state.pop()?.into_string();
                session.remove_ctx_var(&key).map_err(|e| api_error("ctx.remove", e))?;
                Ok(Value::Nil)
            }

            // ===== Utilities =====
            BuiltinId::Log => {
                let msg = state.pop()?.into_string();
                log.push_sensitive(&msg);
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
                let end_val = state.pop()?;
                let end = match &end_val {
                    Value::Int(n) => (*n).max(0) as usize,
                    Value::Nil => 0,
                    _ => {
                        return Err(RuntimeError::TypeError {
                            expected: "Int",
                            got: end_val.type_name(),
                            operation: "substr()",
                        })
                    }
                };
                let start_val = state.pop()?;
                let start = match &start_val {
                    Value::Int(n) => (*n).max(0) as usize,
                    Value::Nil => 0,
                    _ => {
                        return Err(RuntimeError::TypeError {
                            expected: "Int",
                            got: start_val.type_name(),
                            operation: "substr()",
                        })
                    }
                };
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
                let encoded: String = s
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
                            c.to_string()
                        } else {
                            let mut buf = [0u8; 4];
                            let bytes = c.encode_utf8(&mut buf);
                            bytes.as_bytes().iter().map(|b| format!("%{:02X}", b)).collect()
                        }
                    })
                    .collect();
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
                            if let Ok(byte) =
                                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
                            {
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
                use sha2::Digest;
                let s = state.pop()?.into_string();
                let hash = sha2::Sha256::digest(s.as_bytes());
                Ok(Value::Str(hex::encode(hash)))
            }
            BuiltinId::Md5 => {
                use md5::Digest;
                let s = state.pop()?.into_string();
                let hash = md5::Md5::digest(s.as_bytes());
                Ok(Value::Str(hex::encode(hash)))
            }
            BuiltinId::RegexFind => {
                let pattern = state.pop()?.into_string();
                let s = state.pop()?.into_string();
                if pattern.len() > 1024 {
                    return Err(RuntimeError::RegexError {
                        message: "pattern too long (max 1024 bytes)".into(),
                    });
                }
                let re =
                    regex::Regex::new(&pattern).map_err(|e| RuntimeError::RegexError { message: e.to_string() })?;
                match re.find(&s) {
                    Some(m) => {
                        let found = m.as_str().to_string();
                        check_string_len(&found, self.limits.max_string_len)?;
                        Ok(Value::Str(found))
                    }
                    None => Ok(Value::Nil),
                }
            }
            BuiltinId::RegexReplace => {
                let replacement = state.pop()?.into_string();
                let pattern = state.pop()?.into_string();
                let s = state.pop()?.into_string();
                if pattern.len() > 1024 {
                    return Err(RuntimeError::RegexError {
                        message: "pattern too long (max 1024 bytes)".into(),
                    });
                }
                let re =
                    regex::Regex::new(&pattern).map_err(|e| RuntimeError::RegexError { message: e.to_string() })?;
                let result = re.replace_all(&s, replacement.as_str()).into_owned();
                check_string_len(&result, self.limits.max_string_len)?;
                Ok(Value::Str(result))
            }
            BuiltinId::Trim => {
                let s = state.pop()?.into_string();
                Ok(Value::Str(s.trim().to_string()))
            }
            BuiltinId::Split => {
                let delim = state.pop()?.into_string();
                let s = state.pop()?.into_string();
                let parts: Vec<String> = s.split(&*delim).map(|p| p.to_string()).collect();
                Ok(Value::List(parts))
            }
            BuiltinId::Range => {
                let end_val = state.pop()?;
                let end = match &end_val {
                    Value::Int(n) => *n,
                    Value::Nil => 0,
                    _ => {
                        return Err(RuntimeError::TypeError {
                            expected: "Int",
                            got: end_val.type_name(),
                            operation: "range()",
                        })
                    }
                };
                let start_val = state.pop()?;
                let start = match &start_val {
                    Value::Int(n) => *n,
                    Value::Nil => 0,
                    _ => {
                        return Err(RuntimeError::TypeError {
                            expected: "Int",
                            got: start_val.type_name(),
                            operation: "range()",
                        })
                    }
                };
                let count = (end - start).max(0).min(1000) as usize;
                let list: Vec<String> = (start..start + count as i64).map(|i| i.to_string()).collect();
                Ok(Value::List(list))
            }

            // ===== P3 Utility Functions =====
            BuiltinId::HmacSha256 => {
                use hmac::{Hmac, Mac};
                type HmacSha256 = Hmac<sha2::Sha256>;

                let data = state.pop()?.into_string();
                let key = state.pop()?.into_string();
                let mut mac = HmacSha256::new_from_slice(key.as_bytes()).map_err(|e| RuntimeError::ApiError {
                    function: "hmac_sha256".into(),
                    message: format!("invalid key: {}", e),
                })?;
                mac.update(data.as_bytes());
                let result = hex::encode(mac.finalize().into_bytes());
                Ok(Value::Str(result))
            }
            BuiltinId::JsonGet => {
                let path = state.pop()?.into_string();
                let json_str = state.pop()?.into_string();
                // Limit JSON parse size to prevent abuse
                if json_str.len() > self.limits.max_string_len {
                    return Err(RuntimeError::StringTooLong {
                        len: json_str.len(),
                        limit: self.limits.max_string_len,
                    });
                }
                match serde_json::from_str::<serde_json::Value>(&json_str) {
                    Ok(val) => {
                        // Simple dot-notation path traversal with depth limit
                        let mut current = &val;
                        let segments: Vec<&str> = path.split('.').collect();
                        if segments.len() > 32 {
                            return Err(RuntimeError::ApiError {
                                function: "json_get".into(),
                                message: "path too deep (max 32 segments)".into(),
                            });
                        }
                        for key in &segments {
                            match current {
                                serde_json::Value::Object(map) => {
                                    current = match map.get(*key) {
                                        Some(v) => v,
                                        None => return Ok(Value::Nil),
                                    };
                                }
                                serde_json::Value::Array(arr) => {
                                    if let Ok(idx) = key.parse::<usize>() {
                                        current = match arr.get(idx) {
                                            Some(v) => v,
                                            None => return Ok(Value::Nil),
                                        };
                                    } else {
                                        return Ok(Value::Nil);
                                    }
                                }
                                _ => return Ok(Value::Nil),
                            }
                        }
                        match current {
                            serde_json::Value::String(s) => checked_value_str(s.clone(), self.limits.max_string_len),
                            serde_json::Value::Number(n) => {
                                if let Some(i) = n.as_i64() {
                                    Ok(Value::Int(i))
                                } else {
                                    Ok(Value::Str(n.to_string()))
                                }
                            }
                            serde_json::Value::Bool(b) => Ok(Value::Bool(*b)),
                            serde_json::Value::Null => Ok(Value::Nil),
                            other => {
                                let s = other.to_string();
                                check_string_len(&s, self.limits.max_string_len)?;
                                Ok(Value::Str(s))
                            }
                        }
                    }
                    Err(_) => Ok(Value::Nil),
                }
            }
            BuiltinId::Uuid => {
                let id = uuid::Uuid::new_v4().to_string();
                Ok(Value::Str(id))
            }
            BuiltinId::HexEncode => {
                let s = state.pop()?.into_string();
                let encoded = hex::encode(s.as_bytes());
                check_string_len(&encoded, self.limits.max_string_len)?;
                Ok(Value::Str(encoded))
            }
            BuiltinId::HexDecode => {
                let s = state.pop()?.into_string();
                match hex::decode(&s) {
                    Ok(bytes) => {
                        let decoded = String::from_utf8_lossy(&bytes).to_string();
                        check_string_len(&decoded, self.limits.max_string_len)?;
                        Ok(Value::Str(decoded))
                    }
                    Err(_) => Ok(Value::Nil),
                }
            }
            BuiltinId::TimeFormat => {
                let format = state.pop()?.into_string();
                if format.len() > 256 {
                    return Err(RuntimeError::ApiError {
                        function: "time_format".into(),
                        message: format!("format string too long: {} (max 256)", format.len()),
                    });
                }
                let ts_val = state.pop()?;
                let timestamp = match &ts_val {
                    Value::Int(n) => *n,
                    Value::Nil => return Ok(Value::Nil),
                    _ => {
                        return Err(RuntimeError::TypeError {
                            expected: "Int",
                            got: ts_val.type_name(),
                            operation: "time_format()",
                        })
                    }
                };
                let dt = chrono::DateTime::from_timestamp(timestamp, 0);
                match dt {
                    Some(dt) => {
                        let formatted = dt.format(&format).to_string();
                        check_string_len(&formatted, self.limits.max_string_len)?;
                        Ok(Value::Str(formatted))
                    }
                    None => Ok(Value::Nil),
                }
            }
            BuiltinId::IpInCidr => {
                let cidr_str = state.pop()?.into_string();
                let ip_str = state.pop()?.into_string();
                let ip: std::net::IpAddr = match ip_str.parse() {
                    Ok(ip) => ip,
                    Err(_) => return Ok(Value::Bool(false)),
                };
                // Parse CIDR manually: "10.0.0.0/8"
                let parts: Vec<&str> = cidr_str.splitn(2, '/').collect();
                if parts.len() != 2 {
                    return Ok(Value::Bool(false));
                }
                let network_ip: std::net::IpAddr = match parts[0].parse() {
                    Ok(ip) => ip,
                    Err(_) => return Ok(Value::Bool(false)),
                };
                let prefix_len: u8 = match parts[1].parse() {
                    Ok(p) => p,
                    Err(_) => return Ok(Value::Bool(false)),
                };
                let result = match (ip, network_ip) {
                    (std::net::IpAddr::V4(ip), std::net::IpAddr::V4(net)) => {
                        if prefix_len > 32 {
                            false
                        } else if prefix_len == 0 {
                            true
                        } else {
                            let mask = u32::MAX.checked_shl(32 - prefix_len as u32).unwrap_or(0);
                            (u32::from(ip) & mask) == (u32::from(net) & mask)
                        }
                    }
                    (std::net::IpAddr::V6(ip), std::net::IpAddr::V6(net)) => {
                        if prefix_len > 128 {
                            false
                        } else if prefix_len == 0 {
                            true
                        } else {
                            let mask = u128::MAX.checked_shl(128 - prefix_len as u32).unwrap_or(0);
                            (u128::from(ip) & mask) == (u128::from(net) & mask)
                        }
                    }
                    _ => false, // IPv4 vs IPv6 mismatch
                };
                Ok(Value::Bool(result))
            }
            BuiltinId::Join => {
                let delim = state.pop()?.into_string();
                let list = state.pop()?;
                match list {
                    Value::List(items) => {
                        let joined = items.join(&delim);
                        check_string_len(&joined, self.limits.max_string_len)?;
                        Ok(Value::Str(joined))
                    }
                    _ => Err(RuntimeError::TypeError {
                        expected: "List",
                        got: list.type_name(),
                        operation: "join()",
                    }),
                }
            }
            BuiltinId::LogWarn => {
                let value = state.pop()?.into_string();
                log.push_sensitive(&value);
                Ok(Value::Nil)
            }
            BuiltinId::LogError => {
                let value = state.pop()?.into_string();
                log.push_err_sensitive(&value);
                Ok(Value::Nil)
            }
            BuiltinId::LogFmt => {
                // Stack (top → bottom): valueN, valueN-1, ..., value1, template
                // argc = 1 (template) + N (values).
                let total = argc as usize;
                if total == 0 {
                    return Err(RuntimeError::ApiError {
                        function: "log_fmt".into(),
                        message: "log_fmt requires at least the template argument".into(),
                    });
                }
                let value_count = total - 1;
                let mut values: Vec<String> = Vec::with_capacity(value_count);
                for _ in 0..value_count {
                    values.push(state.pop()?.into_string());
                }
                values.reverse();
                let template = state.pop()?.into_string();
                let log_args: Vec<crate::core::gateway::plugins::runtime::log::LogArg<'_>> = values
                    .iter()
                    .map(|s| crate::core::gateway::plugins::runtime::log::LogArg::Sensitive(s.as_str()))
                    .collect();
                log.push_fmt(&template, &log_args);
                Ok(Value::Nil)
            }
        }
    }
}
