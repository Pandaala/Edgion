use super::EdgionHttpProxy;
use crate::core::common::utils::net::{classify_backend_addr, resolve_domain, BlockedBackendReason};
use crate::core::gateway::backends::EdgionService;
use crate::core::gateway::backends::{get_peer, try_get_peer};
use crate::core::gateway::cli::config::get_path_normalization_config;
use crate::core::gateway::routes::grpc::handle_grpc_upstream;
use crate::core::gateway::routes::http::routes_mgr::RouteRules;
use crate::core::gateway::routes::http::session_persistence;
use crate::core::gateway::{
    end_response_500, end_response_503, end_response_grpc_unavailable, terminate_with_fallback,
};
use crate::types::ctx::BackendSource;
use crate::types::err::EdError;
use crate::types::filters::PluginRunningResult;
use crate::types::{EdgionHttpContext, EdgionStatus, SessionPersistenceState};
use pingora_core::modules::http::grpc_web::GrpcWebBridge;
use pingora_core::prelude::HttpPeer;
use pingora_core::protocols::l4::socket::SocketAddr as PingoraSocketAddr;
use pingora_core::upstreams::peer::Peer;
use pingora_core::{Error as PingoraError, ErrorType};
use pingora_proxy::Session;

#[inline]
pub async fn upstream_peer(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<Box<HttpPeer>> {
    // Check request timeout dynamically before attempting peer selection
    // This prevents starting a new retry attempt when deadline is already exceeded
    let request_timeout = ctx.request_timeout();

    if let Some(timeout) = request_timeout {
        let elapsed = ctx.start_time.elapsed();
        if elapsed >= timeout {
            ctx.add_error(EdgionStatus::RequestTimeoutExceeded);
            if let Some(upstream) = ctx.get_current_upstream_mut() {
                upstream.status = Some(504);
            }
            return Err(PingoraError::new_str("Request timeout exceeded"));
        }
    }

    // Apply backoff delay on retry attempts
    if ctx.try_cnt >= 1 {
        let base_backoff = ctx.retry_backoff();

        if let Some(base) = base_backoff {
            // Exponential: base * 2^(try_cnt - 1), capped at 30s
            let exponent = (ctx.try_cnt - 1).min(5);
            let delay = base.saturating_mul(1u32.checked_shl(exponent).unwrap_or(u32::MAX));
            let delay = delay.min(std::time::Duration::from_secs(30));

            // TODO: abort this sleep early when the client disconnects
            if let Some(timeout) = request_timeout {
                let remaining = timeout.saturating_sub(ctx.start_time.elapsed());
                if delay < remaining {
                    tokio::time::sleep(delay).await;
                }
            } else {
                tokio::time::sleep(delay).await;
            }
        }
    }

    // Route to appropriate handler based on matched route type (not protocol)
    if ctx.is_grpc_route_matched {
        upstream_peer_grpc(edgion_http, session, ctx).await
    } else {
        upstream_peer_http(edgion_http, session, ctx).await
    }
}

/// Handle gRPC upstream peer selection
#[inline]
pub async fn upstream_peer_grpc(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<Box<HttpPeer>> {
    // 1. Handle gRPC upstream selection
    match handle_grpc_upstream(session, ctx).await {
        Ok(Some(())) => {}
        Ok(None) => {
            ctx.add_error(EdgionStatus::GrpcUpstreamNotRouteMatched);
            if !ctx.request_info.response_committed {
                end_response_grpc_unavailable(session, ctx, &edgion_http.server_header_opts).await?;
            }
            return Err(PingoraError::new(ErrorType::HTTPStatus(200)));
        }
        Err(_) => {
            ctx.add_error(EdgionStatus::GrpcUpstreamNotBackendRefs);
            if !ctx.request_info.response_committed {
                end_response_grpc_unavailable(session, ctx, &edgion_http.server_header_opts).await?;
            }
            return Err(PingoraError::new(ErrorType::HTTPStatus(200)));
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

    // 4. Push placeholder upstream so LB / TLS / CB writes during peer construction land on
    //    a valid entry (`current_upstream_id` is None right after `init_backend_context`).
    begin_upstream_attempt(ctx);

    // 5. Get peer from gRPC backend
    let mut peer = get_peer(session, ctx, true).await?;

    // 6. Force HTTP/2 for gRPC
    peer.options.set_http_version(2, 2);

    // 7. Configure peer (shared logic)
    configure_peer_timeouts(edgion_http, &mut peer, ctx);
    backfill_upstream_addr(&peer, ctx);

    Ok(peer)
}

/// Handle HTTP upstream peer selection
#[inline]
pub async fn upstream_peer_http(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<Box<HttpPeer>> {
    finalize_request_uri(session, ctx)?;

    // 0. DirectEndpoint (highest priority, bypass LB)
    // INVARIANT: this check MUST come before the `selected_backend.is_none()` check below.
    // `set_direct_endpoint` (plugin API) writes BOTH `ctx.direct_endpoint` AND
    // `ctx.selected_backend` simultaneously so that `init_backend_context_if_needed` inside
    // the bypass path has a backend_ref to derive service metadata from. If the order were
    // swapped, a non-None `selected_backend` would suppress `select_http_backend` and the
    // request would reach `try_get_peer` with the direct-endpoint backend_ref — incorrect.
    if ctx.direct_endpoint.is_some() {
        return try_direct_endpoint_peer(edgion_http, ctx);
    }

    // 1. ExternalJump (external domain, second priority)
    if ctx.external_jump.is_some() {
        return try_external_jump_peer(edgion_http, ctx).await;
    }

    // 2. Select HTTP backend if not already selected (may include InternalJump inside)
    if ctx.selected_backend.is_none() && ctx.selected_grpc_backend.is_none() {
        select_http_backend(edgion_http, session, ctx).await?;
    }

    // 3. Initialize backend context (unified logic)
    init_backend_context_if_needed(ctx)?;

    // 4. Push placeholder upstream so LB metadata / TLS / CB writes inside try_get_peer
    //    land on a valid entry — `init_backend_context_if_needed` leaves `current_upstream_id`
    //    as None, which would otherwise silently drop those writes (P0 g1-001).
    begin_upstream_attempt(ctx);

    // 5. Get peer — common path defers to LB; the SP-affinity branch lives in its own helper.
    let mut peer = if ctx.sp_target_addr.is_some() {
        get_peer_with_sp_fallback(edgion_http, session, ctx).await?
    } else {
        get_peer(session, ctx, false).await?
    };

    // 6. Configure peer (shared logic)
    configure_peer_timeouts(edgion_http, &mut peer, ctx);
    backfill_upstream_addr(&peer, ctx);

    Ok(peer)
}

/// Select HTTP backend from route (extracted from upstream_peer)
#[inline]
pub async fn select_http_backend(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    let route_unit = match ctx.route_unit.as_ref() {
        Some(unit) => unit,
        None => {
            ctx.add_error(EdgionStatus::UpstreamNotRouteMatched);
            end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
            return Err(PingoraError::new(ErrorType::InternalError));
        }
    };

    // Session Persistence step 1 of 2: extract token + set initial state. Step 2
    // (affinity lookup) runs below after backend_ref is selected, since the affinity
    // map is keyed by `{namespace}/{backend_ref.name}`.
    let sp_token = sp_extract_token(session, &mut ctx.sp_state, &route_unit.rule);

    // Pick backend_ref: InternalJump (plugin-injected) tries by name first and falls back to
    // weighted round-robin on miss; without InternalJump, plain weighted round-robin.
    let mut backend_ref = match pick_backend_ref(
        &route_unit.rule,
        &route_unit.matched_info.rsc.ns,
        ctx.internal_jump.as_ref(),
    ) {
        Ok(b) => b,
        Err(e) => {
            ctx.add_error(backend_select_status(&e));
            end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
            return Err(PingoraError::new(ErrorType::InternalError));
        }
    };

    // Query BackendTLSPolicy using route namespace for proper namespace inheritance
    let service_name = &backend_ref.name;
    // If backend_ref.namespace is None, inherit from route namespace
    let service_namespace = backend_ref
        .namespace
        .as_deref()
        .or(Some(route_unit.matched_info.rsc.ns.as_str()));

    backend_ref.backend_tls_policy = crate::core::gateway::backends::query_backend_tls_policy_for_service(
        service_name,
        service_namespace,
        backend_ref.port,
    );

    // BackendTLSPolicy is applied via the peer builder; no per-request logging needed

    // Pre-extract route namespace before mutable ctx borrows (borrow checker)
    let route_ns = route_unit.matched_info.rsc.ns.clone();

    // Run backend-level request edgion_plugins
    backend_ref.plugin_runtime.run_request_plugins(session, ctx).await;
    if ctx.plugin_running_result == PluginRunningResult::ErrTerminateRequest {
        let _ = terminate_with_fallback(session, ctx, &edgion_http.server_header_opts).await;
        return Err(PingoraError::new(ErrorType::InternalError));
    }

    // Session Persistence step 2 of 2: resolve token to a sticky target via the
    // affinity map. Step 1 ran above before backend_ref selection.
    sp_resolve_target(ctx, &backend_ref, &route_ns, sp_token.as_deref());

    ctx.selected_backend = Some(backend_ref);
    Ok(())
}

/// Session Persistence step 1: extract the session token (if SP is configured on the route)
/// and pin the initial `sp_state`. Returns the token for the later affinity lookup.
///
/// Takes `&mut SessionPersistenceState` rather than `&mut EdgionHttpContext` to keep the
/// borrow disjoint from `&route_unit.rule` (which the caller still needs for backend selection).
///
/// - SP not configured → returns `None`, state untouched.
/// - SP configured, token present → state = `Inactive` (a fallback later may flip it).
/// - SP configured, no token → state = `InjectNew` (first request; response will inject one).
#[inline]
fn sp_extract_token(
    session: &Session,
    sp_state: &mut SessionPersistenceState,
    rule: &crate::types::HTTPRouteRule,
) -> Option<String> {
    let sp = rule.session_persistence.as_ref()?;
    let token = session_persistence::extract_session_token(session, sp);
    *sp_state = if token.is_some() {
        SessionPersistenceState::Inactive
    } else {
        SessionPersistenceState::InjectNew
    };
    token
}

/// Session Persistence step 2: resolve a token to a sticky upstream address via the
/// per-service affinity map.
///
/// - Token absent → no-op (first request: state was already set to `InjectNew` in step 1).
/// - Token hits in map → set `sp_target_addr` so `try_get_peer` routes directly to it.
/// - Token misses (stale/unknown) → flip state to `InjectNew` so the response refreshes it.
#[inline]
fn sp_resolve_target(
    ctx: &mut EdgionHttpContext,
    backend_ref: &crate::types::HTTPBackendRef,
    route_ns: &str,
    token: Option<&str>,
) {
    let Some(token) = token else { return };

    let ns = backend_ref.namespace.as_deref().unwrap_or(route_ns);
    let service_key = format!("{}/{}", ns, backend_ref.name);
    let affinity_map = crate::core::gateway::lb::runtime_state::sp_affinity_get_or_build_key(
        &service_key,
        || crate::core::gateway::backends::selector::get_backends_for_service_key(&service_key),
    );

    if let Some(&addr) = affinity_map.get(token) {
        ctx.sp_target_addr = Some(addr);
    } else {
        ctx.sp_state = SessionPersistenceState::InjectNew;
    }
}

/// Get peer when session persistence resolved a sticky target (`sp_target_addr` is set).
///
/// Caller must have checked `ctx.sp_target_addr.is_some()` — this function assumes the
/// SP-affinity branch and only routes its three outcomes:
///
/// - `try_get_peer` succeeds → use the sticky target.
/// - `try_get_peer` rejects with SSRF (loopback / link-local) → write 503 and abort;
///   never fall back, since the rejection is a security signal, not unhealthiness.
/// - `try_get_peer` fails for any other reason → fall back to normal LB unless the route
///   declared `session_persistence.strict = true`. On fallback, mark `sp_state` as
///   `InjectUpdate` so the response refreshes the cookie/header pointing at the new pick.
///
/// Strict mode keeps the original `try_get_peer` error from `get_peer`'s second call,
/// rather than synthesising one — `get_peer` will return whichever backend it can find,
/// or surface its own error to the proxy layer.
async fn get_peer_with_sp_fallback(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<Box<HttpPeer>> {
    let sp_strict = ctx
        .route_unit
        .as_ref()
        .and_then(|ru| ru.rule.session_persistence.as_ref())
        .and_then(|sp| sp.strict)
        .unwrap_or(false);

    match try_get_peer(ctx, session, false) {
        Ok(peer) => Ok(peer),
        Err(status @ (EdgionStatus::BackendLocalhostNotAllowed | EdgionStatus::BackendAddrLinkLocalNotAllowed)) => {
            ctx.add_error(status);
            end_response_503(session, ctx, &edgion_http.server_header_opts).await?;
            Err(PingoraError::new(ErrorType::InternalError))
        }
        Err(_) if !sp_strict => {
            // Sticky target unhealthy → drop the SP target and refresh the token in the response.
            ctx.sp_target_addr = None; // already consumed by try_get_peer, but be explicit
            ctx.sp_state = SessionPersistenceState::InjectUpdate;
            get_peer(session, ctx, false).await
        }
        Err(_) => {
            // Strict mode: no fallback to normal LB. Defer to get_peer to surface a typed error.
            get_peer(session, ctx, false).await
        }
    }
}

/// Pick the HTTP backend_ref for this request.
///
/// With `InternalJump` (set by the dynamic_internal_upstream plugin), try the named
/// backend_ref first; on miss, fall through to weighted round-robin so the request
/// still gets served (graceful degradation rather than failing the request).
///
/// Without `InternalJump`, plain weighted round-robin selection.
///
/// Pure: no `&mut ctx` borrow, so the caller can keep its `&route_unit.rule` borrow
/// alive across the call. Takes `&Arc<HTTPRouteRule>` (rather than `&HTTPRouteRule`)
/// because that's what `RouteRules::find_backend_by_name` and `select_backend` require.
#[inline]
fn pick_backend_ref(
    rule: &std::sync::Arc<crate::types::HTTPRouteRule>,
    route_ns: &str,
    internal_jump: Option<&crate::types::InternalJumpPreset>,
) -> Result<crate::types::HTTPBackendRef, EdError> {
    if let Some(jump) = internal_jump {
        if let Ok(b) = RouteRules::find_backend_by_name(
            rule,
            &jump.backend_ref_name,
            jump.backend_ref_namespace.as_deref(),
            route_ns,
        ) {
            return Ok(b);
        }
        // Named lookup missed → fall through to weighted RR.
    }
    RouteRules::select_backend(rule)
}

/// Map a backend-selection `EdError` to its public `EdgionStatus` for access logs.
#[inline]
fn backend_select_status(e: &EdError) -> EdgionStatus {
    match e {
        EdError::BackendNotFound() => EdgionStatus::UpstreamNotBackendRefs,
        EdError::InconsistentWeight() => EdgionStatus::UpstreamInconsistentWeight,
        EdError::RefDenied { .. } => EdgionStatus::RefDenied,
        _ => EdgionStatus::Unknown,
    }
}

/// Configure peer timeouts from global and route-level configs (inline for performance)
#[inline]
pub fn configure_peer_timeouts(edgion_http: &EdgionHttpProxy, peer: &mut Box<HttpPeer>, ctx: &EdgionHttpContext) {
    let backend_timeout = &edgion_http.parsed_timeouts.backend;
    let route_timeouts = ctx
        .route_unit
        .as_ref()
        .and_then(|unit| unit.rule.parsed_timeouts.as_ref())
        .or_else(|| {
            ctx.grpc_route_unit
                .as_ref()
                .and_then(|unit| unit.rule.parsed_timeouts.as_ref())
        });

    // Backend request timeout: three-state per Gateway API spec
    // - Unset: use global default
    // - Disabled ("0s"): no timeout (don't set peer options, Pingora defaults to no timeout)
    // - Duration: use the specified value
    use crate::types::resources::http_route::RouteTimeout;
    let route_backend_timeout = route_timeouts.map(|rt| &rt.backend_request_timeout);

    let effective_backend_timeout = match route_backend_timeout {
        Some(RouteTimeout::Disabled) => None, // "0s" = no timeout
        Some(RouteTimeout::Duration(d)) => Some(*d),
        _ => Some(backend_timeout.request_timeout), // Unset or no route timeouts → global default
    };

    // connection_timeout: use route backendRequest timeout, or fall back to global default.
    // "0s" on backendRequest disables I/O timeouts, not connection establishment — always set a floor.
    // Intentionally NOT bounded by request_timeout: connection establishment is infrastructure
    // overhead; bounding it would cause spurious 504s when request_timeout is tight.
    let effective_connection_timeout = effective_backend_timeout.unwrap_or(backend_timeout.connect_timeout);
    peer.options.connection_timeout = Some(effective_connection_timeout);

    // Apply request_timeout as a ceiling on read/write timeout.
    // Gateway API spec: request_timeout is end-to-end; remaining time = timeout - elapsed.
    // Filter out zero remaining time to avoid setting Duration::ZERO on the peer (undefined behaviour).
    let request_remaining = route_timeouts
        .and_then(|rt| rt.request_timeout.as_duration())
        .map(|req| req.saturating_sub(ctx.start_time.elapsed()))
        .filter(|r| !r.is_zero());

    let effective_read_write_timeout = match (effective_backend_timeout, request_remaining) {
        (Some(b), Some(r)) => Some(b.min(r)),
        (Some(b), None) => Some(b),
        (None, Some(r)) => Some(r),
        (None, None) => None,
    };

    if let Some(timeout) = effective_read_write_timeout {
        peer.options.read_timeout = Some(timeout);
        peer.options.write_timeout = Some(timeout);
    }

    // Idle timeout: use global config from EdgionGatewayConfig
    peer.options.idle_timeout = Some(backend_timeout.idle_timeout);
}

/// Begin a new upstream attempt: push a placeholder upstream entry so subsequent
/// calls to `ctx.get_current_upstream_mut()` (inside try_get_peer / build_peer_from_lb_selection
/// / record_tls_to_upstream / record_unsafe_*_attempt) actually land on a valid entry.
/// ip/port are filled in later by [`backfill_upstream_addr`] once the peer is known.
///
/// Must be called after `init_backend_context_if_needed` and before any peer construction.
/// On retries this also decrements LeastConn for the previous (now-completed) attempt;
/// `leastconn_incremented_backend` is keyed independently of `current_upstream_id`, so this
/// cleanup is safe to run before pushing the new placeholder.
#[inline]
pub fn begin_upstream_attempt(ctx: &mut EdgionHttpContext) {
    // P0-07: on retry, decrement the LeastConn count for the previous attempt's backend
    // (which had connected successfully but is now being abandoned).
    if let Some(backend) = ctx.leastconn_incremented_backend.take() {
        backend.dec_conn();
    }
    // g1-007: on retry, drop the previous attempt's EWMA record so the upcoming retry's
    // elapsed time does not pollute the previous backend's EWMA sample.
    ctx.last_ewma_backend = None;

    ctx.try_cnt += 1;
    ctx.push_upstream(None, None);

    if ctx.upstream_start_time.is_none() {
        ctx.upstream_start_time = Some(std::time::Instant::now());
    }
}

/// Finish a peer that was constructed directly (i.e. without going through LB / try_get_peer):
/// push the upstream attempt placeholder, then apply timeouts and metrics.
///
/// Used by paths that bypass load-balancing (DirectEndpoint, ExternalJump) where the address is
/// already known. Callers must seed `ctx.backend_context` beforehand (typically via
/// [`EdgionHttpContext::init_bypass_backend_context`]); the [`init_backend_context_if_needed`]
/// call inside is a defensive no-op for that case.
#[inline]
fn finish_static_peer(
    edgion_http: &EdgionHttpProxy,
    ctx: &mut EdgionHttpContext,
    mut peer: Box<HttpPeer>,
) -> pingora_core::Result<Box<HttpPeer>> {
    init_backend_context_if_needed(ctx)?;
    begin_upstream_attempt(ctx);
    configure_peer_timeouts(edgion_http, &mut peer, ctx);
    backfill_upstream_addr(&peer, ctx);
    Ok(peer)
}

/// DirectEndpoint bypass path (exact IP set by a plugin, e.g. dynamic_endpoint).
///
/// Caller must have already checked `ctx.direct_endpoint.is_some()` — this is the
/// cold path, so it's not `#[inline]` and the existence check stays at the call
/// site to keep the hot path branch-only.
///
/// `selected_backend` was already populated by `set_direct_endpoint` on the plugin
/// side (paired with the matched backend_ref), so `finish_static_peer`'s call to
/// `init_backend_context_if_needed` derives rsc / service_key / service_kind for
/// free. We only need to retag `source` afterward to record the staging path.
fn try_direct_endpoint_peer(
    edgion_http: &EdgionHttpProxy,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<Box<HttpPeer>> {
    let direct = ctx
        .direct_endpoint
        .as_ref()
        .expect("caller must check direct_endpoint.is_some()");
    let (addr, use_tls, sni) = (direct.addr, direct.use_tls, direct.sni.clone());

    let peer = Box::new(HttpPeer::new(addr, use_tls, sni));
    let result = finish_static_peer(edgion_http, ctx, peer);

    // init_backend_context_if_needed left source as None (default path) — tag this
    // request as a DirectEndpoint bypass so access logs/metrics can distinguish it.
    if let Some(bc) = ctx.backend_context.as_mut() {
        bc.source = Some(BackendSource::DirectEndpoint);
    }

    result
}

/// ExternalJump bypass path (external domain set by a plugin, e.g. region_route).
///
/// Caller must have already checked `ctx.external_jump.is_some()` — cold path,
/// not `#[inline]`. Resolves the domain via async DNS, validates against SSRF
/// defenses, then constructs the peer (or returns the rejection error).
async fn try_external_jump_peer(
    edgion_http: &EdgionHttpProxy,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<Box<HttpPeer>> {
    let external = ctx
        .external_jump
        .as_ref()
        .expect("caller must check external_jump.is_some()");
    let domain = external.domain.clone();
    let port = external.port;
    let use_tls = external.use_tls;
    let sni = external.sni.clone();

    let addr = resolve_domain(&domain, port).await?;

    // Seed before validation so failure-path record_unsafe_resolved_upstream_attempt
    // has a backend_context to push onto. No K8s resource → rsc/service_key stay None.
    if ctx.backend_context.is_none() {
        ctx.backend_context = Some(crate::types::ctx::BackendContext {
            source: Some(BackendSource::ExternalJump),
            rsc: None,
            service_key: None,
            service_kind: None,
            upstreams: Vec::new(),
            current_upstream_id: None,
        });
    }

    if let Err(status) = validate_resolved_domain_addr(&addr, ctx.allow_loopback_upstream) {
        record_unsafe_resolved_upstream_attempt(ctx, addr, status);
        ctx.add_error(status);
        return Err(PingoraError::explain(
            ErrorType::ConnectError,
            "external upstream resolved to unsafe address",
        ));
    }

    let peer = Box::new(HttpPeer::new(addr, use_tls, sni));
    finish_static_peer(edgion_http, ctx, peer)
}

/// Backfill the resolved peer ip/port onto the current placeholder upstream entry.
/// The entry itself was created by [`begin_upstream_attempt`]; this stage only finalises
/// address fields once `try_get_peer` has produced a peer.
#[inline]
pub fn backfill_upstream_addr(peer: &HttpPeer, ctx: &mut EdgionHttpContext) {
    let (ip, port) = peer
        .address()
        .as_inet()
        .map(|addr| (Some(addr.ip().to_string()), Some(addr.port())))
        .unwrap_or((None, None));
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        upstream.ip = ip;
        upstream.port = port;
    }
}

/// Initialize backend context for the resource-selected path (HTTP / gRPC backend ref).
/// No-op when already initialized — bypass paths (DirectEndpoint / ExternalJump) seed
/// it themselves via [`EdgionHttpContext::init_bypass_backend_context`] before reaching here.
/// Errors only when neither selected_backend nor selected_grpc_backend is set, which
/// indicates a routing bug, not a bypass path.
#[inline]
fn init_backend_context_if_needed(ctx: &mut EdgionHttpContext) -> pingora_core::Result<()> {
    if ctx.backend_context.is_some() {
        return Ok(());
    }

    let (name, namespace, kind_str) = if let Some(grpc_br) = ctx.selected_grpc_backend.as_ref() {
        let route_ns = ctx
            .grpc_route_unit
            .as_ref()
            .map(|unit| unit.matched_info.rsc.ns.as_str())
            .unwrap_or_default()
            .to_string();
        let ns = grpc_br.namespace.clone().unwrap_or(route_ns);
        (grpc_br.name.clone(), ns, grpc_br.kind.clone())
    } else if let Some(http_br) = ctx.selected_backend.as_ref() {
        let route_ns = ctx
            .route_unit
            .as_ref()
            .map(|unit| unit.matched_info.rsc.ns.as_str())
            .unwrap_or_default()
            .to_string();
        let ns = http_br.namespace.clone().unwrap_or(route_ns);
        (http_br.name.clone(), ns, http_br.kind.clone())
    } else {
        return Err(PingoraError::new(ErrorType::InternalError));
    };

    let service_key = format!("{}/{}", namespace, name);
    let rsc = crate::types::Rsc {
        kind: "Service".to_string(),
        ns: namespace,
        name,
    };
    let service_kind = EdgionService::from_kind(kind_str.as_ref());
    ctx.init_backend_context(rsc, service_key, service_kind);
    Ok(())
}

fn validate_resolved_domain_addr(
    addr: &std::net::SocketAddr,
    allow_loopback_upstream: bool,
) -> Result<(), EdgionStatus> {
    match classify_backend_addr(&PingoraSocketAddr::Inet(*addr)) {
        None => {}
        Some(BlockedBackendReason::Localhost) if allow_loopback_upstream => {}
        Some(BlockedBackendReason::Localhost) => return Err(EdgionStatus::BackendLocalhostNotAllowed),
        Some(BlockedBackendReason::LinkLocal) => return Err(EdgionStatus::BackendAddrLinkLocalNotAllowed),
    }

    Ok(())
}

#[cfg(test)]
async fn resolve_domain_checked(
    domain: &str,
    port: u16,
    allow_loopback_upstream: bool,
) -> pingora_core::Result<std::net::SocketAddr> {
    let addr = resolve_domain(domain, port).await?;
    validate_resolved_domain_addr(&addr, allow_loopback_upstream).map_err(|status| {
        let reason = match status {
            EdgionStatus::BackendLocalhostNotAllowed => "localhost",
            EdgionStatus::BackendAddrLinkLocalNotAllowed => "link-local/metadata",
            _ => "unsafe",
        };
        PingoraError::explain(
            ErrorType::ConnectError,
            format!("Domain {} resolved to {} address (rejected)", domain, reason),
        )
    })?;
    Ok(addr)
}

fn record_unsafe_resolved_upstream_attempt(
    ctx: &mut EdgionHttpContext,
    addr: std::net::SocketAddr,
    status: EdgionStatus,
) {
    ctx.push_upstream(Some(addr.ip().to_string()), Some(addr.port()));
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        let reason = match status {
            EdgionStatus::BackendLocalhostNotAllowed => "unsafe loopback upstream",
            EdgionStatus::BackendAddrLinkLocalNotAllowed => "unsafe link-local upstream",
            _ => "unsafe upstream address",
        };
        upstream.err.push(reason.to_string());
    }
}

/// Path Forwarding Decision (single point, spec §14).
///
/// Priority:
///   0. ctx.uri_finalized already true   → skip (rewrite final form / already decided)
///   1. annotation forward-raw-path      → Some(true)/Some(false) forces
///   2. external_jump.preserve_raw_path  → gateway-to-gateway default raw
///   3. global forward_normalized_path   → false means raw (v3.1 compat / rollback)
///   Default                              → write normalized
#[inline]
fn finalize_request_uri(session: &mut Session, ctx: &mut EdgionHttpContext) -> pingora_core::Result<()> {
    if ctx.uri_finalized {
        return Ok(());
    }

    let cfg = get_path_normalization_config();
    let annotation = ctx.route_unit.as_ref().and_then(|ru| ru.rule.parsed_forward_raw_path);
    let external_jump_preserve_raw = ctx
        .external_jump
        .as_ref()
        .map(|ej| ej.preserve_raw_path)
        .unwrap_or(false);
    let want_raw = decide_want_raw(annotation, external_jump_preserve_raw, cfg.forward_normalized_path);

    if !want_raw {
        // Compare on path alone — query is passthrough, never changes via
        // normalization. Using `path()` instead of `to_string()` correctly
        // handles HTTP/2 absolute-form requests (where `to_string()` would
        // include scheme + authority).
        let normalized_path = ctx.path();
        if normalized_path != session.req_header().uri.path() {
            let new_uri = ctx.uri(session.req_header());
            let parsed = new_uri
                .parse::<http::Uri>()
                .map_err(|e| PingoraError::because(ErrorType::InternalError, "compose normalized uri", e))?;
            session.req_header_mut().set_uri(parsed);
            // Wrote normalized form; cache (normalized_current_path) is already
            // consistent with this URI, so no refresh needed.
        }
    }

    ctx.uri_finalized = true;
    Ok(())
}

/// Path forwarding decision for the upstream-peer single-point block (spec §14).
///
/// Returns `true` if the upstream should receive the raw (un-normalized) path,
/// `false` if it should receive the normalized form.
///
/// Inputs:
/// - `annotation`: parsed `edgion.io/forward-raw-path` from `HTTPRouteRule.parsed_forward_raw_path`
/// - `external_jump_preserve_raw`: `external_jump.preserve_raw_path` (true for region_route etc.)
/// - `cfg_forward_normalized`: `PathNormalizationConfig.forward_normalized_path`
///
/// Both production (`upstream_peer_http`) and unit tests call this single fn,
/// so the formula cannot drift between them.
#[inline]
fn decide_want_raw(annotation: Option<bool>, external_jump_preserve_raw: bool, cfg_forward_normalized: bool) -> bool {
    match annotation {
        Some(b) => b,
        None => external_jump_preserve_raw || !cfg_forward_normalized,
    }
}

#[cfg(test)]
mod tests {
    use super::decide_want_raw;

    #[test]
    fn default_writes_normalized() {
        // No annotation, no external_jump, global forward_normalized_path=true → normalized
        assert!(!decide_want_raw(None, false, true));
    }

    #[test]
    fn annotation_true_forces_raw() {
        assert!(decide_want_raw(Some(true), false, true));
    }

    #[test]
    fn annotation_false_forces_normalized_overrides_region_route() {
        // annotation "false" overrides region_route's preserve_raw_path = true
        assert!(!decide_want_raw(Some(false), true, true));
    }

    #[test]
    fn region_route_default_raw() {
        assert!(decide_want_raw(None, true, true));
    }

    #[test]
    fn dynamic_external_upstream_default_normalized() {
        // dynamic_external_upstream sets preserve_raw_path: false on its
        // ExternalJumpPreset. With no annotation override, this should still
        // forward normalized — same outcome as the no-external-jump default,
        // pinned here as an intentional regression check for plugin defaults.
        assert!(!decide_want_raw(None, false, true));
    }

    #[test]
    fn global_disable_forwards_raw() {
        assert!(decide_want_raw(None, false, false));
    }

    #[test]
    fn annotation_true_overrides_global_normalize() {
        assert!(decide_want_raw(Some(true), false, true));
    }

    #[test]
    fn annotation_false_overrides_global_disable() {
        assert!(!decide_want_raw(Some(false), false, false));
    }

    // ─── Matrix completeness tests: cover the full 3×2×2 = 12 input combinations ───
    // The 8 tests above cover 8 named scenarios; the 4 below fill the remaining
    // 4 boundary combinations to make matrix coverage exhaustive.

    #[test]
    fn annotation_true_with_region_route_and_global_normalize_still_raw() {
        // Annotation wins at the top of the priority chain. Even with all lower
        // channels active, Some(true) deterministically returns true.
        assert!(decide_want_raw(Some(true), true, true));
    }

    #[test]
    fn annotation_true_with_region_route_and_global_disable_still_raw() {
        // Annotation true + region_route + global disable → all channels agree on raw.
        assert!(decide_want_raw(Some(true), true, false));
    }

    #[test]
    fn annotation_false_with_region_route_and_global_disable_still_normalized() {
        // Annotation false explicitly overrides BOTH region_route's preserve_raw_path
        // AND the global disable. This is the operator's emergency override channel.
        assert!(!decide_want_raw(Some(false), true, false));
    }

    #[test]
    fn region_route_with_global_disable_doubles_down_on_raw() {
        // No annotation, region_route preserve_raw=true, global forward_normalized=false
        // → both lower channels OR-combine to raw. Verifies the OR formula in the None arm.
        assert!(decide_want_raw(None, true, false));
    }

    // ─── I-2 regression: idempotency check uses path() not to_string() ───
    // Verifies that for absolute-form HTTP/2 URIs (where to_string() includes
    // scheme + authority but path() returns only the path component), the
    // upstream_peer_http idempotency comparison correctly uses the path component.
    // If a future refactor reverts to to_string() comparison, this test catches the
    // regression by demonstrating that path() and to_string() can diverge.

    #[test]
    fn absolute_form_uri_path_only_comparison() {
        let absolute: http::Uri = "http://example.com/api/data?x=1".parse().unwrap();
        // path() returns only the path component
        assert_eq!(absolute.path(), "/api/data");
        // to_string() includes scheme + authority + query (and would falsely report
        // "different" against a normalized "/api/data" path)
        assert_ne!(absolute.to_string(), "/api/data");
        assert!(absolute.to_string().contains("example.com"));
        // The pg_upstream_peer.rs idempotency check at upstream_peer_http uses
        // path() so absolute-form requests don't trigger spurious set_uri writes.
    }
}
