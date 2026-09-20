use super::{resolve_route_ext_refs, select_waf_owner, EdgionHttpProxy};
use crate::ctx::NormalizedHostname;
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
use edgion_resources::path_normalization::get_path_normalization_config;
use edgion_resources::path_normalization::{
    contains_escaped_slash, has_invalid_percent_encoding, normalize_path, PathNormalizationConfig,
};
use edgion_resources::resources::extension_ref::is_supported_edgion_plugins_ref;
use edgion_resources::resources::{CorsConfig, HTTPRouteFilter, HTTPRouteFilterType};
use edgion_resources::{EdgionErrorCode, EdgionInfoCode};
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::Session;
use std::sync::Arc;

/// ACME HTTP-01 challenge path prefix
const ACME_CHALLENGE_PREFIX: &str = "/.well-known/acme-challenge/";

struct XffSnapshot {
    value: Option<String>,
    too_long: bool,
}

/// Combine repeated X-Forwarded-For field lines into their ordered logical
/// value while enforcing the configured aggregate byte limit.
fn snapshot_x_forwarded_for(req_header: &RequestHeader, limit: Option<usize>) -> XffSnapshot {
    let values = req_header.headers.get_all("x-forwarded-for");
    let mut iter = values.iter();
    let Some(first) = iter.next() else {
        return XffSnapshot {
            value: None,
            too_long: false,
        };
    };

    let mut count = 1usize;
    let mut total_len = first.len();
    let mut overflowed = false;
    let mut valid = first.to_str().is_ok();

    for value in iter {
        count += 1;
        valid &= value.to_str().is_ok();
        if !overflowed {
            match total_len.checked_add(2).and_then(|len| len.checked_add(value.len())) {
                Some(len) => total_len = len,
                None => overflowed = true,
            }
        }
    }

    let too_long = overflowed || limit.is_some_and(|limit| total_len > limit);
    if too_long || !valid {
        return XffSnapshot { value: None, too_long };
    }

    if count == 1 {
        let Ok(first) = first.to_str() else {
            return XffSnapshot {
                value: None,
                too_long: false,
            };
        };
        return XffSnapshot {
            value: Some(first.to_owned()),
            too_long: false,
        };
    }

    let mut combined = String::with_capacity(total_len);
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            combined.push_str(", ");
        }
        let Ok(value) = value.to_str() else {
            return XffSnapshot {
                value: None,
                too_long: false,
            };
        };
        combined.push_str(value);
    }

    XffSnapshot {
        value: Some(combined),
        too_long: false,
    }
}

#[inline]
pub async fn request_filter(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<bool> {
    // Also covers direct test callers; normal Pingora entry already ran this.
    super::pg_early_request_filter::initialize_request_ids(edgion_http, session, ctx).await?;
    if ctx.plugin_terminated() {
        let _ = terminate_with_fallback(session, ctx, &edgion_http.server_header_opts).await;
        return Ok(true);
    }
    // Build request metadata (addresses, hostname, path, trace_id, protocol)
    // Validates XFF length and hostname presence, returns true if response sent
    if build_request_metadata(edgion_http, session, ctx).await? {
        return Ok(true); // Response already sent (XFF too long or hostname missing)
    }

    // Step 1: Route matching — single lookup against the global route table.
    // Load the pre-built GatewayInfoIndex for O(1) gateway/listener lookups.
    let gateway_index = get_listener_gateway_info_store().get_index(&edgion_http.listener_key);

    if try_serve_acme_challenge(edgion_http, session, ctx, &gateway_index).await? {
        return Ok(true);
    }

    // HTTPS listener isolation: 421 Misdirected Request (Gateway API spec).
    // If SNI matched listener A but Host should be served by a different listener → 421.
    // Enabled by global requireSniHostMatch OR per-Gateway annotation edgion.io/require-sni-host-match.
    // Only applies to Gateway TLS (tls_matched_listener is None for HTTP/EdgionTls).
    if edgion_http.is_https_listener {
        if let Some((sni_listener_name, gateway_key)) = ctx.request_info.gateway_tls_match() {
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
                            ctx.record_error(EdgionErrorCode::SniHostMismatch);
                            end_response_421(session, ctx, &edgion_http.server_header_opts).await?;
                            return Ok(true);
                        }
                    }
                }
            }
        }
    }

    if ctx.request_info.is_grpc_request {
        if native_grpc_requires_frontend_http2(ctx.request_info.discover_protocol.as_deref(), edgion_http.enable_http2)
        {
            ctx.record_error(EdgionErrorCode::Http2Required);
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
            ctx.record_error(EdgionErrorCode::RouteNotFound);
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
                ctx.gateway_info = Some(result.matched_gateway);
                ctx.capture_matched_path_params(&result.route_unit.matched_info.m);
                ctx.route_unit = Some(result.route_unit);
            }
            Err(_) => {
                ctx.record_error(EdgionErrorCode::RouteNotFound);
                end_response_404(session, ctx, &edgion_http.server_header_opts).await?;
                return Ok(true); // Already sent error response, signal completion
            }
        }
    }

    super::pg_early_request_filter::capture_matched_scope(ctx)?;
    let owner = ctx.plugin_scope().cloned().expect("matched scope was captured");
    if !owner.scope.available {
        crate::end_response_503(session, ctx, &edgion_http.server_header_opts).await?;
        return Ok(true);
    }
    if !ctx.global_request_ids_initialized {
        ctx.install_resolved_global_plugins(super::resolve_global_plugins(&owner.scope));
    }
    if !ctx.gateway_request_ids_initialized {
        ctx.install_resolved_gateway_plugins(super::resolve_gateway_plugins(&owner.scope, &owner.http_keys));
    }
    let keys = ctx
        .route_unit
        .as_ref()
        .map(|unit| unit.plugin_runtime.ext_ref_keys())
        .or_else(|| {
            ctx.grpc_route_unit
                .as_ref()
                .map(|unit| unit.plugin_runtime.ext_ref_keys())
        })
        .unwrap_or_default();
    if !keys.is_empty() {
        ctx.install_resolved_route_ext_refs(resolve_route_ext_refs(&owner.scope, keys));
    }
    // Later-stage restrictions already have a request admission gate. Run it
    // before AI validation, CORS, ordinary plugins, WAF or body access. Global
    // and Gateway request IDs may already have run in Pingora's early hook.
    let global = ctx.resolved_global_plugins();
    let gateway = ctx.resolved_gateway_plugins();
    let route = ctx.resolved_route_ext_refs();
    for runtime in global
        .iter()
        .flat_map(|plugins| plugins.iter())
        .chain(gateway.iter().flat_map(|plugins| plugins.iter()))
        .chain(route.iter().flat_map(|plugins| plugins.iter().flatten()))
    {
        runtime.run_preflight_gate(session, ctx).await;
        if ctx.plugin_terminated() {
            let _ = terminate_with_fallback(session, ctx, &edgion_http.server_header_opts).await;
            return Ok(true);
        }
    }
    let matched_route_runtime = ctx
        .grpc_route_unit
        .as_ref()
        .map(|unit| Arc::clone(&unit.plugin_runtime))
        .or_else(|| ctx.route_unit.as_ref().map(|unit| Arc::clone(&unit.plugin_runtime)));
    if let Some(runtime) = matched_route_runtime {
        runtime.run_preflight_gate(session, ctx).await;
        if ctx.plugin_terminated() {
            let _ = terminate_with_fallback(session, ctx, &edgion_http.server_header_opts).await;
            return Ok(true);
        }
    }
    if let Some(unit) = ctx.route_unit.as_ref() {
        if !unit
            .route
            .as_ref()
            .is_some_and(|source| owner.scope.ai_rule(source, unit.specificity.rule_index as usize))
        {
            end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
            return Ok(true);
        }
    }
    super::pg_early_request_filter::initialize_gateway_request_ids(session, ctx).await?;

    // Step 1.5: Handle preflight requests (before plugin execution)
    let preflight_policy = ctx.preflight_policy;
    if edgion_http.preflight_handler.is_preflight(session, preflight_policy) {
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
            let route_ext_refs = ctx.resolved_route_ext_refs();
            let cors_config = if let Some(ref route_unit) = ctx.route_unit {
                extract_cors_config_from_route_filters(
                    &route_unit.rule.filters,
                    route_ext_refs.as_deref().unwrap_or_default(),
                )
            } else if let Some(_grpc_route_unit) = &ctx.grpc_route_unit {
                // gRPC routes don't have CORS config yet, so None
                None
            } else {
                None
            };

            // Handle preflight request
            match edgion_http
                .preflight_handler
                .handle_preflight(session, ctx, preflight_policy, cors_config.as_ref())
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

    let global_plugins = ctx.resolved_global_plugins();
    let gateway_plugins = ctx.resolved_gateway_plugins();
    let route_ext_refs = ctx.resolved_route_ext_refs();

    let selected_waf = select_waf_owner(
        global_plugins.as_deref().unwrap_or_default(),
        gateway_plugins.as_deref().unwrap_or_default(),
        route_ext_refs.as_deref().unwrap_or_default(),
    );
    if !ctx.select_waf(selected_waf) {
        end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
        return Ok(true);
    }

    // Step 2: Run global plugins from EdgionGatewayConfig (executed before route-level plugins)
    // Reads the per-request snapshot populated above (zero format!/store cost).
    if let Some(plugins) = global_plugins {
        for runtime in plugins.iter() {
            if runtime.request_plugins_count() == 0 {
                continue;
            }
            runtime.run_request_plugins_after_early_ids(session, ctx).await;
            if ctx.plugin_terminated() {
                let _ = terminate_with_fallback(session, ctx, &edgion_http.server_header_opts).await;
                return Ok(true);
            }
        }
    }

    // Step 2b: Run Gateway-level plugins (edgion.io/edgion-plugins annotation), after
    // GatewayClass-global and before route-level. The snapshot was resolved
    // before WAF winner election above.
    if let Some(plugins) = gateway_plugins {
        for runtime in plugins.iter() {
            if runtime.request_plugins_count() == 0 {
                continue;
            }
            runtime.run_request_plugins_after_early_ids(session, ctx).await;
            if ctx.plugin_terminated() {
                let _ = terminate_with_fallback(session, ctx, &edgion_http.server_header_opts).await;
                return Ok(true);
            }
        }
    }

    // Step 3: Run route-level plugins based on matched route type
    if let Some(grpc_route_unit) = ctx.grpc_route_unit.clone() {
        grpc_route_unit.plugin_runtime.run_request_plugins(session, ctx).await;
        if ctx.plugin_terminated() {
            let _ = terminate_with_fallback(session, ctx, &edgion_http.server_header_opts).await;
            return Ok(true);
        }
    } else if let Some(route_unit) = ctx.route_unit.clone() {
        route_unit.plugin_runtime.run_request_plugins(session, ctx).await;
        if ctx.plugin_terminated() {
            let _ = terminate_with_fallback(session, ctx, &edgion_http.server_header_opts).await;
            return Ok(true);
        }
    }

    // Body policy is request-scoped. Global, Gateway, and route request
    // plugins have now had their one opportunity to capture, mutate, observe,
    // or install streamed processing. BackendRef request plugins run later
    // during peer selection and may consume an already-cached snapshot. Their
    // first-attempt dispatcher also permits a read-only mirror tee before the
    // body pump starts; it changes neither framing nor replay policy. Capture,
    // mutation and other body capability installation remain closed here.
    ctx.request_info.in_request_body_stage = false;

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
    if ctx.request_info.is_grpc_request && ctx.request_info.grpc_client_deadline().is_none() {
        if let Some(raw) = req_header.headers.get("grpc-timeout") {
            if let Some(deadline) = raw
                .to_str()
                .ok()
                .and_then(parse_grpc_timeout)
                .and_then(|dur| ctx.start_time.checked_add(dur))
            {
                ctx.request_info.set_grpc_client_deadline(deadline);
            }
        }
    }

    // Preserve both transport identities. Pingora's `peer_addr()` may contain
    // the accepted PROXY-protocol source, while `raw_peer_addr()` always names
    // the physical previous hop.
    let raw = session
        .as_downstream()
        .digest()
        .and_then(|d| d.socket_digest.as_ref())
        .and_then(|d| d.raw_peer_addr());
    let addrs = crate::util::resolve_transport_client_addrs(session.client_addr(), raw, "");
    ctx.request_info.downstream_peer_ip = addrs.remote_ip;
    ctx.request_info.direct_peer_ip = addrs.direct_peer_ip;
    ctx.request_info.direct_peer_port = addrs.direct_peer_port;

    let xff_limit = edgion_http
        .edgion_gateway_config
        .spec
        .security_protect
        .as_ref()
        .map(|config| config.x_forwarded_for_limit);
    let xff_snapshot = snapshot_x_forwarded_for(req_header, xff_limit);
    let xff_too_long = xff_snapshot.too_long;

    let resolved = if let Some(extractor) = &edgion_http.real_ip_extractor {
        if extractor.real_ip_header().eq_ignore_ascii_case("x-forwarded-for") {
            extractor
                .resolve_real_ip(&ctx.request_info.downstream_peer_ip, xff_snapshot.value.as_deref())
                .remote_ip
        } else {
            extractor.extract_real_ip(&ctx.request_info.downstream_peer_ip, req_header)
        }
    } else {
        ctx.request_info.downstream_peer_ip.clone()
    };
    ctx.request_info.remote_ip = crate::ctx::canonicalize_ip_str(&resolved).unwrap_or(resolved);

    // RFC 9110 §7.2: reject HTTP/1.1 requests carrying more than one Host header
    // line. Host smuggling defense — gated by `SecurityProtectConfig.reject_duplicate_host`
    // (default true), matching Envoy's effective behavior (commas in the coalesced
    // :authority fail Envoy's authority validator). HTTP/2 carries authority in the
    // `:authority` pseudo-header and `headers.get_all("host")` is normally empty for
    // h2, so this branch is a no-op for h2 / gRPC-over-h2 traffic.
    if ctx.reject_duplicate_host && req_header.headers.get_all("host").iter().count() > 1 {
        ctx.record_error(EdgionErrorCode::DuplicateHostHeader);
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

    // Publish the bounded XFF snapshot only after the earlier Host validation.
    // A duplicate-Host rejection must not expose partially built metadata.
    ctx.request_info.x_forwarded_for = xff_snapshot.value;

    if let Some(status) = request_metadata_validation_error(ctx, &raw_path, raw_query, path_norm_cfg, xff_too_long) {
        ctx.record_error(status);
        end_response_400(session, ctx, &edgion_http.server_header_opts).await?;
        return Ok(true); // Response sent
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
/// matching exit write lives -- and snapshots whether the DOWNSTREAM method was
/// `HEAD` (`method_is_head`), which the response-side admission predicate reads
/// instead of the live request header a request-stage plugin can rewrite via
/// `set_upstream_method`. All three writes belong here for the same reason:
/// this is the one point that runs before any plugin can mutate the request.
fn snapshot_body_declared(session: &mut Session, ctx: &mut EdgionHttpContext) {
    ctx.request_info.body_declared = Some(!session.is_body_empty());
    ctx.request_info.in_request_body_stage = true;
    ctx.request_info.method_is_head = session.req_header().method == http::Method::HEAD;
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
pub(super) fn detect_request_protocol(req_header: &RequestHeader, ctx: &mut EdgionHttpContext) {
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

#[inline]
pub(super) fn native_grpc_requires_frontend_http2(
    discover_protocol: Option<&str>,
    frontend_http2_enabled: bool,
) -> bool {
    discover_protocol == Some("grpc") && !frontend_http2_enabled
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
    gateway_index: &crate::runtime::store::GatewayInfoIndex,
) -> pingora_core::Result<bool> {
    let store = get_global_challenge_store();

    let Some((token, host)) = acme_challenge_token_and_host(session.req_header()) else {
        return Ok(false);
    };

    let Some(key_auth) = store.lookup_for_listener(
        &edgion_http.listener_key,
        gateway_index,
        token,
        host,
        ctx.request_info.hostname.as_str(),
    ) else {
        return Ok(false);
    };

    if !ctx.claim_final_response(200) {
        return Ok(true);
    }

    let mut resp = ResponseHeader::build(200, None)?;
    edgion_http.server_header_opts.apply_to_response(&mut resp);
    resp.insert_header("Content-Type", "text/plain")?;
    resp.insert_header("Content-Length", key_auth.len().to_string())?;

    ctx.request_info.trace_id.reconcile_response(&mut resp);
    session.write_response_header(Box::new(resp), false).await?;
    session.write_response_body(Some(key_auth), true).await?;
    ctx.record_info(EdgionInfoCode::AcmeChallengeServed);

    Ok(true)
}

fn acme_challenge_token_and_host(req_header: &RequestHeader) -> Option<(&str, &str)> {
    let token = match req_header.uri.path().strip_prefix(ACME_CHALLENGE_PREFIX) {
        Some(t) if !t.is_empty() => t,
        _ => return None,
    };

    let host = req_header
        .headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .map(host_without_port)
        .unwrap_or_default();

    Some((token, host))
}

fn apply_path_normalization_entry(
    ctx: &mut EdgionHttpContext,
    raw_path: &str,
    raw_query: Option<String>,
    cfg: &PathNormalizationConfig,
) -> Option<EdgionErrorCode> {
    ctx.request_info.entry_raw_path = raw_path.to_string();
    ctx.request_info.entry_raw_query = raw_query;

    if cfg.reject_escaped_slashes && contains_escaped_slash(raw_path) {
        return Some(EdgionErrorCode::PathEscapedSlashRejected);
    }

    if cfg.reject_invalid_percent_encoding && has_invalid_percent_encoding(raw_path) {
        return Some(EdgionErrorCode::PathInvalidPercentEncoding);
    }

    ctx.request_info.normalized_current_path = normalize_path(raw_path, cfg).unwrap_or_else(|| raw_path.to_string());

    None
}

/// Apply request-entry validation in wire-visible precedence order.
///
/// RFC 9110 §7.2 and RFC 9113 §8.3.1 require a valid Host / :authority for
/// HTTP and gRPC alike. Path rejection remains earlier because it also freezes
/// the raw URI view, while aggregate XFF rejection remains last so neither can
/// mask the more specific path or Host failure.
fn request_metadata_validation_error(
    ctx: &mut EdgionHttpContext,
    raw_path: &str,
    raw_query: Option<String>,
    path_norm_cfg: &PathNormalizationConfig,
    xff_too_long: bool,
) -> Option<EdgionErrorCode> {
    if let Some(status) = apply_path_normalization_entry(ctx, raw_path, raw_query, path_norm_cfg) {
        return Some(status);
    }
    if ctx.request_info.hostname.is_empty() {
        return Some(EdgionErrorCode::HostMissing);
    }
    xff_too_long.then_some(EdgionErrorCode::XffHeaderTooLong)
}

#[cfg(test)]
mod tests {
    use super::body_declared_snapshot_tests::session_from_request;
    use super::*;
    use edgion_resources::resources::LocalObjectReference;
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

        assert_eq!(status, Some(EdgionErrorCode::PathEscapedSlashRejected));
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

    #[test]
    fn cors_extension_ref_scan_skips_missing_ref_and_finds_later_config() {
        let filters = Some(vec![
            HTTPRouteFilter {
                filter_type: HTTPRouteFilterType::ExtensionRef,
                request_header_modifier: None,
                response_header_modifier: None,
                request_redirect: None,
                url_rewrite: None,
                request_mirror: None,
                cors: None,
                external_auth: None,
                extension_ref: None,
            },
            HTTPRouteFilter {
                filter_type: HTTPRouteFilterType::ExtensionRef,
                request_header_modifier: None,
                response_header_modifier: None,
                request_redirect: None,
                url_rewrite: None,
                request_mirror: None,
                cors: None,
                external_auth: None,
                extension_ref: Some(LocalObjectReference {
                    group: "edgion.io".to_string(),
                    kind: "EdgionPlugins".to_string(),
                    name: "cors".to_string(),
                    resolved_namespace: None,
                }),
            },
        ]);
        let mut runtime = crate::plugins::runtime::PluginRuntime::new();
        runtime.add_from_request_filters(
            &[edgion_resources::resources::RequestFilterEntry::new(
                edgion_resources::resources::EdgionPlugin::Cors(CorsConfig::default()),
            )],
            "default",
            "default/cors",
        );
        let runtime = Arc::new(runtime);
        let expected = runtime.cors_config_arc().expect("Cors config must be cached");

        let actual = extract_cors_config_from_route_filters(&filters, &[Some(runtime)]);

        assert!(actual.is_some_and(|config| Arc::ptr_eq(&config, &expected)));
    }

    #[tokio::test]
    async fn invalid_route_runtime_gate_terminates_options_before_preflight() {
        let filters: Vec<HTTPRouteFilter> = serde_json::from_value(serde_json::json!([
            {"type": "ExtensionRef"}
        ]))
        .expect("HTTP filter fixture must deserialize");
        let runtime = crate::plugins::runtime::PluginRuntime::from_httproute_filters_for_rule(
            &filters,
            "default",
            &Default::default(),
        );
        let mut session = session_from_request(
            b"OPTIONS / HTTP/1.1\r\nHost: test\r\nOrigin: https://app.example\r\nAccess-Control-Request-Method: GET\r\n\r\n",
        )
        .await;
        let mut ctx = EdgionHttpContext::default();

        runtime.run_preflight_gate(&mut session, &mut ctx).await;

        assert!(ctx.plugin_terminated());
    }
}

#[cfg(test)]
mod xff_snapshot_tests {
    use super::*;
    use crate::backends::degrade_threshold::GLOBAL_STORE_TEST_LOCK;
    use crate::observe::AccessLogger;
    use crate::routes::grpc::GrpcPortRouteManager;
    use crate::routes::http::routes_mgr::HttpPortRouteManager;
    use crate::util::RealIpExtractor;
    use edgion_resources::path_normalization::PathNormalizationConfig;
    use edgion_resources::resources::edgion_config_data::IpGroup;
    use edgion_resources::{EdgionGatewayConfig, ListenerKey};
    use http::HeaderValue;
    use serde_json::json;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

    fn request_with_xff(values: &[HeaderValue]) -> RequestHeader {
        let mut request = RequestHeader::build("GET", b"/", None).unwrap();
        request.insert_header("host", "example.test").unwrap();
        for value in values {
            request.append_header("x-forwarded-for", value).unwrap();
        }
        request
    }

    fn proxy(limit: usize) -> EdgionHttpProxy {
        let config: EdgionGatewayConfig = serde_json::from_value(json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionGatewayConfig",
            "metadata": {"name": "xff-test"},
            "spec": {"securityProtect": {"xForwardedForLimit": limit}}
        }))
        .unwrap();
        EdgionHttpProxy {
            listener_port: 80,
            is_https_listener: false,
            listener_key: ListenerKey("127.0.0.1:80/HTTP".to_string()),
            server_header_opts: Default::default(),
            access_logger: Arc::new(AccessLogger::default()),
            edgion_gateway_config: Arc::new(config),
            http_request_config_name: None,
            parsed_timeouts: Default::default(),
            enable_http2: false,
            real_ip_extractor: None,
            preflight_handler: super::super::PreflightHandler,
            http_route_manager: Arc::new(HttpPortRouteManager::default()),
            grpc_route_manager: Arc::new(GrpcPortRouteManager::default()),
        }
    }

    async fn session_from_request(raw_request: &[u8]) -> (Session, DuplexStream) {
        let (mut client, server) = tokio::io::duplex(4096);
        client.write_all(raw_request).await.unwrap();
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        assert!(session.read_request().await.unwrap());
        (session, client)
    }

    fn trusted_proxy_extractor(header: &str) -> RealIpExtractor {
        RealIpExtractor::new(
            &[IpGroup {
                name: "trusted".to_string(),
                description: None,
                cidrs: vec![
                    "0.0.0.0/32".to_string(),
                    "192.168.0.0/16".to_string(),
                    "10.0.0.0/8".to_string(),
                ],
            }],
            header.to_string(),
            true,
            None,
        )
        .unwrap()
    }

    #[test]
    fn snapshot_handles_missing_single_repeated_and_empty_field_lines() {
        let missing = request_with_xff(&[]);
        let snapshot = snapshot_x_forwarded_for(&missing, Some(200));
        assert_eq!(snapshot.value, None);
        assert!(!snapshot.too_long);

        let single = request_with_xff(&[HeaderValue::from_static("203.0.113.7")]);
        let snapshot = snapshot_x_forwarded_for(&single, Some(200));
        assert_eq!(snapshot.value.as_deref(), Some("203.0.113.7"));
        assert!(!snapshot.too_long);

        let repeated = request_with_xff(&[
            HeaderValue::from_static(""),
            HeaderValue::from_static("203.0.113.7"),
            HeaderValue::from_static("10.0.0.5"),
            HeaderValue::from_static(""),
        ]);
        let snapshot = snapshot_x_forwarded_for(&repeated, Some(200));
        assert_eq!(snapshot.value.as_deref(), Some(", 203.0.113.7, 10.0.0.5, "));
        assert!(!snapshot.too_long);
    }

    #[test]
    fn snapshot_limit_counts_separators_and_invalid_lines_cannot_bypass_it() {
        let repeated = request_with_xff(&[HeaderValue::from_static("a"), HeaderValue::from_static("bc")]);
        let exact = snapshot_x_forwarded_for(&repeated, Some(5));
        assert_eq!(exact.value.as_deref(), Some("a, bc"));
        assert!(!exact.too_long);
        let over = snapshot_x_forwarded_for(&repeated, Some(4));
        assert_eq!(over.value, None);
        assert!(over.too_long);

        let invalid = request_with_xff(&[
            HeaderValue::from_bytes(b"\xff").unwrap(),
            HeaderValue::from_static("12345"),
        ]);
        let invalid_exact = snapshot_x_forwarded_for(&invalid, Some(8));
        assert_eq!(invalid_exact.value, None);
        assert!(!invalid_exact.too_long);
        let invalid_over = snapshot_x_forwarded_for(&invalid, Some(7));
        assert_eq!(invalid_over.value, None);
        assert!(invalid_over.too_long);
    }

    #[test]
    fn configured_xff_is_case_insensitive_while_custom_header_keeps_its_path() {
        let mut request = request_with_xff(&[
            HeaderValue::from_static("203.0.113.7"),
            HeaderValue::from_static("10.0.0.5"),
        ]);
        request.insert_header("x-client-ip", "198.51.100.9").unwrap();
        let snapshot = snapshot_x_forwarded_for(&request, Some(200));

        let xff = trusted_proxy_extractor("X-FoRwArDeD-FoR");
        assert!(xff.real_ip_header().eq_ignore_ascii_case("x-forwarded-for"));
        assert_eq!(
            xff.resolve_real_ip("192.168.1.4:8080", snapshot.value.as_deref())
                .remote_ip,
            "203.0.113.7"
        );

        let custom = trusted_proxy_extractor("X-Client-IP");
        assert!(!custom.real_ip_header().eq_ignore_ascii_case("x-forwarded-for"));
        assert_eq!(custom.extract_real_ip("192.168.1.4:8080", &request), "198.51.100.9");

        let invalid = request_with_xff(&[HeaderValue::from_bytes(b"\xff").unwrap()]);
        let snapshot = snapshot_x_forwarded_for(&invalid, Some(200));
        assert_eq!(
            xff.resolve_real_ip("192.168.1.4:8080", snapshot.value.as_deref())
                .remote_ip,
            "192.168.1.4"
        );
    }

    #[tokio::test]
    async fn duplicate_host_precedes_xff_rejection_without_publishing_xff_metadata() {
        let (mut session, _client) =
            session_from_request(b"GET / HTTP/1.1\r\nHost: first.test\r\nX-Forwarded-For: 12345\r\n\r\n").await;
        session.req_header_mut().append_header("host", "second.test").unwrap();
        let mut ctx = EdgionHttpContext::default();

        assert!(build_request_metadata(&proxy(4), &mut session, &mut ctx).await.unwrap());
        assert!(ctx.diagnostics.contains_error(EdgionErrorCode::DuplicateHostHeader));
        assert!(!ctx.diagnostics.contains_error(EdgionErrorCode::XffHeaderTooLong));
        assert_eq!(ctx.request_info.x_forwarded_for, None);
    }

    #[tokio::test]
    async fn duplicate_host_gate_uses_the_request_pinned_egc_generation() {
        let (proxy, mut pinned_true, mut updated_false) = {
            let _guard = GLOBAL_STORE_TEST_LOCK.lock();
            let handler = crate::config::edgion_gateway::create_edgion_gateway_config_handler();
            let config = |reject_duplicate_host| {
                serde_json::from_value::<EdgionGatewayConfig>(json!({
                    "apiVersion": "edgion.io/v1",
                    "kind": "EdgionGatewayConfig",
                    "metadata": {"name": "duplicate-host-policy"},
                    "spec": {"securityProtect": {"rejectDuplicateHost": reject_duplicate_host}}
                }))
                .unwrap()
            };
            handler.full_set(&HashMap::from([(
                "duplicate-host-policy".to_string(),
                Arc::new(config(true)),
            )]));
            let mut proxy = proxy(200);
            proxy.http_request_config_name = Some("duplicate-host-policy".to_string());
            let pinned_true = crate::routes::http::proxy_http::pg_new_ctx::new_ctx(&proxy);

            handler.partial_update(
                HashMap::new(),
                HashMap::from([("duplicate-host-policy".to_string(), Arc::new(config(false)))]),
                HashSet::new(),
            );
            let updated_false = crate::routes::http::proxy_http::pg_new_ctx::new_ctx(&proxy);
            handler.full_set(&HashMap::new());
            (proxy, pinned_true, updated_false)
        };

        let (mut rejected, mut rejected_wire) =
            session_from_request(b"GET / HTTP/1.1\r\nHost: first.test\r\n\r\n").await;
        rejected.req_header_mut().append_header("host", "second.test").unwrap();
        assert!(build_request_metadata(&proxy, &mut rejected, &mut pinned_true)
            .await
            .unwrap());
        assert!(pinned_true
            .diagnostics
            .contains_error(EdgionErrorCode::DuplicateHostHeader));
        drop(rejected);
        let mut response = String::new();
        rejected_wire.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 400"));

        let (mut allowed, _allowed_wire) = session_from_request(b"GET / HTTP/1.1\r\nHost: first.test\r\n\r\n").await;
        allowed.req_header_mut().append_header("host", "second.test").unwrap();
        assert!(!build_request_metadata(&proxy, &mut allowed, &mut updated_false)
            .await
            .unwrap());
        assert!(!updated_false
            .diagnostics
            .contains_error(EdgionErrorCode::DuplicateHostHeader));
        assert_eq!(updated_false.request_info.hostname.as_str(), "first.test");
    }

    #[tokio::test]
    async fn missing_host_precedes_xff_rejection() {
        let (mut session, _client) = session_from_request(b"GET / HTTP/1.1\r\nX-Forwarded-For: 12345\r\n\r\n").await;
        let mut ctx = EdgionHttpContext::default();

        assert!(build_request_metadata(&proxy(4), &mut session, &mut ctx).await.unwrap());
        assert!(ctx.diagnostics.contains_error(EdgionErrorCode::HostMissing));
        assert!(!ctx.diagnostics.contains_error(EdgionErrorCode::XffHeaderTooLong));
        assert_eq!(ctx.request_info.x_forwarded_for, None);
    }

    #[test]
    fn production_validation_seam_prioritizes_path_over_pending_xff_rejection() {
        let request = request_with_xff(&[HeaderValue::from_static("12345")]);
        let snapshot = snapshot_x_forwarded_for(&request, Some(4));
        assert!(snapshot.too_long);

        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.hostname = NormalizedHostname::new("example.test");
        let error = request_metadata_validation_error(
            &mut ctx,
            "/admin%2Fsecret",
            None,
            &PathNormalizationConfig {
                reject_escaped_slashes: true,
                ..Default::default()
            },
            snapshot.too_long,
        );
        assert_eq!(error, Some(EdgionErrorCode::PathEscapedSlashRejected));
    }

    #[tokio::test]
    async fn invalid_under_limit_xff_falls_back_and_upstream_replaces_it_with_peer() {
        let (mut session, _client) = session_from_request(b"GET / HTTP/1.1\r\nHost: example.test\r\n\r\n").await;
        session
            .req_header_mut()
            .append_header("x-forwarded-for", HeaderValue::from_bytes(b"\xff").unwrap())
            .unwrap();
        let mut proxy = proxy(200);
        proxy.real_ip_extractor = Some(Arc::new(trusted_proxy_extractor("X-Forwarded-For")));
        let mut ctx = EdgionHttpContext::default();

        assert!(!build_request_metadata(&proxy, &mut session, &mut ctx).await.unwrap());
        assert_eq!(ctx.request_info.remote_ip, "0.0.0.0");
        assert_eq!(ctx.request_info.x_forwarded_for, None);
        assert!(!ctx.diagnostics.contains_error(EdgionErrorCode::XffHeaderTooLong));

        let expected_peer = ctx.request_info.downstream_peer_ip.clone();
        let mut upstream = session.req_header().clone();
        super::super::pg_upstream_request_filter::upstream_request_filter(
            &proxy,
            &mut session,
            &mut upstream,
            &mut ctx,
        )
        .await
        .unwrap();
        assert_eq!(
            upstream.headers.get("x-forwarded-for").unwrap().to_str().unwrap(),
            expected_peer
        );
    }

    #[tokio::test]
    async fn invalid_overlimit_xff_is_rejected_and_valid_repeated_xff_reaches_upstream() {
        let (mut invalid_session, _client) =
            session_from_request(b"GET / HTTP/1.1\r\nHost: example.test\r\n\r\n").await;
        invalid_session
            .req_header_mut()
            .append_header("x-forwarded-for", HeaderValue::from_bytes(b"\xff").unwrap())
            .unwrap();
        invalid_session
            .req_header_mut()
            .append_header("x-forwarded-for", "12345")
            .unwrap();
        let mut invalid_ctx = EdgionHttpContext::default();
        assert!(
            build_request_metadata(&proxy(7), &mut invalid_session, &mut invalid_ctx)
                .await
                .unwrap()
        );
        assert!(invalid_ctx
            .diagnostics
            .contains_error(EdgionErrorCode::XffHeaderTooLong));
        assert_eq!(invalid_ctx.request_info.x_forwarded_for, None);

        let (mut session, _client) = session_from_request(
            b"GET / HTTP/1.1\r\nHost: example.test\r\nX-Forwarded-For: 203.0.113.7\r\nX-Forwarded-For: 10.0.0.5\r\n\r\n",
        )
        .await;
        let proxy = proxy(200);
        let mut ctx = EdgionHttpContext::default();
        assert!(!build_request_metadata(&proxy, &mut session, &mut ctx).await.unwrap());
        assert_eq!(
            ctx.request_info.x_forwarded_for.as_deref(),
            Some("203.0.113.7, 10.0.0.5")
        );

        let mut upstream = session.req_header().clone();
        super::super::pg_upstream_request_filter::upstream_request_filter(
            &proxy,
            &mut session,
            &mut upstream,
            &mut ctx,
        )
        .await
        .unwrap();
        let expected = format!("203.0.113.7, 10.0.0.5, {}", ctx.request_info.downstream_peer_ip);
        assert_eq!(
            upstream.headers.get("x-forwarded-for").unwrap().to_str().unwrap(),
            expected
        );
    }
}

/// Coverage for `snapshot_body_declared`, the `body_declared` snapshot taken
/// at the top of `build_request_metadata`.
///
/// These focused framing tests do not need the full proxy fixture. The separate
/// `request_entry_tests` below construct a proxy and exercise the complete early
/// hook/module/request-filter sequence for routing and protocol boundaries.
///
/// This drives a real `pingora_proxy::Session` off real HTTP/1.1
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

    /// The `method_is_head` half of the snapshot, off real H1 request bytes:
    /// the response-side admission predicate reads this field instead of the
    /// plugin-mutable live header, so an inverted comparison here would put the
    /// hole straight back (see `RequestInfo::method_is_head`).
    #[tokio::test]
    async fn head_request_snapshots_method_is_head() {
        let mut session = session_from_request(b"HEAD /status HTTP/1.1\r\nHost: test\r\n\r\n").await;
        let mut ctx = EdgionHttpContext::default();

        snapshot_body_declared(&mut session, &mut ctx);

        assert!(ctx.request_info.method_is_head);
    }

    #[tokio::test]
    async fn non_head_request_snapshots_method_is_head_false() {
        let mut session = session_from_request(b"GET /status HTTP/1.1\r\nHost: test\r\n\r\n").await;
        let mut ctx = EdgionHttpContext::default();

        snapshot_body_declared(&mut session, &mut ctx);

        assert!(!ctx.request_info.method_is_head);
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
    resolved: &[Option<Arc<crate::plugins::runtime::PluginRuntime>>],
) -> Option<Arc<CorsConfig>> {
    let filters = filters.as_ref()?;
    let mut extension_index = 0;

    for filter in filters {
        if filter.filter_type != HTTPRouteFilterType::ExtensionRef {
            continue;
        }

        let Some(ext_ref) = filter.extension_ref.as_ref() else {
            continue;
        };
        let runtime = resolved.get(extension_index).and_then(Option::as_ref);
        extension_index += 1;

        // Check if it's EdgionPlugins
        if !is_supported_edgion_plugins_ref(ext_ref) {
            continue;
        }

        let cors = runtime.and_then(|runtime| runtime.cors_config_arc());
        if cors.is_some() {
            return cors;
        }
    }

    None
}

pub(super) fn capture_tls_metadata(session: &Session, ctx: &mut EdgionHttpContext) {
    // Extract TLS connection id (for correlating tls.log and access.log)
    if let Some(digest) = session.as_downstream().digest() {
        if let Some(ssl_digest) = digest.ssl_digest.as_ref() {
            if let Some(meta) = ssl_digest.extension.get::<TlsConnMeta>() {
                // Extract Gateway TLS listener info for 421 Misdirected Request detection.
                // Only set for Gateway TLS (not EdgionTls, which has no listener concept).
                let (matched_listener, matched_gateway_key) = meta
                    .matched
                    .as_ref()
                    .filter(|matched| matched.rsc.kind == "Gateway")
                    .map(|matched| {
                        (
                            matched.section.clone(),
                            Some(format_gateway_key(&matched.rsc.ns, &matched.rsc.name)),
                        )
                    })
                    .unwrap_or((None, None));
                ctx.request_info.install_tls_info(
                    meta.tls_id.clone(),
                    meta.sni.clone(),
                    meta.client_cert_info.clone(),
                    matched_listener,
                    matched_gateway_key,
                );
            } else if let Some(tls_id) = ssl_digest.extension.get::<TlsConnId>() {
                // Backward compatibility for connections created before metadata upgrade.
                ctx.request_info.set_tls_id(tls_id.0.to_string());
            }
        }
    }
}

#[cfg(test)]
mod request_entry_tests {
    use super::*;
    use crate::plugins::publication::inputs::{GatewayInput, RouteInput, RouteKind};
    use crate::plugins::publication::{GatewayScope, PluginPublisher};
    use crate::routes::grpc::{
        parse_route_to_units_for_test, DomainGrpcRouteRules, GrpcMatchEngine, GrpcPortRouteManager, GrpcRouteRules,
    };
    use crate::routes::http::conf_handler_impl::build_domain_route_rules_from_routes;
    use crate::routes::http::routes_mgr::{DomainRouteRules, HttpPortRouteManager};
    use crate::runtime::store::listener_gateway_info::{ListenerGatewayInfoStore, TestListenerIndexGuard};
    use edgion_resources::{
        EdgionGatewayConfig, GRPCRoute, Gateway, GatewayClass, HTTPRoute, ListenerKey, ResourceMeta,
    };
    use pingora_core::modules::http::grpc_web::GrpcWeb;
    use pingora_core::modules::http::HttpModules;
    use serde_json::json;
    use std::collections::HashMap;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
    use tokio::time::timeout;

    const NAMESPACE: &str = "request-entry-test";
    const OWNER: &str = "request-entry-test/gateway";
    const HTTP_ROUTE: &str = "request-entry-test/http-fallback";

    struct Fixture {
        proxy: EdgionHttpProxy,
        owner: Arc<GatewayScope>,
        _index: TestListenerIndexGuard,
    }

    impl Fixture {
        fn new(https: bool, http2: bool, global_421: bool, annotation_421: bool, grpc: bool) -> Self {
            let protocol = if https { "HTTPS" } else { "HTTP" };
            let listener_key = ListenerKey::new("0.0.0.0:18479".parse().unwrap(), protocol);
            let gateway: Gateway = serde_json::from_value(json!({
                "apiVersion":"gateway.networking.k8s.io/v1", "kind":"Gateway",
                "metadata":{"name":"gateway", "namespace":NAMESPACE,
                    "annotations":{"edgion.io/require-sni-host-match":annotation_421.to_string()}},
                "spec":{"gatewayClassName":"request-entry-class", "listeners":[
                    {"name":"sni", "port":18479, "protocol":protocol, "hostname":"sni.example.net"},
                    {"name":"host", "port":18479, "protocol":protocol, "hostname":"*.example.com"}
                ]}
            }))
            .unwrap();
            let config: EdgionGatewayConfig = serde_json::from_value(json!({
                "apiVersion":"edgion.io/v1", "kind":"EdgionGatewayConfig",
                "metadata":{"name":"request-entry-config"},
                "spec":{"securityProtect":{"requireSniHostMatch":global_421}}
            }))
            .unwrap();
            let class: GatewayClass = serde_json::from_value(json!({
                "apiVersion":"gateway.networking.k8s.io/v1", "kind":"GatewayClass",
                "metadata":{"name":"request-entry-class"},
                "spec":{"controllerName":"edgion.io/gateway-controller", "parametersRef":{
                    "group":"edgion.io", "kind":"EdgionGatewayConfig", "name":"request-entry-config"
                }}
            }))
            .unwrap();
            let mut route: HTTPRoute = serde_json::from_value(json!({
                "apiVersion":"gateway.networking.k8s.io/v1", "kind":"HTTPRoute",
                "metadata":{"name":"http-fallback", "namespace":NAMESPACE},
                "spec":{"resolvedStatusController": "edgion.io/gateway-controller", "parentRefs":[{"name":"gateway", "sectionName":"host"}],
                    "hostnames":["*.example.com"], "rules":[{
                        "matches":[{"path":{"type":"PathPrefix", "value":"/"}}],
                        "filters":[{"type":"RequestHeaderModifier", "requestHeaderModifier":{
                            "set":[{"name":"x-entry-http-route", "value":"selected"}]
                        }}],
                        "backendRefs":[{"name":"unused-backend", "port":8080}]
                    }]}
            }))
            .unwrap();
            route.spec.resolved_listeners = Some(vec![listener_key.clone()]);
            route.pre_parse();
            let route = Arc::new(route);
            let config = Arc::new(config);
            let publisher = PluginPublisher::new();
            publisher.replace_gateway_classes(HashMap::from([("request-entry-class".into(), Arc::new(class))]));
            publisher.replace_gateway_configs(HashMap::from([("request-entry-config".into(), config.clone())]));
            publisher.replace_gateways(HashMap::from([(OWNER.into(), GatewayInput::from_gateway(&gateway))]));
            publisher.replace_routes(
                RouteKind::Http,
                HashMap::from([(HTTP_ROUTE.into(), RouteInput::from_http(route.clone()))]),
            );
            let owner = publisher.snapshot().gateway(OWNER).unwrap();
            assert!(owner.scope.available && owner.scope.ai_rule(&route, 0));

            let http_route_manager = Arc::new(HttpPortRouteManager::default());
            http_route_manager.store_route_table(build_domain_route_rules_from_routes(
                &HashMap::from([(HTTP_ROUTE.into(), route)]),
                &DomainRouteRules::new(),
            ));
            let grpc_route_manager = Arc::new(GrpcPortRouteManager::default());
            if grpc {
                let mut route: GRPCRoute = serde_json::from_value(json!({
                    "apiVersion":"gateway.networking.k8s.io/v1", "kind":"GRPCRoute",
                    "metadata":{"name":"grpc-existing", "namespace":NAMESPACE},
                    "spec":{"resolvedStatusController": "edgion.io/gateway-controller", "parentRefs":[{"name":"gateway", "sectionName":"host"}],
                        "hostnames":["*.example.com"], "rules":[{
                            "matches":[{"method":{"service":"svc", "method":"Existing"}}],
                            "backendRefs":[{"name":"unused-backend", "port":8080}]
                        }]}
                }))
                .unwrap();
                route.spec.resolved_listeners = Some(vec![listener_key.clone()]);
                route.pre_parse();
                let units = parse_route_to_units_for_test("request-entry-test/grpc-existing", &Arc::new(route));
                assert_eq!(units.len(), 1, "the unmatched table must contain a real compiled Route");
                let rules = GrpcRouteRules {
                    resource_keys: units.iter().map(|unit| unit.resource_key.clone()).collect(),
                    match_engine: Some(Arc::new(GrpcMatchEngine::new(units.clone()))),
                    route_rules_list: units,
                };
                grpc_route_manager.store_route_table(DomainGrpcRouteRules {
                    grpc_routes: Arc::new(rules),
                });
            }
            let store = ListenerGatewayInfoStore::new();
            store.rebuild(&[gateway]);
            let index = store.get_index(&listener_key);
            assert!(index.get(OWNER, "host").is_some());
            let index = TestListenerIndexGuard::install(listener_key.clone(), index);
            let proxy = EdgionHttpProxy {
                listener_port: 18479,
                is_https_listener: https,
                listener_key,
                server_header_opts: Default::default(),
                access_logger: Arc::new(crate::observe::AccessLogger::default()),
                edgion_gateway_config: config,
                http_request_config_name: None,
                parsed_timeouts: Default::default(),
                enable_http2: http2,
                real_ip_extractor: None,
                preflight_handler: super::super::PreflightHandler,
                http_route_manager,
                grpc_route_manager,
            };
            Self {
                proxy,
                owner,
                _index: index,
            }
        }

        fn context(&self) -> EdgionHttpContext {
            let mut ctx = EdgionHttpContext::new();
            // The actual early hook still initializes IDs. Only its owner input
            // is local, so parallel tests cannot replace this policy generation.
            ctx.capture_plugin_scope(self.owner.clone());
            ctx
        }
    }

    async fn session(content_type: &str) -> (Session, DuplexStream) {
        let (mut client, server) = tokio::io::duplex(16 * 1024);
        let mut modules = HttpModules::new();
        modules.add_module(Box::new(GrpcWeb));
        let mut session = Session::new_h1_with_modules(Box::new(server), &modules);
        let request = format!(
            "POST /svc/Missing HTTP/1.1\r\nHost: A.B.EXAMPLE.COM.:18479\r\nContent-Type: {content_type}\r\nContent-Length: 0\r\n\r\n"
        );
        timeout(Duration::from_secs(2), client.write_all(request.as_bytes()))
            .await
            .unwrap()
            .unwrap();
        assert!(timeout(Duration::from_secs(2), session.read_request())
            .await
            .unwrap()
            .unwrap());
        (session, client)
    }

    async fn run_entry(fixture: &Fixture, session: &mut Session, ctx: &mut EdgionHttpContext) -> bool {
        timeout(Duration::from_secs(2), async {
            super::super::pg_early_request_filter::early_request_filter(&fixture.proxy, session, ctx)
                .await
                .unwrap();
            session
                .downstream_modules_ctx
                .request_header_filter(session.downstream_session.req_header_mut())
                .await
                .unwrap();
            request_filter(&fixture.proxy, session, ctx).await.unwrap()
        })
        .await
        .expect("in-memory entry must complete within two seconds")
    }

    fn assert_http_fallback(session: &Session, ctx: &EdgionHttpContext) {
        assert!(!ctx.is_grpc_route_matched);
        assert_eq!(ctx.route_unit.as_ref().unwrap().resource_key, HTTP_ROUTE);
        assert_eq!(
            session.req_header().headers.get("x-entry-http-route").unwrap(),
            "selected"
        );
        assert!(session.response_written().is_none());
        assert!(!ctx.diagnostics.contains_error(EdgionErrorCode::Http2Required));
        assert!(!ctx.diagnostics.contains_error(EdgionErrorCode::RouteNotFound));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn actual_entry_preserves_421_global_annotation_and_disabled_cases() {
        for (global, annotation, matched_listener, expected_reject) in [
            (true, false, "sni", true),
            (false, true, "sni", true),
            (false, false, "sni", false),
            (true, false, "host", false),
        ] {
            let fixture = Fixture::new(true, true, global, annotation, false);
            let (mut session, _client) = session("application/json").await;
            let mut ctx = fixture.context();
            // Install the existing handshake metadata representation, not a TLS
            // handshake or certificate test. The H1 stream has no SSL digest.
            let sni = if matched_listener == "sni" {
                "sni.example.net"
            } else {
                "a.b.example.com"
            };
            ctx.request_info.install_tls_info(
                None,
                Some(sni.into()),
                None,
                Some(matched_listener.into()),
                Some(OWNER.into()),
            );
            assert_eq!(run_entry(&fixture, &mut session, &mut ctx).await, expected_reject);
            assert_eq!(ctx.request_info.hostname.as_ref(), "a.b.example.com");
            assert_eq!(
                ctx.diagnostics.contains_error(EdgionErrorCode::SniHostMismatch),
                expected_reject
            );
            if expected_reject {
                assert_eq!(session.response_written().unwrap().status.as_u16(), 421);
                assert!(ctx.route_unit.is_none(), "421 must precede Route selection");
            } else {
                assert_http_fallback(&session, &ctx);
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn actual_entry_native_grpc_frontend_gate_and_unmatched_status_12() {
        // These are real H1 Sessions testing the configured frontend gate and
        // entry dispatch. They do not establish H2 negotiation/frame behavior.
        for (http2, grpc_routes, expected_status) in
            [(false, true, Some(13)), (true, true, Some(12)), (true, false, None)]
        {
            let fixture = Fixture::new(false, http2, false, false, grpc_routes);
            let (mut session, mut client) = session("application/grpc").await;
            let mut ctx = fixture.context();
            assert_eq!(
                run_entry(&fixture, &mut session, &mut ctx).await,
                expected_status.is_some()
            );
            assert_eq!(ctx.request_info.discover_protocol.as_deref(), Some("grpc"));
            assert_eq!(ctx.diagnostics.contains_error(EdgionErrorCode::Http2Required), !http2);
            if let Some(status) = expected_status {
                assert_eq!(ctx.request_info.grpc_status(), Some(status));
                let response = session.response_written().unwrap();
                assert_eq!(response.status.as_u16(), 200);
                assert_eq!(
                    response.headers.get("grpc-status").unwrap().to_str().unwrap(),
                    status.to_string()
                );
                assert!(
                    ctx.route_unit.is_none(),
                    "native rejection must not run the HTTP fallback"
                );
                assert_eq!(ctx.diagnostics.contains_error(EdgionErrorCode::RouteNotFound), http2);
                drop(session);
                let mut wire = Vec::new();
                timeout(
                    Duration::from_secs(2),
                    (&mut client).take(16 * 1024).read_to_end(&mut wire),
                )
                .await
                .unwrap()
                .unwrap();
                let wire = String::from_utf8(wire).unwrap().to_ascii_lowercase();
                assert!(wire.starts_with("http/1.1 200"));
                assert!(wire.contains(&format!("grpc-status: {status}\r\n")));
            } else {
                assert_http_fallback(&session, &ctx);
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn actual_entry_grpc_web_keeps_http_fallback_after_module_rewrite() {
        for http2 in [false, true] {
            let fixture = Fixture::new(false, http2, false, false, true);
            let (mut session, _client) = session("application/grpc-web+proto").await;
            let mut ctx = fixture.context();
            assert!(!run_entry(&fixture, &mut session, &mut ctx).await);
            assert_eq!(ctx.request_info.discover_protocol.as_deref(), Some("grpc-web"));
            assert_eq!(
                session.req_header().headers.get("content-type").unwrap(),
                "application/grpc+proto"
            );
            assert_http_fallback(&session, &ctx);
        }
    }

    #[test]
    fn request_entry_index_override_restores_nested_state_on_unwind() {
        let key = ListenerKey::new("0.0.0.0:18479".parse().unwrap(), "HTTP");
        let store = get_listener_gateway_info_store();
        let baseline_empty = store.get_index(&key).is_empty();
        let fixture = Fixture::new(false, false, false, false, false);
        let outer = store.get_index(&key);
        assert!(outer.get(OWNER, "host").is_some());
        let empty = ListenerGatewayInfoStore::new().get_index(&ListenerKey("unused/HTTP".into()));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _nested = TestListenerIndexGuard::install(key.clone(), empty);
            assert!(store.get_index(&key).is_empty());
            panic!("exercise the test-only unwind restoration");
        }));
        assert!(result.is_err());
        assert!(Arc::ptr_eq(&outer, &store.get_index(&key)));
        drop(fixture);
        assert_eq!(store.get_index(&key).is_empty(), baseline_empty);
    }
}
