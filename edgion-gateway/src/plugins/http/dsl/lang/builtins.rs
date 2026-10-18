//! Built-in function dispatch — maps BuiltinId to PluginSession methods
//!
//! Arguments are popped from the VM stack, results pushed back.
//! Each builtin maps to exactly one PluginSession API call.

use bytes::Bytes;
use edgion_resources::redis_rate_limit::validate_rate_limit_call;
use indexmap::IndexMap;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

use crate::link_sys::providers::redis::RedisLinkClient;
use crate::link_sys::runtime::store::get_redis_client;
use crate::plugins::runtime::session_adapter::cookie_value_ref;
use crate::plugins::runtime::{PluginLog, PluginSession, PluginSessionHeaderExt};
use crate::request_body::{BodyReadError, RequestBodySnapshotReader};
use crate::util::http_body::stream_body_capped;

use super::super::runtime::{dsl_http_client, DslRuntime};
use super::body_ops::body_access_error;
use super::bytecode::BuiltinId;
use super::error::RuntimeError;
use super::value::{BodyValue, Value};
use super::vm::{RuntimeString, Vm, VmState};

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

const REDIS_ERROR_FALLBACK: &str = "e";

/// Per-result field/name plus value payload cap, independent of VM admission.
pub(in crate::plugins::http::dsl) const MAX_DSL_HOST_RESULT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug)]
enum RedisFailure {
    InvalidArgument,
    UnknownPolicy,
    UnknownLink,
    ProviderNotReady,
    DependencyError,
    Timeout,
    ResultTooLarge,
    InvalidResponse,
}

impl RedisFailure {
    #[cfg(test)]
    const ALL: [Self; 8] = [
        Self::InvalidArgument,
        Self::UnknownPolicy,
        Self::UnknownLink,
        Self::ProviderNotReady,
        Self::DependencyError,
        Self::Timeout,
        Self::ResultTooLarge,
        Self::InvalidResponse,
    ];

    const fn tag(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid_argument",
            Self::UnknownPolicy => "unknown_policy",
            Self::UnknownLink => "unknown_link",
            Self::ProviderNotReady => "provider_not_ready",
            Self::DependencyError => "dependency_error",
            Self::Timeout => "timeout",
            Self::ResultTooLarge => "result_too_large",
            Self::InvalidResponse => "invalid_response",
        }
    }
}

fn bounded_redis_get_limit(value: &Value, max_string_len: usize) -> Option<usize> {
    match value {
        Value::Int(n) if *n > 0 => usize::try_from(*n)
            .ok()
            .map(|n| n.min(max_string_len).min(edgion_dsl::json::MAX_DSL_JSON_BYTES)),
        _ => None,
    }
}

/// Build a bounded `Map { ok: false, value: nil, error: <tag> }` response.
///
/// Requiring an enumerated failure prevents dependency errors and caller-controlled
/// Redis arguments from entering the script-visible result. When an unusually small
/// string budget cannot hold the specific tag, use one fixed generic tag rather
/// than truncating potentially sensitive text.
fn redis_err(failure: RedisFailure, max_string_len: usize) -> Value {
    let tag = failure.tag();
    let tag = if tag.len() <= max_string_len {
        tag
    } else if REDIS_ERROR_FALLBACK.len() <= max_string_len {
        REDIS_ERROR_FALLBACK
    } else {
        ""
    };
    let mut m = IndexMap::new();
    m.insert("ok".to_string(), Value::Bool(false));
    m.insert("value".to_string(), Value::Nil);
    m.insert("error".to_string(), Value::Str(Arc::from(tag)));
    Value::Map(Box::new(m))
}

fn redis_dependency_err(error: &dyn std::fmt::Display, max_string_len: usize, log: &mut PluginLog) -> Value {
    log.push_err_sensitive(&error.to_string());
    redis_err(RedisFailure::DependencyError, max_string_len)
}

fn redis_hash_result(map: Option<HashMap<String, String>>, max_string_len: usize, log: &mut PluginLog) -> Value {
    let Some(map) = map else {
        log.push_err("redis result too large");
        return redis_err(RedisFailure::ResultTooLarge, max_string_len);
    };
    let mut result = IndexMap::with_capacity(map.len());
    for (key, value) in map {
        let key = truncate_to_max_string_len(key, max_string_len);
        let value = truncate_to_max_string_len(value, max_string_len);
        result.insert(key, Value::Str(value.into()));
    }
    redis_ok(Value::Map(Box::new(result)))
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

pub(crate) fn checked_value_str(s: String, limit: usize) -> Result<Value, RuntimeError> {
    check_string_len(&s, limit)?;
    Ok(Value::Str(s.into()))
}

fn checked_join_output_len<I>(mut item_lens: I, delimiter_len: usize, limit: usize) -> Result<usize, RuntimeError>
where
    I: ExactSizeIterator<Item = usize>,
{
    let separator_len = match item_lens.len() {
        0 | 1 => 0,
        count => delimiter_len
            .checked_mul(count - 1)
            .ok_or(RuntimeError::StringTooLong { len: usize::MAX, limit })?,
    };
    let output_len = item_lens.try_fold(separator_len, |total, item_len| {
        total
            .checked_add(item_len)
            .ok_or(RuntimeError::StringTooLong { len: usize::MAX, limit })
    })?;
    if output_len > limit {
        return Err(RuntimeError::StringTooLong { len: output_len, limit });
    }
    Ok(output_len)
}

fn http_call_failure_map() -> Value {
    let mut result = IndexMap::new();
    result.insert("status".to_string(), Value::Int(0));
    result.insert("body".to_string(), Value::Str(Arc::from("")));
    result.insert("headers".to_string(), Value::Map(Box::new(IndexMap::new())));
    Value::Map(Box::new(result))
}

fn checked_bounded_len(current: usize, addition: usize, limit: usize) -> Option<usize> {
    current.checked_add(addition).filter(|total| *total <= limit)
}

/// Return the exact UTF-8 byte length produced by `String::from_utf8_lossy`
/// without allocating the converted string.
fn lossy_utf8_output_len(mut bytes: &[u8], limit: usize) -> Option<usize> {
    // `from_utf8_lossy` emits a three-byte UTF-8 replacement for each error.
    const REPLACEMENT_LEN: usize = 3;

    let mut output_len = 0;
    loop {
        match std::str::from_utf8(bytes) {
            Ok(valid) => return checked_bounded_len(output_len, valid.len(), limit),
            Err(error) => {
                output_len = checked_bounded_len(output_len, error.valid_up_to(), limit)?;
                output_len = checked_bounded_len(output_len, REPLACEMENT_LEN, limit)?;

                let Some(error_len) = error.error_len() else {
                    return Some(output_len);
                };
                let consumed = error.valid_up_to().checked_add(error_len)?;
                bytes = bytes.get(consumed..)?;
            }
        }
    }
}

fn decode_http_call_body(bytes: &Bytes, limit: usize) -> Option<Arc<str>> {
    match std::str::from_utf8(bytes) {
        Ok(text) => {
            checked_bounded_len(0, text.len(), limit)?;
            Some(Arc::from(text))
        }
        Err(_) => {
            lossy_utf8_output_len(bytes, limit)?;
            Some(Arc::from(String::from_utf8_lossy(bytes).into_owned()))
        }
    }
}

/// Convert one completed outbound attempt into the script-visible `http.call`
/// result while enforcing every response-string and response-body bound.
pub(in crate::plugins::http::dsl) async fn complete_http_call_response(
    send_result: Result<reqwest::Response, reqwest::Error>,
    max_response_body_bytes: usize,
    max_string_len: usize,
    max_map_size: usize,
) -> Value {
    let Ok(response) = send_result else {
        return http_call_failure_map();
    };
    let effective_body_cap = max_response_body_bytes.min(max_string_len);
    if response
        .content_length()
        .is_some_and(|length| usize::try_from(length).map_or(true, |length| length > effective_body_cap))
    {
        return http_call_failure_map();
    }

    let status = response.status().as_u16() as i64;
    let mut response_headers: IndexMap<String, Value> = IndexMap::new();
    let mut header_bytes: usize = 0;
    for (name, value) in response.headers() {
        if response_headers.len() >= max_map_size {
            break;
        }
        let Ok(value) = value.to_str() else {
            continue;
        };
        // HeaderName already exposes its canonical lowercase spelling.
        let name = name.as_str();
        if name.len() > max_string_len || value.len() > max_string_len {
            return http_call_failure_map();
        }
        let (previous_value_bytes, name_bytes) = match response_headers.get(name) {
            Some(Value::Str(previous)) => (previous.len(), 0),
            _ => (0, name.len()),
        };
        let Some(next) = header_bytes
            .checked_sub(previous_value_bytes)
            .and_then(|total| checked_bounded_len(total, name_bytes, MAX_DSL_HOST_RESULT_BYTES))
            .and_then(|total| checked_bounded_len(total, value.len(), MAX_DSL_HOST_RESULT_BYTES))
        else {
            return http_call_failure_map();
        };
        header_bytes = next;
        response_headers.insert(name.to_owned(), Value::Str(Arc::from(value)));
    }

    let Ok((body, truncated)) = stream_body_capped(response, effective_body_cap).await else {
        return http_call_failure_map();
    };
    if truncated {
        return http_call_failure_map();
    }
    let Some(body) = decode_http_call_body(&body, max_string_len) else {
        return http_call_failure_map();
    };

    let mut result = IndexMap::new();
    result.insert("status".to_string(), Value::Int(status));
    result.insert("body".to_string(), Value::Str(body));
    result.insert("headers".to_string(), Value::Map(Box::new(response_headers)));
    Value::Map(Box::new(result))
}

#[cfg(test)]
include!("../../../../../tests/support/plugins/http/dsl/lang/builtins_test/527_function_item.rs");

/// Silently truncate `s` to at most `limit` UTF-8 bytes, walking back to the
/// nearest char boundary so `String::truncate` cannot panic on multi-byte
/// content. `is_char_boundary(0)` always succeeds, so the loop terminates.
///
/// Used by the `redis.*` read builtins (`get` / `hget` / `hgetall`) to restore
/// the sandbox invariant "no DSL string value exceeds max_string_len" for
/// externally-controlled Redis values. We *truncate* rather than raise
/// `StringTooLong` because the redis dispatcher carries the documented "never
/// raises a runtime error" contract (errors surface as a `redis_err` Map value,
/// not a propagated `RuntimeError`). This Redis-specific compatibility behavior
/// does not apply to `http.call`, whose bounded exchange fails on overflow.
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

/// Validate borrowed host data before allocating the VM's independent string.
fn checked_value_opt_str_ref(s: Option<&str>, limit: usize) -> Result<Value, RuntimeError> {
    match s {
        Some(v) => {
            check_string_len(v, limit)?;
            Ok(Value::Str(Arc::from(v)))
        }
        None => Ok(Value::Nil),
    }
}

fn api_error(function: &'static str, err: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::ApiError {
        function: function.into(),
        message: format!("plugin session operation failed: {err}"),
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
                checked_value_opt_str_ref(session.header_str(&name), self.limits.max_string_len)
            }
            BuiltinId::ReqMethod => checked_value_opt_str_ref(Some(session.get_method()), self.limits.max_string_len),
            BuiltinId::ReqRequestMethod => match session.get_request_method() {
                Some(method) => {
                    check_string_len(method, self.limits.max_string_len)?;
                    Ok(Value::Str(Arc::from(method)))
                }
                None => Ok(Value::Nil),
            },
            BuiltinId::ReqPath => checked_value_opt_str_ref(Some(session.get_path()), self.limits.max_string_len),
            BuiltinId::ReqQuery => {
                let name = state.pop()?.runtime_string()?;
                checked_value_opt_str_ref(session.get_query_param_ref(&name), self.limits.max_string_len)
            }
            BuiltinId::ReqQueryString => checked_value_opt_str_ref(session.request_query(), self.limits.max_string_len),
            BuiltinId::ReqCookie => {
                let name = state.pop()?.runtime_string()?;
                checked_value_opt_str_ref(
                    cookie_value_ref(session.request_header_map(), &name),
                    self.limits.max_string_len,
                )
            }
            BuiltinId::ReqDirectPeerIp => {
                checked_value_opt_str_ref(Some(session.direct_peer_ip()), self.limits.max_string_len)
            }
            BuiltinId::ReqRemoteIp => checked_value_opt_str_ref(Some(session.remote_ip()), self.limits.max_string_len),
            BuiltinId::ReqPathParam => {
                let name = state.pop()?.runtime_string()?;
                checked_value_opt_str_ref(session.get_path_param_ref(&name), self.limits.max_string_len)
            }
            BuiltinId::ReqHeaderNames => {
                const MAX_HEADER_NAMES: usize = 256;
                let mut names: Vec<Arc<str>> = Vec::new();
                for (k, v) in session.iter_headers() {
                    if names.len() >= MAX_HEADER_NAMES {
                        break;
                    }
                    // Preserve the owned accessor's behavior: a header whose value
                    // is not valid UTF-8 is absent from the name list.
                    if std::str::from_utf8(v).is_err() {
                        continue;
                    }
                    check_string_len(k, self.limits.max_string_len)?;
                    names.push(Arc::from(k));
                }
                Ok(Value::List(names))
            }
            BuiltinId::ReqScheme => {
                // Detect scheme from common proxy headers, fallback to "http"
                let scheme = session
                    .header_str("X-Forwarded-Proto")
                    .or_else(|| session.header_str("X-Scheme"))
                    .unwrap_or("http");
                checked_value_opt_str_ref(Some(scheme), self.limits.max_string_len)
            }
            BuiltinId::ReqHost => checked_value_opt_str_ref(session.header_str("Host"), self.limits.max_string_len),
            BuiltinId::ReqUri => checked_value_str(session.get_uri(), self.limits.max_string_len),
            BuiltinId::ReqRequestPath => {
                checked_value_opt_str_ref(Some(session.get_request_path()), self.limits.max_string_len)
            }
            BuiltinId::ReqRequestUri => checked_value_str(session.get_request_uri(), self.limits.max_string_len),
            BuiltinId::ReqContentType => {
                checked_value_opt_str_ref(session.header_str("Content-Type"), self.limits.max_string_len)
            }
            BuiltinId::ReqHasHeader => {
                let name = state.pop()?.runtime_string()?;
                Ok(Value::Bool(session.header_str(&name).is_some()))
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
                checked_value_opt_str_ref(
                    session
                        .response_header_map()
                        .get(name.as_str())
                        .and_then(|value| value.to_str().ok()),
                    self.limits.max_string_len,
                )
            }
            BuiltinId::RespHasHeader => {
                let name = state.pop()?.runtime_string()?;
                Ok(Value::Bool(
                    session
                        .response_header_map()
                        .get(name.as_str())
                        .and_then(|value| value.to_str().ok())
                        .is_some(),
                ))
            }

            // ===== req.* metadata =====
            BuiltinId::ReqProtocol => {
                checked_value_opt_str_ref(Some(session.discover_protocol()), self.limits.max_string_len)
            }
            BuiltinId::ReqGrpcService => checked_value_opt_str_ref(session.grpc_service(), self.limits.max_string_len),
            BuiltinId::ReqGrpcMethod => checked_value_opt_str_ref(session.grpc_method(), self.limits.max_string_len),
            BuiltinId::ReqContentLength => match session.header_str("Content-Length") {
                Some(v) => match v.parse::<i64>() {
                    Ok(n) => Ok(Value::Int(n)),
                    Err(_) => Ok(Value::Nil),
                },
                None => Ok(Value::Nil),
            },
            BuiltinId::ReqClientCertCn => {
                let cn = session.client_cert_info().and_then(|c| c.cn.as_deref());
                checked_value_opt_str_ref(cn, self.limits.max_string_len)
            }
            BuiltinId::ReqClientCertFingerprint => {
                let fp = session.client_cert_info().map(|c| c.fingerprint.as_str());
                checked_value_opt_str_ref(fp, self.limits.max_string_len)
            }

            // ===== resp.* read (additional) =====
            BuiltinId::RespHeaderNames => {
                const MAX_HEADER_NAMES: usize = 256;
                let mut result: Vec<Arc<str>> = Vec::new();
                for name in session.response_header_map().keys().take(MAX_HEADER_NAMES) {
                    check_string_len(name.as_str(), self.limits.max_string_len)?;
                    result.push(Arc::from(name.as_str()));
                }
                Ok(Value::List(result))
            }
            BuiltinId::RespContentType => checked_value_opt_str_ref(
                session
                    .response_header_map()
                    .get("Content-Type")
                    .and_then(|value| value.to_str().ok()),
                self.limits.max_string_len,
            ),

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
                checked_value_opt_str_ref(session.ctx().get_ctx_var(&key), self.limits.max_string_len)
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
            // `skills/04-review/dsl/intentional-design/url-decode-form-semantics.md`.
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
                check_string_len(trimmed, self.limits.max_string_len)?;
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
                checked_value_str(id, self.limits.max_string_len)
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
                        let output_len = checked_join_output_len(
                            items.iter().map(|item| item.len()),
                            delim.len(),
                            self.limits.max_string_len,
                        )?;
                        let mut joined = String::with_capacity(output_len);
                        if let Some((first, rest)) = items.split_first() {
                            joined.push_str(first);
                            for item in rest {
                                joined.push_str(&delim);
                                joined.push_str(item);
                            }
                        }
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
                enum PreparedValue {
                    Shared(Arc<str>),
                    Coerced(String),
                }
                impl PreparedValue {
                    fn new(value: Value) -> Result<Self, RuntimeError> {
                        match value {
                            Value::Str(value) => Ok(Self::Shared(value)),
                            other => Ok(Self::Coerced(other.runtime_string()?)),
                        }
                    }

                    fn as_str(&self) -> &str {
                        match self {
                            Self::Shared(value) => value,
                            Self::Coerced(value) => value,
                        }
                    }
                }
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
                let mut values = Vec::with_capacity(value_count);
                for _ in 0..value_count {
                    // Complete every coercion before rendering, including Body
                    // errors after an argument that would overflow the log.
                    values.push(PreparedValue::new(state.pop()?)?);
                }
                values.reverse();
                let template = PreparedValue::new(state.pop()?)?;
                let log_args: Vec<crate::plugins::runtime::log::LogArg<'_>> = values
                    .iter()
                    .map(|s| crate::plugins::runtime::log::LogArg::Sensitive(s.as_str()))
                    .collect();
                log.push_fmt(template.as_str(), &log_args);
                Ok(Value::Nil)
            }

            BuiltinId::JsonRead
            | BuiltinId::JsonGetRaw
            | BuiltinId::JsonGetString
            | BuiltinId::JsonKeys
            | BuiltinId::JsonSetRaw
            | BuiltinId::JsonSetString
            | BuiltinId::JsonSetBody
            | BuiltinId::XmlParse
            | BuiltinId::XmlKind
            | BuiltinId::XmlName
            | BuiltinId::XmlNamespace
            | BuiltinId::XmlValue
            | BuiltinId::XmlChildCount
            | BuiltinId::XmlChild
            | BuiltinId::XmlFind
            | BuiltinId::XmlAttributeCount
            | BuiltinId::XmlAttribute
            | BuiltinId::XmlAttrValue
            | BuiltinId::XmlText
            | BuiltinId::XmlAttr
            | BuiltinId::XmlElement
            | BuiltinId::XmlStringify
            | BuiltinId::XmlSetBody => Err(RuntimeError::Internal {
                message: "XML builtin requires the XML dispatch boundary".into(),
            }),

            // http.call is dispatched asynchronously by the VM before reaching call_builtin
            BuiltinId::HttpCall => Err(RuntimeError::Internal {
                message: "HttpCall must be dispatched via call_builtin_http, not call_builtin".into(),
            }),

            // redis.* builtins are dispatched asynchronously by the VM before reaching call_builtin
            BuiltinId::RedisGet
            | BuiltinId::RedisGetBounded
            | BuiltinId::RedisSet
            | BuiltinId::RedisDel
            | BuiltinId::RedisExists
            | BuiltinId::RedisTtl
            | BuiltinId::RedisExpire
            | BuiltinId::RedisIncr
            | BuiltinId::RedisRateLimit
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
        let Ok(parsed_url) = reqwest::Url::parse(&url_str) else {
            return Err(RuntimeError::ApiError {
                function: "http.call".into(),
                message: format!("invalid URL: {}", url_str),
            });
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
            state
                .body_scan_budget
                .reserve_exact(tracking.len, physical_cap, runtime.max_body_scan_passes)?;
            builder = builder.body(body).header(reqwest::header::CONTENT_LENGTH, tracking.len);
            body_accounting = Some((tracking.len, tracking));
        } else if let Some(body) = body_str {
            if !body.is_empty() {
                builder = builder.body(body.to_string());
            }
        }

        let send_result = builder.send().await;
        if let Some((reserved, tracking)) = body_accounting {
            // A response head can arrive before an HTTP/2 request body has
            // finished. Release the snapshot independently of whether Hyper
            // polls its Body again: Hyper may be blocked on flow-control
            // capacity while holding one already-buffered chunk.
            let (refund, read_error) = settle_opaque_body_upload(&tracking, reserved).await;
            state.body_scan_budget.refund_exact(refund);
            if let Some(error) = read_error {
                return Err(body_access_error(error.to_string()));
            }
        }
        Ok(complete_http_call_response(
            send_result,
            runtime.max_response_body_bytes,
            self.limits.max_string_len,
            self.limits.max_map_size,
        )
        .await)
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
fn resolve_redis_client_with<T>(
    link_name: &str,
    runtime: &DslRuntime,
    max_string_len: usize,
    log: &mut PluginLog,
    get_client: impl FnOnce(&str) -> Option<T>,
) -> Result<T, Value> {
    let Some(ns_name) = runtime.redis_links.get(link_name) else {
        log.push_err("redis link unknown");
        return Err(redis_err(RedisFailure::UnknownLink, max_string_len));
    };
    match get_client(ns_name) {
        Some(c) => Ok(c),
        None => {
            log.push_err("redis provider not ready");
            Err(redis_err(RedisFailure::ProviderNotReady, max_string_len))
        }
    }
}

fn resolve_redis_client(
    link_name: &str,
    runtime: &DslRuntime,
    max_string_len: usize,
    log: &mut PluginLog,
) -> Result<Arc<RedisLinkClient>, Value> {
    resolve_redis_client_with(link_name, runtime, max_string_len, log, get_redis_client)
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
        log: &mut PluginLog,
    ) -> Result<Value, RuntimeError> {
        match id {
            // ===== Basic KV =====
            BuiltinId::RedisRateLimit => {
                // Reserve enough result capacity before a potentially mutating call.
                if self.limits.max_map_size < 5 {
                    return Err(RuntimeError::MapTooLarge {
                        size: 5,
                        limit: self.limits.max_map_size,
                    });
                }
                let cost = state.pop()?;
                let key = state.pop()?;
                let policy_name = state.pop()?;
                let link = state.pop()?;
                let (Value::Int(cost), Value::Str(key), Value::Str(policy_name), Value::Str(link)) =
                    (cost, key, policy_name, link)
                else {
                    return Ok(redis_err(RedisFailure::InvalidArgument, self.limits.max_string_len));
                };
                if key.len() > 4096 {
                    return Ok(redis_err(RedisFailure::InvalidArgument, self.limits.max_string_len));
                }
                let now_ms = chrono::Utc::now().timestamp_millis();
                let Ok(cost) = validate_rate_limit_call(cost, now_ms) else {
                    return Ok(redis_err(RedisFailure::InvalidArgument, self.limits.max_string_len));
                };
                let Some(policy) = runtime.rate_limits.policies.get(policy_name.as_ref()) else {
                    return Ok(redis_err(RedisFailure::UnknownPolicy, self.limits.max_string_len));
                };
                if policy.link != link.as_ref() {
                    return Ok(redis_err(RedisFailure::UnknownPolicy, self.limits.max_string_len));
                }
                let Some(ns_name) = runtime.redis_links.get(link.as_ref()) else {
                    log.push_err("redis link unknown");
                    return Ok(redis_err(RedisFailure::UnknownLink, self.limits.max_string_len));
                };
                let Some(client) = get_redis_client(ns_name) else {
                    log.push_err("redis provider not ready");
                    return Ok(redis_err(RedisFailure::ProviderNotReady, self.limits.max_string_len));
                };
                let check = runtime.rate_limits.limiter.eval_policy(
                    &client,
                    &policy.algorithm,
                    &policy.prefix,
                    0,
                    &key,
                    policy.rate,
                    policy.window_ms,
                    now_ms,
                    cost,
                );
                let result = match tokio::time::timeout(policy.timeout, check).await {
                    Ok(Ok(result)) => result,
                    Ok(Err(error)) => {
                        return Ok(redis_dependency_err(&error, self.limits.max_string_len, log));
                    }
                    Err(_) => {
                        log.push_err("redis timeout");
                        return Ok(redis_err(RedisFailure::Timeout, self.limits.max_string_len));
                    }
                };
                // Reject inconsistent values instead of exposing unbounded results
                // from a Redis key modified outside this policy's ownership.
                let limit = policy.rate as i64;
                if !(0..=limit).contains(&result.current)
                    || !(0..=limit).contains(&result.remaining)
                    || !(0..=i64::from(cost) * policy.window_ms).contains(&result.retry_ms)
                {
                    log.push_err("redis invalid dependency result");
                    return Ok(redis_err(RedisFailure::DependencyError, self.limits.max_string_len));
                }
                let mut decision = IndexMap::with_capacity(5);
                decision.insert("allowed".into(), Value::Bool(result.allowed));
                decision.insert("current".into(), Value::Int(result.current));
                decision.insert("remaining".into(), Value::Int(result.remaining));
                decision.insert("limit".into(), Value::Int(limit));
                decision.insert("retry_ms".into(), Value::Int(result.retry_ms));
                Ok(redis_ok(Value::Map(Box::new(decision))))
            }
            BuiltinId::RedisGetBounded => {
                use crate::link_sys::providers::redis::ops::{BoundedGet, BoundedGetError};
                let requested = state.pop()?;
                let key = state.pop()?;
                let link = state.pop()?;
                let Some(cap) = bounded_redis_get_limit(&requested, self.limits.max_string_len) else {
                    return Ok(redis_err(RedisFailure::InvalidArgument, self.limits.max_string_len));
                };
                let key = key.runtime_string()?;
                let link = link.runtime_string()?;
                let client = match resolve_redis_client(&link, runtime, self.limits.max_string_len, log) {
                    Ok(client) => client,
                    Err(result) => return Ok(result),
                };
                match client.get_bounded(&key, cap).await {
                    Ok(BoundedGet::Missing) => Ok(redis_ok(Value::Nil)),
                    Ok(BoundedGet::TooLarge) => Ok(redis_err(RedisFailure::ResultTooLarge, self.limits.max_string_len)),
                    Ok(BoundedGet::Value(bytes)) => {
                        // Provider preflight checked UTF-8 without allocating an owned String.
                        let text = std::str::from_utf8(&bytes).map_err(|_| RuntimeError::Internal {
                            message: "admitted Redis text is invalid".into(),
                        })?;
                        state.value_work_budget.charge(text.len() as u64 * 2)?;
                        Ok(redis_ok(Value::Str(Arc::from(text))))
                    }
                    Err(BoundedGetError::Timeout) => Ok(redis_err(RedisFailure::Timeout, self.limits.max_string_len)),
                    Err(BoundedGetError::InvalidResponse) => {
                        Ok(redis_err(RedisFailure::InvalidResponse, self.limits.max_string_len))
                    }
                    Err(BoundedGetError::Dependency(error)) => Ok(redis_dependency_err(
                        &anyhow::Error::new(error),
                        self.limits.max_string_len,
                        log,
                    )),
                }
            }
            BuiltinId::RedisGet => {
                // redis.get(link, key)
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
                let client = match resolve_redis_client(&link, runtime, self.limits.max_string_len, log) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client.get(&key).await {
                    Ok(Some(v)) => {
                        let v = truncate_to_max_string_len(v, self.limits.max_string_len);
                        Ok(redis_ok(Value::Str(v.into())))
                    }
                    Ok(None) => Ok(redis_ok(Value::Nil)),
                    Err(error) => Ok(redis_dependency_err(&error, self.limits.max_string_len, log)),
                }
            }
            BuiltinId::RedisSet => {
                // redis.set(link, key, value) or redis.set(link, key, value, ttl_ms)
                if !(3..=4).contains(&(argc as usize)) {
                    return Ok(redis_err(RedisFailure::InvalidArgument, self.limits.max_string_len));
                }
                let ttl_ms_val = if argc == 4 { Some(state.pop()?) } else { None };
                let value = state.pop()?.runtime_string()?;
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
                let client = match resolve_redis_client(&link, runtime, self.limits.max_string_len, log) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                let result = match ttl_ms_val {
                    None => client.set(&key, &value, None).await,
                    Some(Value::Nil) => client.set(&key, &value, None).await,
                    Some(Value::Int(ms)) if ms > 0 => client.set_px(&key, &value, ms as u64).await,
                    Some(Value::Int(_)) => client.set(&key, &value, None).await,
                    Some(_) => {
                        return Ok(redis_err(RedisFailure::InvalidArgument, self.limits.max_string_len));
                    }
                };
                match result {
                    Ok(()) => Ok(redis_ok(Value::Bool(true))),
                    Err(error) => Ok(redis_dependency_err(&error, self.limits.max_string_len, log)),
                }
            }
            BuiltinId::RedisDel => {
                // redis.del(link, key)
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
                let client = match resolve_redis_client(&link, runtime, self.limits.max_string_len, log) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client.del(&[key.as_str()]).await {
                    Ok(count) => Ok(redis_ok(Value::Bool(count > 0))),
                    Err(error) => Ok(redis_dependency_err(&error, self.limits.max_string_len, log)),
                }
            }
            BuiltinId::RedisExists => {
                // redis.exists(link, key)
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
                let client = match resolve_redis_client(&link, runtime, self.limits.max_string_len, log) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client.exists(&key).await {
                    Ok(b) => Ok(redis_ok(Value::Bool(b))),
                    Err(error) => Ok(redis_dependency_err(&error, self.limits.max_string_len, log)),
                }
            }
            BuiltinId::RedisTtl => {
                // redis.ttl(link, key)
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
                let client = match resolve_redis_client(&link, runtime, self.limits.max_string_len, log) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                // TTL: -1 = no expiry, -2 = key not found; ok=false only on connection error
                match client.ttl(&key).await {
                    Ok(v) => Ok(redis_ok(Value::Int(v))),
                    Err(error) => Ok(redis_dependency_err(&error, self.limits.max_string_len, log)),
                }
            }
            BuiltinId::RedisExpire => {
                // redis.expire(link, key, seconds)
                let seconds_val = state.pop()?;
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
                let seconds = match &seconds_val {
                    Value::Int(n) => *n,
                    _ => {
                        return Ok(redis_err(RedisFailure::InvalidArgument, self.limits.max_string_len));
                    }
                };
                let client = match resolve_redis_client(&link, runtime, self.limits.max_string_len, log) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client.expire(&key, seconds).await {
                    Ok(b) => Ok(redis_ok(Value::Bool(b))),
                    Err(error) => Ok(redis_dependency_err(&error, self.limits.max_string_len, log)),
                }
            }
            BuiltinId::RedisIncr => {
                // redis.incr(link, key)
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
                let client = match resolve_redis_client(&link, runtime, self.limits.max_string_len, log) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client.incr(&key).await {
                    Ok(v) => Ok(redis_ok(Value::Int(v))),
                    Err(error) => Ok(redis_dependency_err(&error, self.limits.max_string_len, log)),
                }
            }
            BuiltinId::RedisIncrBy => {
                // redis.incr_by(link, key, delta)
                let delta_val = state.pop()?;
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
                let delta = match &delta_val {
                    Value::Int(n) => *n,
                    _ => {
                        return Ok(redis_err(RedisFailure::InvalidArgument, self.limits.max_string_len));
                    }
                };
                let client = match resolve_redis_client(&link, runtime, self.limits.max_string_len, log) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client.incr_by(&key, delta).await {
                    Ok(v) => Ok(redis_ok(Value::Int(v))),
                    Err(error) => Ok(redis_dependency_err(&error, self.limits.max_string_len, log)),
                }
            }

            // ===== Hash =====
            BuiltinId::RedisHGet => {
                // redis.hget(link, key, field)
                let field = state.pop()?.runtime_string()?;
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
                let client = match resolve_redis_client(&link, runtime, self.limits.max_string_len, log) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client.hget(&key, &field).await {
                    Ok(Some(v)) => {
                        let v = truncate_to_max_string_len(v, self.limits.max_string_len);
                        Ok(redis_ok(Value::Str(v.into())))
                    }
                    Ok(None) => Ok(redis_ok(Value::Nil)),
                    Err(error) => Ok(redis_dependency_err(&error, self.limits.max_string_len, log)),
                }
            }
            BuiltinId::RedisHSet => {
                // redis.hset(link, key, field, value)
                let val = state.pop()?.runtime_string()?;
                let field = state.pop()?.runtime_string()?;
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
                let client = match resolve_redis_client(&link, runtime, self.limits.max_string_len, log) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client.hset(&key, &field, &val).await {
                    Ok(_count) => Ok(redis_ok(Value::Bool(true))),
                    Err(error) => Ok(redis_dependency_err(&error, self.limits.max_string_len, log)),
                }
            }
            BuiltinId::RedisHDel => {
                // redis.hdel(link, key, field)
                let field = state.pop()?.runtime_string()?;
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
                let client = match resolve_redis_client(&link, runtime, self.limits.max_string_len, log) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client.hdel(&key, &[field.as_str()]).await {
                    Ok(count) => Ok(redis_ok(Value::Bool(count > 0))),
                    Err(error) => Ok(redis_dependency_err(&error, self.limits.max_string_len, log)),
                }
            }
            BuiltinId::RedisHGetAll => {
                // redis.hgetall(link, key)
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
                let client = match resolve_redis_client(&link, runtime, self.limits.max_string_len, log) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client
                    .hgetall_bounded(&key, self.limits.max_map_size, MAX_DSL_HOST_RESULT_BYTES)
                    .await
                {
                    Ok(map) => Ok(redis_hash_result(map, self.limits.max_string_len, log)),
                    Err(error) => Ok(redis_dependency_err(&error, self.limits.max_string_len, log)),
                }
            }
            BuiltinId::RedisHExists => {
                // redis.hexists(link, key, field)
                let field = state.pop()?.runtime_string()?;
                let key = state.pop()?.runtime_string()?;
                let link = state.pop()?.runtime_string()?;
                let client = match resolve_redis_client(&link, runtime, self.limits.max_string_len, log) {
                    Ok(c) => c,
                    Err(e) => return Ok(e),
                };
                match client.hexists(&key, &field).await {
                    Ok(b) => Ok(redis_ok(Value::Bool(b))),
                    Err(error) => Ok(redis_dependency_err(&error, self.limits.max_string_len, log)),
                }
            }

            // All non-Redis builtins are dispatched via call_builtin / call_builtin_http and
            // must not reach here. Listed explicitly (no `_ =>` catch-all) so adding a new
            // BuiltinId variant fails to compile until it is routed to the right dispatcher.
            BuiltinId::JsonRead
            | BuiltinId::JsonGetRaw
            | BuiltinId::JsonGetString
            | BuiltinId::JsonKeys
            | BuiltinId::JsonSetRaw
            | BuiltinId::JsonSetString
            | BuiltinId::JsonSetBody
            | BuiltinId::XmlParse
            | BuiltinId::XmlKind
            | BuiltinId::XmlName
            | BuiltinId::XmlNamespace
            | BuiltinId::XmlValue
            | BuiltinId::XmlChildCount
            | BuiltinId::XmlChild
            | BuiltinId::XmlFind
            | BuiltinId::XmlAttributeCount
            | BuiltinId::XmlAttribute
            | BuiltinId::XmlAttrValue
            | BuiltinId::XmlText
            | BuiltinId::XmlAttr
            | BuiltinId::XmlElement
            | BuiltinId::XmlStringify
            | BuiltinId::XmlSetBody
            | BuiltinId::ReqHeader
            | BuiltinId::ReqMethod
            | BuiltinId::ReqRequestMethod
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
#[path = "../../../../../tests/unit/plugins/http/dsl/lang/builtins_test/redis_client_lookup_tests.rs"]
mod redis_client_lookup_tests;

#[cfg(test)]
#[path = "../../../../../tests/unit/plugins/http/dsl/lang/builtins_test/ctx_value_tests.rs"]
mod ctx_value_tests;

#[cfg(test)]
#[path = "../../../../../tests/unit/plugins/http/dsl/lang/builtins_test/ip_tests.rs"]
mod ip_tests;

#[cfg(test)]
#[path = "../../../../../tests/unit/plugins/http/dsl/lang/builtins_test/join_length_tests.rs"]
mod join_length_tests;

#[cfg(test)]
#[path = "../../../../../tests/unit/plugins/http/dsl/lang/builtins_test/truncate_tests.rs"]
mod truncate_tests;

#[cfg(test)]
#[path = "../../../../../tests/unit/plugins/http/dsl/lang/builtins_test/redis_error_tests.rs"]
mod redis_error_tests;

#[cfg(test)]
#[path = "../../../../../tests/unit/plugins/http/dsl/lang/builtins_test/body_tests.rs"]
mod body_tests;

#[cfg(test)]
#[path = "../../../../../tests/unit/plugins/http/dsl/lang/builtins_test/opaque_body_stream_tests.rs"]
mod opaque_body_stream_tests;
