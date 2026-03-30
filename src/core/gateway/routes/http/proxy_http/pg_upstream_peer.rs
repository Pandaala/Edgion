use super::EdgionHttpProxy;
use crate::core::gateway::backends::{get_peer, try_get_peer};
use crate::core::gateway::routes::grpc::handle_grpc_upstream;
use crate::core::gateway::routes::http::routes_mgr::RouteRules;
use crate::core::gateway::routes::http::session_persistence;
use crate::core::gateway::{end_response_500, end_response_grpc_unavailable};
use crate::types::err::EdError;
use crate::types::filters::PluginRunningResult;
use crate::types::{EdgionHttpContext, EdgionStatus, SessionPersistenceState};
use pingora_core::modules::http::grpc_web::GrpcWebBridge;
use pingora_core::prelude::HttpPeer;
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
    let request_timeout = ctx
        .route_unit
        .as_ref()
        .and_then(|unit| unit.rule.parsed_timeouts.as_ref())
        .and_then(|timeouts| timeouts.request_timeout.as_duration())
        .or_else(|| {
            ctx.grpc_route_unit
                .as_ref()
                .and_then(|unit| unit.rule.parsed_timeouts.as_ref())
                .and_then(|timeouts| timeouts.request_timeout.as_duration())
        });

    if let Some(timeout) = request_timeout {
        let elapsed = ctx.start_time.elapsed();
        if elapsed >= timeout {
            ctx.add_error(EdgionStatus::Unknown);
            if let Some(upstream) = ctx.get_current_upstream_mut() {
                upstream.status = Some(504);
            }
            return Err(PingoraError::new_str("Request timeout exceeded"));
        }
    }

    // Apply backoff delay on retry attempts
    if ctx.try_cnt >= 1 {
        let base_backoff = ctx
            .route_unit
            .as_ref()
            .and_then(|unit| unit.rule.parsed_retry.as_ref())
            .and_then(|r| r.backoff);

        if let Some(base) = base_backoff {
            // Exponential: base * 2^(try_cnt - 1), capped at 30s
            let exponent = (ctx.try_cnt - 1).min(5);
            let delay = base.saturating_mul(1u32.checked_shl(exponent).unwrap_or(u32::MAX));
            let delay = delay.min(std::time::Duration::from_secs(30));

            // Only sleep if remaining time allows
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
            end_response_grpc_unavailable(session, ctx, &edgion_http.server_header_opts).await?;
            // gRPC trailers-only response (HTTP 200 + grpc-status: 14, END_STREAM=true) already
            // written above. Return HTTPStatus(200) so Pingora treats this as "response already
            // handled" and does NOT send RST_STREAM. InternalError would cause Pingora to reset
            // the H2 stream, and the client would see code=Internal(13) instead of Unavailable(14).
            return Err(PingoraError::new(ErrorType::HTTPStatus(200)));
        }
        Err(_) => {
            ctx.add_error(EdgionStatus::GrpcUpstreamNotBackendRefs);
            end_response_grpc_unavailable(session, ctx, &edgion_http.server_header_opts).await?;
            // Same reasoning as Ok(None): response already sent, signal Pingora to stop cleanly.
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

    // 4. Get peer from gRPC backend
    let mut peer = get_peer(session, ctx, true).await?;

    // 5. Force HTTP/2 for gRPC
    peer.options.set_http_version(2, 2);

    // 6. Configure peer (shared logic)
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
    // 0. Check for DirectEndpoint (highest priority, bypass LB)
    if let Some(direct) = &ctx.direct_endpoint {
        let backend_ref_idx = direct.backend_ref_idx;
        let addr = direct.addr;
        let use_tls = direct.use_tls;
        let sni = direct.sni.clone();

        // If backend context not initialized, we need to set selected_backend for metadata
        if ctx.selected_backend.is_none() {
            if let Some(route_unit) = ctx.route_unit.as_ref() {
                if let Some(refs) = &route_unit.rule.backend_refs {
                    if let Some(br) = refs.get(backend_ref_idx) {
                        ctx.selected_backend = Some(br.clone());
                    }
                }
            }
        }

        // Initialize backend context (for logs/metrics)
        init_backend_context_if_needed(ctx)?;

        // Create peer directly
        let mut peer = Box::new(HttpPeer::new(addr, use_tls, sni));

        // Configure peer
        configure_peer_timeouts(edgion_http, &mut peer, ctx);
        update_peer_metrics(edgion_http, &peer, ctx);

        return Ok(peer);
    }

    // 1. Check for ExternalJump (external domain, second priority)
    if let Some(ref external) = ctx.external_jump {
        let addr = resolve_domain(&external.domain, external.port).await?;

        // Build HttpPeer from resolved address
        let mut peer = Box::new(HttpPeer::new(addr, external.use_tls, external.sni.clone()));

        // Initialize backend context for logging/metrics
        // Use domain as "name" and "_external_" as namespace marker
        if ctx.backend_context.is_none() {
            ctx.init_backend_context(external.domain.clone(), "_external_".to_string());
        }

        configure_peer_timeouts(edgion_http, &mut peer, ctx);
        update_peer_metrics(edgion_http, &peer, ctx);
        return Ok(peer);
    }

    // 2. Select HTTP backend if not already selected (may include InternalJump inside)
    if ctx.selected_backend.is_none() && ctx.selected_grpc_backend.is_none() {
        select_http_backend(edgion_http, session, ctx).await?;
    }

    // 3. Initialize backend context (unified logic)
    init_backend_context_if_needed(ctx)?;

    // 4. Get peer — with session persistence fallback support
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

    let mut peer = match try_get_peer(ctx, session, false) {
        Ok(peer) => peer,
        Err(_) if sp_had_target && !sp_strict => {
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
    };

    // 5. Configure peer (shared logic)
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

    // Warn if idle_timeout is configured (not supported in current version)
    if let Some(ref sp) = route_unit.rule.session_persistence {
        if sp.idle_timeout.is_some() {
            tracing::warn!(
                "sessionPersistence.idleTimeout is configured but not supported in current version (requires server-side session storage)"
            );
        }
    }

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

    backend_ref.backend_tls_policy =
        crate::core::gateway::backends::query_backend_tls_policy_for_service(service_name, service_namespace);

    // BackendTLSPolicy is applied via the peer builder; no per-request logging needed

    // Pre-extract route namespace before mutable ctx borrows (borrow checker)
    let route_ns = route_unit.matched_info.rsc.ns.clone();

    // Run backend-level request edgion_plugins
    backend_ref.plugin_runtime.run_request_plugins(session, ctx).await;
    if ctx.plugin_running_result == PluginRunningResult::ErrTerminateRequest {
        ctx.add_error(EdgionStatus::Unknown);
        end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
        // Use HTTPStatus(200) for gRPC so fail_to_proxy does not attempt to write
        // another error response on an already-closed H2 stream (would set upstream
        // status to 500 in logs and emit a redundant write attempt).
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

/// Update peer address info and metrics (inline for performance)
#[inline]
pub fn update_peer_metrics(_edgion_http: &EdgionHttpProxy, peer: &HttpPeer, ctx: &mut EdgionHttpContext) {
    // P0-07: retry 时清算上一次成功建连但中途放弃的 LeastConn backend。
    // 必须在 push_upstream 之前执行，此时 current_upstream_id 仍指向上一次 entry。
    if let Some((sk, addr)) = ctx.leastconn_incremented_backend.take() {
        crate::core::gateway::lb::runtime_state::decrement(&sk, &addr);
    }

    // Increment try count
    ctx.try_cnt += 1;

    // Extract and push upstream info (ip/port saved for logging stage)
    let (ip, port) = peer
        .address()
        .as_inet()
        .map(|addr| (Some(addr.ip().to_string()), Some(addr.port())))
        .unwrap_or((None, None));
    ctx.push_upstream(ip, port);

    // Set upstream start time on first try
    if ctx.upstream_start_time.is_none() {
        ctx.upstream_start_time = Some(std::time::Instant::now());
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
///
/// Security: After resolution, validates the address is not localhost.
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

    if addr.ip().is_loopback() {
        return Err(PingoraError::explain(
            ErrorType::ConnectError,
            format!("Domain {} resolved to localhost (rejected)", domain),
        ));
    }

    Ok(addr)
}
