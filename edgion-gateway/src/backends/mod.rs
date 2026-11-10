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
    create_ep_slice_handler, create_service_handler, dns_endpoint_table, get_global_service_store,
    get_roundrobin_store, get_service_discovery_coordinator, upstream_dns_cache, DnsEndpointTableStats,
    DnsRefreshPolicy, EndpointSliceExt, EpSliceStore, ServiceStore, DEFAULT_DNS_CACHE_TTL,
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
pub use selector::{backends_for_service_section, service_section_key_from_port, ServiceDegradePolicy};
pub use service_kind::ServiceBackendKind;
pub use validation::validate_endpoint_in_route;

use crate::routes::common::UNSAFE_UPSTREAM_ADDRESS_LOG;
use crate::runtime::{GatewayInfo, ServerHeaderOpts};
use crate::EdgionHttpContext;
use crate::{end_response_500, end_response_503};
use edgion_common::dns::resolver_select::{resolve_effective_spec, LinkSysKey};
use edgion_common::dns::{
    resolve_answer, DnsAnswer, DnsCachePolicy, DnsError, DnsLookupFamily, DnsQueryPolicy, ResolverSpec,
};
#[cfg(test)]
use edgion_resources::constants::annotations::edgion as edgion_annotations;
use edgion_resources::resources::common::DnsResolutionMode;
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
use std::sync::atomic::{AtomicUsize, Ordering};

use std::sync::Arc;

use discovery::{AvailableServiceProjection, ServiceRuntimeSnapshot};
use selector::{extract_hash_key, CapturedServiceSelection, ResolvedServiceEndpoint};

#[derive(Debug)]
struct ServicePeerInput<'a> {
    service: &'a CompiledServiceTarget,
    backend_tls_policy: &'a BackendTlsPolicyLookup,
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
    // A gateway-mode ExternalName target may still await its first answer
    // (Service just appeared or the Gateway just started). Bounded; one atomic
    // load in the steady state.
    dns_endpoint_table().wait_first_answer(input.service.key()).await;
    let captured = capture(input.service)?;
    if ctx.response_cache.is_some() {
        crate::response_cache::record_discovery(ctx, captured.response_cache_stamp(input.service));
    }
    let degrade_policy = captured_degrade_policy(ctx.backend_context.as_ref());
    match native_service_path(&captured)? {
        ExternalNameSource::Endpoints => {
            let lb_policy = captured_lb_policy(ctx.backend_context.as_ref());
            let endpoint = take_sp_target_or_select(ctx, &captured, |ctx| {
                // Borrow request inputs after sticky target consumption releases its mutable borrow.
                let remote_ip = if matches!(lb_policy, Some(ParsedLBPolicy::ConsistentHash(_))) {
                    ctx.request_info.remote_ip.as_str()
                } else {
                    ""
                };
                let query = Some(&ctx.request_query);
                captured.select(&lb_policy, degrade_policy, Some(session), remote_ip, query)
            })?;
            build_peer_from_lb_selection(
                ctx,
                session,
                input.service.key(),
                lb_policy,
                input.backend_tls_policy,
                endpoint,
                gateway_key_ref,
            )
        }
        ExternalNameSource::System => {
            // This attempt owns one Service geometry through the await; a later
            // attempt recaptures it. Self-resolution has no EndpointSlice
            // affinity contract, so a sticky target is never consumed here.
            let egc_family = crate::config::get_egc_dns_resolver_default().map(|resolved| resolved.lookup_family());
            let family = external_name_family(captured.dns_lookup_family(), egc_family)
                .map_err(|_| EdgionErrorCode::BackendDnsResolutionFailed)?;
            build_system_external_name_peer(ctx, &input, &captured, gateway_key_ref, family, resolve_answer).await
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

/// Where an attempt for a captured Service gets its endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExternalNameSource {
    /// Ordinary EndpointSlice selection. For an `ExternalName` Service the
    /// coordinator feeds it from the Gateway endpoint table (`gateway` mode) or
    /// from Controller-published slices (`controller` mode), never both.
    Endpoints,
    /// OS resolver on every attempt (`system` mode).
    System,
}

/// Pure mode decision: the Service annotation, else the EGC global default.
/// An invalid annotation fails closed. There is no cross-source fallback.
pub(crate) fn external_name_source(
    annotation: Result<Option<DnsResolutionMode>, &'static str>,
    endpoint_table: bool,
) -> Result<ExternalNameSource, EdgionErrorCode> {
    let mode = annotation
        .map_err(|_| EdgionErrorCode::BackendDnsResolutionFailed)?
        .unwrap_or(DnsResolutionMode::global_default(endpoint_table));
    Ok(match mode {
        DnsResolutionMode::System => ExternalNameSource::System,
        DnsResolutionMode::Gateway | DnsResolutionMode::Controller => ExternalNameSource::Endpoints,
    })
}

fn native_service_path(captured: &CapturedServiceSelection) -> Result<ExternalNameSource, EdgionErrorCode> {
    if captured.source().spec.as_ref().and_then(|spec| spec.type_.as_deref()) != Some("ExternalName") {
        return Ok(ExternalNameSource::Endpoints);
    }
    external_name(captured)?;
    if matches!(captured.dns_lookup_family(), Some(Err(_))) {
        return Err(EdgionErrorCode::BackendDnsResolutionFailed);
    }
    let endpoint_table =
        crate::config::get_egc_dns_resolver_default().is_none_or(|resolved| resolved.refresh_policy().endpoint_table);
    external_name_source(captured.dns_resolution_mode(), endpoint_table)
}

/// `ExternalName` family precedence: Service annotation, then the EGC
/// default, then `ipv4Only`. Shared with the Gateway endpoint table; there is
/// no Gateway-resource layer for `ExternalName`.
pub(crate) fn external_name_family(
    service: Option<Result<DnsLookupFamily, &'static str>>,
    egc: Option<DnsLookupFamily>,
) -> Result<DnsLookupFamily, DnsError> {
    service
        .transpose()
        .map(|family| family.or(egc).unwrap_or_default())
        .map_err(|error| DnsError::Parse(error.into()))
}

/// Addresses dialed by this request's earlier attempts (the current attempt's
/// placeholder has none yet).
fn previously_dialed_ips(ctx: &EdgionHttpContext) -> Vec<std::net::IpAddr> {
    ctx.backend_context
        .as_ref()
        .map(|context| {
            context
                .upstreams
                .iter()
                .filter_map(|upstream| upstream.ip.as_deref()?.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Stable-partition `ips` so addresses not yet dialed come first; when every
/// address was dialed the order is unchanged.
fn prefer_undialed(ips: &mut [std::net::IpAddr], dialed: &[std::net::IpAddr]) {
    if !dialed.is_empty() {
        ips.sort_by_key(|ip| dialed.contains(&ip.to_canonical()) || dialed.contains(ip));
    }
}

/// `system` mode always asks the OS resolver, whatever resolver the Service or
/// EGC names for the endpoint table.
static SYSTEM_RESOLVER: ResolverSpec = ResolverSpec::System;

async fn build_system_external_name_peer<'a, R, Fut>(
    ctx: &mut EdgionHttpContext,
    input: &ServicePeerInput<'_>,
    captured: &'a CapturedServiceSelection,
    gateway_key: Option<&str>,
    family: DnsLookupFamily,
    resolver: R,
) -> Result<Box<HttpPeer>, EdgionErrorCode>
where
    R: FnOnce(&'a ResolverSpec, &'a str, DnsQueryPolicy) -> Fut,
    Fut: Future<Output = Result<DnsAnswer, DnsError>>,
{
    let hostname = external_name(captured)?;
    let port = captured.section().identity.number.get();
    let backend_app_protocol = captured.section().prepared_app_protocol;
    // IP literals short-circuit inside the resolver, under the family policy.
    let mut ips = resolver(&SYSTEM_RESOLVER, hostname, DnsQueryPolicy::Family(family))
        .await
        .map(|answer| answer.addrs)
        .map_err(|error| {
            if let Some(upstream) = ctx.get_current_upstream_mut() {
                upstream.port = Some(port);
                upstream.err.push(format!("dns resolution failed: {error}").into());
            }
            EdgionErrorCode::BackendDnsResolutionFailed
        })?;
    // Keep the OS order but rotate the start, so traffic spreads over every
    // address; a retry prefers an address no earlier attempt dialed.
    rotate_addresses(&mut ips, next_address_rotation());
    prefer_undialed(&mut ips, &previously_dialed_ips(ctx));
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

/// Outcome of the async L4 step that runs before synchronous endpoint selection.
pub(crate) enum L4ServicePrep {
    /// Select from the discovery projection, reusing the one the step loaded.
    Select(Option<(Arc<AvailableServiceProjection>, Arc<ServiceRuntimeSnapshot>)>),
    /// `system` mode: dial this freshly resolved address (no pooled endpoint)
    /// of `hostname`.
    Direct {
        backend: Arc<crate::lb::LbBackend>,
        hostname: Box<str>,
    },
    /// `system`-mode resolution failed, or the mode annotation is invalid.
    Failed,
}

/// How an L4 listener consumes a Service: per connection (TCP, TLS) or per
/// new datagram session (UDP), whose receive loop must not wait on DNS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum L4Transport {
    Connection,
    Datagram,
}

/// Longest a UDP datagram waits for a `system`-mode lookup; the lookup keeps
/// running in the background and fills the shared answer cache.
const DATAGRAM_DNS_BUDGET: std::time::Duration = std::time::Duration::from_millis(250);

impl L4ServicePrep {
    /// The hostname a per-connection (`system`-mode) address was resolved from.
    pub(crate) fn direct_hostname(&self) -> Option<&str> {
        match self {
            Self::Direct { hostname, .. } => Some(hostname),
            _ => None,
        }
    }

    /// True when the DNS step itself failed (as opposed to empty endpoints).
    pub(crate) fn is_failed(&self) -> bool {
        matches!(self, Self::Failed)
    }

    /// The backend for this connection: the resolved one, or ordinary L4
    /// selection (no LB policy, live degrade) for `target`.
    pub(crate) fn resolve(&self, target: &CompiledServiceTarget) -> Result<Arc<crate::lb::LbBackend>, EdgionErrorCode> {
        match self {
            Self::Select(Some((available, active))) => {
                selector::select_l4_endpoint_from(Arc::clone(available), Arc::clone(active), target.section())
                    .map(|endpoint| endpoint.backend)
            }
            Self::Select(None) => select_service_endpoint(
                target.key(),
                target.section().clone(),
                &None,
                ServiceDegradePolicy::Live,
                None,
                "",
                None,
            )
            .map(|endpoint| endpoint.backend),
            Self::Direct { backend, .. } => Ok(Arc::clone(backend)),
            Self::Failed => Err(EdgionErrorCode::BackendDnsResolutionFailed),
        }
    }
}

/// The hostname of an `ExternalName` projection whose effective mode is
/// `system` (annotation, else the EGC `endpointTable` default).
pub(crate) fn system_mode_hostname(available: &AvailableServiceProjection) -> Option<&str> {
    let spec = available.raw.service.spec.as_ref()?;
    if spec.type_.as_deref() != Some("ExternalName") {
        return None;
    }
    let endpoint_table =
        crate::config::get_egc_dns_resolver_default().is_none_or(|resolved| resolved.refresh_policy().endpoint_table);
    match external_name_source(available.dns_resolution_mode, endpoint_table) {
        Ok(ExternalNameSource::System) => spec.external_name.as_deref().filter(|name| !name.is_empty()),
        _ => None,
    }
}

/// Async per-connection (or per-new-UDP-session) step for TCP / TLS / UDP
/// Service backends.
///
/// Ordinary Services cost one projection load, reused for selection. A
/// `gateway`-mode `ExternalName` target that is still pending may be waited on
/// (bounded; never on the UDP datagram path). A `system`-mode one is resolved
/// through the OS resolver. Either DNS step runs only after the admission gate
/// the synchronous selection applies, so a refused backend never waits or
/// triggers DNS.
pub(crate) async fn prepare_l4_service(
    selected: SelectedL4Backend<'_>,
    gateway_key: Option<&str>,
    allow_loopback: bool,
    transport: L4Transport,
) -> L4ServicePrep {
    let Ok(target) = selected.service() else {
        return L4ServicePrep::Select(None);
    };
    let table = dns_endpoint_table();
    if transport == L4Transport::Connection && table.is_pending(target.key()) {
        if !policy::service_ancestor_admitted(target, gateway_key) {
            return L4ServicePrep::Select(None);
        }
        table.wait_first_answer(target.key()).await;
    }
    let Some((available, active)) = selector::load_active_projection(target.key()) else {
        return L4ServicePrep::Select(None);
    };
    if available
        .raw
        .service
        .spec
        .as_ref()
        .and_then(|spec| spec.type_.as_deref())
        == Some("ExternalName")
        && available.dns_resolution_mode.is_err()
    {
        return L4ServicePrep::Failed;
    }
    let Some(hostname) = system_mode_hostname(&available).map(Box::<str>::from) else {
        return L4ServicePrep::Select(Some((available, active)));
    };
    if !policy::service_ancestor_admitted(target, gateway_key) {
        // The terminal gate refuses this backend; no DNS work for it.
        return L4ServicePrep::Select(Some((available, active)));
    }
    let egc = crate::config::get_egc_dns_resolver_default();
    let Ok(family) = external_name_family(
        available.dns_lookup_family,
        egc.as_ref().map(|resolved| resolved.lookup_family()),
    ) else {
        return L4ServicePrep::Failed;
    };
    let Ok(section) = available.resolve_section(target.section()) else {
        // Selection reports the port failure.
        return L4ServicePrep::Select(Some((available, active)));
    };
    let port = section.identity.number.get();
    let query = DnsQueryPolicy::Family(family);
    let ips = match transport {
        L4Transport::Connection => match resolve_answer(&SYSTEM_RESOLVER, &hostname, query).await {
            Ok(answer) => answer.addrs,
            Err(_) => return L4ServicePrep::Failed,
        },
        L4Transport::Datagram => {
            // Hostname-keyed and single-flight: a flood of new (spoofable)
            // sources cannot multiply queries, and the receive loop waits at
            // most the budget while the lookup completes in the background.
            let cache_policy = egc
                .as_ref()
                .map(|resolved| resolved.cache_policy())
                .unwrap_or(DnsCachePolicy::new(DEFAULT_DNS_CACHE_TTL));
            let name = hostname.clone();
            let lookup = tokio::spawn(async move {
                upstream_dns_cache()
                    .lookup(&SYSTEM_RESOLVER, &name, query, &cache_policy)
                    .await
            });
            match tokio::time::timeout(DATAGRAM_DNS_BUDGET, lookup).await {
                Ok(Ok(Ok(addrs))) => addrs,
                _ => return L4ServicePrep::Failed,
            }
        }
    };
    let mut ips = ips;
    rotate_addresses(&mut ips, next_address_rotation());
    // An unsafe-only answer still yields an address so the caller's own
    // address-safety check rejects it with its usual status.
    let addr = match first_safe_inet_addr(&ips, port, allow_loopback) {
        Ok(addr) => addr,
        Err(_) => match ips.first() {
            Some(ip) => SocketAddr::Inet(std::net::SocketAddr::new(*ip, port)),
            None => return L4ServicePrep::Failed,
        },
    };
    L4ServicePrep::Direct {
        backend: Arc::new(crate::lb::LbBackend::new(pingora_load_balancing::Backend {
            addr,
            weight: 1,
            ext: Default::default(),
        })),
        hostname,
    }
}

/// Process-wide rotation for spreading attempts over resolved addresses. Shared
/// by AI endpoints, external jumps and `system`-mode `ExternalName` resolution.
static ADDRESS_ROTATION: AtomicUsize = AtomicUsize::new(0);

/// Next rotation offset for one upstream attempt.
pub(crate) fn next_address_rotation() -> usize {
    ADDRESS_ROTATION.fetch_add(1, Ordering::Relaxed)
}

/// Rotate `addrs` so that index `rotation % len` comes first.
pub(crate) fn rotate_addresses<T>(addrs: &mut [T], rotation: usize) {
    if !addrs.is_empty() {
        addrs.rotate_left(rotation % addrs.len());
    }
}

/// Resolve the request's Gateway-scoped DNS policy for a routed external
/// EdgionBackend. Unlike ExternalName there is no Service annotation layer.
/// Resolver, family and answer-cache policy come from one EGC snapshot load.
pub(crate) fn compute_routed_external_resolver(
    ctx: &EdgionHttpContext,
) -> Result<(ResolverSpec, DnsLookupFamily, DnsCachePolicy), DnsError> {
    let gateway_layer = ctx
        .gateway_info
        .as_deref()
        .and_then(|info| info.dns_resolver_candidate.as_ref());
    let egc_default = crate::config::get_egc_dns_resolver_default();
    let egc_layer = egc_default.as_ref().and_then(|resolved| resolved.candidate());
    let family = compute_lookup_family(None, ctx, egc_default.as_ref().map(|resolved| resolved.lookup_family()))?;
    let policy = egc_default
        .as_ref()
        .map(|resolved| resolved.cache_policy())
        .unwrap_or(DnsCachePolicy::new(DEFAULT_DNS_CACHE_TTL));
    let spec = resolve_effective_spec(&[gateway_layer, egc_layer], |key: &LinkSysKey| {
        httpdns_config_for_ref(key)
    });
    Ok((spec, family, policy))
}

/// Family inheritance is independent of resolver selection and captured before DNS I/O.
fn compute_lookup_family(
    service: Option<Result<DnsLookupFamily, &'static str>>,
    ctx: &EdgionHttpContext,
    egc: Option<DnsLookupFamily>,
) -> Result<DnsLookupFamily, DnsError> {
    service
        .or_else(|| ctx.gateway_info.as_ref().and_then(|info| info.dns_lookup_family))
        .transpose()
        .map(|family| family.or(egc).unwrap_or_default())
        .map_err(|error| DnsError::Parse(error.into()))
}

/// Look up a prepared HTTP DNS generation by `namespace/name` from the
/// gateway's typed LinkSys runtime table. Returns `None` when the ref is missing or the target
/// LinkSys is not an httpdns resolver.
fn httpdns_config_for_ref(key: &LinkSysKey) -> Option<Arc<edgion_common::dns::PreparedHttpDns>> {
    let store_key = format!("{}/{}", key.namespace, key.name);
    crate::link_sys::runtime::store::get_httpdns_client(&store_key)
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
    F: FnOnce(&EdgionHttpContext) -> Result<ResolvedServiceEndpoint, EdgionErrorCode>,
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
    select_fn(ctx)
}

/// Shared logic for ordinary Services and `gateway`/`controller`-mode
/// ExternalName Services (both select from published endpoints).
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

    if matches!(ctx.test_metrics_type(), Some(crate::observe::metrics::TestType::Lb))
        && matches!(&lb_policy, Some(ParsedLBPolicy::ConsistentHash(_)))
    {
        let remote_ip = ctx.request_info.remote_ip.as_str();
        let query = Some(&ctx.request_query);
        let hash_key_bytes = extract_hash_key(session, &lb_policy, remote_ip, query);
        let hash_key = std::str::from_utf8(hash_key_bytes).ok().map(str::to_owned);
        ctx.record_test_hash_key(hash_key);
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
        upstream.err.push(reason.into());
    } else {
        ctx.push_upstream(ip, port);
        if let Some(upstream) = ctx.get_current_upstream_mut() {
            upstream.err.push(reason.into());
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

pub(crate) use selector::select_service_endpoint;
