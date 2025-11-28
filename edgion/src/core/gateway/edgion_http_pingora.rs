use std::sync::Arc;
use std::sync::atomic::Ordering;
use async_trait::async_trait;
use pingora_core::InternalError;
use pingora_core::modules::http::grpc_web::{GrpcWeb, GrpcWebBridge};
use pingora_core::modules::http::HttpModules;
use pingora_core::prelude::HttpPeer;
use pingora_proxy::{ProxyHttp, Session};
use crate::core::lb::WeightedRoundRobin;
use crate::core::gateway::edgion_http::EdgionHttp;
use crate::core::gateway::edgion_http_context::EdgionHttpContext;
use crate::core::gateway::{end_response_400, end_response_404};
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
                ctx.matched_http_route = Some(matched_rule);
                Ok(true)
            }
            Err(e) => {
                ctx.add_error(EdgionErrStatus::RouteNotFound);
                end_response_404(session).await?;
                ctx.matched_http_route = None;
                Ok(true)
            }
        }
    }

    async fn upstream_peer(&self, session: &mut Session, ctx: &mut Self::CTX) -> pingora_core::Result<Box<HttpPeer>> {

        let matched_route = match &ctx.matched_http_route {
            Some(route) => route,
            None => {
                ctx.add_error(EdgionErrStatus::UpstreamNotRouteMatched);
                return InternalError;
            }
        };

        if matched_route.backend_refs.is_none() {
            ctx.add_error(EdgionErrStatus::UpstreamNotRouteMatched);
            return InternalError;
        }

        // Extract backend_refs from the matched route
        let backend_refs = match &matched_route.backend_refs {
            Some(refs) if !refs.is_empty() => refs,
            _ => {
                tracing::warn!("No backend_refs found in matched route");
                return Err(pingora_core::Error::new_str("No backend_refs found in matched route"));
            }
        };

        // Log backend_refs information for debugging
        tracing::debug!(
            "Found {} lb reference(s) in matched route",
            backend_refs.len()
        );
        for (idx, backend_ref) in backend_refs.iter().enumerate() {
            tracing::debug!(
                "Backend[{}]: name={}, namespace={:?}, port={:?}, weight={:?}",
                idx,
                backend_ref.name,
                backend_ref.namespace,
                backend_ref.port,
                backend_ref.weight
            );
        }

        // Check if lb is initialized, create it if not
        if matched_route.lb.load().is_none() {
            // Extract weight list from backend_refs
            let weights: Vec<usize> = backend_refs
                .iter()
                .map(|br| br.weight.unwrap_or(1) as usize)
                .collect();

            // Create weighted round-robin selector (clone backend_refs)
            let selector = WeightedRoundRobin::new(backend_refs.clone(), weights);
            matched_route.lb.store(Arc::new(Some(selector)));
            tracing::info!("WeightedRoundRobin initialized with {} backends", backend_refs.len());
        }

        // Use selector to choose lb
        let lb_guard = matched_route.lb.load();
        let selector = match &**lb_guard {
            Some(s) => s,
            None => {
                tracing::error!("Selector not initialized after initialization attempt");
                return Err(pingora_core::Error::new_str("Selector not initialized"));
            }
        };

        // Select lb reference directly
        let backend_ref = selector.select();
        tracing::info!("Selected lb: {:?}", backend_ref);

        // Build HttpPeer (use name:port as address)
        let addr = format!("{}:{}", backend_ref.name, backend_ref.port.unwrap_or(80));
        let peer = HttpPeer::new(addr, false, String::new());
        
        Ok(Box::new(peer))
    }
} 