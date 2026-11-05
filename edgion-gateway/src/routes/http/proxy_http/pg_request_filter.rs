use super::{resolve_route_ext_refs, select_waf_owner, select_waf_owner_chain, EdgionHttpProxy};
use crate::ctx::NormalizedHostname;
use crate::plugins::PluginRuntime;
use crate::routes::grpc::{parse_grpc_timeout, try_match_grpc_route};
use crate::runtime::hostname_matches_listener;
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
use edgion_resources::resources::{CorsConfig, HTTPRouteFilterType};
use edgion_resources::{EdgionErrorCode, EdgionInfoCode};
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::Session;
use std::sync::Arc;

/// Unknown consumers require complete capture. Native route ExtensionRefs are
/// transparent only when every corresponding pinned runtime is available.
fn allows_partial_body_capture<'a>(
    global: &[Arc<PluginRuntime>],
    gateway: &[Arc<PluginRuntime>],
    route_refs: &[Option<Arc<PluginRuntime>>],
    route: &PluginRuntime,
    backends: impl Iterator<Item = &'a PluginRuntime>,
) -> bool {
    !route.requires_complete_request_body()
        && route.ext_ref_keys().len() == route_refs.len()
        && global
            .iter()
            .chain(gateway)
            .all(|runtime| !runtime.requires_complete_request_body())
        && route_refs.iter().all(|runtime| {
            runtime
                .as_ref()
                .is_some_and(|runtime| !runtime.requires_complete_request_body())
        })
        && !backends.into_iter().any(PluginRuntime::requires_complete_backend_body)
}

/// An Upgrade request remains an HTTP entity until an actual 101 is written.
fn finite_http_request_body(ctx: &EdgionHttpContext) -> bool {
    !ctx.request_info.method_is_connect() && !ctx.request_info.is_grpc_request && !ctx.is_grpc_route_matched
}

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
    if limit.is_some_and(|limit| total_len > limit) {
        return XffSnapshot {
            value: None,
            too_long: true,
        };
    }
    let first_text = first.to_str();
    let mut valid = first_text.is_ok();

    for value in iter {
        count += 1;
        let Some(len) = total_len.checked_add(2).and_then(|len| len.checked_add(value.len())) else {
            return XffSnapshot {
                value: None,
                too_long: true,
            };
        };
        if limit.is_some_and(|limit| len > limit) {
            return XffSnapshot {
                value: None,
                too_long: true,
            };
        }
        total_len = len;
        valid &= value.to_str().is_ok();
    }

    if !valid {
        return XffSnapshot {
            value: None,
            too_long: false,
        };
    }

    if count == 1 {
        let Ok(first) = first_text else {
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

    // Match using the tables and full Gateway index captured before early plugins.
    let publication = Arc::clone(
        ctx.listener_publication()
            .expect("early hook captures listener publication"),
    );
    let gateway_index = &publication.index;

    if try_serve_acme_challenge(edgion_http, session, ctx, gateway_index).await? {
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
                    let global_enabled = ctx.egc.require_sni_host_match;
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
        if native_grpc_requires_frontend_http2(ctx.request_info.discover_protocol, edgion_http.enable_http2) {
            ctx.record_error(EdgionErrorCode::Http2Required);
            end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
            return Ok(true);
        }

        let grpc_routes = publication.grpc_routes(&edgion_http.grpc_route_manager);
        let _ = try_match_grpc_route(&grpc_routes, session, ctx, gateway_index).await;

        // If GRPCRoutes exist on this port but none matched, return UNIMPLEMENTED
        // per Gateway API spec. Only fall through to HTTP routes when no GRPCRoutes
        // are configured (user may be routing gRPC via HTTPRoute).
        // Scoped to pure gRPC only — gRPC-web may be routed via HTTPRoute.
        if !ctx.is_grpc_route_matched && grpc_routes.has_routes() && ctx.request_info.discover_protocol == Some("grpc")
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
        let port_routes = publication.http_routes(&edgion_http.http_route_manager);
        match port_routes.match_route(session, ctx, gateway_index) {
            Ok(result) => install_match_result(ctx, result),
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
    // Ancestors of a delegated chain resolve their own ExtensionRefs in the
    // root-matched Gateway's scope. Ordinary requests skip this (one null check).
    if ctx.delegated.is_some() {
        install_delegated_ext_refs(ctx, |key| owner.scope.http_runtime(key));
    }
    // Later-stage restrictions already have a request admission gate. Run it
    // before AI validation, CORS, ordinary plugins, WAF or body access. Global
    // and Gateway request IDs may already have run in Pingora's early hook.
    let global = ctx.resolved_global_plugins();
    let gateway = ctx.resolved_gateway_plugins();
    let route = ctx.resolved_route_ext_refs();
    // Delegated chains only: every ancestor's resolved ExtensionRefs (root first).
    let ancestor_refs = ctx.delegated.as_ref().map(|d| d.ext_refs.clone());
    let invalid_http_route_scope = global
        .iter()
        .chain(gateway.iter())
        .flat_map(|p| p.iter())
        .any(|runtime| runtime.requires_http_route_rule_scope());

    if run_preflight_gates(
        session,
        ctx,
        global.as_deref().unwrap_or_default(),
        gateway.as_deref().unwrap_or_default(),
        ancestor_refs.as_deref().unwrap_or_default(),
        route.as_deref().unwrap_or_default(),
    )
    .await
    {
        let _ = terminate_with_fallback(session, ctx, &edgion_http.server_header_opts).await;
        return Ok(true);
    }
    if invalid_http_route_scope
        || route_owner_conflict(
            global.as_deref().unwrap_or_default(),
            gateway.as_deref().unwrap_or_default(),
            ancestor_refs.as_deref(),
            route.as_deref().unwrap_or_default(),
            ctx.grpc_route_unit.is_some(),
        )
    {
        end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
        return Ok(true);
    }
    if let Some(unit) = ctx.route_unit.as_ref() {
        if !unit.route.as_ref().is_some_and(|source| {
            owner.scope.ai_rule(source, unit.specificity.rule_index as usize)
                && owner.scope.graphql_rule(source, unit.specificity.rule_index as usize)
        }) {
            end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
            return Ok(true);
        }
    }
    // Delegated chains: every level is scope-checked with its own route and
    // rule index.
    if let Some(d) = ctx.delegated.as_deref() {
        let ancestors_in_scope = d.ancestors.iter().all(|unit| {
            unit.route.as_ref().is_some_and(|source| {
                owner.scope.ai_rule(source, unit.specificity.rule_index as usize)
                    && owner.scope.graphql_rule(source, unit.specificity.rule_index as usize)
            })
        });
        if !ancestors_in_scope {
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
        let has_native_cors = ctx.route_unit.as_deref().is_some_and(unit_has_native_cors)
            || ctx
                .delegated
                .as_ref()
                .is_some_and(|d| d.ancestors.iter().any(|unit| unit_has_native_cors(unit)));

        if !has_native_cors {
            // Get CORS config dynamically from matched route's EdgionPlugins
            // This ensures we always get the latest config even if EdgionPlugin is updated
            let route_ext_refs = ctx.resolved_route_ext_refs();
            let cors_config = if ctx.route_unit.is_some() {
                // Leaf first, then the nearest delegating ancestor (if any).
                extract_cors_config_from_route_filters(route_ext_refs.as_deref().unwrap_or_default()).or_else(|| {
                    ctx.delegated.as_ref().and_then(|d| {
                        d.ext_refs
                            .iter()
                            .rev()
                            .flatten()
                            .find_map(|refs| extract_cors_config_from_route_filters(refs))
                    })
                })
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

    let selected_waf = elect_request_waf(
        ctx,
        global_plugins.as_deref().unwrap_or_default(),
        gateway_plugins.as_deref().unwrap_or_default(),
        route_ext_refs.as_deref().unwrap_or_default(),
    );
    let plan_waf_body = selected_waf.is_some() && !ctx.request_info.is_grpc_request && !ctx.is_grpc_route_matched;
    if !ctx.select_waf(selected_waf) {
        end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
        return Ok(true);
    }

    ctx.enforce_request_body_limit = finite_http_request_body(ctx);
    if ctx.enforce_request_body_limit
        && ctx
            .request_info
            .declared_content_length
            .is_some_and(|length| length > ctx.request_body_config.max_body_size_bytes)
    {
        super::pg_request_body_filter_action::reject_request_body_limit(session, ctx).await?;
        return Ok(true);
    }

    let waf_partial_capture_allowed = plan_waf_body
        && ctx.route_unit.as_ref().is_some_and(|route| {
            allows_partial_body_capture(
                global_plugins.as_deref().unwrap_or_default(),
                gateway_plugins.as_deref().unwrap_or_default(),
                route_ext_refs.as_deref().unwrap_or_default(),
                &route.plugin_runtime,
                route.backends.iter().map(|backend| backend.plugin_runtime.as_ref()),
            )
        })
        && ctx.delegated.as_ref().is_none_or(|d| {
            d.ancestors.iter().enumerate().all(|(index, unit)| {
                allows_partial_body_capture(
                    global_plugins.as_deref().unwrap_or_default(),
                    gateway_plugins.as_deref().unwrap_or_default(),
                    d.ext_refs.get(index).cloned().flatten().as_deref().unwrap_or_default(),
                    &unit.plugin_runtime,
                    std::iter::empty(),
                )
            })
        });

    ctx.set_waf_partial_capture_allowed(waf_partial_capture_allowed);

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
    run_route_request_plugins(session, ctx).await;
    if ctx.plugin_terminated() {
        let _ = terminate_with_fallback(session, ctx, &edgion_http.server_header_opts).await;
        return Ok(true);
    }

    // Body policy is request-scoped. Global, Gateway, and route request
    // plugins have now had their one opportunity to capture, mutate, observe,
    // or install streamed processing. BackendRef request plugins run later
    // during peer selection and may consume an already-cached snapshot. Their
    // first-attempt dispatcher also permits a read-only mirror tee before the
    // body pump starts; it changes neither framing nor replay policy. Capture,
    // mutation and other body capability installation remain closed here.
    if ctx.has_strict_jump_conflict() {
        end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
        return Ok(true);
    }
    if ctx.grpc_transcoding.is_some() {
        ctx.ensure_target_synced(session.req_header_mut())
            .map_err(|_| crate::plugins::http::grpc_json_transcoder::failure())?;
    }
    crate::plugins::http::grpc_json_transcoder::validate_request(session.req_header(), ctx)?;
    ctx.request_info.in_request_body_stage = false;

    crate::response_cache::lifecycle::bind_request(session, ctx).await;
    respond_from_cache(edgion_http, session, ctx).await
}

/// Install a route match. `route_unit` always holds the leaf; a delegated
/// chain's ancestors go to `ctx.delegated` (`None` for every ordinary match).
#[inline]
pub(super) fn install_match_result(
    ctx: &mut EdgionHttpContext,
    result: crate::routes::http::match_unit::RouteMatchResult,
) {
    ctx.gateway_info = Some(result.matched_gateway);
    ctx.capture_matched_path_params(&result.route_unit.matched_info.m);
    ctx.route_unit = Some(result.route_unit);
    ctx.delegated = result.delegated;
}

/// Elect the request-wide WAF owner. A delegated chain elects level by level
/// over every ancestor's resolved ExtensionRefs (root first) and then the
/// leaf's (see `select_waf_owner_chain`); ordinary routes keep the flat
/// single-level election over the leaf array.
#[inline]
pub(super) fn elect_request_waf(
    ctx: &EdgionHttpContext,
    global: &[Arc<PluginRuntime>],
    gateway: &[Arc<PluginRuntime>],
    leaf_route_refs: &[Option<Arc<PluginRuntime>>],
) -> Option<crate::ctx::WafSelection> {
    match ctx.delegated.as_deref() {
        None => select_waf_owner(global, gateway, leaf_route_refs),
        Some(d) => select_waf_owner_chain(
            global,
            gateway,
            d.ext_refs
                .iter()
                .map(|refs| refs.as_deref().unwrap_or_default())
                .chain(std::iter::once(leaf_route_refs)),
        ),
    }
}

/// Resolve every ancestor's ExtensionRef keys (its own route namespace) into
/// per-level runtime arrays. Delegated chains only; the leaf's array is the
/// ordinary `resolved_route_ext_refs`.
pub(super) fn install_delegated_ext_refs(
    ctx: &mut EdgionHttpContext,
    mut lookup: impl FnMut(&str) -> Option<Arc<PluginRuntime>>,
) {
    let Some(d) = ctx.delegated.as_deref_mut() else {
        return;
    };
    let ext_refs = d
        .level_ext_ref_keys()
        .map(|keys| {
            (!keys.is_empty()).then(|| {
                Arc::from(super::global_plugins_resolve::resolve_route_ext_refs_with(
                    keys,
                    &mut lookup,
                ))
            })
        })
        .collect();
    d.ext_refs = ext_refs;
}

/// Run every preflight gate (policy block, invalid config/extension, IP
/// availability) root to leaf: Global, Gateway, then each delegated ancestor
/// level's ExtensionRef gates followed by that level's own rule gate, then the
/// leaf's ExtensionRef gates and finally the leaf rule gate. Returns `true`
/// when a gate terminated the request; the caller writes the fallback.
///
/// An ordinary route (no ancestors) keeps its order: Global, Gateway, leaf
/// ExtensionRefs, leaf rule.
pub(super) async fn run_preflight_gates(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    global: &[Arc<PluginRuntime>],
    gateway: &[Arc<PluginRuntime>],
    ancestor_refs: &[Option<crate::routes::delegation::descent::LevelExtRefs>],
    route: &[Option<Arc<PluginRuntime>>],
) -> bool {
    // Preflight gates render no templates and resolve no ExtensionRefs, so
    // ExtensionRef target runtimes run here without a level.
    for runtime in global.iter().chain(gateway.iter()) {
        runtime.run_preflight_gate(session, ctx).await;
        if ctx.plugin_terminated() {
            return true;
        }
    }
    // Delegated chains: each ancestor level, root first, before the leaf.
    for level in 0..ctx.ancestor_count() {
        let level_refs = ancestor_refs.get(level).and_then(Option::as_deref).unwrap_or_default();
        for runtime in level_refs.iter().flatten() {
            runtime.run_preflight_gate(session, ctx).await;
            if ctx.plugin_terminated() {
                return true;
            }
        }
        let Some(runtime) = ctx.ancestor_runtime(level) else {
            break;
        };
        runtime.run_preflight_gate_at(session, ctx, Some(level)).await;
        if ctx.plugin_terminated() {
            return true;
        }
    }
    for runtime in route.iter().flatten() {
        runtime.run_preflight_gate(session, ctx).await;
        if ctx.plugin_terminated() {
            return true;
        }
    }
    if let Some(runtime) = ctx.leaf_route_runtime() {
        runtime.run_preflight_gate(session, ctx).await;
        if ctx.plugin_terminated() {
            return true;
        }
    }
    false
}

/// Exclusive-owner rules for the matched route, evaluated over the whole
/// delegated chain (`ancestors`, root first, then the leaf's `route` refs):
/// at most one gRPC-JSON transcoder; a transcoder excludes AI owners and a
/// GRPCRoute; a GraphQL operation router excludes AI owners, transcoders and a
/// GRPCRoute. These are the ordinary-route rules unchanged; several AI plugins
/// (AiGuard, Guardrail, one AiProxy) may still coexist, and a second AiProxy is
/// refused at runtime by the request-wide AI plan. As defense in depth a
/// router may sit on only one level (Controller validation already rejects a
/// router on a delegating rule). Ordinary routes pass `ancestors = None`.
pub(super) fn route_owner_conflict(
    global: &[Arc<PluginRuntime>],
    gateway: &[Arc<PluginRuntime>],
    ancestors: Option<&[Option<crate::routes::delegation::descent::LevelExtRefs>]>,
    route: &[Option<Arc<PluginRuntime>>],
    grpc_route: bool,
) -> bool {
    let ancestor_runtimes = || {
        ancestors
            .into_iter()
            .flatten()
            .flatten()
            .flat_map(|refs| refs.iter().flatten())
    };
    let route_runtimes = || ancestor_runtimes().chain(route.iter().flatten());
    let conversion_owners = route_runtimes().filter(|runtime| runtime.has_grpc_transcoder()).count();
    let has_ai_owner = global
        .iter()
        .chain(gateway)
        .chain(route_runtimes())
        .any(|runtime| runtime.has_ai_conversion_owner());
    let has_router = route_runtimes().any(|runtime| runtime.has_graphql_operation_router());
    if conversion_owners > 1 || conversion_owners != 0 && (has_ai_owner || grpc_route) {
        return true;
    }
    if has_router && (has_ai_owner || conversion_owners != 0 || grpc_route) {
        return true;
    }
    let Some(ancestors) = ancestors else {
        return false;
    };
    let router_levels = ancestors
        .iter()
        .map(|refs| refs.as_deref().unwrap_or_default())
        .chain(std::iter::once(route))
        .filter(|refs| {
            refs.iter()
                .flatten()
                .any(|runtime| runtime.has_graphql_operation_router())
        })
        .count();
    router_levels > 1
}

/// Run the matched route's request-stage chain: for a delegated chain each
/// ancestor (root first, with its own level), then the leaf exactly as for an
/// ordinary route. Stops at the first level that terminates the request.
pub(super) async fn run_route_request_plugins(session: &mut Session, ctx: &mut EdgionHttpContext) {
    let Some(leaf) = ctx.leaf_route_runtime() else {
        return;
    };
    // Delegated chains (HTTPRoute or GRPCRoute): ancestors root first.
    for level in 0..ctx.ancestor_count() {
        let Some(runtime) = ctx.ancestor_runtime(level) else {
            break;
        };
        runtime.run_request_plugins_at(session, ctx, Some(level)).await;
        if ctx.plugin_terminated() {
            return;
        }
    }
    leaf.run_request_plugins(session, ctx).await;
}

fn unit_has_native_cors(unit: &crate::routes::http::HttpRouteRuleUnit) -> bool {
    unit.rule
        .filters
        .as_ref()
        .is_some_and(|filters| filters.iter().any(|f| f.filter_type == HTTPRouteFilterType::CORS))
}

/// The optional hit enters the same response onion and final-writer claim as an
/// origin response, without creating an upstream attempt.
async fn respond_from_cache(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<bool> {
    if let Some(mut hit) = crate::response_cache::lifecycle::prepare_hit(session, ctx) {
        super::pg_upstream_response_filter::cached_response_filter(edgion_http, session, &mut hit.header, ctx)?;
        super::pg_response_filter::response_filter(edgion_http, session, &mut hit.header, ctx).await?;
        super::pg_response_head::response_head_will_commit(session, &hit.header, ctx)?;
        crate::response_cache::lifecycle::deliver_hit(session, ctx, hit).await?;
        return Ok(true);
    }

    Ok(false)
}

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_request_filter_test/response_cache_tests.rs"]
mod response_cache_tests;

/// Build request metadata: client addresses, hostname, path, trace_id, x-forwarded-for, and protocol detection (inline for performance)
/// Also validates X-Forwarded-For header length against security_protect configuration.
/// Returns Ok(true) if response has been sent (due to validation failure), Ok(false) to continue.
#[inline]
async fn build_request_metadata(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<bool> {
    // Snapshot whether the client declared a body, before ordinary request plugin rewrites.
    // See `snapshot_body_declared` for why this must not be deferred.
    snapshot_body_declared(session, ctx);

    let req_header = session.req_header();
    let raw_path = req_header.uri.path();
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

    let xff_limit = ctx.egc.x_forwarded_for_limit;
    let xff_snapshot = snapshot_x_forwarded_for(req_header, xff_limit);
    let xff_too_long = xff_snapshot.too_long;

    ctx.request_info.remote_ip = if let Some(policy) = &ctx.egc.real_ip_policy {
        let view = crate::plugins::runtime::edgion_config_data::capture_global_edgion_config_data_snapshot();
        let header = if policy.real_ip_header().as_str() == "x-forwarded-for" {
            xff_snapshot.value.as_deref()
        } else {
            req_header
                .headers
                .get(policy.real_ip_header())
                .and_then(|value| value.to_str().ok())
        };
        let resolved = policy
            .resolve_in(&view, &ctx.request_info.downstream_peer_ip, header)
            .remote_ip;
        crate::ctx::canonicalize_ip(resolved)
    } else {
        // Transport extraction above already canonicalized this address (or returned empty).
        ctx.request_info.downstream_peer_ip.clone()
    };

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

    // Publish the bounded XFF snapshot after Pingora's protocol-level Host validation.
    ctx.request_info.x_forwarded_for = xff_snapshot.value;

    if let Some(status) = request_metadata_validation_error(ctx, raw_path, raw_query, path_norm_cfg, xff_too_long)
        .map_err(|_| super::pg_upstream_request_filter::local_pre_dispatch_failure())?
    {
        ctx.record_error(status);
        end_response_400(session, ctx, &edgion_http.server_header_opts).await?;
        return Ok(true); // Response sent
    }

    Ok(false) // Continue processing
}

/// Snapshot whether the client declared a request body, before ordinary request plugin rewrites.
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
/// `HEAD` through the canonical method, which response admission ANDs
/// with the live request header a request-stage plugin can rewrite via
/// `set_upstream_method`. Original CONNECT identity and declared length likewise
/// belong here for physical admission. These writes share the same reason:
/// this point follows early RequestId handling and request modules, but precedes
/// routing and ordinary request plugins that can rewrite the request.
fn snapshot_body_declared(session: &mut Session, ctx: &mut EdgionHttpContext) {
    ctx.request_info.method = Some(session.req_header().method.clone());
    ctx.request_info.body_declared = Some(!session.is_body_empty());
    ctx.request_info.in_request_body_stage = true;
    ctx.request_info.declared_content_length = session
        .req_header()
        .headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok());
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
                ctx.request_info.discover_protocol = Some("websocket");
            }
        }
    }

    // Check for gRPC/gRPC-Web (if not WebSocket)
    if ctx.request_info.discover_protocol.is_none() {
        if let Some(ct) = req_header.headers.get("content-type") {
            if let Ok(ct_str) = ct.to_str() {
                if super::is_grpc_web_content_type(ct_str) {
                    ctx.request_info.discover_protocol = Some("grpc-web");
                    ctx.request_info.is_grpc_request = true;
                } else if super::is_grpc_content_type(ct_str) {
                    ctx.request_info.discover_protocol = Some("grpc");
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
) -> Result<Option<EdgionErrorCode>, crate::request_query::QueryError> {
    ctx.request_query.capture(raw_query)?;
    ctx.request_info.entry_raw_path = raw_path.to_string();

    if cfg.reject_escaped_slashes && contains_escaped_slash(raw_path) {
        return Ok(Some(EdgionErrorCode::PathEscapedSlashRejected));
    }

    if cfg.reject_invalid_percent_encoding && has_invalid_percent_encoding(raw_path) {
        return Ok(Some(EdgionErrorCode::PathInvalidPercentEncoding));
    }

    ctx.request_info.normalized_current_path = normalize_path(raw_path, cfg).unwrap_or_else(|| raw_path.to_string());

    Ok(None)
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
) -> Result<Option<EdgionErrorCode>, crate::request_query::QueryError> {
    if let Some(status) = apply_path_normalization_entry(ctx, raw_path, raw_query, path_norm_cfg)? {
        return Ok(Some(status));
    }
    if ctx.request_info.hostname.is_empty() {
        return Ok(Some(EdgionErrorCode::HostMissing));
    }
    Ok(xff_too_long.then_some(EdgionErrorCode::XffHeaderTooLong))
}

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_request_filter_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_request_filter_test/xff_snapshot_tests.rs"]
mod xff_snapshot_tests;

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
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_request_filter_test/body_declared_snapshot_tests.rs"]
mod body_declared_snapshot_tests;

/// Coverage for `detect_request_protocol`'s `Upgrade` handling.
///
/// This is the property `session_adapter.rs`'s (now-removed) tautological
/// `request_admission_unknown_upgrade_token_buffers` test claimed to cover
/// but never actually exercised (it encoded no `Upgrade` header at all). The
/// real property lives here, at the source of `discover_protocol`: only an
/// exact `Upgrade: websocket` token sets it; an unrecognized token or a
/// comma-list containing `websocket` must not.
#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_request_filter_test/protocol_detection_tests.rs"]
mod protocol_detection_tests;

/// Extract CORS configuration dynamically from route filters
///
/// Walks the route's `ExtensionRef` filters that point at `EdgionPlugins`
/// resources and returns the first pre-resolved `Arc<CorsConfig>` cached on
/// the matched `PluginRuntime`. The cache is rebuilt on every `EdgionPlugins`
/// preparse, so hot-reload semantics are preserved while the preflight hot
/// path avoids deep-cloning the full `CorsConfig` per request.
fn extract_cors_config_from_route_filters(
    resolved: &[Option<Arc<crate::plugins::runtime::PluginRuntime>>],
) -> Option<Arc<CorsConfig>> {
    resolved.iter().flatten().find_map(|runtime| runtime.cors_config_arc())
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
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_request_filter_test/request_entry_tests.rs"]
mod request_entry_tests;

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_request_filter_test/delegation_tests.rs"]
mod delegation_tests;
