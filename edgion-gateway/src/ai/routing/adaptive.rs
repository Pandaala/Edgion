//! Bounded adaptive-routing telemetry aggregation.
//!
//! Attempt completion records into one-second ring buckets. A cold-path tick
//! publishes immutable integer snapshots. Admitted attempts additionally hold
//! an RAII in-flight lease, so selection sees concurrent load immediately
//! instead of waiting for completions to be published.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::plugins::runtime::gateway_api_filters::extension_ref::extension_ref_key;
use arc_swap::ArcSwap;
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::resources::edgion_plugins::{AiBackendSelection, EdgionPlugin, RequestFilterEntry};
use edgion_resources::resources::http_route::{HTTPBackendRefKind, HTTPRoute};
use edgion_resources::resources::refs::route_fields::HttpRouteExtensionRefs;

use parking_lot::Mutex as ParkingMutex;
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;

use super::candidate::AdaptiveSnapshot;
const LATENCY_WINDOW_SECONDS: u64 = 30;
const USAGE_WINDOW_SECONDS: u64 = 60;
const RING_BUCKETS: usize = USAGE_WINDOW_SECONDS as usize + 1;
const TTFT_UPPER_BOUNDS_MICROS: [u64; 32] = [
    100,
    250,
    500,
    750,
    1_000,
    1_500,
    2_000,
    3_000,
    5_000,
    7_500,
    10_000,
    15_000,
    20_000,
    30_000,
    50_000,
    75_000,
    100_000,
    150_000,
    250_000,
    500_000,
    750_000,
    1_000_000,
    1_500_000,
    2_500_000,
    5_000_000,
    10_000_000,
    20_000_000,
    30_000_000,
    60_000_000,
    120_000_000,
    300_000_000,
    u64::MAX,
];
const TTFT_HISTOGRAM_BUCKETS: usize = TTFT_UPPER_BOUNDS_MICROS.len();
const CLOSED_EPOCH_BIT: u64 = 1 << 63;
const ROTATING_EPOCH: u64 = u64::MAX;

static GLOBAL_ADAPTIVE_TELEMETRY_STORE: LazyLock<Arc<AdaptiveTelemetryStore>> =
    LazyLock::new(|| Arc::new(AdaptiveTelemetryStore::new()));
static GLOBAL_ADAPTIVE_CONTRIBUTIONS: LazyLock<ParkingMutex<AdaptiveContributionIndex>> =
    LazyLock::new(|| ParkingMutex::new(AdaptiveContributionIndex::default()));

pub fn get_adaptive_telemetry_store() -> &'static Arc<AdaptiveTelemetryStore> {
    &GLOBAL_ADAPTIVE_TELEMETRY_STORE
}

/// Stable adaptive-state identity. The terminal route UID prevents a
/// delete-create of the same route name from inheriting old telemetry.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AdaptiveTelemetryKey {
    pub terminal_route_uid: String,
    pub rule_name: String,
    pub target: PolicyTargetKey,
    pub logical_model: String,
}

impl AdaptiveTelemetryKey {
    pub fn new(
        terminal_route_uid: String,
        rule_name: String,
        target: PolicyTargetKey,
        logical_model: String,
    ) -> Option<Self> {
        if terminal_route_uid.is_empty()
            || rule_name.is_empty()
            || logical_model.is_empty()
            || target.target_kind != PolicyTargetKind::EdgionBackend
        {
            return None;
        }
        Some(Self {
            terminal_route_uid,
            rule_name,
            target,
            logical_model,
        })
    }
}

struct SecondBucket {
    second: AtomicU64,
    active_writers: AtomicU32,
    ttft_histogram: [AtomicU32; TTFT_HISTOGRAM_BUCKETS],
    successful_ttft_count: AtomicU32,
    failed_attempts: AtomicU32,
    charged_tokens: AtomicU64,
}

impl SecondBucket {
    fn new() -> Self {
        Self {
            second: AtomicU64::new(ROTATING_EPOCH),
            active_writers: AtomicU32::new(0),
            ttft_histogram: std::array::from_fn(|_| AtomicU32::new(0)),
            successful_ttft_count: AtomicU32::new(0),
            failed_attempts: AtomicU32::new(0),
            charged_tokens: AtomicU64::new(0),
        }
    }

    /// Enter only if this slot still represents `second`. Rotation changes the
    /// epoch first, then drains admitted writers before clearing counters.
    fn enter(&self, second: u64) -> bool {
        if self.second.load(Ordering::Acquire) != second {
            return false;
        }
        let mut writers = self.active_writers.load(Ordering::Relaxed);
        loop {
            let Some(next) = writers.checked_add(1) else {
                return false;
            };
            match self
                .active_writers
                .compare_exchange_weak(writers, next, Ordering::AcqRel, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(actual) => writers = actual,
            }
        }
        if self.second.load(Ordering::Acquire) == second {
            true
        } else {
            self.active_writers.fetch_sub(1, Ordering::Release);
            false
        }
    }

    fn leave(&self) {
        self.active_writers.fetch_sub(1, Ordering::Release);
    }

    /// Cold-tick rotation. Request writers never wait or clear shared state.
    fn rotate_to(&self, second: u64) {
        if self.second.load(Ordering::Acquire) == second {
            return;
        }
        self.second.store(ROTATING_EPOCH, Ordering::Release);
        while self.active_writers.load(Ordering::Acquire) != 0 {
            std::thread::yield_now();
        }
        for count in &self.ttft_histogram {
            count.store(0, Ordering::Relaxed);
        }
        self.successful_ttft_count.store(0, Ordering::Relaxed);
        self.failed_attempts.store(0, Ordering::Relaxed);
        self.charged_tokens.store(0, Ordering::Relaxed);
        self.second.store(second, Ordering::Release);
    }

    fn freeze_before(&self, now_second: u64) {
        let second = self.second.load(Ordering::Acquire);
        if second == ROTATING_EPOCH || second & CLOSED_EPOCH_BIT != 0 || second >= now_second {
            return;
        }
        self.second.store(second | CLOSED_EPOCH_BIT, Ordering::Release);
        while self.active_writers.load(Ordering::Acquire) != 0 {
            std::thread::yield_now();
        }
    }
}

struct Buckets {
    ring: [SecondBucket; RING_BUCKETS],
}

impl Default for Buckets {
    fn default() -> Self {
        Self {
            ring: std::array::from_fn(|_| SecondBucket::new()),
        }
    }
}

impl Buckets {
    fn bucket(&self, second: u64) -> &SecondBucket {
        let index = (second % RING_BUCKETS as u64) as usize;
        &self.ring[index]
    }
}

/// One state handle retained by request plans and cleanup-safe captured Arcs.
pub struct AdaptiveTelemetryState {
    buckets: Buckets,
    tick_lock: ParkingMutex<()>,
    published: ArcSwap<AdaptiveSnapshot>,
    /// Admitted attempts that have not finished yet. Maintained only by
    /// [`AdaptiveInFlightLease`], so every increment has exactly one decrement.
    in_flight: AtomicU32,
    /// Provisional `input + output bound` tokens of in-flight attempts. The
    /// real charge replaces it through the windowed bucket at settlement.
    in_flight_tokens: AtomicU64,
    /// Unix second of the latest admission attempt, or 0 if none. Stamped
    /// before any admission check or await, so the LowestLatency idle probe
    /// closes even when admission is refused or the attempt later fails.
    last_attempt_second: AtomicU64,
}

/// Per-lease cap on provisional tokens. With at most `u32::MAX` concurrent
/// leases the aggregate stays below `u64::MAX`, so the wrapping add/sub pair
/// is exact and a reader never observes a wrapped sum.
const MAX_LEASE_TOKENS: u64 = u32::MAX as u64;

/// RAII in-flight accounting for one admitted attempt. Dropping it, on any
/// completion, failure, or request-abort path, releases exactly what it added.
pub struct AdaptiveInFlightLease {
    state: Arc<AdaptiveTelemetryState>,
    tokens: u64,
}

impl Drop for AdaptiveInFlightLease {
    fn drop(&mut self) {
        self.state.in_flight.fetch_sub(1, Ordering::Relaxed);
        self.state.in_flight_tokens.fetch_sub(self.tokens, Ordering::Relaxed);
    }
}

impl AdaptiveTelemetryState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            buckets: Buckets::default(),
            tick_lock: ParkingMutex::new(()),
            published: ArcSwap::from_pointee(AdaptiveSnapshot::default()),
            in_flight: AtomicU32::new(0),
            in_flight_tokens: AtomicU64::new(0),
            last_attempt_second: AtomicU64::new(0),
        })
    }

    /// Mark that an attempt to this candidate entered admission. Called before
    /// circuit, outlier, and quota checks so a candidate that keeps refusing
    /// admission is not re-probed on every selection.
    pub fn mark_attempt(&self, second: u64) {
        self.last_attempt_second.fetch_max(second, Ordering::Relaxed);
    }

    /// Account one admitted attempt as in flight until the lease is dropped.
    pub fn begin_attempt(self: &Arc<Self>, provisional_tokens: u64) -> AdaptiveInFlightLease {
        let tokens = provisional_tokens.min(MAX_LEASE_TOKENS);
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        self.in_flight_tokens.fetch_add(tokens, Ordering::Relaxed);
        AdaptiveInFlightLease {
            state: Arc::clone(self),
            tokens,
        }
    }

    /// Record one attempt whose backend-health verdict was a failure. It feeds
    /// the LowestLatency success-ratio penalty; fast failures must not look
    /// like fast backends.
    pub fn record_failure(&self, second: u64) {
        let bucket = self.buckets.bucket(second);
        if bucket.enter(second) {
            saturating_add_u32(&bucket.failed_attempts, 1);
            bucket.leave();
        }
    }

    /// Record a successful final response's TTFT. Failures must not call this.
    pub fn record_success_ttft(&self, second: u64, ttft_micros: u64) {
        let bucket = self.buckets.bucket(second);
        if bucket.enter(second) {
            saturating_add_u32(&bucket.successful_ttft_count, 1);
            saturating_add_u32(&bucket.ttft_histogram[ttft_bucket(ttft_micros)], 1);
            bucket.leave();
        }
    }

    /// Charge tokens for every dispatched attempt, including failed attempts
    /// whose conservative accounting retains a reservation.
    pub fn record_charged_tokens(&self, second: u64, charged_tokens: u64) {
        let bucket = self.buckets.bucket(second);
        if bucket.enter(second) {
            saturating_add_u64(&bucket.charged_tokens, charged_tokens);
            bucket.leave();
        }
    }

    /// Lock-free request-path read: the last published windows plus the live
    /// in-flight counters. No sorting or floating point occurs.
    #[inline]
    pub fn snapshot(&self) -> AdaptiveSnapshot {
        let mut snapshot = **self.published.load();
        snapshot.in_flight = self.in_flight.load(Ordering::Relaxed);
        snapshot.in_flight_tokens = self.in_flight_tokens.load(Ordering::Relaxed);
        snapshot.last_attempt_second = self.last_attempt_second.load(Ordering::Relaxed);
        snapshot
    }

    /// Publish windows ending immediately before `now_second`; the current
    /// in-progress second is never included.
    pub fn tick(&self, now_second: u64) {
        let _tick_guard = self.tick_lock.lock();
        let latency_start = now_second.saturating_sub(LATENCY_WINDOW_SECONDS);
        let usage_start = now_second.saturating_sub(USAGE_WINDOW_SECONDS);
        let mut histogram = [0u64; TTFT_HISTOGRAM_BUCKETS];
        let mut latency_samples = 0u32;
        let mut latency_failures = 0u32;
        let mut charged_tokens = 0u64;
        for bucket in &self.buckets.ring {
            bucket.freeze_before(now_second);
        }
        for bucket in &self.buckets.ring {
            let epoch = bucket.second.load(Ordering::Acquire);
            if epoch == ROTATING_EPOCH {
                continue;
            }
            let second = epoch & !CLOSED_EPOCH_BIT;
            if second >= now_second {
                continue;
            }
            if second >= usage_start {
                charged_tokens = charged_tokens.saturating_add(bucket.charged_tokens.load(Ordering::Relaxed));
            }
            if second >= latency_start {
                latency_samples = latency_samples.saturating_add(bucket.successful_ttft_count.load(Ordering::Relaxed));
                latency_failures = latency_failures.saturating_add(bucket.failed_attempts.load(Ordering::Relaxed));
                for (aggregate, count) in histogram.iter_mut().zip(&bucket.ttft_histogram) {
                    *aggregate = aggregate.saturating_add(u64::from(count.load(Ordering::Relaxed)));
                }
            }
        }
        let latency_p50_micros = histogram_p50(&histogram);
        self.published.store(Arc::new(AdaptiveSnapshot {
            latency_p50_micros,
            latency_samples,
            latency_failures,
            charged_tokens_60s: charged_tokens,
            ..AdaptiveSnapshot::default()
        }));

        // Prepare the current in-progress second only after publishing all
        // complete buckets. Events arriving before this cold tick are dropped,
        // never written into a stale epoch.
        self.buckets.bucket(now_second).rotate_to(now_second);
    }
}

fn saturating_add_u32(value: &AtomicU32, increment: u32) {
    let mut current = value.load(Ordering::Relaxed);
    loop {
        let next = current.saturating_add(increment);
        match value.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(actual) => current = actual,
        }
    }
}

fn saturating_add_u64(value: &AtomicU64, increment: u64) {
    let mut current = value.load(Ordering::Relaxed);
    loop {
        let next = current.saturating_add(increment);
        match value.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(actual) => current = actual,
        }
    }
}

/// Fixed TTFT histogram. Boundaries are compile-time bounded and the final
/// bucket covers every u64 value.
fn ttft_bucket(value: u64) -> usize {
    TTFT_UPPER_BOUNDS_MICROS.partition_point(|upper| *upper < value)
}

fn histogram_p50(histogram: &[u64; TTFT_HISTOGRAM_BUCKETS]) -> Option<u64> {
    let total = histogram.iter().fold(0u64, |sum, count| sum.saturating_add(*count));
    if total == 0 {
        return None;
    }
    let rank = (total - 1) / 2;
    let mut seen = 0u64;
    for (index, count) in histogram.iter().enumerate() {
        seen = seen.saturating_add(*count);
        if seen > rank {
            return Some(TTFT_UPPER_BOUNDS_MICROS[index]);
        }
    }
    Some(TTFT_UPPER_BOUNDS_MICROS[TTFT_HISTOGRAM_BUCKETS - 1])
}

/// Cold-path lifecycle store. Map publication is immutable; request plans keep
/// captured state Arcs and do not consult the map after construction.
pub struct AdaptiveTelemetryStore {
    states: ArcSwap<HashMap<AdaptiveTelemetryKey, Arc<AdaptiveTelemetryState>>>,
    writer: ParkingMutex<()>,
}

impl Default for AdaptiveTelemetryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AdaptiveTelemetryStore {
    pub fn new() -> Self {
        Self {
            states: ArcSwap::from_pointee(HashMap::new()),
            writer: ParkingMutex::new(()),
        }
    }

    /// Lock-free request-plan lookup. Configuration reconciliation prebuilds
    /// every state, so this method never allocates or mutates the map.
    pub fn get(&self, key: &AdaptiveTelemetryKey) -> Option<Arc<AdaptiveTelemetryState>> {
        self.states.load().get(key).cloned()
    }

    /// Atomically publish the complete cold-path key set. Equivalent keys keep
    /// their Arc; new configured keys are prebuilt; residue is removed.
    pub fn reconcile_keys(&self, retained: &HashSet<AdaptiveTelemetryKey>) {
        let _guard = self.writer.lock();
        let current = self.states.load();
        let next = retained
            .iter()
            .map(|key| {
                let state = current.get(key).cloned().unwrap_or_else(AdaptiveTelemetryState::new);
                (key.clone(), state)
            })
            .collect();
        self.states.store(Arc::new(next));
    }

    #[cfg(test)]
    fn get_or_create(&self, key: AdaptiveTelemetryKey) -> Arc<AdaptiveTelemetryState> {
        let mut keys: HashSet<_> = self.states.load().keys().cloned().collect();
        keys.insert(key.clone());
        self.reconcile_keys(&keys);
        self.get(&key).expect("test key must be reconciled")
    }

    pub fn remove(&self, key: &AdaptiveTelemetryKey) -> bool {
        let _guard = self.writer.lock();
        let mut next = (**self.states.load()).clone();
        let removed = next.remove(key).is_some();
        self.states.store(Arc::new(next));
        removed
    }

    /// Remove every discoverable generation for one typed backend target.
    /// Request plans that already captured a state Arc remain valid.
    pub fn remove_target(&self, target: &PolicyTargetKey) -> usize {
        let _guard = self.writer.lock();
        let current = self.states.load();
        let removed = current.keys().filter(|key| &key.target == target).count();
        let next = current
            .iter()
            .filter(|(key, _)| &key.target != target)
            .map(|(key, state)| (key.clone(), Arc::clone(state)))
            .collect();
        self.states.store(Arc::new(next));
        removed
    }

    /// Cache-residue cleanup: remove global discoverability while request-owned
    /// Arcs naturally remain alive.
    pub fn retain_keys(&self, retained: &HashSet<AdaptiveTelemetryKey>) {
        self.reconcile_keys(retained);
    }

    /// Background publication tick for every currently discoverable state.
    pub fn tick(&self, now_second: u64) {
        let snapshot = self.states.load_full();
        for state in snapshot.values() {
            state.tick(now_second);
        }
    }

    /// Number of adaptive telemetry identities currently discoverable.
    pub fn count(&self) -> usize {
        self.states.load().len()
    }

    /// Adaptive telemetry identities currently tracked (snapshot).
    pub fn keys(&self) -> Vec<String> {
        self.states
            .load()
            .keys()
            .map(|key| {
                format!(
                    "{}/{}/{}/{}",
                    key.terminal_route_uid, key.rule_name, key.target, key.logical_model
                )
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.states.load().is_empty()
    }
}

#[inline]
pub fn current_unix_second() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// One process-wide cold-path publisher. The caller owns the single spawned
/// task and supplies the Gateway auxiliary-backend shutdown receiver.
pub async fn run_adaptive_telemetry_ticker(store: Arc<AdaptiveTelemetryStore>, mut shutdown: watch::Receiver<bool>) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = interval.tick() => store.tick(current_unix_second()),
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
        }
    }
}

#[derive(Clone)]
struct AdaptiveRuleFacts {
    terminal_route_uid: String,
    rule_identity: String,
    plugin_keys: Vec<String>,
    targets: Vec<PolicyTargetKey>,
}

#[derive(Clone, Default)]
struct RouteAdaptiveFacts {
    rules: Vec<AdaptiveRuleFacts>,
    plugin_keys: HashSet<String>,
    targets: HashSet<PolicyTargetKey>,
}

impl RouteAdaptiveFacts {
    fn from_route(route: &HTTPRoute) -> Self {
        let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");
        let mut facts = Self::default();
        let Ok(references) = HttpRouteExtensionRefs::from_route(route) else {
            return facts;
        };
        for (rule_index, rule) in route.spec.rules.as_ref().into_iter().flatten().enumerate() {
            let (Some(route_uid), Some(rule_identity)) = (
                rule.resolved_terminal_route_uid.as_ref(),
                rule.resolved_terminal_rule_identity.as_ref(),
            ) else {
                continue;
            };
            let plugin_keys: Vec<_> = rule
                .filters
                .iter()
                .flatten()
                .enumerate()
                .filter_map(|(filter, _)| references.effective_at(rule_index, None, filter))
                .filter_map(extension_ref_key)
                .collect();
            let targets: Vec<_> = rule
                .backend_refs
                .iter()
                .flatten()
                .filter(|backend| backend.backend_kind() == HTTPBackendRefKind::EdgionBackend)
                .map(|backend| PolicyTargetKey {
                    target_kind: PolicyTargetKind::EdgionBackend,
                    namespace: backend.namespace.as_deref().unwrap_or(route_namespace).to_string(),
                    name: backend.name.clone(),
                })
                .collect();
            if plugin_keys.is_empty() || targets.is_empty() {
                continue;
            }
            facts.plugin_keys.extend(plugin_keys.iter().cloned());
            facts.targets.extend(targets.iter().cloned());
            facts.rules.push(AdaptiveRuleFacts {
                terminal_route_uid: route_uid.clone(),
                rule_identity: rule_identity.clone(),
                plugin_keys,
                targets,
            });
        }
        facts
    }
}

#[derive(Default)]
struct AdaptiveContributionIndex {
    routes: HashMap<String, RouteAdaptiveFacts>,
    plugin_routes: HashMap<String, HashSet<String>>,
    target_routes: HashMap<PolicyTargetKey, HashSet<String>>,
    route_contributions: HashMap<String, HashSet<AdaptiveTelemetryKey>>,
}

impl AdaptiveContributionIndex {
    fn detach_route(&mut self, route_key: &str) {
        let Some(facts) = self.routes.remove(route_key) else {
            self.route_contributions.remove(route_key);
            return;
        };
        for plugin_key in facts.plugin_keys {
            if let Some(routes) = self.plugin_routes.get_mut(&plugin_key) {
                routes.remove(route_key);
                if routes.is_empty() {
                    self.plugin_routes.remove(&plugin_key);
                }
            }
        }
        for target in facts.targets {
            if let Some(routes) = self.target_routes.get_mut(&target) {
                routes.remove(route_key);
                if routes.is_empty() {
                    self.target_routes.remove(&target);
                }
            }
        }
        self.route_contributions.remove(route_key);
    }

    fn attach_route(&mut self, route_key: String, facts: RouteAdaptiveFacts) {
        for plugin_key in &facts.plugin_keys {
            self.plugin_routes
                .entry(plugin_key.clone())
                .or_default()
                .insert(route_key.clone());
        }
        for target in &facts.targets {
            self.target_routes
                .entry(target.clone())
                .or_default()
                .insert(route_key.clone());
        }
        self.routes.insert(route_key, facts);
    }

    fn recompute_route(
        &mut self,
        route_key: &str,
        plugin_uses_adaptive_selection: &mut impl FnMut(&str) -> bool,
        logical_models: &mut impl FnMut(&PolicyTargetKey) -> Option<Vec<String>>,
    ) {
        let Some(facts) = self.routes.get(route_key) else {
            self.route_contributions.remove(route_key);
            return;
        };
        let mut retained = HashSet::new();
        for rule in &facts.rules {
            if !rule
                .plugin_keys
                .iter()
                .any(|plugin_key| plugin_uses_adaptive_selection(plugin_key))
            {
                continue;
            }
            for target in &rule.targets {
                for logical_model in logical_models(target).into_iter().flatten() {
                    if let Some(key) = AdaptiveTelemetryKey::new(
                        rule.terminal_route_uid.clone(),
                        rule.rule_identity.clone(),
                        target.clone(),
                        logical_model,
                    ) {
                        retained.insert(key);
                    }
                }
            }
        }
        if retained.is_empty() {
            self.route_contributions.remove(route_key);
        } else {
            self.route_contributions.insert(route_key.to_string(), retained);
        }
    }

    fn retained_keys(&self) -> HashSet<AdaptiveTelemetryKey> {
        self.route_contributions
            .values()
            .flat_map(|keys| keys.iter().cloned())
            .collect()
    }
}

fn with_global_sources(
    index: &mut AdaptiveContributionIndex,
    route_keys: impl IntoIterator<Item = String>,
) -> HashSet<AdaptiveTelemetryKey> {
    let backends = crate::ai::global_ai_backend_store().snapshot();
    let publication = crate::plugins::publication::global_publisher().snapshot();
    let mut plugin_uses_adaptive = |plugin_key: &str| {
        publication.any_http(plugin_key, |compiled| {
            request_plugins_use_adaptive_selection(compiled.spec.spec.request_plugins.as_deref())
        })
    };
    let mut logical_models = |target: &PolicyTargetKey| {
        let key = format!("{}/{}", target.namespace, target.name);
        backends
            .get_by_key(&key)
            .map(|backend| backend.models.keys().cloned().collect())
    };
    for route_key in route_keys {
        index.recompute_route(&route_key, &mut plugin_uses_adaptive, &mut logical_models);
    }
    index.retained_keys()
}

/// Replace the process-wide route contribution set after an HTTPRoute full sync.
pub(crate) fn replace_adaptive_route_contributions(data: &HashMap<String, Arc<HTTPRoute>>) {
    let mut index = GLOBAL_ADAPTIVE_CONTRIBUTIONS.lock();
    *index = AdaptiveContributionIndex::default();
    for (route_key, route) in data {
        index.attach_route(route_key.clone(), RouteAdaptiveFacts::from_route(route));
    }
    let route_keys: Vec<_> = index.routes.keys().cloned().collect();
    let retained = with_global_sources(&mut index, route_keys);
    GLOBAL_ADAPTIVE_TELEMETRY_STORE.reconcile_keys(&retained);
}

/// Apply an HTTPRoute delta without scanning unrelated cached routes.
pub(crate) fn update_adaptive_route_contributions(
    add_or_update: &HashMap<String, Arc<HTTPRoute>>,
    remove: &HashSet<String>,
) {
    let mut index = GLOBAL_ADAPTIVE_CONTRIBUTIONS.lock();
    for route_key in remove {
        index.detach_route(route_key);
    }
    for (route_key, route) in add_or_update {
        index.detach_route(route_key);
        index.attach_route(route_key.clone(), RouteAdaptiveFacts::from_route(route));
    }
    let route_keys: Vec<_> = add_or_update.keys().cloned().collect();
    let retained = with_global_sources(&mut index, route_keys);
    GLOBAL_ADAPTIVE_TELEMETRY_STORE.reconcile_keys(&retained);
}

/// Re-evaluate routes that reference any changed EdgionPlugins resource.
pub(crate) fn reconcile_adaptive_plugin_keys(plugin_keys: &HashSet<String>) {
    let mut index = GLOBAL_ADAPTIVE_CONTRIBUTIONS.lock();
    let route_keys: HashSet<_> = plugin_keys
        .iter()
        .flat_map(|key| index.plugin_routes.get(key).into_iter().flatten().cloned())
        .collect();
    let retained = with_global_sources(&mut index, route_keys);
    GLOBAL_ADAPTIVE_TELEMETRY_STORE.reconcile_keys(&retained);
}

/// Re-evaluate every indexed route after an EdgionPlugins full sync.
pub(crate) fn reconcile_all_adaptive_plugin_dependents() {
    let mut index = GLOBAL_ADAPTIVE_CONTRIBUTIONS.lock();
    let route_keys: Vec<_> = index.routes.keys().cloned().collect();
    let retained = with_global_sources(&mut index, route_keys);
    GLOBAL_ADAPTIVE_TELEMETRY_STORE.reconcile_keys(&retained);
}

/// Re-evaluate routes that reference any changed EdgionBackend target.
pub(crate) fn reconcile_adaptive_targets(targets: &[PolicyTargetKey]) {
    let mut index = GLOBAL_ADAPTIVE_CONTRIBUTIONS.lock();
    let route_keys: HashSet<_> = targets
        .iter()
        .flat_map(|target| index.target_routes.get(target).into_iter().flatten().cloned())
        .collect();
    let retained = with_global_sources(&mut index, route_keys);
    GLOBAL_ADAPTIVE_TELEMETRY_STORE.reconcile_keys(&retained);
}

fn request_plugins_use_adaptive_selection(entries: Option<&[RequestFilterEntry]>) -> bool {
    entries.into_iter().flatten().filter(|entry| entry.enable).any(|entry| {
        matches!(
            &entry.plugin,
            EdgionPlugin::AiProxy(config)
                if matches!(
                    config.backend_selection.selection_type,
                    AiBackendSelection::LowestLatency | AiBackendSelection::LowestUsage
                )
        )
    })
}

#[cfg(test)]
include!("../../../tests/support/ai/routing/adaptive_test/753_function_item.rs");

#[cfg(test)]
#[path = "../../../tests/unit/ai/routing/adaptive_test.rs"]
mod tests;
