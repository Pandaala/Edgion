//! Built-in function dispatch — maps BuiltinId to PluginSession methods
//!
//! Arguments are popped from the VM stack, results pushed back.
//! Each builtin maps to exactly one PluginSession API call.

use bytes::Bytes;
use futures::StreamExt;
use indexmap::IndexMap;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

use crate::link_sys::providers::redis::RedisLinkClient;
use crate::link_sys::runtime::store::get_redis_client;
use crate::plugins::runtime::traits::session::{BodyReadError, RequestBodySnapshotReader};
use crate::plugins::runtime::{PluginLog, PluginSession};

use super::super::runtime::{dsl_http_client, DslRuntime};
use super::bytecode::BuiltinId;
use super::error::RuntimeError;
use super::value::{BodyValue, Value};
use super::vm::{body_access_error, RuntimeString, Vm, VmState};

struct OpaqueBodyUpload {
    body: reqwest::Body,
    tracking: Arc<OpaqueBodyUploadState>,
}

struct OpaqueBodyUploadState {
    len: u64,
    fetched: AtomicU64,
    read_error: Mutex<Option<BodyReadError>>,
    cancelled: AtomicBool,
    ambiguous_read_cancelled: AtomicBool,
    reader: Mutex<OpaqueBodyReaderState>,
    state_changed: Notify,
}

struct OpaqueBodyReaderState {
    reader: Option<Box<dyn RequestBodySnapshotReader>>,
    in_flight: bool,
    finished: bool,
    abort_handle: Option<tokio::task::AbortHandle>,
}

struct AbortTaskOnDrop<T> {
    task: Option<tokio::task::JoinHandle<T>>,
}

impl<T> AbortTaskOnDrop<T> {
    fn new(task: tokio::task::JoinHandle<T>) -> Self {
        Self { task: Some(task) }
    }

    fn abort_handle(&self) -> tokio::task::AbortHandle {
        self.task.as_ref().expect("task is present").abort_handle()
    }

    async fn join(mut self) -> Result<T, tokio::task::JoinError> {
        let result = self.task.as_mut().expect("task is present").await;
        self.task.take();
        result
    }
}

impl<T> Drop for AbortTaskOnDrop<T> {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

impl OpaqueBodyUploadState {
    async fn cancel_and_release_reader(&self) {
        self.cancelled.store(true, Ordering::Release);

        loop {
            let changed = self.state_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let settled_reader = {
                let mut state = self.reader.lock();
                if state.in_flight {
                    if let Some(handle) = &state.abort_handle {
                        handle.abort();
                    }
                    None
                } else {
                    state.finished = true;
                    Some(state.reader.take())
                }
            };
            if let Some(reader) = settled_reader {
                drop(reader);
                return;
            }
            changed.as_mut().await;
        }
    }
}

struct InFlightBodyReader {
    shared: Arc<OpaqueBodyUploadState>,
    reader: Option<Box<dyn RequestBodySnapshotReader>>,
    settled: bool,
}

impl InFlightBodyReader {
    fn reader_mut(&mut self) -> &mut dyn RequestBodySnapshotReader {
        self.reader.as_deref_mut().expect("in-flight reader is present")
    }

    fn return_to_slot(mut self) {
        let mut state = self.shared.reader.lock();
        state.reader = self.reader.take();
        state.in_flight = false;
        state.abort_handle = None;
        self.settled = true;
        drop(state);
        self.shared.state_changed.notify_waiters();
    }

    fn finish(mut self) {
        let reader = self.reader.take();
        drop(reader);
        let mut state = self.shared.reader.lock();
        state.in_flight = false;
        state.finished = true;
        state.abort_handle = None;
        self.settled = true;
        drop(state);
        self.shared.state_changed.notify_waiters();
    }
}

impl Drop for InFlightBodyReader {
    fn drop(&mut self) {
        if self.settled {
            return;
        }

        let reader = self.reader.take();
        drop(reader);
        self.shared.ambiguous_read_cancelled.store(true, Ordering::Release);
        let mut state = self.shared.reader.lock();
        state.in_flight = false;
        state.finished = true;
        state.abort_handle = None;
        drop(state);
        self.shared.state_changed.notify_waiters();
    }
}

async fn read_opaque_body_chunk_task(mut in_flight: InFlightBodyReader) -> Result<Option<Bytes>, BodyReadError> {
    let shared = Arc::clone(&in_flight.shared);
    let read_result = in_flight.reader_mut().next_chunk().await;

    let mut output = None;
    let mut stream_error = None;
    match read_result {
        Ok(Some(chunk)) => {
            let chunk_len = if chunk.is_empty() {
                None
            } else {
                u64::try_from(chunk.len()).ok()
            };
            match chunk_len
                .and_then(|chunk_len| shared.fetched.load(Ordering::Relaxed).checked_add(chunk_len))
                .filter(|total| *total <= shared.len)
            {
                Some(total) => {
                    shared.fetched.store(total, Ordering::Relaxed);
                    output = Some(chunk);
                }
                None => stream_error = Some(BodyReadError::Incomplete),
            }
        }
        Ok(None) => {
            if shared.fetched.load(Ordering::Relaxed) != shared.len {
                stream_error = Some(BodyReadError::Incomplete);
            }
        }
        Err(error) => stream_error = Some(error),
    }

    if let Some(error) = &stream_error {
        *shared.read_error.lock() = Some(error.clone());
    }
    let was_cancelled = shared.cancelled.load(Ordering::Acquire);
    if was_cancelled || output.is_none() {
        in_flight.finish();
    } else {
        in_flight.return_to_slot();
    }

    if let Some(error) = stream_error {
        return Err(error);
    }
    if was_cancelled {
        return Ok(None);
    }
    Ok(output)
}

async fn next_opaque_body_chunk(
    shared: Arc<OpaqueBodyUploadState>,
) -> Result<Option<(Bytes, Arc<OpaqueBodyUploadState>)>, BodyReadError> {
    let in_flight = {
        let mut state = shared.reader.lock();
        if shared.cancelled.load(Ordering::Acquire) || state.finished {
            let reader = state.reader.take();
            state.finished = true;
            drop(state);
            drop(reader);
            return Ok(None);
        }
        let Some(reader) = state.reader.take() else {
            state.finished = true;
            let error = BodyReadError::Incomplete;
            *shared.read_error.lock() = Some(error.clone());
            return Err(error);
        };
        state.in_flight = true;
        InFlightBodyReader {
            shared: Arc::clone(&shared),
            reader: Some(reader),
            settled: false,
        }
    };
    if shared.cancelled.load(Ordering::Acquire) {
        shared.ambiguous_read_cancelled.store(true, Ordering::Release);
        in_flight.finish();
        return Ok(None);
    }

    let task = AbortTaskOnDrop::new(tokio::spawn(read_opaque_body_chunk_task(in_flight)));
    let abort_handle = task.abort_handle();
    {
        let mut state = shared.reader.lock();
        if state.in_flight {
            state.abort_handle = Some(abort_handle.clone());
            if shared.cancelled.load(Ordering::Acquire) {
                abort_handle.abort();
            }
        }
    }

    match task.join().await {
        Ok(Ok(Some(chunk))) => Ok(Some((chunk, shared))),
        Ok(Ok(None)) => Ok(None),
        Ok(Err(error)) => Err(error),
        Err(error) if error.is_cancelled() && shared.cancelled.load(Ordering::Acquire) => Ok(None),
        Err(_) => {
            let error = BodyReadError::Incomplete;
            *shared.read_error.lock() = Some(error.clone());
            Err(error)
        }
    }
}

fn opaque_body_state(reader: Box<dyn RequestBodySnapshotReader>) -> Arc<OpaqueBodyUploadState> {
    let len = reader.len();
    Arc::new(OpaqueBodyUploadState {
        len,
        fetched: AtomicU64::new(0),
        read_error: Mutex::new(None),
        cancelled: AtomicBool::new(false),
        ambiguous_read_cancelled: AtomicBool::new(false),
        reader: Mutex::new(OpaqueBodyReaderState {
            reader: Some(reader),
            in_flight: false,
            finished: false,
            abort_handle: None,
        }),
        state_changed: Notify::new(),
    })
}

fn opaque_body_stream(
    tracking: Arc<OpaqueBodyUploadState>,
) -> impl futures::Stream<Item = Result<Bytes, BodyReadError>> + Send {
    futures::stream::try_unfold(tracking, next_opaque_body_chunk)
}

fn opaque_body_upload(reader: Box<dyn RequestBodySnapshotReader>) -> OpaqueBodyUpload {
    let tracking = opaque_body_state(reader);
    let stream = opaque_body_stream(Arc::clone(&tracking));
    OpaqueBodyUpload {
        body: reqwest::Body::wrap_stream(stream),
        tracking,
    }
}

async fn settle_opaque_body_upload(tracking: &OpaqueBodyUploadState, reserved: u64) -> (u64, Option<BodyReadError>) {
    tracking.cancel_and_release_reader().await;
    let error = tracking.read_error.lock().clone();
    let refund = if tracking.ambiguous_read_cancelled.load(Ordering::Acquire) {
        0
    } else {
        let fetched = tracking.fetched.load(Ordering::Acquire);
        reserved.saturating_sub(fetched)
    };
    (refund, error)
}

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

#[cfg(test)]
fn body_bytes_to_value(bytes: &[u8], limit: usize) -> Result<Value, RuntimeError> {
    checked_value_str(String::from_utf8_lossy(bytes).into_owned(), limit)
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
/// not a propagated `RuntimeError`); this mirrors the `http.call` body cap.
/// Without it an oversized Redis value (up to 512 MB) would flow into
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
                let name = state.pop()?.runtime_string()?;
                checked_value_opt_str(session.header_value(&name), self.limits.max_string_len)
            }
            BuiltinId::ReqMethod => checked_value_str(session.get_method().to_string(), self.limits.max_string_len),
            BuiltinId::ReqPath => checked_value_str(session.get_path().to_string(), self.limits.max_string_len),
            BuiltinId::ReqQuery => {
                let name = state.pop()?.runtime_string()?;
                checked_value_opt_str(session.get_query_param(&name), self.limits.max_string_len)
            }
            BuiltinId::ReqQueryString => checked_value_opt_str(session.get_query(), self.limits.max_string_len),
            BuiltinId::ReqCookie => {
                let name = state.pop()?.runtime_string()?;
                checked_value_opt_str(session.get_cookie(&name), self.limits.max_string_len)
            }
            BuiltinId::ReqDirectPeerIp => {
                checked_value_str(session.direct_peer_ip().to_string(), self.limits.max_string_len)
            }
            BuiltinId::ReqRemoteIp => checked_value_str(session.remote_ip().to_string(), self.limits.max_string_len),
            BuiltinId::ReqPathParam => {
                let name = state.pop()?.runtime_string()?;
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
                let name = state.pop()?.runtime_string()?;
                Ok(Value::Bool(session.header_value(&name).is_some()))
            }

            // ===== req.* mutation =====
            BuiltinId::ReqSetHeader => {
                let value = state.pop()?.runtime_string()?;
                let name = state.pop()?.runtime_string()?;
                session
                    .set_request_header(&name, &value)
                    .map_err(|e| api_error("req.set_header", e))?;
                Ok(Value::Nil)
            }
            BuiltinId::ReqAppendHeader => {
                let value = state.pop()?.runtime_string()?;
                let name = state.pop()?.runtime_string()?;
                session
                    .append_request_header(&name, &value)
                    .map_err(|e| api_error("req.append_header", e))?;
                Ok(Value::Nil)
            }
            BuiltinId::ReqRemoveHeader => {
                let name = state.pop()?.runtime_string()?;
                session
                    .remove_request_header(&name)
                    .map_err(|e| api_error("req.remove_header", e))?;
                Ok(Value::Nil)
            }
            BuiltinId::ReqSetUri => {
                let uri = state.pop()?.runtime_string()?;
                session
                    .set_upstream_uri(&uri)
                    .map_err(|e| api_error("req.set_uri", e))?;
                Ok(Value::Nil)
            }
            BuiltinId::ReqSetHost => {
                let host = state.pop()?.runtime_string()?;
                session
                    .set_upstream_host(&host)
                    .map_err(|e| api_error("req.set_host", e))?;
                Ok(Value::Nil)
            }
            BuiltinId::ReqSetMethod => {
                let method = state.pop()?.runtime_string()?;
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
                let name = state.pop()?.runtime_string()?;
                checked_value_opt_str(session.get_response_header(&name), self.limits.max_string_len)
            }
            BuiltinId::RespHasHeader => {
                let name = state.pop()?.runtime_string()?;
                Ok(Value::Bool(session.get_response_header(&name).is_some()))
            }

            // ===== req.* metadata =====
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

            // ===== resp.* read (additional) =====
            BuiltinId::RespHeaderNames => {
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
                        if u16::try_from(c)
                            .ok()
                            .is_none_or(|status| !edgion_resources::http_status::is_valid_final_http_status(status))
                        {
                            return Err(RuntimeError::ApiError {
                                function: "resp.set_status".into(),
                                message: format!("invalid final status code: {} (must be 200-599)", c),
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
                let value = state.pop()?.runtime_string()?;
                let name = state.pop()?.runtime_string()?;
                session
                    .set_response_header(&name, &value)
                    .map_err(|e| api_error("resp.set_header", e))?;
                Ok(Value::Nil)
            }
            BuiltinId::RespAppendHeader => {
                let value = state.pop()?.runtime_string()?;
                let name = state.pop()?.runtime_string()?;
                session
                    .append_response_header(&name, &value)
                    .map_err(|e| api_error("resp.append_header", e))?;
                Ok(Value::Nil)
            }
            BuiltinId::RespRemoveHeader => {
                let name = state.pop()?.runtime_string()?;
                session
                    .remove_response_header(&name)
                    .map_err(|e| api_error("resp.remove_header", e))?;
                Ok(Value::Nil)
            }

            // ===== ctx.* =====
            BuiltinId::CtxGet => {
                let key = state.pop()?.runtime_string()?;
                checked_value_opt_str(session.get_ctx_var(&key), self.limits.max_string_len)
            }
            BuiltinId::CtxSet => {
                let value = state.pop()?.runtime_string()?;
                let key = state.pop()?.runtime_string()?;
                session.set_ctx_var(&key, &value).map_err(|e| api_error("ctx.set", e))?;
                Ok(Value::Nil)
            }
            BuiltinId::CtxRemove => {
                let key = state.pop()?.runtime_string()?;
                session.remove_ctx_var(&key).map_err(|e| api_error("ctx.remove", e))?;
                Ok(Value::Nil)
            }

            // ===== Utilities =====
            BuiltinId::Log => {
                let msg = state.pop()?.runtime_string()?;
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
                let s = v.runtime_string()?;
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
                let s = state.pop()?.runtime_string()?;
                let result = s.to_uppercase();
                check_string_len(&result, self.limits.max_string_len)?;
                Ok(Value::Str(result.into()))
            }
            BuiltinId::ToLower => {
                let s = state.pop()?.runtime_string()?;
                let result = s.to_lowercase();
                check_string_len(&result, self.limits.max_string_len)?;
                Ok(Value::Str(result.into()))
            }
            BuiltinId::Base64Encode => {
                use base64::Engine;
                let s = state.pop()?.runtime_string()?;
                let encoded = base64::engine::general_purpose::STANDARD.encode(s.as_bytes());
                check_string_len(&encoded, self.limits.max_string_len)?;
                Ok(Value::Str(encoded.into()))
            }
            BuiltinId::Base64Decode => {
                use base64::Engine;
                let s = state.pop()?.runtime_string()?;
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
                let s = state.pop()?.runtime_string()?;
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
                let s = state.pop()?.runtime_string()?;
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
                let s = state.pop()?.runtime_string()?;
                let hash = sha2::Sha256::digest(s.as_bytes());
                checked_value_str(hex::encode(hash), self.limits.max_string_len)
            }
            BuiltinId::Md5 => {
                use md5::Digest;
                let s = state.pop()?.runtime_string()?;
                let hash = md5::Md5::digest(s.as_bytes());
                checked_value_str(hex::encode(hash), self.limits.max_string_len)
            }
            BuiltinId::Trim => {
                let s = state.pop()?.runtime_string()?;
                let trimmed = s.trim();
                if trimmed.len() == s.len() {
                    Ok(Value::Str(s.into()))
                } else {
                    Ok(Value::Str(Arc::from(trimmed)))
                }
            }
            BuiltinId::Split => {
                let delim = state.pop()?.runtime_string()?;
                let s = state.pop()?.runtime_string()?;
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

            // ===== Additional utilities =====
            BuiltinId::HmacSha256 => {
                use hmac::{Hmac, Mac};
                type HmacSha256 = Hmac<sha2::Sha256>;

                let data = state.pop()?.runtime_string()?;
                let key = state.pop()?.runtime_string()?;
                let mut mac = HmacSha256::new_from_slice(key.as_bytes()).map_err(|e| RuntimeError::ApiError {
                    function: "hmac_sha256".into(),
                    message: format!("invalid key: {}", e),
                })?;
                mac.update(data.as_bytes());
                let result = hex::encode(mac.finalize().into_bytes());
                checked_value_str(result, self.limits.max_string_len)
            }
            BuiltinId::Uuid => {
                let id = uuid::Uuid::new_v4().to_string();
                Ok(Value::Str(id.into()))
            }
            BuiltinId::HexEncode => {
                let s = state.pop()?.runtime_string()?;
                let encoded = hex::encode(s.as_bytes());
                check_string_len(&encoded, self.limits.max_string_len)?;
                Ok(Value::Str(encoded.into()))
            }
            BuiltinId::HexDecode => {
                let s = state.pop()?.runtime_string()?;
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
                let format = state.pop()?.runtime_string()?;
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
                let cidr_str = state.pop()?.runtime_string()?;
                let ip_str = state.pop()?.runtime_string()?;
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
                let delim = state.pop()?.runtime_string()?;
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
                let value = state.pop()?.runtime_string()?;
                log.push_sensitive(&value);
                Ok(Value::Nil)
            }
            BuiltinId::LogError => {
                let value = state.pop()?.runtime_string()?;
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
                    values.push(state.pop()?.runtime_string()?);
                }
                values.reverse();
                let template = state.pop()?.runtime_string()?;
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

            // proxy-body builtins are dispatched asynchronously by the VM before reaching call_builtin
            BuiltinId::ReqBody | BuiltinId::RespBody => Err(RuntimeError::Internal {
                message: format!("{:?} must be dispatched via call_builtin_body, not call_builtin", id),
            }),
        }
    }

    /// Async handler for http.call(method, url, body [, timeout_ms [, headers_map]]) → Map
    pub(crate) async fn call_builtin_http(
        &self,
        argc: u8,
        state: &mut VmState,
        session: &mut dyn PluginSession,
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

        let method_str = method_val.runtime_string()?;
        let url_str = url_val.runtime_string()?;

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

        let (opaque_reader, body_str) = match body_val {
            Value::Body(BodyValue::Request) => {
                let known_bodyless = session.declares_body() == Some(false);
                (
                    Some(
                        session
                            .request_body_snapshot_reader(known_bodyless)
                            .await
                            .map_err(|error| body_access_error(error.to_string()))?,
                    ),
                    None,
                )
            }
            Value::Str(value) => (None, Some(value)),
            Value::Nil => (None, None),
            other => {
                return Err(RuntimeError::TypeError {
                    expected: "Str, Body, or Nil",
                    got: other.type_name(),
                    operation: "http.call body",
                });
            }
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

        let mut request_headers = Vec::new();
        if let Some(hv) = headers_val {
            match hv {
                Value::Map(m) => {
                    for (name, value) in m.iter() {
                        if opaque_reader.is_some()
                            && (name.eq_ignore_ascii_case("content-length")
                                || name.eq_ignore_ascii_case("transfer-encoding"))
                        {
                            return Err(RuntimeError::ApiError {
                                function: "http.call".into(),
                                message:
                                    "Content-Length and Transfer-Encoding are managed by http.call for Body payloads"
                                        .to_string(),
                            });
                        }
                        let value = value.try_as_str().map_err(|error| RuntimeError::TypeError {
                            expected: "a value implicitly convertible to Str",
                            got: error.got(),
                            operation: "http.call headers_map",
                        })?;
                        request_headers.push((name.clone(), value.to_string()));
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

        let is_opaque_body = opaque_reader.is_some();
        // Opaque Body streams are intentionally not replayed across redirects.
        let client = dsl_http_client(runtime.block_private_ips, !is_opaque_body);
        let mut builder = client
            .request(method, &url_str)
            .timeout(std::time::Duration::from_millis(timeout_ms));

        for (name, value) in request_headers {
            builder = builder.header(name, value);
        }

        let mut body_accounting = None;
        if let Some(reader) = opaque_reader {
            let upload = opaque_body_upload(reader);
            let OpaqueBodyUpload { body, tracking } = upload;
            let physical_cap = session.ctx().request_body_config.max_body_size_bytes;
            let configured_cap = u64::try_from(runtime.max_request_body_bytes)
                .map_err(|_| body_access_error("http.call request Body limit cannot be represented as bytes"))?;
            let effective_cap = configured_cap.min(physical_cap);
            if tracking.len > effective_cap {
                return Err(body_access_error(format!(
                    "request Body exceeds http.call limit of {effective_cap} bytes"
                )));
            }
            state.reserve_body_scan_exact(tracking.len, physical_cap, runtime.max_body_scan_passes)?;
            builder = builder.body(body).header(reqwest::header::CONTENT_LENGTH, tracking.len);
            body_accounting = Some((tracking.len, tracking));
        } else if let Some(body) = body_str {
            if !body.is_empty() {
                builder = builder.body(body.to_string());
            }
        }

        let failure_map = || {
            let mut m = IndexMap::new();
            m.insert("status".to_string(), Value::Int(0));
            m.insert("body".to_string(), Value::Str(Arc::from("")));
            m.insert("headers".to_string(), Value::Map(Box::new(IndexMap::new())));
            Value::Map(Box::new(m))
        };

        let send_result = builder.send().await;
        if let Some((reserved, tracking)) = body_accounting {
            // A response head can arrive before an HTTP/2 request body has
            // finished. Release the snapshot independently of whether Hyper
            // polls its Body again: Hyper may be blocked on flow-control
            // capacity while holding one already-buffered chunk.
            let (refund, read_error) = settle_opaque_body_upload(&tracking, reserved).await;
            state.refund_body_scan_exact(refund);
            if let Some(error) = read_error {
                return Err(body_access_error(error.to_string()));
            }
        }

        let resp = match send_result {
            Ok(response) => response,
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

impl Vm {
    /// Dispatch the proxy-body builtins (`req.body` / `resp.body`).
    ///
    /// This performs only a metadata availability check and returns an opaque
    /// Body token. Availability failures use the VM's dedicated body-access
    /// marker and are fixed fail-close, independent of `errorPolicy`.
    pub(crate) async fn call_builtin_body(
        &self,
        id: BuiltinId,
        _state: &mut VmState,
        session: &mut dyn PluginSession,
        _log: &mut PluginLog,
    ) -> Result<Value, RuntimeError> {
        match id {
            BuiltinId::ReqBody => {
                let known_bodyless = session.declares_body() == Some(false);
                session
                    .request_body_available_len(known_bodyless)
                    .map_err(|error| body_access_error(error.to_string()))?;
                Ok(Value::Body(BodyValue::Request))
            }
            BuiltinId::RespBody => Err(body_access_error(
                "proxied upstream response bodies remain streaming-only",
            )),
            other => Err(RuntimeError::Internal {
                message: format!("{:?} is not a body builtin", other),
            }),
        }
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
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
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
                let value = state.pop()?.runtime_string()?;
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
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
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
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
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
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
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
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
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
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
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
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
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
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
                let field = state.pop()?.runtime_string()?;
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
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
                let val = state.pop()?.runtime_string()?;
                let field = state.pop()?.runtime_string()?;
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
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
                let field = state.pop()?.runtime_string()?;
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
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
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
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
                let field = state.pop()?.runtime_string()?;
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
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
            | BuiltinId::ReqDirectPeerIp
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
            | BuiltinId::Uuid
            | BuiltinId::HexEncode
            | BuiltinId::HexDecode
            | BuiltinId::TimeFormat
            | BuiltinId::IpInCidr
            | BuiltinId::Join
            | BuiltinId::LogWarn
            | BuiltinId::LogError
            | BuiltinId::LogFmt
            | BuiltinId::HttpCall
            | BuiltinId::ReqBody
            | BuiltinId::RespBody => Err(RuntimeError::Internal {
                message: format!("{:?} must not be dispatched via call_builtin_redis", id),
            }),
        }
    }
}

/// Return true if `host` is an IP literal that must be blocked to prevent SSRF
/// (loopback / link-local / RFC 1918 private / RFC 4193 ULA / unspecified).
/// Hostnames are not resolved by this helper. Guarded clients use the shared
/// `EgressResolver` to classify their resolved addresses at dial time.
/// IPv6 zone IDs are stripped per RFC 6874 before parsing.
///
/// Exposed `pub(crate)` so the guarded DSL HTTP client's redirect policy
/// (`runtime::dsl_http_client(true)`) re-applies the same IP-literal check to
/// every redirect hop.
pub(crate) fn is_ssrf_blocked_ip_literal(host: &str) -> bool {
    use crate::util::net::{classify_ip, IpClass};
    use std::net::IpAddr;
    // Strip IPv6 zone IDs (RFC 6874: "::1%eth0").
    let host = host.split('%').next().unwrap_or(host);
    let ip: IpAddr = match host.parse() {
        Ok(ip) => ip,
        Err(_) => return false, // hostname, not an IP literal
    };
    // Exhaustive match, no wildcard: a new IpClass variant becomes a compile error
    // here too, so it cannot silently fall outside the DSL blocklist (an SSRF miss).
    // DSL deliberately BLOCKS private / unique-local / unspecified.
    match classify_ip(ip) {
        IpClass::Loopback | IpClass::LinkLocal | IpClass::Private | IpClass::UniqueLocal | IpClass::Unspecified => true,
        IpClass::Global => false,
    }
}

#[cfg(test)]
mod ip_tests {
    use super::is_ssrf_blocked_ip_literal;

    // DSL policy: one representative per blocked IpClass + an allowed public case.
    // Exhaustive per-segment classification is covered by util::net::classify_ip tests.
    #[test]
    fn dsl_policy_blocks_non_global_classes() {
        assert!(is_ssrf_blocked_ip_literal("127.0.0.1")); // Loopback v4
        assert!(is_ssrf_blocked_ip_literal("::1")); // Loopback v6
        assert!(is_ssrf_blocked_ip_literal("169.254.1.1")); // LinkLocal
        assert!(is_ssrf_blocked_ip_literal("10.0.0.1")); // Private
        assert!(is_ssrf_blocked_ip_literal("fc00::1")); // UniqueLocal
        assert!(is_ssrf_blocked_ip_literal("0.0.0.0")); // Unspecified
    }

    #[test]
    fn dsl_policy_allows_public() {
        assert!(!is_ssrf_blocked_ip_literal("8.8.8.8"));
        assert!(!is_ssrf_blocked_ip_literal("2001:db8::1"));
    }

    // str-wrapper behavior — DSL-specific, not covered by classify_ip.
    #[test]
    fn ipv6_zone_id_stripped() {
        assert!(is_ssrf_blocked_ip_literal("fe80::1%eth0")); // link-local with zone ID
        assert!(is_ssrf_blocked_ip_literal("::1%lo")); // loopback with zone ID
    }

    #[test]
    fn hostnames_not_blocked() {
        // Hostnames are not resolved — always returns false.
        assert!(!is_ssrf_blocked_ip_literal("localhost"));
        assert!(!is_ssrf_blocked_ip_literal("example.com"));
        assert!(!is_ssrf_blocked_ip_literal("internal.corp"));
    }

    #[test]
    fn ipv4_mapped_ipv6_via_str() {
        // The str path plus classify_ip's IPv4-mapped normalization together.
        assert!(is_ssrf_blocked_ip_literal("::ffff:192.168.1.1"));
        assert!(!is_ssrf_blocked_ip_literal("::ffff:8.8.8.8")); // mapped public allowed
    }

    #[test]
    fn reqwest_url_host_str_brackets_stripped() {
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

#[cfg(test)]
mod body_tests {
    use super::super::validator::{compile_dsl_source, ValidationLimits};
    use super::super::vm::{execute_safe, DslErrorPolicy, Vm, VmLimits};
    use super::body_bytes_to_value;
    use crate::plugins::http::dsl::lang::bytecode::CompiledScript;
    use crate::plugins::http::dsl::runtime::DslRuntime;
    use crate::plugins::runtime::traits::session::{BodyReadError, MockPluginSession};
    use crate::plugins::runtime::{PluginLog, PluginSession};
    use bytes::Bytes;
    use edgion_resources::filters::PluginRunningResult;

    use super::RuntimeError;
    use super::Value;

    #[test]
    fn body_bytes_to_value_decodes_utf8() {
        let v = body_bytes_to_value(b"hello", 100).unwrap();
        assert_eq!(v, Value::Str("hello".into()));
    }

    #[test]
    fn body_bytes_to_value_lossy_on_invalid_utf8() {
        // 0xFF is not valid UTF-8 → U+FFFD, not truncated.
        let v = body_bytes_to_value(&[0x61, 0xFF, 0x62], 100).unwrap();
        match v {
            Value::Str(s) => {
                assert!(s.starts_with('a') && s.ends_with('b'));
                assert!(s.contains('\u{FFFD}'));
            }
            other => panic!("expected Str, got {other:?}"),
        }
    }

    #[test]
    fn body_bytes_to_value_rejects_oversize() {
        let err = body_bytes_to_value(b"toolong", 3).unwrap_err();
        assert!(matches!(err, RuntimeError::StringTooLong { .. }));
    }

    /// Mirrors the manual current-thread-runtime + `block_on(execute_safe(...))`
    /// harness used throughout `dsl/tests/*` (e.g. `error_policy.rs`) —
    /// `MockPluginSession` is not `Send`, so these tests use `#[test]`, not
    /// `#[tokio::test]`.
    #[test]
    fn req_body_availability_failure_is_fixed_fail_close() {
        let b64 = compile_dsl_source("let b = req.body", &ValidationLimits::default()).expect("compile failed");
        let script = CompiledScript::deserialize_base64(&b64).expect("deserialize failed");
        let vm = Vm::new(script, VmLimits::default()).expect("vm creation failed");

        let mut session = MockPluginSession::new();
        session.expect_declares_body().return_const(Some(true));
        session
            .expect_request_body_available_len()
            .returning(|_| Err(BodyReadError::Unimplemented));
        session.expect_record_outcome().return_const(());

        let mut log = PluginLog::new("test");
        let rt = DslRuntime::default();
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(execute_safe(
                &vm,
                &mut session as &mut dyn PluginSession,
                &mut log,
                &DslErrorPolicy::Ignore,
                &rt,
            ));
        assert_eq!(
            result,
            PluginRunningResult::ErrResponse {
                status: 500,
                body: Some(Bytes::from_static(b"Request body processing failed")),
            }
        );
    }
}

#[cfg(test)]
mod opaque_body_stream_tests {
    use super::{
        opaque_body_state, opaque_body_stream, settle_opaque_body_upload, BodyReadError, RequestBodySnapshotReader,
    };
    use async_trait::async_trait;
    use bytes::Bytes;
    use futures::{poll, StreamExt};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    struct ScriptedReader {
        len: u64,
        results: VecDeque<Result<Option<Bytes>, BodyReadError>>,
    }

    #[async_trait]
    impl RequestBodySnapshotReader for ScriptedReader {
        fn len(&self) -> u64 {
            self.len
        }

        async fn next_chunk(&mut self) -> Result<Option<Bytes>, BodyReadError> {
            self.results.pop_front().unwrap_or(Ok(None))
        }
    }

    #[tokio::test]
    async fn midstream_snapshot_failure_is_recorded_after_fetched_bytes() {
        let reader = ScriptedReader {
            len: 8,
            results: VecDeque::from([
                Ok(Some(Bytes::from_static(b"abcd"))),
                Err(BodyReadError::StorageFailure),
            ]),
        };
        let tracking = opaque_body_state(Box::new(reader));
        let mut stream = Box::pin(opaque_body_stream(Arc::clone(&tracking)));

        assert_eq!(stream.next().await.unwrap().unwrap(), Bytes::from_static(b"abcd"));
        assert!(matches!(
            stream.next().await.unwrap(),
            Err(BodyReadError::StorageFailure)
        ));
        assert_eq!(tracking.fetched.load(Ordering::Acquire), 4);
        assert!(matches!(
            tracking.read_error.lock().as_ref(),
            Some(BodyReadError::StorageFailure)
        ));
        let (refund, error) = settle_opaque_body_upload(&tracking, 8).await;
        assert_eq!(refund, 4);
        assert!(matches!(error, Some(BodyReadError::StorageFailure)));
    }

    struct PendingReader {
        dropped: Arc<AtomicBool>,
    }

    impl Drop for PendingReader {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Release);
        }
    }

    #[async_trait]
    impl RequestBodySnapshotReader for PendingReader {
        fn len(&self) -> u64 {
            1
        }

        async fn next_chunk(&mut self) -> Result<Option<Bytes>, BodyReadError> {
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn cancellation_stops_and_drops_a_pending_snapshot_reader() {
        let dropped = Arc::new(AtomicBool::new(false));
        let reader = PendingReader {
            dropped: Arc::clone(&dropped),
        };
        let tracking = opaque_body_state(Box::new(reader));
        let mut stream = Box::pin(opaque_body_stream(Arc::clone(&tracking)));

        let next = stream.next();
        futures::pin_mut!(next);
        assert!(poll!(next.as_mut()).is_pending());

        let (refund, item) = tokio::join!(settle_opaque_body_upload(&tracking, 1), next.as_mut());
        assert_eq!(refund, (0, None));
        assert!(item.is_none());
        assert!(dropped.load(Ordering::Acquire));
        assert!(tracking.ambiguous_read_cancelled.load(Ordering::Acquire));
    }

    struct FirstChunkThenPendingReader {
        first: bool,
        dropped: Arc<AtomicBool>,
    }

    impl Drop for FirstChunkThenPendingReader {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Release);
        }
    }

    #[async_trait]
    impl RequestBodySnapshotReader for FirstChunkThenPendingReader {
        fn len(&self) -> u64 {
            128 * 1024
        }

        async fn next_chunk(&mut self) -> Result<Option<Bytes>, BodyReadError> {
            if self.first {
                self.first = false;
                return Ok(Some(Bytes::from(vec![b'x'; 64 * 1024])));
            }
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn cancellation_releases_reader_without_another_stream_poll() {
        let dropped = Arc::new(AtomicBool::new(false));
        let reader = FirstChunkThenPendingReader {
            first: true,
            dropped: Arc::clone(&dropped),
        };
        let tracking = opaque_body_state(Box::new(reader));
        let mut stream = Box::pin(opaque_body_stream(Arc::clone(&tracking)));

        assert_eq!(stream.next().await.unwrap().unwrap().len(), 64 * 1024);
        let (refund, error) = settle_opaque_body_upload(&tracking, 128 * 1024).await;

        assert!(dropped.load(Ordering::Acquire));
        assert_eq!(tracking.fetched.load(Ordering::Acquire), 64 * 1024);
        assert!(!tracking.ambiguous_read_cancelled.load(Ordering::Acquire));
        assert_eq!(refund, 64 * 1024);
        assert_eq!(error, None);
    }

    #[tokio::test]
    async fn dropping_a_pending_stream_future_aborts_the_chunk_task() {
        let dropped = Arc::new(AtomicBool::new(false));
        let reader = PendingReader {
            dropped: Arc::clone(&dropped),
        };
        let tracking = opaque_body_state(Box::new(reader));
        let mut stream = Box::pin(opaque_body_stream(Arc::clone(&tracking)));

        {
            let next = stream.next();
            futures::pin_mut!(next);
            assert!(poll!(next.as_mut()).is_pending());
        }
        drop(stream);

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !dropped.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("dropping the upload future must abort its chunk task");

        let settled = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            settle_opaque_body_upload(&tracking, 1),
        )
        .await
        .expect("dropped upload future must not leave in_flight stuck");

        assert_eq!(settled, (0, None));
        assert!(dropped.load(Ordering::Acquire));
        assert!(tracking.ambiguous_read_cancelled.load(Ordering::Acquire));
    }
}
