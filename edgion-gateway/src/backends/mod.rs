pub mod circuit_breaker;
pub mod degrade_threshold;
pub mod discovery;
pub mod gateway_backend_tls_store;
pub mod health;
pub mod outlier_detection;
pub(crate) mod peer_builder;
pub mod policy;
#[cfg(feature = "boringssl")]
mod prepared_backend_tls;
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
pub(crate) use runtime::{CompiledL4Backends, L4BackendRejection, L4ResolveError, SelectedL4Backend};
pub use runtime::{
    CompiledServiceTarget, LogicalBackendHandle, SelectedHttpBackend, SelectedLogicalBackend, SelectedServiceBackend,
};
pub use selector::{
    backends_for_service_section, select_service_endpoint, service_section_key_from_port, ServiceDegradePolicy,
};
pub use service_kind::ServiceBackendKind;
pub use validation::validate_endpoint_in_route;

use crate::routes::common::UNSAFE_UPSTREAM_ADDRESS_LOG;
use crate::runtime::{GatewayInfo, ServerHeaderOpts};
use crate::EdgionHttpContext;
use crate::{end_response_500, end_response_503};
use discovery::DnsCache;
use edgion_common::dns::resolver_select::{resolve_effective_spec, LinkSysKey, ResolverCandidate};
use edgion_common::dns::{resolve, DnsError, ResolverSpec};
use edgion_resources::constants::annotations::edgion as edgion_annotations;
use edgion_resources::resources::lb_policy::ParsedLBPolicy;
use edgion_resources::EdgionErrorCode;
use peer_builder::first_safe_inet_addr;
use peer_builder::{
    apply_backend_app_protocol, extract_tls_config, peer_from_backend_tls_lookup, record_tls_to_upstream,
};
use pingora_core::prelude::HttpPeer;
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_core::{Error as PingoraError, ErrorType};
use pingora_proxy::Session;
use std::future::Future;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use std::sync::Arc;

use selector::{extract_hash_key, CapturedServiceSelection, ResolvedServiceEndpoint};

#[derive(Debug)]
struct ServicePeerInput<'a> {
    service: &'a CompiledServiceTarget,
    backend_tls_policy: &'a BackendTlsPolicyLookup,
    lb_policy: Option<ParsedLBPolicy>,
}

pub(crate) fn service_context_matches(
    backend_context: Option<&crate::BackendContext>,
    service: &CompiledServiceTarget,
) -> bool {
    backend_context.is_some_and(|context| {
        context.service_key.as_deref() == Some(service.key())
            && context.service_kind.as_ref() == Some(&ServiceBackendKind::Service)
    })
}

fn selected_http_service_input<'a>(
    selected: &'a SelectedServiceBackend,
    backend_context: Option<&crate::BackendContext>,
) -> Result<ServicePeerInput<'a>, EdgionErrorCode> {
    let service = selected
        .logical()
        .service_target()
        .ok_or(EdgionErrorCode::UpstreamNotBackendRefs)?;
    if !service_context_matches(backend_context, service) {
        return Err(EdgionErrorCode::UpstreamNotBackendRefs);
    }
    Ok(ServicePeerInput {
        service,
        backend_tls_policy: selected.backend_tls_policy(),
        lb_policy: captured_lb_policy(backend_context),
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
    try_get_peer_with_capture(ctx, session, is_grpc, selected_http, |target| {
        CapturedServiceSelection::capture(get_service_discovery_coordinator(), target)
    })
    .await
}

async fn try_get_peer_with_capture<C>(
    ctx: &mut EdgionHttpContext,
    session: &Session,
    is_grpc: bool,
    selected_http: Option<&SelectedLogicalBackend>,
    capture: C,
) -> Result<Box<HttpPeer>, EdgionErrorCode>
where
    C: FnOnce(&CompiledServiceTarget) -> Result<CapturedServiceSelection, EdgionErrorCode>,
{
    // Keep the shared identity alive while peer selection mutates the context.
    let gateway_info = ctx.gateway_info.clone();
    let gateway_key_ref = gateway_info.as_deref().map(GatewayInfo::gateway_key);

    // Pin the selected gRPC generation independently of the mutable context.
    let grpc_selection = if is_grpc {
        ctx.selected_grpc_backend.clone()
    } else {
        None
    };
    let input = if is_grpc {
        let selected = grpc_selection
            .as_ref()
            .ok_or(EdgionErrorCode::GrpcUpstreamNotBackendRefs)?;
        let service = selected
            .compiled()
            .permitted_service()
            .ok_or(EdgionErrorCode::BackendUnsupportedKind)?;
        if !service_context_matches(ctx.backend_context.as_ref(), service) {
            return Err(EdgionErrorCode::UpstreamNotBackendRefs);
        }
        ServicePeerInput {
            service,
            backend_tls_policy: selected.backend_tls_policy(),
            lb_policy: captured_lb_policy(ctx.backend_context.as_ref()),
        }
    } else {
        match selected_http {
            Some(SelectedLogicalBackend::Service(selected)) => {
                selected_http_service_input(selected, ctx.backend_context.as_ref())?
            }
            Some(SelectedLogicalBackend::Ai(_)) => return Err(EdgionErrorCode::UpstreamNotBackendRefs),
            None if matches!(ctx.selected_backend, Some(SelectedHttpBackend::Invalid(_))) => {
                return Err(EdgionErrorCode::BackendUnsupportedKind)
            }
            None => return Err(EdgionErrorCode::UpstreamNotBackendRefs),
        }
    };
    if input.backend_tls_policy.is_invalid() {
        return Err(EdgionErrorCode::BackendTlsPolicyInvalid);
    }
    let captured = capture(input.service)?;
    if ctx.response_cache.is_some() {
        crate::response_cache::record_discovery(ctx, captured.response_cache_stamp(input.service));
    }
    let degrade_policy = captured_degrade_policy(ctx.backend_context.as_ref());
    match native_service_path(&captured)? {
        ExternalNamePath::EndpointSlice => {
            // Only ConsistentHash consumes RemoteIp; other algorithms keep the
            // existing allocation-free request input preparation.
            let remote_ip = if matches!(input.lb_policy, Some(ParsedLBPolicy::ConsistentHash(_))) {
                ctx.request_info.remote_ip.clone()
            } else {
                String::new()
            };
            let query = ctx.query(session.req_header());
            let endpoint = take_sp_target_or_select(ctx, &captured, || {
                captured.select(&input.lb_policy, degrade_policy, Some(session), &remote_ip, query)
            })?;
            build_peer_from_lb_selection(
                ctx,
                session,
                input.service.key(),
                input.lb_policy,
                input.backend_tls_policy,
                endpoint,
                gateway_key_ref,
            )
        }
        ExternalNamePath::SelfResolve { cache } => {
            // This attempt owns one Service geometry and one resolver candidate/TTL
            // generation through the await. A later attempt recaptures both.
            let egc_default = crate::config::get_egc_dns_resolver_default();
            let cache_ttl = egc_default
                .as_ref()
                .map(|resolved| resolved.cache_ttl())
                .unwrap_or(DEFAULT_DNS_CACHE_TTL);
            let spec = compute_external_name_resolver(
                captured.dns_resolver_candidate(),
                ctx,
                egc_default.as_ref().and_then(|resolved| resolved.candidate()),
            );
            let dns = ExternalNameResolution {
                spec: &spec,
                cache,
                ttl: cache_ttl,
                dns_cache: get_global_dns_cache(),
            };
            // Self-resolution has no EndpointSlice affinity contract: a sticky
            // target cannot replace the captured hostname's admitted DNS result.
            build_external_name_peer_with_resolver(ctx, &input, &captured, gateway_key_ref, dns, resolve).await
        }
    }
}

fn external_name(captured: &CapturedServiceSelection) -> Result<&str, EdgionErrorCode> {
    captured
        .source()
        .spec
        .as_ref()
        .and_then(|spec| spec.external_name.as_deref())
        .filter(|name| !name.is_empty())
        .ok_or(EdgionErrorCode::BackendExternalNameNotFound)
}

fn native_service_path(captured: &CapturedServiceSelection) -> Result<ExternalNamePath, EdgionErrorCode> {
    if captured.source().spec.as_ref().and_then(|spec| spec.type_.as_deref()) != Some("ExternalName") {
        return Ok(ExternalNamePath::EndpointSlice);
    }
    external_name(captured)?;
    let mode = DnsResolutionMode::parse(captured.source().metadata.annotations.as_ref().and_then(|annotations| {
        annotations
            .get(edgion_annotations::DNS_RESOLUTION_MODE)
            .map(String::as_str)
    }));
    Ok(decide_external_name_path(
        mode,
        mode == DnsResolutionMode::Controller && !captured.section().ready_slots.is_empty(),
    ))
}

struct ExternalNameResolution<'a> {
    spec: &'a ResolverSpec,
    dns_cache: &'a DnsCache,
    cache: bool,
    ttl: Duration,
}

async fn build_external_name_peer_with_resolver<'a, R, Fut>(
    ctx: &mut EdgionHttpContext,
    input: &ServicePeerInput<'_>,
    captured: &'a CapturedServiceSelection,
    gateway_key: Option<&str>,
    dns: ExternalNameResolution<'a>,
    resolver: R,
) -> Result<Box<HttpPeer>, EdgionErrorCode>
where
    R: FnOnce(&'a ResolverSpec, &'a str) -> Fut,
    Fut: Future<Output = Result<Vec<IpAddr>, DnsError>>,
{
    let hostname = external_name(captured)?;
    let port = captured.section().identity.number.get();
    let backend_app_protocol = captured.section().prepared_app_protocol;
    let ips = resolve_external_ips_with(
        hostname,
        dns.spec,
        input.service.key(),
        dns.cache,
        dns.ttl,
        dns.dns_cache,
        resolver,
    )
    .await
    .map_err(|error| {
        if let Some(upstream) = ctx.get_current_upstream_mut() {
            upstream.port = Some(port);
            upstream.err.push(format!("dns resolution failed: {error}"));
        }
        EdgionErrorCode::BackendDnsResolutionFailed
    })?;
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
    // TLS absence remains plaintext, including Service port 443.
    let (use_tls, sni) = extract_tls_config(input.backend_tls_policy);
    record_tls_to_upstream(ctx, use_tls, sni);
    let mut peer = peer_from_backend_tls_lookup(addr, input.backend_tls_policy, gateway_key)?;
    apply_backend_app_protocol(&mut peer, backend_app_protocol, ctx);
    Ok(peer)
}

/// Compute the effective [`ResolverSpec`] for an `ExternalName` self-resolution,
/// applying the data-plane precedence Service annotation → Gateway annotation →
/// EGC default → system. httpdns refs are resolved against the gateway's LinkSys
/// store; a dangling ref fails safe to the system resolver.
///
/// `egc_candidate` is the precomputed EGC data-plane default, read once by the
/// caller so this stays free of the store access and config parsing.
fn compute_external_name_resolver(
    service_layer: Option<&ResolverCandidate>,
    ctx: &EdgionHttpContext,
    egc_candidate: Option<&ResolverCandidate>,
) -> ResolverSpec {
    compute_external_name_resolver_with(service_layer, ctx, egc_candidate, httpdns_config_for_ref)
}

fn compute_external_name_resolver_with<F>(
    service_layer: Option<&ResolverCandidate>,
    ctx: &EdgionHttpContext,
    egc_candidate: Option<&ResolverCandidate>,
    lookup: F,
) -> ResolverSpec
where
    F: Fn(&LinkSysKey) -> Option<Arc<edgion_common::dns::PreparedHttpDns>>,
{
    let gateway_layer = ctx
        .gateway_info
        .as_deref()
        .and_then(|info| info.dns_resolver_candidate.as_ref());
    let egc_layer = egc_candidate;

    resolve_effective_spec(&[service_layer, gateway_layer, egc_layer], lookup)
}

/// Resolve the request's Gateway-scoped DNS policy for a routed external
/// EdgionBackend. Unlike ExternalName there is no Service annotation layer.
pub(crate) fn compute_routed_external_resolver(ctx: &EdgionHttpContext) -> ResolverSpec {
    let gateway_layer = ctx
        .gateway_info
        .as_deref()
        .and_then(|info| info.dns_resolver_candidate.as_ref());
    let egc_default = crate::config::get_egc_dns_resolver_default();
    let egc_layer = egc_default.as_ref().and_then(|resolved| resolved.candidate());
    resolve_effective_spec(&[gateway_layer, egc_layer], |key: &LinkSysKey| {
        httpdns_config_for_ref(key)
    })
}

/// Look up a prepared HTTP DNS generation by `namespace/name` from the
/// gateway's typed LinkSys runtime table. Returns `None` when the ref is missing or the target
/// LinkSys is not an httpdns resolver.
fn httpdns_config_for_ref(key: &LinkSysKey) -> Option<Arc<edgion_common::dns::PreparedHttpDns>> {
    let store_key = format!("{}/{}", key.namespace, key.name);
    crate::link_sys::runtime::store::get_httpdns_client(&store_key)
}

/// Resolve an `ExternalName` to IPs through `spec`, honoring the short-TTL
/// data-plane cache when `cache` is set.
///
/// An IP-literal `external_name` short-circuits the resolver (and the cache):
/// there is nothing to resolve, and running a name resolver on a literal would
/// be both wrong and wasteful.
async fn resolve_external_ips_with<'a, R, Fut>(
    external_name: &'a str,
    spec: &'a ResolverSpec,
    service_key: &str,
    cache: bool,
    cache_ttl: Duration,
    dns_cache: &DnsCache,
    resolver: R,
) -> Result<Vec<IpAddr>, DnsError>
where
    R: FnOnce(&'a ResolverSpec, &'a str) -> Fut,
    Fut: Future<Output = Result<Vec<IpAddr>, DnsError>>,
{
    if let Ok(ip) = external_name.parse::<IpAddr>() {
        return Ok(vec![ip]);
    }
    if !cache {
        return resolver(spec, external_name).await;
    }
    let fingerprint = spec.fingerprint();
    if let Some(cached) = dns_cache.get(service_key, external_name, &fingerprint, Instant::now()) {
        return Ok(cached);
    }
    let ips = resolver(spec, external_name).await?;
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

/// Session-persistence-aware selection within one captured Service section.
///
/// If session-persistence `target_addr` is set (the request carried a sticky-session token resolved
/// to a known backend), use that target directly — skipping LB — provided the target
/// is still healthy. The address is `take()`-d unconditionally so a follow-up retry on
/// this request does not double-consume it.
///
/// If `target_addr` is unset, call `select_fn` for the captured section's normal LB path.
///
/// Note: an unhealthy sticky target returns `StickyBackendUnhealthy` rather than calling
/// `select_fn`. Caller (`get_peer_with_sp_fallback` in upstream_peer) is responsible for
/// deciding whether to re-run with normal LB based on `strict` mode.
fn take_sp_target_or_select<F>(
    ctx: &mut EdgionHttpContext,
    captured: &CapturedServiceSelection,
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
        match captured.endpoint_for_addr(target_addr) {
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

/// Shared logic for ordinary and controller-resolved ExternalName Services.
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
    lb_policy: Option<ParsedLBPolicy>,
    backend_tls_policy: &BackendTlsPolicyLookup,
    endpoint: ResolvedServiceEndpoint,
    gateway_key_ref: Option<&str>,
) -> Result<Box<HttpPeer>, EdgionErrorCode> {
    let backend = endpoint.backend;
    let backend_app_protocol = endpoint.section.prepared_app_protocol;
    let lb_addr = backend.addr.clone();
    let peer_addr = lb_addr.clone();
    if let Err(status) = validate_endpoint_backend_addr(&peer_addr, ctx.allow_loopback_upstream) {
        record_unsafe_upstream_attempt(ctx, &peer_addr, status);
        return Err(status);
    }

    let (use_tls, sni) = extract_tls_config(backend_tls_policy);

    if ctx
        .gateway_info
        .as_deref()
        .is_some_and(|info| info.metrics_test_type.is_some())
        && matches!(&lb_policy, Some(ParsedLBPolicy::ConsistentHash(_)))
    {
        let remote_ip = ctx.request_info.remote_ip.clone();
        let query = ctx.query(session.req_header());
        let hash_key_bytes = extract_hash_key(session, &lb_policy, &remote_ip, query);
        ctx.hash_key = std::str::from_utf8(hash_key_bytes).ok().map(str::to_owned);
    }

    if let Some(upstream) = ctx.get_current_upstream_mut() {
        upstream.lb_outlier_completion = get_outlier_detection_store().capture_completion_token(&backend);
        upstream.lb_backend_addr = Some(lb_addr);
        upstream.lb_backend = Some(backend);
        upstream.service_key = Some(service_key.to_string());
        upstream.lb_policy = lb_policy;
    }
    // Write TLS metadata via the shared helper (writes unconditionally so a
    // TLS-then-plaintext fallback cannot leave stale metadata behind). Kept
    // outside the block above because the helper borrows `ctx` mutably.
    record_tls_to_upstream(ctx, use_tls, sni);

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

#[cfg(test)]
#[path = "../../tests/unit/backends/mod_test/native_service_dispatch_tests.rs"]
mod native_service_dispatch_tests;
