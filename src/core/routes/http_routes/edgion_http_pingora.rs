use async_trait::async_trait;
use pingora_core::modules::http::grpc_web::{GrpcWeb, GrpcWebBridge};
use pingora_core::modules::http::HttpModules;
use pingora_core::modules::http::compression::ResponseCompressionBuilder;
use pingora_core::prelude::HttpPeer;
use pingora_core::protocols::Digest;
use pingora_core::upstreams::peer::Peer;
use pingora_core::{ConnectionClosed, Error as PingoraError, Error, ErrorSource, ErrorType, HTTPStatus, ReadError, WriteError};

use pingora_http::ResponseHeader;
use pingora_proxy::{FailToProxy, ProxyHttp, Session};
use tracing::log::error;
use super::edgion_http::EdgionHttp;
use crate::types::EdgionHttpContext;
use crate::types::filters::PluginRunningResult;
use crate::core::gateway::{end_response_400, end_response_404, end_response_500};
use crate::core::backends::get_peer;
use crate::types::EdgionStatus;
use crate::types::err::EdError;
use crate::core::observe::{AccessLogEntry, global_metrics};
use crate::core::routes::http_routes::routes_mgr::RouteRules;
use crate::core::routes::grpc_routes::{
    try_match_grpc_route,
    run_grpc_route_plugins,
    handle_grpc_upstream,
};
use crate::core::routes::http_routes::extract_ip_string;

/// Build request metadata: client addresses, hostname, path, trace_id, x-forwarded-for, and protocol detection (inline for performance)
/// Also validates X-Forwarded-For header length against security_protect configuration.
/// Returns Ok(true) if response has been sent (due to validation failure), Ok(false) to continue.
#[inline]
async fn build_request_metadata(
    edgion_http: &EdgionHttp,
    session: &mut Session,
    ctx: &mut EdgionHttpContext
) -> pingora_core::Result<bool> {
    let req_header = session.req_header();

    // Extract client_addr (TCP connection address)
    let client_addr_str = session.client_addr().map(|addr| addr.to_string()).unwrap_or_default();
    ctx.request_info.client_addr = client_addr_str.clone();
    
    // Extract remote_addr (real client IP, considering trusted proxies)
    ctx.request_info.remote_addr = if let Some(extractor) = &edgion_http.real_ip_extractor {
        extractor.extract_real_ip(&client_addr_str, req_header)
    } else {
        // No extractor configured, use client_addr IP (without port)
        extract_ip_string(&client_addr_str)
    };

    // Extract hostname from URI (HTTP/2), Host header (HTTP/1.1), or :authority (HTTP/2 fallback)
    // In HTTP/2, Pingora puts the hostname in the URI, not as a separate header
    let hostname = req_header.uri.host()
        .map(|h| h.to_string())
        .or_else(|| req_header.headers.get("host").and_then(|h| h.to_str().ok().map(|s| s.to_string())))
        .or_else(|| req_header.headers.get(":authority").and_then(|h| h.to_str().ok().map(|s| s.to_string())));
    
    if let Some(host) = hostname {
        ctx.request_info.hostname = host;
    }
    
    // Extract request path
    ctx.request_info.path = req_header.uri.path().to_string();

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

    // Extract X-Forwarded-For header (before we append to it)
    ctx.request_info.x_forwarded_for = req_header
        .headers
        .get("X-Forwarded-For")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string());

    // Validate X-Forwarded-For length against security configuration
    if let Some(security_config) = &edgion_http.edgion_gateway_config.spec.security_protect {
        if let Some(ref existing_xff) = ctx.request_info.x_forwarded_for {
            let xff_len = existing_xff.len();
            if xff_len > security_config.x_forwarded_for_limit {
                // XFF header too long, send 400 response directly
                ctx.add_error(EdgionStatus::XffHeaderTooLong);
                end_response_400(session, ctx, &edgion_http.server_header_opts).await?;
                return Ok(true); // Response sent
            }
        }
    }

    // Note: SNI and client certificate validation are now handled at TLS layer
    // - SNI is used during certificate_callback for certificate selection
    // - Client certificate SAN/CN whitelist is validated in configure_mtls
    // This ensures validation happens during TLS handshake before HTTP processing

    // Protocol detection: check for WebSocket, gRPC-Web, and gRPC
    if ctx.request_info.discover_protocol.is_none() {
        // Check for WebSocket
        if let Some(upgrade) = req_header.headers.get("upgrade") {
            if let Ok(upgrade_str) = upgrade.to_str() {
                if upgrade_str.eq_ignore_ascii_case("websocket") {
                    ctx.request_info.discover_protocol = Some("websocket".to_string());
                }
            }
        }
        
        // Check for gRPC/gRPC-Web (if not WebSocket)
        if ctx.request_info.discover_protocol.is_none() {
            if let Some(ct) = req_header.headers.get("content-type") {
                if let Ok(ct_str) = ct.to_str() {
                    if ct_str.starts_with("application/grpc-web") {
                        ctx.request_info.discover_protocol = Some("grpc-web".to_string());
                    } else if ct_str.starts_with("application/grpc") {
                        ctx.request_info.discover_protocol = Some("grpc".to_string());
                    }
                }
            }
        }
    }
    
    Ok(false) // Continue processing
}

/// Initialize backend context if not yet initialized (inline for performance)
/// This function handles both gRPC and HTTP backends
#[inline]
fn init_backend_context_if_needed(ctx: &mut EdgionHttpContext) -> pingora_core::Result<()> {
    if ctx.backend_context.is_some() {
        return Ok(()); // Already initialized
    }
    
    // Get namespace from selected backend (gRPC or HTTP)
    let (name, namespace) = if let Some(grpc_br) = ctx.selected_grpc_backend.as_ref() {
        let ns = grpc_br.namespace.clone().unwrap_or_else(|| {
            ctx.grpc_route_unit.as_ref()
                .map(|unit| unit.matched_info.rns.clone())
                .unwrap_or_default()
        });
        (grpc_br.name.clone(), ns)
    } else if let Some(http_br) = ctx.selected_backend.as_ref() {
        let ns = http_br.namespace.clone().unwrap_or_else(|| {
            ctx.route_unit.as_ref()
                .map(|unit| unit.matched_info.rns.clone())
                .unwrap_or_default()
        });
        (http_br.name.clone(), ns)
    } else {
        return Err(PingoraError::new(ErrorType::InternalError));
    };
    
    ctx.init_backend_context(name, namespace);
    Ok(())
}

/// Append client IP to X-Forwarded-For header (inline for performance)
///
/// This function always appends the client IP to maintain the proxy chain,
/// regardless of trusted proxy configuration.
#[inline]
fn append_x_forwarded_for(
    session: &mut Session, 
    ctx: &EdgionHttpContext
) {
    let client_ip = extract_ip_string(&ctx.request_info.client_addr);
    
    // Append client IP to X-Forwarded-For (using pre-extracted value)
    let req_header_mut = session.req_header_mut();
    if let Some(ref existing_xff) = ctx.request_info.x_forwarded_for {
        // X-Forwarded-For exists, append client IP
        let new_xff = format!("{}, {}", existing_xff, client_ip);
        let _ = req_header_mut.insert_header("X-Forwarded-For", &new_xff);
    } else {
        // X-Forwarded-For doesn't exist, create new
        let _ = req_header_mut.insert_header("X-Forwarded-For", &client_ip);
    }
}

/// Set X-Real-IP header with extracted remote_addr (inline for performance)
///
/// This header contains the real client IP address after trusted proxy extraction.
#[inline]
fn set_x_real_ip(session: &mut Session, ctx: &EdgionHttpContext) {
    let req_header_mut = session.req_header_mut();
    let _ = req_header_mut.insert_header("X-Real-IP", &ctx.request_info.remote_addr);
}

#[async_trait]
impl ProxyHttp for EdgionHttp {
    type CTX = EdgionHttpContext;

    fn new_ctx(&self) -> Self::CTX {
        let ctx = EdgionHttpContext::new();
        global_metrics().ctx_created();
        ctx
    }

    async fn upstream_peer(&self, session: &mut Session, ctx: &mut Self::CTX) -> pingora_core::Result<Box<HttpPeer>> {
        // Route to appropriate handler based on matched route type (not protocol)
        if ctx.is_grpc_route {
            self.upstream_peer_grpc(session, ctx).await
        } else {
            self.upstream_peer_http(session, ctx).await
        }
    }

    fn init_downstream_modules(&self, modules: &mut HttpModules) {
        // Configure downstream compression based on global config (default: disabled)
        let enable_compression = self.edgion_gateway_config.spec.server.as_ref()
            .map(|s| s.enable_compression)
            .unwrap_or(false);
        
        if !enable_compression {
            // Explicitly disable compression
            modules.add_module(ResponseCompressionBuilder::enable(0));
        }
        
        // Only add GrpcWeb module if HTTP/2 is enabled
        // gRPC-Web requires HTTP/2 support
        if self.enable_http2 {
            modules.add_module(Box::new(GrpcWeb));
            tracing::info!(gateway=%self.gateway_name, listener=%self.listener.name, "GrpcWeb module enabled");
        }
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> pingora_core::Result<bool>
    where
        Self::CTX: Send + Sync,
    {
        // Build request metadata (addresses, hostname, path, trace_id, protocol) and validate XFF length
        if build_request_metadata(self, session, ctx).await? {
            return Ok(true); // Response already sent (XFF too long)
        }
        
        // Validate hostname is present
        if ctx.request_info.hostname.is_empty() {
            ctx.add_error(EdgionStatus::HostMissing);
            end_response_400(session, ctx, &self.server_header_opts).await?;
            return Ok(true);
        }

        // Step 1: Route matching - try gRPC first if applicable, then HTTP
        let is_grpc_request = ctx.request_info.discover_protocol.as_deref() == Some("grpc") 
            || ctx.request_info.discover_protocol.as_deref() == Some("grpc-web");
        
        if is_grpc_request {
            // Check if HTTP/2 is enabled for gRPC
            if !self.enable_http2 {
                ctx.add_error(EdgionStatus::Http2Required);
                end_response_500(session, ctx, &self.server_header_opts).await?;
                return Ok(true);
            }
            
            // Try to match gRPC route
            match try_match_grpc_route(&self.grpc_routes, session, ctx).await {
                Ok(true) => {
                    // gRPC route matched - mark as gRPC route handling
                    ctx.is_grpc_route = true;
                }
                Ok(false) | Err(_) => {
                    // gRPC route not matched, fallback to HTTP route matching
                    // Note: ctx.grpc_route_unit remains None, is_grpc_route remains false
                }
            }
        }
        
        // If no gRPC route matched, try HTTP route
        if ctx.grpc_route_unit.is_none() {
            match self.domain_routes.match_route(&ctx.request_info.hostname, session) {
                Ok(route_unit) => {
                    ctx.route_unit = Some(route_unit.clone());
                    tracing::debug!(
                        matched_info = %route_unit.matched_info,
                        "HTTP route matched"
                    );
                }
                Err(_) => {
                    // No route found at all
                    ctx.add_error(EdgionStatus::RouteNotFound);
                    end_response_404(session, ctx, &self.server_header_opts).await?;
                    return Ok(true);
                }
            }
        }
        
        // Step 2: Run global plugins from EdgionGatewayConfig (executed before route-level plugins)
        if let Some(ref plugin_refs) = self.edgion_gateway_config.spec.global_plugins_ref {
            for plugin_ref in plugin_refs {
                // Construct plugin key: namespace/name
                let plugin_key = format!("{}/{}", 
                    plugin_ref.namespace.as_deref().unwrap_or("default"),
                    plugin_ref.name
                );
                
                // Get plugin from global store
                if let Some(edgion_plugin) = crate::core::plugins::edgion_plugins::get_global_plugin_store().get(&plugin_key) {
                    // Execute plugin runtime
                    edgion_plugin.spec.plugin_runtime.run_request_plugins(session, ctx).await;
                    
                    // Check if plugin terminated the request
                    if ctx.plugin_running_result == PluginRunningResult::ErrTerminateRequest {
                        tracing::debug!(plugin=%plugin_key, "Request terminated by global plugin");
                        return Ok(true);
                    }
                } else {
                    tracing::warn!(plugin=%plugin_key, "Global plugin not found in store");
                }
            }
        }
        
        // Step 3: Run route-level plugins based on matched route type
        if ctx.is_grpc_route {
            // Run gRPC route plugins
            match run_grpc_route_plugins(session, ctx).await {
                Ok(true) => return Ok(true), // Plugin terminated request
                Ok(false) => return Ok(false), // Continue processing
                Err(e) => {
                    tracing::error!("Error running gRPC route plugins: {:?}", e);
                    ctx.add_error(EdgionStatus::Unknown);
                    end_response_500(session, ctx, &self.server_header_opts).await?;
                    return Ok(true);
                }
            }
        } else if let Some(route_unit) = ctx.route_unit.clone() {
            // Run HTTP route plugins
            route_unit.rule.plugin_runtime.run_request_plugins(session, ctx).await;
            if ctx.plugin_running_result == PluginRunningResult::ErrTerminateRequest {
                return Ok(true);
            }
        }
        
        // Set X-Real-IP header with extracted remote_addr
        set_x_real_ip(session, ctx);
        
        // Append client_addr IP to X-Forwarded-For header
        // This is done after all plugin processing but before forwarding to upstream
        append_x_forwarded_for(session, ctx);
        
        Ok(false)
    }

    async fn early_request_filter(&self, session: &mut Session, _ctx: &mut Self::CTX) -> pingora_core::Result<()>
    where
        Self::CTX: Send + Sync,
    {
        // Set client timeouts from pre-parsed config (no runtime overhead)
        let client_timeout = &self.parsed_timeouts.client;
        
        // Set read timeout (pre-parsed, no runtime overhead)
        session.set_read_timeout(Some(client_timeout.read_timeout));
        
        // Set write timeout (pre-parsed, no runtime overhead)
        session.set_write_timeout(Some(client_timeout.write_timeout));
        
        // Set keepalive timeout (pre-parsed, no runtime overhead)
        session.set_keepalive(Some(client_timeout.keepalive_timeout));

        Ok(())
    }

    /// upstream_response_filter - sync hook
    fn upstream_response_filter(
        &self,
        session: &mut Session,
        upstream_response: &mut ResponseHeader,
        ctx: &mut Self::CTX,
    ) -> pingora_core::Result<()> {
        // Record status code
        let status_code = upstream_response.status.as_u16();
        ctx.request_info.status = status_code;
        
        // Apply custom server headers (including Server header)
        self.server_header_opts.apply_to_response(upstream_response);
        
        // Record header time (time from this upstream's start_time to receiving response header)
        if let Some(upstream) = ctx.get_current_upstream_mut() {
            let ht = upstream.start_time.elapsed().as_millis() as u64;
            upstream.ht = Some(ht);
            upstream.status = Some(status_code);
        }
        
        // Run rule-level upstream_response edgion_plugins (sync)
        if let Some(route_unit) = ctx.route_unit.clone() {
            route_unit.rule.plugin_runtime.run_upstream_response_plugins_sync(session, ctx, upstream_response);
        }

        // Run backend-level upstream_response edgion_plugins (sync)
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
        // Apply custom server headers (including Server header)
        // This catches all responses, including framework-generated error responses (e.g., 502)
        self.server_header_opts.apply_to_response(upstream_response);
        
        // Add x-trace-id to response headers
        if let Some(trace_id) = &ctx.request_info.x_trace_id {
            let _ = upstream_response.insert_header("x-trace-id", trace_id.as_str());
        }
        
        // Run rule-level response edgion_plugins (async)
        if let Some(route_unit) = ctx.route_unit.clone() {
            route_unit.rule.plugin_runtime.run_upstream_response_plugins_async(session, ctx, upstream_response).await;
        }

        // Run backend-level response edgion_plugins (async)
        if let Some(backend) = ctx.selected_backend.clone() {
            backend.plugin_runtime.run_upstream_response_plugins_async(session, ctx, upstream_response).await;
        }

        Ok(())
    }

    /// upstream_response_body_filter - called when receiving body chunks from upstream
    fn upstream_response_body_filter(
        &self,
        _session: &mut Session,
        _body: &mut Option<bytes::Bytes>,
        _end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> pingora_core::Result<()> {
        // Record body time (time from start_time to receiving first body chunk)
        // Only set once when bt is None (first chunk)
        if let Some(upstream) = ctx.get_current_upstream_mut() {
            if upstream.bt.is_none() {
                let bt = upstream.start_time.elapsed().as_millis() as u64;
                upstream.bt = Some(bt);
            }
        }
        Ok(())
    }

    async fn logging(&self, session: &mut Session, _e: Option<&PingoraError>, ctx: &mut Self::CTX)
    where
        Self::CTX: Send + Sync,
    {
        // Update LB metrics based on policy type
        if let Some(upstream) = ctx.get_current_upstream() {
            if let Some(addr) = &upstream.backend_addr {
                match &upstream.lb_policy {
                    Some(crate::types::ParsedLBPolicy::LeastConn) => {
                        // Decrement connection count for LeastConnection LB
                        crate::core::lb::leastconn::decrement(addr);
                    }
                    Some(crate::types::ParsedLBPolicy::Ewma) => {
                        // Update EWMA with response latency
                        let latency_us = upstream.start_time.elapsed().as_micros() as u64;
                        crate::core::lb::ewma::update(addr, latency_us);
                    }
                    _ => {}
                }
            }
        }
        
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

        // Send to access logger
        self.access_logger.send(entry.to_json()).await;
    }

    fn error_while_proxy(
        &self,
        peer: &HttpPeer,
        session: &mut Session,
        e: Box<Error>,
        _ctx: &mut Self::CTX,
        client_reused: bool,
    ) -> Box<Error> {
        let mut e = e.more_context(format!("Peer: {}", peer));
        // only reused client connections where retry buffer is not truncated
        e.retry
            .decide_reuse(client_reused && !session.as_ref().retry_buffer_truncated());
        // todo need add retry logic?
        e
    }


    /// fail_to_connect - called when connection to upstream fails
    fn fail_to_connect(
        &self,
        _session: &mut Session,
        _peer: &HttpPeer,
        ctx: &mut Self::CTX,
        mut e: Box<pingora_core::Error>,
    ) -> Box<pingora_core::Error> {
        // Set status code based on error type: 504 for timeout, 503 for other errors
        if let Some(upstream) = ctx.get_current_upstream_mut() {
            let status_code = match e.etype() {
                ErrorType::ConnectTimedout | ErrorType::TLSHandshakeTimedout => 504,
                _ => 503,
            };
            upstream.status = Some(status_code);
            // Only add error message for non-timeout errors (503)
            if status_code != 504 {
                upstream.err.push(e.etype().as_str().to_string());
            }
            upstream.et = Some(upstream.start_time.elapsed().as_millis() as u64);
        }

        // Determine max_retries: route annotation > global config
        let max_retries = ctx.route_unit.as_ref()
            .and_then(|unit| unit.rule.parsed_max_retries)
            .unwrap_or(self.edgion_gateway_config.spec.max_retries);
        
        if ctx.try_cnt <= max_retries {
            e.set_retry(true);
        }

        e
    }


    async fn fail_to_proxy(
        &self,
        session: &mut Session,
        e: &Error,
        ctx: &mut Self::CTX,
    ) -> FailToProxy
    where
        Self::CTX: Send + Sync,
    {
        let code = match e.etype() {
            HTTPStatus(code) => *code,
            // Check for timeout errors - distinguish between client and backend timeouts
            ErrorType::ReadTimedout | ErrorType::WriteTimedout => {
                match e.esource() {
                    ErrorSource::Downstream => 499,  // Client closed request (client timeout)
                    ErrorSource::Upstream => 504,     // Gateway timeout (backend timeout)
                    _ => 504,  // Default to 504 for other timeout sources
                }
            }
            ErrorType::ConnectTimedout | ErrorType::TLSHandshakeTimedout => 504,  // Backend connection timeouts
            _ => {
                match e.esource() {
                    ErrorSource::Upstream => 502,
                    ErrorSource::Downstream => {
                        match e.etype() {
                            WriteError | ReadError | ConnectionClosed => {
                                // Client closed connection - return 499 for logging, but don't send response
                                499
                            }
                            _ => 400,
                        }
                    }
                    ErrorSource::Internal | ErrorSource::Unset => 500,
                }
            }
        };

        // Record error status code
        if code > 0 {
            // Only update request_info.status if not already set (default is 0)
            if ctx.request_info.status == 0 {
                ctx.request_info.status = code as u16;
            }

            // Always update current upstream status and error message
            if let Some(upstream) = ctx.get_current_upstream_mut() {
                upstream.status = Some(code as u16);
                // Only add error message for non-timeout errors (not 504/499)
                if code != 504 && code != 499 {
                    upstream.err.push(e.etype().as_str().to_string());
                }
            }
        }

        // Don't send error response if connection is already closed (499)
        // For 499, the client has already disconnected, so we can't send a response
        if code > 0 && code != 499 {
            // Generate error response and apply custom server headers
            let mut resp = pingora_core::protocols::http::ServerSession::generate_error(code);
            self.server_header_opts.apply_to_response(&mut resp);

            // Write error response
            session.write_error_response(resp, bytes::Bytes::default()).await.unwrap_or_else(|e| {
                error!("failed to send error response to downstream: {e}");
            });
        }

        FailToProxy {
            error_code: code,
            // default to no reuse, which is safest
            can_reuse_downstream: false,
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
        // Record connection time (time from start_time to connection established)
        if let Some(upstream) = ctx.get_current_upstream_mut() {
            let ct = upstream.start_time.elapsed().as_millis() as u64;
            upstream.ct = Some(ct);
            
            // Increment connection count for LeastConnection LB
            if let (Some(addr), Some(crate::types::ParsedLBPolicy::LeastConn)) = 
                (&upstream.backend_addr, &upstream.lb_policy) 
            {
                crate::core::lb::leastconn::increment(addr);
            }
        }
        
        // For gRPC-Web or WebSocket, log connection establishment immediately
        if let Some(protocol) = &ctx.request_info.discover_protocol {
            if protocol == "grpc-web" || protocol == "websocket" {
                let mut entry = AccessLogEntry::from_context(ctx);
                entry.set_conn_est();
                
                // Send to access logger
                self.access_logger.send(entry.to_json()).await;
            }
        }
        
        Ok(())
    }
}

/// Additional helper methods for EdgionHttp (separate from ProxyHttp trait)
impl EdgionHttp {
    /// Handle gRPC upstream peer selection
    async fn upstream_peer_grpc(&self, session: &mut Session, ctx: &mut EdgionHttpContext) -> pingora_core::Result<Box<HttpPeer>> {
        // 1. Handle gRPC upstream selection
        match handle_grpc_upstream(session, ctx).await {
            Ok(Some(())) => {
                tracing::debug!("gRPC backend selected");
            }
            Ok(None) => {
                // No gRPC route found - this shouldn't happen as route matching
                // should be done in request_filter stage
                tracing::error!("No gRPC route found at upstream_peer stage");
                ctx.add_error(EdgionStatus::GrpcUpstreamNotRouteMatched);
                end_response_500(session, ctx, &self.server_header_opts).await?;
                return Err(PingoraError::new(ErrorType::InternalError));
            }
            Err(e) => {
                tracing::error!("Failed to handle gRPC upstream: {:?}", e);
                ctx.add_error(EdgionStatus::GrpcUpstreamNotBackendRefs);
                end_response_500(session, ctx, &self.server_header_opts).await?;
                return Err(PingoraError::new(ErrorType::InternalError));
            }
        }
        
        // 2. Initialize GrpcWebBridge for gRPC-Web requests
        // Standard gRPC requests don't need protocol conversion
        if ctx.request_info.discover_protocol.as_deref() == Some("grpc-web") {
            if let Some(grpc) = session.downstream_modules_ctx.get_mut::<GrpcWebBridge>() {
                grpc.init();
            }
        }
        
        // 3. Initialize backend context (unified logic)
        init_backend_context_if_needed(ctx)?;
        
        // 4. Get peer from gRPC backend
        let mut peer = get_peer(session, ctx, true).await?;
        
        // 5. Force HTTP/2 for gRPC
        peer.options.set_http_version(2, 2);
        
        // 6. Configure peer (shared logic)
        self.configure_peer_timeouts(&mut peer, ctx);
        self.update_peer_metrics(&mut peer, ctx);
        
        Ok(peer)
    }

    /// Handle HTTP upstream peer selection
    async fn upstream_peer_http(&self, session: &mut Session, ctx: &mut EdgionHttpContext) -> pingora_core::Result<Box<HttpPeer>> {
        // 1. Select HTTP backend if not already selected
        if ctx.selected_backend.is_none() && ctx.selected_grpc_backend.is_none() {
            self.select_http_backend(session, ctx).await?;
        }
        
        // 2. Initialize backend context (unified logic)
        init_backend_context_if_needed(ctx)?;
        
        // 3. Get peer
        let mut peer = get_peer(session, ctx, false).await?;
        
        // 4. Configure peer (shared logic)
        self.configure_peer_timeouts(&mut peer, ctx);
        self.update_peer_metrics(&mut peer, ctx);
        
        Ok(peer)
    }

    /// Select HTTP backend from route (extracted from upstream_peer)
    async fn select_http_backend(&self, session: &mut Session, ctx: &mut EdgionHttpContext) -> pingora_core::Result<()> {
        let route_unit = match ctx.route_unit.as_ref() {
            Some(unit) => unit,
            None => {
                ctx.add_error(EdgionStatus::UpstreamNotRouteMatched);
                end_response_500(session, ctx, &self.server_header_opts).await?;
                return Err(PingoraError::new(ErrorType::InternalError));
            }
        };

        let backend_ref = match RouteRules::select_backend(&route_unit.rule) {
            Ok(backend) => backend,
            Err(e) => {
                tracing::error!("Failed to select backend: {:?}", e);
                ctx.add_error(match e {
                    EdError::BackendNotFound() => EdgionStatus::UpstreamNotBackendRefs,
                    EdError::InconsistentWeight() => EdgionStatus::UpstreamInconsistentWeight,
                    _ => EdgionStatus::Unknown,
                });
                end_response_500(session, ctx, &self.server_header_opts).await?;
                return Err(PingoraError::new(ErrorType::InternalError));
            }
        };
        
        tracing::info!("Selected HTTP backend: {:?}", backend_ref);
        
        // Run backend-level request edgion_plugins
        backend_ref.plugin_runtime.run_request_plugins(session, ctx).await;
        if ctx.plugin_running_result == PluginRunningResult::ErrTerminateRequest {
            ctx.add_error(EdgionStatus::Unknown);
            end_response_500(session, ctx, &self.server_header_opts).await?;
            return Err(PingoraError::new(ErrorType::InternalError));
        }
        
        ctx.selected_backend = Some(backend_ref);
        Ok(())
    }

    /// Configure peer timeouts from global and route-level configs (inline for performance)
    #[inline]
    fn configure_peer_timeouts(&self, peer: &mut Box<HttpPeer>, ctx: &EdgionHttpContext) {
        let backend_timeout = &self.parsed_timeouts.backend;
        let route_timeouts = ctx.route_unit.as_ref()
            .and_then(|unit| unit.rule.parsed_timeouts.as_ref())
            .or_else(|| {
                ctx.grpc_route_unit.as_ref()
                    .and_then(|unit| unit.rule.parsed_timeouts.as_ref())
            });
        
        // Connection timeout: route-level backend_request_timeout overrides global connect_timeout
        peer.options.connection_timeout = Some(
            route_timeouts
                .and_then(|rt| rt.backend_request_timeout)
                .unwrap_or(backend_timeout.connect_timeout)
        );
        
        // Read/Write timeout: route-level overrides global
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

    /// Update peer address info and metrics (inline for performance)
    #[inline]
    fn update_peer_metrics(&self, peer: &Box<HttpPeer>, ctx: &mut EdgionHttpContext) {
        // Increment try count
        ctx.try_cnt += 1;
        
        // Extract and push upstream info
        let (ip, port) = peer.address().as_inet()
            .map(|addr| (Some(addr.ip().to_string()), Some(addr.port())))
            .unwrap_or((None, None));
        ctx.push_upstream(ip, port);
        
        // Set upstream start time on first try
        if ctx.upstream_start_time.is_none() {
            ctx.upstream_start_time = Some(std::time::Instant::now());
        }
    }
} 