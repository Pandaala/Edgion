//! Unary-owned response-head policy and fresh bounded retained metadata.
use super::response::matches_media_type;
use super::{budget::Reservation, limits::UnaryLimits};
use crate::ai::{render_openai_error, AiProxyError};
use crate::routes::http::proxy_http::content_type::response_is_identity_encoded;
use crate::EdgionHttpContext;
use http::{HeaderMap, HeaderName, HeaderValue};
use pingora_core::{Error, ErrorType, Result};
use pingora_http::ResponseHeader;
use pingora_proxy::{ResponseHeadCommitPlan, ResponseHeadHoldLimits, ResponseHeadSource};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
use std::time::Instant;
const MAX_FIELDS: usize = 64;
const ERROR_METADATA: usize = 8192;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeadlineSource {
    Hold,
    Request,
    Grpc,
}
#[derive(Clone, Copy)]
struct EffectiveDeadline {
    at: Instant,
    source: DeadlineSource,
}
fn effective_deadline(
    now: Instant,
    hold: std::time::Duration,
    request: Option<Instant>,
    grpc: Option<Instant>,
) -> Option<EffectiveDeadline> {
    let mut selected = EffectiveDeadline {
        at: now.checked_add(hold)?,
        source: DeadlineSource::Hold,
    };
    // Request wins equal deadlines, then gRPC, then the converter ceiling.
    if let Some(at) = grpc.filter(|at| *at <= selected.at) {
        selected = EffectiveDeadline {
            at,
            source: DeadlineSource::Grpc,
        };
    }
    if let Some(at) = request.filter(|at| *at <= selected.at) {
        selected = EffectiveDeadline {
            at,
            source: DeadlineSource::Request,
        };
    }
    Some(selected)
}
pub(crate) struct UnaryHead {
    pub(crate) limits: UnaryLimits,
    pub(crate) reservation: Arc<Reservation>,
    raw_success: AtomicBool,
    invalid: AtomicBool,
    deadline: OnceLock<EffectiveDeadline>,
    timeout_generated: AtomicBool,
    template: OnceLock<ResponseHeader>,
}
impl UnaryHead {
    pub(crate) fn new(limits: UnaryLimits, reservation: Arc<Reservation>) -> Self {
        Self {
            limits,
            reservation,
            raw_success: AtomicBool::new(false),
            invalid: AtomicBool::new(false),
            deadline: OnceLock::new(),
            timeout_generated: AtomicBool::new(false),
            template: OnceLock::new(),
        }
    }
    pub(crate) fn observe_header(&self, raw_success: bool, valid: bool) {
        self.raw_success.store(raw_success, Ordering::Release);
        self.invalid.store(raw_success && !valid, Ordering::Release);
    }
    pub(crate) fn raw_success(&self) -> bool {
        self.raw_success.load(Ordering::Acquire)
    }
    pub(crate) fn expired(&self) -> bool {
        self.deadline.get().is_some_and(|d| Instant::now() >= d.at)
    }
    pub(crate) fn timeout_failure(&self) -> Box<Error> {
        self.timeout_generated.store(true, Ordering::Release);
        failure(true)
    }
    pub(crate) fn plan(
        &self,
        source: ResponseHeadSource,
        response: &ResponseHeader,
        ctx: &EdgionHttpContext,
    ) -> Result<ResponseHeadCommitPlan> {
        if !self.raw_success() {
            return Ok(ResponseHeadCommitPlan::Immediate);
        }
        if ctx
            .ai_state()
            .and_then(|s| s.response_head_hold_claim)
            .is_some_and(|claim| claim.active && claim.owner != crate::ctx::ResponseHeadHoldOwner::AnthropicUnary)
            || self.invalid.load(Ordering::Acquire)
            || source != ResponseHeadSource::Origin
            || response.status.as_u16() != 200
            || ctx
                .ai_state()
                .and_then(|state| state.final_response_eligibility)
                .is_none_or(|e| !e.method_allows_body || !e.status_allows_body)
            || !matches_media_type(response, "application", "json")
            || !response_is_identity_encoded(&response.headers)
        {
            return Err(failure(false));
        }
        let now = Instant::now();
        let selected = effective_deadline(
            now,
            self.limits.timeout,
            ctx.request_timeout().and_then(|t| ctx.start_time.checked_add(t)),
            ctx.request_info.grpc_client_deadline(),
        )
        .ok_or_else(|| self.timeout_failure())?;
        let deadline = self.deadline.get_or_init(|| selected);
        let timeout = deadline.at.saturating_duration_since(now);
        if timeout.is_zero() {
            return Err(self.timeout_failure());
        }
        let template = error_template(response, ctx)?;
        let _ = self.template.set(template);
        Ok(ResponseHeadCommitPlan::hold(ResponseHeadHoldLimits::new(
            self.limits.input,
            self.limits.output,
            8,
            self.limits.events,
            self.limits.metadata,
            self.limits.events as u64,
            timeout,
        )))
    }
    pub(crate) fn error_reply(&self, status: u16, ctx: &EdgionHttpContext) -> Result<(ResponseHeader, bytes::Bytes)> {
        let kind = if status == 504 {
            AiProxyError::UpstreamTimeout
        } else {
            AiProxyError::UpstreamResponseInvalid
        };
        let (_, body) = render_openai_error(kind);
        let mut header = if let Some(template) = self.template.get() {
            template.clone()
        } else {
            minimal_error(ctx)?
        };
        header.set_status(status)?;
        header.insert_header("content-length", body.len().to_string())?;
        Ok((header, bytes::Bytes::from(body)))
    }
}
pub(crate) fn record_timeout_diagnostic(ctx: &mut EdgionHttpContext, error: &Error) {
    if error.etype() != &ErrorType::HTTPStatus(504) {
        return;
    }
    let diagnostic = ctx.ai_state().and_then(|s| s.unary_owner.as_ref()).and_then(|owner| {
        if !owner.timeout_generated.load(Ordering::Acquire) {
            return None;
        }
        match owner.deadline.get()?.source {
            DeadlineSource::Request => Some(edgion_resources::EdgionErrorCode::RequestTimeoutExceeded),
            DeadlineSource::Grpc => Some(edgion_resources::EdgionErrorCode::GrpcClientDeadlineExceeded),
            DeadlineSource::Hold => None,
        }
    });
    if let Some(diagnostic) = diagnostic {
        ctx.record_error(diagnostic);
    }
}
pub(crate) fn failure(timeout: bool) -> Box<Error> {
    let mut error = Error::explain(
        ErrorType::HTTPStatus(if timeout { 504 } else { 502 }),
        if timeout {
            "AI unary response deadline expired"
        } else {
            "AI unary response could not be processed"
        },
    );
    error.set_retry(false);
    error
}
fn compact_map(original: &HeaderMap, limit: usize, base: usize) -> Result<HeaderMap> {
    let mut metadata = base;
    let mut fields = 0usize;
    // Validate the complete logical projection before copying any names/values.
    for (name, value) in original {
        fields = fields.checked_add(1).ok_or_else(|| failure(false))?;
        metadata = metadata
            .checked_add(32)
            .and_then(|n| n.checked_add(name.as_str().len()))
            .and_then(|n| n.checked_add(value.as_bytes().len()))
            .ok_or_else(|| failure(false))?;
        if fields > MAX_FIELDS || metadata > limit {
            return Err(failure(false));
        }
    }
    let mut result = HeaderMap::new();
    for (name, value) in original {
        let name = HeaderName::from_bytes(name.as_str().as_bytes()).map_err(|_| failure(false))?;
        let value = HeaderValue::from_bytes(value.as_bytes()).map_err(|_| failure(false))?;
        result.append(name, value);
    }
    Ok(result)
}
pub(crate) fn compact_final_header(response: &mut ResponseHeader, ctx: &EdgionHttpContext) -> Result<()> {
    let Some(owner) = ctx
        .ai_state()
        .and_then(|s| s.unary_owner.as_ref())
        .filter(|o| o.raw_success())
    else {
        return Ok(());
    };
    let headers = compact_map(&response.headers, owner.limits.metadata, 64)?;
    let fresh = ResponseHeader::build_no_case(response.status, Some(0))?;
    let mut parts: http::response::Parts = fresh.into();
    parts.headers = headers;
    *response = ResponseHeader::from(parts);
    Ok(())
}
pub(crate) fn compact_trailers(trailers: &mut HeaderMap, ctx: &EdgionHttpContext) -> Result<()> {
    let Some(owner) = ctx
        .ai_state()
        .and_then(|s| s.unary_owner.as_ref())
        .filter(|o| o.raw_success())
    else {
        return Ok(());
    };
    let fresh = compact_map(trailers, owner.limits.metadata, 32)?;
    *trailers = fresh;
    Ok(())
}
fn minimal_error(ctx: &EdgionHttpContext) -> Result<ResponseHeader> {
    let mut head = ResponseHeader::build_no_case(502, Some(0))?;
    head.insert_header("content-type", "application/json")?;
    head.insert_header("cache-control", "no-store")?;
    ctx.request_info.trace_id.reconcile_response(&mut head);
    let map = compact_map(&head.headers, ERROR_METADATA, 64)?;
    let mut parts: http::response::Parts = head.into();
    parts.headers = map;
    Ok(ResponseHeader::from(parts))
}
fn error_template(original: &ResponseHeader, ctx: &EdgionHttpContext) -> Result<ResponseHeader> {
    let mut head = minimal_error(ctx)?;
    for name in ["server", "strict-transport-security"] {
        for value in original.headers.get_all(name) {
            if value.as_bytes().len() > 512 {
                return minimal_error(ctx);
            }
            let value = HeaderValue::from_bytes(value.as_bytes()).map_err(|_| failure(false))?;
            head.append_header(name, value)?;
        }
    }
    match compact_map(&head.headers, ERROR_METADATA, 64) {
        Ok(map) => {
            let mut parts: http::response::Parts = head.into();
            parts.headers = map;
            Ok(ResponseHeader::from(parts))
        }
        Err(_) => minimal_error(ctx),
    }
}
#[cfg(test)]
#[path = "../../../../tests/unit/ai/conversion/chat_to_anthropic/head_test.rs"]
mod tests;
