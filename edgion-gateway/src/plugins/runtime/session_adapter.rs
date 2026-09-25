use async_trait::async_trait;
use bytes::Bytes;
use edgion_resources::response_header_policy::validate_response_header_mutation;
use http::Uri;
use pingora_core::protocols::http::RequestBodyBuffer;
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::Session;
use std::collections::HashMap;
use std::sync::Arc;

use super::conditions::EvaluationResult;
use super::dye_plan::{CompiledDyePlan, CompiledDyeRule};
use super::log::{EdgionPluginsLogToken, PluginLog};
use super::traits::session::{
    empty_response_header_map, BodyReadError, BodyReadResult, RequestBodySnapshotReader, RouteProto,
};
use super::traits::{DyeOutcome, PluginSession, PluginSessionError, PluginSessionResult};
use crate::ai::{AiAttemptPlan, AiRequestContext};
use crate::link_sys::template::evaluate_template;
use crate::request_body::BodyStorage;
use crate::request_body::{BodyOperationError, BodyScanResult};
use crate::runtime::server::{http_to_grpc_status, send_grpc_trailers_only_frame};
use crate::{EdgionHttpContext, MirrorState, PendingHeaderOp, ResponseStageDenial, SetJumpOutcome, UpstreamJumpTarget};
use edgion_resources::common::{KeyGet, KeySet};
#[cfg(test)]
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::sanitize_field_value;
use edgion_resources::EdgionErrorCode;

fn disable_h1_reuse_after_capture_failure(downstream: &mut pingora_core::protocols::http::server::Session) {
    // `set_keepalive` disables reuse for H1 and is deliberately a no-op for
    // H2, where dropping the unfinished request resets only that stream.
    downstream.set_keepalive(None);
}

fn reject_over_limit_request_body(
    downstream: &mut pingora_core::protocols::http::server::Session,
    limit: u64,
) -> BodyReadError {
    // HTTP/1.x cannot be reused with unread request bytes. For HTTP/2 this
    // is intentionally a no-op: after the 413 response closes its send
    // side, dropping the request session resets only the unfinished stream.
    downstream.set_keepalive(None);
    BodyReadError::BufferLimitExceeded { limit }
}

fn reject_capture_storage_failure(downstream: &mut pingora_core::protocols::http::server::Session) -> BodyReadError {
    // Capture can fail before the downstream body reaches EOS. H1 must not
    // parse those unread bytes as a new request; for H2 this only affects
    // the current request stream and the connection remains multiplexable.
    disable_h1_reuse_after_capture_failure(downstream);
    BodyReadError::StorageFailure
}

async fn capture_request_body_core(
    downstream: &mut pingora_core::protocols::http::server::Session,
    ctx: &mut EdgionHttpContext,
    max_bytes: Option<u64>,
) -> Result<u64, BodyReadError> {
    if let Some(len) = ctx.request_body.cached_len()? {
        return Ok(len);
    }
    if !ctx.request_body_config.enabled {
        return Err(BodyReadError::Unimplemented);
    }

    let cap = resolve_cap(max_bytes, ctx.request_body_config.max_body_size_bytes);
    if downstream
        .req_header()
        .headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|declared| declared > cap)
    {
        return Err(reject_over_limit_request_body(downstream, cap));
    }
    let expect_continue = downstream.req_header().version == http::Version::HTTP_11
        && downstream
            .req_header()
            .headers
            .get(http::header::EXPECT)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("100-continue"));
    if expect_continue {
        downstream
            .write_continue_response()
            .await
            .map_err(|_| BodyReadError::Incomplete)?;
        // This hop has accepted the expectation. Do not forward it and
        // provoke a second 100 response from the upstream.
        downstream.req_header_mut().remove_header(&http::header::EXPECT);
    }

    let (storage, view) = BodyStorage::with_storage_timeout(
        ctx.request_body_config.memory_buffer_bytes,
        ctx.request_body_config.max_body_size_bytes,
        ctx.request_body_config.storage_operation_timeout,
    );
    downstream
        .set_request_body_buffer(Box::new(storage))
        .map_err(|_| body_buffer_install_error())?;

    let mut captured = 0u64;
    loop {
        let chunk = match downstream.read_request_body().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(err) => {
                return Err(match err.etype() {
                    pingora_core::ErrorType::ReadTimedout => BodyReadError::Timeout,
                    pingora_core::ErrorType::HTTPStatus(413) => reject_over_limit_request_body(downstream, cap),
                    // Gateway-local capture failure (spill-file IO, capture
                    // poisoned mid-chunk). Kept apart from the client-caused
                    // `Incomplete` so the access log can alert on it.
                    pingora_core::ErrorType::InternalError => reject_capture_storage_failure(downstream),
                    _ => BodyReadError::Incomplete,
                });
            }
        };
        captured = match captured.checked_add(chunk.len() as u64) {
            Some(total) => total,
            None => return Err(reject_over_limit_request_body(downstream, cap)),
        };
        if captured > cap {
            return Err(reject_over_limit_request_body(downstream, cap));
        }
    }

    let len = view.len()?;
    ctx.request_body.install(view);
    Ok(len)
}

/// Resolve a `${matchedHttpRoute:<selector>}` / `${matchedGrpcRoute:<selector>}`
/// selector against route metadata.
/// Selectors: `name`, `namespace`, `label.<key>`, `annotation.<key>`.
/// The split is on the FIRST `.` so keys containing `.`/`/`
/// (e.g. `edgion.io/team`) round-trip. Absent/unknown -> None.
pub(crate) fn route_meta_field(meta: &kube::api::ObjectMeta, selector: &str) -> Option<String> {
    match selector {
        "name" => meta.name.clone(),
        "namespace" => meta.namespace.clone(),
        _ => {
            if let Some(key) = selector.strip_prefix("label.") {
                meta.labels.as_ref()?.get(key).cloned()
            } else if let Some(key) = selector.strip_prefix("annotation.") {
                meta.annotations.as_ref()?.get(key).cloned()
            } else {
                None
            }
        }
    }
}

/// Borrow one cookie value from all request `Cookie` fields in field order.
///
/// An invalid UTF-8 field reached before a match makes the lookup fail rather
/// than allowing a later field to override an uninterpretable credential. A
/// match returns immediately, so malformed fields after it are irrelevant.
pub(crate) fn cookie_value_ref<'a>(headers: &'a http::HeaderMap, name: &str) -> Option<&'a str> {
    for header in headers.get_all(http::header::COOKIE).iter() {
        let cookies = header.to_str().ok()?;
        if let Some(value) = cookies.split(';').find_map(|pair| {
            let mut parts = pair.trim().splitn(2, '=');
            let key = parts.next()?;
            if key == name {
                parts.next()
            } else {
                None
            }
        }) {
            return Some(value);
        }
    }
    None
}

/// Read one cookie from a request's `Cookie` headers.
///
/// Free function so both `PluginSession::get_cookie` and the access-log `ext`
/// collector share one parser — the collector cannot build a
/// `PingoraSessionAdapter` (that needs `&mut` borrows the logging stage does not
/// have).
pub fn cookie_value(headers: &http::HeaderMap, name: &str) -> Option<String> {
    cookie_value_ref(headers, name).map(str::to_owned)
}

/// Return every value for one header, preserving duplicate occurrences.
///
/// `None` reports that at least one matching value is not valid UTF-8. This
/// prevents security-sensitive consumers from silently treating an invalid
/// duplicate as absent.
fn utf8_header_values(headers: &http::HeaderMap, name: &str) -> Option<Vec<String>> {
    headers
        .get_all(name)
        .iter()
        .map(|value| value.to_str().ok().map(str::to_owned))
        .collect()
}

/// Resolve request identity keys from the immutable entry-time request snapshot.
///
/// Callers must pass only the variants handled here. Keeping this independent
/// from `Session` makes the trust-boundary distinction unit-testable.
fn request_identity_key_core(ctx: &EdgionHttpContext, key: &KeyGet) -> Option<String> {
    let value = match key {
        KeyGet::DirectPeerIp => ctx.request_info.direct_peer_ip.as_str(),
        KeyGet::Host => ctx.request_info.hostname.as_str(),
        _ => unreachable!("request_identity_key_core called with a non-identity key"),
    };
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/plugins/runtime/session_adapter_test/request_identity_key_tests.rs"]
mod request_identity_key_tests;

/// Pure core of the once-per-name request-direction anti-spoof strip.
///
/// For each request-direction rule in `plan`, removes the client-supplied
/// header the first time any entry declares that name (recorded into
/// `handled`, matched case-insensitively) and never strips it again — a
/// value written between two installs by an earlier entry (a gateway-
/// rendered dye value) is left intact for the next entry to see. Free
/// function over a plain `RequestHeader` + the handled-names list — no
/// `PingoraSessionAdapter` / `Session` needed — so it is unit-testable
/// without standing up a full pingora session.
pub(crate) fn strip_request_dye_once_core(
    req_header: &mut RequestHeader,
    plan: &CompiledDyePlan,
    handled: &mut Vec<String>,
) {
    for rule in &plan.request {
        let already_handled = handled.iter().any(|n| n.eq_ignore_ascii_case(&rule.name));
        if !already_handled {
            req_header.remove_header(&rule.name);
            handled.push(rule.name.clone());
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/plugins/runtime/session_adapter_test/backend_ext_ref_pin_tests.rs"]
mod backend_ext_ref_pin_tests;

pub struct PingoraSessionAdapter<'a> {
    inner: &'a mut Session,
    ctx: &'a mut EdgionHttpContext,
    response_header: Option<&'a mut ResponseHeader>,
    /// Callback-local identity used for first-wins terminal arbitration.
    response_processor_slot: Option<crate::ai::terminal::ResponseProcessorSlot>,
    /// Set to true after a gRPC trailers-only error response has been sent.
    /// Subsequent write_response_body and shutdown calls become no-ops.
    grpc_response_terminated: bool,
    /// Prevent repeated header/body failures from emitting duplicate transport
    /// diagnostics during one adapter-owned plugin stage.
    delivery_failure_recorded: bool,
    /// Adapter-local permission for a read-only mirror tee during the first
    /// backend request dispatch, before Pingora starts delivering body events.
    backend_mirror_allowed: bool,
}

fn backend_mirror_allowed_for_attempt(attempt: Option<usize>) -> bool {
    attempt == Some(1)
}

/// Build a fresh local-reply `ResponseHeader` + body and stash them in `ctx`
/// for `response_filter` to write downstream. Pure/synchronous and
/// unit-testable; refuses gRPC routes (Upgrade/1xx are refused by the caller,
/// `apply_response_local_reply`, which has the session + upstream header).
///
/// `is_head` or a 204/304 status makes the reply body-less (no content-length,
/// empty stashed body).
pub(crate) fn build_response_local_reply(
    ctx: &mut EdgionHttpContext,
    status: u16,
    body: Option<Bytes>,
    is_head: bool,
) -> bool {
    if ctx.request_info.is_grpc_request || ctx.is_grpc_route_matched {
        return false;
    }
    let Ok(code) = http::StatusCode::from_u16(status) else {
        return false;
    };
    let bodyless = is_head || matches!(status, 204 | 304);
    let Ok(mut fresh) = ResponseHeader::build(code, None) else {
        return false;
    };
    let body_bytes = body.unwrap_or_default();
    if !bodyless {
        let _ = fresh.insert_header("content-type", "text/plain");
    }
    // Apply queued plugin header ops, then re-assert framing so a queued
    // Set/Remove Content-Length cannot desync the wire from the body.
    ctx.drain_pending_response_header_ops(&mut fresh);
    ctx.request_info.trace_id.reconcile_response(&mut fresh);
    if !bodyless {
        let _ = fresh.remove_header(&http::header::TRANSFER_ENCODING);
        let _ = fresh.insert_header("content-length", body_bytes.len().to_string());
    }
    ctx.stage_response_local_reply(Box::new(fresh), if bodyless { Bytes::new() } else { body_bytes });
    true
}

impl<'a> PingoraSessionAdapter<'a> {
    pub(crate) fn set_response_processor_slot(&mut self, slot: crate::ai::terminal::ResponseProcessorSlot) {
        self.response_processor_slot = Some(slot);
    }

    pub(crate) fn response_terminal_winner(&self) -> Option<crate::ai::terminal::ResponseProcessorSlot> {
        self.ctx.response_terminal_state.winner()
    }

    #[inline]
    pub fn new(inner: &'a mut Session, ctx: &'a mut EdgionHttpContext) -> Self {
        Self {
            inner,
            ctx,
            response_header: None,
            response_processor_slot: None,
            grpc_response_terminated: false,
            delivery_failure_recorded: false,
            backend_mirror_allowed: false,
        }
    }

    /// Only the backend request dispatcher may use this constructor. Its first
    /// canonical attempt runs before the body pump. Mirror observation changes
    /// neither framing nor replay policy; all other body capabilities remain
    /// subject to the request-stage freeze. Dropping this adapter revokes the
    /// permission, including when plugin execution is cancelled.
    pub(super) fn for_backend_request(inner: &'a mut Session, ctx: &'a mut EdgionHttpContext) -> Self {
        let allowed = backend_mirror_allowed_for_attempt(inner.request_attempt_id().map(|id| id.get()));
        let mut adapter = Self::new(inner, ctx);
        adapter.backend_mirror_allowed = allowed;
        adapter
    }

    #[inline]
    pub fn with_response_header(
        inner: &'a mut Session,
        ctx: &'a mut EdgionHttpContext,
        response_header: &'a mut ResponseHeader,
    ) -> Self {
        Self {
            inner,
            ctx,
            response_header: Some(response_header),
            response_processor_slot: None,
            grpc_response_terminated: false,
            delivery_failure_recorded: false,
            backend_mirror_allowed: false,
        }
    }

    #[inline]
    fn observe_delivery<T>(&mut self, result: PluginSessionResult<T>) -> PluginSessionResult<T> {
        if result.is_err() && !self.delivery_failure_recorded {
            self.ctx.record_error(EdgionErrorCode::ClientClosed);
            self.delivery_failure_recorded = true;
        }
        result
    }

    #[inline]
    pub fn set_terminate(&mut self) {
        self.ctx.terminate_plugin_execution();
    }

    async fn ensure_bodyless_request_replay_owner(&mut self) -> Result<(), BodyReadError> {
        if self.ctx.waf_prefix_inspected {
            return Err(BodyReadError::NotBuffered);
        }
        if !self.ctx.request_body_config.enabled {
            return Err(BodyReadError::Unimplemented);
        }
        if !self.ctx.request_info.in_request_body_stage {
            self.ctx.record_error(EdgionErrorCode::BodyRefusedStage);
            return Err(BodyReadError::NotBuffered);
        }
        if self.ctx.request_body.has_cached_body() {
            return Ok(());
        }
        if self.ctx.request_info.discover_protocol.as_deref() == Some("websocket")
            || self.inner.req_header().method == http::Method::CONNECT
        {
            self.ctx.record_error(EdgionErrorCode::BodyRefusedWebsocket);
            return Err(BodyReadError::NotBuffered);
        }
        if self.ctx.request_info.is_grpc_request || self.ctx.is_grpc_route_matched {
            self.ctx.record_error(EdgionErrorCode::BodyRefusedGrpc);
            return Err(BodyReadError::NotBuffered);
        }
        if self.ctx.request_info.body_declared != Some(false) {
            return Err(BodyReadError::Incomplete);
        }

        let (mut storage, view) = BodyStorage::with_storage_timeout(
            self.ctx.request_body_config.memory_buffer_bytes,
            self.ctx.request_body_config.max_body_size_bytes,
            self.ctx.request_body_config.storage_operation_timeout,
        );
        storage.finish().await.map_err(|_| BodyReadError::Incomplete)?;
        self.inner
            .set_bodyless_request_replay_buffer(Box::new(storage))
            .map_err(|_| BodyReadError::Incomplete)?;
        self.ctx.request_body.install(view);
        Ok(())
    }

    async fn capture_request_body(&mut self, max_bytes: Option<u64>) -> Result<u64, BodyReadError> {
        capture_request_body_core(self.inner.as_mut(), self.ctx, max_bytes).await
    }

    /// Decide + stash a response-phase local reply. Returns `true` when a reply
    /// was stashed (HTTP route, not a switched-protocol/informational response).
    /// Refuses: gRPC route, 1xx/101 upstream response, an established CONNECT
    /// tunnel, or no bound response header. The actual downstream write happens
    /// later in `response_filter`.
    ///
    /// Whether the exchange is still a replaceable HTTP response is decided from
    /// the RESPONSE, not from the request's stated intent. A client may send
    /// `Upgrade: <anything>` on an ordinary request; unless the upstream answers
    /// `101` no protocol switch happened, the response header is still in hand
    /// (nothing has been written downstream at either response-stage entry
    /// point — response-write ownership is only claimed by final writer paths),
    /// and a plugin's denial must be able to replace it. Keying the refusal on
    /// the request header instead let any request carrying an unrecognized
    /// `Upgrade` token drop a plugin's `ErrResponse` on the floor and forward the
    /// upstream response unmodified. `CONNECT` is refused only once the tunnel is
    /// actually established (2xx); an upstream-rejected CONNECT is an ordinary
    /// replaceable response.
    pub fn apply_response_local_reply(&mut self, status: u16, body: Option<Bytes>) -> bool {
        let Some(resp_status) = self.response_header.as_deref().map(|h| h.status) else {
            return false;
        };
        let is_head = self.inner.req_header().method == http::Method::HEAD;
        // `is_informational()` covers 100..=199, so a real `101 Switching
        // Protocols` is refused by this arm too.
        let switched = resp_status.is_informational()
            || (self.inner.req_header().method == http::Method::CONNECT && resp_status.is_success());
        if switched {
            return false;
        }
        build_response_local_reply(self.ctx, status, body, is_head)
    }

    /// Record that a request-phase `ErrResponse` body on a gRPC route was not
    /// valid UTF-8 and so could not be carried as `grpc-message`.
    ///
    /// Not a termination marker — the denial still goes out with the mapped
    /// gRPC status. This records what the translation cost: the plugin's body
    /// is replaced by the status-mapped default message. Without it the drop is
    /// invisible, and the access log cannot tell a dropped binary body apart
    /// from a plugin that returned no body at all.
    pub fn note_grpc_message_body_not_utf8(&mut self) {
        self.ctx.record_error(EdgionErrorCode::PluginGrpcMessageBodyNotUtf8);
    }

    /// Record that a response-phase `ErrResponse` on a gRPC route could not be
    /// expressed as a response replacement.
    ///
    /// Not a termination marker — [`Self::set_response_denied`] is what enforces
    /// the denial, and both are recorded together. This one narrows the access
    /// log to what the gRPC translation cost: the plugin's body is dropped and
    /// its status only survives as far as `http_to_grpc_status` maps it.
    pub fn note_response_local_reply_unsupported(&mut self) {
        self.ctx
            .record_error(EdgionErrorCode::PluginResponseReplaceUnsupportedGrpc);
    }

    /// Arm the response-stage abort for an `ErrResponse` that
    /// [`Self::apply_response_local_reply`] refused to stash.
    ///
    /// A refusal means "this exchange cannot carry a replacement response", NOT
    /// "let the upstream response through". The response-stage entry point reads
    /// the context's response denial after the plugin chains and aborts,
    /// so the denial is downstream-visible.
    ///
    /// The access-log status is overwritten with the plugin's status on purpose:
    /// `upstream_response_filter` already stamped the upstream's own status, so
    /// `fail_to_proxy`'s `status.is_none()` fill-in would never fire and the log
    /// would attribute the request to the response the client never received.
    pub fn set_response_denied(&mut self, status: u16) {
        self.ctx.select_final_downstream_status(status);
        self.ctx.set_response_denial(ResponseStageDenial::Http(status));
        self.ctx.record_error(EdgionErrorCode::PluginResponseDenyAborted);
    }

    /// Arm the response-stage abort for a naked `ErrTerminateRequest`.
    ///
    /// Mirrors the request stage's contract for the same result
    /// (`error_response::terminate_with_fallback`): a plugin that terminates
    /// without writing a response is a plugin bug, answered with a 502 fallback
    /// and [`EdgionErrorCode::PluginTerminateWithoutResponse`]. The response stage
    /// had no equivalent — it returned "terminate" and the upstream response was
    /// forwarded anyway, unconditionally, with no marker at all.
    pub fn set_response_terminate_without_reply(&mut self) {
        self.ctx.record_error(EdgionErrorCode::PluginTerminateWithoutResponse);
        self.set_response_denied(502);
    }

    /// Arm the response-stage abort for an `ErrGrpcError`.
    ///
    /// The response-stage runners have no gRPC writer: the sync one cannot await
    /// [`Self::send_grpc_error_direct`], which is why the request stage's
    /// treatment of this result has no response-stage counterpart. Carrying the
    /// gRPC status verbatim keeps `fail_to_proxy` from having to reconstruct it
    /// from an HTTP status through the lossy `http_to_grpc_status` mapping.
    ///
    /// The wire status of a gRPC error is 200 (the error lives in the trailers),
    /// so that — not the gRPC code — is what the access log records.
    pub fn set_response_denied_grpc(&mut self, status: u16, message: Option<String>) {
        self.ctx.select_final_downstream_status(200);
        self.ctx.request_info.set_grpc_status(status);
        self.ctx
            .set_response_denial(ResponseStageDenial::Grpc { status, message });
        self.ctx.record_error(EdgionErrorCode::PluginResponseDenyAborted);
    }

    /// Whether the in-flight request is gRPC/gRPC-Web. Used by the response
    /// runner to record the gRPC-unsupported marker only for genuine gRPC
    /// routes (other local-reply refusals — Upgrade/1xx — are not gRPC).
    pub fn is_grpc_request(&self) -> bool {
        self.ctx.request_info.is_grpc_request || self.ctx.is_grpc_route_matched
    }

    /// Apply queued response-header ops onto a freshly-built `ResponseHeader`.
    /// Used by request-stage termination paths (ErrResponse) that build their
    /// own response and would otherwise drop plugin-queued headers on the floor.
    #[inline]
    pub fn drain_pending_response_header_ops(&mut self, response_header: &mut ResponseHeader) {
        self.ctx.drain_pending_response_header_ops(response_header);
    }

    /// Resolve one dye rule's rendered value: an operator template wins over
    /// the recorded value; a `Success` recorded value is sanitized verbatim
    /// (no provenance prefix — that's the "selection value" contract); every
    /// other outcome is prefixed `<type_prefix>:<outcome>:<cause>` so a reader
    /// can tell a gateway-recorded failure from an operator-authored value at
    /// a glance. Renders BEFORE any mutation of `self` so the immutable
    /// `evaluate_template` borrow never overlaps the later `&mut self` header
    /// write.
    fn dye_rule_value(&mut self, rule: &CompiledDyeRule, prefix: &str, outcome: DyeOutcome, raw: &str) -> String {
        match &rule.value {
            Some(tpl) => sanitize_field_value(&evaluate_template(tpl, self)),
            None if outcome == DyeOutcome::Success => sanitize_field_value(raw),
            None => format!("{}:{}:{}", prefix, outcome.as_str(), sanitize_field_value(raw)),
        }
    }

    /// Send a gRPC error response with user-specified grpc-status and grpc-message using
    /// the Trailers-Only single-frame form: one HEADERS frame with END_STREAM set, carrying
    /// grpc-status (and grpc-message if provided) as ordinary header fields alongside the
    /// HTTP/2 pseudo-headers and content-type. No DATA frame is emitted.
    pub async fn send_grpc_error_direct(&mut self, grpc_status: u16, message: Option<&str>) -> PluginSessionResult<()> {
        if !self.ctx.claim_final_response(200) {
            return Err(Box::new(std::io::Error::other("final response writer already claimed")));
        }
        self.grpc_response_terminated = true;
        self.ctx.request_info.set_grpc_status(grpc_status);

        let mut resp = ResponseHeader::build(200, None).map_err(|e| Box::new(e) as PluginSessionError)?;
        // Apply queued ops first so request-stage plugins (e.g. cookie issuance) still land
        // their headers, then strip framing headers and re-assert content-type LAST so a queued
        // Set cannot break the gRPC contract. On a body-less single HEADERS+END_STREAM frame a
        // queued content-length / transfer-encoding would be a framing inconsistency strict
        // clients reject, so remove them after draining.
        self.ctx.drain_pending_response_header_ops(&mut resp);
        self.ctx.request_info.trace_id.reconcile_response(&mut resp);
        resp.remove_header(&http::header::CONTENT_LENGTH);
        resp.remove_header(&http::header::TRANSFER_ENCODING);
        resp.insert_header("content-type", "application/grpc")
            .map_err(|e| Box::new(e) as PluginSessionError)?;
        let result = send_grpc_trailers_only_frame(self.inner, resp, grpc_status, message)
            .await
            .map_err(|e| Box::new(e) as PluginSessionError);
        self.observe_delivery(result)
    }

    /// Extract path parameters for synthetic/test contexts that bypass the
    /// normal route-match capture hook.
    ///
    /// Parses the route pattern (e.g., "/api/:uid/profile") against the actual
    /// request path (e.g., "/api/123/profile") and extracts named parameters.
    fn extract_path_params(&self) -> HashMap<String, String> {
        let mut params = HashMap::new();

        // Get the route pattern from matched route
        let pattern = self
            .ctx
            .route_unit
            .as_ref()
            .and_then(|ru| ru.matched_info.m.path.as_ref())
            .and_then(|p| p.value.as_ref())
            .map(|s| s.as_str());

        if let Some(pattern) = pattern {
            // Only parse if pattern contains parameters
            if pattern.contains(':') {
                // Use normalized current path so :param extraction matches the
                // view the route matcher used (strips ;param, merges //, decodes
                // unreserved %xx, removes /./ /../).
                let actual_path = self.ctx.path();
                params = Self::parse_path_params(pattern, actual_path);
            }
        }

        params
    }

    /// Send a gRPC error response as a single-frame Trailers-Only response (HEADERS+END_STREAM),
    /// converting an HTTP status code to the corresponding gRPC status code. Mirrors
    /// `send_grpc_error_direct` but is the dispatch target for the `write_response_header`
    /// trait path when `is_grpc_request` and an error status are seen.
    async fn send_grpc_trailers_only(&mut self, http_status: u16) -> PluginSessionResult<()> {
        let (grpc_status, grpc_message) = http_to_grpc_status(http_status);
        if !self.ctx.claim_final_response(200) {
            return Err(Box::new(std::io::Error::other("final response writer already claimed")));
        }
        self.grpc_response_terminated = true;
        self.ctx.request_info.set_grpc_status(grpc_status);

        let mut resp = ResponseHeader::build(200, None).map_err(|e| Box::new(e) as PluginSessionError)?;
        // Drain queued response-header ops, strip framing headers, then re-assert content-type
        // LAST. See send_grpc_error_direct for why content-length / transfer-encoding are removed.
        self.ctx.drain_pending_response_header_ops(&mut resp);
        self.ctx.request_info.trace_id.reconcile_response(&mut resp);
        resp.remove_header(&http::header::CONTENT_LENGTH);
        resp.remove_header(&http::header::TRANSFER_ENCODING);
        resp.insert_header("content-type", "application/grpc")
            .map_err(|e| Box::new(e) as PluginSessionError)?;
        let result = send_grpc_trailers_only_frame(self.inner, resp, grpc_status, Some(grpc_message))
            .await
            .map_err(|e| Box::new(e) as PluginSessionError);
        self.observe_delivery(result)
    }

    /// Parse path parameters from pattern and actual path.
    ///
    /// # Arguments
    /// * `pattern` - Route pattern with `:param` syntax (e.g., "/api/:uid/profile")
    /// * `actual_path` - Actual request path (e.g., "/api/123/profile")
    ///
    /// # Returns
    /// HashMap of parameter names to values.
    fn parse_path_params(pattern: &str, actual_path: &str) -> HashMap<String, String> {
        crate::ctx::parse_path_params(pattern, actual_path)
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/plugins/runtime/session_adapter_test/storage_io_tests.rs"]
mod storage_io_tests;

fn body_buffer_install_error() -> BodyReadError {
    BodyReadError::StorageFailure
}

/// Outcome of the request-side body admission predicate. Kept pure and
/// separate from the adapter so the ordering — which IS the semantics — is
/// unit-testable without a live `Session`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestAdmission {
    /// The request truthfully declared no body.
    PassThrough,
    /// We decline to buffer. Always fail-close; never softened by policy.
    Refuse(EdgionErrorCode),
    /// Buffer the body.
    Buffer,
}

/// Request-side admission predicate. The evaluation ORDER is the semantics:
/// step 1 must precede the websocket check, or a genuine bodyless WebSocket
/// handshake would be refused and every WebSocket through a Gateway carrying a
/// body plugin would fail.
///
/// Reads only pre-plugin ctx snapshots — never `get_method()`, never a live
/// request header — so a plugin calling `set_upstream_method("CONNECT")` or
/// removing `content-length` cannot influence the decision.
pub(crate) fn request_admission(ctx: &EdgionHttpContext, past_request_stage: bool) -> RequestAdmission {
    // 1. Did the client declare a body at all?
    match ctx.request_info.body_declared {
        Some(false) => return RequestAdmission::PassThrough,
        // The snapshot never ran. Fail closed rather than guess.
        //
        // Deliberately NOT `BodyRefusedStage`: that variant fires on every
        // request for any body consumer placed in a post-request stage, which
        // is expected traffic. This arm is a BUG — a call site reached body
        // admission without `snapshot_body_declared`. Sharing one status would
        // bury the bug signal under the expected case's volume.
        None => return RequestAdmission::Refuse(EdgionErrorCode::BodyRefusedNoSnapshot),
        Some(true) => {}
    }

    // 2. Wrong stage: the request body is long gone once request-stage
    //    processing has completed (any of: the response header is bound, the
    //    upstream response body stage, or any later stage).
    if past_request_stage {
        return RequestAdmission::Refuse(EdgionErrorCode::BodyRefusedStage);
    }

    // 3. Declared a body AND claims WebSocket — an anomalous shape no
    //    legitimate client produces.
    if ctx.request_info.discover_protocol.as_deref() == Some("websocket") {
        return RequestAdmission::Refuse(EdgionErrorCode::BodyRefusedWebsocket);
    }

    // 4. gRPC, by EITHER fact. `is_grpc_request` is client-controlled and
    //    `is_grpc_route_matched` misses gRPC that fell through to HTTPRoute on
    //    a listener with no GRPCRoutes; each covers the other's blind spot.
    //    Widening a refusal with a client-controlled fact is safe precisely
    //    because a refusal cannot be softened into a skip.
    if ctx.request_info.is_grpc_request || ctx.is_grpc_route_matched {
        return RequestAdmission::Refuse(EdgionErrorCode::BodyRefusedGrpc);
    }

    RequestAdmission::Buffer
}

/// Whether a status code is one that carries no response body (RFC 9110).
///
/// Deliberately module-private: its only legitimate use is inside
/// `read_response_body`, where it is applied to BOTH the live status and the
/// ctx snapshot to build `bodyless_agreed`. Applying it to one of them alone is
/// the bug this split exists to prevent, so the helper is not offered crate-wide
/// as an invitation to do that.
#[inline]
fn is_bodyless_status(status: u16) -> bool {
    matches!(status, 100..=199 | 204 | 304)
}

/// Response-side admission predicate. Keys on the RESPONSE, not on the
/// request's protocol label.
///
/// `was_upgraded()` is deliberately not used: it hardcodes `false` for H2, and
/// on H1 it is set only when the 101 is written downstream — after this runs.
/// `status == 101` is the correct and sufficient key.
///
/// Step 4 — the only step that returns the SILENT `PassThrough` — takes its two
/// inputs pre-agreed by the caller. Both `is_head` and `bodyless_agreed` must
/// already be the AND of a live, plugin-mutable fact and its pre-plugin
/// snapshot; see `read_response_body`. Steps 1–3 deliberately keep reading the
/// live status alone, because they all steer toward `Refuse` — the loud
/// direction, which a mutated fact may safely widen (the same reasoning
/// `request_admission`'s gRPC OR relies on).
pub(crate) fn response_admission(
    ctx: &EdgionHttpContext,
    status: Option<u16>,
    is_head: bool,
    bodyless_agreed: bool,
) -> RequestAdmission {
    // 1. No response header bound: this is a request-stage call.
    let Some(status) = status else {
        return RequestAdmission::Refuse(EdgionErrorCode::BodyRefusedStage);
    };

    // 2. Only a real 101 is a tunnel.
    if status == 101 {
        return RequestAdmission::Refuse(EdgionErrorCode::BodyRefusedWebsocket);
    }

    // 3. A gRPC response body does not end until the RPC does.
    if ctx.request_info.is_grpc_request || ctx.is_grpc_route_matched {
        return RequestAdmission::Refuse(EdgionErrorCode::BodyRefusedGrpc);
    }

    // 4. Statuses that carry no body, plus HEAD (RFC 9110 9.3.2).
    if is_head || bodyless_agreed {
        return RequestAdmission::PassThrough;
    }

    RequestAdmission::Buffer
}

/// Resolve the effective buffer cap. `None` from a caller means "the gateway
/// default", never "unbounded", and a per-call value can only tighten.
pub(crate) fn resolve_cap(max_bytes: Option<u64>, ctx_cap: u64) -> u64 {
    match max_bytes {
        Some(v) => v.min(ctx_cap),
        None => ctx_cap,
    }
}

#[async_trait]
impl<'a> PluginSession for PingoraSessionAdapter<'a> {
    fn request_header_map(&self) -> &http::HeaderMap {
        &self.inner.req_header().headers
    }

    fn header_value(&self, name: &str) -> Option<String> {
        self.inner
            .req_header()
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(|s| s.to_string())
    }

    fn request_headers(&self) -> Vec<(String, String)> {
        self.inner
            .req_header()
            .headers
            .iter()
            .filter_map(|(name, value)| value.to_str().ok().map(|v| (name.as_str().to_string(), v.to_string())))
            .collect()
    }

    fn request_headers_bytes(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.inner
            .req_header()
            .headers
            .iter()
            .map(|(name, value)| (name.as_str().as_bytes().to_vec(), value.as_bytes().to_vec()))
            .collect()
    }

    fn request_header_values(&self, name: &str) -> Option<Vec<String>> {
        utf8_header_values(&self.inner.req_header().headers, name)
    }

    fn get_query_param(&self, name: &str) -> Option<String> {
        // Decoded values (percent-decoded, '+' → ' ') shared with the route
        // matcher via EdgionHttpContext::cached_query. Parsed at most once
        // per request regardless of how many plugins call this method.
        // Use entry_raw_query (frozen at request entry) so plugins observe
        // the client-sent query, not any rewritten upstream URI — keeps the
        // cache order-independent across plugin reads.
        self.ctx
            .cached_query(self.ctx.request_info.entry_raw_query.as_deref())
            .get(name)
            .cloned()
    }

    fn get_cookie(&self, name: &str) -> Option<String> {
        cookie_value(&self.inner.req_header().headers, name)
    }

    fn get_path(&self) -> &str {
        self.ctx.path()
    }

    fn get_request_path(&self) -> &str {
        self.ctx.request_path()
    }

    fn get_uri(&self) -> String {
        self.ctx.uri(self.inner.req_header())
    }

    fn get_request_uri(&self) -> String {
        self.ctx.request_uri()
    }

    fn get_query(&self) -> Option<String> {
        self.inner.req_header().uri.query().map(|s| s.to_string())
    }

    fn get_method(&self) -> &str {
        self.inner.req_header().method.as_str()
    }

    fn get_http_protocol(&self) -> String {
        match self.inner.req_header().version {
            http::Version::HTTP_09 => "HTTP/0.9",
            http::Version::HTTP_10 => "HTTP/1.0",
            http::Version::HTTP_11 => "HTTP/1.1",
            http::Version::HTTP_2 => "HTTP/2.0",
            http::Version::HTTP_3 => "HTTP/3.0",
            _ => "HTTP/1.1",
        }
        .to_string()
    }

    fn get_request_id(&self) -> String {
        self.ctx
            .request_info
            .trace_id
            .standard_trace_id()
            .unwrap_or_default()
            .to_owned()
    }

    fn get_ctx_var(&self, key: &str) -> Option<String> {
        self.ctx.get_ctx_var(key).map(|s| s.to_string())
    }

    fn set_ctx_var(&mut self, key: &str, value: &str) -> PluginSessionResult<()> {
        self.ctx.set_ctx_var(key.to_string(), value.to_string());
        Ok(())
    }

    fn remove_ctx_var(&mut self, key: &str) -> PluginSessionResult<()> {
        self.ctx.remove_ctx_var(key);
        Ok(())
    }

    fn get_path_param(&mut self, name: &str) -> Option<String> {
        if self.ctx.cached_path_params().is_none() {
            let params = self.extract_path_params();
            self.ctx.cache_path_params(params);
        }
        self.ctx.cached_path_params()?.get(name).cloned()
    }

    fn matched_route_meta(&self, proto: RouteProto, selector: &str) -> Option<String> {
        let meta = match proto {
            RouteProto::Http => &self.ctx.route_unit.as_ref()?.route.as_ref()?.metadata,
            RouteProto::Grpc => &self.ctx.grpc_route_unit.as_ref()?.route.as_ref()?.metadata,
        };
        route_meta_field(meta, selector)
    }

    async fn write_response_header(
        &mut self,
        resp: Box<ResponseHeader>,
        end_of_stream: bool,
    ) -> PluginSessionResult<()> {
        let http_status = resp.status.as_u16();
        if !edgion_resources::http_status::is_valid_final_http_status(http_status) {
            return Err(Box::new(std::io::Error::other("invalid final response status")));
        }
        if self.ctx.request_info.is_grpc_request && http_status != 200 {
            return self.send_grpc_trailers_only(http_status).await;
        }
        if !self.ctx.claim_final_response(http_status) {
            return Err(Box::new(std::io::Error::other(
                "invalid or duplicate final response status",
            )));
        }
        let mut resp = resp;
        if self.ctx.has_pending_response_header_ops() {
            // Unify: queued response-header ops (dye/record_outcome from
            // fail-close plugins that self-write their response via this
            // method) must land on the first response header that leaves the
            // gateway, whoever writes it -- matching the drain already done
            // by build_response_local_reply / send_grpc_error_direct /
            // send_grpc_trailers_only.
            //
            // Framing protection: unlike those sibling drain sites, callers
            // of this method write the body SEPARATELY afterwards (e.g.
            // rate_limit_redis, auth_common's header-then-body sequence), so
            // this drain cannot recompute content-length from a body it
            // can't see. A queued Set/Remove on content-length or
            // transfer-encoding would desync the wire from the
            // already-decided body, so the caller's original framing always
            // wins: capture it before draining, then restore it after.
            let orig_cl = resp.headers.get(http::header::CONTENT_LENGTH).cloned();
            let orig_te = resp.headers.get(http::header::TRANSFER_ENCODING).cloned();
            self.ctx.drain_pending_response_header_ops(&mut resp);
            self.ctx.request_info.trace_id.reconcile_response(resp.as_mut());
            let _ = resp.remove_header(&http::header::CONTENT_LENGTH);
            if let Some(cl) = orig_cl {
                let _ = resp.insert_header(http::header::CONTENT_LENGTH, cl);
            }
            let _ = resp.remove_header(&http::header::TRANSFER_ENCODING);
            if let Some(te) = orig_te {
                let _ = resp.insert_header(http::header::TRANSFER_ENCODING, te);
            }
        }
        self.ctx.request_info.trace_id.reconcile_response(resp.as_mut());
        let result = self
            .inner
            .write_response_header(resp, end_of_stream)
            .await
            .map_err(|e| Box::new(e) as PluginSessionError);
        self.observe_delivery(result)
    }

    fn set_response_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()> {
        validate_response_header_mutation(name)?;
        let value = match http::HeaderValue::from_str(value) {
            Ok(value) => value,
            // Queued malformed values historically disappeared during reply construction.
            Err(_) if self.response_header.is_none() => return Ok(()),
            Err(error) => return Err(Box::new(error)),
        };
        self.set_response_header_value(name, value)
    }

    fn set_response_header_value(&mut self, name: &str, value: http::HeaderValue) -> PluginSessionResult<()> {
        validate_response_header_mutation(name)?;
        if let Some(resp) = &mut self.response_header {
            resp.insert_header(name.to_string(), value)
                .map_err(|e| Box::new(e) as PluginSessionError)?;
        } else {
            self.ctx.queue_pending_response_header_op(PendingHeaderOp::Set {
                name: name.to_string(),
                value,
            });
        }
        Ok(())
    }

    fn append_response_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()> {
        validate_response_header_mutation(name)?;
        let value = match http::HeaderValue::from_str(value) {
            Ok(value) => value,
            Err(_) if self.response_header.is_none() => return Ok(()),
            Err(error) => return Err(Box::new(error)),
        };
        self.append_response_header_value(name, value)
    }

    fn append_response_header_value(&mut self, name: &str, value: http::HeaderValue) -> PluginSessionResult<()> {
        validate_response_header_mutation(name)?;
        if let Some(resp) = &mut self.response_header {
            resp.append_header(name.to_string(), value)
                .map_err(|e| Box::new(e) as PluginSessionError)?;
        } else {
            self.ctx.queue_pending_response_header_op(PendingHeaderOp::Append {
                name: name.to_string(),
                value,
            });
        }
        Ok(())
    }

    fn remove_response_header(&mut self, name: &str) -> PluginSessionResult<()> {
        validate_response_header_mutation(name)?;
        if let Some(resp) = &mut self.response_header {
            resp.remove_header(name);
        } else {
            self.ctx
                .queue_pending_response_header_op(PendingHeaderOp::Remove { name: name.to_string() });
        }
        Ok(())
    }

    fn response_header_names(&self) -> Vec<String> {
        match &self.response_header {
            Some(resp) => resp.headers.keys().map(|k| k.as_str().to_string()).collect(),
            None => Vec::new(),
        }
    }

    fn response_headers(&self) -> Vec<(String, String)> {
        match &self.response_header {
            Some(resp) => resp
                .headers
                .iter()
                .filter_map(|(name, value)| value.to_str().ok().map(|v| (name.as_str().to_string(), v.to_string())))
                .collect(),
            None => Vec::new(),
        }
    }

    fn response_headers_bytes(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        match &self.response_header {
            Some(resp) => resp
                .headers
                .iter()
                .map(|(name, value)| (name.as_str().as_bytes().to_vec(), value.as_bytes().to_vec()))
                .collect(),
            None => Vec::new(),
        }
    }

    fn get_response_header(&self, name: &str) -> Option<String> {
        self.response_header
            .as_ref()
            .and_then(|resp| resp.headers.get(name))
            .and_then(|value| value.to_str().ok())
            .map(|s| s.to_string())
    }

    fn response_header_map(&self) -> &http::HeaderMap {
        match self.response_header.as_deref() {
            Some(response) => &response.headers,
            None => empty_response_header_map(),
        }
    }

    fn has_response_header(&self, name: &str) -> bool {
        self.response_header
            .as_ref()
            .is_some_and(|response| response.headers.contains_key(name))
    }

    fn get_response_status(&self) -> Option<u16> {
        self.response_header.as_ref().map(|h| h.status.as_u16())
    }

    fn set_response_status(&mut self, status: u16) -> PluginSessionResult<()> {
        if !edgion_resources::http_status::is_valid_final_http_status(status) {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Invalid final response status code: {status}"),
            )));
        }
        if let Some(resp) = &mut self.response_header {
            let status_code = http::StatusCode::from_u16(status).map_err(|_| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("Invalid status code: {}", status),
                )) as PluginSessionError
            })?;
            resp.set_status(status_code)
                .map_err(|e| Box::new(e) as PluginSessionError)?;
        }
        Ok(())
    }

    fn set_request_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()> {
        self.inner
            .req_header_mut()
            .insert_header(name.to_string(), value.to_string())
            .map_err(|e| Box::new(e) as PluginSessionError)?;
        self.ctx.remove_deferred_credential_mutation(name);
        Ok(())
    }

    fn set_request_header_value(&mut self, name: &str, value: http::HeaderValue) -> PluginSessionResult<()> {
        self.inner
            .req_header_mut()
            .insert_header(name.to_string(), value)
            .map_err(|e| Box::new(e) as PluginSessionError)?;
        self.ctx.remove_deferred_credential_mutation(name);
        Ok(())
    }

    fn append_request_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()> {
        let value = http::HeaderValue::from_str(value).map_err(|e| Box::new(e) as PluginSessionError)?;
        self.append_request_header_value(name, value)
    }

    fn append_request_header_value(&mut self, name: &str, value: http::HeaderValue) -> PluginSessionResult<()> {
        // Append as a new header field line (preserves every existing field line
        // per RFC 7230 §3.2.2 list semantics; insert_header would replace all).
        self.inner
            .req_header_mut()
            .append_header(name.to_string(), value)
            .map_err(|e| Box::new(e) as PluginSessionError)?;
        self.ctx.remove_deferred_credential_mutation(name);
        Ok(())
    }

    fn remove_request_header(&mut self, name: &str) -> PluginSessionResult<()> {
        self.inner.req_header_mut().remove_header(name);
        self.ctx.remove_deferred_credential_mutation(name);
        Ok(())
    }

    fn is_remote_ip_header(&self, name: &str) -> bool {
        self.ctx.remote_ip_header.as_str().eq_ignore_ascii_case(name)
    }

    fn defer_credential_header(
        &mut self,
        name: http::HeaderName,
        value: Arc<http::HeaderValue>,
        valid_until: Option<std::time::Instant>,
    ) -> PluginSessionResult<()> {
        self.ctx.upsert_deferred_credential_mutation(name, value, valid_until);
        Ok(())
    }

    fn set_request_query(&mut self, query: &str) -> PluginSessionResult<()> {
        // Keep the raw request-target form (including an absolute-form scheme
        // and authority) while replacing only its query. Path normalization is
        // query-agnostic, and the upstream-peer stage still chooses the final
        // raw or normalized path because this does not set `uri_finalized`.
        let uri = &self.inner.req_header().uri;
        if uri.scheme().is_some() || uri.authority().is_some() {
            // HTTP/2 carries :scheme and :authority in the Uri. A raw-target
            // update would replace them with a path-only Uri.
            let path_and_query = if query.is_empty() {
                uri.path().to_string()
            } else {
                format!("{}?{}", uri.path(), query)
            };
            let replacement = path_and_query
                .parse::<Uri>()
                .map_err(|e| Box::new(e) as PluginSessionError)?;
            let updated = crate::ctx::retain_uri_authority_for_path_only(uri, replacement)
                .map_err(|e| Box::new(e) as PluginSessionError)?;
            self.inner.req_header_mut().set_uri(updated);
            return Ok(());
        }

        let current = self.inner.req_header().raw_path();
        let path_end = current.iter().position(|byte| *byte == b'?').unwrap_or(current.len());
        let mut target = Vec::with_capacity(path_end + usize::from(!query.is_empty()) + query.len());
        target.extend_from_slice(&current[..path_end]);
        if !query.is_empty() {
            target.push(b'?');
            target.extend_from_slice(query.as_bytes());
        }
        self.inner
            .req_header_mut()
            .set_raw_path(&target)
            .map_err(|e| Box::new(e) as PluginSessionError)?;
        Ok(())
    }

    fn set_upstream_uri(&mut self, uri: &str) -> PluginSessionResult<()> {
        let parsed_uri = uri.parse::<Uri>().map_err(|e| Box::new(e) as PluginSessionError)?;
        let updated = crate::ctx::retain_uri_authority_for_path_only(&self.inner.req_header().uri, parsed_uri)
            .map_err(|e| Box::new(e) as PluginSessionError)?;
        self.inner.req_header_mut().set_uri(updated);
        let cfg = edgion_resources::path_normalization::get_path_normalization_config();
        self.ctx.refresh_normalized_path(self.inner.req_header(), cfg);
        self.ctx.uri_finalized = true;
        Ok(())
    }

    fn set_upstream_host(&mut self, host: &str) -> PluginSessionResult<()> {
        self.inner
            .req_header_mut()
            .insert_header("Host".to_string(), host.to_string())
            .map_err(|e| Box::new(e) as PluginSessionError)?;
        Ok(())
    }

    fn set_upstream_method(&mut self, method: &str) -> PluginSessionResult<()> {
        let parsed_method = method
            .parse::<http::Method>()
            .map_err(|e| Box::new(e) as PluginSessionError)?;
        self.inner.req_header_mut().set_method(parsed_method);
        Ok(())
    }

    async fn write_response_body(&mut self, body: Option<Bytes>, end_of_stream: bool) -> PluginSessionResult<()> {
        if self.grpc_response_terminated {
            // trailers-only response already sent; discard body from HTTP plugin
            return Ok(());
        }
        let result = self
            .inner
            .write_response_body(body, end_of_stream)
            .await
            .map_err(|e| Box::new(e) as PluginSessionError);
        self.observe_delivery(result)
    }

    async fn shutdown(&mut self) {
        if self.ctx.request_info.is_grpc_request {
            // gRPC streams are terminated by END_STREAM on the HEADERS frame.
            // Calling shutdown() would send RST_STREAM on the H2 stream, which
            // the client interprets as Internal(13) regardless of any grpc-status
            // already written.
            return;
        }
        // No `is_http2()` gate here, unlike `shutdown_after_error_response` in
        // runtime/server/error_response.rs. Pingora's H2 arm of
        // `HttpSession::shutdown` only sends RST_STREAM, and only when the stream
        // has not already ended — it never closes the TCP connection. Every caller
        // of this method writes an END_STREAM body immediately before, so the plain
        // HTTP/2 case is already a no-op. The one path where the reset does fire is
        // a caller that swallowed a write error (`forward_auth`,
        // `all_endpoint_status` use `let _ = ...`), leaving a half-written response
        // on the wire — resetting that stream is the correct outcome, so adding an
        // `is_http2()` gate here would be a regression, not a fix.
        self.inner.shutdown().await;
    }

    fn direct_peer_ip(&self) -> &str {
        &self.ctx.request_info.direct_peer_ip
    }
    fn downstream_peer_ip(&self) -> &str {
        &self.ctx.request_info.downstream_peer_ip
    }
    fn remote_ip(&self) -> &str {
        &self.ctx.request_info.remote_ip
    }

    fn set_remote_ip(&mut self, addr: &str) -> PluginSessionResult<()> {
        let canonical = crate::ctx::canonicalize_ip_str(addr).ok_or_else(|| {
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "remote IP must be a valid IP address",
            )) as PluginSessionError
        })?;
        if self.ctx.request_info.remote_ip != canonical {
            self.ctx.request_info.remote_ip = canonical;
            // A RealIp plugin may run after an earlier GeoIpLocation consumer.
            // Never retain facts resolved for the previous RemoteIp.
            self.ctx.request_info.invalidate_remote_geo();
        }
        Ok(())
    }

    fn apply_request_ids(
        &mut self,
        default: Option<&edgion_resources::plugin_configs::DefaultIdConfig>,
        rules: &[crate::request_trace_id::CompiledRequestIdRule],
    ) -> bool {
        use crate::request_trace_id::{single_header, ApplyOutcome};
        if let Some(patch) = default {
            self.ctx.request_info.trace_id.apply_default_patch(patch);
            self.ctx
                .request_info
                .trace_id
                .reconcile_request(self.inner.req_header_mut());
        }
        let mut within_limit = true;
        for rule in rules {
            let target = single_header(&self.inner.req_header().headers, rule.header.as_str());
            let source = rule.source.as_ref().and_then(|key| {
                if let KeyGet::Header { name } = key {
                    single_header(&self.inner.req_header().headers, name.as_str())
                } else {
                    self.key_get(key)
                        .and_then(|value| http::HeaderValue::from_str(&value).ok())
                }
            });
            let outcome = self.ctx.request_info.trace_id.apply_rule(rule, target, source);
            within_limit &= outcome != ApplyOutcome::LimitExceeded;
            self.ctx
                .request_info
                .trace_id
                .reconcile_request(self.inner.req_header_mut());
        }
        within_limit
    }

    fn client_cert_info(&self) -> Option<crate::ctx::ClientCertInfo> {
        self.ctx.request_info.client_cert_info().cloned()
    }

    fn downstream_is_tls(&self) -> Option<bool> {
        // Same source as the session-persistence cookie `Secure` decision:
        // the downstream digest carries an ssl_digest only for a connection
        // that completed a TLS handshake on this listener. A missing digest is
        // "unknown", not "plaintext".
        self.inner.as_downstream().digest().map(|d| d.ssl_digest.is_some())
    }

    fn begin_rate_limit_local_frame(
        &mut self,
        count: usize,
    ) -> Option<crate::plugins::runtime::conditions::rate_limit_local_outcome::RateLimitLocalFrame> {
        self.ctx.begin_rate_limit_local_frame(count)
    }

    fn end_rate_limit_local_frame(
        &mut self,
        frame: Option<crate::plugins::runtime::conditions::rate_limit_local_outcome::RateLimitLocalFrame>,
    ) {
        self.ctx.end_rate_limit_local_frame(frame);
    }

    fn rate_limit_local_outcome(
        &self,
        slot: usize,
    ) -> crate::plugins::runtime::conditions::rate_limit_local_outcome::RateLimitLocalOutcome {
        self.ctx.rate_limit_local_outcome(slot)
    }

    fn set_rate_limit_local_outcome(
        &mut self,
        slot: usize,
        outcome: crate::plugins::runtime::conditions::rate_limit_local_outcome::RateLimitLocalOutcome,
    ) -> bool {
        self.ctx.set_rate_limit_local_outcome(slot, outcome)
    }

    fn ctx(&self) -> &EdgionHttpContext {
        self.ctx
    }

    fn install_waf_transaction(
        &mut self,
        owner: Arc<()>,
        transaction: edgion_waf::seclang::Transaction<'static>,
    ) -> bool {
        self.ctx
            .waf
            .as_deref_mut()
            .is_some_and(|waf| waf.install(owner, transaction))
    }

    fn take_waf_transaction(&mut self, owner: &Arc<()>) -> Option<edgion_waf::seclang::Transaction<'static>> {
        self.ctx.waf.as_deref_mut()?.take(owner)
    }

    fn claim_waf_phase(&mut self, owner: &Arc<()>, phase: crate::ctx::WafPhase) -> crate::ctx::WafPhaseClaim {
        self.ctx.claim_waf_phase(owner, phase)
    }

    fn record_waf_phase_outcome(
        &mut self,
        owner: &Arc<()>,
        phase: crate::ctx::WafLogPhase,
        execution_us: u64,
        outcome: &edgion_waf::seclang::PhaseOutcome,
    ) {
        self.ctx.record_waf_phase_outcome(owner, phase, execution_us, outcome);
    }

    fn record_waf_terminal_error(
        &mut self,
        owner: &Arc<()>,
        phase: crate::ctx::WafLogPhase,
        code: crate::ctx::WafErrorCode,
        status: u16,
    ) {
        self.ctx.record_waf_terminal_error(owner, phase, code, status);
    }

    fn enable_waf_request_body_streaming(&mut self, owner: &Arc<()>, body_limit: u64) -> bool {
        if !self.ctx.request_info.in_request_body_stage {
            return false;
        }
        let enabled = self
            .ctx
            .waf
            .as_deref_mut()
            .is_some_and(|waf| waf.enable_request_body_streaming(owner, body_limit));
        if enabled {
            self.ctx.activate_waf_request_body_streaming();
        }
        enabled
    }

    fn record_error(&mut self, status: edgion_resources::EdgionErrorCode) {
        self.ctx.record_error(status);
    }

    fn set_ai_request_context(&mut self, context: AiRequestContext) -> bool {
        self.ctx.try_set_ai_request_context(context)
    }

    fn set_ai_attempt_plan(&mut self, plan: AiAttemptPlan) -> bool {
        self.ctx.try_set_ai_attempt_plan(plan)
    }

    fn install_stream_processor(
        &mut self,
        processor: Box<dyn crate::plugins::runtime::traits::StreamingResponseProcessor>,
    ) {
        self.ctx.push_response_stream_processor(processor);
    }

    fn semantic_guard_active(&self) -> bool {
        self.ctx.ai_state().is_some_and(|state| state.semantic_guard_claimed)
    }

    fn publish_canonical_records(
        &mut self,
        batch: crate::ai::canonical_sse::CanonicalRecordBatch,
    ) -> Result<(), crate::ai::canonical_sse::CanonicalRecordBatch> {
        let state = self.ctx.ensure_ai_state();
        if !state.semantic_guard_claimed || state.canonical_record_batch.is_some() {
            return Err(batch);
        }
        state.canonical_record_batch = Some(batch);
        Ok(())
    }

    fn take_canonical_records(&mut self) -> Option<crate::ai::canonical_sse::CanonicalRecordBatch> {
        self.ctx
            .ai_state_mut()
            .and_then(|state| state.canonical_record_batch.take())
    }

    fn try_claim_response_terminal(&mut self, cause: crate::ai::terminal::ResponseTerminalCause) -> bool {
        self.response_processor_slot
            .is_some_and(|slot| self.ctx.response_terminal_state.try_claim(slot, cause))
    }

    fn mark_response_terminal_emission(&mut self, emission: crate::ai::terminal::TerminalEmission) {
        self.ctx.response_terminal_state.mark_emission(emission);
    }

    fn ai_terminal_render_context(
        &self,
    ) -> Option<(
        crate::ai::terminal::AiClientStreamDialect,
        crate::ai::terminal::FinalResponseEligibility,
    )> {
        let state = self.ctx.ai_state()?;
        Some((state.client_stream_dialect?, state.final_response_eligibility?))
    }

    fn disable_semantic_guard(&mut self) {
        if !self.ctx.response_body_processing_started {
            if let Some(state) = self.ctx.ai_state_mut() {
                state.semantic_guard_claimed = false;
                state.canonical_record_batch = None;
                state.semantic_processor = None;
                state.response_head_hold_claim = None;
            }
        }
    }

    fn install_semantic_response_processor(
        &mut self,
        processor: Box<dyn crate::plugins::runtime::traits::StreamingResponseProcessor>,
    ) -> bool {
        if self.ctx.response_processors_frozen()
            || self
                .ctx
                .ai_state()
                .is_some_and(|state| state.semantic_processor.is_some())
        {
            return false;
        }
        let state = self.ctx.ensure_ai_state();
        state.semantic_processor = Some(processor);
        if let Some(claim) = state.response_head_hold_claim.as_mut() {
            claim.active = true;
        }
        true
    }

    fn install_attached_ext_proc(
        &mut self,
        identity: String,
        owner: std::sync::Arc<crate::plugins::http::ext_proc::AttachedExtProc>,
        reserve_framing: bool,
    ) -> bool {
        self.ctx.install_attached_ext_proc(identity, owner, reserve_framing)
    }

    fn attached_ext_proc(
        &self,
        identity: &str,
    ) -> Option<std::sync::Arc<crate::plugins::http::ext_proc::AttachedExtProc>> {
        self.ctx.attached_ext_proc(identity)
    }

    fn install_attached_guardrail(
        &mut self,
        identity: String,
        owner: std::sync::Arc<crate::plugins::http::guardrail::AttachedGuardrail>,
        head_limits: Option<pingora_proxy::ResponseHeadHoldLimits>,
    ) -> bool {
        if self
            .ctx
            .ai_state()
            .is_some_and(|state| state.attached_guardrail.is_some() || state.semantic_guard_claimed)
        {
            return false;
        }
        let state = self.ctx.ensure_ai_state();
        state.attached_guardrail = Some((identity, owner));
        state.semantic_guard_claimed = true;
        state.response_head_hold_claim =
            head_limits.map(|limits| crate::ctx::ResponseHeadHoldClaim { limits, active: false });
        self.ctx.reserve_response_framing();
        true
    }

    fn attached_guardrail(
        &self,
        identity: &str,
    ) -> Option<std::sync::Arc<crate::plugins::http::guardrail::AttachedGuardrail>> {
        self.ctx
            .ai_state()?
            .attached_guardrail
            .as_ref()
            .filter(|(installed, _)| installed == identity)
            .map(|(_, owner)| std::sync::Arc::clone(owner))
    }

    fn install_attached_response_processor(
        &mut self,
        processor: Box<dyn crate::plugins::runtime::traits::StreamingResponseProcessor>,
    ) -> bool {
        if self.ctx.response_processors_frozen() {
            return false;
        }
        self.ctx.push_response_stream_processor(processor);
        true
    }

    fn install_upstream_response_trailer(
        &mut self,
        processor: Box<dyn crate::plugins::runtime::traits::UpstreamResponseTrailer>,
    ) -> bool {
        if self.ctx.response_processors_frozen() || !self.ctx.upstream_trailer_lifecycle_is_open() {
            return false;
        }
        self.ctx.install_upstream_response_trailer(processor)
    }

    fn upstream_response_trailer_count(&self) -> usize {
        self.ctx.upstream_response_trailer_count()
    }

    fn downstream_response_trailers_representable(&self) -> bool {
        self.inner.as_downstream().response_trailers_supported()
    }

    fn downstream_response_trailer_name_allowed(&self, name: &http::HeaderName) -> bool {
        let downstream = self.inner.as_downstream();
        if downstream.is_http2() || downstream.is_subrequest() || downstream.is_custom() {
            return true;
        }
        downstream
            .response_written()
            .into_iter()
            .flat_map(|response| response.headers.get_all(http::header::TRAILER).iter())
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .any(|declared| declared.trim().eq_ignore_ascii_case(name.as_str()))
    }

    fn upstream_response_headers_end_stream(&self) -> Option<bool> {
        self.ctx.upstream_response_headers_end_stream
    }

    fn claim_json_schema_validation(&mut self) -> bool {
        self.ctx.claim_json_schema_validation()
    }

    fn discover_protocol(&self) -> String {
        self.ctx.request_info.discover_protocol.clone().unwrap_or_else(|| {
            if self.ctx.request_info.is_grpc_request {
                "grpc".to_string()
            } else {
                "http".to_string()
            }
        })
    }

    fn declares_body(&self) -> Option<bool> {
        self.ctx.request_info.body_declared
    }

    async fn ensure_request_body(&mut self, max_bytes: Option<u64>) -> Result<u64, BodyReadError> {
        // A response-stage consumer may read a body that an earlier request
        // plugin explicitly buffered. The stage boundary forbids starting a
        // new capture; it does not invalidate the request-scoped shared cache.
        if let Some(len) = self.ctx.request_body.cached_len()? {
            return Ok(len);
        }
        if self.inner.request_body_prefix_active() {
            return Err(BodyReadError::NotBuffered);
        }

        // `in_request_body_stage` (set at request entry and cleared when the
        // global/Gateway/route request-plugin chain completes) is the
        // ctx-snapshot source of truth for "are we still in the request
        // stage", covering backendRef and all later construction sites
        // uniformly (response-header-bound stages, the upstream response
        // *body* stage, and beyond) rather than the
        // `response_header.is_some()` proxy this replaced, which was `false`
        // at the upstream response body stage even though that stage runs
        // strictly after the request body is fully proxied upstream.
        let past_request_stage = !self.ctx.request_info.in_request_body_stage;
        match request_admission(self.ctx, past_request_stage) {
            RequestAdmission::PassThrough => {
                if let Some(len) = self.ctx.request_body.cached_len()? {
                    return Ok(len);
                }
                Ok(0)
            }
            RequestAdmission::Refuse(status) => {
                self.ctx.record_error(status);
                Err(BodyReadError::NotBuffered)
            }
            RequestAdmission::Buffer => self.capture_request_body(max_bytes).await,
        }
    }

    async fn read_request_body(&mut self, max_bytes: Option<u64>) -> BodyReadResult {
        self.ensure_request_body(max_bytes).await?;
        if let Some(body) = self.ctx.request_body.read_all().await? {
            return Ok(body);
        }
        Ok(bytes::Bytes::new())
    }

    async fn read_request_body_prefix(&mut self, limit: usize) -> Result<(Bytes, bool), BodyReadError> {
        if limit == 0
            || limit > edgion_waf::seclang::MAX_REQUEST_BODY_PREFIX_BYTES
            || !self.ctx.request_info.in_request_body_stage
            || self.ctx.request_body_stream_active()
        {
            return Err(BodyReadError::NotBuffered);
        }
        // Reuse a prior complete capture, including its current logical edits.
        // Do not materialize the remainder merely to inspect the prefix.
        if let Some(len) = self.ctx.request_body.cached_len()? {
            self.ctx.request_body.freeze_mutation();
            let prefix = self
                .ctx
                .request_body
                .read_range(0, limit as u64, limit as u64)
                .await
                .map_err(|error| match error {
                    BodyOperationError::Body(error) => error,
                    _ => BodyReadError::Incomplete,
                })?
                .value;
            self.ctx.waf_prefix_inspected = true;
            return Ok((prefix, len <= limit as u64));
        }
        match request_admission(self.ctx, false) {
            RequestAdmission::PassThrough => {
                self.ctx.waf_prefix_inspected = true;
                return Ok((Bytes::new(), true));
            }
            RequestAdmission::Refuse(code) => {
                self.ctx.record_error(code);
                return Err(BodyReadError::NotBuffered);
            }
            RequestAdmission::Buffer => {}
        }
        if !self.ctx.request_body_config.enabled {
            return Err(BodyReadError::Unimplemented);
        }
        let cap = self.ctx.request_body_config.max_body_size_bytes;
        if self
            .inner
            .req_header()
            .headers
            .get(http::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|len| len > cap)
        {
            return Err(reject_over_limit_request_body(self.inner.as_mut(), cap));
        }
        let expect_continue = self.inner.req_header().version == http::Version::HTTP_11
            && self
                .inner
                .req_header()
                .headers
                .get(http::header::EXPECT)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.eq_ignore_ascii_case("100-continue"));
        if expect_continue {
            self.inner
                .write_continue_response()
                .await
                .map_err(|_| BodyReadError::Incomplete)?;
            self.inner.req_header_mut().remove_header(&http::header::EXPECT);
        }
        // A smaller physical cap needs at most one additional byte to prove
        // overflow. This is a capture budget, not the whole-request size cap.
        let capture_limit = limit.min(usize::try_from(cap.saturating_add(1)).unwrap_or(usize::MAX));
        // WAF prefixes are capped at 1 MiB and stay in memory independently
        // of the spill threshold used by complete-body consumers.
        let (storage, view) = BodyStorage::with_storage_timeout(
            capture_limit,
            capture_limit as u64,
            self.ctx.request_body_config.storage_operation_timeout,
        );
        if let Err(error) = self
            .inner
            .capture_request_body_prefix(Box::new(storage), capture_limit)
            .await
        {
            disable_h1_reuse_after_capture_failure(self.inner.as_mut());
            return Err(match error.etype() {
                pingora_core::ErrorType::ReadTimedout => BodyReadError::Timeout,
                pingora_core::ErrorType::HTTPStatus(413) => BodyReadError::BufferLimitExceeded { limit: cap },
                pingora_core::ErrorType::InternalError => BodyReadError::StorageFailure,
                _ => BodyReadError::Incomplete,
            });
        }
        if self.inner.body_bytes_read() as u64 > cap {
            return Err(reject_over_limit_request_body(self.inner.as_mut(), cap));
        }
        let complete =
            self.inner.request_body_prefix_transport_complete() && self.inner.body_bytes_read() <= capture_limit;
        let prefix = view.read_all().await?;
        // The storage remains owned by Pingora for first-attempt delivery.
        // Never install this view as a complete request-body cache.
        self.ctx.waf_prefix_inspected = true;
        Ok((prefix, complete))
    }

    async fn read_response_body(&mut self, max_bytes: Option<u64>) -> BodyReadResult {
        // Copy the facts out before touching `self.ctx` mutably.
        let status = self.response_header.as_ref().map(|h| h.status.as_u16());
        // Same unanimity rule as `is_head` below, for the other half of the
        // bodyless short-circuit. The bound response header is response-stage
        // mutable — `set_response_status` writes straight into it, reachable
        // from DSL's `resp.set_status` and from `response_rewrite` — so a
        // `resp.set_status(204)` ahead of a body consumer in the same array
        // would otherwise fabricate `Ok(Bytes::new())` for a response whose
        // body is real and is on its way downstream. `pre_plugin_response_status`
        // is the normalized upstream status, captured in `upstream_response_filter`
        // before any response-stage plugin runs. `None` (never stamped) yields
        // `false` -> `Buffer`, the loud direction.
        let bodyless_agreed = status.is_some_and(is_bodyless_status)
            && self.ctx.pre_plugin_response_status.is_some_and(is_bodyless_status);
        // `PassThrough` is the SILENT disposition — it resolves to
        // `Ok(Bytes::new())`, whose contract is "the response truthfully
        // declared no body". It therefore requires BOTH the pre-plugin
        // snapshot and the live header to agree the request is HEAD; any
        // disagreement steers to `Buffer`, which can only fail loudly.
        //
        // Neither fact is sufficient alone, because `set_upstream_method`
        // rewrites the live header and each direction breaks one of them:
        //
        // - snapshot alone: a downstream `HEAD` rewritten to a body-carrying
        //   method makes pingora write the real body downstream
        //   (`init_body_writer` keys on this same live header), while the
        //   snapshot still says HEAD -> a consumer inspects nothing and
        //   believes the response was empty.
        // - live header alone: a rewrite TO `HEAD` (reachable at the response
        //   stage too — DSL's `req.set_method` is not stage-gated) fabricates
        //   the same silent skip after the body already arrived.
        //
        // The mutable fact can only ever NARROW `PassThrough`, never widen it —
        // the mirror of `request_admission`'s gRPC OR, where a client-controlled
        // fact can only widen a refusal. See `method_is_head`'s doc.
        let is_head = self.ctx.request_info.method_is_head && self.inner.req_header().method == http::Method::HEAD;

        match response_admission(self.ctx, status, is_head, bodyless_agreed) {
            RequestAdmission::PassThrough => Ok(bytes::Bytes::new()),
            RequestAdmission::Refuse(status) => {
                self.ctx.record_error(status);
                Err(BodyReadError::NotBuffered)
            }
            RequestAdmission::Buffer => {
                // Full upstream-response reads are intentionally unsupported:
                // the main response must never be accumulated and replayed.
                // Response processing uses current chunks or the bounded
                // semantic-window streaming path.
                let _cap = resolve_cap(max_bytes, self.ctx.request_body_config.max_body_size_bytes);
                Err(BodyReadError::Unimplemented)
            }
        }
    }

    async fn replace_request_body_slice(
        &mut self,
        range: std::ops::Range<usize>,
        replacement: bytes::Bytes,
    ) -> BodyReadResult {
        self.ensure_bodyless_request_replay_owner().await?;
        self.ctx.replace_request_body_slice(range, replacement).await
    }

    async fn replace_request_body_spans(
        &mut self,
        expected_generation: u64,
        edits: &[(std::ops::Range<u64>, bytes::Bytes)],
    ) -> Result<u64, BodyReadError> {
        // Carried explicitly rather than inherited: the two sibling mutation
        // methods below omit this check, and copying their wiring verbatim
        // would silently reproduce that omission.
        if !self.ctx.request_info.in_request_body_stage {
            return Err(BodyReadError::NotBuffered);
        }
        self.ensure_bodyless_request_replay_owner().await?;
        self.ctx
            .request_body
            .replace_spans(expected_generation, edits)
            .map_err(|error| match error {
                crate::request_body::BodyOperationError::Body(error) => error,
                _ => BodyReadError::Incomplete,
            })
    }

    fn record_ai_guard_decision(&mut self, identity: String, generation: Option<u64>) {
        let state = self.ctx.ensure_ai_state();
        if !state.guard_decided.iter().any(|existing| existing == &identity) {
            state.guard_decided.push(identity);
        }
        if generation.is_some() {
            state.guard_body_generation = generation;
        }
    }

    async fn overwrite_request_body(&mut self, body: bytes::Bytes) -> Result<(), BodyReadError> {
        self.ensure_bodyless_request_replay_owner().await?;
        self.ctx.overwrite_request_body(body)
    }

    async fn request_body_sha256(&mut self) -> Result<[u8; 32], BodyReadError> {
        if !self.ctx.request_body.has_cached_body() {
            self.ensure_request_body(None).await?;
        }
        if let Some(hash) = self.ctx.request_body.sha256().await? {
            return Ok(hash);
        }
        Ok(crate::request_body::EMPTY_SHA256)
    }

    fn request_body_available_len(&mut self, known_bodyless: bool) -> Result<u64, BodyReadError> {
        if known_bodyless && !self.ctx.request_body.has_cached_body() {
            self.ctx.request_body.mark_known_empty();
        }
        self.ctx.request_body.available_len(known_bodyless)
    }

    async fn request_body_snapshot_reader(
        &mut self,
        known_bodyless: bool,
    ) -> Result<Box<dyn RequestBodySnapshotReader>, BodyReadError> {
        if known_bodyless && !self.ctx.request_body.has_cached_body() {
            self.ctx.request_body.mark_known_empty();
        }
        self.ctx.request_body.snapshot_reader(known_bodyless)
    }

    async fn request_body_clear(&mut self) -> Result<(), BodyOperationError> {
        if !self.ctx.request_info.in_request_body_stage {
            return Err(BodyReadError::NotBuffered.into());
        }
        self.ensure_bodyless_request_replay_owner().await?;
        self.ctx.request_body.clear()
    }

    async fn request_body_append(&mut self, bytes: Bytes) -> Result<(), BodyOperationError> {
        if !self.ctx.request_info.in_request_body_stage {
            return Err(BodyReadError::NotBuffered.into());
        }
        self.ensure_bodyless_request_replay_owner().await?;
        self.ctx.request_body.append(bytes)
    }

    async fn request_body_prepend(&mut self, bytes: Bytes) -> Result<(), BodyOperationError> {
        if !self.ctx.request_info.in_request_body_stage {
            return Err(BodyReadError::NotBuffered.into());
        }
        self.ensure_bodyless_request_replay_owner().await?;
        self.ctx.request_body.prepend(bytes)
    }

    async fn request_body_remove(&mut self, range: std::ops::Range<u64>) -> Result<(), BodyOperationError> {
        if !self.ctx.request_info.in_request_body_stage {
            return Err(BodyReadError::NotBuffered.into());
        }
        self.ensure_bodyless_request_replay_owner().await?;
        self.ctx.request_body.remove(range)
    }

    async fn request_body_replace_literal(
        &mut self,
        from: Bytes,
        to: Bytes,
        max_scan_bytes: u64,
    ) -> Result<BodyScanResult<()>, BodyOperationError> {
        if !self.ctx.request_info.in_request_body_stage {
            return Err(BodyReadError::NotBuffered.into());
        }
        self.ensure_bodyless_request_replay_owner().await?;
        self.ctx.request_body.replace_literal(from, to, max_scan_bytes).await
    }

    fn grpc_service(&self) -> Option<String> {
        self.ctx.request_info.grpc_service().map(str::to_string)
    }

    fn grpc_method(&self) -> Option<String> {
        self.ctx.request_info.grpc_method().map(str::to_string)
    }

    fn resolved_route_ext_ref(&self, index: usize) -> Option<std::sync::Arc<crate::plugins::runtime::PluginRuntime>> {
        self.ctx.resolved_route_ext_ref(index)
    }

    fn pinned_backend_ext_ref(&mut self, key: &str) -> Option<std::sync::Arc<crate::plugins::runtime::PluginRuntime>> {
        let scope = self.ctx.plugin_scope().cloned();
        self.ctx.pinned_backend_ext_ref(key, || {
            scope?
                .scope
                .http(key)
                .map(|compiled| Arc::clone(&compiled.plugin_runtime))
        })
    }

    fn start_edgion_plugins_log(&mut self, name: String) -> EdgionPluginsLogToken {
        let idx = self.ctx.start_pending_edgion_plugins_log(name);
        EdgionPluginsLogToken::new(idx)
    }

    fn push_to_edgion_plugins_log(&mut self, token: &EdgionPluginsLogToken, log: PluginLog) {
        if let Some(edgion_log) = self.ctx.pending_edgion_plugins_log_mut(token.idx()) {
            edgion_log.logs.push(log);
        }
    }

    fn key_get(&self, key: &KeyGet) -> Option<String> {
        match key {
            KeyGet::TraceId => self.ctx.request_info.trace_id.standard_trace_id().map(str::to_owned),
            KeyGet::RemoteIp => {
                let ip = self.remote_ip();
                if ip.is_empty() {
                    None
                } else {
                    Some(ip.to_string())
                }
            }
            KeyGet::DirectPeerIp | KeyGet::Host => request_identity_key_core(self.ctx, key),
            KeyGet::Header { name } => self.header_value(name),
            KeyGet::Cookie { name } => self.get_cookie(name),
            KeyGet::Query { name } => self.get_query_param(name),
            KeyGet::Path => Some(self.get_path().to_string()),
            KeyGet::RequestPath => Some(self.get_request_path().to_string()),
            KeyGet::Method => Some(self.get_method().to_string()),
            KeyGet::Ctx { name } => self.get_ctx_var(name),
            KeyGet::RemoteIpAndPath => {
                let ip = self.remote_ip();
                if ip.is_empty() {
                    None
                } else {
                    Some(format!("{}:{}", ip, self.get_path()))
                }
            }
        }
    }

    fn key_set(&mut self, key: &KeySet, value: Option<String>) -> PluginSessionResult<()> {
        match (key, value) {
            (KeySet::Header { name }, Some(v)) => self.set_request_header(name, &v),
            (KeySet::Header { name }, None) => self.remove_request_header(name),
            (KeySet::ResponseHeader { name }, Some(v)) => self.set_response_header(name, &v),
            (KeySet::ResponseHeader { name }, None) => self.remove_response_header(name),
            (KeySet::Cookie { name }, Some(v)) => {
                // Set cookie via Set-Cookie header. Append, not set: Set-Cookie is a
                // multi-value header; insert_header would replace every upstream/
                // other-plugin Set-Cookie (RFC 6265 §3 / RFC 7230 §3.2.2 exception).
                self.append_response_header("Set-Cookie", &format!("{}={}", name, v))
            }
            (KeySet::Cookie { name }, None) => {
                // Remove cookie by setting Max-Age=0. Append so the deletion targets
                // only this cookie name without wiping other Set-Cookie values; the
                // queue drains after the upstream header, so this line wins per RFC 6265.
                self.append_response_header("Set-Cookie", &format!("{}=; Max-Age=0", name))
            }
            (KeySet::Ctx { name }, Some(v)) => self.set_ctx_var(name, &v),
            (KeySet::Ctx { name }, None) => self.remove_ctx_var(name),
        }
    }

    fn set_upstream_jump(&mut self, target: UpstreamJumpTarget) -> SetJumpOutcome {
        self.ctx.try_set_jump(target)
    }

    fn set_mirror_state(&mut self, state: MirrorState) -> bool {
        if !self.ctx.request_info.in_request_body_stage && !self.backend_mirror_allowed {
            return false;
        }
        self.ctx.set_mirror_state(state);
        true
    }

    /// Install the dye plan of the plugin entry about to be invoked.
    /// Called by every stage loop before each invocation (including an
    /// `ExtensionRef` bundle's own inner iteration — see
    /// `gateway_api_filters::extension_ref`); overwritten per invocation.
    /// `Arc` so `record_outcome` can lift the plan back out of `ctx` before
    /// iterating its rules while mutating `ctx`.
    #[inline]
    fn install_dye_plan(&mut self, plan: Option<Arc<CompiledDyePlan>>) {
        self.ctx.install_current_dye_plan(plan);
    }

    /// Once-per-name anti-spoof strip: remove the client-supplied value of
    /// each request-direction rule name the first time an entry declares it;
    /// never re-strip (a prior entry's rendered value is gateway-authored).
    /// Runs before the plugin's kill switch / gating / conditions. No-op
    /// when no plan is installed. Core logic lives in the free function
    /// `strip_request_dye_once_core` so it is unit-testable without a full
    /// `Session`.
    fn strip_request_dye_once(&mut self) {
        let Some(plan) = self.ctx.current_dye_plan() else {
            return;
        };
        let mut handled = self.ctx.take_dye_handled_request_names();
        strip_request_dye_once_core(self.inner.req_header_mut(), &plan, &mut handled);
        self.ctx.restore_dye_handled_request_names(handled);
    }

    fn request_headers_end_stream(&mut self) -> Option<bool> {
        self.inner.as_downstream_mut().request_headers_end_stream()
    }

    fn is_grpc_request(&self) -> bool {
        self.ctx.request_info.is_grpc_request || self.ctx.is_grpc_route_matched
    }

    fn queue_local_reply_header_op(&mut self, op: PendingHeaderOp) -> PluginSessionResult<()> {
        validate_response_header_mutation(op.name())?;
        self.ctx.queue_pending_response_header_op(op);
        Ok(())
    }

    fn current_dye_plan(&self) -> Option<Arc<CompiledDyePlan>> {
        self.ctx.current_dye_plan()
    }

    #[allow(private_interfaces)]
    fn replace_plugin_entry_identity(
        &mut self,
        identity: Option<crate::ctx::CurrentPluginEntryIdentity>,
    ) -> Option<crate::ctx::CurrentPluginEntryIdentity> {
        self.ctx.replace_plugin_entry_identity(identity)
    }

    #[allow(private_interfaces)]
    fn restore_plugin_entry_identity(&mut self, previous: Option<crate::ctx::CurrentPluginEntryIdentity>) {
        let _ = self.ctx.replace_plugin_entry_identity(previous);
    }

    #[allow(private_interfaces)]
    fn install_request_body_observer(
        &mut self,
        factory: crate::request_body::observer::RequestBodyObserverFactory,
        limits: crate::request_body::observer::ObserverLimits,
    ) -> bool {
        let refuse = |factory| {
            let _ = edgion_common::startup::catch_redacted_panic(|| drop(factory));
            false
        };
        if !self.ctx.request_info.in_request_body_stage || self.request_headers_end_stream().is_none() {
            return refuse(factory);
        }
        let req = self.inner.req_header();
        if req.method == http::Method::CONNECT
            || pingora_core::protocols::http::v1::common::is_upgrade_req(req)
            || req.version < http::Version::HTTP_11
        {
            return refuse(factory);
        }
        let Some(identity) = self.ctx.current_plugin_entry_identity() else {
            return refuse(factory);
        };
        if self.ctx.request_body_observers_are_full() {
            return refuse(factory);
        }
        if let Some(observers) = self.ctx.request_body_observers_mut() {
            return observers.try_install(identity, factory, limits);
        }

        let mut observers = Box::new(crate::request_body::observer::RequestBodyObservers::default());
        if !observers.try_install(identity, factory, limits) {
            return false;
        }
        self.ctx.install_request_body_observers(observers);
        true
    }

    fn install_request_body_stream_handler(
        &mut self,
        handler: Box<dyn crate::plugins::runtime::traits::request_body_stream::RequestBodyStreamHandler>,
    ) -> bool {
        if self.ctx.waf_prefix_inspected {
            return false;
        }
        // The transport fact answers `None` for session types that cannot be
        // driven per chunk (custom-connector, subrequest) — refuse so the
        // caller applies its failure policy.
        if !self.ctx.request_info.in_request_body_stage || self.request_headers_end_stream().is_none() {
            return false;
        }
        // Reject intrinsically incompatible downstream shapes before a
        // Streamed plan can be frozen. The proxy independently fails closed if
        // an attempt-local hook later synthesizes upgrade/CONNECT or H1.0:
        // length-changing mutation under ordinary/tunnel framing can desync an
        // upstream, and post-101 tunnel bytes must never reach an inspection
        // processor as pseudo body chunks.
        let req = self.inner.req_header();
        if req.method == http::Method::CONNECT
            || pingora_core::protocols::http::v1::common::is_upgrade_req(req)
            || req.version < http::Version::HTTP_11
        {
            return false;
        }
        self.ctx.push_request_body_stream_handler(handler);
        true
    }

    fn record_outcome(&mut self, outcome: DyeOutcome, value: &str) {
        let Some(plan) = self.ctx.current_dye_plan() else {
            return;
        };
        // Two explicit loops (request then response) rather than a chained
        // iterator with a `ptr::eq` direction check: same behavior, no
        // borrow-checker fight between the immutable rule reference and the
        // `&mut self` header write below.
        for rule in &plan.request {
            if !rule.on.contains(&outcome) {
                continue;
            }
            if let Some(conds) = &rule.conditions {
                if conds.should_evaluate() && conds.evaluate(self) == EvaluationResult::Skip {
                    continue;
                }
            }
            let v = self.dye_rule_value(rule, plan.type_prefix, outcome, value);
            let _ = self.set_request_header(&rule.name, &v);
        }
        for rule in &plan.response {
            if !rule.on.contains(&outcome) {
                continue;
            }
            if let Some(conds) = &rule.conditions {
                if conds.should_evaluate() && conds.evaluate(self) == EvaluationResult::Skip {
                    continue;
                }
            }
            let v = self.dye_rule_value(rule, plan.type_prefix, outcome, value);
            if outcome == DyeOutcome::Success {
                // Set semantics — dual-mode via the existing adapter logic:
                // bound response header (response stages) writes directly;
                // otherwise queued. Going through `set_response_header`
                // instead of pushing onto `pending_response_header_ops`
                // directly is load-bearing (D-1 fix): a response-stage
                // record must land in the bound header, not the queue.
                let _ = self.set_response_header(&rule.name, &v);
            } else {
                let _ = self.append_response_header(&rule.name, &v);
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/plugins/runtime/session_adapter_test/local_reply_tests.rs"]
mod local_reply_tests;

#[cfg(test)]
#[path = "../../../tests/unit/plugins/runtime/session_adapter_test/dye_strip_tests.rs"]
mod dye_strip_tests;

#[cfg(test)]
#[path = "../../../tests/unit/plugins/runtime/session_adapter_test/route_meta_tests.rs"]
mod route_meta_tests;

#[cfg(test)]
#[path = "../../../tests/unit/plugins/runtime/session_adapter_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/unit/plugins/runtime/session_adapter_test/record_outcome_tests.rs"]
pub(crate) mod record_outcome_tests;
