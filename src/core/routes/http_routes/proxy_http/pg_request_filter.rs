use super::EdgionHttp;
use crate::core::gateway::{end_response_400, end_response_404, end_response_421, end_response_500};
use crate::core::plugins::edgion_plugins::get_global_plugin_store;
use crate::core::routes::grpc_routes::get_global_grpc_route_manager;
use crate::core::routes::grpc_routes::try_match_grpc_route;
use crate::core::routes::http_routes::routes_mgr::get_global_route_manager;
use crate::types::filters::PluginRunningResult;
use crate::types::resources::{CorsConfig, EdgionPlugin, HTTPRouteFilter, HTTPRouteFilterType};
use crate::types::{EdgionHttpContext, EdgionStatus, TlsConnId, TlsConnMeta};
use pingora_proxy::Session;
use std::sync::Arc;

#[inline]
pub async fn request_filter(
    edgion_http: &EdgionHttp,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<bool> {
    // Build request metadata (addresses, hostname, path, trace_id, protocol)
    // Validates XFF length and hostname presence, returns true if response sent
    if build_request_metadata(edgion_http, session, ctx).await? {
        return Ok(true); // Response already sent (XFF too long or hostname missing)
    }

    // Step 1: Route matching — single lookup against the global route table.
    // `gateway_infos` carries all Gateway/Listener contexts for this port;
    // deep_match inside the route table validates which gateway each route belongs to.
    let gateway_infos = &edgion_http.gateway_infos;

    if ctx.request_info.is_grpc_request {
        if ctx.request_info.discover_protocol.as_deref() == Some("grpc") && !edgion_http.enable_http2 {
            ctx.add_error(EdgionStatus::Http2Required);
            end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
            return Ok(true);
        }

        let grpc_route_manager = get_global_grpc_route_manager();
        let grpc_routes = grpc_route_manager.get_global_grpc_routes();
        let _ = try_match_grpc_route(&grpc_routes, session, ctx, gateway_infos).await;
    }

    if !ctx.is_grpc_route_matched {
        let route_manager = get_global_route_manager();
        let global_routes = route_manager.get_global_routes();
        match global_routes.match_route(session, ctx, gateway_infos) {
            Ok(result) => {
                ctx.gateway_info = result.matched_gateway;
                ctx.route_unit = Some(result.route_unit);
            }
            Err(_) => {
                ctx.add_error(EdgionStatus::RouteNotFound);
                end_response_404(session, ctx, &edgion_http.server_header_opts).await?;
                return Ok(true);
            }
        }
    }

    // Step 1.5: Handle preflight requests (before plugin execution)
    // Check if this is a preflight request and handle it early
    if edgion_http.preflight_handler.is_preflight(session) {
        // Get CORS config dynamically from matched route's EdgionPlugins
        // This ensures we always get the latest config even if EdgionPlugin is updated
        let cors_config = if let Some(ref route_unit) = ctx.route_unit {
            let namespace = &route_unit.matched_info.rns;
            extract_cors_config_from_route_filters(&route_unit.rule.filters, namespace)
        } else if let Some(_grpc_route_unit) = &ctx.grpc_route_unit {
            // gRPC routes don't have CORS config yet, so None
            None
        } else {
            None
        };

        // Handle preflight request
        match edgion_http
            .preflight_handler
            .handle_preflight(session, ctx, cors_config.as_ref())
            .await
        {
            Ok(true) => {
                // Preflight handled, terminate request
                tracing::debug!("Preflight request handled, terminating");
                return Ok(true);
            }
            Ok(false) => {
                // Continue to plugin chain (shouldn't happen with current implementation)
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to handle preflight request");
                end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
                return Ok(true);
            }
        }
    }

    // Step 2: Run global plugins from EdgionGatewayConfig (executed before route-level plugins)
    if let Some(ref plugin_refs) = edgion_http.edgion_gateway_config.spec.global_plugins_ref {
        for plugin_ref in plugin_refs {
            // Construct plugin key: namespace/name
            let plugin_key = format!(
                "{}/{}",
                plugin_ref.namespace.as_deref().unwrap_or("default"),
                plugin_ref.name
            );

            // Get plugin from global store
            if let Some(edgion_plugin) = get_global_plugin_store().get(&plugin_key) {
                edgion_plugin
                    .spec
                    .plugin_runtime
                    .run_request_plugins(session, ctx)
                    .await;
                if ctx.plugin_running_result == PluginRunningResult::ErrTerminateRequest {
                    return Ok(true);
                }
            }
        }
    }

    // Step 3: Run route-level plugins based on matched route type
    if let Some(grpc_route_unit) = ctx.grpc_route_unit.clone() {
        // Run gRPC route plugins (inline for consistency with HTTP route handling)
        grpc_route_unit
            .rule
            .plugin_runtime
            .run_request_plugins(session, ctx)
            .await;
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
    ctx: &mut EdgionHttpContext,
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
    // uri.host() returns host without port; Host header may include port so we strip it.
    ctx.request_info.hostname = req_header
        .uri
        .host()
        .map(normalize_host_for_matching)
        .or_else(|| {
            req_header
                .headers
                .get("host")
                .and_then(|h| h.to_str().ok().map(normalize_host_for_matching))
        })
        .or_else(|| {
            req_header
                .headers
                .get(":authority")
                .and_then(|h| h.to_str().ok().map(normalize_host_for_matching))
        })
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

    // Extract TLS connection id (for correlating tls.log and access.log)
    if let Some(digest) = session.as_downstream().digest() {
        if let Some(ssl_digest) = digest.ssl_digest.as_ref() {
            if let Some(meta) = ssl_digest.extension.get::<TlsConnMeta>() {
                ctx.request_info.tls_id = Some(meta.tls_id);
                ctx.request_info.sni = meta.sni.clone();
                ctx.request_info.client_cert_info = meta.client_cert_info.clone();
            } else if let Some(tls_id) = ssl_digest.extension.get::<TlsConnId>() {
                // Backward compatibility for connections created before metadata upgrade.
                ctx.request_info.tls_id = Some(tls_id.0);
            }
        }
    }

    // HTTPS listener isolation:
    // 1) enforce SNI and Host consistency when enabled
    // 2) reject requests whose Host does not match listener hostname constraint
    if should_enforce_listener_isolation(edgion_http) {
        if is_sni_host_mismatch(ctx.request_info.sni.as_deref(), &ctx.request_info.hostname) {
            ctx.add_error(EdgionStatus::SniHostMismatch);
            end_response_421(session, ctx, &edgion_http.server_header_opts).await?;
            return Ok(true);
        }

        if listener_hostname_mismatch(edgion_http.listener.hostname.as_deref(), &ctx.request_info.hostname) {
            ctx.add_error(EdgionStatus::SniHostMismatch);
            end_response_421(session, ctx, &edgion_http.server_header_opts).await?;
            return Ok(true);
        }
    }

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

#[inline]
fn should_enforce_listener_isolation(edgion_http: &EdgionHttp) -> bool {
    let require_sni_host_match = edgion_http
        .edgion_gateway_config
        .spec
        .security_protect
        .as_ref()
        .map(|s| s.require_sni_host_match)
        .unwrap_or(true);

    require_sni_host_match && edgion_http.listener.protocol.eq_ignore_ascii_case("HTTPS")
}

#[inline]
fn listener_hostname_mismatch(listener_hostname: Option<&str>, host: &str) -> bool {
    let Some(listener_hostname) = listener_hostname else {
        return false;
    };
    normalize_host_for_matching(listener_hostname) != normalize_host_for_matching(host)
}

#[inline]
fn is_sni_host_mismatch(sni: Option<&str>, host: &str) -> bool {
    let Some(sni) = sni else {
        return false;
    };
    normalize_host_for_matching(sni) != normalize_host_for_matching(host)
}

/// Normalize a hostname from Host / :authority header for route matching:
/// 1. Trim whitespace
/// 2. Strip port (handling both IPv4 and IPv6 bracket notation)
/// 3. Remove trailing dot (FQDN normalization)
/// 4. Convert to lowercase (case-insensitive matching per RFC 4343)
#[inline]
fn normalize_host_for_matching(raw: &str) -> String {
    let h = raw.trim();

    let host = if h.starts_with('[') {
        // IPv6 bracket notation: [::1]:port → ::1
        match h.find(']') {
            Some(end) => &h[1..end],
            None => h,
        }
    } else {
        match h.rfind(':') {
            Some(pos) if h[pos + 1..].bytes().all(|b| b.is_ascii_digit()) => &h[..pos],
            _ => h,
        }
    };

    let host = host.strip_suffix('.').unwrap_or(host);
    host.to_ascii_lowercase()
}

/// Append client IP to X-Forwarded-For header (inline for performance)
///
/// This function always appends the client IP to maintain the proxy chain,
/// regardless of trusted proxy configuration.
#[inline]
fn append_x_forwarded_for(session: &mut Session, ctx: &EdgionHttpContext) {
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

/// Extract CORS configuration dynamically from route filters
///
/// This function searches through route filters to find EdgionPlugins references
/// and extracts the CORS configuration from the global plugin store.
/// This ensures we always get the latest CORS config even if EdgionPlugin is updated.
fn extract_cors_config_from_route_filters(
    filters: &Option<Vec<HTTPRouteFilter>>,
    namespace: &str,
) -> Option<Arc<CorsConfig>> {
    let filters = filters.as_ref()?;

    for filter in filters {
        if filter.filter_type != HTTPRouteFilterType::ExtensionRef {
            continue;
        }

        let ext_ref = filter.extension_ref.as_ref()?;

        // Check if it's EdgionPlugins
        if ext_ref.kind != "EdgionPlugins" || (!ext_ref.group.is_empty() && ext_ref.group != "edgion.io") {
            continue;
        }

        // Look up EdgionPlugins in global store with namespace/name key
        let key = format!("{}/{}", namespace, ext_ref.name);
        let store = get_global_plugin_store();

        if let Some(edgion_plugins) = store.get(&key) {
            // Search for CORS plugin in request_plugins
            if let Some(ref entries) = edgion_plugins.spec.request_plugins {
                for entry in entries {
                    if let EdgionPlugin::Cors(cors_config) = &entry.plugin {
                        if entry.is_enabled() {
                            return Some(Arc::new(cors_config.clone()));
                        }
                    }
                }
            }
        }
    }

    None
}
