use async_trait::async_trait;
use bytes::Bytes;
use http::Uri;
use pingora_core::protocols::http::RequestBodyBuffer;
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::Session;
use std::collections::HashMap;
use std::sync::Arc;

use super::conditions::{EvaluationResult, PluginConditionsExt};
use super::dye_plan::{CompiledDyePlan, CompiledDyeRule};
use super::log::{EdgionPluginsLog, EdgionPluginsLogToken, PluginLog};
use super::traits::session::{BodyReadError, BodyReadResult, RequestBodySnapshotReader, RouteProto};
use super::traits::{DyeOutcome, PluginSession, PluginSessionError, PluginSessionResult};
use crate::ai::{AiAttemptPlan, AiRequestContext};
use crate::link_sys::template::evaluate_template;
use crate::request_body::BodyStorage;
use crate::request_body::{BodyOperationError, BodyScanResult};
use crate::runtime::server::{http_to_grpc_status, send_grpc_trailers_only_frame};
use crate::{EdgionHttpContext, MirrorState, PendingHeaderOp, ResponseStageDenial, SetJumpOutcome, UpstreamJumpTarget};
use edgion_resources::common::{KeyGet, KeySet, WebhookExtract};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::sanitize_field_value;
use edgion_resources::EdgionStatus;

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

/// Read one cookie from a request's `Cookie` header.
///
/// Free function so both `PluginSession::get_cookie` and the access-log `ext`
/// collector share one parser — the collector cannot build a
/// `PingoraSessionAdapter` (that needs `&mut` borrows the logging stage does not
/// have).
pub fn cookie_value(headers: &http::HeaderMap, name: &str) -> Option<String> {
    headers.get("Cookie").and_then(|v| v.to_str().ok()).and_then(|cookies| {
        cookies.split(';').find_map(|pair| {
            let mut parts = pair.trim().splitn(2, '=');
            let key = parts.next()?;
            if key == name {
                parts.next().map(|v| v.to_string())
            } else {
                None
            }
        })
    })
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

pub struct PingoraSessionAdapter<'a> {
    inner: &'a mut Session,
    ctx: &'a mut EdgionHttpContext,
    response_header: Option<&'a mut ResponseHeader>,
    /// Set to true after a gRPC trailers-only error response has been sent.
    /// Subsequent write_response_body and shutdown calls become no-ops.
    grpc_response_terminated: bool,
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
    body: Option<String>,
    is_head: bool,
) -> bool {
    if ctx.request_info.is_grpc_request {
        return false;
    }
    let Ok(code) = http::StatusCode::from_u16(status) else {
        return false;
    };
    let bodyless = is_head || matches!(status, 204 | 304);
    let Ok(mut fresh) = ResponseHeader::build(code, None) else {
        return false;
    };
    let body_bytes = body.map(bytes::Bytes::from).unwrap_or_default();
    if !bodyless {
        let _ = fresh.insert_header("content-type", "text/plain");
    }
    if let Some(tid) = ctx.request_info.x_trace_id.as_deref() {
        let _ = fresh.insert_header("x-trace-id", tid);
    }
    // Apply queued plugin header ops, then re-assert framing so a queued
    // Set/Remove Content-Length cannot desync the wire from the body.
    ctx.drain_pending_response_header_ops(&mut fresh);
    if !bodyless {
        let _ = fresh.remove_header(&http::header::TRANSFER_ENCODING);
        let _ = fresh.insert_header("content-length", body_bytes.len().to_string());
    }
    ctx.local_reply_header = Some(Box::new(fresh));
    ctx.local_reply_body = Some(if bodyless { bytes::Bytes::new() } else { body_bytes });
    ctx.add_info(EdgionStatus::PluginResponseReplaced);
    true
}

impl<'a> PingoraSessionAdapter<'a> {
    #[inline]
    pub fn new(inner: &'a mut Session, ctx: &'a mut EdgionHttpContext) -> Self {
        Self {
            inner,
            ctx,
            response_header: None,
            grpc_response_terminated: false,
        }
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
            grpc_response_terminated: false,
        }
    }

    #[inline]
    pub fn set_terminate(&mut self) {
        self.ctx.plugin_running_result = PluginRunningResult::ErrTerminateRequest;
    }

    async fn ensure_bodyless_request_replay_owner(&mut self) -> Result<(), BodyReadError> {
        if !self.ctx.request_body_config.enabled {
            return Err(BodyReadError::Unimplemented);
        }
        if !self.ctx.request_info.in_request_body_stage {
            self.ctx.add_error(EdgionStatus::BodyRefusedStage);
            return Err(BodyReadError::NotBuffered);
        }
        if self.ctx.request_body.has_cached_body() {
            return Ok(());
        }
        if self.ctx.request_info.discover_protocol.as_deref() == Some("websocket")
            || self.inner.req_header().method == http::Method::CONNECT
        {
            self.ctx.add_error(EdgionStatus::BodyRefusedWebsocket);
            return Err(BodyReadError::NotBuffered);
        }
        if self.ctx.request_info.is_grpc_request || self.ctx.is_grpc_route_matched {
            self.ctx.add_error(EdgionStatus::BodyRefusedGrpc);
            return Err(BodyReadError::NotBuffered);
        }
        if self.ctx.request_info.body_declared != Some(false) {
            return Err(BodyReadError::Incomplete);
        }

        let (mut storage, view) = BodyStorage::new(
            self.ctx.request_body_config.memory_buffer_bytes,
            self.ctx.request_body_config.max_body_size_bytes,
        );
        storage.finish().await.map_err(|_| BodyReadError::Incomplete)?;
        self.inner
            .set_bodyless_request_replay_buffer(Box::new(storage))
            .map_err(|_| BodyReadError::Incomplete)?;
        self.ctx.request_body.install(view);
        Ok(())
    }

    fn reject_over_limit_request_body(&mut self, limit: u64) -> BodyReadError {
        // HTTP/1.x cannot be reused with unread request bytes. For HTTP/2 this
        // is intentionally a no-op: after the 413 response closes its send
        // side, dropping the request session resets only the unfinished stream.
        self.inner.set_keepalive(None);
        BodyReadError::BufferLimitExceeded { limit }
    }

    async fn capture_request_body(&mut self, max_bytes: Option<u64>) -> Result<u64, BodyReadError> {
        if let Some(len) = self.ctx.request_body.cached_len()? {
            return Ok(len);
        }
        if !self.ctx.request_body_config.enabled {
            return Err(BodyReadError::Unimplemented);
        }

        let cap = resolve_cap(max_bytes, self.ctx.request_body_config.max_body_size_bytes);
        if self
            .inner
            .req_header()
            .headers
            .get(http::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|declared| declared > cap)
        {
            return Err(self.reject_over_limit_request_body(cap));
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
            // This hop has accepted the expectation. Do not forward it and
            // provoke a second 100 response from the upstream.
            self.inner.req_header_mut().remove_header(&http::header::EXPECT);
        }

        let (storage, view) = BodyStorage::new(
            self.ctx.request_body_config.memory_buffer_bytes,
            self.ctx.request_body_config.max_body_size_bytes,
        );
        self.inner
            .set_request_body_buffer(Box::new(storage))
            .map_err(|_| BodyReadError::Incomplete)?;

        let mut captured = 0u64;
        loop {
            let chunk = match self.inner.read_request_body().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(err) => {
                    return Err(match err.etype() {
                        pingora_core::ErrorType::ReadTimedout => BodyReadError::Timeout,
                        pingora_core::ErrorType::HTTPStatus(413) => self.reject_over_limit_request_body(cap),
                        // Gateway-local capture failure (spill-file IO, capture
                        // poisoned mid-chunk). Kept apart from the client-caused
                        // `Incomplete` so the access log can alert on it.
                        pingora_core::ErrorType::InternalError => BodyReadError::StorageFailure,
                        _ => BodyReadError::Incomplete,
                    });
                }
            };
            captured = match captured.checked_add(chunk.len() as u64) {
                Some(total) => total,
                None => return Err(self.reject_over_limit_request_body(cap)),
            };
            if captured > cap {
                return Err(self.reject_over_limit_request_body(cap));
            }
        }

        let len = view.len()?;
        self.ctx.request_body.install(view);
        Ok(len)
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
    /// point — `response_committed` is only set by the local-reply/error paths),
    /// and a plugin's denial must be able to replace it. Keying the refusal on
    /// the request header instead let any request carrying an unrecognized
    /// `Upgrade` token drop a plugin's `ErrResponse` on the floor and forward the
    /// upstream response unmodified. `CONNECT` is refused only once the tunnel is
    /// actually established (2xx); an upstream-rejected CONNECT is an ordinary
    /// replaceable response.
    pub fn apply_response_local_reply(&mut self, status: u16, body: Option<String>) -> bool {
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

    /// Record that a response-phase `ErrResponse` on a gRPC route could not be
    /// expressed as a response replacement.
    ///
    /// Not a termination marker — [`Self::set_response_denied`] is what enforces
    /// the denial, and both are recorded together. This one narrows the access
    /// log to what the gRPC translation cost: the plugin's body is dropped and
    /// its status only survives as far as `http_to_grpc_status` maps it.
    pub fn note_response_local_reply_unsupported(&mut self) {
        self.ctx.add_error(EdgionStatus::PluginResponseReplaceUnsupportedGrpc);
    }

    /// Arm the response-stage abort for an `ErrResponse` that
    /// [`Self::apply_response_local_reply`] refused to stash.
    ///
    /// A refusal means "this exchange cannot carry a replacement response", NOT
    /// "let the upstream response through". The response-stage entry point reads
    /// [`EdgionHttpContext::response_denial`] after the plugin chains and aborts,
    /// so the denial is downstream-visible.
    ///
    /// The access-log status is overwritten with the plugin's status on purpose:
    /// `upstream_response_filter` already stamped the upstream's own status, so
    /// `fail_to_proxy`'s `status.is_none()` fill-in would never fire and the log
    /// would attribute the request to the response the client never received.
    pub fn set_response_denied(&mut self, status: u16) {
        self.ctx.request_info.status = Some(status);
        self.ctx.response_denial = Some(ResponseStageDenial::Http(status));
        self.ctx.add_error(EdgionStatus::PluginResponseDenyAborted);
    }

    /// Arm the response-stage abort for a naked `ErrTerminateRequest`.
    ///
    /// Mirrors the request stage's contract for the same result
    /// (`error_response::terminate_with_fallback`): a plugin that terminates
    /// without writing a response is a plugin bug, answered with a 502 fallback
    /// and [`EdgionStatus::PluginTerminateWithoutResponse`]. The response stage
    /// had no equivalent — it returned "terminate" and the upstream response was
    /// forwarded anyway, unconditionally, with no marker at all.
    pub fn set_response_terminate_without_reply(&mut self) {
        self.ctx.add_error(EdgionStatus::PluginTerminateWithoutResponse);
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
        self.ctx.request_info.status = Some(200);
        self.ctx.request_info.grpc_status = Some(status);
        self.ctx.response_denial = Some(ResponseStageDenial::Grpc { status, message });
        self.ctx.add_error(EdgionStatus::PluginResponseDenyAborted);
    }

    /// Whether the in-flight request is gRPC/gRPC-Web. Used by the response
    /// runner to record the gRPC-unsupported marker only for genuine gRPC
    /// routes (other local-reply refusals — Upgrade/1xx — are not gRPC).
    pub fn is_grpc_request(&self) -> bool {
        self.ctx.request_info.is_grpc_request
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
        self.ctx.request_info.status = Some(200);
        self.ctx.request_info.grpc_status = Some(grpc_status);
        self.ctx.request_info.response_committed = true;
        self.grpc_response_terminated = true;

        let mut resp = ResponseHeader::build(200, None).map_err(|e| Box::new(e) as PluginSessionError)?;
        // Apply queued ops first so request-stage plugins (e.g. cookie issuance) still land
        // their headers, then strip framing headers and re-assert content-type LAST so a queued
        // Set cannot break the gRPC contract. On a body-less single HEADERS+END_STREAM frame a
        // queued content-length / transfer-encoding would be a framing inconsistency strict
        // clients reject, so remove them after draining.
        self.ctx.drain_pending_response_header_ops(&mut resp);
        resp.remove_header(&http::header::CONTENT_LENGTH);
        resp.remove_header(&http::header::TRANSFER_ENCODING);
        resp.insert_header("content-type", "application/grpc")
            .map_err(|e| Box::new(e) as PluginSessionError)?;
        send_grpc_trailers_only_frame(self.inner, resp, grpc_status, message)
            .await
            .map_err(|e| Box::new(e) as PluginSessionError)
    }

    /// Resolve a key value by calling an external webhook service.
    ///
    /// Looks up the WebhookRuntime from the global registry, performs health/rate-limit
    /// checks, sends the HTTP request with retry, and extracts the value from the response.
    ///
    /// The outbound request shape (method, path, headers, body) is defined entirely
    /// by the webhook resource and applied inside `webhook_call`; the consumer only
    /// supplies the `session` and an optional `path_override`.
    async fn resolve_webhook(
        &self,
        webhook_ref: &str,
        path_override: Option<&str>,
        extract: &WebhookExtract,
    ) -> Option<String> {
        use crate::link_sys::providers::webhook::resolve_webhook_key;
        resolve_webhook_key(self, webhook_ref, None, path_override, extract).await
    }

    /// Extract path parameters from route pattern (lazy extraction).
    ///
    /// Parses the route pattern (e.g., "/api/:uid/profile") against the actual
    /// request path (e.g., "/api/123/profile") and extracts named parameters.
    fn extract_path_params(&mut self) {
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

        self.ctx.path_params = Some(params);
    }

    /// Send a gRPC error response as a single-frame Trailers-Only response (HEADERS+END_STREAM),
    /// converting an HTTP status code to the corresponding gRPC status code. Mirrors
    /// `send_grpc_error_direct` but is the dispatch target for the `write_response_header`
    /// trait path when `is_grpc_request` and an error status are seen.
    async fn send_grpc_trailers_only(
        inner: &mut Session,
        ctx: &mut EdgionHttpContext,
        http_status: u16,
    ) -> PluginSessionResult<()> {
        let (grpc_status, grpc_message) = http_to_grpc_status(http_status);
        ctx.request_info.status = Some(200);
        ctx.request_info.grpc_status = Some(grpc_status);
        ctx.request_info.response_committed = true;

        let mut resp = ResponseHeader::build(200, None).map_err(|e| Box::new(e) as PluginSessionError)?;
        // Drain queued response-header ops, strip framing headers, then re-assert content-type
        // LAST. See send_grpc_error_direct for why content-length / transfer-encoding are removed.
        ctx.drain_pending_response_header_ops(&mut resp);
        resp.remove_header(&http::header::CONTENT_LENGTH);
        resp.remove_header(&http::header::TRANSFER_ENCODING);
        resp.insert_header("content-type", "application/grpc")
            .map_err(|e| Box::new(e) as PluginSessionError)?;
        send_grpc_trailers_only_frame(inner, resp, grpc_status, Some(grpc_message))
            .await
            .map_err(|e| Box::new(e) as PluginSessionError)
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
        let mut params = HashMap::new();

        let pattern_segments: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
        let actual_segments: Vec<&str> = actual_path.split('/').filter(|s| !s.is_empty()).collect();

        for (i, pat_seg) in pattern_segments.iter().enumerate() {
            // Check if this is a parameter segment (starts with ':' but not '::')
            if pat_seg.starts_with(':') && !pat_seg.starts_with("::") {
                // Extract parameter name (skip the ':')
                let param_name = &pat_seg[1..];
                if !param_name.is_empty() {
                    if let Some(&actual_value) = actual_segments.get(i) {
                        params.insert(param_name.to_string(), actual_value.to_string());
                    }
                }
            }
        }

        params
    }
}

/// Outcome of the request-side body admission predicate. Kept pure and
/// separate from the adapter so the ordering — which IS the semantics — is
/// unit-testable without a live `Session`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestAdmission {
    /// The request truthfully declared no body.
    PassThrough,
    /// We decline to buffer. Always fail-close; never softened by policy.
    Refuse(EdgionStatus),
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
        None => return RequestAdmission::Refuse(EdgionStatus::BodyRefusedNoSnapshot),
        Some(true) => {}
    }

    // 2. Wrong stage: the request body is long gone once request-stage
    //    processing has completed (any of: the response header is bound, the
    //    upstream response body stage, or any later stage).
    if past_request_stage {
        return RequestAdmission::Refuse(EdgionStatus::BodyRefusedStage);
    }

    // 3. Declared a body AND claims WebSocket — an anomalous shape no
    //    legitimate client produces.
    if ctx.request_info.discover_protocol.as_deref() == Some("websocket") {
        return RequestAdmission::Refuse(EdgionStatus::BodyRefusedWebsocket);
    }

    // 4. gRPC, by EITHER fact. `is_grpc_request` is client-controlled and
    //    `is_grpc_route_matched` misses gRPC that fell through to HTTPRoute on
    //    a listener with no GRPCRoutes; each covers the other's blind spot.
    //    Widening a refusal with a client-controlled fact is safe precisely
    //    because a refusal cannot be softened into a skip.
    if ctx.request_info.is_grpc_request || ctx.is_grpc_route_matched {
        return RequestAdmission::Refuse(EdgionStatus::BodyRefusedGrpc);
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
        return RequestAdmission::Refuse(EdgionStatus::BodyRefusedStage);
    };

    // 2. Only a real 101 is a tunnel.
    if status == 101 {
        return RequestAdmission::Refuse(EdgionStatus::BodyRefusedWebsocket);
    }

    // 3. A gRPC response body does not end until the RPC does.
    if ctx.request_info.is_grpc_request || ctx.is_grpc_route_matched {
        return RequestAdmission::Refuse(EdgionStatus::BodyRefusedGrpc);
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
        // Lazy extraction: only parse on first call
        if self.ctx.path_params.is_none() {
            self.extract_path_params();
        }
        self.ctx.path_params.as_ref()?.get(name).cloned()
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
        if self.ctx.request_info.is_grpc_request && http_status != 200 {
            self.grpc_response_terminated = true;
            return Self::send_grpc_trailers_only(self.inner, self.ctx, http_status).await;
        }
        let mut resp = resp;
        if !self.ctx.pending_response_header_ops.is_empty() {
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
            let _ = resp.remove_header(&http::header::CONTENT_LENGTH);
            if let Some(cl) = orig_cl {
                let _ = resp.insert_header(http::header::CONTENT_LENGTH, cl);
            }
            let _ = resp.remove_header(&http::header::TRANSFER_ENCODING);
            if let Some(te) = orig_te {
                let _ = resp.insert_header(http::header::TRANSFER_ENCODING, te);
            }
        }
        let result = self
            .inner
            .write_response_header(resp, end_of_stream)
            .await
            .map_err(|e| Box::new(e) as PluginSessionError);
        if result.is_ok() {
            self.ctx.request_info.response_committed = true;
        }
        result
    }

    fn set_response_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()> {
        if let Some(resp) = &mut self.response_header {
            resp.insert_header(name.to_string(), value.to_string())
                .map_err(|e| Box::new(e) as PluginSessionError)?;
        } else {
            self.ctx.pending_response_header_ops.push(PendingHeaderOp::Set {
                name: name.to_string(),
                value: value.to_string(),
            });
        }
        Ok(())
    }

    fn append_response_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()> {
        if let Some(resp) = &mut self.response_header {
            resp.append_header(name.to_string(), value.to_string())
                .map_err(|e| Box::new(e) as PluginSessionError)?;
        } else {
            self.ctx.pending_response_header_ops.push(PendingHeaderOp::Append {
                name: name.to_string(),
                value: value.to_string(),
            });
        }
        Ok(())
    }

    fn remove_response_header(&mut self, name: &str) -> PluginSessionResult<()> {
        if let Some(resp) = &mut self.response_header {
            resp.remove_header(name);
        } else {
            self.ctx
                .pending_response_header_ops
                .push(PendingHeaderOp::Remove { name: name.to_string() });
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

    fn get_response_header(&self, name: &str) -> Option<String> {
        self.response_header
            .as_ref()
            .and_then(|resp| resp.headers.get(name))
            .and_then(|value| value.to_str().ok())
            .map(|s| s.to_string())
    }

    fn get_response_status(&self) -> Option<u16> {
        self.response_header.as_ref().map(|h| h.status.as_u16())
    }

    fn set_response_status(&mut self, status: u16) -> PluginSessionResult<()> {
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
        Ok(())
    }

    fn append_request_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()> {
        // Append as a new header field line (preserves every existing field line
        // per RFC 7230 §3.2.2 list semantics; insert_header would replace all).
        self.inner
            .req_header_mut()
            .append_header(name.to_string(), value.to_string())
            .map_err(|e| Box::new(e) as PluginSessionError)?;
        Ok(())
    }

    fn remove_request_header(&mut self, name: &str) -> PluginSessionResult<()> {
        self.inner.req_header_mut().remove_header(name);
        Ok(())
    }

    fn set_request_query(&mut self, query: &str) -> PluginSessionResult<()> {
        // Rebuild the request target from the current live path plus the new
        // query, leaving the path byte-for-byte. Deliberately does NOT set
        // `ctx.uri_finalized`, so the upstream-peer stage still applies the
        // raw-vs-normalized path-forwarding decision. The path is unchanged,
        // so the normalized-path cache stays consistent (it is query-agnostic).
        let path = self.inner.req_header().uri.path().to_string();
        let new_target = if query.is_empty() {
            path
        } else {
            format!("{}?{}", path, query)
        };
        let parsed_uri = new_target
            .parse::<Uri>()
            .map_err(|e| Box::new(e) as PluginSessionError)?;
        self.inner.req_header_mut().set_uri(parsed_uri);
        Ok(())
    }

    fn set_upstream_uri(&mut self, uri: &str) -> PluginSessionResult<()> {
        let parsed_uri = uri.parse::<Uri>().map_err(|e| Box::new(e) as PluginSessionError)?;
        self.inner.req_header_mut().set_uri(parsed_uri);
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
        self.inner
            .write_response_body(body, end_of_stream)
            .await
            .map_err(|e| Box::new(e) as PluginSessionError)
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

    fn client_addr(&self) -> &str {
        &self.ctx.request_info.client_addr
    }
    fn remote_addr(&self) -> &str {
        &self.ctx.request_info.remote_addr
    }

    fn set_remote_addr(&mut self, addr: &str) -> PluginSessionResult<()> {
        self.ctx.request_info.remote_addr = addr.to_string();
        Ok(())
    }

    fn set_trace_id(&mut self, value: &str) -> PluginSessionResult<()> {
        self.ctx.request_info.x_trace_id = Some(value.to_string());
        Ok(())
    }

    fn client_cert_info(&self) -> Option<crate::ctx::ClientCertInfo> {
        self.ctx.request_info.client_cert_info.clone()
    }

    fn downstream_is_tls(&self) -> Option<bool> {
        // Same source as the session-persistence cookie `Secure` decision:
        // the downstream digest carries an ssl_digest only for a connection
        // that completed a TLS handshake on this listener. A missing digest is
        // "unknown", not "plaintext".
        self.inner.as_downstream().digest().map(|d| d.ssl_digest.is_some())
    }

    fn ctx(&self) -> &EdgionHttpContext {
        self.ctx
    }

    fn add_error(&mut self, status: edgion_resources::EdgionStatus) {
        self.ctx.add_error(status);
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
        self.ctx.stream_processors.push(processor);
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

        // `in_request_body_stage` (set at request entry, cleared in
        // `upstream_request_filter` once the upstream is connected and the
        // request is about to be written — see that field's doc) is the
        // ctx-snapshot source of truth for "are we still in the request
        // stage", covering every post-request-stage
        // construction site uniformly (response-header-bound stages, the
        // upstream response *body* stage, and beyond) rather than the
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
                self.ctx.add_error(status);
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

    async fn read_response_body(&mut self, max_bytes: Option<u64>) -> BodyReadResult {
        // Copy the facts out before touching `self.ctx` mutably.
        let status = self.response_header.as_ref().map(|h| h.status.as_u16());
        // Same unanimity rule as `is_head` below, for the other half of the
        // bodyless short-circuit. The bound response header is response-stage
        // mutable — `set_response_status` writes straight into it, reachable
        // from DSL's `resp.set_status` and from `response_rewrite` — so a
        // `resp.set_status(204)` ahead of a body consumer in the same array
        // would otherwise fabricate `Ok(Bytes::new())` for a response whose
        // body is real and is on its way to the client. `ctx.request_info.status`
        // is the upstream's own status, stamped in `upstream_response_filter`
        // before any response-stage plugin runs. `None` (never stamped) yields
        // `false` -> `Buffer`, the loud direction.
        let bodyless_agreed =
            status.is_some_and(is_bodyless_status) && self.ctx.request_info.status.is_some_and(is_bodyless_status);
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
                self.ctx.add_error(status);
                Err(BodyReadError::NotBuffered)
            }
            RequestAdmission::Buffer => {
                // Not implementable at this stage: `response_filter` is handed
                // only the downstream Session, and the upstream body arrives
                // via the per-chunk `upstream_response_body_filter` callback.
                // The chunk-accumulate-then-replay mechanism is P0's spike.
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
        use sha2::Digest;
        Ok(sha2::Sha256::digest([]).into())
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
        self.ctx.request_info.grpc_service.clone()
    }

    fn grpc_method(&self) -> Option<String> {
        self.ctx.request_info.grpc_method.clone()
    }

    fn resolved_route_ext_ref(&self, index: usize) -> Option<std::sync::Arc<crate::plugins::runtime::PluginRuntime>> {
        self.ctx
            .resolved_route_ext_refs
            .as_ref()?
            .get(index)?
            .as_ref()
            .map(std::sync::Arc::clone)
    }

    fn push_edgion_plugins_log(&mut self, log: EdgionPluginsLog) {
        self.ctx.pending_edgion_plugins_logs.push(log);
    }

    fn start_edgion_plugins_log(&mut self, name: String) -> EdgionPluginsLogToken {
        let idx = self.ctx.pending_edgion_plugins_logs.len();
        self.ctx
            .pending_edgion_plugins_logs
            .push(EdgionPluginsLog { name, logs: Vec::new() });
        EdgionPluginsLogToken::new(idx)
    }

    fn push_to_edgion_plugins_log(&mut self, token: &EdgionPluginsLogToken, log: PluginLog) {
        if let Some(edgion_log) = self.ctx.pending_edgion_plugins_logs.get_mut(token.idx()) {
            edgion_log.logs.push(log);
        }
    }

    async fn key_get(&self, key: &KeyGet) -> Option<String> {
        match key {
            // ---- Sync variants: zero-cost, no actual await ----
            KeyGet::ClientIp => {
                let ip = self.remote_addr();
                if ip.is_empty() {
                    None
                } else {
                    Some(ip.to_string())
                }
            }
            KeyGet::Header { name } => self.header_value(name),
            KeyGet::Cookie { name } => self.get_cookie(name),
            KeyGet::Query { name } => self.get_query_param(name),
            KeyGet::Path => Some(self.get_path().to_string()),
            KeyGet::Method => Some(self.get_method().to_string()),
            KeyGet::Ctx { name } => self.get_ctx_var(name),
            KeyGet::ClientIpAndPath => {
                let ip = self.remote_addr();
                if ip.is_empty() {
                    None
                } else {
                    Some(format!("{}:{}", ip, self.get_path()))
                }
            }

            // ---- Async variant: calls external webhook service ----
            KeyGet::Webhook {
                webhook_ref,
                path_override,
                extract,
            } => {
                self.resolve_webhook(webhook_ref, path_override.as_deref(), extract)
                    .await
            }
        }
    }

    fn key_get_local(&self, key: &KeyGet) -> Option<String> {
        match key {
            KeyGet::ClientIp => {
                let ip = self.remote_addr();
                if ip.is_empty() {
                    None
                } else {
                    Some(ip.to_string())
                }
            }
            KeyGet::Header { name } => self.header_value(name),
            KeyGet::Cookie { name } => self.get_cookie(name),
            KeyGet::Query { name } => self.get_query_param(name),
            KeyGet::Path => Some(self.get_path().to_string()),
            KeyGet::Method => Some(self.get_method().to_string()),
            KeyGet::Ctx { name } => self.get_ctx_var(name),
            KeyGet::ClientIpAndPath => {
                let ip = self.remote_addr();
                if ip.is_empty() {
                    None
                } else {
                    Some(format!("{}:{}", ip, self.get_path()))
                }
            }
            KeyGet::Webhook { .. } => None,
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

    fn set_mirror_state(&mut self, state: MirrorState) {
        self.ctx.mirror_state = Some(state);
    }

    /// Install the dye plan of the plugin entry about to be invoked.
    /// Called by every stage loop before each invocation (including an
    /// `ExtensionRef` bundle's own inner iteration — see
    /// `gateway_api_filters::extension_ref`); overwritten per invocation.
    /// `Arc` so `record_outcome` can lift the plan back out of `ctx` before
    /// iterating its rules while mutating `ctx`.
    #[inline]
    fn install_dye_plan(&mut self, plan: Option<Arc<CompiledDyePlan>>) {
        self.ctx.current_dye_plan = plan;
    }

    /// Once-per-name anti-spoof strip: remove the client-supplied value of
    /// each request-direction rule name the first time an entry declares it;
    /// never re-strip (a prior entry's rendered value is gateway-authored).
    /// Runs before the plugin's kill switch / gating / conditions. No-op
    /// when no plan is installed. Core logic lives in the free function
    /// `strip_request_dye_once_core` so it is unit-testable without a full
    /// `Session`.
    fn strip_request_dye_once(&mut self) {
        let Some(plan) = self.ctx.current_dye_plan.clone() else {
            return;
        };
        let mut handled = std::mem::take(&mut self.ctx.dye_handled_request_names);
        strip_request_dye_once_core(self.inner.req_header_mut(), &plan, &mut handled);
        self.ctx.dye_handled_request_names = handled;
    }

    fn request_headers_end_stream(&mut self) -> Option<bool> {
        self.inner.as_downstream_mut().request_headers_end_stream()
    }

    fn current_dye_plan(&self) -> Option<Arc<CompiledDyePlan>> {
        self.ctx.current_dye_plan.clone()
    }

    fn install_request_body_stream_handler(
        &mut self,
        handler: Box<dyn crate::plugins::runtime::traits::request_body_stream::RequestBodyStreamHandler>,
    ) -> bool {
        // The transport fact answers `None` for session types that cannot be
        // driven per chunk (custom-connector, subrequest) — refuse so the
        // caller applies its failure policy.
        if self.request_headers_end_stream().is_none() {
            return false;
        }
        // Preempt the proxy's disposition coercion: upgrade/CONNECT requests
        // and pre-HTTP/1.1 downstreams get their `Streamed` disposition
        // coerced back to `Ordinary` at the transport, while installed
        // handlers would keep running — a length-changing chunk mutation
        // under the preserved Content-Length desyncs the upstream, and
        // post-101 tunnel bytes must never reach an inspection processor as
        // pseudo body chunks. Refuse the install so the caller fails per its
        // policy instead of silently degrading.
        let req = self.inner.req_header();
        if req.method == http::Method::CONNECT
            || pingora_core::protocols::http::v1::common::is_upgrade_req(req)
            || req.version < http::Version::HTTP_11
        {
            return false;
        }
        self.ctx
            .request_body_stream
            .push(sync_wrapper::SyncWrapper::new(handler));
        // Monotonic latch for the retry/disposition predicates; never cleared.
        self.ctx.request_body_stream_active = true;
        true
    }

    fn record_outcome(&mut self, outcome: DyeOutcome, value: &str) {
        let Some(plan) = self.ctx.current_dye_plan.clone() else {
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
                if conds.should_evaluate() && conds.evaluate_sync(self) == EvaluationResult::Skip {
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
                if conds.should_evaluate() && conds.evaluate_sync(self) == EvaluationResult::Skip {
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
mod local_reply_tests {
    use super::*;
    use crate::EdgionHttpContext;

    #[test]
    fn build_stashes_status_and_framed_body() {
        let mut ctx = EdgionHttpContext::default();
        let applied = build_response_local_reply(&mut ctx, 403, Some("{\"error\":\"forbidden\"}".to_string()), false);
        assert!(applied);
        let h = ctx.local_reply_header.as_ref().expect("header stashed");
        assert_eq!(h.status.as_u16(), 403);
        assert_eq!(h.headers.get("content-type").unwrap().to_str().unwrap(), "text/plain");
        assert_eq!(h.headers.get("content-length").unwrap().to_str().unwrap(), "21");
        assert!(h.headers.get("content-encoding").is_none());
        assert_eq!(
            ctx.local_reply_body.as_ref().unwrap().as_ref(),
            b"{\"error\":\"forbidden\"}"
        );
    }

    #[test]
    fn build_none_body_is_zero_length() {
        let mut ctx = EdgionHttpContext::default();
        assert!(build_response_local_reply(&mut ctx, 500, None, false));
        let h = ctx.local_reply_header.as_ref().unwrap();
        assert_eq!(h.headers.get("content-length").unwrap().to_str().unwrap(), "0");
        assert_eq!(ctx.local_reply_body.as_ref().unwrap().len(), 0);
    }

    #[test]
    fn build_head_is_bodyless() {
        let mut ctx = EdgionHttpContext::default();
        assert!(build_response_local_reply(&mut ctx, 403, Some("denied".into()), true));
        let h = ctx.local_reply_header.as_ref().unwrap();
        assert!(h.headers.get("content-length").is_none());
        assert_eq!(ctx.local_reply_body.as_ref().unwrap().len(), 0);
    }

    #[test]
    fn build_204_is_bodyless() {
        let mut ctx = EdgionHttpContext::default();
        assert!(build_response_local_reply(&mut ctx, 204, Some("ignored".into()), false));
        let h = ctx.local_reply_header.as_ref().unwrap();
        assert!(h.headers.get("content-length").is_none());
        assert_eq!(ctx.local_reply_body.as_ref().unwrap().len(), 0);
    }

    #[test]
    fn build_grpc_route_is_refused() {
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.is_grpc_request = true;
        assert!(!build_response_local_reply(&mut ctx, 502, Some("x".into()), false));
        assert!(ctx.local_reply_header.is_none());
        assert!(ctx.local_reply_body.is_none());
    }

    #[test]
    fn build_drains_queued_ops_then_reasserts_content_length() {
        let mut ctx = EdgionHttpContext::default();
        ctx.pending_response_header_ops.push(PendingHeaderOp::Set {
            name: "content-type".to_string(),
            value: "application/json".to_string(),
        });
        ctx.pending_response_header_ops.push(PendingHeaderOp::Set {
            name: "content-length".to_string(),
            value: "999".to_string(),
        });
        assert!(build_response_local_reply(&mut ctx, 502, Some("{}".into()), false));
        let h = ctx.local_reply_header.as_ref().unwrap();
        // queued content-type wins; queued content-length is overridden by the framing re-assert.
        assert_eq!(
            h.headers.get("content-type").unwrap().to_str().unwrap(),
            "application/json"
        );
        assert_eq!(h.headers.get("content-length").unwrap().to_str().unwrap(), "2");
    }

    #[test]
    fn apply_pending_ops_is_nondestructive_and_applies() {
        let mut ctx = EdgionHttpContext::default();
        ctx.pending_response_header_ops.push(PendingHeaderOp::Set {
            name: "x-foo".to_string(),
            value: "bar".to_string(),
        });
        let mut hdr = ResponseHeader::build(200, None).unwrap();
        ctx.apply_pending_response_header_ops(&mut hdr);
        // Applied onto the header...
        assert_eq!(hdr.headers.get("x-foo").unwrap().to_str().unwrap(), "bar");
        // ...but the queue is left intact (non-destructive) for a later local reply.
        assert_eq!(ctx.pending_response_header_ops.len(), 1);
    }

    #[test]
    fn queued_ops_survive_phase6_apply_onto_local_reply() {
        // Simulates the two-phase flow: the sync stage applies queued ops onto
        // the (soon-discarded) upstream header non-destructively; a response-phase
        // plugin then replaces the response, and build_response_local_reply
        // re-applies the still-intact queue onto the fresh reply. Proves BOTH the
        // sync and async local-reply paths preserve the ops, since neither
        // pre-drains the queue.
        let mut ctx = EdgionHttpContext::default();
        ctx.pending_response_header_ops.push(PendingHeaderOp::Set {
            name: "x-foo".to_string(),
            value: "bar".to_string(),
        });
        let mut upstream = ResponseHeader::build(200, None).unwrap();
        ctx.apply_pending_response_header_ops(&mut upstream);
        assert!(build_response_local_reply(&mut ctx, 403, Some("denied".into()), false));
        let reply = ctx.local_reply_header.as_ref().unwrap();
        assert_eq!(reply.headers.get("x-foo").unwrap().to_str().unwrap(), "bar");
        // The builder's destructive drain consumed the queue.
        assert!(ctx.pending_response_header_ops.is_empty());
    }
}

#[cfg(test)]
mod dye_strip_tests {
    use super::*;
    use crate::plugins::runtime::dye_plan::CompiledDyePlan;
    use crate::EdgionHttpContext;
    use edgion_resources::resources::edgion_plugins::DyeSuite;

    fn plan_declaring(name: &str) -> Arc<CompiledDyePlan> {
        let suite: DyeSuite = serde_yaml::from_str(&format!("request:\n  - name: {name}\n")).unwrap();
        CompiledDyePlan::compile(&suite, "canary").unwrap()
    }

    /// `PingoraSessionAdapter::strip_request_dye_once` needs a real `Session`,
    /// which nothing in this codebase constructs in a unit test (see
    /// `PingoraSessionAdapter::new`'s only test-adjacent caller,
    /// `preflight_handler.rs`, which has the same "TODO: need a proper
    /// Session" gap). Drive the exact same ctx + header sequence the adapter
    /// method runs through the pure `strip_request_dye_once_core` instead —
    /// this is the adapter method's body verbatim, minus the `Session` wrapper.
    fn strip_once(ctx: &mut EdgionHttpContext, req: &mut RequestHeader) {
        let Some(plan) = ctx.current_dye_plan.clone() else {
            return;
        };
        let mut handled = std::mem::take(&mut ctx.dye_handled_request_names);
        strip_request_dye_once_core(req, &plan, &mut handled);
        ctx.dye_handled_request_names = handled;
    }

    #[test]
    fn strip_request_dye_once_is_once_per_name() {
        let mut ctx = EdgionHttpContext::default();
        let mut req = RequestHeader::build("GET", b"/", None).unwrap();
        req.insert_header("X-T", "client-forged").unwrap();

        // Entry A declares X-T: strip removes the client-supplied value and
        // marks the name handled.
        ctx.current_dye_plan = Some(plan_declaring("X-T"));
        strip_once(&mut ctx, &mut req);
        assert!(req.headers.get("X-T").is_none());
        assert_eq!(ctx.dye_handled_request_names, vec!["X-T".to_string()]);

        // Simulate entry A's own plugin rendering its dye value onto the
        // request between the two installs.
        req.insert_header("X-T", "gateway-rendered").unwrap();

        // Entry B also declares X-T: no second strip — the gateway-rendered
        // value from entry A survives, and the handled list is not duplicated.
        ctx.current_dye_plan = Some(plan_declaring("X-T"));
        strip_once(&mut ctx, &mut req);
        assert_eq!(req.headers.get("X-T").unwrap().to_str().unwrap(), "gateway-rendered");
        assert_eq!(ctx.dye_handled_request_names, vec!["X-T".to_string()]);
    }

    #[test]
    fn strip_request_dye_once_matches_names_case_insensitively() {
        let mut ctx = EdgionHttpContext::default();
        let mut req = RequestHeader::build("GET", b"/", None).unwrap();
        req.insert_header("x-t", "client-forged").unwrap();

        ctx.current_dye_plan = Some(plan_declaring("X-T"));
        strip_once(&mut ctx, &mut req);
        assert!(req.headers.get("x-t").is_none());

        req.insert_header("X-T", "gateway-rendered").unwrap();
        // A later entry declaring a differently-cased spelling of the same
        // name still matches the already-handled record.
        ctx.current_dye_plan = Some(plan_declaring("x-t"));
        strip_once(&mut ctx, &mut req);
        assert_eq!(req.headers.get("X-T").unwrap().to_str().unwrap(), "gateway-rendered");
    }

    #[test]
    fn strip_request_dye_once_noop_without_installed_plan() {
        let mut ctx = EdgionHttpContext::default();
        let mut req = RequestHeader::build("GET", b"/", None).unwrap();
        req.insert_header("X-T", "client-forged").unwrap();

        // No plan installed (ctx.current_dye_plan is None): nothing stripped.
        strip_once(&mut ctx, &mut req);
        assert_eq!(req.headers.get("X-T").unwrap().to_str().unwrap(), "client-forged");
        assert!(ctx.dye_handled_request_names.is_empty());
    }

    #[test]
    fn strip_request_dye_once_ignores_response_direction_rules() {
        let mut ctx = EdgionHttpContext::default();
        let mut req = RequestHeader::build("GET", b"/", None).unwrap();
        req.insert_header("X-O", "client-forged").unwrap();

        let suite: DyeSuite = serde_yaml::from_str("response:\n  - name: X-O\n").unwrap();
        ctx.current_dye_plan = Some(CompiledDyePlan::compile(&suite, "canary").unwrap());
        strip_once(&mut ctx, &mut req);
        // Response-direction rules never drive the request-side strip.
        assert_eq!(req.headers.get("X-O").unwrap().to_str().unwrap(), "client-forged");
        assert!(ctx.dye_handled_request_names.is_empty());
    }
}

#[cfg(test)]
mod route_meta_tests {
    use super::route_meta_field;
    use kube::api::ObjectMeta;
    use std::collections::BTreeMap;

    fn meta() -> ObjectMeta {
        let mut labels = BTreeMap::new();
        labels.insert("app".to_string(), "web".to_string());
        labels.insert("edgion.io/team".to_string(), "team-a".to_string());
        let mut ann = BTreeMap::new();
        ann.insert("owner".to_string(), "platform".to_string());
        ObjectMeta {
            name: Some("route1".to_string()),
            namespace: Some("ns1".to_string()),
            labels: Some(labels),
            annotations: Some(ann),
            ..Default::default()
        }
    }

    #[test]
    fn resolves_identity_and_metadata() {
        let m = meta();
        assert_eq!(route_meta_field(&m, "name").as_deref(), Some("route1"));
        assert_eq!(route_meta_field(&m, "namespace").as_deref(), Some("ns1"));
        assert_eq!(route_meta_field(&m, "label.app").as_deref(), Some("web"));
        // dotted/slashed key round-trips (split on first '.')
        assert_eq!(route_meta_field(&m, "label.edgion.io/team").as_deref(), Some("team-a"));
        assert_eq!(route_meta_field(&m, "annotation.owner").as_deref(), Some("platform"));
    }

    #[test]
    fn missing_and_unknown_return_none() {
        let m = meta();
        assert_eq!(route_meta_field(&m, "label.absent"), None);
        assert_eq!(route_meta_field(&m, "annotation.absent"), None);
        assert_eq!(route_meta_field(&m, "bogus"), None);
        assert_eq!(route_meta_field(&ObjectMeta::default(), "name"), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_value_finds_named_cookie_among_others() {
        let mut headers = http::HeaderMap::new();
        headers.insert("Cookie", http::HeaderValue::from_static("a=1; b=2"));
        assert_eq!(cookie_value(&headers, "b").as_deref(), Some("2"));
        assert_eq!(cookie_value(&headers, "c"), None);
    }

    #[test]
    fn utf8_header_values_preserves_duplicates_and_rejects_invalid_values() {
        let mut headers = http::HeaderMap::new();
        headers.append("content-type", http::HeaderValue::from_static("application/json"));
        headers.append(
            "content-type",
            http::HeaderValue::from_static("application/problem+json"),
        );
        assert_eq!(
            utf8_header_values(&headers, "content-type"),
            Some(vec![
                "application/json".to_string(),
                "application/problem+json".to_string()
            ])
        );
        assert_eq!(utf8_header_values(&headers, "content-encoding"), Some(Vec::new()));

        headers.append(
            "content-type",
            http::HeaderValue::from_bytes(&[0xff]).expect("obs-text is a valid raw header value"),
        );
        assert_eq!(utf8_header_values(&headers, "content-type"), None);
    }

    #[test]
    fn test_parse_path_params_single_param() {
        let pattern = "/api/:uid/profile";
        let actual = "/api/123/profile";
        let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

        assert_eq!(params.get("uid"), Some(&"123".to_string()));
        assert_eq!(params.len(), 1);
    }

    #[test]
    fn test_parse_path_params_multiple_params() {
        let pattern = "/users/:uid/posts/:post_id";
        let actual = "/users/456/posts/789";
        let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

        assert_eq!(params.get("uid"), Some(&"456".to_string()));
        assert_eq!(params.get("post_id"), Some(&"789".to_string()));
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn test_parse_path_params_no_params() {
        let pattern = "/api/v1/users";
        let actual = "/api/v1/users";
        let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

        assert!(params.is_empty());
    }

    #[test]
    fn test_parse_path_params_escaped_colon() {
        // :: is escaped colon, not a parameter
        let pattern = "/api/::version/data";
        let actual = "/api/:v1/data";
        let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

        assert!(params.is_empty());
    }

    #[test]
    fn test_parse_path_params_param_at_end() {
        let pattern = "/users/:id";
        let actual = "/users/999";
        let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

        assert_eq!(params.get("id"), Some(&"999".to_string()));
    }

    #[test]
    fn test_parse_path_params_prefix_match() {
        // For prefix match, actual path may have more segments
        let pattern = "/api/:version";
        let actual = "/api/v2/users/123";
        let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

        assert_eq!(params.get("version"), Some(&"v2".to_string()));
    }

    #[test]
    fn test_parse_path_params_empty_param_name() {
        // Empty param name should be skipped
        let pattern = "/api/:/data";
        let actual = "/api/test/data";
        let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

        assert!(params.is_empty());
    }

    #[test]
    fn test_parse_path_params_special_characters_in_value() {
        let pattern = "/search/:query";
        let actual = "/search/hello%20world";
        let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

        assert_eq!(params.get("query"), Some(&"hello%20world".to_string()));
    }

    #[test]
    fn resolve_cap_never_treats_none_as_unbounded() {
        // Three production call sites pass `None`. It means "gateway default",
        // never "unbounded".
        assert_eq!(resolve_cap(None, 1024), 1024);
        // A plugin may tighten...
        assert_eq!(resolve_cap(Some(512), 1024), 512);
        // ...but never enlarge past the gateway-wide cap.
        assert_eq!(resolve_cap(Some(4096), 1024), 1024);
    }

    /// Build a request-stage ctx with the given snapshot + protocol facts.
    fn admission_ctx(declared: Option<bool>, proto: Option<&str>, grpc: bool) -> EdgionHttpContext {
        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.body_declared = declared;
        ctx.request_info.discover_protocol = proto.map(str::to_string);
        ctx.request_info.is_grpc_request = grpc;
        ctx
    }

    #[test]
    fn request_admission_no_declared_body_passes_through() {
        let ctx = admission_ctx(Some(false), None, false);
        assert_eq!(request_admission(&ctx, false), RequestAdmission::PassThrough);
    }

    #[test]
    fn request_admission_missing_snapshot_refuses() {
        let ctx = admission_ctx(None, None, false);
        assert_eq!(
            request_admission(&ctx, false),
            RequestAdmission::Refuse(EdgionStatus::BodyRefusedNoSnapshot)
        );
    }

    #[test]
    fn missing_snapshot_and_wrong_stage_are_distinguishable() {
        // Both refuse, but they must not share one status: the missing-snapshot
        // arm is a code-path bug, the wrong-stage arm is expected traffic for
        // any body consumer in a post-request stage. A shared variant would make
        // the bug undetectable in the access log.
        let bug = request_admission(&admission_ctx(None, None, false), false);
        let expected = request_admission(&admission_ctx(Some(true), None, false), true);
        assert_eq!(bug, RequestAdmission::Refuse(EdgionStatus::BodyRefusedNoSnapshot));
        assert_eq!(expected, RequestAdmission::Refuse(EdgionStatus::BodyRefusedStage));
        assert_ne!(bug, expected);
    }

    #[test]
    fn request_admission_response_stage_refuses() {
        // After the request body is proxied the H1 reader is Complete(n>0), so a
        // naive predicate would fall through to "buffer" and answer a zero-length
        // Ok. That is a silent violation of the Empty/Absent distinction.
        let ctx = admission_ctx(Some(true), None, false);
        assert_eq!(
            request_admission(&ctx, true),
            RequestAdmission::Refuse(EdgionStatus::BodyRefusedStage)
        );
    }

    #[test]
    fn request_admission_websocket_with_body_refuses() {
        let ctx = admission_ctx(Some(true), Some("websocket"), false);
        assert_eq!(
            request_admission(&ctx, false),
            RequestAdmission::Refuse(EdgionStatus::BodyRefusedWebsocket)
        );
    }

    #[test]
    fn request_admission_bodyless_websocket_handshake_passes_through() {
        // A genuine handshake declares no body, so step 1 answers before the
        // websocket check is ever consulted. Order is the semantics.
        let ctx = admission_ctx(Some(false), Some("websocket"), false);
        assert_eq!(request_admission(&ctx, false), RequestAdmission::PassThrough);
    }

    #[test]
    fn request_admission_grpc_refuses_via_either_fact() {
        // Forged content-type (client-controlled) — safe to widen a REFUSAL with.
        let ctx = admission_ctx(Some(true), None, true);
        assert_eq!(
            request_admission(&ctx, false),
            RequestAdmission::Refuse(EdgionStatus::BodyRefusedGrpc)
        );

        // Route-derived, with no gRPC content-type at all.
        let mut ctx = admission_ctx(Some(true), None, false);
        ctx.is_grpc_route_matched = true;
        assert_eq!(
            request_admission(&ctx, false),
            RequestAdmission::Refuse(EdgionStatus::BodyRefusedGrpc)
        );
    }

    #[test]
    fn request_admission_ordinary_post_buffers() {
        let ctx = admission_ctx(Some(true), None, false);
        assert_eq!(request_admission(&ctx, false), RequestAdmission::Buffer);
    }

    #[test]
    fn response_admission_request_stage_refuses() {
        // No response header bound means this was called from the request stage.
        // Without this guard steps 2-4 all miss and step 5 would block on a
        // response that does not exist yet.
        let ctx = EdgionHttpContext::new();
        assert_eq!(
            response_admission(&ctx, None, false, false),
            RequestAdmission::Refuse(EdgionStatus::BodyRefusedStage)
        );
    }

    #[test]
    fn response_admission_real_101_refuses() {
        let ctx = EdgionHttpContext::new();
        assert_eq!(
            response_admission(&ctx, Some(101), false, false),
            RequestAdmission::Refuse(EdgionStatus::BodyRefusedWebsocket)
        );
    }

    #[test]
    fn response_admission_upgrade_request_answered_200_buffers() {
        // The request's protocol label is irrelevant to whether the RESPONSE body
        // is bufferable. Only a real 101 is a tunnel.
        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.discover_protocol = Some("websocket".to_string());
        assert_eq!(
            response_admission(&ctx, Some(200), false, false),
            RequestAdmission::Buffer
        );
    }

    #[test]
    fn response_admission_grpc_refuses() {
        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.is_grpc_request = true;
        assert_eq!(
            response_admission(&ctx, Some(200), false, false),
            RequestAdmission::Refuse(EdgionStatus::BodyRefusedGrpc)
        );
    }

    #[test]
    fn response_admission_bodyless_statuses_pass_through() {
        let ctx = EdgionHttpContext::new();
        for status in [100u16, 199, 204, 304] {
            assert_eq!(
                response_admission(&ctx, Some(status), false, true),
                RequestAdmission::PassThrough,
                "status {status} carries no body"
            );
        }
    }

    #[test]
    fn response_admission_bodyless_status_without_agreement_buffers() {
        // The silent PassThrough needs `bodyless_agreed`, not just a bodyless
        // live status: `read_response_body` only sets it when the upstream's own
        // stamped status agrees, so a plugin rewriting 200 -> 204 ahead of a body
        // consumer cannot fabricate "this response has no body".
        let ctx = EdgionHttpContext::new();
        for status in [100u16, 199, 204, 304] {
            assert_eq!(
                response_admission(&ctx, Some(status), false, false),
                RequestAdmission::Buffer,
                "status {status} must not short-circuit without agreement"
            );
        }
    }

    #[test]
    fn response_admission_head_response_passes_through() {
        // RFC 9110 9.3.2: a HEAD response carries Content-Length but no body.
        // Same class as 204/304 — the one place a request fact legitimately
        // enters the response predicate.
        let ctx = EdgionHttpContext::new();
        assert_eq!(
            response_admission(&ctx, Some(200), true, false),
            RequestAdmission::PassThrough
        );
    }

    #[test]
    fn response_admission_ordinary_200_buffers() {
        let ctx = EdgionHttpContext::new();
        assert_eq!(
            response_admission(&ctx, Some(200), false, false),
            RequestAdmission::Buffer
        );
    }
}

#[cfg(test)]
mod record_outcome_tests {
    use super::*;
    use crate::plugins::http::RequestBodyBuffer as RequestBodyBufferPlugin;
    use crate::plugins::runtime::{BodyGatedRequestFilter, RequestFilter};
    use crate::request_body::DEFAULT_MEMORY_BUFFER_BYTES;
    use edgion_resources::resources::edgion_plugins::BodyRequirement;
    use edgion_resources::resources::edgion_plugins::{DyeOutcome, DyeSuite};
    use pingora_proxy::RequestBodyEvent;
    use sha2::Digest;
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;

    /// Build a minimal real `pingora_proxy::Session` for adapter-level tests.
    ///
    /// `record_outcome`'s real implementation lives on `PingoraSessionAdapter`
    /// (not a generic `&dyn PluginSession` free function), and constructing
    /// the adapter needs a live `&mut Session` field even for a test that
    /// never touches the downstream request. Nothing else in this codebase
    /// builds one (see `dye_strip_tests`'s comment above, which sidesteps the
    /// same gap by testing a pure core function instead) — but pingora's own
    /// `Session::new_h1` doc comment says it "is mostly used for testing and
    /// mocking", and `tokio::io::DuplexStream` implements pingora's `IO`
    /// trait for exactly this purpose
    /// (`pingora_core::protocols::ext_io_impl`). A `Session` needs a request
    /// already read off the wire before any other method is callable, so
    /// this writes a minimal literal request into one end of a duplex pair
    /// and reads it back off the other end.
    async fn test_session() -> Session {
        let (mut client, server) = tokio::io::duplex(4096);
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        client
            .write_all(b"GET /dye-test HTTP/1.1\r\nHost: test\r\n\r\n")
            .await
            .expect("write minimal request into duplex pair");
        drop(client); // header bytes are already buffered; no further writes needed
        session
            .read_request()
            .await
            .expect("read_request should succeed")
            .then_some(())
            .expect("client must not have closed before sending the request");
        session
    }

    /// Like `test_session`, but keeps the client half of the duplex pair
    /// alive and returns it to the caller. `write_response_header` tests
    /// (Task 10) actually write bytes back over the wire; dropping the
    /// client half (as `test_session` does once the request is buffered)
    /// closes the reader on the other end of the duplex, so the response
    /// write fails with `BrokenPipe`. The returned client is unused beyond
    /// keeping the pipe open -- callers just need to hold it until the
    /// session is done writing.
    async fn test_session_with_client() -> (Session, tokio::io::DuplexStream) {
        let (mut client, server) = tokio::io::duplex(4096);
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        client
            .write_all(b"GET /dye-test HTTP/1.1\r\nHost: test\r\n\r\n")
            .await
            .expect("write minimal request into duplex pair");
        session
            .read_request()
            .await
            .expect("read_request should succeed")
            .then_some(())
            .expect("client must not have closed before sending the request");
        (session, client)
    }

    async fn test_session_with_body(body: &[u8]) -> Session {
        let capacity = body.len().saturating_add(1024).max(4096);
        let (mut client, server) = tokio::io::duplex(capacity);
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        let header = format!(
            "POST /body-test HTTP/1.1\r\nHost: test\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        client
            .write_all(header.as_bytes())
            .await
            .expect("write request headers into duplex pair");
        client
            .write_all(body)
            .await
            .expect("write request body into duplex pair");
        drop(client);
        session
            .read_request()
            .await
            .expect("read_request should succeed")
            .then_some(())
            .expect("client must not close before sending the request");
        session
    }

    fn compile_plan(yaml: &str, type_prefix: &'static str) -> Arc<CompiledDyePlan> {
        let suite: DyeSuite = serde_yaml::from_str(yaml).unwrap();
        CompiledDyePlan::compile(&suite, type_prefix).unwrap()
    }

    fn pending_ops(ctx: &EdgionHttpContext) -> &[PendingHeaderOp] {
        &ctx.pending_response_header_ops
    }

    /// `PendingHeaderOp` has no `PartialEq` (nothing else in the codebase
    /// needed one), so assert on the single queued op's shape by matching
    /// instead of deriving one just for this test module.
    fn assert_single_append(ctx: &EdgionHttpContext, expect_name: &str, expect_value: &str) {
        match pending_ops(ctx) {
            [PendingHeaderOp::Append { name, value }] => {
                assert_eq!(name, expect_name);
                assert_eq!(value, expect_value);
            }
            other => panic!("expected exactly one Append op, got {other:?}"),
        }
    }

    fn assert_single_set(ctx: &EdgionHttpContext, expect_name: &str, expect_value: &str) {
        match pending_ops(ctx) {
            [PendingHeaderOp::Set { name, value }] => {
                assert_eq!(name, expect_name);
                assert_eq!(value, expect_value);
            }
            other => panic!("expected exactly one Set op, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn record_outcome_failure_appends_prefixed_value_to_queue() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.current_dye_plan = Some(compile_plan(
            "response:\n  - name: X-O\n    on: [fail-open]\n",
            "rate_limit_redis",
        ));

        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        adapter.record_outcome(DyeOutcome::FailOpen, "redis-unreachable");

        assert_single_append(&ctx, "X-O", "rate_limit_redis:fail-open:redis-unreachable");
    }

    #[tokio::test]
    async fn record_outcome_success_sets_recorded_value_verbatim() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        // Omitted `on` defaults to [Success]; omitted `value` renders the
        // recorded value (no operator template, no provenance prefix).
        ctx.current_dye_plan = Some(compile_plan("response:\n  - name: X-O\n", "canary"));

        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        adapter.record_outcome(DyeOutcome::Success, "app-v1");

        assert_single_set(&ctx, "X-O", "app-v1");
    }

    struct NoopStreamHandler;

    #[async_trait::async_trait]
    impl crate::plugins::runtime::traits::RequestBodyStreamHandler for NoopStreamHandler {
        fn name(&self) -> &str {
            "noop"
        }
        async fn on_chunk(
            &mut self,
            _chunk: &mut Option<Bytes>,
            _event: RequestBodyEvent,
        ) -> crate::plugins::runtime::traits::ChunkVerdict {
            crate::plugins::runtime::traits::ChunkVerdict::Continue
        }
        fn take_log(&mut self) -> Option<PluginLog> {
            None
        }
        fn dye_plan(&self) -> Option<Arc<CompiledDyePlan>> {
            None
        }
    }

    /// A real H1 downstream can answer the transport facts, so the streamed
    /// handler seam installs into the ctx vector and sets the monotonic latch.
    #[tokio::test]
    async fn h1_session_installs_request_body_stream_handler() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert!(adapter.request_headers_end_stream().is_some());
        assert!(adapter.install_request_body_stream_handler(Box::new(NoopStreamHandler)));
        assert_eq!(ctx.request_body_stream.len(), 1);
        assert!(
            ctx.request_body_stream_active,
            "install must set the monotonic retry/disposition latch"
        );
    }

    #[tokio::test]
    async fn record_outcome_no_plan_is_noop() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        // No plan installed at all.
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        adapter.record_outcome(DyeOutcome::FailOpen, "cause");
        assert!(pending_ops(&ctx).is_empty());
    }

    #[tokio::test]
    async fn record_outcome_no_matching_rule_is_noop() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        // Plan installed, but the only rule declares `on: [success]` while
        // the recorded outcome is a failure -- no rule matches.
        ctx.current_dye_plan = Some(compile_plan("response:\n  - name: X-O\n", "canary"));
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        adapter.record_outcome(DyeOutcome::FailOpen, "cause");
        assert!(pending_ops(&ctx).is_empty());
    }

    #[tokio::test]
    async fn record_outcome_template_value_renders_and_sanitizes() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.current_dye_plan = Some(compile_plan(
            "response:\n  - name: X-O\n    on: [fail-open]\n    value: 'v ${ctx:id}'\n",
            "canary",
        ));

        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        // Control bytes on a rendered value can only arrive through a
        // non-header-validated source (an `http::HeaderValue` rejects raw
        // CR/LF outright, so a wire-level request header can never carry
        // them) -- a ctx var set by an earlier plugin (webhook response, JWT
        // claim, ...) is the realistic vector `sanitize_field_value` guards.
        adapter.set_ctx_var("id", "abc\r\ninjected").unwrap();
        adapter.record_outcome(DyeOutcome::FailOpen, "unused");

        assert_single_append(&ctx, "X-O", "v abcinjected");
    }

    #[tokio::test]
    async fn record_outcome_bound_response_header_writes_directly() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.current_dye_plan = Some(compile_plan(
            "response:\n  - name: X-O\n    on: [fail-open]\n",
            "canary",
        ));
        let mut response_header = ResponseHeader::build(200, None).unwrap();

        let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
        adapter.record_outcome(DyeOutcome::FailOpen, "cause");

        // D-1 regression pin: a response-stage record must land directly in
        // the bound header via the adapter's existing dual-mode
        // set_response_header/append_response_header, never bypass it by
        // pushing straight onto `pending_response_header_ops`.
        assert_eq!(
            response_header.headers.get("X-O").unwrap().to_str().unwrap(),
            "canary:fail-open:cause"
        );
        assert!(pending_ops(&ctx).is_empty());
    }

    /// Task 10 (write_response_header drain amendment): a plugin that
    /// self-writes its response via `write_response_header` (the
    /// ErrTerminateRequest fail-close path -- rate_limit_redis,
    /// forward_auth, ldap_auth) must still carry any queued dye/header ops,
    /// same as the runtime's own local-reply / gRPC error builders. But
    /// unlike those builders, the caller here writes the body SEPARATELY
    /// afterwards, so a queued framing op (Set content-length here) must
    /// NOT win over the caller's own framing -- the caller's
    /// content-length/transfer-encoding must be restored after the drain.
    #[tokio::test]
    async fn self_written_response_drains_queue_with_framing_protected() {
        let (mut session, _client) = test_session_with_client().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.pending_response_header_ops.push(PendingHeaderOp::Append {
            name: "X-O".to_string(),
            value: "v1".to_string(),
        });
        // Hostile framing op: without protection this would clobber the
        // caller's real content-length and desync the wire from the body
        // the caller is about to write in a separate call.
        ctx.pending_response_header_ops.push(PendingHeaderOp::Set {
            name: "content-length".to_string(),
            value: "999".to_string(),
        });

        let mut resp = ResponseHeader::build(200, None).unwrap();
        resp.insert_header("content-length", "5").unwrap();

        {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            adapter
                .write_response_header(Box::new(resp), false)
                .await
                .expect("write_response_header should succeed");
        }

        let written = session
            .response_written()
            .expect("header must have been written to the session");
        assert_eq!(written.headers.get("X-O").unwrap().to_str().unwrap(), "v1");
        assert_eq!(
            written.headers.get("content-length").unwrap().to_str().unwrap(),
            "5",
            "caller's original content-length must survive a queued hostile Set"
        );
        assert!(
            written.headers.get("transfer-encoding").is_none(),
            "no transfer-encoding should be introduced by the drain"
        );
        assert!(pending_ops(&ctx).is_empty());
    }

    /// Task 10 regression pin: the gRPC reroute inside `write_response_header`
    /// (status != 200 on a gRPC route) already drained queued ops via
    /// `send_grpc_trailers_only` before this task -- confirm that path is
    /// untouched by the plain-HTTP-arm framing-protection amendment.
    #[tokio::test]
    async fn grpc_reroute_still_drains_via_trailers_path() {
        let (mut session, _client) = test_session_with_client().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.is_grpc_request = true;
        ctx.pending_response_header_ops.push(PendingHeaderOp::Append {
            name: "X-O".to_string(),
            value: "v1".to_string(),
        });

        let resp = ResponseHeader::build(404, None).unwrap();

        {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            adapter
                .write_response_header(Box::new(resp), false)
                .await
                .expect("write_response_header should succeed");
        }

        let written = session
            .response_written()
            .expect("trailers-only header must have been written");
        assert_eq!(written.headers.get("X-O").unwrap().to_str().unwrap(), "v1");
        assert!(written.headers.get("grpc-status").is_some());
        assert_eq!(ctx.request_info.status, Some(200));
        assert!(ctx.request_info.grpc_status.is_some());
        assert!(pending_ops(&ctx).is_empty());
    }

    /// Pins the trait accessor a real adapter exposes over the ctx field,
    /// using the same `test_session()` construction the rest of this module
    /// already relies on for adapter-level tests.
    #[tokio::test]
    async fn declares_body_accessor_forwards_ctx_snapshot() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();

        ctx.request_info.body_declared = Some(true);
        let adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert_eq!(adapter.declares_body(), Some(true));
    }

    #[tokio::test]
    async fn opaque_body_metadata_marks_transport_proven_empty_without_capture() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.body_declared = Some(false);

        {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            assert_eq!(adapter.request_body_available_len(true), Ok(0));
            assert_eq!(adapter.request_body_available_len(false), Ok(0));
            let mut reader = adapter
                .request_body_snapshot_reader(false)
                .await
                .expect("known-empty snapshot reader");
            assert_eq!(reader.len(), 0);
            assert_eq!(reader.next_chunk().await, Ok(None));
        }

        assert!(!ctx.request_body.has_cached_body());
        assert_eq!(
            ctx.request_body
                .text_operation(crate::request_body::BodyTextOperation::IsEmpty, 0)
                .await
                .unwrap()
                .value,
            crate::request_body::BodyTextResult::Bool(true)
        );
    }

    #[tokio::test]
    async fn runtime_disabled_capture_reports_unimplemented() {
        let mut session = test_session_with_body(b"disabled").await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.body_declared = Some(true);
        ctx.request_info.in_request_body_stage = true;
        ctx.request_body_config.enabled = false;

        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert_eq!(adapter.read_request_body(None).await, Err(BodyReadError::Unimplemented));
        assert!(!ctx.request_body.has_cached_body());
    }

    /// Regression pin for Finding 1: `read_request_body` must derive its
    /// stage signal from `ctx.request_info.in_request_body_stage`, not from
    /// whether a response header happens to be bound. This is the exact
    /// shape of the original bug -- `PingoraSessionAdapter::new` (no bound
    /// response header) constructed at the upstream response *body* stage
    /// (`plugin_runtime.rs`'s `run_upstream_response_body_plugins`), which
    /// runs strictly after the request body has been proxied upstream.
    /// Simulates that stage by clearing the marker directly, the same way
    /// `request_filter`'s single exit-point write does in production, then
    /// drives the real trait method (not the bare `request_admission`
    /// predicate) so a future regression that re-derives the stage signal
    /// from `response_header.is_some()` fails this test.
    #[tokio::test]
    async fn read_request_body_refuses_once_past_request_stage() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.body_declared = Some(true);
        ctx.request_info.in_request_body_stage = false;

        {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            let result = adapter.read_request_body(None).await;
            assert!(matches!(result, Err(BodyReadError::NotBuffered)));
        }
        assert!(ctx.edgion_status.contains(&EdgionStatus::BodyRefusedStage));
    }

    /// A truthfully bodyless request needs no capture and remains readable as
    /// the canonical empty body after the request stage.
    #[tokio::test]
    async fn read_request_body_bodyless_past_request_stage_is_empty() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.body_declared = Some(false);
        ctx.request_info.in_request_body_stage = false;

        let body = {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            adapter.read_request_body(None).await.expect("bodyless late read")
        };

        assert!(body.is_empty());
        assert!(!ctx.request_body.has_cached_body());
        assert!(!ctx.edgion_status.contains(&EdgionStatus::BodyRefusedStage));
    }

    #[tokio::test]
    async fn bodyless_request_body_mutations_install_replay_owner() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.body_declared = Some(false);
        ctx.request_info.in_request_body_stage = true;

        {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            adapter.request_body_append(Bytes::from_static(b"abc")).await.unwrap();
            adapter.request_body_prepend(Bytes::from_static(b"[")).await.unwrap();
            adapter
                .request_body_replace_literal(Bytes::from_static(b"b"), Bytes::from_static(b"B"), 64)
                .await
                .unwrap();
            adapter.request_body_remove(0..1).await.unwrap();
            adapter.request_body_clear().await.unwrap();
            adapter.request_body_append(Bytes::from_static(b"x")).await.unwrap();
        }

        assert_eq!(
            ctx.request_body.read_all().await.unwrap().unwrap(),
            Bytes::from_static(b"x")
        );
        assert_eq!(ctx.request_body.modified_len_for_framing().unwrap(), Some(1));
    }

    /// A late consumer may reuse an existing request-scoped body cache, but
    /// may not initiate capture after the request-body service window.
    #[tokio::test]
    async fn read_request_body_reuses_existing_cache_past_request_stage() {
        let mut session = test_session_with_body(b"buffered-before-upstream").await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.body_declared = Some(true);
        ctx.request_info.in_request_body_stage = true;

        let first = {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            adapter.read_request_body(None).await.expect("request-stage capture")
        };
        ctx.request_info.in_request_body_stage = false;
        let second = {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            adapter.read_request_body(None).await.expect("late cached read")
        };

        assert_eq!(first, Bytes::from_static(b"buffered-before-upstream"));
        assert_eq!(first.as_ptr(), second.as_ptr());
        assert!(!ctx.edgion_status.contains(&EdgionStatus::BodyRefusedStage));
    }

    /// Late stages may read an existing cache, but must never acquire mutable
    /// replay ownership even when that cache is still present.
    #[tokio::test]
    async fn overwrite_request_body_refuses_existing_cache_past_request_stage() {
        let mut session = test_session_with_body(b"immutable-after-upstream").await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.body_declared = Some(true);
        ctx.request_info.in_request_body_stage = true;

        {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            adapter.read_request_body(None).await.expect("request-stage capture");
        }
        ctx.request_info.in_request_body_stage = false;
        {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            let result = adapter
                .overwrite_request_body(Bytes::from_static(b"late mutation"))
                .await;
            assert!(matches!(result, Err(BodyReadError::NotBuffered)));
        }

        assert_eq!(
            ctx.request_body.read_all().await.unwrap().unwrap(),
            Bytes::from_static(b"immutable-after-upstream")
        );
        assert!(ctx.edgion_status.contains(&EdgionStatus::BodyRefusedStage));
    }

    /// Regression pin for the backendRef-scoped plugin stage: `request_filter`
    /// no longer clears `in_request_body_stage` at its own tail -- the marker
    /// stays `true` through the backendRef request-plugin call sites in
    /// `pg_upstream_peer.rs` (HTTP) and `routes/grpc/integration.rs` (gRPC),
    /// both of which run after `request_filter` returns but before Pingora has
    /// started pumping the downstream body upstream. Drives the real
    /// `PingoraSessionAdapter::read_request_body` with the exact ctx shape
    /// that holds at those call sites and asserts it resolves to the
    /// real buffering path, not the stage/protocol refusal (`NotBuffered` /
    /// `EdgionStatus::BodyRefusedStage`) that
    /// `read_request_body_refuses_once_past_request_stage` above pins for the
    /// opposite (post-`upstream_request_filter`) direction.
    #[tokio::test]
    async fn read_request_body_admits_during_backend_ref_plugin_stage() {
        let mut session = test_session_with_body(b"backend-body").await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.body_declared = Some(true);
        ctx.request_info.in_request_body_stage = true;

        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        let first = adapter.read_request_body(None).await.expect("body must buffer");
        let second = adapter
            .read_request_body(None)
            .await
            .expect("body cache must be reusable");
        assert_eq!(first, Bytes::from_static(b"backend-body"));
        assert_eq!(first.as_ptr(), second.as_ptr());
    }

    #[tokio::test]
    async fn read_request_body_global_cap_is_hard_failure() {
        let mut session = test_session_with_body(b"12345678").await;
        assert!(session.get_keepalive().is_some());
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.body_declared = Some(true);
        ctx.request_info.in_request_body_stage = true;
        ctx.request_body_config.max_body_size_bytes = 4;

        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert_eq!(
            adapter.read_request_body(None).await,
            Err(BodyReadError::BufferLimitExceeded { limit: 4 })
        );
        assert!(
            session.get_keepalive().is_none(),
            "an unread over-limit HTTP/1 body must make the connection non-reusable"
        );
    }

    #[tokio::test]
    async fn chunked_physical_cap_rejects_without_waiting_for_eof() {
        let (mut client, server) = tokio::io::duplex(4096);
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        client
            .write_all(
                b"POST /body-test HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\n\r\n\
                  5\r\n12345\r\n",
            )
            .await
            .expect("write an over-limit chunk without the terminating chunk");
        session.read_request().await.expect("read chunked request headers");
        assert!(session.get_keepalive().is_some());

        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.body_declared = Some(true);
        ctx.request_info.in_request_body_stage = true;
        ctx.request_body_config.max_body_size_bytes = 4;
        let result = tokio::time::timeout(std::time::Duration::from_millis(100), async {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            adapter.read_request_body(None).await
        })
        .await
        .expect("over-limit detection must not wait for request-body EOF");

        assert_eq!(result, Err(BodyReadError::BufferLimitExceeded { limit: 4 }));
        assert!(
            session.get_keepalive().is_none(),
            "the partially unread HTTP/1 request must disable connection reuse"
        );
    }

    #[tokio::test]
    async fn ensure_request_body_hashes_disk_without_materializing() {
        let body = vec![b'x'; DEFAULT_MEMORY_BUFFER_BYTES + 1];
        let expected: [u8; 32] = sha2::Sha256::digest(&body).into();
        let mut session = test_session_with_body(&body).await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.body_declared = Some(true);
        ctx.request_info.in_request_body_stage = true;

        {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            assert_eq!(
                adapter
                    .ensure_request_body(None)
                    .await
                    .expect("body capture must complete"),
                body.len() as u64
            );
            assert_eq!(
                adapter.request_body_sha256().await.expect("disk-backed body must hash"),
                expected
            );
            assert_eq!(
                adapter
                    .request_body_sha256()
                    .await
                    .expect("the cached disk-backed digest must remain available"),
                expected
            );
        }

        assert_eq!(
            ctx.request_body.cached_body_is_materialized(),
            Some(false),
            "streaming hash must not fill read_all cache"
        );
    }

    #[tokio::test]
    async fn request_body_snapshot_reader_streams_disk_without_materializing() {
        let body = vec![b's'; DEFAULT_MEMORY_BUFFER_BYTES + 17];
        let mut session = test_session_with_body(&body).await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.body_declared = Some(true);
        ctx.request_info.in_request_body_stage = true;

        let mut reader = {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            adapter
                .ensure_request_body(None)
                .await
                .expect("body capture must complete");
            adapter
                .request_body_snapshot_reader(false)
                .await
                .expect("captured snapshot reader")
        };
        assert_eq!(reader.len(), body.len() as u64);

        let mut streamed = bytes::BytesMut::new();
        while let Some(chunk) = reader.next_chunk().await.expect("snapshot chunk") {
            assert!(chunk.len() <= 64 * 1024);
            streamed.extend_from_slice(&chunk);
        }
        assert_eq!(streamed.as_ref(), body.as_slice());
        assert_eq!(ctx.request_body.cached_body_is_materialized(), Some(false));
    }

    #[tokio::test]
    async fn request_body_buffer_gate_preserves_disk_spill_without_materializing() {
        let body = vec![b'x'; DEFAULT_MEMORY_BUFFER_BYTES + 1];
        let mut session = test_session_with_body(&body).await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.body_declared = Some(true);
        ctx.request_info.in_request_body_stage = true;

        {
            let gate = BodyGatedRequestFilter::new(RequestBodyBufferPlugin::create(), BodyRequirement::fail_close());
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            let mut log = PluginLog::new("RequestBodyBuffer");
            assert_eq!(
                gate.run_request(&mut adapter, &mut log).await,
                PluginRunningResult::GoodNext
            );
        }

        assert_eq!(
            ctx.request_body.cached_body_is_materialized(),
            Some(false),
            "the no-op prebuffer plugin must leave a disk-backed body on disk"
        );

        {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            assert_eq!(
                adapter
                    .read_request_body(None)
                    .await
                    .expect("a later byte consumer must still read the captured body"),
                Bytes::from(body)
            );
        }
        assert_eq!(
            ctx.request_body.cached_body_is_materialized(),
            Some(true),
            "the spill should materialize only when a later consumer requests all bytes"
        );
    }

    #[tokio::test]
    async fn read_request_body_answers_expect_continue_before_drain() {
        let (mut client, server) = tokio::io::duplex(4096);
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        client
            .write_all(b"POST /body-test HTTP/1.1\r\nHost: test\r\nContent-Length: 4\r\nExpect: 100-continue\r\n\r\n")
            .await
            .expect("write expect request headers");
        session
            .read_request()
            .await
            .expect("read_request should succeed")
            .then_some(())
            .expect("client must not close before sending request headers");

        let client_task = tokio::spawn(async move {
            let mut response = [0u8; 25];
            client
                .read_exact(&mut response)
                .await
                .expect("read 100 Continue response");
            assert_eq!(&response, b"HTTP/1.1 100 Continue\r\n\r\n");
            client.write_all(b"body").await.expect("write body after continue");
        });

        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.body_declared = Some(true);
        ctx.request_info.in_request_body_stage = true;
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert_eq!(
            adapter.read_request_body(None).await.expect("expect body must buffer"),
            Bytes::from_static(b"body")
        );
        assert!(adapter.inner.req_header().headers.get(http::header::EXPECT).is_none());
        client_task.await.expect("expect client task must finish");
    }

    #[tokio::test]
    async fn expect_continue_declared_over_physical_cap_rejects_without_interim() {
        let (mut client, server) = tokio::io::duplex(4096);
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        client
            .write_all(b"POST /body-test HTTP/1.1\r\nHost: test\r\nContent-Length: 8\r\nExpect: 100-continue\r\n\r\n")
            .await
            .expect("write expect request headers");
        session.read_request().await.expect("read request headers");

        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.body_declared = Some(true);
        ctx.request_info.in_request_body_stage = true;
        ctx.request_body_config.max_body_size_bytes = 4;
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert_eq!(
            adapter.read_request_body(None).await,
            Err(BodyReadError::BufferLimitExceeded { limit: 4 })
        );

        let mut byte = [0u8; 1];
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), client.read(&mut byte))
                .await
                .is_err(),
            "the gateway must not send 100 Continue for a known-over-cap body"
        );
    }

    /// Regression coverage for `read_response_body`'s own wiring (the
    /// override at the top of this file), mirroring
    /// `read_request_body_refuses_once_past_request_stage` above: a bare
    /// `response_admission` unit test (see the `response_admission_*` tests
    /// earlier in this module) cannot catch a bug in how the override reads
    /// `self.response_header` / `self.inner.req_header().method` or whether
    /// it forwards the refusal to `ctx.add_error`. This drives the real
    /// trait method through a `with_response_header`-bound adapter over a
    /// real `pingora_proxy::Session`, status 101, the upgrade-refusal case.
    #[tokio::test]
    async fn read_response_body_refuses_upgrade_status() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        let mut response_header = ResponseHeader::build(101, None).unwrap();

        {
            let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
            let result = adapter.read_response_body(None).await;
            assert!(matches!(result, Err(BodyReadError::NotBuffered)));
        }
        assert!(ctx.edgion_status.contains(&EdgionStatus::BodyRefusedWebsocket));
    }

    /// Stage-guard counterpart: `PingoraSessionAdapter::new` binds no
    /// response header at all, which `response_admission` treats as "not at
    /// the response stage" and refuses. Confirms the override still routes
    /// through `response_admission`'s `None` arm and records the refusal.
    #[tokio::test]
    async fn read_response_body_refuses_without_bound_response_header() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();

        {
            let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
            let result = adapter.read_response_body(None).await;
            assert!(matches!(result, Err(BodyReadError::NotBuffered)));
        }
        assert!(ctx.edgion_status.contains(&EdgionStatus::BodyRefusedStage));
    }

    /// PassThrough path: a bodyless status (204) that the UPSTREAM itself sent —
    /// both the bound header and `ctx.request_info.status` agree — must return
    /// an empty, non-error body rather than falling into the
    /// `Buffer`/`Unimplemented` arm.
    #[tokio::test]
    async fn read_response_body_bodyless_status_passes_through_empty() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.status = Some(204);
        let mut response_header = ResponseHeader::build(204, None).unwrap();

        let result = {
            let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
            adapter.read_response_body(None).await
        };
        assert_eq!(result.expect("204 must pass through"), Bytes::new());
    }

    /// Regression pin for the status half of the same defect shape as Finding 3:
    /// the bound response header is response-stage mutable
    /// (`set_response_status`, reachable from DSL's `resp.set_status` and from
    /// `response_rewrite`), so a `resp.set_status(204)` ordered ahead of a body
    /// consumer in the SAME `upstreamResponseFilterPlugins` array must not
    /// fabricate the bodyless short-circuit — the upstream sent 200 and its body
    /// is real and downstream-bound. Must stay on the `Buffer` arm.
    #[tokio::test]
    async fn read_response_body_ignores_plugin_rewritten_bodyless_status() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        // The upstream's own status, stamped in `upstream_response_filter`
        // before any response-stage plugin ran.
        ctx.request_info.status = Some(200);
        let mut response_header = ResponseHeader::build(200, None).unwrap();

        let result = {
            let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
            adapter.set_response_status(204).expect("204 is a valid status");
            assert_eq!(adapter.response_header.as_ref().unwrap().status.as_u16(), 204);
            adapter.read_response_body(None).await
        };
        assert!(
            matches!(result, Err(BodyReadError::Unimplemented)),
            "a plugin-rewritten bodyless status must not downgrade the read to PassThrough"
        );
    }

    /// The mirror direction: the upstream really did send 204, a plugin rewrote
    /// it to 200. There is no body either way, but the disagreement must still
    /// steer to the loud `Buffer` rather than the silent `PassThrough` — the
    /// same conservative choice the HEAD pair makes.
    #[tokio::test]
    async fn read_response_body_ignores_bodyless_snapshot_when_status_rewritten_away() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.status = Some(204);
        let mut response_header = ResponseHeader::build(204, None).unwrap();

        let result = {
            let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
            adapter.set_response_status(200).expect("200 is a valid status");
            adapter.read_response_body(None).await
        };
        assert!(
            matches!(result, Err(BodyReadError::Unimplemented)),
            "a rewritten-away-from-bodyless response must not keep the short-circuit"
        );
    }

    /// Buffer path: an ordinary 200 on a non-HEAD, non-gRPC request must
    /// reach the `Buffer` arm and fail with `Unimplemented` (real chunk
    /// buffering is not wired up yet). `method_is_head` is left at its
    /// `false` default, so this also pins the `is_head` routing: an inverted
    /// read would misroute this case to `PassThrough` instead.
    #[tokio::test]
    async fn read_response_body_ordinary_200_is_unimplemented() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        let mut response_header = ResponseHeader::build(200, None).unwrap();

        let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
        let result = adapter.read_response_body(None).await;
        assert!(matches!(result, Err(BodyReadError::Unimplemented)));
    }

    /// A genuine, unrewritten HEAD request still short-circuits to
    /// `PassThrough`: both the snapshot and the live header say HEAD, so
    /// closing the plugin-mutable holes must not break the RFC 9110 9.3.2 case
    /// itself.
    #[tokio::test]
    async fn read_response_body_genuine_head_passes_through_empty() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.method_is_head = true;
        session.req_header_mut().set_method(http::Method::HEAD);
        let mut response_header = ResponseHeader::build(200, None).unwrap();

        let result = {
            let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
            adapter.read_response_body(None).await
        };
        assert_eq!(result.expect("genuine HEAD must pass through"), Bytes::new());
    }

    /// Regression pin for Finding 3, rewrite-TO-HEAD direction: a plugin
    /// calling `set_upstream_method("HEAD")` must not fabricate the bodyless
    /// short-circuit. Reachable at the response stage too — DSL's
    /// `req.set_method` is not stage-gated — where the upstream body has
    /// already arrived. The read must stay on the `Buffer` arm
    /// (`Unimplemented` today) rather than silently resolving to
    /// `PassThrough` -> `Ok(empty)`, which a body-inspecting plugin reads as
    /// "the response genuinely had no body".
    #[tokio::test]
    async fn read_response_body_ignores_plugin_rewritten_head_method() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        // The snapshot ran on the real downstream GET.
        ctx.request_info.method_is_head = false;
        let mut response_header = ResponseHeader::build(200, None).unwrap();

        let result = {
            let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
            adapter.set_upstream_method("HEAD").expect("HEAD parses");
            assert_eq!(adapter.inner.req_header().method, http::Method::HEAD);
            adapter.read_response_body(None).await
        };
        assert!(
            matches!(result, Err(BodyReadError::Unimplemented)),
            "a plugin-rewritten HEAD must not downgrade the read to PassThrough"
        );
    }

    /// Regression pin for Finding 3, rewrite-AWAY-FROM-HEAD direction (the
    /// mirror hole a snapshot-only read would open): the client sent HEAD, a
    /// request-stage plugin rewrote the method to a body-carrying one, so the
    /// upstream returns a real body AND pingora writes it downstream — its
    /// `init_body_writer` keys on the same live header the rewrite mutated.
    /// Trusting the snapshot alone here would hand a body consumer
    /// `Ok(empty)` for a response that demonstrably has a body.
    #[tokio::test]
    async fn read_response_body_ignores_head_snapshot_when_method_rewritten_away() {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        // The client really did send HEAD.
        ctx.request_info.method_is_head = true;
        let mut response_header = ResponseHeader::build(200, None).unwrap();

        let result = {
            let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
            // A request-stage plugin (e.g. proxy_rewrite `method: GET`) rewrites
            // it, so the upstream response carries a body after all.
            adapter.set_upstream_method("GET").expect("GET parses");
            adapter.read_response_body(None).await
        };
        assert!(
            matches!(result, Err(BodyReadError::Unimplemented)),
            "a rewritten-away-from-HEAD request must not keep the bodyless short-circuit"
        );
    }

    /// Like `test_session`, but the caller supplies the whole request line +
    /// headers. `apply_response_local_reply` keys on the request method and
    /// (previously) on request headers, neither of which the fixed
    /// `GET /dye-test` request can vary.
    async fn test_session_raw(request: &str) -> Session {
        let (mut client, server) = tokio::io::duplex(4096);
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        client
            .write_all(request.as_bytes())
            .await
            .expect("write request into duplex pair");
        drop(client);
        session
            .read_request()
            .await
            .expect("read_request should succeed")
            .then_some(())
            .expect("client must not have closed before sending the request");
        session
    }

    async fn apply_local_reply(request: &str, upstream_status: u16) -> (bool, EdgionHttpContext) {
        let mut session = test_session_raw(request).await;
        let mut ctx = EdgionHttpContext::default();
        let mut response_header = ResponseHeader::build(upstream_status, None).unwrap();
        let applied = {
            let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
            adapter.apply_response_local_reply(403, Some("denied".to_string()))
        };
        (applied, ctx)
    }

    /// The defect this refusal predicate was rewritten for: an `Upgrade` header
    /// carrying a token nothing recognizes is not a protocol switch. The request
    /// proxies as ordinary HTTP (only `websocket` is detected in
    /// `pg_request_filter::detect_request_protocol`) and the upstream answers
    /// 200, so a response-stage plugin's denial MUST be able to replace it.
    /// Keying the refusal on the request header let any client append
    /// `Upgrade: foo` and have the denial silently dropped — the response-stage
    /// runner only logs when this returns false, and nothing there consults
    /// `ctx.plugin_running_result`.
    #[tokio::test]
    async fn local_reply_replaces_response_for_unrecognized_upgrade_token() {
        let (applied, ctx) = apply_local_reply("GET /x HTTP/1.1\r\nHost: test\r\nUpgrade: foo\r\n\r\n", 200).await;
        assert!(applied, "an unrecognized Upgrade token must not block the denial");
        assert!(ctx.local_reply_header.is_some());
        assert_eq!(ctx.local_reply_header.as_deref().unwrap().status.as_u16(), 403);
    }

    /// The genuine switch stays refused: the upstream answered 101, so the
    /// exchange is no longer a replaceable HTTP response.
    #[tokio::test]
    async fn local_reply_refused_for_real_protocol_switch() {
        let (applied, ctx) = apply_local_reply(
            "GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
            101,
        )
        .await;
        assert!(!applied, "a real 101 must stay unreplaceable");
        assert!(ctx.local_reply_header.is_none());
    }

    /// Interim informational responses (100/103) are not the final response
    /// there is nothing to replace yet.
    #[tokio::test]
    async fn local_reply_refused_for_interim_informational() {
        let (applied, _ctx) = apply_local_reply("GET /x HTTP/1.1\r\nHost: test\r\n\r\n", 103).await;
        assert!(!applied, "an interim 1xx must stay unreplaceable");
    }

    /// CONNECT is refused only once the tunnel actually exists.
    #[tokio::test]
    async fn local_reply_refused_for_established_connect_tunnel() {
        let (applied, _ctx) =
            apply_local_reply("CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n", 200).await;
        assert!(!applied, "an established CONNECT tunnel must stay unreplaceable");
    }

    /// ...and an upstream-rejected CONNECT never became a tunnel, so it is an
    /// ordinary replaceable response.
    #[tokio::test]
    async fn local_reply_replaces_rejected_connect() {
        let (applied, ctx) =
            apply_local_reply("CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n", 403).await;
        assert!(applied, "a CONNECT the upstream refused carries no tunnel");
        assert!(ctx.local_reply_header.is_some());
    }

    async fn terminate_for(request: &str, upstream_status: u16) -> (bool, EdgionHttpContext) {
        use crate::plugins::runtime::plugin_runtime::check_response_stage_terminate;

        let mut session = test_session_raw(request).await;
        let mut ctx = EdgionHttpContext::default();
        // As stamped by `upstream_response_filter` before the plugin chains run.
        ctx.request_info.status = Some(upstream_status);
        let mut response_header = ResponseHeader::build(upstream_status, None).unwrap();
        let result = PluginRunningResult::ErrResponse {
            status: 403,
            body: Some("denied".to_string()),
        };
        let terminate = {
            let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
            let mut log = PluginLog::with_alias("dsl", None);
            check_response_stage_terminate(&result, &mut log, &mut adapter)
        };
        (terminate, ctx)
    }

    /// The termination half of the same defect: a refused local reply must ARM
    /// the abort, not just log. Before this, the runner's `set_terminate` wrote
    /// `ctx.plugin_running_result`, whose readers are all request-stage, so the
    /// response-stage entry point forwarded the upstream response and the denial
    /// vanished. Driven on a real `101` — the case that stays unreplaceable even
    /// after the refusal predicate was narrowed to response-side facts.
    #[tokio::test]
    async fn refused_local_reply_arms_the_response_stage_abort() {
        let (terminate, ctx) = terminate_for(
            "GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
            101,
        )
        .await;

        assert!(terminate);
        assert!(ctx.local_reply_header.is_none(), "a 101 carries no local reply");
        assert_eq!(
            ctx.response_denial,
            Some(ResponseStageDenial::Http(403)),
            "the denial must survive as an abort the entry point can act on"
        );
        assert!(ctx.edgion_status.contains(&EdgionStatus::PluginResponseDenyAborted));
        assert_eq!(
            ctx.request_info.status,
            Some(403),
            "the access log must show the denial, not the upstream response the client never got"
        );
    }

    /// The ordinary path is unchanged: a replaceable response stashes the reply
    /// and never arms the abort (which would double-handle the same denial).
    #[tokio::test]
    async fn accepted_local_reply_does_not_arm_the_abort() {
        let (terminate, ctx) = terminate_for("GET /x HTTP/1.1\r\nHost: test\r\n\r\n", 200).await;

        assert!(terminate);
        assert!(ctx.local_reply_header.is_some());
        assert_eq!(ctx.response_denial, None);
        assert!(!ctx.edgion_status.contains(&EdgionStatus::PluginResponseDenyAborted));
    }

    /// `ErrGrpcError` has no response-stage writer at all — the sync runner
    /// cannot await `send_grpc_error_direct` — so before this it was a pure log
    /// line and the upstream response was forwarded. The gRPC status must be
    /// carried verbatim: deriving it from an HTTP status later would flatten
    /// `RESOURCE_EXHAUSTED` (8) into `INTERNAL` (13).
    #[tokio::test]
    async fn response_stage_grpc_error_arms_the_abort_with_the_exact_status() {
        use crate::plugins::runtime::plugin_runtime::check_response_stage_terminate;

        let mut session = test_session_raw("POST /svc/M HTTP/1.1\r\nHost: test\r\n\r\n").await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.is_grpc_request = true;
        ctx.request_info.status = Some(200);
        let mut response_header = ResponseHeader::build(200, None).unwrap();
        let result = PluginRunningResult::ErrGrpcError {
            grpc_status: 8,
            message: Some("quota exceeded".to_string()),
        };

        let terminate = {
            let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
            let mut log = PluginLog::with_alias("dsl", None);
            check_response_stage_terminate(&result, &mut log, &mut adapter)
        };

        assert!(terminate);
        assert_eq!(
            ctx.response_denial,
            Some(ResponseStageDenial::Grpc {
                status: 8,
                message: Some("quota exceeded".to_string()),
            })
        );
        assert_eq!(ctx.request_info.grpc_status, Some(8));
        assert_eq!(
            ctx.request_info.status,
            Some(200),
            "a gRPC error is HTTP 200 on the wire"
        );
        assert!(ctx.edgion_status.contains(&EdgionStatus::PluginResponseDenyAborted));
    }

    /// The broadest leak of the three: a naked `ErrTerminateRequest` needs NO
    /// precondition to be dropped — the arm simply returned `true`, the runner
    /// called `set_terminate`, and the upstream response was forwarded. Now it
    /// mirrors the request stage's fallback contract
    /// (`error_response::terminate_with_fallback`).
    #[tokio::test]
    async fn response_stage_naked_terminate_arms_the_abort() {
        use crate::plugins::runtime::plugin_runtime::check_response_stage_terminate;

        let mut session = test_session_raw("GET /x HTTP/1.1\r\nHost: test\r\n\r\n").await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.status = Some(200);
        let mut response_header = ResponseHeader::build(200, None).unwrap();
        let result = PluginRunningResult::ErrTerminateRequest;

        let terminate = {
            let mut adapter = PingoraSessionAdapter::with_response_header(&mut session, &mut ctx, &mut response_header);
            let mut log = PluginLog::with_alias("dsl", None);
            check_response_stage_terminate(&result, &mut log, &mut adapter)
        };

        assert!(terminate);
        assert_eq!(ctx.response_denial, Some(ResponseStageDenial::Http(502)));
        assert!(ctx
            .edgion_status
            .contains(&EdgionStatus::PluginTerminateWithoutResponse));
        assert!(ctx.edgion_status.contains(&EdgionStatus::PluginResponseDenyAborted));
        assert_eq!(ctx.request_info.status, Some(502));
    }
}
