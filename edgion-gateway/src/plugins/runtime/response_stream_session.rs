//! Request-context adapters for cancellation-safe response callbacks.

use crate::ai::protocols::openai::canonical_sse::CanonicalRecordBatch;
use crate::ai::protocols::openai::terminal::AiClientStreamDialect;
use crate::ctx::ResponseStreamKind;
use crate::link_sys::template::TrustedTemplateVariables;
use crate::plugins::runtime::traits::response_stream_session::{ResponseStreamSession, ResponseTrailerSession};
use crate::plugins::runtime::traits::session::RouteProto;
use crate::plugins::runtime::traits::{StreamingResponseEvent, StreamingResponseProcessor};
use crate::response::terminal::{
    FinalResponseEligibility, ResponseProcessorSlot, ResponseTerminalCause, TerminalEmission,
};
use crate::EdgionHttpContext;
use edgion_resources::EdgionErrorCode;

pub(crate) struct ResponseStreamAdapter<'a> {
    ctx: &'a mut EdgionHttpContext,
    slot: ResponseProcessorSlot,
}

impl<'a> ResponseStreamAdapter<'a> {
    pub(crate) fn new(ctx: &'a mut EdgionHttpContext) -> Self {
        Self {
            ctx,
            slot: ResponseProcessorSlot::Ai,
        }
    }
    pub(crate) fn set_response_processor_slot(&mut self, slot: ResponseProcessorSlot) {
        self.slot = slot;
    }
    pub(crate) fn response_terminal_winner(&self) -> Option<ResponseProcessorSlot> {
        self.ctx.response_terminal_state.winner()
    }
}

impl TrustedTemplateVariables for ResponseStreamAdapter<'_> {
    fn trusted_ctx_var(&self, key: &str) -> Option<String> {
        self.ctx.get_ctx_var(key).map(str::to_owned)
    }
    fn trusted_remote_ip(&self) -> &str {
        &self.ctx.request_info.remote_ip
    }
    fn trusted_matched_route_meta(&self, proto: RouteProto, selector: &str) -> Option<String> {
        super::session_adapter::matched_route_meta(self.ctx, proto, selector)
    }
}

impl ResponseStreamSession for ResponseStreamAdapter<'_> {
    fn record_error(&mut self, code: EdgionErrorCode) {
        self.ctx.record_error(code);
    }
    fn try_claim_response_terminal(&mut self, cause: ResponseTerminalCause) -> bool {
        self.ctx.response_terminal_state.try_claim(self.slot, cause)
    }
    fn mark_response_terminal_emission(&mut self, emission: TerminalEmission) {
        self.ctx.response_terminal_state.mark_emission(emission);
    }
    fn semantic_guard_active(&mut self) -> bool {
        self.ctx.semantic_guard_active()
    }
    fn publish_canonical_records(&mut self, batch: CanonicalRecordBatch) -> Result<(), CanonicalRecordBatch> {
        self.ctx.publish_canonical_records(batch)
    }
    fn take_canonical_records(&mut self) -> Option<CanonicalRecordBatch> {
        self.ctx.take_canonical_records()
    }
    fn ai_terminal_render_context(&mut self) -> Option<(AiClientStreamDialect, FinalResponseEligibility)> {
        self.ctx.ai_terminal_render_context()
    }
}

/// Only the Trailer declaration values are retained, once per trailer dispatch.
/// HeaderValue clones share their backing bytes; body callbacks create no snapshot.
pub(crate) struct ResponseTrailerPolicy {
    representable: bool,
    all_names: bool,
    declarations: Vec<http::HeaderValue>,
}

impl ResponseTrailerPolicy {
    pub(crate) fn capture(session: &pingora_proxy::Session) -> Self {
        let downstream = session.as_downstream();
        let all_names = downstream.is_http2() || downstream.is_subrequest() || downstream.is_custom();
        Self {
            representable: downstream.response_trailers_supported(),
            all_names,
            declarations: if all_names {
                Vec::new()
            } else {
                downstream
                    .response_written()
                    .into_iter()
                    .flat_map(|response| response.headers.get_all(http::header::TRAILER).iter())
                    .cloned()
                    .collect()
            },
        }
    }
}

pub(crate) struct ResponseTrailerAdapter<'a> {
    pub(crate) ctx: &'a mut EdgionHttpContext,
    pub(crate) policy: ResponseTrailerPolicy,
}

impl ResponseTrailerSession for ResponseTrailerAdapter<'_> {
    fn record_error(&mut self, code: EdgionErrorCode) {
        self.ctx.record_error(code);
    }
    fn is_grpc_request(&self) -> bool {
        self.ctx.request_info.is_grpc_request || self.ctx.is_grpc_route_matched
    }
    fn downstream_response_trailers_representable(&self) -> bool {
        self.policy.representable
    }
    fn downstream_response_trailer_name_allowed(&self, name: &http::HeaderName) -> bool {
        self.policy.all_names
            || self
                .policy
                .declarations
                .iter()
                .filter_map(|value| value.to_str().ok())
                .flat_map(|value| value.split(','))
                .any(|declared| declared.trim().eq_ignore_ascii_case(name.as_str()))
    }
}

/// The delegated-chain ancestor a processor was installed by.
#[derive(Clone)]
pub(crate) enum LevelRoute {
    Http(std::sync::Arc<crate::routes::http::HttpRouteRuleUnit>),
    Grpc(std::sync::Arc<crate::routes::grpc::GrpcRouteRuleUnit>),
}

/// A response processor installed by a delegated-chain ancestor's plugin.
///
/// Trusted `${matchedHttpRoute:...}` / `${matchedGrpcRoute:...}` templates
/// rendered from its callbacks resolve against the installing level's route,
/// never the tenant leaf (spec §6.5). Every other capability forwards
/// unchanged. Processors installed by the leaf or an ordinary route are never
/// wrapped.
pub(crate) struct LevelScopedProcessor {
    unit: LevelRoute,
    inner: Box<dyn StreamingResponseProcessor>,
}

impl LevelScopedProcessor {
    pub(crate) fn wrap(
        unit: LevelRoute,
        inner: Box<dyn StreamingResponseProcessor>,
    ) -> Box<dyn StreamingResponseProcessor> {
        Box::new(Self { unit, inner })
    }
}

/// Response session view that answers route metadata from one level.
struct LevelScopedSession<'a> {
    unit: &'a LevelRoute,
    inner: &'a mut dyn ResponseStreamSession,
}

impl TrustedTemplateVariables for LevelScopedSession<'_> {
    fn trusted_ctx_var(&self, key: &str) -> Option<String> {
        self.inner.trusted_ctx_var(key)
    }
    fn trusted_remote_ip(&self) -> &str {
        self.inner.trusted_remote_ip()
    }
    fn trusted_matched_route_meta(&self, proto: RouteProto, selector: &str) -> Option<String> {
        match (proto, self.unit) {
            (RouteProto::Http, LevelRoute::Http(unit)) => {
                super::session_adapter::route_meta_field(&unit.route.as_ref()?.metadata, selector)
            }
            (RouteProto::Grpc, LevelRoute::Grpc(unit)) => {
                super::session_adapter::route_meta_field(&unit.route.as_ref()?.metadata, selector)
            }
            _ => self.inner.trusted_matched_route_meta(proto, selector),
        }
    }
}

impl ResponseStreamSession for LevelScopedSession<'_> {
    fn record_error(&mut self, code: EdgionErrorCode) {
        self.inner.record_error(code);
    }
    fn try_claim_response_terminal(&mut self, cause: ResponseTerminalCause) -> bool {
        self.inner.try_claim_response_terminal(cause)
    }
    fn mark_response_terminal_emission(&mut self, emission: TerminalEmission) {
        self.inner.mark_response_terminal_emission(emission);
    }
    fn semantic_guard_active(&mut self) -> bool {
        self.inner.semantic_guard_active()
    }
    fn publish_canonical_records(&mut self, batch: CanonicalRecordBatch) -> Result<(), CanonicalRecordBatch> {
        self.inner.publish_canonical_records(batch)
    }
    fn take_canonical_records(&mut self) -> Option<CanonicalRecordBatch> {
        self.inner.take_canonical_records()
    }
    fn ai_terminal_render_context(&mut self) -> Option<(AiClientStreamDialect, FinalResponseEligibility)> {
        self.inner.ai_terminal_render_context()
    }
}

#[async_trait::async_trait]
impl StreamingResponseProcessor for LevelScopedProcessor {
    fn on_response_header(&mut self, response: &mut pingora_http::ResponseHeader) {
        self.inner.on_response_header(response);
    }
    async fn on_actual_upstream_trailers(
        &mut self,
        trailers: Option<&http::HeaderMap>,
        header_end_stream: bool,
    ) -> pingora_core::Result<()> {
        self.inner
            .on_actual_upstream_trailers(trailers, header_end_stream)
            .await
    }
    fn changes_body_length(&self) -> bool {
        self.inner.changes_body_length()
    }
    fn finalize_incomplete(&mut self, outcome: &'static str, reason: &'static str) {
        self.inner.finalize_incomplete(outcome, reason);
    }
    fn on_terminal_preempted(&mut self, winner: ResponseTerminalCause) {
        self.inner.on_terminal_preempted(winner);
    }
    fn release_inflight(&mut self) {
        self.inner.release_inflight();
    }
    async fn on_event(
        &mut self,
        chunk: &mut Option<bytes::Bytes>,
        event: StreamingResponseEvent,
        stream_kind: ResponseStreamKind,
        sink: &mut pingora_proxy::ResponseBodySink,
        session: &mut dyn ResponseStreamSession,
    ) {
        let mut scoped = LevelScopedSession {
            unit: &self.unit,
            inner: session,
        };
        self.inner.on_event(chunk, event, stream_kind, sink, &mut scoped).await;
    }
    async fn on_chunk(
        &mut self,
        chunk: &mut Option<bytes::Bytes>,
        end_of_stream: bool,
        stream_kind: ResponseStreamKind,
        sink: &mut pingora_proxy::ResponseBodySink,
        session: &mut dyn ResponseStreamSession,
    ) {
        let mut scoped = LevelScopedSession {
            unit: &self.unit,
            inner: session,
        };
        self.inner
            .on_chunk(chunk, end_of_stream, stream_kind, sink, &mut scoped)
            .await;
    }
}
