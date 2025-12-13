use async_trait::async_trait;
use pingora_core::modules::http::grpc_web::{GrpcWeb, GrpcWebBridge};
use pingora_core::modules::http::HttpModules;
use pingora_core::prelude::HttpPeer;
use pingora_core::protocols::Digest;
use pingora_core::upstreams::peer::Peer;
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
use crate::core::routes::routes_mgr::RouteRules;

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

        // Select backend_ref if not already selected (only once)
        if ctx.selected_backend.is_none() {
            // First time, select backend from route
            let route_unit = match ctx.route_unit.as_ref() {
                Some(unit) => unit,
                None => {
                    ctx.add_error(EdgionStatus::UpstreamNotRouteMatched);
                    end_response_500(session, ctx).await?;
                    return Err(PingoraError::new(ErrorType::InternalError));
                }
            };

            // Select backend from the route unit
            let backend_ref = match RouteRules::select_backend(&route_unit.rule) {
                Ok(backend) => backend,
                Err(e) => {
                    tracing::error!("Failed to select backend: {:?}", e);
                    ctx.add_error(match e {
                        EdError::BackendNotFound() => EdgionStatus::UpstreamNotBackendRefs,
                        EdError::InconsistentWeight() => EdgionStatus::UpstreamInconsistentWeight,
                        _ => EdgionStatus::Unknown,
                    });
                    end_response_500(session, ctx).await?;
                    return Err(PingoraError::new(ErrorType::InternalError));
                }
            };
            
            tracing::info!("Selected backend: {:?}", backend_ref);
            
            // Run backend-level request plugins (only on first selection)
            backend_ref.plugin_runtime.run_request_plugins(session, ctx).await;
            if ctx.plugin_running_result == PluginRunningResult::ErrTerminateRequest {
                ctx.add_error(EdgionStatus::Unknown);
                end_response_500(session, ctx).await?;
                return Err(PingoraError::new(ErrorType::InternalError));
            }
            
            // Store selected backend in context (only once)
            ctx.selected_backend = Some(backend_ref);
        }
        
        // Initialize backend context on first call (only once)
        if ctx.backend_context.is_none() {
            let backend_ref = ctx.selected_backend.as_ref().unwrap();
            let namespace = backend_ref.namespace.clone().unwrap_or_else(|| {
                ctx.route_unit.as_ref()
                    .map(|unit| unit.matched_info.rns.clone())
                    .unwrap_or_default()
            });
            ctx.init_backend_context(backend_ref.name.clone(), namespace);
        }
        
        // Get peer from backend (will update upstream info with ip and port)
        let mut peer = get_peer(session, ctx).await?;
        
        // Set backend timeouts from pre-parsed config (no runtime overhead)
        if let Some(parsed_timeouts) = &self.parsed_timeouts {
            let backend_timeout = &parsed_timeouts.backend;
            
            // Check for route-level timeout overrides
            let route_timeouts = ctx.route_unit.as_ref()
                .and_then(|unit| unit.rule.parsed_timeouts.as_ref());
            
            // Connection timeout (only from global config)
            peer.options.connection_timeout = Some(backend_timeout.connect_timeout);
            
            // Read/Write timeout: route-level per_try_timeout overrides global
            let effective_per_try_timeout = route_timeouts
                .and_then(|rt| rt.per_try_timeout)
                .unwrap_or(backend_timeout.per_try_timeout);
            
            peer.options.read_timeout = Some(effective_per_try_timeout);
            peer.options.write_timeout = Some(effective_per_try_timeout);
            
            // Idle timeout: route-level overrides global
            peer.options.idle_timeout = Some(
                route_timeouts
                    .and_then(|rt| rt.idle_timeout)
                    .unwrap_or(backend_timeout.idle_timeout)
            );
        }
        
        // Increment try count
        ctx.try_cnt += 1;
        
        // Extract peer address info and push upstream connection attempt
        let (ip, port) = peer.address().as_inet()
            .map(|addr| (Some(addr.ip().to_string()), Some(addr.port())))
            .unwrap_or((None, None));
        ctx.push_upstream(ip, port);
        
        // Set upstream start time on first try
        if ctx.upstream_start_time.is_none() {
            ctx.upstream_start_time = Some(std::time::Instant::now());
        }
        
        Ok(peer)
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

        // Match route
        match self.domain_routes.match_route(&ctx.request_info.hostname, session) {
            Ok(route_unit) => {
                // Store route unit in context
                ctx.route_unit = Some(route_unit.clone());
                
                tracing::debug!(
                    matched_info = %route_unit.matched_info,
                    "Route matched"
                );
                
                // Run rule-level request plugins
                route_unit.rule.plugin_runtime.run_request_plugins(session, ctx).await;
                if ctx.plugin_running_result == PluginRunningResult::ErrTerminateRequest {
                    return Ok(true);
                }
                
                Ok(false)
            }
            Err(_e) => {
                // Route not found, return 404
                ctx.add_error(EdgionStatus::RouteNotFound);
                end_response_404(session, ctx).await?;
                Ok(true)
            }
        }
    }

    async fn early_request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> pingora_core::Result<()>
    where
        Self::CTX: Send + Sync,
    {
        // Set client timeouts from pre-parsed config (no runtime overhead)
        if let Some(parsed_timeouts) = &self.parsed_timeouts {
            let client_timeout = &parsed_timeouts.client;
            
            // Set read timeout (pre-parsed, no runtime overhead)
            session.set_read_timeout(Some(client_timeout.read_timeout));
            
            // Set write timeout (pre-parsed, no runtime overhead)
            session.set_write_timeout(Some(client_timeout.write_timeout));
            
            // Set keepalive timeout (pre-parsed, no runtime overhead)
            session.set_keepalive(Some(client_timeout.keepalive_timeout));
        }
        
        // Extract or generate trace_id and request_id
        let req_header = session.req_header();

        // Try to get X-Trace-Id from request headers, generate if not present
        ctx.request_info.x_trace_id = req_header
            .headers
            .get("x-trace-id")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.to_string())
            .or_else(|| {
                // Generate new trace_id if not present
                Some(uuid::Uuid::new_v4().to_string())
            });

        // Try to get X-Request-Id from request headers
        ctx.request_id = req_header
            .headers
            .get("x-request-id")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.to_string());

        // process gprc

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

    /// upstream_response_filter - sync hook
    fn upstream_response_filter(
        &self,
        session: &mut Session,
        upstream_response: &mut ResponseHeader,
        ctx: &mut Self::CTX,
    ) -> pingora_core::Result<()> {
        // Run rule-level upstream_response plugins (sync)
        if let Some(route_unit) = ctx.route_unit.clone() {
            route_unit.rule.plugin_runtime.run_upstream_response_plugins_sync(session, ctx, upstream_response);
        }

        // Run backend-level upstream_response plugins (sync)
        if let Some(backend) = ctx.selected_backend.clone() {
            backend.plugin_runtime.run_upstream_response_plugins_sync(session, ctx, upstream_response);
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
        // Run rule-level response plugins (async)
        if let Some(route_unit) = ctx.route_unit.clone() {
            route_unit.rule.plugin_runtime.run_upstream_response_plugins_async(session, ctx, upstream_response).await;
        }

        // Run backend-level response plugins (async)
        if let Some(backend) = ctx.selected_backend.clone() {
            backend.plugin_runtime.run_upstream_response_plugins_async(session, ctx, upstream_response).await;
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

        // Create access log entry
        let entry = AccessLogEntry::from_context(ctx);
        
        // In DEBUG mode, print access log to terminal
        if tracing::level_filters::LevelFilter::current() >= tracing::level_filters::LevelFilter::DEBUG {
            tracing::debug!(
                access_log = %entry.to_json(),
                "Access log"
            );
        }
        
        // Send to access logger if configured
        if let Some(logger) = &self.access_logger {
            logger.send(entry.to_json()).await;
        }
    }
    
    async fn connected_to_upstream(
        &self,
        _session: &mut Session,
        _reused: bool,
        _peer: &HttpPeer,
        #[cfg(unix)] _fd: std::os::unix::io::RawFd,
        #[cfg(windows)] _sock: std::os::windows::io::RawSocket,
        _digest: Option<&Digest>,
        ctx: &mut Self::CTX,
    ) -> pingora_core::Result<()>
    where
        Self::CTX: Send + Sync,
    {
        // Record connection time in current upstream
        if let Some(upstream) = ctx.get_current_upstream_mut() {
            upstream.ct = Some(std::time::Instant::now());
        }
        
        Ok(())
    }
} 