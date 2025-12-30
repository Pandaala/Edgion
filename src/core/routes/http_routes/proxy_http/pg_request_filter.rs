use pingora_proxy::Session;
use crate::types::{EdgionHttpContext, EdgionStatus};
use crate::types::filters::PluginRunningResult;
use crate::core::gateway::{end_response_400, end_response_404, end_response_500};
use crate::core::plugins::edgion_plugins::get_global_plugin_store;
use crate::core::routes::grpc_routes::try_match_grpc_route;
use super::EdgionHttp;

#[inline]
pub async fn request_filter(
    edgion_http: &EdgionHttp,
    session: &mut Session,
    ctx: &mut EdgionHttpContext
) -> pingora_core::Result<bool> {
    // Build request metadata (addresses, hostname, path, trace_id, protocol)
    // Validates XFF length and hostname presence, returns true if response sent
    if build_request_metadata(edgion_http, session, ctx).await? {
        return Ok(true); // Response already sent (XFF too long or hostname missing)
    }

    // Step 1: Route matching - try gRPC first if applicable, then HTTP
    if ctx.request_info.is_grpc_request {
        // Only pure gRPC requires HTTP/2, gRPC-Web can work on HTTP/1.1
        if ctx.request_info.discover_protocol.as_deref() == Some("grpc") {
            if !edgion_http.enable_http2 {
                ctx.add_error(EdgionStatus::Http2Required);
                end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
                return Ok(true);
            }
        }
        
        // Try to match gRPC route (sets ctx.is_grpc_route_matched internally if matched)
        let _ = try_match_grpc_route(&edgion_http.grpc_routes, session, ctx, &edgion_http.listener.name).await;
    }
    
    // HTTP route Match, if grpc route already matched, skip here
    if !ctx.is_grpc_route_matched {
        match edgion_http.domain_routes.match_route(&ctx.request_info.hostname, session, &edgion_http.listener.name) {
            Ok(route_unit) => {
                ctx.route_unit = Some(route_unit.clone());
            }
            Err(_) => {
                ctx.add_error(EdgionStatus::RouteNotFound);
                end_response_404(session, ctx, &edgion_http.server_header_opts).await?;
                return Ok(true);
            }
        }
    }
    
    // Step 2: Run global plugins from EdgionGatewayConfig (executed before route-level plugins)
    if let Some(ref plugin_refs) = edgion_http.edgion_gateway_config.spec.global_plugins_ref {
        for plugin_ref in plugin_refs {
            // Construct plugin key: namespace/name
            let plugin_key = format!("{}/{}", 
                plugin_ref.namespace.as_deref().unwrap_or("default"),
                plugin_ref.name
            );
            
            // Get plugin from global store
            if let Some(edgion_plugin) = get_global_plugin_store().get(&plugin_key) {
                edgion_plugin.spec.plugin_runtime.run_request_plugins(session, ctx).await;
                if ctx.plugin_running_result == PluginRunningResult::ErrTerminateRequest {
                    return Ok(true);
                }
            }
        }
    }
    
    // Step 3: Run route-level plugins based on matched route type
    if let Some(grpc_route_unit) = ctx.grpc_route_unit.clone() {
        // Run gRPC route plugins (inline for consistency with HTTP route handling)
        grpc_route_unit.rule.plugin_runtime.run_request_plugins(session, ctx).await;
        if ctx.plugin_running_result == PluginRunningResult::ErrTerminateRequest {
            return Ok(true);
        }
    } else if let Some(route_unit) = ctx.route_unit.clone() {
        // Run HTTP route plugins
        route_unit.rule.plugin_runtime.run_request_plugins(session, ctx).await;
        if ctx.plugin_running_result == PluginRunningResult::ErrTerminateRequest {
            return Ok(true);
        }
    }
    
    set_x_real_ip(session, ctx);
    append_x_forwarded_for(session, ctx);
    
    Ok(false)
}

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
                        ctx.request_info.is_grpc_request = true;
                    } else if ct_str.starts_with("application/grpc") {
                        ctx.request_info.discover_protocol = Some("grpc".to_string());
                        ctx.request_info.is_grpc_request = true;
                    }
                }
            }
        }
    }

    // Extract client_addr and client_port (TCP connection address)
    let client_addr_str = session.client_addr().map(|addr| addr.to_string()).unwrap_or_default();
    let parsed_addr = client_addr_str.parse::<std::net::SocketAddr>().ok();
    ctx.request_info.client_addr = parsed_addr
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|| client_addr_str.clone());
    ctx.request_info.client_port = parsed_addr.map(|a| a.port()).unwrap_or(0);
    
    // Extract remote_addr (real client IP, considering trusted proxies)
    ctx.request_info.remote_addr = if let Some(extractor) = &edgion_http.real_ip_extractor {
        extractor.extract_real_ip(&client_addr_str, req_header)
    } else {
        // No extractor configured, use client_addr (already IP only)
        ctx.request_info.client_addr.clone()
    };

    // Extract hostname from URI (HTTP/2), Host header (HTTP/1.1), or :authority (HTTP/2 fallback)
    // In HTTP/2, Pingora puts the hostname in the URI, not as a separate header
    ctx.request_info.hostname = req_header.uri.host()
        .map(|h| h.to_string())
        .or_else(|| req_header.headers.get("host").and_then(|h| h.to_str().ok().map(|s| s.to_string())))
        .or_else(|| req_header.headers.get(":authority").and_then(|h| h.to_str().ok().map(|s| s.to_string())))
        .unwrap_or_default();
    
    // Validate hostname is present (not required for gRPC requests)
    if !ctx.request_info.is_grpc_request && ctx.request_info.hostname.is_empty() {
        ctx.add_error(EdgionStatus::HostMissing);
        end_response_400(session, ctx, &edgion_http.server_header_opts).await?;
        return Ok(true); // Response sent
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
    
    Ok(false) // Continue processing
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
    // client_addr is already IP only (without port)
    let client_ip = &ctx.request_info.client_addr;
    
    // Append client IP to X-Forwarded-For (using pre-extracted value)
    let req_header_mut = session.req_header_mut();
    if let Some(ref existing_xff) = ctx.request_info.x_forwarded_for {
        // X-Forwarded-For exists, append client IP
        let new_xff = format!("{}, {}", existing_xff, client_ip);
        let _ = req_header_mut.insert_header("X-Forwarded-For", &new_xff);
    } else {
        // X-Forwarded-For doesn't exist, create new
        let _ = req_header_mut.insert_header("X-Forwarded-For", client_ip);
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

