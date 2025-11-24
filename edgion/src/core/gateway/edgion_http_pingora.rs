use std::sync::atomic::Ordering;
use async_trait::async_trait;
use pingora_core::modules::http::grpc_web::{GrpcWeb, GrpcWebBridge};
use pingora_core::modules::http::HttpModules;
use pingora_core::prelude::HttpPeer;
use pingora_http::ResponseHeader;
use pingora_proxy::{ProxyHttp, Session};
use crate::core::gateway::edgion_http::EdgionHttp;
use crate::core::gateway::edgion_http_context::EdgionHttpContext;

#[async_trait]
impl ProxyHttp for EdgionHttp {
    type CTX = EdgionHttpContext;

    fn new_ctx(&self) -> Self::CTX {
        let ctx = EdgionHttpContext::new();
        self.ctx_cnt.fetch_add(1, Ordering::Relaxed);
        ctx
    }

    fn init_downstream_modules(&self, modules: &mut HttpModules) {
        modules.add_module(Box::new(GrpcWeb));
    }


    async fn early_request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> pingora_core::Result<()>
    where
        Self::CTX: Send + Sync,
    {

        // process gprc
        let req_header = session.req_header();
        if let Some(content_type) = req_header.headers.get("content-type") {
            if let Ok(ct_str) = content_type.to_str() {
                if ct_str.len() >= 21 && ct_str[..21].eq_ignore_ascii_case("application/grpc-web") {
                    if let Some(grpc) = session.downstream_modules_ctx.get_mut::<GrpcWebBridge>() {
                        grpc.init();
                        ctx.auto_gprc = true;
                    }
                }
            }
        }

        Ok(())
    }

    async fn request_filter(&self, session: &mut Session, _ctx: &mut Self::CTX) -> pingora_core::Result<bool>
    where
        Self::CTX: Send + Sync,
    {
        // Get hostname from request header
        let hostname = match session
            .req_header()
            .headers
            .get("host")
            .and_then(|h| h.to_str().ok())
        {
            Some(host) => host.to_string(),
            None => {
                // No hostname provided, return 404
                tracing::warn!(
                    "Request missing Host header, path: {}",
                    session.req_header().uri.path()
                );
                let resp = Box::new(ResponseHeader::build(400, None).unwrap());
                session.write_response_header(resp, false).await?;
                return Ok(false);
            }
        };

        // Match route using domain_routes
        match self.domain_routes.match_route(&hostname, session) {
            Ok(route_entry) => {
                tracing::info!(
                    "Route matched: {} for hostname: {}, path: {}",
                    route_entry.identifier(),
                    hostname,
                    session.req_header().uri.path()
                );
                // TODO: Store matched route in context for later use (plugins, upstream selection, etc.)
                // For now, just return true to continue processing
                Ok(true)
            }
            Err(e) => {
                tracing::debug!(
                    "No route matched for hostname: {}, path: {}, error: {:?}",
                    hostname,
                    session.req_header().uri.path(),
                    e
                );
                // Return 404 for no route matched
                let resp = Box::new(ResponseHeader::build(404, None).unwrap());
                session.write_response_header(resp, false).await?;
                Ok(false)
            }
        }
    }

    async fn upstream_peer(&self, _session: &mut Session, _ctx: &mut Self::CTX) -> pingora_core::Result<Box<HttpPeer>> {
        todo!()
    }
}