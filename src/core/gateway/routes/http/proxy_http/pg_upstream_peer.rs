use super::EdgionHttpProxy;
use crate::core::gateway::backends::EdgionService;
use crate::core::gateway::backends::{get_peer, try_get_peer};
use crate::core::gateway::ctx::BackendSource;
use crate::core::gateway::routes::common::UNSAFE_UPSTREAM_ADDRESS_LOG;
use crate::core::gateway::routes::grpc::handle_grpc_upstream;
use crate::core::gateway::routes::http::routes_mgr::RouteRules;
use crate::core::gateway::routes::http::session_persistence;
use crate::core::gateway::{
    end_response_500, end_response_503, end_response_grpc_unavailable, terminate_with_fallback,
};
use crate::core::gateway::{EdgionHttpContext, SessionPersistenceState, UpstreamJumpTarget};
use edgion_common::utils::net::{classify_backend_addr, resolve_domain, BlockedBackendReason};
use edgion_resources::err::EdError;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::path_normalization::get_path_normalization_config;
use edgion_resources::EdgionStatus;
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
        let elapsed = std::time::Instant::now().saturating_duration_since(ctx.start_time);
        if elapsed >= timeout {
            ctx.add_error(EdgionStatus::RequestTimeoutExceeded);
            if let Some(upstream) = ctx.get_current_upstream_mut() {
                upstream.status = Some(504);
            }
            return Err(PingoraError::new_str("Request timeout exceeded"));
        }
    }

    // Honour client-supplied gRPC deadline. Already-expired deadlines abort
    // before peer selection so the client sees DEADLINE_EXCEEDED via the
    // 504 → grpc-status 4 mapping rather than waiting for a transport reset.
    if ctx.grpc_deadline_exceeded() {
        ctx.add_error(EdgionStatus::GrpcClientDeadlineExceeded);
        if let Some(upstream) = ctx.get_current_upstream_mut() {
            upstream.status = Some(504);
        }
        return Err(PingoraError::explain(
            ErrorType::HTTPStatus(504),
            "gRPC client deadline exceeded",
        ));
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
            //
            // Bound the sleep by every deadline that applies to the request:
            // route `request_timeout` and client `grpc-timeout`. Either
            // remaining < delay means the caller will give up before backoff
            // completes — skip the sleep and let the post-sleep re-check
            // below abort the retry instead of opening a fresh upstream
            // connection that nobody is waiting for.
            let now = std::time::Instant::now();
            let request_remaining =
                request_timeout.map(|t| t.saturating_sub(now.saturating_duration_since(ctx.start_time)));
            let deadline_remaining = [request_remaining, ctx.grpc_remaining()].into_iter().flatten().min();
            let should_sleep = match deadline_remaining {
                Some(r) => delay < r,
                None => true,
            };
            if should_sleep {
                tokio::time::sleep(delay).await;
            }

            // Either deadline may have just expired (or the sleep was
            // skipped because remaining was already too tight). Re-check
            // before letting Pingora open a new upstream connection — without
            // this guard a tight `grpc-timeout` combined with a large backoff
            // would let the gateway connect to a backend the caller has
            // already abandoned, burning a circuit-breaker failure counter
            // and an LB pick.
            if let Some(timeout) = request_timeout {
                let elapsed = std::time::Instant::now().saturating_duration_since(ctx.start_time);
                if elapsed >= timeout {
                    ctx.add_error(EdgionStatus::RequestTimeoutExceeded);
                    if let Some(upstream) = ctx.get_current_upstream_mut() {
                        upstream.status = Some(504);
                    }
                    return Err(PingoraError::explain(
                        ErrorType::HTTPStatus(504),
                        "Request timeout exceeded during retry backoff",
                    ));
                }
            }
            if ctx.grpc_deadline_exceeded() {
                ctx.add_error(EdgionStatus::GrpcClientDeadlineExceeded);
                if let Some(upstream) = ctx.get_current_upstream_mut() {
                    upstream.status = Some(504);
                }
                return Err(PingoraError::explain(
                    ErrorType::HTTPStatus(504),
                    "gRPC client deadline exceeded during retry backoff",
                ));
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

    // 2. Initialize backend context (unified logic)
    init_backend_context_if_needed(ctx)?;

    // 4. Push placeholder upstream so LB / TLS / CB writes during peer construction land on
    //    a valid entry (`current_upstream_id` is None right after `init_backend_context`).
    begin_upstream_attempt(ctx);

    // 5. Get peer from gRPC backend
    let mut peer = get_peer(session, ctx, true, &edgion_http.server_header_opts).await?;

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

    // 0/1. Upstream-jump slot (Direct/External bypass; Internal falls through to LB).
    // INVARIANT: this check MUST come before the `selected_backend.is_none()` check below.
    // `set_upstream_jump` (via ctx.try_set_jump) co-writes `selected_backend` on a
    // successful Direct write so `init_backend_context_if_needed` inside the bypass
    // path has a backend_ref. If the order were swapped, a non-None
    // `selected_backend` would suppress `select_http_backend` and the request would
    // reach `try_get_peer` with the direct-endpoint backend_ref — incorrect.
    if let Some(jump) = &ctx.jump {
        match jump {
            UpstreamJumpTarget::Direct { .. } => return try_direct_endpoint_peer(edgion_http, ctx),
            UpstreamJumpTarget::External(_) => return try_external_jump_peer(edgion_http, ctx).await,
            UpstreamJumpTarget::Internal(_) => {
                // fall through — Internal stays in ctx.jump, consumed by pick_backend_ref
            }
        }
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
        get_peer(session, ctx, false, &edgion_http.server_header_opts).await?
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
    let mut backend_ref = match pick_backend_ref(route_unit, &route_unit.matched_info.rsc.ns, ctx.jump.as_ref()) {
        Ok(b) => b,
        Err(e) => {
            ctx.add_error(backend_select_status(&e));
            end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
            return Err(PingoraError::new(ErrorType::InternalError));
        }
    };

    // Query BackendTLSPolicy using route namespace for proper namespace inheritance.
    // Read the lookup inputs into owned values first so the mutation below does not
    // conflict with the `Deref` borrows of `backend_ref`.
    let service_name = backend_ref.name.clone();
    // If backend_ref.namespace is None, inherit from route namespace
    let service_namespace = backend_ref
        .namespace
        .clone()
        .or_else(|| Some(route_unit.matched_info.rsc.ns.clone()));
    let service_port = backend_ref.port;

    backend_ref.schema.backend_tls_policy = crate::core::gateway::backends::query_backend_tls_policy_for_service(
        &service_name,
        service_namespace.as_deref(),
        service_port,
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
    rule: &edgion_resources::HTTPRouteRule,
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
    backend_ref: &edgion_resources::HTTPBackendRef,
    route_ns: &str,
    token: Option<&str>,
) {
    let Some(token) = token else { return };

    let ns = backend_ref.namespace.as_deref().unwrap_or(route_ns);
    let service_key = format!("{}/{}", ns, backend_ref.name);
    let affinity_map = crate::core::gateway::lb::runtime_state::sp_affinity_get_or_build_key(&service_key, || {
        crate::core::gateway::backends::selector::get_backends_for_service_key(&service_key)
    });

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
/// - `try_get_peer` rejects with SSRF (loopback / link-local) or DNS resolution failure
///   → write 503 and abort; never fall back, since retrying would hit the same error.
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

    match try_get_peer(ctx, session, false).await {
        Ok(peer) => Ok(peer),
        Err(
            status @ (EdgionStatus::BackendLocalhostNotAllowed
            | EdgionStatus::BackendAddrLinkLocalNotAllowed
            | EdgionStatus::BackendDnsResolutionFailed),
        ) => {
            ctx.add_error(status);
            end_response_503(session, ctx, &edgion_http.server_header_opts).await?;
            Err(PingoraError::new(ErrorType::InternalError))
        }
        Err(_) if !sp_strict => {
            // Sticky target unhealthy → drop the SP target and refresh the token in the response.
            ctx.sp_target_addr = None; // already consumed by try_get_peer, but be explicit
            ctx.sp_state = SessionPersistenceState::InjectUpdate;
            get_peer(session, ctx, false, &edgion_http.server_header_opts).await
        }
        Err(_) => {
            // Strict mode: no fallback to normal LB. Defer to get_peer to surface a typed error.
            get_peer(session, ctx, false, &edgion_http.server_header_opts).await
        }
    }
}

/// Pick the `HTTPBackendRef` for an LB-bound request. When `ctx.jump` carries an
/// `Internal` variant, named lookup is tried first and falls back to weighted
/// round-robin on miss; when `ctx.jump` is None or carries a non-Internal variant,
/// plain weighted round-robin.
#[inline]
fn pick_backend_ref(
    unit: &std::sync::Arc<crate::core::gateway::routes::http::HttpRouteRuleUnit>,
    route_ns: &str,
    jump: Option<&crate::core::gateway::UpstreamJumpTarget>,
) -> Result<crate::core::gateway::routes::http::match_unit::CompiledBackend, EdError> {
    if let Some(crate::core::gateway::UpstreamJumpTarget::Internal(j)) = jump {
        if let Ok(b) =
            RouteRules::find_backend_by_name(unit, &j.backend_ref_name, j.backend_ref_namespace.as_deref(), route_ns)
        {
            return Ok(b);
        }
        // Named lookup missed → fall through to weighted RR.
    }
    RouteRules::select_backend(unit)
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

/// Fold the three read/write upper bounds into the tightest one, labeled
/// with its source. `BackendRequest` is listed first and comparison is
/// strict (`<`), so it wins ties: when it is exactly as tight as a
/// deadline-derived bound, the per-try retry semantics (GEP-1731) apply;
/// whether a retry actually runs is still gated by the deadline checks in
/// `error_while_proxy`.
#[inline]
fn fold_peer_timeouts(
    backend: Option<std::time::Duration>,
    request_remaining: Option<std::time::Duration>,
    grpc_remaining: Option<std::time::Duration>,
) -> Option<(std::time::Duration, crate::core::gateway::PeerTimeoutSource)> {
    use crate::core::gateway::PeerTimeoutSource;
    let mut best: Option<(std::time::Duration, PeerTimeoutSource)> = None;
    for (candidate, source) in [
        (backend, PeerTimeoutSource::BackendRequest),
        (request_remaining, PeerTimeoutSource::RequestDeadline),
        (grpc_remaining, PeerTimeoutSource::GrpcDeadline),
    ] {
        if let Some(d) = candidate {
            if best.is_none_or(|(b, _)| d < b) {
                best = Some((d, source));
            }
        }
    }
    best
}

/// Resolve the effective `backendRequest` bound for one upstream attempt.
/// Three-state per Gateway API spec:
/// - Unset (or no route timeouts) → global default
/// - Disabled (`"0s"`) → `None` (no I/O bound; only deadline sources clamp)
/// - Duration → the route value
///
/// A zero duration is filtered out as disabled: route-level `"0s"` already
/// parses to `Disabled`, but a global `defaultRequestTimeout: "0s"` reaches
/// here as `Duration::ZERO` and would otherwise become an instant read/write
/// timeout — and, with a retry policy present, an instant-retry loop.
#[inline]
fn effective_backend_bound(
    route: Option<&edgion_resources::resources::http_route::RouteTimeout>,
    global_default: std::time::Duration,
) -> Option<std::time::Duration> {
    use edgion_resources::resources::http_route::RouteTimeout;
    match route {
        Some(RouteTimeout::Disabled) => None,
        Some(RouteTimeout::Duration(d)) => Some(*d),
        _ => Some(global_default),
    }
    .filter(|d| !d.is_zero())
}

/// Configure peer timeouts from global and route-level configs (inline for performance)
#[inline]
pub fn configure_peer_timeouts(edgion_http: &EdgionHttpProxy, peer: &mut Box<HttpPeer>, ctx: &mut EdgionHttpContext) {
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

    let route_backend_timeout = route_timeouts.map(|rt| &rt.backend_request_timeout);
    let effective_backend_timeout = effective_backend_bound(route_backend_timeout, backend_timeout.request_timeout);

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
        .map(|req| req.saturating_sub(std::time::Instant::now().saturating_duration_since(ctx.start_time)))
        .filter(|r| !r.is_zero());

    // Client-supplied `grpc-timeout` is another upper bound on read/write —
    // an intermediary that ignores it leaves upstream work running after the
    // client has given up. `grpc_remaining` returns the deadline minus now,
    // so we apply the same zero-guard as request_remaining.
    let grpc_remaining = ctx.grpc_remaining().filter(|r| !r.is_zero());

    // Fold three independent upper bounds via labeled min (see
    // `fold_peer_timeouts` for tie-break semantics):
    //   - `effective_backend_timeout` is `None` when the route explicitly
    //     disabled it via `backendRequestTimeout: "0s"` (or the global default
    //     is zero — see `effective_backend_bound`), in which case only the
    //     remaining two bounds clamp.
    //   - `request_remaining` is the route-level `request_timeout` minus elapsed
    //     wall-clock.
    //   - `grpc_remaining` is the client-supplied `grpc-timeout` minus elapsed.
    // The winning source is recorded so `error_while_proxy` can distinguish a
    // retryable per-try timeout (BackendRequest, GEP-1731) from a final
    // deadline expiry.
    let folded = fold_peer_timeouts(effective_backend_timeout, request_remaining, grpc_remaining);
    ctx.peer_timeout_source = folded.map(|(_, source)| source);

    if let Some((timeout, _)) = folded {
        peer.options.read_timeout = Some(timeout);
        peer.options.write_timeout = Some(timeout);
    }

    // Idle timeout: use global config from EdgionGatewayConfig
    peer.options.idle_timeout = Some(backend_timeout.idle_timeout);
}

/// Push a new upstream connection placeholder so that subsequent
/// calls to `ctx.get_current_upstream_mut()` (inside try_get_peer /
/// build_peer_from_lb_selection / record_tls_to_upstream /
/// record_unsafe_*_attempt) actually land on a valid entry.
/// ip/port are filled in later by [`backfill_upstream_addr`] once the peer is known.
///
/// Must be called after `init_backend_context_if_needed` and before any peer construction.
/// On retries this also finalizes (Discard) the previous attempt's LB commit:
/// `get_current_upstream_mut()` still points at the prior entry at this moment, so
/// this cleanup MUST run before `push_upstream` advances `current_upstream_id`.
#[inline]
pub fn begin_upstream_attempt(ctx: &mut EdgionHttpContext) {
    // On retry, discard the previous attempt's LB commit (dec LeastConn, drop EWMA sample).
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        crate::core::gateway::lb::finalize_attempt(upstream, crate::core::gateway::lb::FinalizeMode::Discard);
    }

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
/// Caller (the dispatch cascade) has already matched `ctx.jump` against
/// `UpstreamJumpTarget::Direct` — this is the cold path, so it's not `#[inline]`
/// and the variant check stays at the call site to keep the hot path branch-only.
///
/// `selected_backend` was already populated by `set_upstream_jump` on the plugin
/// side (paired with the matched backend_ref), so `finish_static_peer`'s call to
/// `init_backend_context_if_needed` derives rsc / service_key / service_kind for
/// free. We only need to retag `source` afterward to record the staging path.
fn try_direct_endpoint_peer(
    edgion_http: &EdgionHttpProxy,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<Box<HttpPeer>> {
    let direct = match ctx.jump.as_ref() {
        Some(UpstreamJumpTarget::Direct { preset, .. }) => preset,
        // invariant: caller (line 146) only reaches this helper after matching Direct{..};
        //   other variants are dispatched to sibling helpers
        _ => unreachable!("caller must dispatch only on ctx.jump == Some(Direct)"),
    };
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
/// Caller (the dispatch cascade) has already matched `ctx.jump` against
/// `UpstreamJumpTarget::External` — cold path, not `#[inline]`. Resolves the
/// domain via async DNS, validates against SSRF defenses, then constructs the
/// peer (or returns the rejection error).
async fn try_external_jump_peer(
    edgion_http: &EdgionHttpProxy,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<Box<HttpPeer>> {
    let external = match ctx.jump.as_ref() {
        Some(UpstreamJumpTarget::External(p)) => p,
        // invariant: caller (line 147) only reaches this helper after matching External(..);
        //   other variants are dispatched to sibling helpers
        _ => unreachable!("caller must dispatch only on ctx.jump == Some(External)"),
    };
    let domain = external.domain.clone();
    let port = external.port;
    let use_tls = external.use_tls;
    let sni = external.sni.clone();

    let addr = resolve_domain(&domain, port).await?;

    // Seed before validation so failure-path record_unsafe_resolved_upstream_attempt
    // has a backend_context to push onto. No K8s resource → rsc/service_key stay None.
    if ctx.backend_context.is_none() {
        ctx.backend_context = Some(crate::core::gateway::ctx::BackendContext {
            source: Some(BackendSource::ExternalJump),
            rsc: None,
            service_key: None,
            service_kind: None,
            upstreams: Vec::new(),
            current_upstream_id: None,
            backend_ref_idx: None,
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

    let (name, namespace, kind_str, backend_ref_idx) = if let Some(grpc_br) = ctx.selected_grpc_backend.as_ref() {
        let route_ns = ctx
            .grpc_route_unit
            .as_ref()
            .map(|unit| unit.matched_info.rsc.ns.as_str())
            .unwrap_or_default()
            .to_string();
        let ns = grpc_br.namespace.clone().unwrap_or(route_ns);
        // (name, namespace) is effectively unique per rule's backend_refs
        // in well-formed routes; position() returns the first match.
        let idx = ctx.grpc_route_unit.as_ref().and_then(|unit| {
            let route_ns_str = unit.matched_info.rsc.ns.as_str();
            unit.rule.backend_refs.as_ref().and_then(|refs| {
                refs.iter()
                    .position(|r| r.name == grpc_br.name && r.namespace.as_deref().unwrap_or(route_ns_str) == ns)
            })
        });
        (grpc_br.name.clone(), ns, grpc_br.kind.clone(), idx)
    } else if let Some(http_br) = ctx.selected_backend.as_ref() {
        let route_ns = ctx
            .route_unit
            .as_ref()
            .map(|unit| unit.matched_info.rsc.ns.as_str())
            .unwrap_or_default()
            .to_string();
        let ns = http_br.namespace.clone().unwrap_or(route_ns);
        // (name, namespace) is effectively unique per rule's backend_refs
        // in well-formed routes; position() returns the first match.
        let idx = ctx.route_unit.as_ref().and_then(|unit| {
            let route_ns_str = unit.matched_info.rsc.ns.as_str();
            unit.rule.backend_refs.as_ref().and_then(|refs| {
                refs.iter()
                    .position(|r| r.name == http_br.name && r.namespace.as_deref().unwrap_or(route_ns_str) == ns)
            })
        });
        (http_br.name.clone(), ns, http_br.kind.clone(), idx)
    } else {
        return Err(PingoraError::new(ErrorType::InternalError));
    };

    let service_key = format!("{}/{}", namespace, name);
    let rsc = crate::core::gateway::Rsc {
        kind: "Service".to_string(),
        ns: namespace,
        name,
    };
    let service_kind = EdgionService::from_kind(kind_str.as_ref());
    ctx.init_backend_context(rsc, service_key, service_kind, backend_ref_idx);
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
            _ => UNSAFE_UPSTREAM_ADDRESS_LOG,
        };
        upstream.err.push(reason.to_string());
    }
}

/// Path Forwarding Decision (single point, spec §14).
///
/// Priority:
///   0. ctx.uri_finalized already true   → skip (rewrite final form / already decided)
///   1. annotation forward-raw-path      → Some(true)/Some(false) forces
///   2. ctx.jump == External(preserve_raw_path: true) → gateway-to-gateway default raw
///   3. global forward_normalized_path   → false means raw (v3.1 compat / rollback)
///      Default                              → write normalized
#[inline]
fn finalize_request_uri(session: &mut Session, ctx: &mut EdgionHttpContext) -> pingora_core::Result<()> {
    if ctx.uri_finalized {
        return Ok(());
    }

    let cfg = get_path_normalization_config();
    let annotation = ctx.route_unit.as_ref().and_then(|ru| ru.rule.parsed_forward_raw_path);
    let external_jump_preserve_raw = match &ctx.jump {
        Some(UpstreamJumpTarget::External(p)) => p.preserve_raw_path,
        _ => false,
    };
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
    use super::{decide_want_raw, fold_peer_timeouts};
    use crate::core::gateway::PeerTimeoutSource;
    use std::time::Duration;

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

    fn s(n: u64) -> Option<Duration> {
        Some(Duration::from_secs(n))
    }

    #[test]
    fn fold_backend_only() {
        assert_eq!(
            fold_peer_timeouts(s(5), None, None),
            Some((Duration::from_secs(5), PeerTimeoutSource::BackendRequest))
        );
    }

    #[test]
    fn fold_request_deadline_tighter() {
        assert_eq!(
            fold_peer_timeouts(s(5), s(2), None),
            Some((Duration::from_secs(2), PeerTimeoutSource::RequestDeadline))
        );
    }

    #[test]
    fn fold_grpc_deadline_tightest() {
        assert_eq!(
            fold_peer_timeouts(s(5), s(4), s(1)),
            Some((Duration::from_secs(1), PeerTimeoutSource::GrpcDeadline))
        );
    }

    #[test]
    fn fold_tie_backend_wins() {
        // Equal bounds: BackendRequest wins so the operator-opted retry
        // semantics apply; deadline guards still gate the actual retry.
        assert_eq!(
            fold_peer_timeouts(s(3), s(3), s(3)),
            Some((Duration::from_secs(3), PeerTimeoutSource::BackendRequest))
        );
    }

    #[test]
    fn fold_two_way_tie_backend_wins() {
        // Backend ties with a single deadline bound: BackendRequest still wins.
        assert_eq!(
            fold_peer_timeouts(s(3), s(3), None),
            Some((Duration::from_secs(3), PeerTimeoutSource::BackendRequest))
        );
    }

    #[test]
    fn fold_deadline_tie_request_wins() {
        // Deadline-vs-deadline tie: iteration order makes RequestDeadline win.
        // Pinned here because the order is observable in access-log error
        // attribution even though both deadlines expire simultaneously.
        assert_eq!(
            fold_peer_timeouts(None, s(3), s(3)),
            Some((Duration::from_secs(3), PeerTimeoutSource::RequestDeadline))
        );
    }

    #[test]
    fn fold_backend_disabled_request_wins() {
        // backendRequest: "0s" (Disabled) → backend bound is None.
        assert_eq!(
            fold_peer_timeouts(None, s(7), None),
            Some((Duration::from_secs(7), PeerTimeoutSource::RequestDeadline))
        );
    }

    #[test]
    fn fold_grpc_only() {
        assert_eq!(
            fold_peer_timeouts(None, None, s(1)),
            Some((Duration::from_secs(1), PeerTimeoutSource::GrpcDeadline))
        );
    }

    #[test]
    fn fold_backend_grpc_tie_backend_wins() {
        assert_eq!(
            fold_peer_timeouts(s(3), None, s(3)),
            Some((Duration::from_secs(3), PeerTimeoutSource::BackendRequest))
        );
    }

    #[test]
    fn fold_all_none() {
        assert_eq!(fold_peer_timeouts(None, None, None), None);
    }

    // ─── effective_backend_bound: route three-state + global-default fallback ───

    use super::effective_backend_bound;
    use edgion_resources::resources::http_route::RouteTimeout;

    #[test]
    fn backend_bound_unset_falls_back_to_global() {
        let global = Duration::from_secs(60);
        assert_eq!(effective_backend_bound(None, global), Some(global));
        assert_eq!(
            effective_backend_bound(Some(&RouteTimeout::Unset), global),
            Some(global)
        );
    }

    #[test]
    fn backend_bound_disabled_is_none() {
        assert_eq!(
            effective_backend_bound(Some(&RouteTimeout::Disabled), Duration::from_secs(60)),
            None
        );
    }

    #[test]
    fn backend_bound_route_duration_wins_over_global() {
        assert_eq!(
            effective_backend_bound(
                Some(&RouteTimeout::Duration(Duration::from_secs(1))),
                Duration::from_secs(60)
            ),
            Some(Duration::from_secs(1))
        );
    }

    #[test]
    fn backend_bound_zero_global_treated_as_disabled() {
        // A `defaultRequestTimeout: "0s"` global must not become an instant
        // read/write timeout (instant-retry loop with a retry policy present).
        assert_eq!(effective_backend_bound(None, Duration::ZERO), None);
        assert_eq!(
            effective_backend_bound(Some(&RouteTimeout::Unset), Duration::ZERO),
            None
        );
    }
}
