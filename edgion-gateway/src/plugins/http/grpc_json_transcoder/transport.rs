use super::budget::Reservation;
use crate::EdgionHttpContext;
use edgion_resources::grpc_transcoding::{CompiledBinding, CompiledGrpcTranscoder};
use parking_lot::Mutex;
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::{
    ResponseHeadBoundary, ResponseHeadBoundaryAction, ResponseHeadCommitPlan, ResponseHeadHoldLimits,
    ResponseHeadReplacement, ResponseHeadSource,
};
use std::{
    hash::{Hash, Hasher},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

/// Request-owned upstream RPC intent. This never changes downstream protocol flags.
pub struct GrpcTranscodingPlan {
    pub(crate) compiled: Arc<CompiledGrpcTranscoder>,
    pub(crate) binding: Arc<CompiledBinding>,
    reservation: Mutex<Option<Arc<Reservation>>>,
    pub(crate) deadline: Instant,
    pub(crate) request_len: usize,
    pub(crate) generation: Option<u64>,
    pub(crate) conflict: AtomicBool,
    pub(crate) invalid_response: AtomicBool,
    pub(crate) template: Mutex<Option<ResponseHeader>>,
    pub(crate) ready: Mutex<Option<ResponseHeadReplacement>>,
    pub(crate) terminal: Mutex<Option<bytes::Bytes>>,
}
impl GrpcTranscodingPlan {
    pub(super) fn new(
        compiled: Arc<CompiledGrpcTranscoder>,
        binding: Arc<CompiledBinding>,
        reservation: Arc<Reservation>,
        request_len: usize,
    ) -> Self {
        Self {
            compiled,
            binding,
            reservation: Mutex::new(Some(reservation)),
            request_len,
            deadline: Instant::now() + Duration::from_secs(30),
            generation: None,
            conflict: AtomicBool::new(false),
            invalid_response: AtomicBool::new(false),
            template: Mutex::new(None),
            ready: Mutex::new(None),
            terminal: Mutex::new(None),
        }
    }
    pub(crate) fn output(&self, bytes: Vec<u8>) -> Result<bytes::Bytes, ()> {
        self.reservation.lock().as_ref().ok_or(())?.output(bytes)
    }
    pub(crate) fn release_reservation(&self) {
        self.reservation.lock().take();
    }
    pub(crate) fn streaming(&self) -> bool {
        self.binding.method.is_server_streaming()
    }
    pub(crate) fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }
    pub(crate) fn reject_mutation(&self) {
        self.conflict.store(true, Ordering::Release);
    }
    pub(crate) fn head_plan(
        &self,
        source: ResponseHeadSource,
        response: &ResponseHeader,
    ) -> pingora_core::Result<ResponseHeadCommitPlan> {
        if self.expired()
            || self.invalid_response.load(Ordering::Acquire)
            || source != ResponseHeadSource::Origin
            || response.status.as_u16() != 200
            || !super::response::json_headers_valid(response, self.streaming(), self.compiled.format)
        {
            return Err(failure());
        }
        if self.streaming() {
            return Ok(ResponseHeadCommitPlan::Immediate);
        }
        let mut size = 0usize;
        let mut fields = 0;
        for (name, value) in &response.headers {
            size = size.saturating_add(name.as_str().len() + value.as_bytes().len() + 4);
            fields += 1;
        }
        if size > 8192 || fields > 64 {
            return Err(failure());
        }
        *self.template.lock() = Some(response.clone());
        Ok(ResponseHeadCommitPlan::hold(ResponseHeadHoldLimits::new(
            self.compiled.limits.response + 5,
            self.compiled.limits.json,
            8,
            4096,
            8192,
            4096,
            self.deadline.saturating_duration_since(Instant::now()),
        )))
    }
    pub(crate) fn boundary(&self, boundary: ResponseHeadBoundary) -> ResponseHeadBoundaryAction {
        if boundary == ResponseHeadBoundary::CleanTerminalWithoutDecision && !self.expired() {
            if let Some(ready) = self.ready.lock().take() {
                return ResponseHeadBoundaryAction::Replace(ready);
            }
        }
        self.ready.lock().take();
        ResponseHeadBoundaryAction::Fail(failure())
    }
    pub(crate) fn clear_output(&self) {
        self.ready.lock().take();
        self.terminal.lock().take();
        self.template.lock().take();
        self.release_reservation();
    }
}
pub(crate) fn failure() -> Box<pingora_core::Error> {
    let mut e = pingora_core::Error::explain(pingora_core::ErrorType::HTTPStatus(502), "gRPC transcoding failed");
    e.set_retry(false);
    e
}
pub(crate) fn validate_request(request: &RequestHeader, ctx: &EdgionHttpContext) -> pingora_core::Result<()> {
    let Some(plan) = &ctx.grpc_transcoding else {
        return Ok(());
    };
    if plan.conflict.load(Ordering::Acquire)
        || ctx.has_request_body_stream_handlers()
        || plan.expired()
        || request.method != http::Method::POST
        || request.uri.path_and_query().map(|p| p.as_str()) != Some(plan.binding.rpc_path.as_str())
        || ctx.request_body.generation() != plan.generation
    {
        return Err(failure());
    }
    Ok(())
}
pub(crate) fn require_h2(peer: &mut pingora_core::upstreams::peer::HttpPeer) {
    peer.options.set_http_version(2, 2);
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    ("edgion.mandatory-h2.v1", peer.group_key).hash(&mut hash);
    peer.group_key = hash.finish();
}
pub(crate) fn prepare_attempt(request: &mut RequestHeader, ctx: &EdgionHttpContext) -> pingora_core::Result<()> {
    validate_request(request, ctx)?;
    let Some(plan) = &ctx.grpc_transcoding else {
        return Ok(());
    };
    if request.version != http::Version::HTTP_2 {
        return Err(failure());
    }
    for name in ["content-encoding", "transfer-encoding", "grpc-encoding", "grpc-timeout"] {
        request.remove_header(name);
    }
    request.insert_header("content-type", "application/grpc+proto")?;
    request.insert_header("te", "trailers")?;
    request.insert_header("grpc-accept-encoding", "identity")?;
    request.insert_header("content-length", plan.request_len.to_string())?;
    let remaining = plan.deadline.saturating_duration_since(Instant::now());
    request.insert_header("grpc-timeout", crate::routes::grpc::format_grpc_timeout(remaining))?;
    Ok(())
}
pub(crate) fn protected(name: &str) -> bool {
    [
        "content-type",
        "content-length",
        "transfer-encoding",
        "content-encoding",
        "grpc-encoding",
        "grpc-accept-encoding",
        "grpc-timeout",
        "te",
        "connection",
    ]
    .iter()
    .any(|field| name.eq_ignore_ascii_case(field))
}
pub(crate) fn validate_attempt(request: &RequestHeader, ctx: &EdgionHttpContext) -> pingora_core::Result<()> {
    validate_request(request, ctx)?;
    let Some(plan) = &ctx.grpc_transcoding else {
        return Ok(());
    };
    if request.version != http::Version::HTTP_2
        || request
            .headers
            .get("content-type")
            .is_none_or(|v| v != "application/grpc+proto")
        || request.headers.get("te").is_none_or(|v| v != "trailers")
        || request
            .headers
            .get("grpc-accept-encoding")
            .is_none_or(|v| v != "identity")
        || request
            .headers
            .get("content-length")
            .is_none_or(|v| v.to_str().ok().and_then(|v| v.parse::<usize>().ok()) != Some(plan.request_len))
        || ["content-encoding", "grpc-encoding", "transfer-encoding", "connection"]
            .iter()
            .any(|n| request.headers.contains_key(*n))
    {
        return Err(failure());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/grpc_json_transcoder/transport_test.rs"]
mod tests;
