use super::EdgionHttp;
use crate::core::gateway::backends::get_peer;
use crate::core::gateway::end_response_500;
use crate::core::gateway::routes::grpc::handle_grpc_upstream;
use crate::core::gateway::routes::http::routes_mgr::RouteRules;
use crate::types::err::EdError;
use crate::types::filters::PluginRunningResult;
use crate::types::{EdgionHttpContext, EdgionStatus};
use pingora_core::modules::http::grpc_web::GrpcWebBridge;
use pingora_core::prelude::HttpPeer;
use pingora_core::upstreams::peer::Peer;
use pingora_core::{Error as PingoraError, ErrorType};
use pingora_proxy::Session;

#[inline]
pub async fn upstream_peer(
    edgion_http: &EdgionHttp,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<Box<HttpPeer>> {
    // Check request timeout dynamically before attempting peer selection
    // This prevents starting a new retry attempt when deadline is already exceeded
    let request_timeout = ctx
        .route_unit
        .as_ref()
        .and_then(|unit| unit.rule.parsed_timeouts.as_ref())
        .and_then(|timeouts| timeouts.request_timeout)
        .or_else(|| {
            ctx.grpc_route_unit
                .as_ref()
                .and_then(|unit| unit.rule.parsed_timeouts.as_ref())
                .and_then(|timeouts| timeouts.request_timeout)
        });

    if let Some(timeout) = request_timeout {
        let elapsed = ctx.start_time.elapsed();
        if elapsed >= timeout {
            tracing::warn!(
                total_attempts = ctx.try_cnt,
                elapsed_secs = elapsed.as_secs_f64(),
                timeout_secs = timeout.as_secs_f64(),
                "Request timeout exceeded before upstream_peer"
            );
            ctx.add_error(EdgionStatus::Unknown);
            if let Some(upstream) = ctx.get_current_upstream_mut() {
                upstream.status = Some(504);
            }
            return Err(PingoraError::new_str("Request timeout exceeded"));
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
    edgion_http: &EdgionHttp,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<Box<HttpPeer>> {
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
            end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
            return Err(PingoraError::new(ErrorType::InternalError));
        }
        Err(e) => {
            tracing::error!("Failed to handle gRPC upstream: {:?}", e);
            ctx.add_error(EdgionStatus::GrpcUpstreamNotBackendRefs);
            end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
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
    configure_peer_timeouts(edgion_http, &mut peer, ctx);
    update_peer_metrics(edgion_http, &peer, ctx);

    Ok(peer)
}

/// Handle HTTP upstream peer selection
#[inline]
pub async fn upstream_peer_http(
    edgion_http: &EdgionHttp,
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
        tracing::debug!(
            domain = %external.domain,
            port = %external.port,
            use_tls = %external.use_tls,
            try_cnt = ctx.try_cnt,
            "Using external jump peer"
        );

        // Async DNS resolution (re-resolves on each attempt, including retries)
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

    // 4. Get peer
    let mut peer = get_peer(session, ctx, false).await?;

    // 5. Configure peer (shared logic)
    configure_peer_timeouts(edgion_http, &mut peer, ctx);
    update_peer_metrics(edgion_http, &peer, ctx);

    Ok(peer)
}

/// Select HTTP backend from route (extracted from upstream_peer)
#[inline]
pub async fn select_http_backend(
    edgion_http: &EdgionHttp,
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

    // ===== DynamicInternalUpstream: select specific backend_ref by name =====
    let mut backend_ref = if let Some(ref jump) = ctx.internal_jump {
        tracing::debug!(
            target_name = %jump.backend_ref_name,
            target_namespace = ?jump.backend_ref_namespace,
            "DynamicInternalUpstream: selecting backend by name"
        );
        match RouteRules::find_backend_by_name(
            &route_unit.rule,
            &jump.backend_ref_name,
            jump.backend_ref_namespace.as_deref(),
            &route_unit.matched_info.rns,
        ) {
            Ok(backend) => backend,
            Err(e) => {
                // This shouldn't happen because plugin already pre-validated,
                // but handle gracefully (e.g., race condition on route reload)
                tracing::error!(
                    target_name = %jump.backend_ref_name,
                    error = ?e,
                    "DynamicInternalUpstream: backend_ref not found (fallback to weighted selection)"
                );
                // Fall through to normal weighted selection
                match RouteRules::select_backend(&route_unit.rule) {
                    Ok(backend) => backend,
                    Err(e) => {
                        tracing::error!("Failed to select backend after internal jump fallback: {:?}", e);
                        ctx.add_error(match &e {
                            EdError::BackendNotFound() => EdgionStatus::UpstreamNotBackendRefs,
                            EdError::InconsistentWeight() => EdgionStatus::UpstreamInconsistentWeight,
                            EdError::RefDenied {
                                target_namespace,
                                target_name,
                                reason,
                            } => {
                                tracing::warn!(
                                    target_namespace = %target_namespace,
                                    target_name = %target_name,
                                    reason = %reason,
                                    "Cross-namespace reference denied"
                                );
                                EdgionStatus::RefDenied
                            }
                            _ => EdgionStatus::Unknown,
                        });
                        end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
                        return Err(PingoraError::new(ErrorType::InternalError));
                    }
                }
            }
        }
    } else {
        // ===== Normal: weighted round-robin selection =====
        match RouteRules::select_backend(&route_unit.rule) {
            Ok(backend) => backend,
            Err(e) => {
                tracing::error!("Failed to select backend: {:?}", e);
                ctx.add_error(match &e {
                    EdError::BackendNotFound() => EdgionStatus::UpstreamNotBackendRefs,
                    EdError::InconsistentWeight() => EdgionStatus::UpstreamInconsistentWeight,
                    EdError::RefDenied {
                        target_namespace,
                        target_name,
                        reason,
                    } => {
                        tracing::warn!(
                            target_namespace = %target_namespace,
                            target_name = %target_name,
                            reason = %reason,
                            "Cross-namespace reference denied"
                        );
                        EdgionStatus::RefDenied
                    }
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
        .or(Some(route_unit.matched_info.rns.as_str()));

    backend_ref.backend_tls_policy =
        crate::core::gateway::backends::query_backend_tls_policy_for_service(service_name, service_namespace);

    if let Some(ref policy) = backend_ref.backend_tls_policy {
        tracing::debug!(
            policy = %format!("{}/{}",
                policy.namespace().unwrap_or(""),
                policy.name()
            ),
            service = %format!("{}/{}",
                service_namespace.unwrap_or(""),
                service_name
            ),
            sni = %policy.spec.validation.hostname,
            "BackendTLSPolicy found for selected backend"
        );
    }

    tracing::info!("Selected HTTP backend: {:?}", backend_ref);

    // Run backend-level request edgion_plugins
    backend_ref.plugin_runtime.run_request_plugins(session, ctx).await;
    if ctx.plugin_running_result == PluginRunningResult::ErrTerminateRequest {
        ctx.add_error(EdgionStatus::Unknown);
        end_response_500(session, ctx, &edgion_http.server_header_opts).await?;
        return Err(PingoraError::new(ErrorType::InternalError));
    }

    ctx.selected_backend = Some(backend_ref);
    Ok(())
}

/// Configure peer timeouts from global and route-level configs (inline for performance)
#[inline]
pub fn configure_peer_timeouts(edgion_http: &EdgionHttp, peer: &mut Box<HttpPeer>, ctx: &EdgionHttpContext) {
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

    // Backend request timeout: route-level backend_request_timeout overrides global request_timeout
    // This timeout covers connection + read + write for a single backend request
    let effective_backend_timeout = route_timeouts
        .and_then(|rt| rt.backend_request_timeout)
        .unwrap_or(backend_timeout.request_timeout);

    peer.options.connection_timeout = Some(effective_backend_timeout);
    peer.options.read_timeout = Some(effective_backend_timeout);
    peer.options.write_timeout = Some(effective_backend_timeout);

    // Idle timeout: use global config from EdgionGatewayConfig
    peer.options.idle_timeout = Some(backend_timeout.idle_timeout);
}

/// Update peer address info and metrics (inline for performance)
#[inline]
pub fn update_peer_metrics(_edgion_http: &EdgionHttp, peer: &HttpPeer, ctx: &mut EdgionHttpContext) {
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
                .map(|unit| unit.matched_info.route_ns.clone())
                .unwrap_or_default()
        });
        (grpc_br.name.clone(), ns)
    } else if let Some(http_br) = ctx.selected_backend.as_ref() {
        let ns = http_br.namespace.clone().unwrap_or_else(|| {
            ctx.route_unit
                .as_ref()
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
        tracing::error!(domain = %domain, port = %port, error = %e, "DNS resolution failed");
        PingoraError::explain(
            ErrorType::ConnectError,
            format!("DNS resolution failed for {}: {}", domain, e),
        )
    })?;

    let addr = addrs.next().ok_or_else(|| {
        tracing::error!(domain = %domain, port = %port, "DNS resolution returned no addresses");
        PingoraError::explain(ErrorType::ConnectError, format!("No addresses found for {}", domain))
    })?;

    // Security: reject localhost connections
    if addr.ip().is_loopback() {
        tracing::error!(
            domain = %domain,
            resolved_ip = %addr.ip(),
            "Rejected: external jump domain resolved to localhost"
        );
        return Err(PingoraError::explain(
            ErrorType::ConnectError,
            format!("Domain {} resolved to localhost (rejected)", domain),
        ));
    }

    Ok(addr)
}
