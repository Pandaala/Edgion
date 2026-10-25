use arc_swap::{ArcSwap, ArcSwapOption};
use dashmap::DashMap;
use edgion_resources::constants::annotations::edgion::{DNS_RESOLUTION_MODE, DNS_RESOLVER, DNS_RESOLVER_REF};
use edgion_resources::constants::labels::k8s::SERVICE_NAME;
use k8s_openapi::api::core::v1::{Service, ServicePort};
use k8s_openapi::api::discovery::v1::{EndpointPort, EndpointSlice};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use parking_lot::Mutex;
use pingora_core::protocols::l4::socket::SocketAddr as PingoraSocketAddr;
use pingora_load_balancing::Backend;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroU16;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use crate::backends::peer_builder::{parse_backend_app_protocol, BackendAppProtocol};
use crate::lb::compiled_selector::CompiledSelectors;
use crate::lb::{BackendRuntimeCounters, EndpointRuntimeIdentity, EndpointRuntimeScope, LbBackend};

const PROOF_DOMAIN: &[u8] = b"edgion/service-selector-proof/v1\0";
const MEMBERSHIP_DOMAIN: &[u8] = b"edgion/service-membership/v1\0";
const STATS_KEY_LIMIT: usize = 32;
static RUNTIME_CLOCK_ORIGIN: LazyLock<Instant> = LazyLock::new(Instant::now);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LiveSourceIdentity {
    KubernetesUid { uid: Arc<str>, live_epoch: u64 },
    AuthoritativeEpoch(u64),
}

#[derive(Clone)]
struct LiveSourceRecord<T> {
    resource: Arc<T>,
    identity: Option<LiveSourceIdentity>,
    semantic_digest: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceUnavailableReason {
    MissingService,
    InvalidEpoch,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ServiceSectionKey {
    Number(NonZeroU16),
    Name(Arc<str>),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServicePortIdentity {
    pub name: Option<Arc<str>>,
    pub number: NonZeroU16,
    pub protocol: Arc<str>,
    pub app_protocol: Option<Arc<str>>,
    pub target_port: NormalizedTargetPort,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NormalizedTargetPort {
    Number(NonZeroU16),
    Name(Arc<str>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceSectionInvalid {
    InvalidServicePort,
    MissingRequestedPort,
    AmbiguousServicePort,
    MissingEndpointPort,
    AmbiguousEndpointPort,
    InvalidEndpointAddress,
    AddressFamilyConflict,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EndpointOrigin {
    pub slice_key: Arc<str>,
    pub source_identity: LiveSourceIdentity,
    pub address: IpAddr,
    pub serving_port: NonZeroU16,
    pub ready: bool,
}

// Pure discovery geometry has no executable selectors or runtime identity.
struct NormalizedBackendSlot {
    socket: SocketAddr,
    ready: bool,
    origin_set: Arc<[EndpointOrigin]>,
    proof: [u8; 32],
}

struct NormalizedServiceSection {
    identity: ServicePortIdentity,
    slots: Vec<NormalizedBackendSlot>,
}

enum NormalizedSectionOutcome {
    Resolved(Arc<NormalizedServiceSection>),
    Invalid(ServiceSectionInvalid),
}

pub struct ResolvedBackendSlot {
    pub backend: Arc<LbBackend>,
    pub ready: bool,
    pub origin_set: Arc<[EndpointOrigin]>,
    pub proof: [u8; 32],
}

pub struct ResolvedServiceSection {
    pub identity: ServicePortIdentity,
    pub(crate) prepared_app_protocol: Option<BackendAppProtocol>,
    pub all_slots: Arc<[Arc<ResolvedBackendSlot>]>,
    pub ready_slots: Arc<[Arc<ResolvedBackendSlot>]>,
    pub selectors: CompiledSelectors,
    direct_endpoint_ports: BTreeMap<IpAddr, BTreeSet<NonZeroU16>>,
}

impl ResolvedServiceSection {
    /// Authorize a direct endpoint within this Service port section.
    ///
    /// Direct overrides intentionally include published unready slots. Without
    /// an explicit port, the IP must have exactly one serving port in this section.
    pub fn direct_endpoint_addr(&self, ip: IpAddr, explicit_port: Option<u16>) -> Option<SocketAddr> {
        let ports = self.direct_endpoint_ports.get(&ip)?;
        let port = match explicit_port {
            Some(port) => {
                let port = NonZeroU16::new(port)?;
                ports.get(&port).copied()?
            }
            None if ports.len() == 1 => *ports.first()?,
            None => return None,
        };
        Some(SocketAddr::new(ip, port.get()))
    }
}

fn compile_direct_endpoint_ports(slots: &[Arc<ResolvedBackendSlot>]) -> BTreeMap<IpAddr, BTreeSet<NonZeroU16>> {
    let mut ports_by_ip: BTreeMap<IpAddr, BTreeSet<NonZeroU16>> = BTreeMap::new();
    for slot in slots {
        if let Some(socket) = slot.backend.addr.as_inet() {
            if let Some(port) = NonZeroU16::new(socket.port()) {
                ports_by_ip.entry(socket.ip()).or_default().insert(port);
            }
        }
    }
    ports_by_ip
}

pub enum ServiceSectionOutcome {
    Resolved(Arc<ResolvedServiceSection>),
    Invalid(ServiceSectionInvalid),
}

pub struct LegacyRawProjection {
    pub service: Arc<Service>,
    pub endpoint_slices: Arc<[Arc<EndpointSlice>]>,
}

pub struct AvailableServiceProjection {
    pub(crate) dns_resolver_candidate: Option<edgion_common::dns::resolver_select::ResolverCandidate>,
    pub(crate) dns_resolution_mode: super::DnsResolutionMode,
    content_fingerprint: [u8; 32],
    pub membership_identity: u64,
    pub runtime_cell: Arc<ServiceRuntimeCell>,
    pub raw: Arc<LegacyRawProjection>,
    pub sections: Arc<BTreeMap<ServiceSectionKey, Arc<ServiceSectionOutcome>>>,
}

impl AvailableServiceProjection {
    /// Full canonical live-source/geometry proof, borrowed from this projection.
    pub(crate) fn content_fingerprint(&self) -> &[u8; 32] {
        &self.content_fingerprint
    }

    pub fn resolve_section(
        &self,
        key: &ServiceSectionKey,
    ) -> Result<Arc<ResolvedServiceSection>, ServiceSectionInvalid> {
        match self.sections.get(key).map(Arc::as_ref) {
            Some(ServiceSectionOutcome::Resolved(section)) => Ok(Arc::clone(section)),
            Some(ServiceSectionOutcome::Invalid(reason)) => Err(reason.clone()),
            None => Err(ServiceSectionInvalid::MissingRequestedPort),
        }
    }

    /// Union of every Resolved section's `all_slots` wrappers.
    ///
    /// Number/name aliases of one section share the same identity and are
    /// returned once. Different sections that share a socket stay separate.
    /// Invalid sections contribute nothing.
    pub fn canonical_probe_backends(&self) -> Arc<[Arc<LbBackend>]> {
        let mut seen = HashSet::new();
        let mut backends = Vec::new();
        for section in unique_resolved_sections(&self.sections) {
            for slot in section.all_slots.iter() {
                let Some(identity) = slot.backend.runtime_identity() else {
                    continue;
                };
                if seen.insert(Arc::as_ptr(identity)) {
                    backends.push(Arc::clone(&slot.backend));
                }
            }
        }
        backends.into()
    }
}

pub enum ServicePublishedState {
    Available(Arc<AvailableServiceProjection>),
    Unavailable(ServiceUnavailableReason),
}

#[derive(Clone, Default, PartialEq, Eq, Serialize)]
pub struct ServiceSelectorGenerationStats {
    pub service_sources: usize,
    pub endpoint_slice_sources: usize,
    pub identified_service_sources: usize,
    pub identified_endpoint_slice_sources: usize,
    pub unidentified_service_sources: usize,
    pub unidentified_endpoint_slice_sources: usize,
    pub unavailable_services: usize,
    pub available_services: usize,
    pub valid_sections: usize,
    pub invalid_sections: usize,
    pub canonical_slots: usize,
}

/// One bounded debug snapshot collected from one published discovery
/// generation. Keys identify Services only; endpoint addresses and source
/// identities are deliberately excluded.
#[derive(Clone, Default, Serialize)]
pub struct ServiceDiscoveryStatsSnapshot {
    #[serde(flatten)]
    pub generation: ServiceSelectorGenerationStats,
    pub active_runtime_services: usize,
    pub active_runtime_endpoints: usize,
    pub sticky_runtime_services: usize,
    pub health_policy_services: usize,
    pub outlier_policy_services: usize,
    pub degrade_policy_services: usize,
    pub degrade_metric_handles: usize,
    pub degrade_metric_service_keys: Vec<String>,
    pub degrade_metric_service_keys_truncated: bool,
    pub draining_backends: usize,
    pub service_keys: Vec<String>,
    pub service_keys_truncated: bool,
    pub draining_service_keys: Vec<String>,
    pub draining_service_keys_truncated: bool,
}

pub struct ServiceSelectorGeneration {
    pub services: HashMap<String, ServicePublishedState>,
    pub stats: ServiceSelectorGenerationStats,
}

impl ServiceSelectorGeneration {
    fn empty() -> Self {
        Self {
            services: HashMap::new(),
            stats: ServiceSelectorGenerationStats::default(),
        }
    }

    pub fn service(&self, key: &str) -> Option<&ServicePublishedState> {
        self.services.get(key)
    }

    pub fn available(&self, key: &str) -> Option<&Arc<AvailableServiceProjection>> {
        match self.services.get(key) {
            Some(ServicePublishedState::Available(service)) => Some(service),
            Some(ServicePublishedState::Unavailable(_)) | None => None,
        }
    }
}

pub struct ServiceRuntimeSnapshot {
    pub membership_identity: Option<u64>,
    pub entries: Arc<HashMap<EndpointRuntimeIdentity, Arc<EndpointRuntimeState>>>,
    pub health_policy_present: bool,
    pub outlier_policy_present: bool,
    pub degrade_threshold: u8,
    pub(crate) degrade_selections: Option<Arc<crate::observe::metrics::registry::DegradeSelectionCounter>>,
}

impl ServiceRuntimeSnapshot {
    fn inactive() -> Self {
        Self {
            membership_identity: None,
            entries: Arc::new(HashMap::new()),
            health_policy_present: false,
            outlier_policy_present: false,
            degrade_threshold: 0,
            degrade_selections: None,
        }
    }

    /// Metric ownership follows the Service snapshots, not individual requests
    /// or sections. Keep a registered handle across policy/membership changes;
    /// historical selection events are never reset when capability is removed.
    fn with_degrade_counter(mut self, service_key: &str, previous: &Self) -> Self {
        self.degrade_selections = previous.degrade_selections.clone().or_else(|| {
            // An in-flight request may retain a nonzero override even when the
            // live threshold is zero. Prepare its counter on publication too.
            (self.health_policy_present || self.outlier_policy_present)
                .then(|| crate::observe::metrics::GatewayMetrics::lb_degrade_selections(service_key))
        });
        self
    }
}

pub struct EndpointRuntimeState {
    pub identity: Arc<EndpointRuntimeIdentity>,
    pub backend: Arc<LbBackend>,
    completion_lock: Mutex<EndpointCompletionState>,
    attached: AtomicBool,
    health_eligible: AtomicBool,
    frozen_health_eligible: AtomicBool,
    outlier_ejected: AtomicBool,
    frozen_outlier_ejected: AtomicBool,
    outlier_until_millis: std::sync::atomic::AtomicU64,
    gauge_accounted: AtomicBool,
    ejected_metric: edgion_common::observe::metrics_facade::GaugeContribution,
    passive_outlier: ArcSwapOption<crate::backends::outlier_detection::PassiveEjectionState>,
}

struct EndpointCompletionState {
    consecutive_health_successes: u32,
    consecutive_health_failures: u32,
    health_policy: Option<Arc<HealthPolicyIdentity>>,
    outlier_policy: Option<Arc<OutlierPolicyIdentity>>,
}

#[derive(Debug)]
pub(crate) struct OutlierPolicyIdentity {
    pub generation: u64,
}

#[derive(Debug)]
pub(crate) struct HealthPolicyIdentity {
    pub generation: u64,
}

impl EndpointRuntimeState {
    fn fresh(
        identity: Arc<EndpointRuntimeIdentity>,
        backend: Arc<LbBackend>,
        health_policy: Option<Arc<HealthPolicyIdentity>>,
        outlier_policy: Option<Arc<OutlierPolicyIdentity>>,
    ) -> Self {
        Self {
            ejected_metric: crate::observe::metrics::registry::cb_ejected_endpoint_contribution(
                identity.service_key.as_ref(),
            ),
            identity,
            backend,
            completion_lock: Mutex::new(EndpointCompletionState {
                consecutive_health_successes: 0,
                consecutive_health_failures: 0,
                health_policy,
                outlier_policy,
            }),
            attached: AtomicBool::new(true),
            health_eligible: AtomicBool::new(true),
            frozen_health_eligible: AtomicBool::new(true),
            outlier_ejected: AtomicBool::new(false),
            frozen_outlier_ejected: AtomicBool::new(false),
            outlier_until_millis: std::sync::atomic::AtomicU64::new(0),
            gauge_accounted: AtomicBool::new(false),
            passive_outlier: ArcSwapOption::const_empty(),
        }
    }

    #[inline]
    pub fn health_eligible(&self) -> bool {
        if self.attached.load(Ordering::Acquire) {
            self.health_eligible.load(Ordering::Acquire)
        } else {
            self.frozen_health_eligible.load(Ordering::Acquire)
        }
    }

    #[inline]
    pub fn outlier_ejected(&self) -> bool {
        if !self.attached.load(Ordering::Acquire) {
            return self.frozen_outlier_ejected.load(Ordering::Acquire);
        }
        let until = self.outlier_until_millis.load(Ordering::Acquire);
        if !self.outlier_ejected.load(Ordering::Acquire) || until == 0 || runtime_now_millis() < until {
            return self.outlier_ejected.load(Ordering::Acquire);
        }

        // Expiry is a cold edge of the selection path. Serialize it with
        // completion writes so the passive state, runtime gate, gauge, and
        // percent-arbitration view cross the cooldown boundary together.
        let _completion = self.completion_lock.lock();
        let until = self.outlier_until_millis.load(Ordering::Acquire);
        if !self.attached.load(Ordering::Acquire)
            || !self.outlier_ejected.load(Ordering::Acquire)
            || until == 0
            || runtime_now_millis() < until
        {
            return self.outlier_ejected.load(Ordering::Acquire);
        }
        if let Some(passive) = self.passive_outlier.load().as_ref() {
            passive.expire_legacy_ejection();
        }
        self.clear_outlier_ejected();
        false
    }

    /// Expire both deadline representations without performing recovery itself.
    #[cfg(test)]
    pub(crate) fn expire_outlier_deadlines_for_test(&self) {
        if let Some(passive) = self.passive_outlier.load_full() {
            passive.expire_deadlines_for_test();
        }
        // Zero means no deadline. Wait only for the clock origin's first
        // millisecond so 1 is an already-elapsed, non-sentinel deadline.
        while runtime_now_millis() < 1 {
            std::thread::yield_now();
        }
        self.outlier_until_millis.store(1, Ordering::Release);
    }

    pub(crate) fn set_health_eligible(&self, eligible: bool) {
        self.health_eligible.store(eligible, Ordering::Release);
    }

    pub(crate) fn set_outlier_ejected_for(
        &self,
        duration: Duration,
        passive: Arc<crate::backends::outlier_detection::PassiveEjectionState>,
    ) {
        let duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX).max(1);
        let until = runtime_now_millis().saturating_add(duration_ms);
        self.passive_outlier.store(Some(passive));
        self.outlier_until_millis.store(until, Ordering::Release);
        self.outlier_ejected.store(true, Ordering::Release);
    }

    pub(crate) fn clear_outlier_ejected(&self) -> bool {
        self.outlier_until_millis.store(0, Ordering::Release);
        self.passive_outlier.store(None);
        let was_ejected = self.outlier_ejected.swap(false, Ordering::AcqRel);
        if was_ejected && self.gauge_accounted.swap(false, Ordering::AcqRel) {
            self.ejected_metric.set(0.);
        }
        was_ejected
    }

    pub(crate) fn set_gauge_accounted(&self, accounted: bool) {
        self.gauge_accounted.store(accounted, Ordering::Release);
        self.ejected_metric.set(if accounted { 1. } else { 0. });
    }

    fn settle_detached_gauge(&self) -> bool {
        let was_accounted = self.gauge_accounted.swap(false, Ordering::AcqRel);
        if was_accounted {
            self.ejected_metric.clear();
        }
        was_accounted
    }

    fn freeze_detached(&self) {
        let health = self.health_eligible.load(Ordering::Acquire);
        let until = self.outlier_until_millis.load(Ordering::Acquire);
        let outlier = self.outlier_ejected.load(Ordering::Acquire) && (until == 0 || runtime_now_millis() < until);
        self.frozen_health_eligible.store(health, Ordering::Release);
        self.frozen_outlier_ejected.store(outlier, Ordering::Release);
        self.attached.store(false, Ordering::Release);
    }
}

fn runtime_now_millis() -> u64 {
    let elapsed = Instant::now().saturating_duration_since(*RUNTIME_CLOCK_ORIGIN);
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

pub struct ServiceRuntimeCell {
    pub active: ArcSwap<ServiceRuntimeSnapshot>,
    pub sp: ArcSwap<BTreeMap<ServiceSectionKey, Arc<crate::lb::runtime_state::SpAffinitySnapshot>>>,
    pub sp_build_lock: Mutex<()>,
}

impl ServiceRuntimeCell {
    fn new() -> Self {
        Self {
            active: ArcSwap::from_pointee(ServiceRuntimeSnapshot::inactive()),
            sp: ArcSwap::from_pointee(BTreeMap::new()),
            sp_build_lock: Mutex::new(()),
        }
    }
}

#[derive(Clone)]
struct CoordinatorState {
    services: HashMap<String, LiveSourceRecord<Service>>,
    endpoint_slices: HashMap<String, LiveSourceRecord<EndpointSlice>>,
    next_epoch: u64,
    exhausted: bool,
}

impl Default for CoordinatorState {
    fn default() -> Self {
        Self {
            services: HashMap::new(),
            endpoint_slices: HashMap::new(),
            next_epoch: 1,
            exhausted: false,
        }
    }
}

pub struct ServiceDiscoveryCoordinator {
    state: Mutex<CoordinatorState>,
    published: ArcSwap<ServiceSelectorGeneration>,
    runtime_cells: DashMap<String, Arc<ServiceRuntimeCell>>,
    health_policies: DashMap<String, Arc<HealthPolicyIdentity>>,
    outlier_policies: DashMap<String, Arc<OutlierPolicyIdentity>>,
    #[cfg(test)]
    completion_barrier: Mutex<Option<(Arc<std::sync::Barrier>, Arc<std::sync::Barrier>)>>,
}

static GLOBAL_COORDINATOR: LazyLock<Arc<ServiceDiscoveryCoordinator>> =
    LazyLock::new(|| Arc::new(ServiceDiscoveryCoordinator::new()));

pub fn get_service_discovery_coordinator() -> &'static Arc<ServiceDiscoveryCoordinator> {
    &GLOBAL_COORDINATOR
}

impl ServiceDiscoveryCoordinator {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(CoordinatorState::default()),
            published: ArcSwap::from_pointee(ServiceSelectorGeneration::empty()),
            runtime_cells: DashMap::new(),
            health_policies: DashMap::new(),
            outlier_policies: DashMap::new(),
            #[cfg(test)]
            completion_barrier: Mutex::new(None),
        }
    }

    #[inline]
    pub fn load_generation(&self) -> Arc<ServiceSelectorGeneration> {
        self.published.load_full()
    }

    pub fn runtime_cell(&self, service_key: &str) -> Option<Arc<ServiceRuntimeCell>> {
        self.runtime_cells
            .get(service_key)
            .map(|entry| Arc::clone(entry.value()))
    }

    /// Collect one bounded administrative view from one loaded generation.
    /// Runtime membership is counted only when it belongs to that generation.
    pub fn stats(&self) -> ServiceDiscoveryStatsSnapshot {
        let generation = self.load_generation();
        let (degrade_metric_handles, degrade_metric_service_keys) =
            crate::observe::metrics::GatewayMetrics::lb_degrade_metric_stats(STATS_KEY_LIMIT);
        let mut stats = ServiceDiscoveryStatsSnapshot {
            generation: generation.stats.clone(),
            degrade_metric_handles,
            degrade_metric_service_keys_truncated: degrade_metric_handles > degrade_metric_service_keys.len(),
            degrade_metric_service_keys,
            ..Default::default()
        };
        let mut service_keys: Vec<_> = generation.services.keys().collect();
        service_keys.sort_unstable();
        stats.service_keys_truncated = service_keys.len() > STATS_KEY_LIMIT;
        service_keys.truncate(STATS_KEY_LIMIT);
        stats.service_keys = service_keys.into_iter().cloned().collect();

        for published in generation.services.values() {
            let ServicePublishedState::Available(service) = published else {
                continue;
            };
            let runtime = service.runtime_cell.active.load_full();
            if runtime.membership_identity != Some(service.membership_identity) {
                continue;
            }
            stats.active_runtime_services += 1;
            stats.active_runtime_endpoints += runtime.entries.len();
            stats.sticky_runtime_services += usize::from(!service.runtime_cell.sp.load_full().is_empty());
            stats.health_policy_services += usize::from(runtime.health_policy_present);
            stats.outlier_policy_services += usize::from(runtime.outlier_policy_present);
            stats.degrade_policy_services += usize::from(runtime.degrade_threshold > 0);
        }

        let draining = crate::lb::runtime_state::draining_stats(STATS_KEY_LIMIT);
        stats.draining_backends = draining.backend_count;
        stats.draining_service_keys = draining.service_keys;
        stats.draining_service_keys_truncated = draining.service_keys_truncated;
        stats
    }

    fn publish_policy_snapshot(
        service_key: &str,
        cell: &ServiceRuntimeCell,
        active: &Arc<ServiceRuntimeSnapshot>,
        health_policy_present: bool,
        outlier_policy_present: bool,
        degrade_threshold: u8,
    ) {
        cell.active.store(Arc::new(
            ServiceRuntimeSnapshot {
                membership_identity: active.membership_identity,
                entries: Arc::clone(&active.entries),
                health_policy_present,
                outlier_policy_present,
                degrade_threshold,
                degrade_selections: None,
            }
            .with_degrade_counter(service_key, active),
        ));
    }

    pub(crate) fn set_health_policy(&self, service_key: &str, policy: Option<Arc<HealthPolicyIdentity>>) {
        let _coordinator_guard = self.state.lock();
        match policy.as_ref() {
            Some(policy) => {
                self.health_policies.insert(service_key.to_string(), Arc::clone(policy));
            }
            None => {
                self.health_policies.remove(service_key);
            }
        }
        let Some(cell) = self.runtime_cell(service_key) else {
            return;
        };
        let active = cell.active.load_full();
        for state in active.entries.values() {
            let mut completion = state.completion_lock.lock();
            let current = cell.active.load_full();
            if current
                .entries
                .get(state.identity.as_ref())
                .is_some_and(|candidate| Arc::ptr_eq(candidate, state))
            {
                completion.health_policy = policy.clone();
                completion.consecutive_health_successes = 0;
                completion.consecutive_health_failures = 0;
                state.set_health_eligible(true);
            }
        }
        let current = cell.active.load_full();
        if Arc::ptr_eq(&active, &current) {
            Self::publish_policy_snapshot(
                service_key,
                &cell,
                &active,
                policy.is_some(),
                active.outlier_policy_present,
                active.degrade_threshold,
            );
        }
    }

    pub(crate) fn set_outlier_policy(&self, service_key: &str, policy: Option<Arc<OutlierPolicyIdentity>>) {
        let _coordinator_guard = self.state.lock();
        match policy.as_ref() {
            Some(policy) => {
                self.outlier_policies
                    .insert(service_key.to_string(), Arc::clone(policy));
            }
            None => {
                self.outlier_policies.remove(service_key);
            }
        }
        let Some(cell) = self.runtime_cell(service_key) else {
            return;
        };
        let active = cell.active.load_full();
        for state in active.entries.values() {
            let mut completion = state.completion_lock.lock();
            let current = cell.active.load_full();
            if current
                .entries
                .get(state.identity.as_ref())
                .is_some_and(|candidate| Arc::ptr_eq(candidate, state))
            {
                completion.outlier_policy = policy.clone();
                state.clear_outlier_ejected();
            }
        }
        let current = cell.active.load_full();
        if Arc::ptr_eq(&active, &current) {
            Self::publish_policy_snapshot(
                service_key,
                &cell,
                &active,
                active.health_policy_present,
                policy.is_some(),
                active.degrade_threshold,
            );
        }
    }

    fn refresh_degrade_threshold_locked(
        &self,
        generation: &ServiceSelectorGeneration,
        service_key: &str,
        resolve: impl FnOnce() -> u8,
    ) {
        let Some(service) = generation.available(service_key) else {
            return;
        };
        let cell = Arc::clone(&service.runtime_cell);
        let active = cell.active.load_full();
        if active.membership_identity != Some(service.membership_identity) {
            return;
        }
        let degrade_threshold = resolve();
        Self::publish_policy_snapshot(
            service_key,
            &cell,
            &active,
            active.health_policy_present,
            active.outlier_policy_present,
            degrade_threshold,
        );
    }

    fn refresh_degrade_threshold_with(&self, service_key: &str, resolve: impl FnOnce() -> u8) {
        let _coordinator_guard = self.state.lock();
        let generation = self.load_generation();
        self.refresh_degrade_threshold_locked(&generation, service_key, resolve);
    }

    pub(crate) fn refresh_degrade_threshold(&self, service_key: &str) {
        self.refresh_degrade_threshold_with(service_key, || {
            crate::backends::degrade_threshold::resolve_degrade_threshold(service_key)
        });
    }

    #[cfg(test)]
    pub(crate) fn set_degrade_threshold(&self, service_key: &str, degrade_threshold: u8) {
        self.refresh_degrade_threshold_with(service_key, || degrade_threshold);
    }

    pub(crate) fn refresh_all_degrade_thresholds(&self) {
        let _coordinator_guard = self.state.lock();
        let generation = self.load_generation();
        let keys: Vec<String> = generation.services.keys().cloned().collect();
        for key in keys {
            self.refresh_degrade_threshold_locked(&generation, &key, || {
                crate::backends::degrade_threshold::resolve_degrade_threshold(&key)
            });
        }
    }

    pub(crate) fn record_health_completion(
        &self,
        policy: &Arc<HealthPolicyIdentity>,
        membership_identity: u64,
        endpoint_identity: &EndpointRuntimeIdentity,
        success: bool,
        healthy_threshold: u32,
        unhealthy_threshold: u32,
    ) -> bool {
        let service_key = endpoint_identity.service_key.as_ref();
        self.with_current_endpoint_locked(
            service_key,
            membership_identity,
            endpoint_identity,
            |state, completion, _active_endpoint_count| {
                let Some(current_policy) = completion.health_policy.as_ref() else {
                    return false;
                };
                if current_policy.generation != policy.generation || !Arc::ptr_eq(current_policy, policy) {
                    return false;
                }
                if success {
                    completion.consecutive_health_failures = 0;
                    completion.consecutive_health_successes = completion.consecutive_health_successes.saturating_add(1);
                    if !state.health_eligible() && completion.consecutive_health_successes >= healthy_threshold.max(1) {
                        state.set_health_eligible(true);
                    }
                } else {
                    completion.consecutive_health_successes = 0;
                    completion.consecutive_health_failures = completion.consecutive_health_failures.saturating_add(1);
                    if state.health_eligible() && completion.consecutive_health_failures >= unhealthy_threshold.max(1) {
                        state.set_health_eligible(false);
                    }
                }
                true
            },
        )
        .unwrap_or(false)
    }

    pub(crate) fn with_current_outlier_endpoint<R>(
        &self,
        service_key: &str,
        membership_identity: u64,
        endpoint_identity: &EndpointRuntimeIdentity,
        policy: &Arc<OutlierPolicyIdentity>,
        apply: impl FnOnce(&EndpointRuntimeState, usize) -> R,
    ) -> Option<R> {
        self.with_current_endpoint_locked(
            service_key,
            membership_identity,
            endpoint_identity,
            |state, completion, active_endpoint_count| {
                let current = completion.outlier_policy.as_ref()?;
                (current.generation == policy.generation && Arc::ptr_eq(current, policy))
                    .then(|| apply(state, active_endpoint_count))
            },
        )?
    }

    fn with_current_endpoint_locked<R>(
        &self,
        service_key: &str,
        membership_identity: u64,
        endpoint_identity: &EndpointRuntimeIdentity,
        apply: impl FnOnce(&EndpointRuntimeState, &mut EndpointCompletionState, usize) -> R,
    ) -> Option<R> {
        let generation = self.load_generation();
        let cell = &generation.available(service_key)?.runtime_cell;
        let active = cell.active.load_full();
        if active.membership_identity != Some(membership_identity) {
            return None;
        }
        let state = active.entries.get(endpoint_identity)?;
        if state.identity.as_ref() != endpoint_identity {
            return None;
        }
        let mut completion = state.completion_lock.lock();
        let current = cell.active.load_full();
        if current.membership_identity != Some(membership_identity) {
            return None;
        }
        let current_state = current.entries.get(endpoint_identity)?;
        if !Arc::ptr_eq(current_state, state) || current_state.identity.as_ref() != endpoint_identity {
            return None;
        }
        #[cfg(test)]
        if let Some((reached, resume)) = self.completion_barrier.lock().clone() {
            reached.wait();
            resume.wait();
        }
        Some(apply(state, &mut completion, current.entries.len()))
    }

    pub fn replace_services(&self, data: &HashMap<String, Arc<Service>>) {
        self.transact(|state, affected| {
            let old_keys: HashSet<String> = state.services.keys().cloned().collect();
            let new_keys: HashSet<String> = data.keys().cloned().collect();
            affected.extend(old_keys.union(&new_keys).cloned());
            let mut next = HashMap::with_capacity(data.len());
            for (key, service) in data {
                next.insert(
                    key.clone(),
                    stage_service_record(state.services.get(key), Arc::clone(service)),
                );
            }
            state.services = next;
        });
    }

    pub fn update_services(
        &self,
        add: HashMap<String, Arc<Service>>,
        update: HashMap<String, Arc<Service>>,
        remove: HashSet<String>,
    ) {
        self.transact(move |state, affected| {
            let mut fresh_keys: HashSet<String> = add.keys().cloned().collect();
            let mut incoming: BTreeMap<String, Arc<Service>> = add.into_iter().collect();
            incoming.extend(update);
            fresh_keys.extend(remove.iter().filter(|key| incoming.contains_key(*key)).cloned());
            for key in remove.into_iter().filter(|key| !incoming.contains_key(key)) {
                state.services.remove(&key);
                affected.insert(key);
            }
            for (key, service) in incoming {
                affected.insert(key.clone());
                let old = (!fresh_keys.contains(&key)).then(|| state.services.get(&key)).flatten();
                let record = stage_service_record(old, service);
                state.services.insert(key, record);
            }
        });
    }

    pub fn replace_endpoint_slices(&self, data: &HashMap<String, Arc<EndpointSlice>>) {
        self.transact(|state, affected| {
            for old in state.endpoint_slices.values() {
                affected.insert(endpoint_slice_service_key(&old.resource));
            }
            let mut next = HashMap::with_capacity(data.len());
            for (key, slice) in data {
                affected.insert(endpoint_slice_service_key(slice));
                next.insert(
                    key.clone(),
                    stage_endpoint_slice_record(state.endpoint_slices.get(key), Arc::clone(slice)),
                );
            }
            state.endpoint_slices = next;
        });
    }

    pub fn update_endpoint_slices(
        &self,
        add: HashMap<String, Arc<EndpointSlice>>,
        update: HashMap<String, Arc<EndpointSlice>>,
        remove: HashSet<String>,
    ) {
        self.transact(move |state, affected| {
            let mut fresh_keys: HashSet<String> = add.keys().cloned().collect();
            let mut incoming: BTreeMap<String, Arc<EndpointSlice>> = add.into_iter().collect();
            incoming.extend(update);
            fresh_keys.extend(remove.iter().filter(|key| incoming.contains_key(*key)).cloned());
            for key in remove.into_iter().filter(|key| !incoming.contains_key(key)) {
                if let Some(old) = state.endpoint_slices.remove(&key) {
                    affected.insert(endpoint_slice_service_key(&old.resource));
                }
            }
            for (key, slice) in incoming {
                if let Some(old) = state.endpoint_slices.get(&key) {
                    affected.insert(endpoint_slice_service_key(&old.resource));
                }
                affected.insert(endpoint_slice_service_key(&slice));
                let old = (!fresh_keys.contains(&key))
                    .then(|| state.endpoint_slices.get(&key))
                    .flatten();
                let record = stage_endpoint_slice_record(old, slice);
                state.endpoint_slices.insert(key, record);
            }
        });
    }

    fn transact(&self, mutate: impl FnOnce(&mut CoordinatorState, &mut HashSet<String>)) {
        let mut state = self.state.lock();
        let mut staged = state.clone();
        let mut affected = HashSet::new();
        mutate(&mut staged, &mut affected);
        if affected.is_empty() {
            return;
        }

        let current = self.published.load_full();
        let mut services = clone_published_map(&current.services);
        let mut sorted: Vec<String> = affected.into_iter().collect();
        sorted.sort();

        let failure_state = staged.clone();
        let source_epochs = source_epoch_requirement(&staged, &sorted);
        // Preview the exact source identities in a throwaway staged state, then
        // run the same normalized projection builder used by publication. This
        // makes reservation size independent of HashMap iteration order and
        // removes count/build drift at the terminal epoch boundary.
        let mut preflight_state = staged.clone();
        let source_preview = preview_epoch_range(state.next_epoch, source_epochs);
        if let Some(epochs) = source_preview.as_ref() {
            assign_source_epochs(&mut preflight_state, &sorted, epochs.iter().copied());
        }
        let projection_epochs = source_preview.and_then(|_| {
            sorted.iter().try_fold(0usize, |total, key| {
                total.checked_add(projection_epoch_requirement(key, &preflight_state, services.get(key)))
            })
        });
        let total_epochs = projection_epochs.and_then(|projection| source_epochs.checked_add(projection));
        let reservation = total_epochs.and_then(|total| reserve_transaction_epochs(&state, total));
        let allocation_failed = reservation.is_none();
        if let Some(reservation) = reservation.as_ref() {
            assign_source_epochs(
                &mut staged,
                &sorted,
                reservation.epochs[..source_epochs].iter().copied(),
            );
        } else {
            staged = failure_state;
            staged.exhausted = true;
        }

        let mut projection_epoch_values = reservation
            .as_ref()
            .map(|reservation| reservation.epochs[source_epochs..].iter().copied())
            .into_iter()
            .flatten();
        let allocations_available = !allocation_failed;
        let mut changed = Vec::new();
        for key in &sorted {
            let runtime_cell = match services.get(key) {
                Some(ServicePublishedState::Available(old)) => Some(Arc::clone(&old.runtime_cell)),
                Some(ServicePublishedState::Unavailable(_)) | None
                    if allocations_available && service_sources_are_identified(key, &staged) =>
                {
                    Some(
                        self.runtime_cells
                            .entry(key.clone())
                            .or_insert_with(|| Arc::new(ServiceRuntimeCell::new()))
                            .clone(),
                    )
                }
                Some(ServicePublishedState::Unavailable(_)) | None => None,
            };
            let next = build_service_projection(
                key,
                &staged,
                services.get(key),
                allocations_available,
                &mut projection_epoch_values,
                runtime_cell,
            );
            let differs = !published_state_semantically_equal(services.get(key), next.as_ref());
            match next {
                Some(next) => {
                    services.insert(key.clone(), next);
                }
                None => {
                    services.remove(key);
                }
            }
            if differs {
                changed.push(key.clone());
            }
        }
        assert!(
            projection_epoch_values.next().is_none(),
            "projection epoch preflight must reserve the exact normalized candidate count"
        );

        if let Some(reservation) = reservation {
            staged.next_epoch = reservation.next_epoch;
            staged.exhausted = reservation.exhausted;
        }
        let stats = generation_stats(&staged, &services);
        let inventory_changed = current.stats != stats;
        *state = staged;
        if changed.is_empty() && !inventory_changed {
            return;
        }

        let next = Arc::new(ServiceSelectorGeneration { services, stats });
        // Publication precedes per-Service activation. Until a Service's cell
        // is activated, selectors see a membership mismatch and return no
        // backends (fail-closed 503), never a stale membership.
        self.published.store(next.clone());

        for key in changed {
            self.activate_service(&key, next.available(&key));
        }
    }

    fn activate_service(&self, service_key: &str, projection: Option<&Arc<AvailableServiceProjection>>) {
        let cell = projection
            .map(|projection| Arc::clone(&projection.runtime_cell))
            .or_else(|| self.runtime_cell(service_key))
            .unwrap_or_else(|| Arc::new(ServiceRuntimeCell::new()));
        let old_active = cell.active.load_full();
        cell.active.store(Arc::new(ServiceRuntimeSnapshot::inactive()));

        match projection {
            Some(projection) => {
                let health_policy = self
                    .health_policies
                    .get(service_key)
                    .map(|entry| Arc::clone(entry.value()));
                let outlier_policy = self
                    .outlier_policies
                    .get(service_key)
                    .map(|entry| Arc::clone(entry.value()));
                let probe_backends = projection.canonical_probe_backends();
                let mut entries = HashMap::with_capacity(probe_backends.len());
                for backend in probe_backends.iter() {
                    let Some(identity) = backend.runtime_identity().cloned() else {
                        continue;
                    };
                    let runtime = Arc::new(EndpointRuntimeState::fresh(
                        Arc::clone(&identity),
                        Arc::clone(backend),
                        health_policy.clone(),
                        outlier_policy.clone(),
                    ));
                    entries.insert(identity.as_ref().clone(), runtime);
                }
                for old in old_active.entries.values() {
                    let _completion = old.completion_lock.lock();
                    old.freeze_detached();
                    crate::backends::get_outlier_detection_store().remove_runtime_identity(old.identity.as_ref());
                    let address = old.backend.inner().addr.clone();
                    if old.backend.owned_conn() > 0 {
                        crate::lb::runtime_state::begin_draining(service_key, &address, Arc::clone(&old.backend));
                    } else {
                        crate::lb::runtime_state::remove_backend_exact(service_key, &address, &old.backend);
                    }
                    old.settle_detached_gauge();
                }
                cell.active.store(Arc::new(
                    ServiceRuntimeSnapshot {
                        membership_identity: Some(projection.membership_identity),
                        entries: Arc::new(entries),
                        health_policy_present: health_policy.is_some(),
                        outlier_policy_present: outlier_policy.is_some(),
                        degrade_threshold: crate::backends::degrade_threshold::resolve_degrade_threshold(service_key),
                        degrade_selections: None,
                    }
                    .with_degrade_counter(service_key, &old_active),
                ));
                let _guard = cell.sp_build_lock.lock();
                cell.sp.store(Arc::new(BTreeMap::new()));
            }
            None => {
                for old in old_active.entries.values() {
                    let _completion = old.completion_lock.lock();
                    old.freeze_detached();
                    crate::backends::get_outlier_detection_store().remove_runtime_identity(old.identity.as_ref());
                    let address = old.backend.inner().addr.clone();
                    if old.backend.owned_conn() > 0 {
                        crate::lb::runtime_state::begin_draining(service_key, &address, Arc::clone(&old.backend));
                    } else {
                        crate::lb::runtime_state::remove_backend_exact(service_key, &address, &old.backend);
                    }
                    old.settle_detached_gauge();
                }
                let _guard = cell.sp_build_lock.lock();
                cell.sp.store(Arc::new(BTreeMap::new()));
                drop(_guard);
                self.runtime_cells
                    .remove_if(service_key, |_, current| Arc::ptr_eq(current, &cell));
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn set_next_epoch_for_test(&self, next_epoch: u64) {
        let mut state = self.state.lock();
        state.next_epoch = next_epoch;
        state.exhausted = false;
    }

    #[cfg(test)]
    fn set_completion_barrier_for_test(&self, barrier: Option<(Arc<std::sync::Barrier>, Arc<std::sync::Barrier>)>) {
        *self.completion_barrier.lock() = barrier;
    }
}

impl Default for ServiceDiscoveryCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

fn clone_published_map(input: &HashMap<String, ServicePublishedState>) -> HashMap<String, ServicePublishedState> {
    input
        .iter()
        .map(|(key, state)| {
            let state = match state {
                ServicePublishedState::Available(value) => ServicePublishedState::Available(Arc::clone(value)),
                ServicePublishedState::Unavailable(reason) => ServicePublishedState::Unavailable(reason.clone()),
            };
            (key.clone(), state)
        })
        .collect()
}

fn source_identity_is_same_live<T>(record: &LiveSourceRecord<T>, uid: Option<&str>) -> bool {
    match (&record.identity, uid.filter(|uid| !uid.is_empty())) {
        (Some(LiveSourceIdentity::KubernetesUid { uid: old, .. }), Some(new)) => old.as_ref() == new,
        (Some(LiveSourceIdentity::AuthoritativeEpoch(_)), None) => true,
        _ => false,
    }
}

fn source_identity(uid: Option<&str>, epoch: u64) -> LiveSourceIdentity {
    match uid.filter(|uid| !uid.is_empty()) {
        Some(uid) => LiveSourceIdentity::KubernetesUid {
            uid: Arc::from(uid),
            live_epoch: epoch,
        },
        None => LiveSourceIdentity::AuthoritativeEpoch(epoch),
    }
}

fn stage_service_record(old: Option<&LiveSourceRecord<Service>>, resource: Arc<Service>) -> LiveSourceRecord<Service> {
    let identity = old
        .filter(|old| source_identity_is_same_live(old, resource.metadata.uid.as_deref()))
        .and_then(|old| old.identity.clone());
    LiveSourceRecord {
        semantic_digest: semantic_digest_service(&resource),
        resource,
        identity,
    }
}

fn stage_endpoint_slice_record(
    old: Option<&LiveSourceRecord<EndpointSlice>>,
    resource: Arc<EndpointSlice>,
) -> LiveSourceRecord<EndpointSlice> {
    let identity = old
        .filter(|old| source_identity_is_same_live(old, resource.metadata.uid.as_deref()))
        .and_then(|old| old.identity.clone());
    LiveSourceRecord {
        semantic_digest: semantic_digest_endpoint_slice(&resource),
        resource,
        identity,
    }
}

fn source_epoch_requirement(state: &CoordinatorState, affected: &[String]) -> usize {
    let affected: HashSet<&str> = affected.iter().map(String::as_str).collect();
    state
        .services
        .iter()
        .filter(|(key, source)| affected.contains(key.as_str()) && source.identity.is_none())
        .count()
        .saturating_add(
            state
                .endpoint_slices
                .values()
                .filter(|source| {
                    source.identity.is_none()
                        && affected.contains(endpoint_slice_service_key(&source.resource).as_str())
                })
                .count(),
        )
}

fn assign_source_epochs(state: &mut CoordinatorState, affected: &[String], epochs: impl Iterator<Item = u64>) {
    let affected: HashSet<&str> = affected.iter().map(String::as_str).collect();
    let mut epochs = epochs;
    let mut service_keys: Vec<_> = state
        .services
        .iter()
        .filter(|(key, source)| affected.contains(key.as_str()) && source.identity.is_none())
        .map(|(key, _)| key.clone())
        .collect();
    service_keys.sort();
    for key in service_keys {
        let source = state.services.get_mut(&key).expect("staged service source exists");
        let epoch = epochs
            .next()
            .expect("source epoch preflight reserved exact service count");
        source.identity = Some(source_identity(source.resource.metadata.uid.as_deref(), epoch));
    }
    let mut slice_keys: Vec<_> = state
        .endpoint_slices
        .iter()
        .filter(|(_, source)| {
            source.identity.is_none() && affected.contains(endpoint_slice_service_key(&source.resource).as_str())
        })
        .map(|(key, _)| key.clone())
        .collect();
    slice_keys.sort();
    for key in slice_keys {
        let source = state
            .endpoint_slices
            .get_mut(&key)
            .expect("staged EndpointSlice source exists");
        let epoch = epochs
            .next()
            .expect("source epoch preflight reserved exact EndpointSlice count");
        source.identity = Some(source_identity(source.resource.metadata.uid.as_deref(), epoch));
    }
    assert!(
        epochs.next().is_none(),
        "source epoch preflight must reserve the exact sorted source count"
    );
}

struct EpochReservation {
    epochs: Vec<u64>,
    next_epoch: u64,
    exhausted: bool,
}

fn preview_epoch_range(start: u64, count: usize) -> Option<Vec<u64>> {
    if count == 0 {
        return Some(Vec::new());
    }
    let count = u64::try_from(count).ok()?;
    let last = start.checked_add(count - 1)?;
    Some((start..=last).collect())
}

fn reserve_transaction_epochs(state: &CoordinatorState, count: usize) -> Option<EpochReservation> {
    if count == 0 {
        return Some(EpochReservation {
            epochs: Vec::new(),
            next_epoch: state.next_epoch,
            exhausted: state.exhausted,
        });
    }
    if state.exhausted {
        return None;
    }
    let epochs = preview_epoch_range(state.next_epoch, count)?;
    let last = *epochs.last().expect("non-empty epoch reservation");
    let next = last.checked_add(1);
    Some(EpochReservation {
        epochs,
        next_epoch: next.unwrap_or(last),
        exhausted: next.is_none(),
    })
}

fn semantic_digest<T: serde::Serialize>(resource: &T) -> [u8; 32] {
    let encoded = serde_json::to_vec(resource).unwrap_or_default();
    Sha256::digest(encoded).into()
}

fn strip_non_routing_metadata(metadata: &mut k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta) {
    metadata.resource_version = None;
    metadata.managed_fields = None;
    metadata.creation_timestamp = None;
    metadata.annotations = None;
    metadata.labels = None;
}

fn routing_service_annotations(service: &Service) -> Option<BTreeMap<String, String>> {
    let annotations = service.metadata.annotations.as_ref()?;
    let mut kept = BTreeMap::new();
    for key in [DNS_RESOLUTION_MODE, DNS_RESOLVER, DNS_RESOLVER_REF] {
        if let Some(value) = annotations.get(key) {
            kept.insert(key.to_string(), value.clone());
        }
    }
    (!kept.is_empty()).then_some(kept)
}

fn semantic_digest_service(service: &Service) -> [u8; 32] {
    let mut copy = service.clone();
    let routing_annotations = routing_service_annotations(service);
    strip_non_routing_metadata(&mut copy.metadata);
    copy.metadata.annotations = routing_annotations;
    copy.status = None;
    semantic_digest(&copy)
}

fn semantic_digest_endpoint_slice(slice: &EndpointSlice) -> [u8; 32] {
    let mut copy = slice.clone();
    strip_non_routing_metadata(&mut copy.metadata);
    semantic_digest(&copy)
}

fn endpoint_slice_service_name(slice: &EndpointSlice) -> &str {
    slice
        .metadata
        .labels
        .as_ref()
        .and_then(|labels| labels.get(SERVICE_NAME))
        .map(String::as_str)
        .unwrap_or_else(|| slice.metadata.name.as_deref().unwrap_or(""))
}

pub(super) fn endpoint_slice_service_key(slice: &EndpointSlice) -> String {
    let service_name = endpoint_slice_service_name(slice);
    match slice.metadata.namespace.as_deref() {
        Some(namespace) => format!("{namespace}/{service_name}"),
        None => service_name.to_string(),
    }
}

fn endpoint_slice_matches_service(slice: &EndpointSlice, service_key: &str) -> bool {
    let service_name = endpoint_slice_service_name(slice);
    match slice.metadata.namespace.as_deref() {
        Some(namespace) => {
            service_key
                .strip_prefix(namespace)
                .and_then(|suffix| suffix.strip_prefix('/'))
                == Some(service_name)
        }
        None => service_key == service_name,
    }
}

fn build_service_projection(
    service_key: &str,
    state: &CoordinatorState,
    old: Option<&ServicePublishedState>,
    allocations_available: bool,
    epochs: &mut impl Iterator<Item = u64>,
    runtime_cell: Option<Arc<ServiceRuntimeCell>>,
) -> Option<ServicePublishedState> {
    let Some(service_record) = state.services.get(service_key) else {
        let has_slices = state
            .endpoint_slices
            .values()
            .any(|slice| endpoint_slice_matches_service(&slice.resource, service_key));
        return has_slices.then_some(ServicePublishedState::Unavailable(
            ServiceUnavailableReason::MissingService,
        ));
    };
    if service_record.identity.is_none() {
        return Some(ServicePublishedState::Unavailable(
            ServiceUnavailableReason::InvalidEpoch,
        ));
    }

    let mut slices: Vec<(&str, &LiveSourceRecord<EndpointSlice>)> = state
        .endpoint_slices
        .iter()
        .filter(|(_, slice)| endpoint_slice_matches_service(&slice.resource, service_key))
        .map(|(key, slice)| (key.as_str(), slice))
        .collect();
    slices.sort_by(|left, right| left.0.cmp(right.0));
    if slices.iter().any(|(_, slice)| slice.identity.is_none()) {
        return Some(ServicePublishedState::Unavailable(
            ServiceUnavailableReason::InvalidEpoch,
        ));
    }

    let geometry = normalize_sections(service_record, &slices);
    let content_fingerprint = service_content_fingerprint(service_record, &slices, &geometry);
    if let Some(ServicePublishedState::Available(old)) = old {
        if old.content_fingerprint == content_fingerprint {
            return Some(ServicePublishedState::Available(Arc::clone(old)));
        }
    }
    if !allocations_available {
        return Some(ServicePublishedState::Unavailable(
            ServiceUnavailableReason::InvalidEpoch,
        ));
    }
    let membership_identity = projection_membership_identity(&geometry);
    let Some(sections) = materialize_sections(service_key, membership_identity, geometry, old, epochs) else {
        return Some(ServicePublishedState::Unavailable(
            ServiceUnavailableReason::InvalidEpoch,
        ));
    };
    let raw = Arc::new(LegacyRawProjection {
        service: Arc::clone(&service_record.resource),
        endpoint_slices: slices
            .iter()
            .map(|(_, slice)| Arc::clone(&slice.resource))
            .collect::<Vec<_>>()
            .into(),
    });
    Some(ServicePublishedState::Available(Arc::new(AvailableServiceProjection {
        dns_resolution_mode: super::DnsResolutionMode::parse(
            raw.service
                .metadata
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.get(DNS_RESOLUTION_MODE))
                .map(String::as_str),
        ),
        dns_resolver_candidate: edgion_common::dns::resolver_select::candidate_from_dns_owner(
            edgion_resources::resources::refs::dns_fields::DnsResolverOwner::Service(&service_record.resource),
        ),
        content_fingerprint,
        membership_identity,
        runtime_cell: runtime_cell.expect("identified service projection has a runtime cell"),
        raw,
        sections: Arc::new(sections),
    })))
}

fn service_sources_are_identified(service_key: &str, state: &CoordinatorState) -> bool {
    state
        .services
        .get(service_key)
        .is_some_and(|service| service.identity.is_some())
        && state
            .endpoint_slices
            .values()
            .all(|slice| !endpoint_slice_matches_service(&slice.resource, service_key) || slice.identity.is_some())
}

fn projection_epoch_requirement(
    service_key: &str,
    state: &CoordinatorState,
    old: Option<&ServicePublishedState>,
) -> usize {
    let Some(service) = state.services.get(service_key) else {
        return 0;
    };
    if service.identity.is_none() {
        return 0;
    }
    let mut slices: Vec<(&str, &LiveSourceRecord<EndpointSlice>)> = state
        .endpoint_slices
        .iter()
        .filter(|(_, slice)| endpoint_slice_matches_service(&slice.resource, service_key))
        .map(|(key, slice)| (key.as_str(), slice))
        .collect();
    slices.sort_by(|left, right| left.0.cmp(right.0));
    if slices.iter().any(|(_, slice)| slice.identity.is_none()) {
        return 0;
    }
    let sections = normalize_sections(service, &slices);
    let fingerprint = service_content_fingerprint(service, &slices, &sections);
    if matches!(old, Some(ServicePublishedState::Available(old)) if old.content_fingerprint == fingerprint) {
        return 0;
    }
    unique_normalized_slot_count(&sections)
}

fn normalize_sections(
    service: &LiveSourceRecord<Service>,
    slices: &[(&str, &LiveSourceRecord<EndpointSlice>)],
) -> BTreeMap<ServiceSectionKey, Arc<NormalizedSectionOutcome>> {
    let mut aliases: BTreeMap<ServiceSectionKey, Vec<Arc<NormalizedSectionOutcome>>> = BTreeMap::new();
    let mut sections = HashMap::<ServicePortIdentity, Arc<NormalizedSectionOutcome>>::new();
    let ports = service
        .resource
        .spec
        .as_ref()
        .and_then(|spec| spec.ports.as_deref())
        .unwrap_or(&[]);

    for port in ports {
        let identity = normalize_service_port(port);
        let outcome = match identity.as_ref() {
            Some(identity) => sections
                .entry(identity.clone())
                .or_insert_with(|| Arc::new(normalize_section(service, identity, slices)))
                .clone(),
            None => Arc::new(NormalizedSectionOutcome::Invalid(
                ServiceSectionInvalid::InvalidServicePort,
            )),
        };
        if let Some(number) = checked_port(port.port) {
            aliases
                .entry(ServiceSectionKey::Number(number))
                .or_default()
                .push(Arc::clone(&outcome));
        }
        if let Some(name) = port.name.as_deref().filter(|name| !name.is_empty()) {
            aliases
                .entry(ServiceSectionKey::Name(Arc::from(name)))
                .or_default()
                .push(outcome);
        }
    }

    aliases
        .into_iter()
        .map(|(key, identities)| {
            let outcome = match identities.as_slice() {
                [outcome] => Arc::clone(outcome),
                _ => Arc::new(NormalizedSectionOutcome::Invalid(
                    ServiceSectionInvalid::AmbiguousServicePort,
                )),
            };
            (key, outcome)
        })
        .collect()
}

fn normalize_service_port(port: &ServicePort) -> Option<ServicePortIdentity> {
    let number = checked_port(port.port)?;
    let target_port = match port.target_port.as_ref() {
        Some(IntOrString::Int(value)) => NormalizedTargetPort::Number(checked_port(*value)?),
        Some(IntOrString::String(value)) if !value.is_empty() => NormalizedTargetPort::Name(Arc::from(value.as_str())),
        Some(IntOrString::String(_)) => return None,
        None => NormalizedTargetPort::Number(number),
    };
    Some(ServicePortIdentity {
        name: port.name.as_deref().filter(|name| !name.is_empty()).map(Arc::from),
        number,
        protocol: Arc::from(port.protocol.as_deref().unwrap_or("TCP")),
        app_protocol: port
            .app_protocol
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(Arc::from),
        target_port,
    })
}

fn normalize_section(
    service: &LiveSourceRecord<Service>,
    identity: &ServicePortIdentity,
    slices: &[(&str, &LiveSourceRecord<EndpointSlice>)],
) -> NormalizedSectionOutcome {
    let mut grouped: BTreeMap<SocketAddr, BTreeSet<EndpointOrigin>> = BTreeMap::new();
    for (slice_key, slice) in slices {
        let ports = slice.resource.ports.as_deref().unwrap_or(&[]);
        let matching = match matching_endpoint_port(ports, identity) {
            Ok(Some(port)) => port,
            Ok(None) => {
                // A sibling slice may omit this Service port name (shared-topology
                // fixtures, or a rollout slice that has not advertised the port yet).
                // Skip it; MissingEndpointPort is only for "no slice served this port".
                continue;
            }
            Err(reason) => return NormalizedSectionOutcome::Invalid(reason),
        };
        let Some(port) = matching.port.and_then(checked_port) else {
            return NormalizedSectionOutcome::Invalid(ServiceSectionInvalid::InvalidServicePort);
        };
        let expected_v6 = match slice.resource.address_type.as_str() {
            "IPv4" => false,
            "IPv6" => true,
            _ => return NormalizedSectionOutcome::Invalid(ServiceSectionInvalid::AddressFamilyConflict),
        };
        let source_identity = slice.identity.as_ref().expect("identified slices are prevalidated");
        let mut origin_slice_key = None;
        for endpoint in &slice.resource.endpoints {
            let ready = endpoint
                .conditions
                .as_ref()
                .and_then(|conditions| conditions.ready)
                .unwrap_or(false);
            for address in &endpoint.addresses {
                let Ok(ip) = address.parse::<IpAddr>() else {
                    return NormalizedSectionOutcome::Invalid(ServiceSectionInvalid::InvalidEndpointAddress);
                };
                if ip.is_ipv6() != expected_v6 {
                    return NormalizedSectionOutcome::Invalid(ServiceSectionInvalid::AddressFamilyConflict);
                }
                let socket = SocketAddr::new(ip, port.get());
                grouped.entry(socket).or_default().insert(EndpointOrigin {
                    slice_key: Arc::clone(origin_slice_key.get_or_insert_with(|| Arc::from(*slice_key))),
                    source_identity: source_identity.clone(),
                    address: ip,
                    serving_port: port,
                    ready,
                });
            }
        }
    }

    let mut all = Vec::with_capacity(grouped.len());
    for (socket, origins) in grouped {
        let ready = origins.iter().any(|origin| origin.ready);
        let origin_set: Arc<[EndpointOrigin]> = origins.into_iter().collect::<Vec<_>>().into();
        let proof = slot_proof(service, identity, socket, ready, &origin_set);
        all.push(NormalizedBackendSlot {
            socket,
            ready,
            origin_set,
            proof,
        });
    }
    NormalizedSectionOutcome::Resolved(Arc::new(NormalizedServiceSection {
        identity: identity.clone(),
        slots: all,
    }))
}

fn matching_endpoint_port<'a>(
    ports: &'a [EndpointPort],
    identity: &ServicePortIdentity,
) -> Result<Option<&'a EndpointPort>, ServiceSectionInvalid> {
    let mut matching = ports.iter().filter(|port| endpoint_port_matches(port, identity));
    let first = matching.next();
    if matching.next().is_some() {
        return Err(ServiceSectionInvalid::AmbiguousEndpointPort);
    }
    if first.is_some() || identity.name.is_some() || ports.len() != 1 {
        return Ok(first);
    }
    // An unnamed single-port Service may omit ServicePort.name. Accept the
    // unique EndpointPort when protocol matches, even if that port is named.
    Ok(ports
        .iter()
        .find(|port| port.protocol.as_deref().unwrap_or("TCP") == identity.protocol.as_ref()))
}

fn endpoint_port_matches(port: &EndpointPort, identity: &ServicePortIdentity) -> bool {
    let protocol = port.protocol.as_deref().unwrap_or("TCP");
    if protocol != identity.protocol.as_ref() {
        return false;
    }
    match identity.name.as_deref() {
        Some(name) => port.name.as_deref() == Some(name),
        None => port.name.as_deref().is_none_or(str::is_empty),
    }
}

fn slot_proof(
    service: &LiveSourceRecord<Service>,
    identity: &ServicePortIdentity,
    socket: SocketAddr,
    ready: bool,
    origins: &[EndpointOrigin],
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(PROOF_DOMAIN);
    match service.identity.as_ref().expect("identified service is prevalidated") {
        LiveSourceIdentity::KubernetesUid { uid, live_epoch } => {
            digest.update(b"uid\0");
            digest.update(uid.as_bytes());
            digest.update(live_epoch.to_be_bytes());
        }
        LiveSourceIdentity::AuthoritativeEpoch(epoch) => {
            digest.update(b"epoch\0");
            digest.update(epoch.to_be_bytes());
        }
    }
    digest.update(identity.number.get().to_be_bytes());
    digest.update(identity.name.as_deref().unwrap_or("").as_bytes());
    digest.update(identity.protocol.as_bytes());
    digest.update(identity.app_protocol.as_deref().unwrap_or("").as_bytes());
    match &identity.target_port {
        NormalizedTargetPort::Number(port) => {
            digest.update([0]);
            digest.update(port.get().to_be_bytes());
        }
        NormalizedTargetPort::Name(name) => {
            digest.update([1]);
            digest.update(name.as_bytes());
        }
    }
    digest.update(socket.to_string().as_bytes());
    digest.update([u8::from(ready)]);
    for origin in origins {
        digest.update(origin.slice_key.as_bytes());
        digest.update([0]);
        update_source_identity_digest(&mut digest, Some(&origin.source_identity));
        digest.update(origin.address.to_string().as_bytes());
        digest.update(origin.serving_port.get().to_be_bytes());
        digest.update([u8::from(origin.ready)]);
    }
    digest.finalize().into()
}

fn unique_resolved_sections(
    sections: &BTreeMap<ServiceSectionKey, Arc<ServiceSectionOutcome>>,
) -> Vec<&ResolvedServiceSection> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for outcome in sections.values() {
        let ServiceSectionOutcome::Resolved(section) = outcome.as_ref() else {
            continue;
        };
        if seen.insert(Arc::as_ptr(section)) {
            out.push(section.as_ref());
        }
    }
    out
}

fn unique_normalized_sections(
    sections: &BTreeMap<ServiceSectionKey, Arc<NormalizedSectionOutcome>>,
) -> Vec<&NormalizedServiceSection> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for outcome in sections.values() {
        let NormalizedSectionOutcome::Resolved(section) = outcome.as_ref() else {
            continue;
        };
        if seen.insert(Arc::as_ptr(section)) {
            out.push(section.as_ref());
        }
    }
    out
}

fn unique_normalized_slot_count(sections: &BTreeMap<ServiceSectionKey, Arc<NormalizedSectionOutcome>>) -> usize {
    unique_normalized_sections(sections)
        .iter()
        .map(|section| section.slots.len())
        .sum()
}

fn projection_membership_identity(sections: &BTreeMap<ServiceSectionKey, Arc<NormalizedSectionOutcome>>) -> u64 {
    let mut proofs: Vec<[u8; 32]> = unique_normalized_sections(sections)
        .iter()
        .flat_map(|section| section.slots.iter().map(|slot| slot.proof))
        .collect();
    proofs.sort_unstable();
    let mut digest = Sha256::new();
    digest.update(MEMBERSHIP_DOMAIN);
    for proof in proofs {
        digest.update(proof);
    }
    let hash = digest.finalize();
    u64::from_be_bytes(hash[..8].try_into().expect("SHA-256 digest is 32 bytes"))
}

#[allow(clippy::type_complexity)]
fn identified_backends_for_reuse(
    old: Option<&ServicePublishedState>,
) -> (HashMap<[u8; 32], Arc<LbBackend>>, HashMap<SocketAddr, Arc<LbBackend>>) {
    let Some(ServicePublishedState::Available(old)) = old else {
        return (HashMap::new(), HashMap::new());
    };
    let mut by_proof = HashMap::new();
    let mut by_socket = HashMap::new();
    for section in unique_resolved_sections(&old.sections) {
        for slot in section.all_slots.iter() {
            by_proof.insert(slot.proof, Arc::clone(&slot.backend));
            if let Some(socket) = slot.backend.addr.as_inet().copied() {
                by_socket.insert(socket, Arc::clone(&slot.backend));
            }
        }
    }
    (by_proof, by_socket)
}

fn materialize_section(
    service_key: &str,
    membership_identity: u64,
    section: &NormalizedServiceSection,
    old_by_proof: &HashMap<[u8; 32], Arc<LbBackend>>,
    old_by_socket: &HashMap<SocketAddr, Arc<LbBackend>>,
    epochs: &mut impl Iterator<Item = u64>,
) -> Option<ResolvedServiceSection> {
    let mut all = Vec::with_capacity(section.slots.len());
    for slot in section.slots.iter() {
        let incarnation = epochs.next()?;
        let socket = slot.socket;
        let counters = old_by_proof
            .get(&slot.proof)
            .or_else(|| old_by_socket.get(&socket))
            .map(|backend| backend.runtime_counters())
            .unwrap_or_else(|| Arc::new(BackendRuntimeCounters::default()));
        let identity = Arc::new(EndpointRuntimeIdentity {
            service_key: Arc::from(service_key),
            scope: EndpointRuntimeScope::Canonical(slot.proof),
            socket,
            incarnation,
            proof_digest: slot.proof,
        });
        let backend = Arc::new(LbBackend::with_runtime_identity(
            Backend {
                addr: PingoraSocketAddr::Inet(socket),
                weight: 1,
                ext: Default::default(),
            },
            identity,
            membership_identity,
            counters,
        ));
        all.push(Arc::new(ResolvedBackendSlot {
            backend,
            ready: slot.ready,
            origin_set: Arc::clone(&slot.origin_set),
            proof: slot.proof,
        }));
    }
    let all_slots: Arc<[Arc<ResolvedBackendSlot>]> = all.into();
    let ready_slots: Arc<[Arc<ResolvedBackendSlot>]> = all_slots
        .iter()
        .filter(|slot| slot.ready)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    let ready_backends = ready_slots
        .iter()
        .map(|slot| Arc::clone(&slot.backend))
        .collect::<Vec<_>>()
        .into();
    Some(ResolvedServiceSection {
        identity: section.identity.clone(),
        prepared_app_protocol: section
            .identity
            .app_protocol
            .as_deref()
            .and_then(parse_backend_app_protocol),
        direct_endpoint_ports: compile_direct_endpoint_ports(&all_slots),
        all_slots,
        ready_slots,
        selectors: CompiledSelectors::build(ready_backends),
    })
}

fn materialize_sections(
    service_key: &str,
    membership_identity: u64,
    sections: BTreeMap<ServiceSectionKey, Arc<NormalizedSectionOutcome>>,
    old: Option<&ServicePublishedState>,
    epochs: &mut impl Iterator<Item = u64>,
) -> Option<BTreeMap<ServiceSectionKey, Arc<ServiceSectionOutcome>>> {
    let (old_by_proof, old_by_socket) = identified_backends_for_reuse(old);
    let mut remapped_unique = HashMap::new();
    let mut remapped_invalid = HashMap::new();
    let mut out = BTreeMap::new();
    for (key, outcome) in sections {
        let remapped = match outcome.as_ref() {
            NormalizedSectionOutcome::Invalid(reason) => Arc::clone(
                remapped_invalid
                    .entry(Arc::as_ptr(&outcome))
                    .or_insert_with(|| Arc::new(ServiceSectionOutcome::Invalid(reason.clone()))),
            ),
            NormalizedSectionOutcome::Resolved(section) => {
                let ptr = Arc::as_ptr(section) as usize;
                if let Some(existing) = remapped_unique.get(&ptr) {
                    Arc::clone(existing)
                } else {
                    let materialized = materialize_section(
                        service_key,
                        membership_identity,
                        section,
                        &old_by_proof,
                        &old_by_socket,
                        epochs,
                    )?;
                    let wrapped = Arc::new(ServiceSectionOutcome::Resolved(Arc::new(materialized)));
                    remapped_unique.insert(ptr, Arc::clone(&wrapped));
                    wrapped
                }
            }
        };
        out.insert(key, remapped);
    }
    Some(out)
}

fn service_content_fingerprint(
    service: &LiveSourceRecord<Service>,
    slices: &[(&str, &LiveSourceRecord<EndpointSlice>)],
    sections: &BTreeMap<ServiceSectionKey, Arc<NormalizedSectionOutcome>>,
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(PROOF_DOMAIN);
    update_source_identity_digest(&mut digest, service.identity.as_ref());
    digest.update(service.semantic_digest);
    for (key, slice) in slices {
        digest.update(key.as_bytes());
        update_source_identity_digest(&mut digest, slice.identity.as_ref());
        digest.update(slice.semantic_digest);
    }
    for (key, outcome) in sections {
        digest.update(format!("{key:?}").as_bytes());
        match outcome.as_ref() {
            NormalizedSectionOutcome::Resolved(section) => {
                for slot in section.slots.iter() {
                    digest.update(slot.proof);
                }
            }
            NormalizedSectionOutcome::Invalid(reason) => digest.update(format!("{reason:?}").as_bytes()),
        }
    }
    digest.finalize().into()
}

fn update_source_identity_digest(digest: &mut Sha256, identity: Option<&LiveSourceIdentity>) {
    match identity {
        Some(LiveSourceIdentity::KubernetesUid { uid, live_epoch }) => {
            digest.update(b"uid\0");
            digest.update(uid.as_bytes());
            digest.update(live_epoch.to_be_bytes());
        }
        Some(LiveSourceIdentity::AuthoritativeEpoch(epoch)) => {
            digest.update(b"authoritative\0");
            digest.update(epoch.to_be_bytes());
        }
        None => digest.update(b"unidentified\0"),
    }
}

fn checked_port(port: i32) -> Option<NonZeroU16> {
    u16::try_from(port).ok().and_then(NonZeroU16::new)
}

fn published_state_semantically_equal(
    old: Option<&ServicePublishedState>,
    new: Option<&ServicePublishedState>,
) -> bool {
    match (old, new) {
        (None, None) => true,
        (Some(ServicePublishedState::Unavailable(left)), Some(ServicePublishedState::Unavailable(right))) => {
            left == right
        }
        (Some(ServicePublishedState::Available(left)), Some(ServicePublishedState::Available(right))) => {
            projection_fingerprint(left) == projection_fingerprint(right)
        }
        _ => false,
    }
}

fn projection_fingerprint(projection: &AvailableServiceProjection) -> [u8; 32] {
    projection.content_fingerprint
}

fn generation_stats(
    state: &CoordinatorState,
    services: &HashMap<String, ServicePublishedState>,
) -> ServiceSelectorGenerationStats {
    let mut stats = ServiceSelectorGenerationStats {
        service_sources: state.services.len(),
        endpoint_slice_sources: state.endpoint_slices.len(),
        identified_service_sources: state
            .services
            .values()
            .filter(|source| source.identity.is_some())
            .count(),
        identified_endpoint_slice_sources: state
            .endpoint_slices
            .values()
            .filter(|source| source.identity.is_some())
            .count(),
        unidentified_service_sources: state
            .services
            .values()
            .filter(|source| source.identity.is_none())
            .count(),
        unidentified_endpoint_slice_sources: state
            .endpoint_slices
            .values()
            .filter(|source| source.identity.is_none())
            .count(),
        ..Default::default()
    };
    for published in services.values() {
        match published {
            ServicePublishedState::Unavailable(_) => stats.unavailable_services += 1,
            ServicePublishedState::Available(service) => {
                stats.available_services += 1;
                let mut seen = HashSet::new();
                for outcome in service.sections.values() {
                    if !seen.insert(Arc::as_ptr(outcome)) {
                        continue;
                    }
                    match outcome.as_ref() {
                        ServiceSectionOutcome::Resolved(section) => {
                            stats.valid_sections += 1;
                            stats.canonical_slots += section.all_slots.len();
                        }
                        ServiceSectionOutcome::Invalid(_) => stats.invalid_sections += 1,
                    }
                }
            }
        }
    }
    stats
}

#[cfg(test)]
#[path = "../../../tests/unit/backends/discovery/coordinator_test.rs"]
mod tests;
