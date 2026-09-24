pub mod circuit_breaker;
pub mod degrade_threshold;
pub mod discovery;
pub mod gateway_backend_tls_store;
pub mod health;
pub mod outlier_detection;
pub(crate) mod peer_builder;
pub mod policy;
pub mod runtime;
pub mod selector;
pub mod service_kind;
pub mod validation;

pub use degrade_threshold::{
    configure_lb_degrade_default, degrade_floor, get_degrade_threshold_store, get_lb_degrade_default,
    resolve_degrade_threshold,
};
pub use discovery::{
    create_ep_slice_handler, create_service_handler, decide_external_name_path, get_global_dns_cache,
    get_global_service_store, get_roundrobin_store, get_service_discovery_coordinator, DnsResolutionMode,
    EndpointSliceExt, EpSliceStore, ExternalNamePath, ServiceStore, DEFAULT_DNS_CACHE_TTL,
};
pub use outlier_detection::{get_outlier_detection_store, FailureClass, OutlierCompletionToken};

pub use gateway_backend_tls_store::{get_global_gateway_backend_tls_store, GatewayBackendTlsStore};
pub use health::{get_health_check_manager, get_health_status_store};
pub(crate) use peer_builder::validate_endpoint_backend_addr;
pub use policy::{
    create_backend_tls_policy_handler, get_global_backend_tls_policy_store,
    query_backend_tls_policy_for_logical_backend, BackendTLSPolicyStore, BackendTlsPolicyLookup,
    EffectiveBackendTlsPolicy,
};
pub use runtime::{
    LogicalBackendHandle, SelectedAiBackend, SelectedHttpBackend, SelectedLogicalBackend, SelectedServiceBackend,
};
pub use selector::{
    backends_for_service_section, select_service_endpoint, service_section_key_from_port, ServiceDegradePolicy,
};
pub use service_kind::ServiceBackendKind;
pub use validation::validate_endpoint_in_route;

use crate::link_sys::get_global_link_sys_store;
use crate::routes::common::UNSAFE_UPSTREAM_ADDRESS_LOG;
use crate::runtime::{GatewayInfo, ServerHeaderOpts};
use crate::EdgionHttpContext;
use crate::{end_response_500, end_response_503};
use edgion_common::dns::resolver_select::{
    candidate_from_annotations, resolve_effective_spec, LinkSysKey, ResolverCandidate,
};
use edgion_common::dns::{resolve, ResolverSpec};
use edgion_resources::constants::annotations::edgion as edgion_annotations;
use edgion_resources::resources::lb_policy::ParsedLBPolicy;
use edgion_resources::resources::link_sys::HttpDnsResolverConfig;
use edgion_resources::EdgionErrorCode;
use peer_builder::first_safe_inet_addr;
use peer_builder::{
    apply_backend_app_protocol, extract_tls_config, parse_backend_app_protocol, peer_from_backend_tls_lookup,
    record_tls_to_upstream, validate_backend_addr,
};
use pingora_core::prelude::HttpPeer;
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_core::{Error as PingoraError, ErrorType};
use pingora_proxy::Session;
use std::net::IpAddr;
use std::time::{Duration, Instant};

#[cfg(test)]
use std::sync::Arc;

use crate::backends::discovery::ServiceSectionKey;
use selector::{extract_hash_key, prove_service_section, resolved_endpoint_for_addr, ResolvedServiceEndpoint};

#[derive(Debug)]
struct ServicePeerInput {
    port: Option<i32>,
    backend_tls_policy: BackendTlsPolicyLookup,
    service_key: String,
    service_type: ServiceBackendKind,
    lb_policy: Option<ParsedLBPolicy>,
}

/// Derive HTTP Service transport identity from the selected proof and use the
/// mutable context only as an invariant check. A stale/mismatched context must
/// never redirect a selected logical backend to another Service.
fn selected_http_service_input(
    selected: &SelectedServiceBackend,
    backend_context: Option<&crate::BackendContext>,
) -> Result<ServicePeerInput, EdgionErrorCode> {
    let compiled = selected.logical().compiled();
    let (namespace, name) = match selected.logical().target() {
        crate::routes::http::match_unit::CompiledBackendTarget::Service { namespace, name } => (namespace, name),
        crate::routes::http::match_unit::CompiledBackendTarget::EdgionBackend { .. }
        | crate::routes::http::match_unit::CompiledBackendTarget::Invalid { .. } => {
            return Err(EdgionErrorCode::UpstreamNotBackendRefs);
        }
    };
    let service_key = format!("{namespace}/{name}");
    let service_kind = ServiceBackendKind::from_kind(compiled.kind.as_ref());
    let context_matches = backend_context.is_some_and(|context| {
        context.service_key.as_deref() == Some(service_key.as_str())
            && context.service_kind.as_ref() == Some(&service_kind)
    });
    if !context_matches {
        return Err(EdgionErrorCode::UpstreamNotBackendRefs);
    }
    Ok(ServicePeerInput {
        port: compiled.port,
        backend_tls_policy: selected.backend_tls_policy().clone(),
        lb_policy: captured_lb_policy(backend_context),
        service_key,
        service_type: service_kind,
    })
}

fn captured_lb_policy(backend_context: Option<&crate::BackendContext>) -> Option<ParsedLBPolicy> {
    backend_context
        .and_then(|context| context.effective_policy.as_ref())
        .and_then(|policy| policy.load_balancer.clone())
}

pub(crate) fn captured_degrade_policy(backend_context: Option<&crate::BackendContext>) -> ServiceDegradePolicy {
    ServiceDegradePolicy::CapturedOverride(
        backend_context
            .and_then(|context| context.effective_policy.as_ref())
            .and_then(|policy| policy.degrade_threshold_override),
    )
}

/// Internal: try to get peer, returns Result with EdgionErrorCode on error
pub(crate) async fn try_get_peer(
    ctx: &mut EdgionHttpContext,
    session: &Session,
    is_grpc: bool,
    selected_http: Option<&SelectedLogicalBackend>,
) -> Result<Box<HttpPeer>, EdgionErrorCode> {
    // Keep the shared identity alive while peer selection mutates the context.
    let gateway_info = ctx.gateway_info.clone();
    let gateway_key_ref = gateway_info.as_deref().map(GatewayInfo::gateway_key);

    // HTTP routed Service identity comes exclusively from the selected proof.
    // gRPC and Invalid retain their existing context compatibility input.
    let input = if is_grpc {
        // gRPC backend
        let grpc_br = ctx
            .selected_grpc_backend
            .as_ref()
            .ok_or(EdgionErrorCode::GrpcUpstreamNotBackendRefs)?;
        let (service_key, service_type) = ctx
            .backend_context
            .as_ref()
            .and_then(|bc| match (bc.service_key.clone(), bc.service_kind.clone()) {
                (Some(key), Some(kind)) => Some((key, kind)),
                _ => None,
            })
            .ok_or(EdgionErrorCode::UpstreamNotBackendRefs)?;
        let lb_policy = captured_lb_policy(ctx.backend_context.as_ref());
        ServicePeerInput {
            port: grpc_br.compiled().port,
            backend_tls_policy: grpc_br.backend_tls_policy().clone(),
            service_key,
            lb_policy,
            service_type,
        }
    } else if let Some(selected) = selected_http {
        match selected {
            SelectedLogicalBackend::Service(selected) => {
                selected_http_service_input(selected, ctx.backend_context.as_ref())?
            }
            SelectedLogicalBackend::Ai(_) => return Err(EdgionErrorCode::UpstreamNotBackendRefs),
        }
    } else {
        let http_br = match ctx.selected_backend.as_ref() {
            Some(SelectedHttpBackend::Invalid(logical)) => logical.compiled(),
            Some(SelectedHttpBackend::Service(_)) | Some(SelectedHttpBackend::Ai(_)) | None => {
                return Err(EdgionErrorCode::UpstreamNotBackendRefs);
            }
        };
        let (service_key, service_type) = ctx
            .backend_context
            .as_ref()
            .and_then(|bc| match (bc.service_key.clone(), bc.service_kind.clone()) {
                (Some(key), Some(kind)) => Some((key, kind)),
                _ => None,
            })
            .ok_or(EdgionErrorCode::UpstreamNotBackendRefs)?;
        let lb_policy = captured_lb_policy(ctx.backend_context.as_ref());
        ServicePeerInput {
            port: http_br.port,
            // Invalid targets cannot resolve a typed BackendTLSPolicy. The old
            // clone-and-mutate path also replaced any schema-local value with
            // `None` before this branch.
            backend_tls_policy: BackendTlsPolicyLookup::Absent,
            service_key,
            lb_policy,
            service_type,
        }
    };
    let ServicePeerInput {
        port: br_port,
        backend_tls_policy,
        service_key,
        service_type,
        lb_policy,
    } = input;
    if backend_tls_policy.is_invalid() {
        return Err(EdgionErrorCode::BackendTlsPolicyInvalid);
    }

    let section_key = service_section_key_from_port(br_port)?;
    let degrade_policy = captured_degrade_policy(ctx.backend_context.as_ref());

    match service_type {
        ServiceBackendKind::Service => {
            if !get_global_service_store().contains(&service_key) {
                return Err(EdgionErrorCode::BackendServiceNotFound);
            }

            // Only ConsistentHash consumes RemoteIp; skip the clone (and its
            // allocation) for the RR/LeastConn/Ewma hot paths.
            let remote_ip = if matches!(lb_policy, Some(ParsedLBPolicy::ConsistentHash(_))) {
                ctx.request_info.remote_ip.clone()
            } else {
                String::new()
            };
            let query = ctx.query(session.req_header());
            let endpoint = take_sp_target_or_select(ctx, &service_key, &section_key, || {
                select_service_endpoint(
                    &service_key,
                    section_key.clone(),
                    &lb_policy,
                    degrade_policy,
                    Some(session),
                    &remote_ip,
                    query,
                )
            })?;

            build_peer_from_lb_selection(
                ctx,
                session,
                &service_key,
                &lb_policy,
                &backend_tls_policy,
                endpoint,
                gateway_key_ref,
            )
        }

        ServiceBackendKind::ServiceClusterIp => {
            let svc_store = get_global_service_store();
            let service = svc_store
                .get(&service_key)
                .ok_or(EdgionErrorCode::BackendServiceNotFound)?;

            let cluster_ip = service
                .spec
                .as_ref()
                .and_then(|spec| spec.cluster_ip.as_ref())
                .ok_or(EdgionErrorCode::BackendClusterIpNotFound)?;

            let section = prove_service_section(&service_key, &section_key)?;
            let port = section.identity.number.get();
            let backend_app_protocol = section
                .identity
                .app_protocol
                .as_deref()
                .and_then(parse_backend_app_protocol);

            // cluster_ip is a bare IP literal (no brackets). Parse it as IpAddr and
            // build the SocketAddr directly so IPv6 ClusterIP services work without
            // manual bracketing (mirrors the EndpointSlice discovery_impl path).
            let ip: std::net::IpAddr = cluster_ip
                .parse()
                .map_err(|_| EdgionErrorCode::BackendAddressParsingFailed)?;
            let addr = SocketAddr::Inet(std::net::SocketAddr::new(ip, port));

            // Security check: reject loopback / link-local / metadata (SSRF defense)
            if let Err(status) = validate_backend_addr(&addr, ctx.allow_loopback_upstream) {
                record_unsafe_upstream_attempt(ctx, &addr, status);
                return Err(status);
            }

            // Extract TLS configuration and record to upstream
            let (use_tls, sni) = extract_tls_config(&backend_tls_policy);
            record_tls_to_upstream(ctx, use_tls, &sni);

            let mut peer = peer_from_backend_tls_lookup(addr, &backend_tls_policy, gateway_key_ref)?;
            apply_backend_app_protocol(&mut peer, backend_app_protocol, ctx);
            Ok(peer)
        }

        ServiceBackendKind::ServiceExternalName => {
            let svc_store = get_global_service_store();
            let service = svc_store
                .get(&service_key)
                .ok_or(EdgionErrorCode::BackendServiceNotFound)?;

            let external_name = service
                .spec
                .as_ref()
                .and_then(|spec| spec.external_name.as_ref())
                .ok_or(EdgionErrorCode::BackendExternalNameNotFound)?
                .clone();

            let section = prove_service_section(&service_key, &section_key)?;
            let port = section.identity.number.get();
            let backend_app_protocol = section
                .identity
                .app_protocol
                .as_deref()
                .and_then(parse_backend_app_protocol);

            // Mode-first dispatch (edgion.io/dns-resolution-mode on the Service,
            // default "controller"). `controller` prefers a controller-published
            // EndpointSlice and bridges to self-resolution when it has no ready
            // backend; `gateway`/`per-request` always self-resolve (cached / not).
            let svc_annotations = service.metadata.annotations.as_ref();
            let mode = DnsResolutionMode::parse(svc_annotations.and_then(|annotations| {
                annotations
                    .get(edgion_annotations::DNS_RESOLUTION_MODE)
                    .map(String::as_str)
            }));
            // The controller EndpointSlice is only consulted in `controller` mode,
            // so skip the (lock-taking, string-allocating) readiness scan otherwise.
            let slice_has_ready = mode == DnsResolutionMode::Controller
                && !get_roundrobin_store()
                    .ready_endpoints_for_port(&service_key, br_port)
                    .is_empty();

            match decide_external_name_path(mode, slice_has_ready) {
                ExternalNamePath::EndpointSlice => {
                    // Controller loop resolved this ExternalName to a ready
                    // EndpointSlice — route through it like any LB-backed service.
                    // Only ConsistentHash consumes RemoteIp; skip the clone
                    // (and its allocation) for the RR/LeastConn/Ewma hot paths.
                    let remote_ip = if matches!(lb_policy, Some(ParsedLBPolicy::ConsistentHash(_))) {
                        ctx.request_info.remote_ip.clone()
                    } else {
                        String::new()
                    };
                    let query = ctx.query(session.req_header());
                    let endpoint = take_sp_target_or_select(ctx, &service_key, &section_key, || {
                        select_service_endpoint(
                            &service_key,
                            section_key.clone(),
                            &lb_policy,
                            degrade_policy,
                            Some(session),
                            &remote_ip,
                            query,
                        )
                    })?;
                    build_peer_from_lb_selection(
                        ctx,
                        session,
                        &service_key,
                        &lb_policy,
                        &backend_tls_policy,
                        endpoint,
                        gateway_key_ref,
                    )
                }
                ExternalNamePath::SelfResolve { cache } => {
                    // Read the pre-resolved EGC data-plane default once. The Arc
                    // pins a single candidate/TTL generation for this request.
                    let egc_default = crate::config::get_egc_dns_resolver_default();
                    let cache_ttl = egc_default
                        .as_ref()
                        .map(|resolved| resolved.cache_ttl())
                        .unwrap_or(DEFAULT_DNS_CACHE_TTL);

                    // Compute the effective resolver from the data-plane precedence
                    // (Service annotation → Gateway annotation → EGC default → system).
                    let spec = compute_external_name_resolver(
                        svc_annotations,
                        ctx,
                        egc_default.as_ref().and_then(|resolved| resolved.candidate()),
                    );

                    let ips = resolve_external_ips(&external_name, &spec, &service_key, cache, cache_ttl)
                        .await
                        .map_err(|e| {
                            if let Some(upstream) = ctx.get_current_upstream_mut() {
                                upstream.port = Some(port);
                                upstream.err.push(format!("dns resolution failed: {e}"));
                            }
                            EdgionErrorCode::BackendDnsResolutionFailed
                        })?;

                    // SSRF: route only to the first resolved IP that passes the
                    // loopback/link-local gate; reject when none are safe. On the
                    // all-unsafe path `first_safe_inet_addr` returns the *last*
                    // candidate's status, so record the last IP to keep the logged
                    // address and reason consistent.
                    let addr = match first_safe_inet_addr(&ips, port, ctx.allow_loopback_upstream) {
                        Ok(addr) => addr,
                        Err(status) => {
                            if let Some(ip) = ips.last() {
                                let rejected = SocketAddr::Inet(std::net::SocketAddr::new(*ip, port));
                                record_unsafe_upstream_attempt(ctx, &rejected, status);
                            }
                            return Err(status);
                        }
                    };

                    // TLS is disabled when no BackendTLSPolicy is attached — operators
                    // must create one explicitly for FQDN ExternalName backends serving
                    // TLS (e.g. port 443); otherwise the gateway sends plaintext.
                    let (use_tls, sni) = extract_tls_config(&backend_tls_policy);
                    record_tls_to_upstream(ctx, use_tls, &sni);

                    let mut peer = peer_from_backend_tls_lookup(addr, &backend_tls_policy, gateway_key_ref)?;
                    apply_backend_app_protocol(&mut peer, backend_app_protocol, ctx);
                    Ok(peer)
                }
            }
        }

        ServiceBackendKind::ServiceImport => Err(EdgionErrorCode::BackendServiceImportNotImplemented),

        ServiceBackendKind::Unsupported(_) => Err(EdgionErrorCode::BackendUnsupportedKind),

        ServiceBackendKind::ServiceEndpointSlice => {
            // Only ConsistentHash consumes RemoteIp; skip the clone (and its
            // allocation) for the RR/LeastConn/Ewma hot paths.
            let remote_ip = if matches!(lb_policy, Some(ParsedLBPolicy::ConsistentHash(_))) {
                ctx.request_info.remote_ip.clone()
            } else {
                String::new()
            };
            let query = ctx.query(session.req_header());
            let endpoint = take_sp_target_or_select(ctx, &service_key, &section_key, || {
                select_service_endpoint(
                    &service_key,
                    section_key.clone(),
                    &lb_policy,
                    degrade_policy,
                    Some(session),
                    &remote_ip,
                    query,
                )
            })?;
            build_peer_from_lb_selection(
                ctx,
                session,
                &service_key,
                &lb_policy,
                &backend_tls_policy,
                endpoint,
                gateway_key_ref,
            )
        }
    }
}

/// Compute the effective [`ResolverSpec`] for an `ExternalName` self-resolution,
/// applying the data-plane precedence Service annotation → Gateway annotation →
/// EGC default → system. httpdns refs are resolved against the gateway's LinkSys
/// store; a dangling ref fails safe to the system resolver.
///
/// `egc_candidate` is the precomputed EGC data-plane default, read once by the
/// caller so this stays free of the store access and config parsing.
fn compute_external_name_resolver(
    svc_annotations: Option<&std::collections::BTreeMap<String, String>>,
    ctx: &EdgionHttpContext,
    egc_candidate: Option<&ResolverCandidate>,
) -> ResolverSpec {
    let service_layer = svc_annotations.and_then(candidate_from_annotations);
    let gateway_layer = ctx
        .gateway_info
        .as_deref()
        .and_then(|info| info.dns_resolver_candidate.clone());
    let egc_layer = egc_candidate.cloned();

    resolve_effective_spec(&[service_layer, gateway_layer, egc_layer], |key: &LinkSysKey| {
        httpdns_config_for_ref(key)
    })
}

/// Resolve the request's Gateway-scoped DNS policy for a routed external
/// EdgionBackend. Unlike ExternalName there is no Service annotation layer.
pub(crate) fn compute_routed_external_resolver(ctx: &EdgionHttpContext) -> ResolverSpec {
    let gateway_layer = ctx
        .gateway_info
        .as_deref()
        .and_then(|info| info.dns_resolver_candidate.clone());
    let egc_default = crate::config::get_egc_dns_resolver_default();
    let egc_layer = egc_default.as_ref().and_then(|resolved| resolved.candidate()).cloned();
    resolve_effective_spec(&[gateway_layer, egc_layer], |key: &LinkSysKey| {
        httpdns_config_for_ref(key)
    })
}

/// Look up an httpdns [`HttpDnsResolverConfig`] by `namespace/name` from the
/// gateway's LinkSys store. Returns `None` when the ref is missing or the target
/// LinkSys is not an httpdns resolver.
fn httpdns_config_for_ref(key: &LinkSysKey) -> Option<HttpDnsResolverConfig> {
    let store_key = format!("{}/{}", key.namespace, key.name);
    get_global_link_sys_store()
        .get(&store_key)
        .and_then(|ls| ls.spec.config.as_http_dns().cloned())
}

/// Resolve an `ExternalName` to IPs through `spec`, honoring the short-TTL
/// data-plane cache when `cache` is set.
///
/// An IP-literal `external_name` short-circuits the resolver (and the cache):
/// there is nothing to resolve, and running a name resolver on a literal would
/// be both wrong and wasteful.
async fn resolve_external_ips(
    external_name: &str,
    spec: &ResolverSpec,
    service_key: &str,
    cache: bool,
    cache_ttl: Duration,
) -> Result<Vec<IpAddr>, edgion_common::dns::DnsError> {
    if let Ok(ip) = external_name.parse::<IpAddr>() {
        return Ok(vec![ip]);
    }
    if !cache {
        return resolve(spec, external_name).await;
    }
    let dns_cache = get_global_dns_cache();
    let fingerprint = spec.fingerprint();
    if let Some(cached) = dns_cache.get(service_key, external_name, &fingerprint, Instant::now()) {
        return Ok(cached);
    }
    let ips = resolve(spec, external_name).await?;
    dns_cache.insert(
        service_key,
        external_name,
        &fingerprint,
        ips.clone(),
        cache_ttl,
        Instant::now(),
    );
    Ok(ips)
}

/// Session-persistence-aware backend pick for the LB-based service kinds.
///
/// If session-persistence `target_addr` is set (the request carried a sticky-session token resolved
/// to a known backend), use that target directly — skipping LB — provided the target
/// is still healthy. The address is `take()`-d unconditionally so a follow-up retry on
/// this request does not double-consume it.
///
/// If `target_addr` is unset, or the sticky target is unhealthy, fall back to the
/// caller-supplied `select_fn` (the kind's normal LB path).
///
/// Note: an unhealthy sticky target returns `StickyBackendUnhealthy` rather than calling
/// `select_fn`. Caller (`get_peer_with_sp_fallback` in upstream_peer) is responsible for
/// deciding whether to re-run with normal LB based on `strict` mode.
fn take_sp_target_or_select<F>(
    ctx: &mut EdgionHttpContext,
    service_key: &str,
    section: &ServiceSectionKey,
    select_fn: F,
) -> Result<ResolvedServiceEndpoint, EdgionErrorCode>
where
    F: FnOnce() -> Result<ResolvedServiceEndpoint, EdgionErrorCode>,
{
    if let Some(target_addr) = ctx
        .session_persistence_mut()
        .and_then(|persistence| persistence.target_addr.take())
    {
        // Mark consumption regardless of the taken target's health: the
        // healthy/unhealthy outcomes are handled by the in-attempt fallback
        // state machine, while this flag is the cross-attempt refresh evidence.
        ctx.ensure_session_persistence().sticky_consumed = true;
        // Look up the live Arc from the cell so inc_conn/dec_conn/observe_latency
        // operate on the same object that the selector tracks. Wrapping a fresh
        // LbBackend here would create an orphan: metric calls would be on a
        // throwaway object and the real cell backend would always show conn==0.
        match resolved_endpoint_for_addr(service_key, section, target_addr) {
            Some((endpoint, eligible)) => {
                if eligible {
                    return Ok(endpoint);
                }
                return Err(EdgionErrorCode::StickyBackendUnhealthy);
            }
            // SP target no longer in the cell (pod removed after sticky token was
            // issued). Treat as unhealthy so the caller's strict-mode logic and
            // InjectUpdate state machine apply identically to the gone-backend case.
            None => return Err(EdgionErrorCode::StickyBackendUnhealthy),
        }
    }
    select_fn()
}

/// Shared logic for all three LB-based service kinds.
///
/// Takes the already-selected `ResolvedServiceEndpoint`, validates the proven
/// serving address, writes upstream metadata, and constructs the peer.
/// Stores the `Arc<LbBackend>` on `upstream.lb_backend` so that
/// `connected_to_upstream` can call `inc_conn()` and `lb_runtime_cleanup` can
/// call `dec_conn()` / `observe_latency()` on the same live object.
fn build_peer_from_lb_selection(
    ctx: &mut EdgionHttpContext,
    session: &Session,
    service_key: &str,
    lb_policy: &Option<ParsedLBPolicy>,
    backend_tls_policy: &BackendTlsPolicyLookup,
    endpoint: ResolvedServiceEndpoint,
    gateway_key_ref: Option<&str>,
) -> Result<Box<HttpPeer>, EdgionErrorCode> {
    let backend = endpoint.backend;
    let backend_app_protocol = endpoint
        .section
        .identity
        .app_protocol
        .as_deref()
        .and_then(parse_backend_app_protocol);
    let lb_addr = backend.addr.clone();
    let peer_addr = lb_addr.clone();
    if let Err(status) = validate_endpoint_backend_addr(&peer_addr, ctx.allow_loopback_upstream) {
        record_unsafe_upstream_attempt(ctx, &peer_addr, status);
        return Err(status);
    }

    let (use_tls, sni) = extract_tls_config(backend_tls_policy);
    let lb_policy_clone = lb_policy.clone();

    if ctx
        .gateway_info
        .as_deref()
        .is_some_and(|info| info.metrics_test_type.is_some())
        && matches!(lb_policy, Some(ParsedLBPolicy::ConsistentHash(_)))
    {
        let remote_ip = ctx.request_info.remote_ip.clone();
        let query = ctx.query(session.req_header());
        let hash_key_bytes = extract_hash_key(session, lb_policy, &remote_ip, query);
        ctx.hash_key = std::str::from_utf8(hash_key_bytes).ok().map(str::to_owned);
    }

    if let Some(upstream) = ctx.get_current_upstream_mut() {
        upstream.lb_outlier_completion = get_outlier_detection_store().capture_completion_token(&backend);
        upstream.lb_backend_addr = Some(lb_addr);
        upstream.lb_backend = Some(backend);
        upstream.service_key = Some(service_key.to_string());
        upstream.lb_policy = lb_policy_clone;
    }
    // Write TLS metadata via the shared helper (writes unconditionally so a
    // TLS-then-plaintext fallback cannot leave stale metadata behind). Kept
    // outside the block above because the helper borrows `ctx` mutably.
    record_tls_to_upstream(ctx, use_tls, &sni);

    let mut peer = peer_from_backend_tls_lookup(peer_addr, backend_tls_policy, gateway_key_ref)?;
    apply_backend_app_protocol(&mut peer, backend_app_protocol, ctx);
    Ok(peer)
}

fn record_unsafe_upstream_attempt(ctx: &mut EdgionHttpContext, addr: &SocketAddr, status: EdgionErrorCode) {
    let (ip, port) = addr
        .as_inet()
        .map(|addr| (Some(addr.ip().to_string()), Some(addr.port())))
        .unwrap_or((None, None));
    let reason = match status {
        EdgionErrorCode::BackendLocalhostNotAllowed => "unsafe loopback upstream",
        EdgionErrorCode::BackendAddrLinkLocalNotAllowed => "unsafe link-local upstream",
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
    selected_http: Option<&SelectedLogicalBackend>,
    server_header_opts: &ServerHeaderOpts,
) -> pingora_core::Result<Box<HttpPeer>> {
    match try_get_peer(ctx, session, is_grpc, selected_http).await {
        Ok(peer) => Ok(peer),
        Err(status) => {
            ctx.record_error(status);
            match status {
                EdgionErrorCode::BackendUnsupportedKind
                | EdgionErrorCode::BackendServiceNotFound
                | EdgionErrorCode::RefDenied
                | EdgionErrorCode::BackendServiceImportNotImplemented
                | EdgionErrorCode::BackendTlsPolicyInvalid => {
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

#[cfg(test)]
#[path = "../../tests/unit/backends/mod_test/selected_service_input_tests.rs"]
mod selected_service_input_tests;
