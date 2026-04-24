pub mod circuit_breaker;
pub mod discovery;
pub mod endpoint_mode;
pub mod gateway_backend_tls_store;
pub mod health;
pub(crate) mod peer_builder;
pub mod policy;
pub mod port_resolver;
pub mod preload;
pub mod selector;
pub mod service_kind;
pub mod validation;

use crate::types::resources::BackendTLSPolicy;
pub use circuit_breaker::{get_circuit_breaker_store, parse_circuit_breaker_annotation};
pub use discovery::{
    create_endpoint_handler, create_ep_slice_handler, create_service_handler, get_endpoint_roundrobin_store,
    get_global_service_store, get_roundrobin_store, EndpointDiscovery, EndpointExt, EndpointSliceExt, EndpointStore,
    EpSliceStore, ServiceStore,
};
pub use endpoint_mode::{
    get_global_endpoint_mode, init_global_endpoint_mode, is_endpoint_slice_mode, try_get_global_endpoint_mode,
    EndpointMode,
};
pub use gateway_backend_tls_store::{get_global_gateway_backend_tls_store, GatewayBackendTlsStore};
pub use health::{get_health_check_manager, get_health_status_store};
pub use policy::{
    create_backend_tls_policy_handler, get_global_backend_tls_policy_store, query_backend_tls_policy_for_service,
    BackendTLSPolicyStore,
};
pub use port_resolver::{resolve_target_port, PortLookupSource};
pub use preload::preload_load_balancers;
pub use selector::{get_backends_for_service, select_roundrobin_backend};
pub use service_kind::EdgionService;
pub use validation::validate_endpoint_in_route;

use crate::core::gateway::{end_response_500, end_response_503};
use crate::types::edgion_status::EdgionStatus;
use crate::types::{EdgionHttpContext, ParsedLBPolicy};
use peer_builder::{
    apply_backend_app_protocol, create_tls_peer, detect_backend_app_protocol, extract_tls_config,
    record_tls_to_upstream, validate_backend_addr, BackendAppProtocol,
};
use pingora_core::prelude::HttpPeer;
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_core::{Error as PingoraError, ErrorType};
use pingora_proxy::Session;
use std::sync::Arc;

use selector::{
    extract_hash_key, is_backend_healthy, select_backend_by_policy, select_from_endpoint_slice, select_from_endpoints,
};

/// Internal: try to get peer, returns Result with EdgionStatus on error
pub(crate) fn try_get_peer(
    ctx: &mut EdgionHttpContext,
    session: &Session,
    is_grpc: bool,
) -> Result<Box<HttpPeer>, EdgionStatus> {
    let gateway_key = ctx.gateway_info.gateway_key();
    let gateway_key_ref = Some(gateway_key.as_str());

    // Extract needed fields from backend_ref to avoid clone and borrow checker issues
    let (br_name, br_port, br_kind, lb_policy, backend_tls_policy) = if is_grpc {
        // gRPC backend
        let grpc_br = ctx
            .selected_grpc_backend
            .as_ref()
            .ok_or(EdgionStatus::GrpcUpstreamNotBackendRefs)?;
        (
            &grpc_br.name,
            grpc_br.port,
            grpc_br.kind.as_ref(),
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
            &http_br.name,
            http_br.port,
            http_br.kind.as_ref(),
            &http_br.extension_info.lb_policy,
            &http_br.backend_tls_policy,
        )
    };

    let service_type = EdgionService::from_kind(br_kind);

    // Form service_key while the immutable borrows of ctx are still alive — avoids
    // cloning br_name / namespace on the hot path.
    let namespace = ctx
        .backend_context
        .as_ref()
        .map(|bc| bc.rsc.ns.as_str())
        .ok_or(EdgionStatus::Unknown)?;
    let service_key = format!("{}/{}", namespace, br_name);

    // Clone only the fields we hand to build_peer_from_lb_selection as owned values
    // (the helper takes &mut ctx, which would invalidate the ctx-borrowed references).
    let lb_policy = lb_policy.clone();
    let backend_tls_policy = backend_tls_policy.clone();

    let backend_app_protocol = detect_backend_app_protocol(&service_key, br_port);

    match service_type {
        EdgionService::Service => {
            if !get_global_service_store().contains(&service_key) {
                return Err(EdgionStatus::BackendServiceNotFound);
            }

            // Session persistence: if sp_target_addr is set, use it directly (skip LB)
            let backend = if let Some(target_addr) = ctx.sp_target_addr.take() {
                let health_store = get_health_status_store();
                let b = pingora_load_balancing::Backend {
                    addr: pingora_core::protocols::l4::socket::SocketAddr::Inet(target_addr),
                    weight: 1,
                    ext: Default::default(),
                };
                if is_backend_healthy(&service_key, &b, &health_store) {
                    b
                } else {
                    return Err(EdgionStatus::BackendEndpointSliceNotFoundByConsistent);
                }
            } else {
                select_backend_by_policy(&service_key, &lb_policy, session)?
            };

            build_peer_from_lb_selection(
                ctx,
                session,
                &service_key,
                br_port,
                &lb_policy,
                &backend_tls_policy,
                backend,
                PortLookupSource::Auto,
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

            // Get port from br_port or service
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

            let addr_str = format!("{}:{}", cluster_ip, port);
            let addr = addr_str
                .parse::<SocketAddr>()
                .map_err(|_| EdgionStatus::BackendAddressParsingFailed)?;

            // Security check: reject loopback / link-local / metadata (SSRF defense)
            validate_backend_addr(&addr)?;

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

            // Get port from br_port or service
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

            let addr_str = format!("{}:{}", external_name, port);
            let addr = addr_str
                .parse::<SocketAddr>()
                .map_err(|_| EdgionStatus::BackendAddressParsingFailed)?;

            // Security check: reject loopback / link-local / metadata (SSRF defense)
            validate_backend_addr(&addr)?;

            // Extract TLS configuration and record to upstream
            let (use_tls, sni) = extract_tls_config(&backend_tls_policy);
            record_tls_to_upstream(ctx, use_tls, &sni);

            let mut peer = create_tls_peer(addr, &backend_tls_policy, gateway_key_ref)?;
            apply_backend_app_protocol(&mut peer, backend_app_protocol, ctx);
            Ok(peer)
        }

        EdgionService::ServiceImport => Err(EdgionStatus::BackendServiceImportNotImplemented),

        EdgionService::Unsupported(_) => Err(EdgionStatus::BackendUnsupportedKind),

        EdgionService::ServiceEndpoint => {
            let backend = select_from_endpoints(&service_key, &lb_policy, session)?;
            build_peer_from_lb_selection(
                ctx,
                session,
                &service_key,
                br_port,
                &lb_policy,
                &backend_tls_policy,
                backend,
                PortLookupSource::Endpoint,
                backend_app_protocol,
                gateway_key_ref,
            )
        }

        EdgionService::ServiceEndpointSlice => {
            let backend = select_from_endpoint_slice(&service_key, &lb_policy, session)?;
            build_peer_from_lb_selection(
                ctx,
                session,
                &service_key,
                br_port,
                &lb_policy,
                &backend_tls_policy,
                backend,
                PortLookupSource::EndpointSlice,
                backend_app_protocol,
                gateway_key_ref,
            )
        }
    }
}

/// 三个 LB-based kind 分支的公共逻辑。
///
/// 入参：已经选出来的 backend + 对应的 `PortLookupSource`。
/// 作用：解析 targetPort → 提取 TLS → 记录 hash_key/upstream → 构造 peer + app protocol。
#[allow(clippy::too_many_arguments)]
fn build_peer_from_lb_selection(
    ctx: &mut EdgionHttpContext,
    session: &Session,
    service_key: &str,
    br_port: Option<i32>,
    lb_policy: &Option<ParsedLBPolicy>,
    backend_tls_policy: &Option<Arc<BackendTLSPolicy>>,
    backend: pingora_load_balancing::Backend,
    port_source: PortLookupSource,
    backend_app_protocol: Option<BackendAppProtocol>,
    gateway_key_ref: Option<&str>,
) -> Result<Box<HttpPeer>, EdgionStatus> {
    let lb_addr = backend.addr.clone();
    let mut peer_addr = lb_addr.clone();
    resolve_target_port(&mut peer_addr, br_port, service_key, port_source);

    let (use_tls, sni) = extract_tls_config(backend_tls_policy);
    let lb_policy_clone = lb_policy.clone();

    if ctx.gateway_info.metrics_test_type.is_some() && matches!(lb_policy, Some(ParsedLBPolicy::ConsistentHash(_))) {
        let hash_key_bytes = extract_hash_key(session, lb_policy);
        ctx.hash_key = String::from_utf8(hash_key_bytes).ok();
    }

    if let Some(upstream) = ctx.get_current_upstream_mut() {
        upstream.backend_addr = Some(peer_addr.clone());
        upstream.lb_backend_addr = Some(lb_addr);
        upstream.service_key = Some(service_key.to_string());
        upstream.lb_policy = lb_policy_clone;
        if use_tls {
            upstream.tls = Some(crate::types::BackendTlsInfo {
                sni: if sni.is_empty() { None } else { Some(sni.clone()) },
                handshake_ok: None,
                protocol: None,
                cipher: None,
            });
        }
    }

    let mut peer = create_tls_peer(peer_addr, backend_tls_policy, gateway_key_ref)?;
    apply_backend_app_protocol(&mut peer, backend_app_protocol, ctx);
    Ok(peer)
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
pub async fn get_peer(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    is_grpc: bool,
) -> pingora_core::Result<Box<HttpPeer>> {
    match try_get_peer(ctx, session, is_grpc) {
        Ok(peer) => Ok(peer),
        Err(status) => {
            ctx.add_error(status);
            let server_header_opts = crate::core::gateway::runtime::server::server_header::ServerHeaderOpts::default();
            match status {
                EdgionStatus::BackendUnsupportedKind
                | EdgionStatus::BackendServiceNotFound
                | EdgionStatus::RefDenied
                | EdgionStatus::BackendServiceImportNotImplemented => {
                    let _ = end_response_500(session, ctx, &server_header_opts).await;
                }
                _ => {
                    let _ = end_response_503(session, ctx, &server_header_opts).await;
                }
            }
            Err(PingoraError::new(ErrorType::InternalError))
        }
    }
}
