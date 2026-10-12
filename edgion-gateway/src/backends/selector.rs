use crate::backends::discovery::{
    get_service_discovery_coordinator, AvailableServiceProjection, ResolvedServiceSection, ServiceDiscoveryCoordinator,
    ServiceRuntimeSnapshot, ServiceSectionKey,
};
use crate::backends::CompiledServiceTarget;
use crate::lb::{compiled_selector, LbBackend, PolicyKind};
use edgion_resources::resources::lb_policy::{ConsistentHashOn, ParsedLBPolicy};
use edgion_resources::uri_utils;
use edgion_resources::EdgionErrorCode;
use k8s_openapi::api::core::v1::Service;
use pingora_proxy::Session;
use std::net::SocketAddr;
use std::num::NonZeroU16;
use std::sync::Arc;

pub struct ResolvedServiceEndpoint {
    pub backend: Arc<LbBackend>,
    pub section: Arc<ResolvedServiceSection>,
    pub serving_port: NonZeroU16,
    pub ready: bool,
    pub slot_proof: [u8; 32],
}

/// Configuration boundary for one Service selection. HTTP/gRPC preserve their
/// captured Service override, including absence; L4 and mirrors use live policy.
#[derive(Clone, Copy)]
pub enum ServiceDegradePolicy {
    /// `None` uses the independently updated global default, never a live
    /// Service override. `Some(0)` explicitly disables degrade mode.
    CapturedOverride(Option<u8>),
    Live,
}

impl ServiceDegradePolicy {
    fn threshold(self, active: &ServiceRuntimeSnapshot) -> u8 {
        match self {
            Self::CapturedOverride(value) => value.unwrap_or_else(super::degrade_threshold::get_lb_degrade_default),
            Self::Live => active.degrade_threshold,
        }
    }
}

pub fn service_section_key_from_port(port: Option<i32>) -> Result<ServiceSectionKey, EdgionErrorCode> {
    port.and_then(|value| u16::try_from(value).ok())
        .and_then(NonZeroU16::new)
        .map(ServiceSectionKey::Number)
        .ok_or(EdgionErrorCode::BackendPortResolutionFailed)
}

fn load_active_projection(service_key: &str) -> Option<(Arc<AvailableServiceProjection>, Arc<ServiceRuntimeSnapshot>)> {
    let coordinator = get_service_discovery_coordinator();
    let generation = coordinator.load_generation();
    let available = Arc::clone(generation.available(service_key)?);
    let active = available.runtime_cell.active.load_full();
    (active.membership_identity == Some(available.membership_identity)).then_some((available, active))
}

#[inline]
fn runtime_eligible(active: &ServiceRuntimeSnapshot, backend: &LbBackend, record_rejection: bool) -> bool {
    let Some(identity) = backend.runtime_identity() else {
        return false;
    };
    let Some(state) = active.entries.get(identity.as_ref()) else {
        return false;
    };
    if !state.health_eligible() {
        return false;
    }
    if state.outlier_ejected() {
        if record_rejection {
            crate::observe::metrics::record_cb_rejection(identity.service_key.as_ref());
        }
        return false;
    }
    true
}

/// Decide whether degrade mode should engage for this selection. Gated so the
/// O(n) survivor count only runs for services that actually have health-check
/// or outlier-detection state (otherwise every backend passes and degrade can
/// never trigger). The caller records an event only when selection takes the
/// degrade branch, using the handle captured in this same runtime snapshot.
fn degrade_engaged(
    selector: &compiled_selector::CompiledSelector,
    active: &ServiceRuntimeSnapshot,
    threshold: u8,
) -> bool {
    if !active.health_policy_present && !active.outlier_policy_present {
        return false;
    }
    if threshold == 0 {
        return false;
    }
    let total = selector.backend_count();
    if total == 0 {
        return false;
    }
    let floor = crate::backends::degrade_threshold::degrade_floor(threshold, total);
    selector.degrade_should_engage(floor, |b| runtime_eligible(active, b, false))
}

/// Map a parsed LB policy to its eager-selector discriminator.
#[inline]
fn policy_to_kind(lb_policy: &Option<ParsedLBPolicy>) -> PolicyKind {
    match lb_policy {
        None => PolicyKind::RoundRobin,
        Some(ParsedLBPolicy::ConsistentHash(_)) => PolicyKind::ConsistentHash,
        Some(ParsedLBPolicy::LeastConn) => PolicyKind::LeastConn,
        Some(ParsedLBPolicy::Ewma) => PolicyKind::Ewma,
    }
}

/// Extract hash key from request based on LB policy configuration.
///
/// `remote_ip` is the RealIp-normalized client source IP (`ctx.request_info.remote_ip`),
/// used only by the `SourceIp` mode; the other modes read from the request headers/URI.
pub(crate) fn extract_hash_key<'a>(
    session: &'a Session,
    lb_policy: &Option<ParsedLBPolicy>,
    remote_ip: &'a str,
    query: Option<&'a str>,
) -> &'a [u8] {
    let Some(ParsedLBPolicy::ConsistentHash(hash_on)) = lb_policy else {
        return &[];
    };

    let req_header = session.req_header();

    match hash_on {
        ConsistentHashOn::Header(header_name) => req_header
            .headers
            .get(header_name.as_str())
            .and_then(|v| v.to_str().ok())
            .map(str::as_bytes)
            .unwrap_or_default(),
        ConsistentHashOn::Cookie(cookie_name) => req_header
            .headers
            .get("cookie")
            .and_then(|v| v.to_str().ok())
            .and_then(|cookies| {
                cookies.split(';').map(|s| s.trim()).find_map(|cookie| {
                    let (name, value) = cookie.split_once('=')?;
                    if name == cookie_name {
                        Some(value.as_bytes())
                    } else {
                        None
                    }
                })
            })
            .unwrap_or_default(),
        ConsistentHashOn::Arg(arg_name) => query
            .and_then(|query| {
                query.split('&').find_map(|param| {
                    let (name, value) = param.split_once('=')?;
                    // Fast path: most query param names are plain identifiers
                    // that match arg_name byte-for-byte, so the common case stays
                    // allocation-free. Only percent/plus-encoded names need
                    // decoding before comparison (e.g. `session%20id` / `foo+bar`
                    // matching an arg_name of `session id` / `foo bar`). The value
                    // is intentionally left raw — decoding it would change the hash
                    // key of existing deployments.
                    let matched =
                        name == arg_name || (name.contains(['%', '+']) && uri_utils::url_decode(name) == *arg_name);
                    matched.then_some(value.as_bytes())
                })
            })
            .unwrap_or_default(),
        // SourceIp hashes on the normalized client IP; an empty IP yields an
        // empty key, driving the internal RR fallback like the other modes when
        // their value is missing.
        ConsistentHashOn::SourceIp => remote_ip.as_bytes(),
    }
}

/// Errors raised when the cell yields no backend, broken out per LB policy
/// so the caller maps to the historically-correct `EdgionErrorCode`.
struct NotFoundMap {
    /// Empty backend list (no policy applies).
    empty: EdgionErrorCode,
    /// Round-robin selection found nothing healthy.
    rr: EdgionErrorCode,
    /// Consistent hash selected nothing (non-empty key path).
    ch: EdgionErrorCode,
    /// LeastConn found nothing.
    lc: EdgionErrorCode,
    /// EWMA found nothing.
    ewma: EdgionErrorCode,
}

impl NotFoundMap {
    fn for_kind(&self, kind: PolicyKind, ch_used_hash_key: bool) -> EdgionErrorCode {
        match kind {
            PolicyKind::RoundRobin => self.rr,
            PolicyKind::ConsistentHash if ch_used_hash_key => self.ch,
            // Empty hash key falls back to RR-style selection internally, so
            // surface the RR not-found code to match historical behaviour.
            PolicyKind::ConsistentHash => self.rr,
            PolicyKind::LeastConn => self.lc,
            PolicyKind::Ewma => self.ewma,
        }
    }
}

fn endpoint_slice_not_found() -> NotFoundMap {
    NotFoundMap {
        empty: EdgionErrorCode::BackendEndpointSliceNotFoundByRoundRobinDefault,
        rr: EdgionErrorCode::BackendEndpointSliceNotFoundByRoundRobin,
        ch: EdgionErrorCode::BackendEndpointSliceNotFoundByConsistent,
        lc: EdgionErrorCode::BackendEndpointSliceNotFoundByLeastConn,
        ewma: EdgionErrorCode::BackendEndpointSliceNotFoundByEwma,
    }
}

fn slot_for_backend<'a>(
    section: &'a ResolvedServiceSection,
    backend: &Arc<LbBackend>,
) -> Option<&'a crate::backends::discovery::ResolvedBackendSlot> {
    section
        .all_slots
        .iter()
        .find(|slot| Arc::ptr_eq(&slot.backend, backend))
        .map(Arc::as_ref)
}

/// One coherent discovery projection for a peer attempt. Runtime endpoint state
/// remains live through its existing atomic handles; later attempts recapture.
pub(crate) struct CapturedServiceSelection {
    available: Arc<AvailableServiceProjection>,
    section: Arc<ResolvedServiceSection>,
    active: Arc<ServiceRuntimeSnapshot>,
}

impl CapturedServiceSelection {
    pub(crate) fn response_cache_stamp(
        &self,
        target: &CompiledServiceTarget,
    ) -> Option<crate::response_cache::DiscoveryStamp> {
        crate::response_cache::discovery_stamp(target, &self.available, &self.section)
    }

    pub(crate) fn capture(
        coordinator: &ServiceDiscoveryCoordinator,
        target: &CompiledServiceTarget,
    ) -> Result<Self, EdgionErrorCode> {
        let generation = coordinator.load_generation();
        let available = Arc::clone(
            generation
                .available(target.key())
                .ok_or(EdgionErrorCode::BackendServiceNotFound)?,
        );
        let active = available.runtime_cell.active.load_full();
        if active.membership_identity != Some(available.membership_identity) {
            return Err(endpoint_slice_not_found().empty);
        }
        Self::from_projection(available, active, target.section())
    }

    fn from_projection(
        available: Arc<AvailableServiceProjection>,
        active: Arc<ServiceRuntimeSnapshot>,
        section: &ServiceSectionKey,
    ) -> Result<Self, EdgionErrorCode> {
        let section = available
            .resolve_section(section)
            .map_err(|_| EdgionErrorCode::BackendPortResolutionFailed)?;
        Ok(Self {
            available,
            section,
            active,
        })
    }

    pub(crate) fn dns_resolver_candidate(&self) -> Option<&edgion_common::dns::resolver_select::ResolverCandidate> {
        self.available.dns_resolver_candidate.as_ref()
    }

    pub(crate) fn source(&self) -> &Service {
        &self.available.raw.service
    }

    pub(crate) fn section(&self) -> &ResolvedServiceSection {
        &self.section
    }

    pub(crate) fn select(
        &self,
        lb_policy: &Option<ParsedLBPolicy>,
        degrade_policy: ServiceDegradePolicy,
        session: Option<&Session>,
        remote_ip: &str,
        query: Option<&str>,
    ) -> Result<ResolvedServiceEndpoint, EdgionErrorCode> {
        let kind = policy_to_kind(lb_policy);
        let section = &self.section;
        let active = &self.active;
        let not_found = endpoint_slice_not_found();
        let selector = section.selectors.get(kind);
        let (hash_key, hash_key_used) = match (kind, session) {
            (PolicyKind::ConsistentHash, Some(session)) => {
                let key = extract_hash_key(session, lb_policy, remote_ip, query);
                let used = !key.is_empty();
                (Some(key), used)
            }
            (PolicyKind::ConsistentHash, None) => (None, false),
            _ => (None, false),
        };

        let now = crate::lb::ewma::score::now_units();
        let result = if degrade_engaged(selector, active, degrade_policy.threshold(active)) {
            if let Some(counter) = &active.degrade_selections {
                counter.record_selection();
            }
            selector.select(hash_key, |_| true, now)
        } else {
            selector.select(hash_key, |b| runtime_eligible(active, b, true), now)
        };
        let backend = result.ok_or(not_found.for_kind(kind, hash_key_used))?;
        let (ready, slot_proof) = slot_for_backend(section, &backend)
            .map(|slot| (slot.ready, slot.proof))
            .ok_or(EdgionErrorCode::BackendPortResolutionFailed)?;
        let serving_port = backend
            .addr
            .as_inet()
            .and_then(|addr| NonZeroU16::new(addr.port()))
            .ok_or(EdgionErrorCode::BackendPortResolutionFailed)?;
        Ok(ResolvedServiceEndpoint {
            backend,
            section: Arc::clone(section),
            serving_port,
            ready,
            slot_proof,
        })
    }

    pub(crate) fn endpoint_for_addr(&self, target: SocketAddr) -> Option<(ResolvedServiceEndpoint, bool)> {
        let section = &self.section;
        let slot = section
            .all_slots
            .iter()
            .find(|slot| slot.backend.addr.as_inet().is_some_and(|addr| *addr == target))?;
        let backend = Arc::clone(&slot.backend);
        let ready = slot.ready;
        let slot_proof = slot.proof;
        let eligible = slot.ready && runtime_eligible(&self.active, &backend, true);
        let serving_port = NonZeroU16::new(target.port())?;
        Some((
            ResolvedServiceEndpoint {
                backend,
                section: Arc::clone(section),
                serving_port,
                ready,
                slot_proof,
            },
            eligible,
        ))
    }
}

pub fn select_service_endpoint(
    service_key: &str,
    section: ServiceSectionKey,
    lb_policy: &Option<ParsedLBPolicy>,
    degrade_policy: ServiceDegradePolicy,
    session: Option<&Session>,
    remote_ip: &str,
    query: Option<&str>,
) -> Result<ResolvedServiceEndpoint, EdgionErrorCode> {
    let (available, active) = load_active_projection(service_key).ok_or(endpoint_slice_not_found().empty)?;
    CapturedServiceSelection::from_projection(available, active, &section)?.select(
        lb_policy,
        degrade_policy,
        session,
        remote_ip,
        query,
    )
}

/// Current backends for one proven Service section. Used by health-adjacent
/// session-affinity construction for that section only.
pub fn backends_for_service_section(service_key: &str, section: &ServiceSectionKey) -> Arc<[Arc<LbBackend>]> {
    let Some((available, _)) = load_active_projection(service_key) else {
        return Arc::from(Vec::new());
    };
    match available.resolve_section(section) {
        Ok(section) => section
            .all_slots
            .iter()
            .map(|slot| Arc::clone(&slot.backend))
            .collect::<Vec<_>>()
            .into(),
        Err(_) => Arc::from(Vec::new()),
    }
}

#[cfg(test)]
#[path = "../../tests/unit/backends/selector_test.rs"]
mod tests;
