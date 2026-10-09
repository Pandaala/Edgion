use async_trait::async_trait;
use pingora_core::modules::http::HttpModules;
use pingora_core::prelude::HttpPeer;
use pingora_core::protocols::Digest;
use pingora_core::Error;
use pingora_http::ResponseHeader;
use pingora_proxy::{
    FailToProxy, ProxyHttp, RequestBodyEvent, ResponseBodySink, ResponseHeadBoundary, ResponseHeadBoundaryAction,
    ResponseHeadCommitPlan, ResponseHeadOutcome, ResponseHeadSource, ResponseHeadUsage, Session,
    UpstreamResponseBodyEvent,
};
use std::sync::Arc;

use crate::observe::AccessLogger;
use crate::routes::grpc::GrpcPortRouteManager;
use crate::routes::http::routes_mgr::HttpPortRouteManager;
use crate::runtime::server::server_header::ServerHeaderOpts;
use crate::EdgionHttpContext;
use edgion_resources::ListenerKey;

// Sub-modules
pub mod content_type;
pub mod global_plugins_resolve;
pub mod parse_timeout;
pub mod preflight_handler;

// Pingora ProxyHttp trait implementation stages
mod pg_connected_to_upstream;
mod pg_early_request_filter;
mod pg_error_while_proxy;
mod pg_fail_to_connect;
mod pg_fail_to_proxy;
mod pg_init_downstream_modules;
mod pg_logging;
mod pg_new_ctx;
mod pg_request_body_filter;
mod pg_request_body_filter_action;
mod pg_request_filter;
mod pg_response_filter;
mod pg_response_head;
mod pg_upstream_peer;
mod pg_upstream_request_filter;
mod pg_upstream_response_body_filter;
mod pg_upstream_response_filter;
mod pg_upstream_response_trailer_filter;

#[cfg(test)]
pub(crate) use pg_upstream_response_body_filter::run_stream_processors;

pub use content_type::{is_grpc_content_type, is_grpc_web_content_type};
pub(crate) use global_plugins_resolve::select_waf_owner;
pub(crate) use global_plugins_resolve::{resolve_gateway_plugins, resolve_global_plugins, resolve_route_ext_refs};
pub use parse_timeout::{ParsedBackendTimeout, ParsedClientTimeout, ParsedTimeouts};
pub(crate) use pg_early_request_filter::apply_client_policy;
pub use preflight_handler::PreflightHandler;

/// Returns whether the configured retry budget permits another upstream attempt.
///
/// `completed_attempts` includes the initial request, while `retry_attempts`
/// counts only attempts after it. This keeps the public contract at
/// `1 + retry_attempts` total upstream requests without overflow at the route
/// schema's `i32::MAX` ceiling.
#[inline]
fn retry_budget_allows_next_attempt(completed_attempts: u32, retry_attempts: u32) -> bool {
    completed_attempts <= retry_attempts
}

/// HTTP proxy service for Gateway HTTP/HTTPS listeners.
pub struct EdgionHttpProxy {
    pub listener_port: u16,

    pub is_https_listener: bool,

    pub listener_key: ListenerKey,

    pub server_header_opts: ServerHeaderOpts,

    /// Access logger for writing access logs
    pub access_logger: Arc<AccessLogger>,

    /// Stable EGC binding; each request captures its current immutable generation.
    pub egc: Arc<crate::config::GatewayConfigHandle>,

    /// Whether HTTP/2 is enabled for this listener
    pub enable_http2: bool,

    /// Preflight request handler
    pub preflight_handler: PreflightHandler,

    /// Cached per-listener HTTP route manager. The outer `Arc` is stable for the
    /// lifetime of a serving listener (route hot-reload only swaps the inner
    /// `ArcSwap<DomainRouteRules>`), so resolving it once at construction
    /// removes a `DashMap` shard write-lock and a `ListenerKey(String)` clone
    /// from every request.
    pub http_route_manager: Arc<HttpPortRouteManager>,

    /// Cached per-listener gRPC route manager. Same stability rationale as
    /// `http_route_manager`.
    pub grpc_route_manager: Arc<GrpcPortRouteManager>,
}

#[async_trait]
impl ProxyHttp for EdgionHttpProxy {
    type CTX = EdgionHttpContext;

    fn new_ctx(&self) -> Self::CTX {
        pg_new_ctx::new_ctx(self)
    }

    fn persist_connection_context(
        &self,
        _session: &Session,
        ctx: &Self::CTX,
    ) -> Option<Box<dyn std::any::Any + Send + Sync>> {
        ctx.downstream_keepalive_initialized
            .then(|| Box::new(()) as Box<dyn std::any::Any + Send + Sync>)
    }

    fn on_connection_reuse(
        &self,
        _session: &mut Session,
        ctx: &mut Self::CTX,
        _previous: Box<dyn std::any::Any + Send + Sync>,
    ) {
        ctx.downstream_keepalive_initialized = true;
    }

    async fn upstream_peer(&self, session: &mut Session, ctx: &mut Self::CTX) -> pingora_core::Result<Box<HttpPeer>> {
        pg_upstream_peer::upstream_peer(self, session, ctx).await
    }

    fn init_downstream_modules(&self, modules: &mut HttpModules) {
        pg_init_downstream_modules::init_downstream_modules(self, modules)
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> pingora_core::Result<bool>
    where
        Self::CTX: Send + Sync,
    {
        pg_request_filter::request_filter(self, session, ctx).await
    }

    async fn early_request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> pingora_core::Result<()>
    where
        Self::CTX: Send + Sync,
    {
        pg_early_request_filter::early_request_filter(self, session, ctx).await
    }

    // `request_body_filter` intentionally keeps its default: overriding
    // `request_body_filter_action` means the proxy never calls the legacy
    // hook. The action drives streamed body handlers first, then the mirror
    // tee (`pg_request_body_filter`), so the mirror sees post-mutation chunks.
    async fn request_body_filter_action(
        &self,
        session: &mut Session,
        body: &mut Option<bytes::Bytes>,
        event: RequestBodyEvent,
        ctx: &mut Self::CTX,
    ) -> pingora_core::Result<pingora_proxy::RequestBodyAction>
    where
        Self::CTX: Send + Sync,
    {
        pg_request_body_filter_action::request_body_filter_action(self, session, body, event, ctx).await
    }

    async fn request_trailer_filter(
        &self,
        session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> pingora_core::Result<pingora_proxy::RequestBodyAction> {
        pg_request_body_filter_action::request_trailer_filter(session, ctx).await
    }

    fn request_relay_plan(&self, _session: &Session, ctx: &Self::CTX) -> pingora_proxy::RequestRelayPlan {
        // Streamed once any per-chunk handler was installed: chunk mutation
        // may change the length, so the upstream must use chunked framing.
        // Strictly empty bodies are safely coerced back to Ordinary. A final
        // upgrade/CONNECT or H1.0 upstream shape instead fails closed before
        // its header is written. Keyed on the monotonic latch, not the handler
        // vector (see the ctx field docs).
        if ctx.request_body_stream_active() {
            pingora_proxy::RequestRelayPlan::streamed()
        } else {
            pingora_proxy::RequestRelayPlan::ordinary()
        }
    }

    fn response_head_may_hold(&self, _session: &Session, ctx: &Self::CTX) -> bool {
        pg_response_head::response_head_may_hold(ctx)
    }

    async fn upstream_request_filter(
        &self,
        session: &mut Session,
        upstream_request: &mut pingora_http::RequestHeader,
        ctx: &mut Self::CTX,
    ) -> pingora_core::Result<()>
    where
        Self::CTX: Send + Sync,
    {
        pg_upstream_request_filter::upstream_request_filter(self, session, upstream_request, ctx).await
    }

    async fn upstream_response_filter(
        &self,
        session: &mut Session,
        upstream_response: &mut ResponseHeader,
        ctx: &mut Self::CTX,
    ) -> pingora_core::Result<()>
    where
        Self::CTX: Send + Sync,
    {
        pg_upstream_response_filter::upstream_response_filter(self, session, upstream_response, ctx)
    }

    async fn upstream_response_header_filter_event(
        &self,
        session: &mut Session,
        upstream_response: &mut ResponseHeader,
        end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> pingora_core::Result<()>
    where
        Self::CTX: Send + Sync,
    {
        if !pg_upstream_response_filter::is_interim_informational(upstream_response.status) {
            ctx.upstream_response_headers_end_stream = Some(end_of_stream);
        }
        // The fork follows a terminal Header event with the typed
        // TerminalWithoutTrailers callback. Keep the header hook focused on
        // final-header processing so body processors flush before companions.
        pg_upstream_response_filter::upstream_response_filter(self, session, upstream_response, ctx)
    }

    async fn response_filter(
        &self,
        session: &mut Session,
        upstream_response: &mut ResponseHeader,
        ctx: &mut Self::CTX,
    ) -> pingora_core::Result<()>
    where
        Self::CTX: Send + Sync,
    {
        pg_response_filter::response_filter(self, session, upstream_response, ctx).await
    }

    fn response_head_commit_plan(
        &self,
        session: &Session,
        source: ResponseHeadSource,
        response: &ResponseHeader,
        ctx: &Self::CTX,
    ) -> pingora_core::Result<ResponseHeadCommitPlan> {
        pg_response_head::response_head_commit_plan(session, source, response, ctx)
    }

    fn response_head_will_commit(
        &self,
        session: &Session,
        chosen_header: &ResponseHeader,
        ctx: &mut Self::CTX,
    ) -> pingora_core::Result<()> {
        pg_response_head::response_head_will_commit(session, chosen_header, ctx)
    }

    fn response_head_hold_boundary(
        &self,
        _session: &Session,
        boundary: ResponseHeadBoundary,
        ctx: &mut Self::CTX,
    ) -> ResponseHeadBoundaryAction {
        pg_response_head::response_head_hold_boundary(boundary, ctx)
    }

    fn response_head_hold_outcome(
        &self,
        _session: &Session,
        outcome: ResponseHeadOutcome,
        usage: ResponseHeadUsage,
        ctx: &mut Self::CTX,
    ) {
        pg_response_head::response_head_hold_outcome(outcome, usage, ctx)
    }

    /// Returns `Ok(None)` to continue, `Ok(Some(duration))` to rate-limit for the given duration.
    async fn upstream_response_body_filter(
        &self,
        session: &mut Session,
        body: &mut Option<bytes::Bytes>,
        end_of_stream: bool,
        sink: &mut ResponseBodySink,
        ctx: &mut Self::CTX,
    ) -> pingora_core::Result<Option<std::time::Duration>>
    where
        Self::CTX: Send + Sync,
    {
        pg_upstream_response_body_filter::upstream_response_body_filter(self, session, body, end_of_stream, sink, ctx)
            .await
    }

    async fn upstream_response_body_filter_event(
        &self,
        session: &mut Session,
        body: &mut Option<bytes::Bytes>,
        event: UpstreamResponseBodyEvent,
        sink: &mut ResponseBodySink,
        ctx: &mut Self::CTX,
    ) -> pingora_core::Result<Option<std::time::Duration>>
    where
        Self::CTX: Send + Sync,
    {
        pg_upstream_response_body_filter::upstream_response_body_filter_event(self, session, body, event, sink, ctx)
            .await
    }

    async fn upstream_response_trailer_filter(
        &self,
        session: &mut Session,
        trailers: &mut http::HeaderMap,
        ctx: &mut Self::CTX,
    ) -> pingora_core::Result<()>
    where
        Self::CTX: Send + Sync,
    {
        pg_upstream_response_trailer_filter::dispatch_upstream_response_trailers(session, ctx, Some(trailers)).await?;
        crate::ai::conversion::chat_to_anthropic::head::compact_trailers(trailers, ctx)
    }

    async fn response_trailer_filter(
        &self,
        _session: &mut Session,
        trailers: &mut http::HeaderMap,
        ctx: &mut Self::CTX,
    ) -> pingora_core::Result<Option<bytes::Bytes>> {
        let Some(plan) = &ctx.grpc_transcoding else {
            return Ok(None);
        };
        trailers.clear();
        Ok(plan.terminal.lock().take())
    }

    async fn logging(&self, session: &mut Session, e: Option<&Error>, ctx: &mut Self::CTX)
    where
        Self::CTX: Send + Sync,
    {
        pg_logging::logging(self, session, e, ctx).await
    }

    fn error_while_proxy(
        &self,
        peer: &HttpPeer,
        session: &mut Session,
        e: Box<Error>,
        ctx: &mut Self::CTX,
        client_reused: bool,
    ) -> Box<Error> {
        pg_error_while_proxy::error_while_proxy(self, peer, session, e, ctx, client_reused)
    }

    fn fail_to_connect(
        &self,
        session: &mut Session,
        peer: &HttpPeer,
        ctx: &mut Self::CTX,
        e: Box<Error>,
    ) -> Box<Error> {
        pg_fail_to_connect::fail_to_connect(self, session, peer, ctx, e)
    }

    async fn fail_to_proxy(&self, session: &mut Session, e: &Error, ctx: &mut Self::CTX) -> FailToProxy
    where
        Self::CTX: Send + Sync,
    {
        pg_fail_to_proxy::fail_to_proxy(self, session, e, ctx).await
    }

    fn suppress_error_log(&self, _session: &Session, ctx: &Self::CTX, _error: &Error) -> bool {
        // filter-04b: the sentinel abort from a written local reply is not a real
        // error; hide its error-log line (the access log still records the reply).
        ctx.local_reply_written()
    }

    fn request_summary(&self, session: &Session, ctx: &Self::CTX) -> String {
        // Default summary plus the request's trace id (always present: client
        // header or generated at entry), so a "Fail to proxy" error-log line
        // can be joined with its access-log entry instead of a heuristic
        // timestamp+path match.
        let summary = session.as_ref().request_summary();
        match ctx.request_info.trace_id.standard_trace_id() {
            Some(trace_id) => format!("{summary}, trace-id: {trace_id}"),
            None => summary,
        }
    }

    async fn connected_to_upstream(
        &self,
        session: &mut Session,
        reused: bool,
        peer: &HttpPeer,
        #[cfg(unix)] fd: std::os::unix::io::RawFd,
        #[cfg(windows)] sock: std::os::windows::io::RawSocket,
        digest: Option<&Digest>,
        ctx: &mut Self::CTX,
    ) -> pingora_core::Result<()>
    where
        Self::CTX: Send + Sync,
    {
        pg_connected_to_upstream::connected_to_upstream(
            self,
            session,
            reused,
            peer,
            #[cfg(unix)]
            fd,
            #[cfg(windows)]
            sock,
            digest,
            ctx,
        )
        .await
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/mod_test/retry_budget_tests.rs"]
mod retry_budget_tests;
