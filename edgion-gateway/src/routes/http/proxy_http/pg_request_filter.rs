use super::{resolve_gateway_plugins, resolve_global_plugins, resolve_route_ext_refs, EdgionHttpProxy};
use crate::ctx::NormalizedHostname;
use crate::plugins::http::get_global_plugin_store;
use crate::routes::grpc::{parse_grpc_timeout, try_match_grpc_route};
use crate::runtime::hostname_matches_listener;
use crate::runtime::store::get_listener_gateway_info_store;
use crate::runtime::store::listener_gateway_info::format_gateway_key;
use crate::services::acme::challenge_store::get_global_challenge_store;
use crate::util::net::host_without_port;
use crate::{
    end_response_400, end_response_404, end_response_421, end_response_500, end_response_grpc_unimplemented,
    terminate_with_fallback,
};
use crate::{EdgionHttpContext, TlsConnId, TlsConnMeta};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::path_normalization::get_path_normalization_config;
use edgion_resources::path_normalization::{
    contains_escaped_slash, has_invalid_percent_encoding, normalize_path, PathNormalizationConfig,
};
use edgion_resources::resources::{CorsConfig, HTTPRouteFilter, HTTPRouteFilterType};
use edgion_resources::EdgionStatus;
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::Session;
use std::sync::Arc;

/// ACME HTTP-01 challenge path prefix
const ACME_CHALLENGE_PREFIX: &str = "/.well-known/acme-challenge/";

#[inline]
pub async fn request_filter(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<bool> {
    // Build request metadata (addresses, hostname, path, trace_id, protocol)
    // Validates XFF length and hostname presence, returns true if response sent
    if build_request_metadata(edgion_http, session, ctx).await? {
        return Ok(true); // Response already sent (XFF too long or hostname missing)
    }

    // Step 1: Route matching — single lookup against the global route table.
    // Load the pre-built GatewayInfoIndex for O(1) gateway/listener lookups.
    let gateway_index = get_listener_gateway_info_store().get_index(&edgion_http.listener_key);

    // HTTPS listener isolation: 421 Misdirected Request (Gateway API spec).
    // If SNI matched listener A but Host should be served by a different listener → 421.
    // Enabled by global requireSniHostMatch OR per-Gateway annotation edgion.io/require-sni-host-match.
    // Only applies to Gateway TLS (tls_matched_listener is None for HTTP/EdgionTls).
    if edgion_http.listener.protocol.eq_ignore_ascii_case("HTTPS") {
        if let (Some(sni_listener_name), Some(gateway_key)) = (
            ctx.request_info.tls_matched_listener.as_deref(),
            ctx.request_info.tls_matched_gateway_key.as_deref(),
        ) {
            if let Some(gw_listeners) = gateway_index.get_gateway(gateway_key) {
                if let Some(sni_entry) = gw_listeners.get(sni_listener_name) {
                    let global_enabled = edgion_http
                        .edgion_gateway_config
                        .spec
                        .security_protect
                        .as_ref()
                        .map(|s| s.require_sni_host_match)
                        .unwrap_or(true);
                    let gw_annotation_enabled = sni_entry.info.require_sni_host_match.unwrap_or(false);

                    if global_enabled || gw_annotation_enabled {
                        let sni_hostname = sni_entry.hostname.as_deref();
                        let host: &str = &ctx.request_info.hostname;

                        let host_matches_sni_listener = match sni_hostname {
                            Some(h) => hostname_matches_listener(host, h),
                            None => true, // catch-all listener accepts all hostnames
                        };

                        if !host_matches_sni_listener || gw_listeners.has_more_specific(host, sni_hostname) {
                            ctx.add_error(EdgionStatus::SniHostMismatch);
                            end_response_421(session, ctx, &edgion_http.server_header_opts).await?;
                            return Ok(true);
                        }
                    }
                }
            }
        }
    }

    if ctx.request_info.is_grpc_request {
        if ctx.request_info.discover_protocol.as_deref() == Some("grpc") && !edgion_http.enable_http2 {
            ctx.add_error(EdgionStatus::Http2Required);
            end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
            return Ok(true);
        }

        let grpc_routes = edgion_http.grpc_route_manager.load_route_table();
        let _ = try_match_grpc_route(&grpc_routes, session, ctx, &gateway_index).await;

        // If GRPCRoutes exist on this port but none matched, return UNIMPLEMENTED
        // per Gateway API spec. Only fall through to HTTP routes when no GRPCRoutes
        // are configured (user may be routing gRPC via HTTPRoute).
        // Scoped to pure gRPC only — gRPC-web may be routed via HTTPRoute.
        if !ctx.is_grpc_route_matched
            && grpc_routes.has_routes()
            && ctx.request_info.discover_protocol.as_deref() == Some("grpc")
        {
            ctx.add_error(EdgionStatus::RouteNotFound);
            end_response_grpc_unimplemented(session, ctx, &edgion_http.server_header_opts).await?;
            // gRPC trailers-only response (HTTP 200 + grpc-status: 12, END_STREAM=true) already
            // written above. Return Ok(true) to signal response is fully handled.
            // Returning Err causes Pingora to call fail_to_proxy(), overwriting our response.
            return Ok(true);
        }
    }

    if !ctx.is_grpc_route_matched {
        let port_routes = edgion_http.http_route_manager.load_route_table();
        match port_routes.match_route(session, ctx, &gateway_index) {
            Ok(result) => {
                ctx.gateway_info = result.matched_gateway;
                ctx.route_unit = Some(result.route_unit);
            }
            Err(_) => {
                ctx.add_error(EdgionStatus::RouteNotFound);
                end_response_404(session, ctx, &edgion_http.server_header_opts).await?;
                return Ok(true); // Already sent error response, signal completion
            }
        }
    }

    // Step 1.5: Handle preflight requests (before plugin execution)
    if edgion_http.preflight_handler.is_preflight(session) {
        // If route has a native CORS filter (type: CORS), skip this preflight handler.
        // GapiCorsFilter in Step 3 already has full preflight support and will handle it.
        let has_native_cors = ctx
            .route_unit
            .as_ref()
            .and_then(|ru| ru.rule.filters.as_ref())
            .map(|filters| filters.iter().any(|f| f.filter_type == HTTPRouteFilterType::CORS))
            .unwrap_or(false);

        if !has_native_cors {
            // Get CORS config dynamically from matched route's EdgionPlugins
            // This ensures we always get the latest config even if EdgionPlugin is updated
            let cors_config = if let Some(ref route_unit) = ctx.route_unit {
                let namespace = &route_unit.matched_info.rsc.ns;
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
                    return Ok(true);
                }
                Ok(false) => {}
                Err(_) => {
                    end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
                    return Ok(true);
                }
            }
        }
    }

    // Per-request resolve: populate ctx.resolved_* once here; later stages read
    // directly without paying any format!() / store-lookup cost.
    // Fetch the global plugin store once and share &PluginStore across both
    // resolve calls to avoid redundant Arc::clone on the hot path.
    if ctx.resolved_global_plugins.is_none() || ctx.resolved_route_ext_refs.is_none() {
        let plugin_store = get_global_plugin_store();

        if ctx.resolved_global_plugins.is_none() {
            let resolved = resolve_global_plugins(
                &plugin_store,
                edgion_http.edgion_gateway_config.spec.global_plugins_ref.as_deref(),
            );
            ctx.resolved_global_plugins = Some(Arc::new(resolved));
        }

        if ctx.resolved_route_ext_refs.is_none() {
            let resolved = match (&ctx.route_unit, &ctx.grpc_route_unit) {
                (Some(unit), _) => resolve_route_ext_refs(&plugin_store, unit.plugin_runtime.ext_ref_keys()),
                (_, Some(unit)) => resolve_route_ext_refs(&plugin_store, unit.plugin_runtime.ext_ref_keys()),
                _ => Vec::new(),
            };
            ctx.resolved_route_ext_refs = Some(Arc::new(resolved));
        }
    }

    // Step 2: Run global plugins from EdgionGatewayConfig (executed before route-level plugins)
    // Reads the per-request snapshot populated above (zero format!/store cost).
    if let Some(plugins) = ctx.resolved_global_plugins.as_ref().map(Arc::clone) {
        for runtime in plugins.iter() {
            if runtime.request_plugins_count() == 0 {
                continue;
            }
            runtime.run_request_plugins(session, ctx).await;
            if ctx.plugin_running_result == PluginRunningResult::ErrTerminateRequest {
                let _ = terminate_with_fallback(session, ctx, &edgion_http.server_header_opts).await;
                return Ok(true);
            }
        }
    }

    // Step 2b: Run Gateway-level plugins (edgion.io/edgion-plugins annotation), after
    // GatewayClass-global and before route-level. Resolve once into the per-request snapshot.
    if ctx.resolved_gateway_plugins.is_none() {
        let keys = ctx.gateway_info.plugin_store_keys.clone();
        let resolved = if keys.is_empty() {
            Vec::new()
        } else {
            resolve_gateway_plugins(&get_global_plugin_store(), &keys)
        };
        ctx.resolved_gateway_plugins = Some(Arc::new(resolved));
    }
    if let Some(plugins) = ctx.resolved_gateway_plugins.as_ref().map(Arc::clone) {
        for runtime in plugins.iter() {
            if runtime.request_plugins_count() == 0 {
                continue;
            }
            runtime.run_request_plugins(session, ctx).await;
            if ctx.plugin_running_result == PluginRunningResult::ErrTerminateRequest {
                let _ = terminate_with_fallback(session, ctx, &edgion_http.server_header_opts).await;
                return Ok(true);
            }
        }
    }

    // Step 3: Run route-level plugins based on matched route type
    if let Some(grpc_route_unit) = ctx.grpc_route_unit.clone() {
        grpc_route_unit.plugin_runtime.run_request_plugins(session, ctx).await;
        if ctx.plugin_running_result == PluginRunningResult::ErrTerminateRequest {
            let _ = terminate_with_fallback(session, ctx, &edgion_http.server_header_opts).await;
            return Ok(true);
        }
    } else if let Some(route_unit) = ctx.route_unit.clone() {
        route_unit.plugin_runtime.run_request_plugins(session, ctx).await;
        if ctx.plugin_running_result == PluginRunningResult::ErrTerminateRequest {
            let _ = terminate_with_fallback(session, ctx, &edgion_http.server_header_opts).await;
            return Ok(true);
        }
    }

    set_x_real_ip(session, ctx);
    append_x_forwarded_for(session, ctx);

    // Request-stage plugins have run, but this is NOT the exit point for
    // `in_request_body_stage`: two more request-plugin stages still run after
    // this function returns -- the backendRef-scoped chain in
    // `pg_upstream_peer.rs::upstream_peer` (HTTP and gRPC) -- and Pingora has
    // not started pumping the downstream body upstream yet at that point.
    // The marker is cleared later, at the tail of `upstream_request_filter`
    // (`pg_upstream_request_filter.rs`), which is the true boundary right
    // before body proxying begins. See `in_request_body_stage`'s doc for the
    // full lifecycle.

    Ok(false)
}

/// Build request metadata: client addresses, hostname, path, trace_id, x-forwarded-for, and protocol detection (inline for performance)
/// Also validates X-Forwarded-For header length against security_protect configuration.
/// Returns Ok(true) if response has been sent (due to validation failure), Ok(false) to continue.
#[inline]
async fn build_request_metadata(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<bool> {
    // Snapshot whether the client declared a body, BEFORE any plugin runs.
    // See `snapshot_body_declared` for why this must not be deferred.
    snapshot_body_declared(session, ctx);

    let req_header = session.req_header();
    let raw_path = req_header.uri.path().to_string();
    let raw_query = req_header.uri.query().map(str::to_string);
    let path_norm_cfg = get_path_normalization_config();

    // Protocol detection: check for WebSocket, gRPC-Web, and gRPC
    detect_request_protocol(req_header, ctx);

    // Parse client-supplied `grpc-timeout` deadline. The gRPC-over-HTTP/2 spec
    // expects servers / intermediaries to honour it; the parsed deadline is
    // anchored to `ctx.start_time` so peer timeout computation and the
    // upstream `grpc-timeout` rewrite share one source of truth. Malformed,
    // missing, or overflowing headers fall through to route-level
    // `request_timeout` behaviour. `checked_add` guards against an upper-bound
    // value (e.g. `99999999H` from a hostile client) overflowing `Instant`.
    if ctx.request_info.is_grpc_request && ctx.request_info.grpc_client_deadline.is_none() {
        if let Some(raw) = req_header.headers.get("grpc-timeout") {
            if let Some(deadline) = raw
                .to_str()
                .ok()
                .and_then(parse_grpc_timeout)
                .and_then(|dur| ctx.start_time.checked_add(dur))
            {
                ctx.request_info.grpc_client_deadline = Some(deadline);
            }
        }
    }

    // Extract client_addr and client_port (TCP connection address).
    // as_inet() avoids the to_string() -> parse() round trip; UDS downstream
    // is not a real scenario for the HTTP gateway, so falling back to empty
    // string for non-inet addresses is acceptable.
    let inet = session.client_addr().and_then(|a| a.as_inet());
    ctx.request_info.client_addr = inet.map(|s| s.ip().to_string()).unwrap_or_default();
    ctx.request_info.client_port = inet.map(|s| s.port()).unwrap_or(0);

    // Extract remote_addr (real client IP, considering trusted proxies).
    // extract_real_ip accepts a bare IP string as well as "ip:port".
    ctx.request_info.remote_addr = if let Some(extractor) = &edgion_http.real_ip_extractor {
        extractor.extract_real_ip(&ctx.request_info.client_addr, req_header)
    } else {
        ctx.request_info.client_addr.clone()
    };

    // Generate x_trace_id before any early-return guards so 400/4xx access-log
    // entries carry a trace id that can be correlated with tls.log.
    ctx.request_info.x_trace_id = req_header
        .headers
        .get("x-trace-id")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| Some(uuid::Uuid::new_v4().to_string()));

    // RFC 9110 §7.2: reject HTTP/1.1 requests carrying more than one Host header
    // line. Host smuggling defense — gated by `SecurityProtectConfig.reject_duplicate_host`
    // (default true), matching Envoy's effective behavior (commas in the coalesced
    // :authority fail Envoy's authority validator). HTTP/2 carries authority in the
    // `:authority` pseudo-header and `headers.get_all("host")` is normally empty for
    // h2, so this branch is a no-op for h2 / gRPC-over-h2 traffic.
    let reject_duplicate_host = edgion_http
        .edgion_gateway_config
        .spec
        .security_protect
        .as_ref()
        .map(|s| s.reject_duplicate_host)
        .unwrap_or(true);
    if reject_duplicate_host && req_header.headers.get_all("host").iter().count() > 1 {
        ctx.add_error(EdgionStatus::DuplicateHostHeader);
        end_response_400(session, ctx, &edgion_http.server_header_opts).await?;
        return Ok(true);
    }

    // Extract hostname from URI (HTTP/2), Host header (HTTP/1.1), or :authority (HTTP/2 fallback).
    // NormalizedHostname::new() applies trim, port-strip, FQDN-dot-strip, and lowercase once.
    ctx.request_info.hostname = req_header
        .uri
        .host()
        .map(NormalizedHostname::new)
        .or_else(|| {
            req_header
                .headers
                .get("host")
                .and_then(|h| h.to_str().ok().map(NormalizedHostname::new))
        })
        .or_else(|| {
            req_header
                .headers
                .get(":authority")
                .and_then(|h| h.to_str().ok().map(NormalizedHostname::new))
        })
        .unwrap_or_default();

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
                ctx.request_info.tls_id = meta.tls_id.clone();
                ctx.request_info.sni = meta.sni.clone();
                ctx.request_info.client_cert_info = meta.client_cert_info.clone();
                // Extract Gateway TLS listener info for 421 Misdirected Request detection.
                // Only set for Gateway TLS (not EdgionTls, which has no listener concept).
                if let Some(ref matched) = meta.matched {
                    if matched.rsc.kind == "Gateway" {
                        ctx.request_info.tls_matched_listener = matched.section.clone();
                        ctx.request_info.tls_matched_gateway_key =
                            Some(format_gateway_key(&matched.rsc.ns, &matched.rsc.name));
                    }
                }
            } else if let Some(tls_id) = ssl_digest.extension.get::<TlsConnId>() {
                // Backward compatibility for connections created before metadata upgrade.
                ctx.request_info.tls_id = Some(tls_id.0.to_string());
            }
        }
    }

    if let Some(status) = apply_path_normalization_entry(ctx, &raw_path, raw_query, path_norm_cfg) {
        ctx.add_error(status);
        end_response_400(session, ctx, &edgion_http.server_header_opts).await?;
        return Ok(true);
    }

    if try_serve_acme_challenge(edgion_http, session, ctx).await? {
        return Ok(true);
    }

    // Validate hostname is present.
    //
    // RFC 9110 §7.2 (HTTP/1.1) and RFC 9113 §8.3.1 (HTTP/2 :authority) both
    // require rejecting messages whose target host field is missing or has an
    // invalid value. `NormalizedHostname` trims whitespace and strips the port,
    // so a `Host` / `:authority` of "  " collapses to empty here.
    //
    // gRPC traffic must take the same guard: without it an empty `:authority`
    // would land on the first catch-all GRPCRoute and bypass per-hostname
    // tenant isolation. `end_response_400` already routes gRPC requests to
    // `grpc-status 3` (INVALID_ARGUMENT) trailers via `send_grpc_trailers_only`.
    if ctx.request_info.hostname.is_empty() {
        ctx.add_error(EdgionStatus::HostMissing);
        end_response_400(session, ctx, &edgion_http.server_header_opts).await?;
        return Ok(true); // Response sent
    }

    // Validate X-Forwarded-For length against security configuration
    if let Some(security_config) = &edgion_http.edgion_gateway_config.spec.security_protect {
        if let Some(ref existing_xff) = ctx.request_info.x_forwarded_for {
            let xff_len = existing_xff.len();
            if xff_len > security_config.x_forwarded_for_limit {
                ctx.add_error(EdgionStatus::XffHeaderTooLong);
                end_response_400(session, ctx, &edgion_http.server_header_opts).await?;
                return Ok(true); // Response sent
            }
        }
    }

    Ok(false) // Continue processing
}

/// Snapshot whether the client declared a request body, BEFORE any plugin runs.
///
/// This must not be deferred to the body gate. `is_body_empty()` is pingora's
/// transport-aware predicate (H1: RFC 9112 6.3; h2: END_STREAM / content-length
/// 0), but on H1 it derives framing from the LIVE header map via
/// `init_body_reader()`. The gate runs mid-chain, so an earlier plugin removing
/// `Content-Length` would make a bodied request look empty. Taken here, the
/// header map is still the client's, and the H1 framing is pinned at the one
/// moment when pinning it is correct.
///
/// Extracted so the derivation is directly testable: an inversion here would
/// make a bodied request look bodyless and silently skip body inspection.
///
/// Also marks entry into the request-processing stage (`in_request_body_stage`)
/// -- see that field's doc for why this is the paired entry point and where the
/// matching exit write lives.
fn snapshot_body_declared(session: &mut Session, ctx: &mut EdgionHttpContext) {
    ctx.request_info.body_declared = Some(!session.is_body_empty());
    ctx.request_info.in_request_body_stage = true;
}

/// Detect WebSocket / gRPC / gRPC-Web from the request's `Upgrade` and
/// `Content-Type` headers, when no protocol has already been discovered
/// upstream of this call. A no-op once `discover_protocol` is already set.
///
/// WebSocket is checked first: only an exact (case-insensitive) `Upgrade:
/// websocket` token match sets it -- a comma-separated upgrade list (e.g.
/// `Upgrade: websocket, h2c`) or an unrecognized single token (e.g. `Upgrade:
/// foo`) leaves `discover_protocol` unset here, falling through to the
/// gRPC/gRPC-Web content-type check. gRPC-Web is tested before native gRPC
/// since its media-type prefix also satisfies the gRPC prefix check (RFC 9110
/// §8.3.1 media types are case-insensitive).
///
/// Extracted from `build_request_metadata` so protocol detection is directly
/// unit-testable against a bare `RequestHeader` -- see
/// `protocol_detection_tests` -- following the same extraction precedent as
/// `snapshot_body_declared`.
fn detect_request_protocol(req_header: &RequestHeader, ctx: &mut EdgionHttpContext) {
    if ctx.request_info.discover_protocol.is_some() {
        return;
    }

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
                if super::is_grpc_web_content_type(ct_str) {
                    ctx.request_info.discover_protocol = Some("grpc-web".to_string());
                    ctx.request_info.is_grpc_request = true;
                } else if super::is_grpc_content_type(ct_str) {
                    ctx.request_info.discover_protocol = Some("grpc".to_string());
                    ctx.request_info.is_grpc_request = true;
                }
            }
        }
    }
}

/// Serve ACME HTTP-01 challenge response if there is an active challenge matching the request.
///
/// Called after request metadata and path normalization entry are populated so
/// the access log retains client/host/trace/raw URI context for challenge hits.
#[inline]
async fn try_serve_acme_challenge(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<bool> {
    let store = get_global_challenge_store();
    if store.is_empty() {
        return Ok(false);
    }

    let Some((token, host)) = acme_challenge_token_and_host(session.req_header()) else {
        return Ok(false);
    };

    let key_auth = match store.lookup(token, &host) {
        Some(v) => v,
        None => return Ok(false),
    };

    ctx.add_info(EdgionStatus::AcmeChallengeServed);
    ctx.request_info.status = Some(200);
    ctx.request_info.response_committed = true;

    let mut resp = ResponseHeader::build(200, None)?;
    edgion_http.server_header_opts.apply_to_response(&mut resp);
    resp.insert_header("Content-Type", "text/plain")?;
    resp.insert_header("Content-Length", key_auth.len().to_string())?;

    session.write_response_header(Box::new(resp), false).await?;
    session.write_response_body(Some(key_auth), true).await?;

    Ok(true)
}

fn acme_challenge_token_and_host(req_header: &RequestHeader) -> Option<(&str, String)> {
    let token = match req_header.uri.path().strip_prefix(ACME_CHALLENGE_PREFIX) {
        Some(t) if !t.is_empty() => t,
        _ => return None,
    };

    let host = req_header
        .headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .map(host_without_port)
        .unwrap_or_default()
        .to_string();

    Some((token, host))
}

fn apply_path_normalization_entry(
    ctx: &mut EdgionHttpContext,
    raw_path: &str,
    raw_query: Option<String>,
    cfg: &PathNormalizationConfig,
) -> Option<EdgionStatus> {
    ctx.request_info.entry_raw_path = raw_path.to_string();
    ctx.request_info.entry_raw_query = raw_query;

    if cfg.reject_escaped_slashes && contains_escaped_slash(raw_path) {
        return Some(EdgionStatus::PathEscapedSlashRejected);
    }

    if cfg.reject_invalid_percent_encoding && has_invalid_percent_encoding(raw_path) {
        return Some(EdgionStatus::PathInvalidPercentEncoding);
    }

    ctx.request_info.normalized_current_path = normalize_path(raw_path, cfg).unwrap_or_else(|| raw_path.to_string());

    None
}

/// Append client IP to X-Forwarded-For header (inline for performance)
///
/// This function always appends the client IP to maintain the proxy chain,
/// regardless of trusted proxy configuration.
#[inline]
fn append_x_forwarded_for(session: &mut Session, ctx: &EdgionHttpContext) {
    // client_addr is already IP only (without port)
    let client_ip = &ctx.request_info.client_addr;

    let req_header_mut = session.req_header_mut();
    if let Some(ref existing_xff) = ctx.request_info.x_forwarded_for {
        let mut new_xff = String::with_capacity(existing_xff.len() + 2 + client_ip.len());
        new_xff.push_str(existing_xff);
        new_xff.push_str(", ");
        new_xff.push_str(client_ip);
        let _ = req_header_mut.insert_header("X-Forwarded-For", &new_xff);
    } else {
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

#[cfg(test)]
mod tests {
    use super::*;
    use pingora_http::RequestHeader;

    #[test]
    fn acme_challenge_token_and_host_extracts_token_and_strips_host_port() {
        let mut req = RequestHeader::build("GET", b"/.well-known/acme-challenge/token-1", None).unwrap();
        req.insert_header("host", "example.com:80").unwrap();

        let (token, host) = acme_challenge_token_and_host(&req).unwrap();

        assert_eq!(token, "token-1");
        assert_eq!(host, "example.com");
    }

    #[test]
    fn acme_challenge_token_and_host_strips_port_from_ipv6_literal_host() {
        // A bracketed IPv6 literal with a port must keep its brackets, not be
        // mangled to "[" by a naive split(':') — otherwise the challenge lookup
        // never matches the stored entry.domain.
        let mut req = RequestHeader::build("GET", b"/.well-known/acme-challenge/token-2", None).unwrap();
        req.insert_header("host", "[::1]:80").unwrap();

        let (token, host) = acme_challenge_token_and_host(&req).unwrap();

        assert_eq!(token, "token-2");
        assert_eq!(host, "[::1]");
    }

    #[test]
    fn acme_challenge_token_and_host_ignores_non_challenge_paths() {
        let req = RequestHeader::build("GET", b"/api/.well-known/acme-challenge/token-1", None).unwrap();

        assert!(acme_challenge_token_and_host(&req).is_none());
    }

    #[test]
    fn path_reject_freezes_entry_raw_uri_for_access_log() {
        let mut ctx = EdgionHttpContext::new();
        let cfg = PathNormalizationConfig {
            reject_escaped_slashes: true,
            reject_invalid_percent_encoding: true,
            ..Default::default()
        };

        let status = apply_path_normalization_entry(&mut ctx, "/admin%2Fsecret", Some("trace=1".to_string()), &cfg);

        assert_eq!(status, Some(EdgionStatus::PathEscapedSlashRejected));
        assert_eq!(ctx.request_path(), "/admin%2Fsecret");
        assert_eq!(ctx.request_uri(), "/admin%2Fsecret?trace=1");
    }

    #[test]
    fn path_normalization_entry_refreshes_normalized_current_path() {
        let mut ctx = EdgionHttpContext::new();
        let cfg = PathNormalizationConfig::default();

        let status = apply_path_normalization_entry(&mut ctx, "/admin//../secret", None, &cfg);

        assert_eq!(status, None);
        assert_eq!(ctx.request_path(), "/admin//../secret");
        assert_eq!(ctx.path(), "/secret");
    }
}

/// Coverage for `snapshot_body_declared`, the `body_declared` snapshot taken
/// at the top of `build_request_metadata`.
///
/// `build_request_metadata` itself cannot be driven from a unit test: it is a
/// private `async fn` that takes `&EdgionHttpProxy`, and every field of that
/// struct (access logger, gateway config, route managers, ...) is built only
/// by the live listener bootstrap in `runtime/server/listener_builder.rs` —
/// there is no test helper anywhere in this crate that constructs one, and
/// hand-assembling all of it just to reach this one line would be far more
/// scaffolding than the line under test.
///
/// Instead this drives a real `pingora_proxy::Session` off real HTTP/1.1
/// request bytes over a `tokio::io::duplex` pair — the same pattern
/// `session_adapter.rs`'s `test_session()` helper uses — and calls
/// `snapshot_body_declared` directly against it, asserting on
/// `ctx.request_info.body_declared`. Unlike the hand-set `body_declared` in
/// `ctx.rs` / `session_adapter.rs`'s tests, this exercises pingora's real H1
/// framing (`init_body_reader()`), so a dropped `!` in `snapshot_body_declared`
/// would flip these assertions and fail.
#[cfg(test)]
mod body_declared_snapshot_tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    /// Build a real HTTP/1.1 `Session` from literal request bytes.
    ///
    /// `pub(super)` so `protocol_detection_tests` (a sibling module) can reuse
    /// it instead of duplicating the duplex-pair plumbing.
    pub(super) async fn session_from_request(raw_request: &[u8]) -> Session {
        let (mut client, server) = tokio::io::duplex(4096);
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        client
            .write_all(raw_request)
            .await
            .expect("write request into duplex pair");
        drop(client); // header (and body, if any) bytes are already buffered
        session
            .read_request()
            .await
            .expect("read_request should succeed")
            .then_some(())
            .expect("client must not have closed before sending the request");
        session
    }

    #[tokio::test]
    async fn post_with_content_length_and_body_declares_body_true() {
        let mut session =
            session_from_request(b"POST /submit HTTP/1.1\r\nHost: test\r\nContent-Length: 5\r\n\r\nhello").await;
        let mut ctx = EdgionHttpContext::default();

        snapshot_body_declared(&mut session, &mut ctx);

        assert_eq!(ctx.request_info.body_declared, Some(true));
    }

    #[tokio::test]
    async fn get_with_no_content_length_or_transfer_encoding_declares_body_false() {
        let mut session = session_from_request(b"GET /status HTTP/1.1\r\nHost: test\r\n\r\n").await;
        let mut ctx = EdgionHttpContext::default();

        snapshot_body_declared(&mut session, &mut ctx);

        assert_eq!(ctx.request_info.body_declared, Some(false));
    }
}

/// Coverage for `detect_request_protocol`'s `Upgrade` handling.
///
/// This is the property `session_adapter.rs`'s (now-removed) tautological
/// `request_admission_unknown_upgrade_token_buffers` test claimed to cover
/// but never actually exercised (it encoded no `Upgrade` header at all). The
/// real property lives here, at the source of `discover_protocol`: only an
/// exact `Upgrade: websocket` token sets it; an unrecognized token or a
/// comma-list containing `websocket` must not.
#[cfg(test)]
mod protocol_detection_tests {
    use super::body_declared_snapshot_tests::session_from_request;
    use super::*;

    #[tokio::test]
    async fn upgrade_websocket_sets_discover_protocol() {
        let session = session_from_request(b"GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\r\n").await;
        let mut ctx = EdgionHttpContext::default();

        detect_request_protocol(session.req_header(), &mut ctx);

        assert_eq!(ctx.request_info.discover_protocol.as_deref(), Some("websocket"));
    }

    #[tokio::test]
    async fn upgrade_unknown_token_does_not_set_websocket() {
        let session = session_from_request(b"GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: foo\r\n\r\n").await;
        let mut ctx = EdgionHttpContext::default();

        detect_request_protocol(session.req_header(), &mut ctx);

        assert_ne!(ctx.request_info.discover_protocol.as_deref(), Some("websocket"));
    }

    #[tokio::test]
    async fn upgrade_comma_list_does_not_set_websocket() {
        // A comma-separated Upgrade list is not an exact "websocket" token
        // match -- treating it as one would let a client smuggle a body past
        // the websocket-handshake shape checks in `request_admission`.
        let session = session_from_request(b"GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket, h2c\r\n\r\n").await;
        let mut ctx = EdgionHttpContext::default();

        detect_request_protocol(session.req_header(), &mut ctx);

        assert_ne!(ctx.request_info.discover_protocol.as_deref(), Some("websocket"));
    }
}

/// Extract CORS configuration dynamically from route filters
///
/// Walks the route's `ExtensionRef` filters that point at `EdgionPlugins`
/// resources and returns the first pre-resolved `Arc<CorsConfig>` cached on
/// the matched `PluginRuntime`. The cache is rebuilt on every `EdgionPlugins`
/// preparse, so hot-reload semantics are preserved while the preflight hot
/// path avoids deep-cloning the full `CorsConfig` per request.
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

        let key = format!("{}/{}", namespace, ext_ref.name);
        let cors = get_global_plugin_store()
            .with_plugin(&key, |c| c.plugin_runtime.cors_config_arc())
            .flatten();
        if cors.is_some() {
            return cors;
        }
    }

    None
}
