use std::sync::atomic::Ordering;
use async_trait::async_trait;
use pingora_core::modules::http::grpc_web::{GrpcWeb, GrpcWebBridge};
use pingora_core::modules::http::HttpModules;
use pingora_core::prelude::HttpPeer;
use pingora_http::ResponseHeader;
use pingora_proxy::{ProxyHttp, Session};
use crate::core::gateway::edgion_http::EdgionHttp;
use crate::core::gateway::edgion_http_context::EdgionHttpContext;
use crate::types::EdgionErrCode;

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
        // Get hostname from request header
        let hostname = match session
            .req_header()
            .headers
            .get("host")
            .and_then(|h| h.to_str().ok())
        {
            Some(host) => host.to_string(),
            None => {
                // No hostname provided, add error code and return 400
                let err_code = EdgionErrCode::HostMissing;
                ctx.add_error(err_code);
                tracing::warn!(
                    "Request missing Host header, path: {}, error_code: 0x{:X}, message: {}",
                    session.req_header().uri.path(),
                    err_code.code(),
                    err_code.message()
                );
                let resp = Box::new(ResponseHeader::build(err_code.http_status(), None).unwrap());
                session.write_response_header(resp, true).await?;
                session.shutdown().await;
                return Ok(false);
            }
        };

        // Match route using domain_routes
        match self.domain_routes.match_route(&hostname, session) {
            Ok(matched_rule) => {
                tracing::info!(
                    "Route matched for hostname: {}, path: {}",
                    hostname,
                    session.req_header().uri.path()
                );
                ctx.matched_http_route = Option::from(matched_rule);
                Ok(true)
            }
            Err(e) => {
                // Route not found, add error code and return 404
                let err_code = EdgionErrCode::RouteNotFound;
                ctx.add_error(err_code);
                tracing::info!(
                    "No route matched for hostname: {}, path: {}, error: {:?}, error_code: 0x{:X}, message: {}",
                    hostname,
                    session.req_header().uri.path(),
                    e,
                    err_code.code(),
                    err_code.message()
                );
                let resp = Box::new(ResponseHeader::build(err_code.http_status(), None).unwrap());
                session.write_response_header(resp, true).await?;
                session.shutdown().await;
                Ok(false)
            }
        }
    }

    async fn upstream_peer(&self, _session: &mut Session, ctx: &mut Self::CTX) -> pingora_core::Result<Box<HttpPeer>> {
        // 检查是否有匹配的路由
        let matched_route = match &ctx.matched_http_route {
            Some(route) => route,
            None => {
                tracing::warn!("No matched route found in context");
                return Err(pingora_core::Error::new_str("No matched route found in context"));
            }
        };

        // 从匹配的路由中提取 backend_refs
        let backend_refs = match &matched_route.backend_refs {
            Some(refs) if !refs.is_empty() => refs,
            _ => {
                tracing::warn!("No backend_refs found in matched route");
                return Err(pingora_core::Error::new_str("No backend_refs found in matched route"));
            }
        };

        // 打印 backend_refs 信息用于调试
        tracing::debug!(
            "Found {} backend reference(s) in matched route",
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

        // TODO: 实现后端服务选择逻辑
        // 根据 backend_refs 选择后端服务并创建 HttpPeer
        // 当前简单跳过，返回错误
        Err(pingora_core::Error::new_str("Upstream peer selection not implemented yet"))
    }
} 