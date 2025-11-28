use std::sync::atomic::Ordering;
use async_trait::async_trait;
use pingora_core::modules::http::grpc_web::{GrpcWeb, GrpcWebBridge};
use pingora_core::modules::http::HttpModules;
use pingora_core::prelude::HttpPeer;
use pingora_core::{Error as PingoraError, ErrorType};
use pingora_proxy::{ProxyHttp, Session};
use crate::core::gateway::edgion_http::EdgionHttp;
use crate::core::gateway::edgion_http_context::EdgionHttpContext;
use crate::core::gateway::{end_response_400, end_response_404, end_response_500, select_backend_ref};
use crate::types::EdgionErrStatus;

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

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> pingora_core::Result<bool>
    where
        Self::CTX: Send + Sync,
    {

        let req_header = session.req_header();
        match req_header.headers.get("host").and_then(|h| h.to_str().ok())
        {
            Some(host) => {
                ctx.hostname = host.to_string();
            }
            None => {
                ctx.add_error(EdgionErrStatus::HostMissing);
                end_response_400(session).await?;
                return Ok(true);
            }
        }

        // Match route using domain_routes
        match self.domain_routes.match_route(&ctx.hostname, session) {
            Ok(matched_rule) => {
                tracing::info!("matched_rule: {:?}", matched_rule);
                ctx.matched_http_route = Some(matched_rule);
                Ok(false)
            }
            Err(_e) => {
                ctx.add_error(EdgionErrStatus::RouteNotFound);
                end_response_404(session).await?;
                ctx.matched_http_route = None;
                Ok(true)
            }
        }
    }

    async fn upstream_peer(&self, session: &mut Session, ctx: &mut Self::CTX) -> pingora_core::Result<Box<HttpPeer>> {
        tracing::info!("upstream_peer");

        // Get matched route from context
        let matched_route = match &ctx.matched_http_route {
            Some(route) => route.clone(),
            None => {
                ctx.add_error(EdgionErrStatus::UpstreamNotRouteMatched);
                end_response_500(session).await?;
                return Err(PingoraError::new(ErrorType::InternalError));
            }
        };

        // Select backend using weighted round-robin
        let backend_ref = select_backend_ref(session, ctx, &matched_route).await?;
        tracing::info!("Selected backend: {:?}", backend_ref);

        // Build HttpPeer (use name:port as address)
        let addr = format!("{}:{}", backend_ref.name, backend_ref.port.unwrap_or(80));
        let peer = HttpPeer::new(addr, false, String::new());
        
        Ok(Box::new(peer))
    }
} 