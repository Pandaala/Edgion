use async_trait::async_trait;
use pingora_core::modules::http::HttpModules;
use pingora_core::prelude::HttpPeer;
use pingora_core::protocols::Digest;
use pingora_core::Error;
use pingora_http::ResponseHeader;
use pingora_proxy::{FailToProxy, ProxyHttp, RequestBodyEvent, ResponseBodySink, Session};
use std::sync::Arc;
use std::time::SystemTime;

use crate::observe::AccessLogger;
use crate::routes::grpc::GrpcPortRouteManager;
use crate::routes::http::routes_mgr::HttpPortRouteManager;
use crate::runtime::server::server_header::ServerHeaderOpts;
use crate::EdgionHttpContext;
use edgion_resources::{EdgionGatewayConfig, Listener, ListenerKey};

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
mod pg_upstream_peer;
mod pg_upstream_request_filter;
mod pg_upstream_response_body_filter;
mod pg_upstream_response_filter;

#[cfg(test)]
pub(crate) use pg_upstream_peer::backend_snapshot_for_request;
#[cfg(test)]
pub(crate) use pg_upstream_response_body_filter::run_stream_processors;

pub use content_type::{is_grpc_content_type, is_grpc_web_content_type};
pub use global_plugins_resolve::{resolve_gateway_plugins, resolve_global_plugins, resolve_route_ext_refs};
pub use parse_timeout::{ParsedBackendTimeout, ParsedClientTimeout, ParsedTimeouts};
pub use preflight_handler::PreflightHandler;

/// HTTP proxy service for Gateway HTTP/HTTPS listeners.
pub struct EdgionHttpProxy {
    pub gateway_class_name: Option<String>,

    pub listener: Listener,

    pub listener_key: ListenerKey,

    pub server_start_time: SystemTime,

    pub server_header_opts: ServerHeaderOpts,

    /// Access logger for writing access logs
    pub access_logger: Arc<AccessLogger>,

    /// Global gateway configuration
    pub edgion_gateway_config: Arc<EdgionGatewayConfig>,

    /// EGC name used to load the pre-resolved request-body policy for each new
    /// request. `None` selects the built-in default.
    pub request_body_config_name: Option<String>,

    /// Pre-parsed timeout configurations (always has default values if not configured)
    pub parsed_timeouts: ParsedTimeouts,

    /// Whether HTTP/2 is enabled for this listener
    pub enable_http2: bool,

    /// Real IP extractor for trusted proxy support
    pub real_ip_extractor: Option<Arc<crate::util::RealIpExtractor>>,

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
    //
    // `request_trailer_filter` also stays default: the proxy never forwards
    // request trailers (transport drops them), so a trailer-bearing request
    // loses nothing a streamed inspection policy could have seen — every body
    // byte still flows through the hook.
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

    fn upstream_request_body_disposition(
        &self,
        _session: &Session,
        ctx: &Self::CTX,
    ) -> pingora_proxy::UpstreamRequestBodyDisposition {
        // Streamed once any per-chunk handler was installed: chunk mutation
        // may change the length, so the upstream must use chunked framing.
        // The proxy coerces back to Ordinary where Streamed is unsafe
        // (upgrade/CONNECT, empty-and-done body, H1.0 upstream). Keyed on the
        // monotonic latch, not the handler vector (see the ctx field docs).
        if ctx.request_body_stream_active {
            pingora_proxy::UpstreamRequestBodyDisposition::Streamed
        } else {
            pingora_proxy::UpstreamRequestBodyDisposition::Ordinary
        }
    }

    fn request_retry_allowed(&self, _session: &Session, ctx: &Self::CTX) -> bool {
        // Streamed chunks are forwarded without any replay buffer, so a
        // request stops being replayable the moment a handler is installed.
        // The latch is monotonic by construction (set once, never cleared),
        // as the contract requires — the handler vector is not, because the
        // hook takes it across await points and the future may be dropped.
        !ctx.request_body_stream_active
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
        ctx.local_reply_written
    }

    fn request_summary(&self, session: &Session, ctx: &Self::CTX) -> String {
        // Default summary plus the request's trace id (always present: client
        // header or generated at entry), so a "Fail to proxy" error-log line
        // can be joined with its access-log entry instead of a heuristic
        // timestamp+path match.
        let summary = session.as_ref().request_summary();
        match ctx.request_info.x_trace_id.as_deref() {
            Some(trace_id) => format!("{summary}, x-trace-id: {trace_id}"),
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

    fn allow_spawning_subrequest(&self, _session: &Session, ctx: &Self::CTX) -> bool
    where
        Self::CTX: Send + Sync,
    {
        // Allow subrequests only when a plugin opts in (e.g. the Mirror plugin).
        ctx.get_ctx_var("_enable_subrequest") == Some("true")
    }
}
