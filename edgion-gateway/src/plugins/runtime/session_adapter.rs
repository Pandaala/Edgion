use async_trait::async_trait;
use bytes::Bytes;
use http::Uri;
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::Session;
use std::collections::HashMap;
use std::sync::Arc;

use super::conditions::{EvaluationResult, PluginConditionsExt};
use super::dye_plan::{CompiledDyePlan, CompiledDyeRule};
use super::log::{EdgionPluginsLog, EdgionPluginsLogToken, PluginLog};
use super::traits::session::RouteProto;
use super::traits::{DyeOutcome, PluginSession, PluginSessionError, PluginSessionResult};
use crate::link_sys::template::evaluate_template;
use crate::runtime::server::{http_to_grpc_status, send_grpc_trailers_only_frame};
use crate::{EdgionHttpContext, MirrorState, PendingHeaderOp, SetJumpOutcome, UpstreamJumpTarget};
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

    /// Decide + stash a response-phase local reply. Returns `true` when a reply
    /// was stashed (HTTP route, not an upgrade/informational response).
    /// Refuses: gRPC route, Upgrade/CONNECT request, 1xx/101 upstream response,
    /// or no bound response header. The actual downstream write happens later in
    /// `response_filter`.
    pub fn apply_response_local_reply(&mut self, status: u16, body: Option<String>) -> bool {
        if self.response_header.is_none() {
            return false;
        }
        let (is_head, refuse) = {
            let req = self.inner.req_header();
            let upgrade = req.method == http::Method::CONNECT || req.headers.get(http::header::UPGRADE).is_some();
            let informational = self
                .response_header
                .as_deref()
                .map(|h| h.status.is_informational())
                .unwrap_or(false);
            (req.method == http::Method::HEAD, upgrade || informational)
        };
        if refuse {
            return false;
        }
        build_response_local_reply(self.ctx, status, body, is_head)
    }

    /// Record that a response-phase `ErrResponse` could not be turned into a
    /// local reply (gRPC route). Surfaces the otherwise-silent termination in
    /// the access log.
    pub fn note_response_local_reply_unsupported(&mut self) {
        self.ctx.add_error(EdgionStatus::PluginResponseReplaceUnsupportedGrpc);
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

    fn ctx(&self) -> &EdgionHttpContext {
        self.ctx
    }

    fn add_error(&mut self, status: edgion_resources::EdgionStatus) {
        self.ctx.add_error(status);
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
}

#[cfg(test)]
mod record_outcome_tests {
    use super::*;
    use edgion_resources::resources::edgion_plugins::{DyeOutcome, DyeSuite};
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
}
