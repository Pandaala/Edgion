use async_trait::async_trait;
use pingora_core::modules::http::HttpModules;
use pingora_core::prelude::HttpPeer;
use pingora_core::protocols::Digest;
use pingora_core::Error;
use pingora_http::ResponseHeader;
use pingora_proxy::{FailToProxy, ProxyHttp, Session};
use std::sync::Arc;
use std::time::SystemTime;

use crate::core::gateway::server_header::ServerHeaderOpts;
use crate::core::observe::AccessLogger;
use crate::types::{EdgionGatewayConfig, EdgionHttpContext, Listener};

// Sub-modules
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
mod pg_request_filter;
mod pg_response_filter;
mod pg_upstream_peer;
mod pg_upstream_response_body_filter;
mod pg_upstream_response_filter;

// Re-exports
pub use parse_timeout::{ParsedBackendTimeout, ParsedClientTimeout, ParsedTimeouts};
pub use preflight_handler::PreflightHandler;

/// EdgionHttp proxy structure
pub struct EdgionHttp {
    pub gateway_class_name: Option<String>,

    pub listener: Listener,

    pub server_start_time: SystemTime,

    pub server_header_opts: ServerHeaderOpts,

    /// Access logger for writing access logs
    pub access_logger: Arc<AccessLogger>,

    /// Global gateway configuration
    pub edgion_gateway_config: Arc<EdgionGatewayConfig>,

    /// Pre-parsed timeout configurations (always has default values if not configured)
    pub parsed_timeouts: ParsedTimeouts,

    /// Whether HTTP/2 is enabled for this listener
    pub enable_http2: bool,

    /// Real IP extractor for trusted proxy support
    pub real_ip_extractor: Option<Arc<crate::core::utils::RealIpExtractor>>,

    /// Preflight request handler
    pub preflight_handler: PreflightHandler,
}

#[async_trait]
impl ProxyHttp for EdgionHttp {
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

    async fn request_body_filter(
        &self,
        session: &mut Session,
        body: &mut Option<bytes::Bytes>,
        end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> pingora_core::Result<()>
    where
        Self::CTX: Send + Sync,
    {
        pg_request_body_filter::request_body_filter(self, session, body, end_of_stream, ctx).await
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

    /// response_filter - async hook
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

    /// upstream_response_body_filter - called when receiving body chunks from upstream
    /// Returns Ok(None) to continue, Ok(Some(duration)) to rate limit for the given duration
    fn upstream_response_body_filter(
        &self,
        session: &mut Session,
        body: &mut Option<bytes::Bytes>,
        end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> pingora_core::Result<Option<std::time::Duration>> {
        pg_upstream_response_body_filter::upstream_response_body_filter(self, session, body, end_of_stream, ctx)
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

    /// fail_to_connect - called when connection to upstream fails
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
        // Allow subrequests if explicitly enabled via context variable
        // This allows plugins (like Mirror plugin) to enable it on demand
        ctx.get_ctx_var("_enable_subrequest") == Some("true")
    }
}
