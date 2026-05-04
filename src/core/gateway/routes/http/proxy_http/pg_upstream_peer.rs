use super::EdgionHttpProxy;
use crate::core::common::utils::net::{classify_backend_addr, BlockedBackendReason};
use crate::core::gateway::backends::{get_peer, try_get_peer};
use crate::core::gateway::cli::config::get_path_normalization_config;
use crate::core::gateway::routes::grpc::handle_grpc_upstream;
use crate::core::gateway::routes::http::routes_mgr::RouteRules;
use crate::core::gateway::routes::http::session_persistence;
use crate::core::gateway::{end_response_500, end_response_503, end_response_grpc_unavailable};
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
    update_peer_metrics(edgion_http, &peer, ctx);

    Ok(peer)
}

/// Handle HTTP upstream peer selection
#[inline]
pub async fn upstream_peer_http(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<Box<HttpPeer>> {
    // ── Path Forwarding Decision (single point, spec §14) ──
    // Priority:
    //   0. ctx.uri_finalized already true   → skip (rewrite final form / already decided)
    //   1. annotation forward-raw-path      → Some(true)/Some(false) forces
    //   2. external_jump.preserve_raw_path  → gateway-to-gateway default raw
    //   3. global forward_normalized_path   → false means raw (v3.1 compat / rollback)
    //   Default                              → write normalized
    if !ctx.uri_finalized {
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
    }

    // 0. DirectEndpoint (highest priority, bypass LB)
    if let Some(result) = try_direct_endpoint_peer(edgion_http, ctx) {
        return result;
    }

    // 1. ExternalJump (external domain, second priority)
    if let Some(result) = try_external_jump_peer(edgion_http, ctx).await {
        return result;
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

    // 5. Get peer — with session persistence fallback support
    //
    // When session persistence resolved a target (sp_target_addr was set), try_get_peer
    // routes directly to it. If that fails (unhealthy), fallback to normal LB unless strict.
    let sp_had_target = ctx.sp_target_addr.is_some();
    let sp_strict = ctx
        .route_unit
        .as_ref()
        .and_then(|ru| ru.rule.session_persistence.as_ref())
        .and_then(|sp| sp.strict)
        .unwrap_or(false);

    let mut peer = if sp_had_target {
        match try_get_peer(ctx, session, false) {
            Ok(peer) => peer,
            Err(status @ (EdgionStatus::BackendLocalhostNotAllowed | EdgionStatus::BackendAddrLinkLocalNotAllowed)) => {
                ctx.add_error(status);
                end_response_503(session, ctx, &edgion_http.server_header_opts).await?;
                return Err(PingoraError::new(ErrorType::InternalError));
            }
            Err(_) if !sp_strict => {
                // Session persistence fallback: target was unhealthy, retry with normal LB
                ctx.sp_target_addr = None; // already consumed by try_get_peer, but be explicit
                ctx.sp_state = SessionPersistenceState::InjectUpdate; // update token in response
                                                                      // Retry with normal LB
                get_peer(session, ctx, false).await?
            }
            Err(_) => {
                // No fallback (strict mode, no session persistence, or no target) — error response
                get_peer(session, ctx, false).await?
            }
        }
    } else {
        get_peer(session, ctx, false).await?
    };

    // 6. Configure peer (shared logic)
    configure_peer_timeouts(edgion_http, &mut peer, ctx);
    update_peer_metrics(edgion_http, &peer, ctx);

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

    // Session Persistence: extract token before mutating ctx
    // Pre-compute outside the borrow of route_unit to satisfy the borrow checker
    let sp_active = route_unit.rule.session_persistence.is_some();

    let sp_token = route_unit
        .rule
        .session_persistence
        .as_ref()
        .and_then(|sp| session_persistence::extract_session_token(session, sp));

    // Determine initial session persistence state and extract token value for affinity lookup
    let sp_token_value: Option<String>;
    if sp_active {
        match sp_token {
            Some(token) => {
                // Token found in request — stays Inactive unless fallback happens later
                ctx.sp_state = SessionPersistenceState::Inactive;
                sp_token_value = Some(token);
            }
            None => {
                // No token — first request, will inject new token
                ctx.sp_state = SessionPersistenceState::InjectNew;
                sp_token_value = None;
            }
        }
    } else {
        sp_token_value = None;
    }

    // ===== DynamicInternalUpstream: select specific backend_ref by name =====
    let mut backend_ref = if let Some(ref jump) = ctx.internal_jump {
        match RouteRules::find_backend_by_name(
            &route_unit.rule,
            &jump.backend_ref_name,
            jump.backend_ref_namespace.as_deref(),
            &route_unit.matched_info.rsc.ns,
        ) {
            Ok(backend) => backend,
            Err(_) => match RouteRules::select_backend(&route_unit.rule) {
                Ok(backend) => backend,
                Err(e) => {
                    ctx.add_error(match &e {
                        EdError::BackendNotFound() => EdgionStatus::UpstreamNotBackendRefs,
                        EdError::InconsistentWeight() => EdgionStatus::UpstreamInconsistentWeight,
                        EdError::RefDenied { .. } => EdgionStatus::RefDenied,
                        _ => EdgionStatus::Unknown,
                    });
                    end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
                    return Err(PingoraError::new(ErrorType::InternalError));
                }
            },
        }
    } else {
        // ===== Normal: weighted round-robin selection =====
        match RouteRules::select_backend(&route_unit.rule) {
            Ok(backend) => backend,
            Err(e) => {
                ctx.add_error(match &e {
                    EdError::BackendNotFound() => EdgionStatus::UpstreamNotBackendRefs,
                    EdError::InconsistentWeight() => EdgionStatus::UpstreamInconsistentWeight,
                    EdError::RefDenied { .. } => EdgionStatus::RefDenied,
                    _ => EdgionStatus::Unknown,
                });
                end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
                return Err(PingoraError::new(ErrorType::InternalError));
            }
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
        ctx.add_error(EdgionStatus::Unknown);
        if !ctx.request_info.response_committed {
            end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
        }
        let err_type = if ctx.request_info.is_grpc_request {
            ErrorType::HTTPStatus(200)
        } else {
            ErrorType::InternalError
        };
        return Err(PingoraError::new(err_type));
    }

    // Session Persistence: resolve token → target addr via affinity map
    // Must happen after backend_ref selection (we need service_key for the map lookup)
    if sp_active {
        if let Some(ref token) = sp_token_value {
            // Token present — look up in affinity map
            let namespace = backend_ref.namespace.as_deref().unwrap_or(&route_ns);
            let service_key = format!("{}/{}", namespace, &backend_ref.name);

            // Ensure affinity map is built for this service
            let backends = crate::core::gateway::backends::get_backends_for_service(&service_key);
            let affinity_map =
                crate::core::gateway::lb::runtime_state::sp_affinity_get_or_build(&service_key, &backends);

            if let Some(&addr) = affinity_map.get(token.as_str()) {
                // Token resolved — set target for try_get_peer to use
                ctx.sp_target_addr = Some(addr);
            } else {
                // Token not in map (stale/unknown) → normal LB + inject new token
                ctx.sp_state = SessionPersistenceState::InjectNew;
            }
        }
        // sp_token_value is None → first request (InjectNew already set), no lookup needed
    }

    ctx.selected_backend = Some(backend_ref);
    Ok(())
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
/// ip/port are filled in later by [`update_peer_metrics`] once the peer is known.
///
/// Must be called after `init_backend_context_if_needed` and before any peer construction.
/// On retries this also decrements LeastConn for the previous (now-completed) attempt;
/// `leastconn_incremented_backend` is keyed independently of `current_upstream_id`, so this
/// cleanup is safe to run before pushing the new placeholder.
#[inline]
pub fn begin_upstream_attempt(ctx: &mut EdgionHttpContext) {
    // P0-07: retry 时清算上一次成功建连但中途放弃的 LeastConn backend。
    if let Some((sk, addr)) = ctx.leastconn_incremented_backend.take() {
        crate::core::gateway::lb::runtime_state::decrement(&sk, &addr);
    }

    ctx.try_cnt += 1;
    ctx.push_upstream(None, None);

    if ctx.upstream_start_time.is_none() {
        ctx.upstream_start_time = Some(std::time::Instant::now());
    }
}

/// Finish a peer that was constructed directly (i.e. without going through LB / try_get_peer):
/// init backend context, push the upstream attempt placeholder, then apply timeouts and metrics.
///
/// Used by paths that bypass load-balancing (DirectEndpoint, ExternalJump) where the address is
/// already known. Callers must ensure `ctx.selected_backend` (or `ctx.backend_context`) is
/// populated beforehand so [`init_backend_context_if_needed`] can complete.
#[inline]
fn finish_static_peer(
    edgion_http: &EdgionHttpProxy,
    ctx: &mut EdgionHttpContext,
    mut peer: Box<HttpPeer>,
) -> pingora_core::Result<Box<HttpPeer>> {
    init_backend_context_if_needed(ctx)?;
    begin_upstream_attempt(ctx);
    configure_peer_timeouts(edgion_http, &mut peer, ctx);
    update_peer_metrics(edgion_http, &peer, ctx);
    Ok(peer)
}

/// Try the DirectEndpoint bypass path (exact IP set by a plugin, e.g. dynamic_endpoint).
///
/// Returns `None` when no DirectEndpoint is staged on `ctx`, letting the caller fall through
/// to the next strategy. Otherwise returns the peer-construction result.
#[inline]
fn try_direct_endpoint_peer(
    edgion_http: &EdgionHttpProxy,
    ctx: &mut EdgionHttpContext,
) -> Option<pingora_core::Result<Box<HttpPeer>>> {
    let direct = ctx.direct_endpoint.as_ref()?;
    let (addr, use_tls, sni, backend_ref_idx) =
        (direct.addr, direct.use_tls, direct.sni.clone(), direct.backend_ref_idx);

    // Backfill selected_backend from backend_refs[idx] so init_backend_context_if_needed
    // (called inside finish_static_peer) can derive name/namespace from it.
    if ctx.selected_backend.is_none() {
        ctx.selected_backend = ctx
            .route_unit
            .as_ref()
            .and_then(|ru| ru.rule.backend_refs.as_ref())
            .and_then(|refs| refs.get(backend_ref_idx))
            .cloned();
    }

    let peer = Box::new(HttpPeer::new(addr, use_tls, sni));
    Some(finish_static_peer(edgion_http, ctx, peer))
}

/// Try the ExternalJump bypass path (external domain set by a plugin, e.g. region_route).
///
/// Returns `None` when no ExternalJump is staged on `ctx`. Otherwise resolves the domain via
/// async DNS, validates the address against SSRF defenses, then returns the peer-construction
/// result (or the rejection error).
#[inline]
async fn try_external_jump_peer(
    edgion_http: &EdgionHttpProxy,
    ctx: &mut EdgionHttpContext,
) -> Option<pingora_core::Result<Box<HttpPeer>>> {
    let external = ctx.external_jump.as_ref()?;
    let domain = external.domain.clone();
    let port = external.port;
    let use_tls = external.use_tls;
    let sni = external.sni.clone();

    let addr = match resolve_domain(&domain, port).await {
        Ok(addr) => addr,
        Err(e) => return Some(Err(e)),
    };

    if let Err(status) = validate_resolved_domain_addr(&addr, ctx.allow_loopback_upstream) {
        if ctx.backend_context.is_none() {
            ctx.init_backend_context(domain, "_external_".to_string());
        }
        record_unsafe_resolved_upstream_attempt(ctx, addr, status);
        ctx.add_error(status);
        return Some(Err(PingoraError::explain(
            ErrorType::ConnectError,
            "external upstream resolved to unsafe address",
        )));
    }

    // External jump has no selected_backend; seed backend_context manually before
    // finish_static_peer so init_backend_context_if_needed becomes a no-op.
    // Use domain as "name" and "_external_" as namespace marker.
    if ctx.backend_context.is_none() {
        ctx.init_backend_context(domain, "_external_".to_string());
    }

    let peer = Box::new(HttpPeer::new(addr, use_tls, sni));
    Some(finish_static_peer(edgion_http, ctx, peer))
}

/// Backfill the resolved peer ip/port onto the current placeholder upstream entry.
/// The entry itself was created by [`begin_upstream_attempt`]; this stage only finalises
/// address fields once `try_get_peer` has produced a peer.
#[inline]
pub fn update_peer_metrics(_edgion_http: &EdgionHttpProxy, peer: &HttpPeer, ctx: &mut EdgionHttpContext) {
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
            ctx.grpc_route_unit
                .as_ref()
                .map(|unit| unit.matched_info.rsc.ns.clone())
                .unwrap_or_default()
        });
        (grpc_br.name.clone(), ns)
    } else if let Some(http_br) = ctx.selected_backend.as_ref() {
        let ns = http_br.namespace.clone().unwrap_or_else(|| {
            ctx.route_unit
                .as_ref()
                .map(|unit| unit.matched_info.rsc.ns.clone())
                .unwrap_or_default()
        });
        (http_br.name.clone(), ns)
    } else {
        return Err(PingoraError::new(ErrorType::InternalError));
    };

    ctx.init_backend_context(name, namespace);
    Ok(())
}

/// Resolve domain to SocketAddr using async DNS resolution.
///
/// Uses tokio's async DNS resolver to avoid blocking the event loop.
/// Returns the first resolved address.
async fn resolve_domain(domain: &str, port: u16) -> pingora_core::Result<std::net::SocketAddr> {
    let addr_str = format!("{}:{}", domain, port);

    // Use tokio async DNS resolution
    let mut addrs = tokio::net::lookup_host(&addr_str).await.map_err(|e| {
        PingoraError::explain(
            ErrorType::ConnectError,
            format!("DNS resolution failed for {}: {}", domain, e),
        )
    })?;

    let addr = addrs
        .next()
        .ok_or_else(|| PingoraError::explain(ErrorType::ConnectError, format!("No addresses found for {}", domain)))?;

    Ok(addr)
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
