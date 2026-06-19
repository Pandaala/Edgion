pub mod circuit_breaker;
pub mod discovery;
pub mod gateway_backend_tls_store;
pub mod health;
pub mod panic_threshold;
pub(crate) mod peer_builder;
pub mod policy;
pub mod port_resolver;
pub mod preload;
pub mod selector;
pub mod service_kind;
pub mod validation;

pub use circuit_breaker::{get_circuit_breaker_store, parse_outlier_detection_annotation, FailureClass};
pub use discovery::{
    create_ep_slice_handler, create_service_handler, get_global_service_store, get_roundrobin_store, EndpointSliceExt,
    EpSliceStore, ServiceStore,
};
use edgion_resources::resources::BackendTLSPolicy;
pub use panic_threshold::{
    configure_lb_panic_default, get_lb_panic_default, get_panic_threshold_store, panic_floor,
    parse_panic_threshold_annotation, resolve_panic_threshold,
};

pub use gateway_backend_tls_store::{get_global_gateway_backend_tls_store, GatewayBackendTlsStore};
pub use health::{get_health_check_manager, get_health_status_store};
pub(crate) use peer_builder::validate_endpoint_backend_addr;
pub use policy::{
    create_backend_tls_policy_handler, get_global_backend_tls_policy_store, query_backend_tls_policy_for_service,
    BackendTLSPolicyStore,
};
pub use port_resolver::resolve_target_port_key;
pub use preload::preload_load_balancers;
pub use selector::{get_backends_for_service_key, select_roundrobin_backend_key};
pub use service_kind::EdgionService;
pub use validation::validate_endpoint_in_route;

use crate::routes::common::UNSAFE_UPSTREAM_ADDRESS_LOG;
use crate::runtime::ServerHeaderOpts;
use crate::util::net::resolve_domain;
use crate::EdgionHttpContext;
use crate::{end_response_500, end_response_503};
use edgion_resources::edgion_status::EdgionStatus;
use edgion_resources::ParsedLBPolicy;
use peer_builder::{
    apply_backend_app_protocol, create_tls_peer, detect_backend_app_protocol_key, extract_tls_config,
    record_tls_to_upstream, validate_backend_addr, BackendAppProtocol,
};
use pingora_core::prelude::HttpPeer;
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_core::{Error as PingoraError, ErrorType};
use pingora_proxy::Session;
use std::sync::Arc;

use selector::{extract_hash_key, is_backend_healthy, select_from_endpoint_slice};

/// Internal: try to get peer, returns Result with EdgionStatus on error
pub(crate) async fn try_get_peer(
    ctx: &mut EdgionHttpContext,
    session: &Session,
    is_grpc: bool,
) -> Result<Box<HttpPeer>, EdgionStatus> {
    let gateway_key = ctx.gateway_info.gateway_key();
    let gateway_key_ref = Some(gateway_key.as_str());

    // Extract needed fields from backend_ref to avoid clone and borrow checker issues
    let (br_port, lb_policy, backend_tls_policy) = if is_grpc {
        // gRPC backend
        let grpc_br = ctx
            .selected_grpc_backend
            .as_ref()
            .ok_or(EdgionStatus::GrpcUpstreamNotBackendRefs)?;
        (
            grpc_br.port,
            &grpc_br.extension_info.lb_policy,
            &grpc_br.backend_tls_policy,
        )
    } else {
        // HTTP backend
        let http_br = ctx
            .selected_backend
            .as_ref()
            .ok_or(EdgionStatus::UpstreamNotBackendRefs)?;
        (
            http_br.port,
            &http_br.extension_info.lb_policy,
            &http_br.backend_tls_policy,
        )
    };

    // Read service_key and service_kind from BackendContext (set by init_backend_context_if_needed).
    // Bypass paths (DirectEndpoint / ExternalJump) leave service_key / service_kind as None and
    // never reach this function — they construct peers via finish_static_peer instead.
    let (service_key, service_type) = ctx
        .backend_context
        .as_ref()
        .and_then(|bc| match (bc.service_key.clone(), bc.service_kind.clone()) {
            (Some(sk), Some(skd)) => Some((sk, skd)),
            _ => None,
        })
        .ok_or(EdgionStatus::UpstreamNotBackendRefs)?;

    // Clone only the fields we hand to build_peer_from_lb_selection as owned values
    // (the helper takes &mut ctx, which would invalidate the ctx-borrowed references).
    let lb_policy = lb_policy.clone();
    let backend_tls_policy = backend_tls_policy.clone();

    let backend_app_protocol = detect_backend_app_protocol_key(&service_key, br_port);

    match service_type {
        EdgionService::Service => {
            if !get_global_service_store().contains(&service_key) {
                return Err(EdgionStatus::BackendServiceNotFound);
            }

            let backend = take_sp_target_or_select(ctx, &service_key, || {
                select_from_endpoint_slice(&service_key, &lb_policy, session)
            })?;

            build_peer_from_lb_selection(
                ctx,
                session,
                &service_key,
                br_port,
                &lb_policy,
                &backend_tls_policy,
                backend,
                backend_app_protocol,
                gateway_key_ref,
            )
        }

        EdgionService::ServiceClusterIp => {
            let svc_store = get_global_service_store();
            let service = svc_store
                .get(&service_key)
                .ok_or(EdgionStatus::BackendServiceNotFound)?;

            let cluster_ip = service
                .spec
                .as_ref()
                .and_then(|spec| spec.cluster_ip.as_ref())
                .ok_or(EdgionStatus::BackendClusterIpNotFound)?;

            let port = match br_port {
                Some(p) => p as u16,
                None => service
                    .spec
                    .as_ref()
                    .and_then(|spec| spec.ports.as_ref())
                    .and_then(|ports| ports.first())
                    .map(|p| p.port as u16)
                    .ok_or(EdgionStatus::BackendPortResolutionFailed)?,
            };

            // cluster_ip is a bare IP literal (no brackets). Parse it as IpAddr and
            // build the SocketAddr directly so IPv6 ClusterIP services work without
            // manual bracketing (mirrors the EndpointSlice discovery_impl path).
            let ip: std::net::IpAddr = cluster_ip
                .parse()
                .map_err(|_| EdgionStatus::BackendAddressParsingFailed)?;
            let addr = SocketAddr::Inet(std::net::SocketAddr::new(ip, port));

            // Security check: reject loopback / link-local / metadata (SSRF defense)
            if let Err(status) = validate_backend_addr(&addr, ctx.allow_loopback_upstream) {
                record_unsafe_upstream_attempt(ctx, &addr, status);
                return Err(status);
            }

            // Extract TLS configuration and record to upstream
            let (use_tls, sni) = extract_tls_config(&backend_tls_policy);
            record_tls_to_upstream(ctx, use_tls, &sni);

            let mut peer = create_tls_peer(addr, &backend_tls_policy, gateway_key_ref)?;
            apply_backend_app_protocol(&mut peer, backend_app_protocol, ctx);
            Ok(peer)
        }

        EdgionService::ServiceExternalName => {
            let svc_store = get_global_service_store();
            let service = svc_store
                .get(&service_key)
                .ok_or(EdgionStatus::BackendServiceNotFound)?;

            let external_name = service
                .spec
                .as_ref()
                .and_then(|spec| spec.external_name.as_ref())
                .ok_or(EdgionStatus::BackendExternalNameNotFound)?;

            let port = match br_port {
                Some(p) => p as u16,
                None => service
                    .spec
                    .as_ref()
                    .and_then(|spec| spec.ports.as_ref())
                    .and_then(|ports| ports.first())
                    .map(|p| p.port as u16)
                    .ok_or(EdgionStatus::BackendPortResolutionFailed)?,
            };

            // Resolve the external name asynchronously. IP literals are parsed without
            // a network round-trip; FQDNs use tokio::net::lookup_host (non-blocking).
            // Only the first resolved address is used; multi-A DNS round-robin is not
            // applied at this layer. DNS is re-resolved on every request — there is no
            // TTL-aware cache.
            let std_addr = match resolve_domain(external_name, port).await {
                Ok(addr) => addr,
                Err(e) => {
                    if let Some(upstream) = ctx.get_current_upstream_mut() {
                        upstream.port = Some(port);
                        upstream.err.push(format!("dns resolution failed: {}", e));
                    }
                    return Err(EdgionStatus::BackendDnsResolutionFailed);
                }
            };
            let addr = SocketAddr::Inet(std_addr);

            // Security check: reject loopback / link-local / metadata (SSRF defense)
            if let Err(status) = validate_backend_addr(&addr, ctx.allow_loopback_upstream) {
                record_unsafe_upstream_attempt(ctx, &addr, status);
                return Err(status);
            }

            // Extract TLS configuration and record to upstream. TLS is disabled when no
            // BackendTLSPolicy is attached — operators must create one explicitly for FQDN
            // ExternalName backends that serve TLS (e.g. port 443). Without it the gateway
            // sends plaintext HTTP to the resolved IP, which produces a connection error
            // indistinguishable from DNS failure.
            let (use_tls, sni) = extract_tls_config(&backend_tls_policy);
            record_tls_to_upstream(ctx, use_tls, &sni);

            let mut peer = create_tls_peer(addr, &backend_tls_policy, gateway_key_ref)?;
            apply_backend_app_protocol(&mut peer, backend_app_protocol, ctx);
            Ok(peer)
        }

        EdgionService::ServiceImport => Err(EdgionStatus::BackendServiceImportNotImplemented),

        EdgionService::Unsupported(_) => Err(EdgionStatus::BackendUnsupportedKind),

        EdgionService::ServiceEndpointSlice => {
            let backend = take_sp_target_or_select(ctx, &service_key, || {
                select_from_endpoint_slice(&service_key, &lb_policy, session)
            })?;
            build_peer_from_lb_selection(
                ctx,
                session,
                &service_key,
                br_port,
                &lb_policy,
                &backend_tls_policy,
                backend,
                backend_app_protocol,
                gateway_key_ref,
            )
        }
    }
}

/// Session-persistence-aware backend pick for the LB-based service kinds.
///
/// If `ctx.sp_target_addr` is set (the request carried a sticky-session token resolved
/// to a known backend), use that target directly — skipping LB — provided the target
/// is still healthy. The address is `take()`-d unconditionally so a follow-up retry on
/// this request does not double-consume it.
///
/// If `sp_target_addr` is unset, or the sticky target is unhealthy, fall back to the
/// caller-supplied `select_fn` (the kind's normal LB path).
///
/// Note: an unhealthy sticky target returns `StickyBackendUnhealthy` rather than calling
/// `select_fn`. Caller (`get_peer_with_sp_fallback` in upstream_peer) is responsible for
/// deciding whether to re-run with normal LB based on `strict` mode.
fn take_sp_target_or_select<F>(
    ctx: &mut EdgionHttpContext,
    service_key: &str,
    select_fn: F,
) -> Result<Arc<crate::lb::EdgionBackend>, EdgionStatus>
where
    F: FnOnce() -> Result<Arc<crate::lb::EdgionBackend>, EdgionStatus>,
{
    if let Some(target_addr) = ctx.sp_target_addr.take() {
        // Mark consumption regardless of the taken target's health: the
        // healthy/unhealthy outcomes are handled by the in-attempt fallback
        // state machine, while this flag is the cross-attempt refresh evidence.
        ctx.sp_sticky_consumed = true;
        // Look up the live Arc from the cell so inc_conn/dec_conn/update_ewma
        // operate on the same object that the selector tracks. Wrapping a fresh
        // EdgionBackend here would create an orphan: metric calls would be on a
        // throwaway object and the real cell backend would always show conn==0.
        let snap = selector::get_backends_for_service_key(service_key);
        let live = snap
            .iter()
            .find(|b| matches!(b.addr, SocketAddr::Inet(a) if a == target_addr))
            .cloned();

        match live {
            Some(arc) => {
                let health_store = get_health_status_store();
                if is_backend_healthy(service_key, arc.inner(), &health_store) {
                    return Ok(arc);
                }
                return Err(EdgionStatus::StickyBackendUnhealthy);
            }
            // SP target no longer in the cell (pod removed after sticky token was
            // issued). Treat as unhealthy so the caller's strict-mode logic and
            // InjectUpdate state machine apply identically to the gone-backend case.
            None => return Err(EdgionStatus::StickyBackendUnhealthy),
        }
    }
    select_fn()
}

/// Shared logic for all three LB-based service kinds.
///
/// Takes the already-selected `Arc<EdgionBackend>`, resolves the target port,
/// validates the address, writes upstream metadata, and constructs the peer.
/// Stores the `Arc<EdgionBackend>` on `upstream.lb_edgion_backend` so that
/// `connected_to_upstream` can call `inc_conn()` and `lb_runtime_cleanup` can
/// call `dec_conn()` / `update_ewma()` on the same live object.
#[allow(clippy::too_many_arguments)]
fn build_peer_from_lb_selection(
    ctx: &mut EdgionHttpContext,
    session: &Session,
    service_key: &str,
    br_port: Option<i32>,
    lb_policy: &Option<ParsedLBPolicy>,
    backend_tls_policy: &Option<Arc<BackendTLSPolicy>>,
    backend: Arc<crate::lb::EdgionBackend>,
    backend_app_protocol: Option<BackendAppProtocol>,
    gateway_key_ref: Option<&str>,
) -> Result<Box<HttpPeer>, EdgionStatus> {
    let lb_addr = backend.addr.clone();
    let mut peer_addr = lb_addr.clone();
    port_resolver::resolve_target_port_key(&mut peer_addr, br_port, service_key);
    if let Err(status) = validate_endpoint_backend_addr(&peer_addr, ctx.allow_loopback_upstream) {
        record_unsafe_upstream_attempt(ctx, &peer_addr, status);
        return Err(status);
    }

    let (use_tls, sni) = extract_tls_config(backend_tls_policy);
    let lb_policy_clone = lb_policy.clone();

    if ctx.gateway_info.metrics_test_type.is_some() && matches!(lb_policy, Some(ParsedLBPolicy::ConsistentHash(_))) {
        let hash_key_bytes = extract_hash_key(session, lb_policy);
        ctx.hash_key = String::from_utf8(hash_key_bytes).ok();
    }

    if let Some(upstream) = ctx.get_current_upstream_mut() {
        upstream.lb_backend_addr = Some(lb_addr);
        upstream.lb_edgion_backend = Some(backend);
        upstream.service_key = Some(service_key.to_string());
        upstream.lb_policy = lb_policy_clone;
    }
    // Write TLS metadata via the shared helper (writes unconditionally so a
    // TLS-then-plaintext fallback cannot leave stale metadata behind). Kept
    // outside the block above because the helper borrows `ctx` mutably.
    record_tls_to_upstream(ctx, use_tls, &sni);

    let mut peer = create_tls_peer(peer_addr, backend_tls_policy, gateway_key_ref)?;
    apply_backend_app_protocol(&mut peer, backend_app_protocol, ctx);
    Ok(peer)
}

fn record_unsafe_upstream_attempt(ctx: &mut EdgionHttpContext, addr: &SocketAddr, status: EdgionStatus) {
    let (ip, port) = addr
        .as_inet()
        .map(|addr| (Some(addr.ip().to_string()), Some(addr.port())))
        .unwrap_or((None, None));
    let reason = match status {
        EdgionStatus::BackendLocalhostNotAllowed => "unsafe loopback upstream",
        EdgionStatus::BackendAddrLinkLocalNotAllowed => "unsafe link-local upstream",
        _ => UNSAFE_UPSTREAM_ADDRESS_LOG,
    };
    // Backfill the placeholder pushed by `begin_upstream_attempt`. Fall back to
    // pushing a fresh entry if no placeholder exists (defensive — should not happen
    // on the normal `try_get_peer` path).
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        upstream.ip = ip;
        upstream.port = port;
        upstream.err.push(reason.to_string());
    } else {
        ctx.push_upstream(ip, port);
        if let Some(upstream) = ctx.get_current_upstream_mut() {
            upstream.err.push(reason.to_string());
        }
    }
}

/// Get HTTP peer from service and endpoint slice stores using load balancing
///
/// On error, sets error status to ctx and sends an error response:
/// - 500 for invalid backend configuration (non-existent service, unsupported kind,
///   denied cross-namespace ref) per Gateway API conformance
/// - 503 for transient backend resolution failures (no endpoints, address errors)
///
/// # Parameters
/// - `is_grpc`: true if this is for gRPC backend, false for HTTP backend
/// - `server_header_opts`: the listener's configured response headers; passed in so the
///   error path reuses the listener's HSTS decision (stripped for non-TLS listeners)
///   instead of a fresh `ServerHeaderOpts::default()` that always carries HSTS
pub async fn get_peer(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    is_grpc: bool,
    server_header_opts: &ServerHeaderOpts,
) -> pingora_core::Result<Box<HttpPeer>> {
    match try_get_peer(ctx, session, is_grpc).await {
        Ok(peer) => Ok(peer),
        Err(status) => {
            ctx.add_error(status);
            match status {
                EdgionStatus::BackendUnsupportedKind
                | EdgionStatus::BackendServiceNotFound
                | EdgionStatus::RefDenied
                | EdgionStatus::BackendServiceImportNotImplemented => {
                    let _ = end_response_500(session, ctx, server_header_opts).await;
                }
                _ => {
                    let _ = end_response_503(session, ctx, server_header_opts).await;
                }
            }
            Err(PingoraError::new(ErrorType::InternalError))
        }
    }
}
