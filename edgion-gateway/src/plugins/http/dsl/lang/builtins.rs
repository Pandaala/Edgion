//! Built-in function dispatch — maps BuiltinId to PluginSession methods
//!
//! Arguments are popped from the VM stack, results pushed back.
//! Each builtin maps to exactly one PluginSession API call.

use futures::StreamExt;
use indexmap::IndexMap;
use std::sync::Arc;

use crate::link_sys::providers::redis::RedisLinkClient;
use crate::link_sys::runtime::store::get_redis_client;
use crate::plugins::runtime::{PluginLog, PluginSession};

use super::super::runtime::{dsl_http_client, DslRuntime};
use super::bytecode::BuiltinId;
use super::error::RuntimeError;
use super::value::Value;
use super::vm::{Vm, VmState};

/// Build a `Map { ok: true, value: <val>, error: nil }` success response for redis.* builtins.
fn redis_ok(value: Value) -> Value {
    let mut m = IndexMap::new();
    m.insert("ok".to_string(), Value::Bool(true));
    m.insert("value".to_string(), value);
    m.insert("error".to_string(), Value::Nil);
    Value::Map(Box::new(m))
}

/// Build a `Map { ok: false, value: nil, error: <msg> }` error response for redis.* builtins.
fn redis_err(msg: String) -> Value {
    let mut m = IndexMap::new();
    m.insert("ok".to_string(), Value::Bool(false));
    m.insert("value".to_string(), Value::Nil);
    m.insert("error".to_string(), Value::Str(msg.into()));
    Value::Map(Box::new(m))
}

/// Check that a string result doesn't exceed the VM's `max_string_len` limit.
///
/// The limit is enforced in **UTF-8 bytes** (matching the sandbox memory budget
/// documented on `VmLimits::max_string_len`). This is deliberately a different
/// unit from the DSL `len()` builtin, which returns Unicode character count;
/// scripts that need to predict this guard from inside the VM should account
/// for multi-byte characters. See the `len` entry in
/// `docs/.../dsl/api-reference.md` for the user-facing contract.
fn check_string_len(s: &str, limit: usize) -> Result<(), RuntimeError> {
    if s.len() > limit {
        Err(RuntimeError::StringTooLong { len: s.len(), limit })
    } else {
        Ok(())
    }
}

fn checked_value_str(s: String, limit: usize) -> Result<Value, RuntimeError> {
    check_string_len(&s, limit)?;
    Ok(Value::Str(s.into()))
}

/// Silently truncate `s` to at most `limit` UTF-8 bytes, walking back to the
/// nearest char boundary so `String::truncate` cannot panic on multi-byte
/// content. `is_char_boundary(0)` always succeeds, so the loop terminates.
///
/// Used by the `redis.*` read builtins (`get` / `hget` / `hgetall`) to restore
/// the sandbox invariant "no DSL string value exceeds max_string_len" for
/// externally-controlled Redis values. We *truncate* rather than raise
/// `StringTooLong` because the redis dispatcher carries the documented "never
/// raises a runtime error" contract (errors surface as a `redis_err` Map value,
/// not a propagated `RuntimeError`); this mirrors the `http.call` body cap
/// (FB-03). Without it an oversized Redis value (up to 512 MB) would flow into
/// `req.set_header` / `ctx.set` / `log` without further size gating.
fn truncate_to_max_string_len(mut s: String, limit: usize) -> String {
    if s.len() > limit {
        let mut end = limit;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

fn checked_value_opt_str(s: Option<String>, limit: usize) -> Result<Value, RuntimeError> {
    match s {
        Some(v) => checked_value_str(v, limit),
        None => Ok(Value::Nil),
    }
}

fn api_error(function: &'static str, err: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::ApiError {
        function: function.into(),
        message: format!("plugin session operation failed: {err}").into(),
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
                let mut names: Vec<Arc<str>> = Vec::new();
                for (k, _) in headers.into_iter().take(MAX_HEADER_NAMES) {
                    check_string_len(&k, self.limits.max_string_len)?;
                    names.push(Arc::from(k.as_str()));
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
                let mut result: Vec<Arc<str>> = Vec::new();
                for name in names.into_iter().take(MAX_HEADER_NAMES) {
                    check_string_len(&name, self.limits.max_string_len)?;
                    result.push(Arc::from(name.as_str()));
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
                    Value::Map(m) => Ok(Value::Int(m.len() as i64)),
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
                        // Single char_indices pass resolves both UTF-8 boundaries,
                        // avoiding three separate O(n) scans. Defaults of s.len() cover
                        // the "index >= char_count" case (matches the previous
                        // nth(...).unwrap_or(s.len()) fallback).
                        let mut byte_start = s.len();
                        let mut byte_end = s.len();
                        for (i, (byte_pos, _)) in s.char_indices().enumerate() {
                            if i == start {
                                byte_start = byte_pos;
                            }
                            if i == end {
                                byte_end = byte_pos;
                                break;
                            }
                        }
                        if byte_start <= byte_end {
                            let out = s[byte_start..byte_end].to_string();
                            check_string_len(&out, self.limits.max_string_len)?;
                            Ok(Value::Str(out.into()))
                        } else {
                            Ok(Value::Str(Arc::from("")))
                        }
                    }
                    Value::Nil => Ok(Value::Str(Arc::from(""))),
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
                Ok(Value::Str(s.into()))
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
                Ok(Value::Str(result.into()))
            }
            BuiltinId::ToLower => {
                let s = state.pop()?.into_string();
                let result = s.to_lowercase();
                check_string_len(&result, self.limits.max_string_len)?;
                Ok(Value::Str(result.into()))
            }
            BuiltinId::Base64Encode => {
                use base64::Engine;
                let s = state.pop()?.into_string();
                let encoded = base64::engine::general_purpose::STANDARD.encode(s.as_bytes());
                check_string_len(&encoded, self.limits.max_string_len)?;
                Ok(Value::Str(encoded.into()))
            }
            BuiltinId::Base64Decode => {
                use base64::Engine;
                let s = state.pop()?.into_string();
                match base64::engine::general_purpose::STANDARD.decode(s.as_bytes()) {
                    Ok(bytes) => {
                        let decoded = String::from_utf8_lossy(&bytes).to_string();
                        check_string_len(&decoded, self.limits.max_string_len)?;
                        Ok(Value::Str(decoded.into()))
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
                use std::fmt::Write as _;
                let s = state.pop()?.into_string();
                let limit = self.limits.max_string_len;
                // Build the encoded string incrementally and bail as soon as the running
                // length crosses `max_string_len`, instead of collecting the full result
                // and only then size-checking. Percent-encoding can expand each input byte
                // to 3 bytes ("%XX"), so the previous "encode-then-check" path allowed a
                // transient allocation of up to 3 * max_string_len before rejection; this
                // loop caps the transient at max_string_len + a few trailing bytes from
                // the partial char that crossed the limit. Legitimate inputs (those that
                // would have encoded within `limit`) remain unaffected — the early bail
                // only fires when the post-check would have rejected anyway.
                let mut encoded = String::with_capacity(s.len());
                for c in s.chars() {
                    if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
                        encoded.push(c);
                    } else {
                        let mut buf = [0u8; 4];
                        for b in c.encode_utf8(&mut buf).as_bytes() {
                            // write! into a String never fails
                            let _ = write!(encoded, "%{:02X}", b);
                        }
                    }
                    if encoded.len() > limit {
                        return Err(RuntimeError::StringTooLong {
                            len: encoded.len(),
                            limit,
                        });
                    }
                }
                Ok(Value::Str(encoded.into()))
            }
            // `url_decode(s)` uses `application/x-www-form-urlencoded` semantics: `%XX` is
            // decoded AND a literal `+` is decoded to a space. This is NOT RFC 3986 path
            // decoding (in RFC 3986 a literal `+` in a path is a reserved character and must
            // be preserved). The form-semantics choice intentionally mirrors
            // `edgion_resources::uri_utils::url_decode` (see `src/types/uri_utils.rs`) so the DSL
            // and the routing/consistent-hash layers agree on query-string decoding.
            //
            // Security note: callers must NOT pass `req.path()` here and then compare the
            // result against an ACL that contains a literal `+` — `%20` and `+` collapse to
            // the same byte, which is exploitable for allowlist bypass. The api-reference
            // doc carries the operator-facing warning; this comment is the in-source pointer
            // for future maintainers considering a rename / semantics change. See
            // `skills/04-review/dsl-builtins/url-decode-form-semantics.md`.
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
                        // application/x-www-form-urlencoded: '+' → space (see arm-level
                        // doc-comment above for the security implication)
                        result.push(b' ');
                        i += 1;
                        continue;
                    }
                    result.push(bytes[i]);
                    i += 1;
                }
                let decoded = String::from_utf8_lossy(&result).to_string();
                check_string_len(&decoded, self.limits.max_string_len)?;
                Ok(Value::Str(decoded.into()))
            }
            BuiltinId::Sha256 => {
                use sha2::Digest;
                let s = state.pop()?.into_string();
                let hash = sha2::Sha256::digest(s.as_bytes());
                checked_value_str(hex::encode(hash), self.limits.max_string_len)
            }
            BuiltinId::Md5 => {
                use md5::Digest;
                let s = state.pop()?.into_string();
                let hash = md5::Md5::digest(s.as_bytes());
                checked_value_str(hex::encode(hash), self.limits.max_string_len)
            }
            BuiltinId::Trim => {
                let s = state.pop()?.into_string();
                let trimmed = s.trim();
                if trimmed.len() == s.len() {
                    Ok(Value::Str(s.into()))
                } else {
                    Ok(Value::Str(Arc::from(trimmed)))
                }
            }
            BuiltinId::Split => {
                let delim = state.pop()?.into_string();
                let s = state.pop()?.into_string();
                let estimated = if delim.is_empty() {
                    // split("") inserts "" at both ends: N chars → N+2 parts
                    s.chars().count() + 2
                } else {
                    s.matches(&*delim).count() + 1
                };
                if estimated > self.limits.max_list_len {
                    return Err(RuntimeError::ListTooLong {
                        len: estimated,
                        limit: self.limits.max_list_len,
                    });
                }
                let parts: Vec<Arc<str>> = s.split(&*delim).map(Arc::from).collect();
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
                let diff = end
                    .checked_sub(start)
                    .ok_or(RuntimeError::IntegerOverflow { operation: "range" })?;
                // Honor configured max_list_len while keeping 1000 as a hard ceiling
                // so default config behavior is preserved.
                let hard_cap = self.limits.max_list_len.min(1000) as i64;
                let count = diff.max(0).min(hard_cap) as usize;
                let end_bound = start
                    .checked_add(count as i64)
                    .ok_or(RuntimeError::IntegerOverflow { operation: "range" })?;
                let list: Vec<Arc<str>> = (start..end_bound).map(|i| Arc::from(i.to_string().as_str())).collect();
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
                checked_value_str(result, self.limits.max_string_len)
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
                                    Ok(Value::Str(n.to_string().into()))
                                }
                            }
                            serde_json::Value::Bool(b) => Ok(Value::Bool(*b)),
                            serde_json::Value::Null => Ok(Value::Nil),
                            other => {
                                let s = other.to_string();
                                check_string_len(&s, self.limits.max_string_len)?;
                                Ok(Value::Str(s.into()))
                            }
                        }
                    }
                    Err(_) => Ok(Value::Nil),
                }
            }
            BuiltinId::Uuid => {
                let id = uuid::Uuid::new_v4().to_string();
                Ok(Value::Str(id.into()))
            }
            BuiltinId::HexEncode => {
                let s = state.pop()?.into_string();
                let encoded = hex::encode(s.as_bytes());
                check_string_len(&encoded, self.limits.max_string_len)?;
                Ok(Value::Str(encoded.into()))
            }
            BuiltinId::HexDecode => {
                let s = state.pop()?.into_string();
                match hex::decode(&s) {
                    Ok(bytes) => {
                        let decoded = String::from_utf8_lossy(&bytes).to_string();
                        check_string_len(&decoded, self.limits.max_string_len)?;
                        Ok(Value::Str(decoded.into()))
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
                        // chrono's DelayedFormat::fmt returns Err for invalid specifiers (e.g. "%Q");
                        // calling to_string() on it panics. Use write! to surface the Err safely.
                        use std::fmt::Write;
                        let mut formatted = String::new();
                        match write!(formatted, "{}", dt.format(&format)) {
                            Ok(()) => {
                                check_string_len(&formatted, self.limits.max_string_len)?;
                                Ok(Value::Str(formatted.into()))
                            }
                            Err(_) => Err(RuntimeError::ApiError {
                                function: "time_format".into(),
                                message: format!("invalid format string: {:?}", format),
                            }),
                        }
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
                        Ok(Value::Str(joined.into()))
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
                let log_args: Vec<crate::plugins::runtime::log::LogArg<'_>> = values
                    .iter()
                    .map(|s| crate::plugins::runtime::log::LogArg::Sensitive(s.as_str()))
                    .collect();
                log.push_fmt(&template, &log_args);
                Ok(Value::Nil)
            }

            // http.call is dispatched asynchronously by the VM before reaching call_builtin
            BuiltinId::HttpCall => Err(RuntimeError::Internal {
                message: "HttpCall must be dispatched via call_builtin_http, not call_builtin".into(),
            }),

            // redis.* builtins are dispatched asynchronously by the VM before reaching call_builtin
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
            | BuiltinId::RedisHExists => Err(RuntimeError::Internal {
                message: format!("{:?} must be dispatched via call_builtin_redis, not call_builtin", id),
            }),
        }
    }

    /// Async handler for http.call(method, url, body [, timeout_ms [, headers_map]]) → Map
    pub(crate) async fn call_builtin_http(
        &self,
        argc: u8,
        state: &mut VmState,
        runtime: &DslRuntime,
    ) -> Result<Value, RuntimeError> {
        if !(3..=5).contains(&(argc as usize)) {
            return Err(RuntimeError::ApiError {
                function: "http.call".into(),
                message: format!("http.call requires 3-5 arguments, got {}", argc),
            });
        }

        // Pop in reverse order (last arg pushed is on top of stack)
        let headers_val = if argc >= 5 { Some(state.pop()?) } else { None };
        let timeout_val = if argc >= 4 { Some(state.pop()?) } else { None };
        let body_val = state.pop()?;
        let url_val = state.pop()?;
        let method_val = state.pop()?;

        let method_str = method_val.into_string();
        let url_str = url_val.into_string();

        // Reject non-http/https schemes to prevent file://, ftp://, etc.
        let parsed_url = match reqwest::Url::parse(&url_str) {
            Ok(u) => u,
            Err(_) => {
                return Err(RuntimeError::ApiError {
                    function: "http.call".into(),
                    message: format!("invalid URL: {}", url_str),
                });
            }
        };
        if !matches!(parsed_url.scheme(), "http" | "https") {
            return Err(RuntimeError::ApiError {
                function: "http.call".into(),
                message: format!(
                    "http.call only supports http/https schemes, got: {}",
                    parsed_url.scheme()
                ),
            });
        }

        if runtime.block_private_ips {
            let raw = parsed_url.host_str().unwrap_or("");
            // url crate serializes IPv6 hosts with brackets (e.g. "[::1]") — strip them
            let host = raw.trim_matches(|c: char| c == '[' || c == ']');
            if is_ssrf_blocked_ip_literal(host) {
                return Err(RuntimeError::ApiError {
                    function: "http.call".into(),
                    message: format!("http.call blocked: private/loopback IP not allowed: {}", host),
                });
            }
        }

        let body_str = match body_val {
            Value::Nil => String::new(),
            other => other.into_string(),
        };

        let timeout_ms = match timeout_val {
            None | Some(Value::Nil) => runtime.max_timeout_ms,
            Some(Value::Int(n)) => (n as u64).min(runtime.max_timeout_ms),
            Some(other) => {
                return Err(RuntimeError::TypeError {
                    expected: "Int or Nil",
                    got: other.type_name(),
                    operation: "http.call timeout_ms",
                });
            }
        };

        let method = match method_str.to_uppercase().as_str() {
            "GET" => reqwest::Method::GET,
            "POST" => reqwest::Method::POST,
            "PUT" => reqwest::Method::PUT,
            "DELETE" => reqwest::Method::DELETE,
            "PATCH" => reqwest::Method::PATCH,
            "HEAD" => reqwest::Method::HEAD,
            "OPTIONS" => reqwest::Method::OPTIONS,
            _ => {
                return Err(RuntimeError::ApiError {
                    function: "http.call".into(),
                    message: format!("unsupported HTTP method: {}", method_str),
                });
            }
        };

        // When block_private_ips is set, select the guarded client whose redirect
        // policy re-applies is_ssrf_blocked_ip_literal to every hop (F-012).
        let client = dsl_http_client(runtime.block_private_ips);
        let mut builder = client
            .request(method, &url_str)
            .timeout(std::time::Duration::from_millis(timeout_ms));

        if !body_str.is_empty() {
            builder = builder.body(body_str);
        }

        if let Some(hv) = headers_val {
            match hv {
                Value::Map(m) => {
                    for (k, v) in m.iter() {
                        builder = builder.header(k.as_str(), v.as_str());
                    }
                }
                Value::Nil => {}
                other => {
                    return Err(RuntimeError::TypeError {
                        expected: "Map or Nil",
                        got: other.type_name(),
                        operation: "http.call headers_map",
                    });
                }
            }
        }

        let failure_map = || {
            let mut m = IndexMap::new();
            m.insert("status".to_string(), Value::Int(0));
            m.insert("body".to_string(), Value::Str(Arc::from("")));
            m.insert("headers".to_string(), Value::Map(Box::new(IndexMap::new())));
            Value::Map(Box::new(m))
        };

        let resp = match builder.send().await {
            Ok(r) => r,
            Err(_) => return Ok(failure_map()),
        };

        let status = resp.status().as_u16() as i64;

        // Silently truncate at max_map_size — response header count is controlled by the
        // upstream server (SSRF + large-response amplification vector), so cap rather than
        // raise. Matches the take(256) pattern used for ReqHeaderNames / RespHeaderNames.
        let mut resp_headers: IndexMap<String, Value> = IndexMap::new();
        for (k, v) in resp.headers() {
            if resp_headers.len() >= self.limits.max_map_size {
                break;
            }
            if let Ok(val) = v.to_str() {
                resp_headers.insert(k.as_str().to_lowercase(), Value::Str(Arc::from(val)));
            }
        }

        // Stream the response body, accumulating at most max_response_body_bytes and
        // discarding the rest. Buffering the whole body first (resp.bytes()) then
        // truncating let a slow/large upstream allocate the full body before the cap
        // applied (memory exhaustion / SSRF amplification, esp. with block_private_ips
        // false). Streaming bounds peak memory to the cap plus one chunk. A read error
        // yields failure_map(), preserving the prior all-or-nothing bytes() semantics.
        // Mirrors http_util::fetch_json_limited and webhook stream_body_capped.
        let max_body = runtime.max_response_body_bytes;
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::with_capacity(8192.min(max_body));
        loop {
            match stream.next().await {
                Some(Ok(chunk)) => {
                    let remaining = max_body - buf.len();
                    if chunk.len() >= remaining {
                        buf.extend_from_slice(&chunk[..remaining]);
                        break;
                    }
                    buf.extend_from_slice(&chunk);
                }
                Some(Err(_)) => return Ok(failure_map()),
                None => break,
            }
        }

        let mut body_text = String::from_utf8_lossy(&buf).to_string();
        // Restore the sandbox invariant "no DSL string value exceeds max_string_len"
        // for r["body"]: without this guard, a body up to max_response_body_bytes
        // (default 1 MB = 128x max_string_len) would flow into req.set_header /
        // resp.set_header / ctx.set / log without further size gating. We *truncate*
        // rather than raise StringTooLong to keep http.call's documented "never raises
        // a runtime error" contract and to stay consistent with the other size caps in
        // the same return value (body bytes capped silently at max_response_body_bytes,
        // headers map capped silently at max_map_size). Operators who need to see a
        // larger body must raise max_string_len. Walk back to a UTF-8 char boundary so
        // String::truncate cannot panic — String::from_utf8_lossy guarantees valid
        // UTF-8, so is_char_boundary(0) always succeeds and the loop terminates.
        if body_text.len() > self.limits.max_string_len {
            let mut end = self.limits.max_string_len;
            while !body_text.is_char_boundary(end) {
                end -= 1;
            }
            body_text.truncate(end);
        }

        let mut result = IndexMap::new();
        result.insert("status".to_string(), Value::Int(status));
        result.insert("body".to_string(), Value::Str(body_text.into()));
        result.insert("headers".to_string(), Value::Map(Box::new(resp_headers)));

        Ok(Value::Map(Box::new(result)))
    }
}

/// Resolve a Redis client from the runtime store using the link alias.
///
/// Looks up the alias in `runtime.redis_links` to get the "namespace/name" key,
/// then calls `get_redis_client` to retrieve the live client from the ArcSwap store.
/// Returns `Err(redis_err(...))` — i.e. `Ok(redis_err(...))` at the call site — on failure.
fn resolve_redis_client(link_name: &str, runtime: &DslRuntime) -> Result<Arc<RedisLinkClient>, Value> {
    let ns_name = match runtime.redis_links.get(link_name) {
        Some(k) => k.clone(),
        None => return Err(redis_err(format!("link '{}' not found in linkSysRefs", link_name))),
    };
    match get_redis_client(&ns_name) {
        Some(c) => Ok(c),
        None => Err(redis_err(format!("link '{}' not ready", link_name))),
    }
}

/// Async handler for all `redis.*` builtins.
///
/// Never panics, never raises a runtime error. Every call returns
/// `Map { ok: Bool, value: <type>, error: Str|Nil }` so script authors decide
/// how to handle failures (fail-open, fail-close, or graceful degradation).
///
/// Each builtin takes a link alias as its first argument. The client is resolved
/// per-call via `resolve_redis_client` which looks up the alias in `runtime.redis_links`.
impl Vm {
    pub(crate) async fn call_builtin_redis(
        &self,
        id: BuiltinId,
        argc: u8,
        state: &mut VmState,
        runtime: &DslRuntime,
    ) -> Result<Value, RuntimeError> {
        match id {
            // ===== Basic KV =====
            BuiltinId::RedisGet => {
                // redis.get(link, key)
                let key = state.pop()?.into_string();
                let link = state.pop()?.into_string();
                let client = match resolve_redis_client(&link, runtime) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client.get(&key).await {
                    Ok(Some(v)) => {
                        let v = truncate_to_max_string_len(v, self.limits.max_string_len);
                        Ok(redis_ok(Value::Str(v.into())))
                    }
                    Ok(None) => Ok(redis_ok(Value::Nil)),
                    Err(e) => Ok(redis_err(e.to_string())),
                }
            }
            BuiltinId::RedisSet => {
                // redis.set(link, key, value) or redis.set(link, key, value, ttl_ms)
                if !(3..=4).contains(&(argc as usize)) {
                    return Ok(redis_err(format!("redis.set requires 3 or 4 arguments, got {}", argc)));
                }
                let ttl_ms_val = if argc == 4 { Some(state.pop()?) } else { None };
                let value = state.pop()?.into_string();
                let key = state.pop()?.into_string();
                let link = state.pop()?.into_string();
                let client = match resolve_redis_client(&link, runtime) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                let result = match ttl_ms_val {
                    None => client.set(&key, &value, None).await,
                    Some(Value::Nil) => client.set(&key, &value, None).await,
                    Some(Value::Int(ms)) if ms > 0 => client.set_px(&key, &value, ms as u64).await,
                    Some(Value::Int(_)) => client.set(&key, &value, None).await,
                    Some(other) => {
                        return Ok(redis_err(format!(
                            "redis.set ttl_ms must be Int or Nil, got {}",
                            other.type_name()
                        )));
                    }
                };
                match result {
                    Ok(()) => Ok(redis_ok(Value::Bool(true))),
                    Err(e) => Ok(redis_err(e.to_string())),
                }
            }
            BuiltinId::RedisDel => {
                // redis.del(link, key)
                let key = state.pop()?.into_string();
                let link = state.pop()?.into_string();
                let client = match resolve_redis_client(&link, runtime) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client.del(&[key.as_str()]).await {
                    Ok(count) => Ok(redis_ok(Value::Bool(count > 0))),
                    Err(e) => Ok(redis_err(e.to_string())),
                }
            }
            BuiltinId::RedisExists => {
                // redis.exists(link, key)
                let key = state.pop()?.into_string();
                let link = state.pop()?.into_string();
                let client = match resolve_redis_client(&link, runtime) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client.exists(&key).await {
                    Ok(b) => Ok(redis_ok(Value::Bool(b))),
                    Err(e) => Ok(redis_err(e.to_string())),
                }
            }
            BuiltinId::RedisTtl => {
                // redis.ttl(link, key)
                let key = state.pop()?.into_string();
                let link = state.pop()?.into_string();
                let client = match resolve_redis_client(&link, runtime) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                // TTL: -1 = no expiry, -2 = key not found; ok=false only on connection error
                match client.ttl(&key).await {
                    Ok(v) => Ok(redis_ok(Value::Int(v))),
                    Err(e) => Ok(redis_err(e.to_string())),
                }
            }
            BuiltinId::RedisExpire => {
                // redis.expire(link, key, seconds)
                let seconds_val = state.pop()?;
                let key = state.pop()?.into_string();
                let link = state.pop()?.into_string();
                let seconds = match &seconds_val {
                    Value::Int(n) => *n,
                    other => {
                        return Ok(redis_err(format!(
                            "redis.expire seconds must be Int, got {}",
                            other.type_name()
                        )));
                    }
                };
                let client = match resolve_redis_client(&link, runtime) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client.expire(&key, seconds).await {
                    Ok(b) => Ok(redis_ok(Value::Bool(b))),
                    Err(e) => Ok(redis_err(e.to_string())),
                }
            }
            BuiltinId::RedisIncr => {
                // redis.incr(link, key)
                let key = state.pop()?.into_string();
                let link = state.pop()?.into_string();
                let client = match resolve_redis_client(&link, runtime) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client.incr(&key).await {
                    Ok(v) => Ok(redis_ok(Value::Int(v))),
                    Err(e) => Ok(redis_err(e.to_string())),
                }
            }
            BuiltinId::RedisIncrBy => {
                // redis.incr_by(link, key, delta)
                let delta_val = state.pop()?;
                let key = state.pop()?.into_string();
                let link = state.pop()?.into_string();
                let delta = match &delta_val {
                    Value::Int(n) => *n,
                    other => {
                        return Ok(redis_err(format!(
                            "redis.incr_by delta must be Int, got {}",
                            other.type_name()
                        )));
                    }
                };
                let client = match resolve_redis_client(&link, runtime) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client.incr_by(&key, delta).await {
                    Ok(v) => Ok(redis_ok(Value::Int(v))),
                    Err(e) => Ok(redis_err(e.to_string())),
                }
            }

            // ===== Hash =====
            BuiltinId::RedisHGet => {
                // redis.hget(link, key, field)
                let field = state.pop()?.into_string();
                let key = state.pop()?.into_string();
                let link = state.pop()?.into_string();
                let client = match resolve_redis_client(&link, runtime) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client.hget(&key, &field).await {
                    Ok(Some(v)) => {
                        let v = truncate_to_max_string_len(v, self.limits.max_string_len);
                        Ok(redis_ok(Value::Str(v.into())))
                    }
                    Ok(None) => Ok(redis_ok(Value::Nil)),
                    Err(e) => Ok(redis_err(e.to_string())),
                }
            }
            BuiltinId::RedisHSet => {
                // redis.hset(link, key, field, value)
                let val = state.pop()?.into_string();
                let field = state.pop()?.into_string();
                let key = state.pop()?.into_string();
                let link = state.pop()?.into_string();
                let client = match resolve_redis_client(&link, runtime) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client.hset(&key, &field, &val).await {
                    Ok(_count) => Ok(redis_ok(Value::Bool(true))),
                    Err(e) => Ok(redis_err(e.to_string())),
                }
            }
            BuiltinId::RedisHDel => {
                // redis.hdel(link, key, field)
                let field = state.pop()?.into_string();
                let key = state.pop()?.into_string();
                let link = state.pop()?.into_string();
                let client = match resolve_redis_client(&link, runtime) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client.hdel(&key, &[field.as_str()]).await {
                    Ok(count) => Ok(redis_ok(Value::Bool(count > 0))),
                    Err(e) => Ok(redis_err(e.to_string())),
                }
            }
            BuiltinId::RedisHGetAll => {
                // redis.hgetall(link, key)
                let key = state.pop()?.into_string();
                let link = state.pop()?.into_string();
                let client = match resolve_redis_client(&link, runtime) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client.hgetall(&key).await {
                    Ok(map) => {
                        if map.len() > self.limits.max_map_size {
                            return Ok(redis_err(format!(
                                "hgetall result too large: {} entries (limit {})",
                                map.len(),
                                self.limits.max_map_size
                            )));
                        }
                        let mut result: IndexMap<String, Value> = IndexMap::new();
                        for (k, v) in map {
                            // Both key and value are externally-controlled Redis
                            // strings; truncate each to keep the sandbox invariant.
                            let k = truncate_to_max_string_len(k, self.limits.max_string_len);
                            let v = truncate_to_max_string_len(v, self.limits.max_string_len);
                            result.insert(k, Value::Str(v.into()));
                        }
                        Ok(redis_ok(Value::Map(Box::new(result))))
                    }
                    Err(e) => Ok(redis_err(e.to_string())),
                }
            }
            BuiltinId::RedisHExists => {
                // redis.hexists(link, key, field)
                let field = state.pop()?.into_string();
                let key = state.pop()?.into_string();
                let link = state.pop()?.into_string();
                let client = match resolve_redis_client(&link, runtime) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client.hexists(&key, &field).await {
                    Ok(b) => Ok(redis_ok(Value::Bool(b))),
                    Err(e) => Ok(redis_err(e.to_string())),
                }
            }

            // All non-Redis builtins are dispatched via call_builtin / call_builtin_http and
            // must not reach here. Listed explicitly (no `_ =>` catch-all) so adding a new
            // BuiltinId variant fails to compile until it is routed to the right dispatcher.
            BuiltinId::ReqHeader
            | BuiltinId::ReqMethod
            | BuiltinId::ReqPath
            | BuiltinId::ReqQuery
            | BuiltinId::ReqQueryString
            | BuiltinId::ReqCookie
            | BuiltinId::ReqClientIp
            | BuiltinId::ReqRemoteIp
            | BuiltinId::ReqPathParam
            | BuiltinId::ReqHeaderNames
            | BuiltinId::ReqScheme
            | BuiltinId::ReqHost
            | BuiltinId::ReqUri
            | BuiltinId::ReqRequestPath
            | BuiltinId::ReqRequestUri
            | BuiltinId::ReqContentType
            | BuiltinId::ReqHasHeader
            | BuiltinId::ReqSetHeader
            | BuiltinId::ReqAppendHeader
            | BuiltinId::ReqRemoveHeader
            | BuiltinId::ReqSetUri
            | BuiltinId::ReqSetHost
            | BuiltinId::ReqSetMethod
            | BuiltinId::RespSetHeader
            | BuiltinId::RespAppendHeader
            | BuiltinId::RespRemoveHeader
            | BuiltinId::RespStatus
            | BuiltinId::RespHeader
            | BuiltinId::RespHasHeader
            | BuiltinId::ReqProtocol
            | BuiltinId::ReqGrpcService
            | BuiltinId::ReqGrpcMethod
            | BuiltinId::ReqContentLength
            | BuiltinId::ReqClientCertCn
            | BuiltinId::ReqClientCertFingerprint
            | BuiltinId::RespHeaderNames
            | BuiltinId::RespContentType
            | BuiltinId::RespSetStatus
            | BuiltinId::CtxGet
            | BuiltinId::CtxSet
            | BuiltinId::CtxRemove
            | BuiltinId::Log
            | BuiltinId::Len
            | BuiltinId::Substr
            | BuiltinId::ToInt
            | BuiltinId::ToStr
            | BuiltinId::ToUpper
            | BuiltinId::ToLower
            | BuiltinId::Base64Encode
            | BuiltinId::Base64Decode
            | BuiltinId::UrlEncode
            | BuiltinId::UrlDecode
            | BuiltinId::Sha256
            | BuiltinId::Md5
            | BuiltinId::TimeNow
            | BuiltinId::Range
            | BuiltinId::Trim
            | BuiltinId::Split
            | BuiltinId::HmacSha256
            | BuiltinId::JsonGet
            | BuiltinId::Uuid
            | BuiltinId::HexEncode
            | BuiltinId::HexDecode
            | BuiltinId::TimeFormat
            | BuiltinId::IpInCidr
            | BuiltinId::Join
            | BuiltinId::LogWarn
            | BuiltinId::LogError
            | BuiltinId::LogFmt
            | BuiltinId::HttpCall => Err(RuntimeError::Internal {
                message: format!("{:?} must not be dispatched via call_builtin_redis", id),
            }),
        }
    }
}

/// Return true if `host` is an IP literal that must be blocked to prevent SSRF
/// (loopback / link-local / RFC 1918 private / RFC 4193 ULA / unspecified).
/// Hostnames are NOT resolved here — callers rely on network-level egress controls for those.
/// IPv6 zone IDs are stripped per RFC 6874 before parsing.
///
/// Exposed `pub(crate)` so the guarded DSL HTTP client's redirect policy
/// (`runtime::dsl_http_client(true)`) re-applies the same IP-literal check to
/// every redirect hop — F-012's per-hop SSRF guard.
pub(crate) fn is_ssrf_blocked_ip_literal(host: &str) -> bool {
    use std::net::IpAddr;
    // Strip IPv6 zone IDs (RFC 6874: "::1%eth0")
    let host = host.split('%').next().unwrap_or(host);
    let ip: IpAddr = match host.parse() {
        Ok(ip) => ip,
        Err(_) => return false, // hostname, not an IP literal
    };
    match ip {
        IpAddr::V4(v4) => is_private_v4(v4),
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() {
                return true;
            }
            // IPv4-mapped IPv6 (::ffff:a.b.c.d) — check the embedded IPv4
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_v4(v4);
            }
            let s = v6.segments();
            // fc00::/7 (unique local) or fe80::/10 (link-local)
            (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80
        }
    }
}

#[inline]
fn is_private_v4(v4: std::net::Ipv4Addr) -> bool {
    v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()
}

#[cfg(test)]
mod ip_tests {
    use super::is_ssrf_blocked_ip_literal;

    #[test]
    fn test_ipv4_private_ranges() {
        assert!(is_ssrf_blocked_ip_literal("127.0.0.1"));
        assert!(is_ssrf_blocked_ip_literal("10.0.0.1"));
        assert!(is_ssrf_blocked_ip_literal("172.16.0.1"));
        assert!(is_ssrf_blocked_ip_literal("172.31.255.255"));
        assert!(is_ssrf_blocked_ip_literal("192.168.1.1"));
        assert!(is_ssrf_blocked_ip_literal("169.254.1.1")); // link-local
        assert!(is_ssrf_blocked_ip_literal("0.0.0.0")); // unspecified
    }

    #[test]
    fn test_ipv4_public() {
        assert!(!is_ssrf_blocked_ip_literal("8.8.8.8"));
        assert!(!is_ssrf_blocked_ip_literal("1.1.1.1"));
        assert!(!is_ssrf_blocked_ip_literal("203.0.113.1"));
    }

    #[test]
    fn test_ipv6_private() {
        assert!(is_ssrf_blocked_ip_literal("::1")); // loopback
        assert!(is_ssrf_blocked_ip_literal("fc00::1")); // unique local
        assert!(is_ssrf_blocked_ip_literal("fd12:3456::1")); // unique local
        assert!(is_ssrf_blocked_ip_literal("fe80::1")); // link-local
    }

    #[test]
    fn test_ipv4_mapped_ipv6() {
        assert!(is_ssrf_blocked_ip_literal("::ffff:192.168.1.1"));
        assert!(is_ssrf_blocked_ip_literal("::ffff:127.0.0.1"));
        assert!(is_ssrf_blocked_ip_literal("::ffff:10.0.0.1"));
        assert!(!is_ssrf_blocked_ip_literal("::ffff:8.8.8.8")); // public
    }

    #[test]
    fn test_ipv6_zone_id_stripped() {
        assert!(is_ssrf_blocked_ip_literal("fe80::1%eth0")); // link-local with zone ID
        assert!(is_ssrf_blocked_ip_literal("::1%lo")); // loopback with zone ID
    }

    #[test]
    fn test_ipv6_public() {
        assert!(!is_ssrf_blocked_ip_literal("2001:db8::1"));
        assert!(!is_ssrf_blocked_ip_literal("2606:4700::1111"));
    }

    #[test]
    fn test_hostnames_not_blocked() {
        // Hostnames are not resolved — always returns false
        assert!(!is_ssrf_blocked_ip_literal("localhost"));
        assert!(!is_ssrf_blocked_ip_literal("example.com"));
        assert!(!is_ssrf_blocked_ip_literal("internal.corp"));
    }

    #[test]
    fn test_reqwest_url_host_str_brackets_stripped() {
        // url crate returns "[::ffff:c0a8:101]" (with brackets) for IPv6 hosts;
        // the call site strips brackets before calling is_ssrf_blocked_ip_literal.
        let url = reqwest::Url::parse("http://[::ffff:192.168.1.1]:80").unwrap();
        let raw = url.host_str().unwrap_or("");
        let host = raw.trim_matches(|c: char| c == '[' || c == ']');
        assert!(
            is_ssrf_blocked_ip_literal(host),
            "IPv4-mapped IPv6 not detected as private: {:?}",
            host
        );
    }
}

#[cfg(test)]
mod truncate_tests {
    use super::truncate_to_max_string_len;

    #[test]
    fn under_limit_is_unchanged() {
        assert_eq!(truncate_to_max_string_len("abc".to_string(), 8), "abc");
        assert_eq!(truncate_to_max_string_len("abcdefgh".to_string(), 8), "abcdefgh");
    }

    #[test]
    fn over_limit_truncated_to_byte_len() {
        assert_eq!(truncate_to_max_string_len("abcdefghij".to_string(), 4), "abcd");
        // limit 0 yields the empty string.
        assert_eq!(truncate_to_max_string_len("abc".to_string(), 0), "");
    }

    #[test]
    fn truncation_walks_back_to_char_boundary() {
        // "é" is 2 UTF-8 bytes (0xC3 0xA9). Byte layout of "aéé": a@0, é@1..3, é@3..5.
        // Char boundaries are at {0, 1, 3, 5}.
        let s = "aéé".to_string();
        // limit 2 splits the first "é" mid-codepoint; walk back to boundary 1 ("a").
        assert_eq!(truncate_to_max_string_len(s.clone(), 2), "a");
        // limit 3 already lands on a boundary; no walk-back, keep "aé".
        let out = truncate_to_max_string_len(s, 3);
        assert_eq!(out, "aé");
        assert_eq!(out.len(), 3);
    }
}
