use async_trait::async_trait;
use pingora_core::modules::http::grpc_web::{GrpcWeb, GrpcWebBridge};
use pingora_core::modules::http::HttpModules;
use pingora_core::prelude::HttpPeer;
use pingora_core::{Error as PingoraError, ErrorType};

use pingora_http::ResponseHeader;
use pingora_proxy::{ProxyHttp, Session};
use crate::core::gateway::edgion_http::EdgionHttp;
use crate::types::EdgionHttpContext;
use crate::types::filters::PluginRunningResult;
use crate::core::gateway::{end_response_400, end_response_404, end_response_500};
use crate::core::backends::get_peer;
use crate::types::EdgionStatus;
use crate::types::err::EdError;
use crate::core::observe::{AccessLogEntry, global_metrics};

#[async_trait]
impl ProxyHttp for EdgionHttp {
    type CTX = EdgionHttpContext;

    fn new_ctx(&self) -> Self::CTX {
        let ctx = EdgionHttpContext::new();
        global_metrics().ctx_created();
        ctx
    }

    async fn upstream_peer(&self, session: &mut Session, ctx: &mut Self::CTX) -> pingora_core::Result<Box<HttpPeer>> {
        tracing::info!("upstream_peer");

        // Get selected backend from context (already selected in request_filter)
        let backend_ref = match ctx.selected_backend.clone() {
            Some(backend) => backend,
            None => {
                ctx.add_error(EdgionStatus::UpstreamNotRouteMatched);
                end_response_500(session, ctx).await?;
                return Err(PingoraError::new(ErrorType::InternalError));
            }
        };
        tracing::info!("Selected backend: {:?}", backend_ref);

        let match_info = ctx.matched_info.clone().unwrap();
        get_peer(&match_info, &backend_ref, session, ctx).await
    }


    fn init_downstream_modules(&self, modules: &mut HttpModules) {
        modules.add_module(Box::new(GrpcWeb));
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> pingora_core::Result<bool>
    where
        Self::CTX: Send + Sync,
    {

        let req_header = session.req_header();
        match req_header.headers.get("host").and_then(|h| h.to_str().ok())
        {
            Some(host) => {
                ctx.request_info.hostname = host.to_string();
            }
            None => {
                ctx.add_error(EdgionStatus::HostMissing);
                end_response_400(session, ctx).await?;
                return Ok(true);
            }
        }
        ctx.request_info.path = req_header.uri.path().to_string();

        // Match route and select backend
        match self.domain_routes.match_route(&ctx.request_info.hostname, session) {
            Ok((match_info, selected_backend)) => {
                tracing::info!("selected_backend: {:?}", selected_backend);

                // Run rule-level request filters first
                match_info.rule_filter_runtime.run_request_filters(session, ctx).await;
                if ctx.filter_running_result == PluginRunningResult::ErrTerminateRequest {
                    return Ok(true);
                }

                // Then run backend-level request filters
                selected_backend.filter_runtime.run_request_filters(session, ctx).await;
                if ctx.filter_running_result == PluginRunningResult::ErrTerminateRequest {
                    return Ok(true);
                }

                ctx.matched_info = Some(match_info);
                ctx.selected_backend = Some(selected_backend);
                Ok(false)
            }
            Err(e) => {
                match e {
                    EdError::RouteNotFound() => {
                        ctx.add_error(EdgionStatus::RouteNotFound);
                        end_response_404(session, ctx).await?;
                    }
                    EdError::BackendNotFound() => {
                        ctx.add_error(EdgionStatus::UpstreamNotBackendRefs);
                        end_response_500(session, ctx).await?;
                    }
                    EdError::InconsistentWeight() => {
                        ctx.add_error(EdgionStatus::UpstreamInconsistentWeight);
                        end_response_500(session, ctx).await?;
                    }
                    _ => {
                        ctx.add_error(EdgionStatus::Unknown);
                        end_response_500(session, ctx).await?;
                    }
                }
                ctx.matched_info = None;
                ctx.selected_backend = None;
                Ok(true)
            }
        }
    }

    /// upstream_response_filter - sync hook
    fn upstream_response_filter(
        &self,
        session: &mut Session,
        upstream_response: &mut ResponseHeader,
        ctx: &mut Self::CTX,
    ) -> pingora_core::Result<()> {
        // Run rule-level upstream_response_filter (sync)
        if let Some(match_info) = ctx.matched_info.clone() {
            match_info.rule_filter_runtime.run_upstream_response_filters_sync(session, ctx, upstream_response);
        }

        // Run backend-level upstream_response_filter (sync)
        if let Some(backend) = ctx.selected_backend.clone() {
            backend.filter_runtime.run_upstream_response_filters_sync(session, ctx, upstream_response);
        }

        Ok(())
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
        // Run rule-level response filters (async)
        if let Some(match_info) = ctx.matched_info.clone() {
            match_info.rule_filter_runtime.run_upstream_response_filters_async(session, ctx, upstream_response).await;
        }

        // Run backend-level response filters (async)
        if let Some(backend) = ctx.selected_backend.clone() {
            backend.filter_runtime.run_upstream_response_filters_async(session, ctx, upstream_response).await;
        }

        Ok(())
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

    async fn logging(&self, session: &mut Session, _e: Option<&PingoraError>, ctx: &mut Self::CTX)
    where
        Self::CTX: Send + Sync,
    {
        // Update response status from session
        if let Some(resp_header) = session.response_written() {
            ctx.request_info.status = resp_header.status.as_u16();
        }

        // Calculate latency
        let latency_ms = ctx.start_time.elapsed().as_millis() as u64;

        // Create access log entry and send
        if let Some(logger) = &self.access_logger {
            let entry = AccessLogEntry::from_context(ctx, latency_ms);
            logger.send(entry.to_json()).await;
        }
    }
} 