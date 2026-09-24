//! Bounded adaptive-routing telemetry aggregation.
//!
//! Attempt completion records into one-second ring buckets. A cold-path tick
//! publishes immutable integer snapshots; selection reads only the ArcSwap.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::resources::edgion_plugins::{AiBackendSelection, EdgionPlugin, RequestFilterEntry};
use edgion_resources::resources::extension_ref::is_supported_edgion_plugins_ref;
use edgion_resources::resources::http_route::{HTTPBackendRefKind, HTTPRoute, HTTPRouteFilterType};
use parking_lot::Mutex as ParkingMutex;
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;

use super::routing::{AdaptiveSnapshot, LATENCY_EXPLORATION_INTERVAL};
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

pub fn get_adaptive_telemetry_store() -> Arc<AdaptiveTelemetryStore> {
    Arc::clone(&GLOBAL_ADAPTIVE_TELEMETRY_STORE)
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
    charged_tokens: AtomicU64,
}

impl SecondBucket {
    fn new() -> Self {
        Self {
            second: AtomicU64::new(ROTATING_EPOCH),
            active_writers: AtomicU32::new(0),
            ttft_histogram: std::array::from_fn(|_| AtomicU32::new(0)),
            successful_ttft_count: AtomicU32::new(0),
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
}

impl AdaptiveTelemetryState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            buckets: Buckets::default(),
            tick_lock: ParkingMutex::new(()),
            published: ArcSwap::from_pointee(AdaptiveSnapshot::default()),
        })
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

    /// Lock-free request-path snapshot read. No sorting or floating point occurs.
    #[inline]
    pub fn snapshot(&self) -> AdaptiveSnapshot {
        **self.published.load()
    }

    /// Publish windows ending immediately before `now_second`; the current
    /// in-progress second is never included.
    pub fn tick(&self, now_second: u64) {
        let _tick_guard = self.tick_lock.lock();
        let latency_start = now_second.saturating_sub(LATENCY_WINDOW_SECONDS);
        let usage_start = now_second.saturating_sub(USAGE_WINDOW_SECONDS);
        let mut histogram = [0u64; TTFT_HISTOGRAM_BUCKETS];
        let mut latency_samples = 0u32;
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
                for (aggregate, count) in histogram.iter_mut().zip(&bucket.ttft_histogram) {
                    *aggregate = aggregate.saturating_add(u64::from(count.load(Ordering::Relaxed)));
                }
            }
        }
        let latency_p50_micros = histogram_p50(&histogram);
        self.published.store(Arc::new(AdaptiveSnapshot {
            latency_p50_micros,
            latency_samples,
            charged_tokens_60s: charged_tokens,
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
    selection_sequence: AtomicU8,
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
            selection_sequence: AtomicU8::new(0),
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

    /// Return a request-independent sequence in `[0, 20)`. The bounded CAS
    /// avoids overflow behavior and lets LowestLatency explore exactly every
    /// twentieth plan without an RNG or mutex.
    pub fn next_selection_sequence(&self) -> u8 {
        let mut current = self.selection_sequence.load(Ordering::Relaxed);
        loop {
            let next = if current + 1 == LATENCY_EXPLORATION_INTERVAL {
                0
            } else {
                current + 1
            };
            match self
                .selection_sequence
                .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return current,
                Err(actual) => current = actual,
            }
        }
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
        for rule in route.effective_rules().into_iter().flatten() {
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
                .filter_map(|filter| {
                    if filter.filter_type != HTTPRouteFilterType::ExtensionRef {
                        return None;
                    }
                    let reference = filter.extension_ref.as_ref()?;
                    if !is_supported_edgion_plugins_ref(reference) {
                        return None;
                    }
                    let namespace = reference.resolved_namespace.as_deref().unwrap_or(route_namespace);
                    Some(format!("{namespace}/{}", reference.name))
                })
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
    let backends = super::global_ai_backend_store().snapshot();
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
fn collect_retained_keys(
    routes: &[Arc<HTTPRoute>],
    mut plugin_uses_adaptive_selection: impl FnMut(&str) -> bool,
    mut logical_models: impl FnMut(&PolicyTargetKey) -> Option<Vec<String>>,
) -> HashSet<AdaptiveTelemetryKey> {
    let mut index = AdaptiveContributionIndex::default();
    for (position, route) in routes.iter().enumerate() {
        let route_key = position.to_string();
        index.attach_route(route_key.clone(), RouteAdaptiveFacts::from_route(route));
        index.recompute_route(&route_key, &mut plugin_uses_adaptive_selection, &mut logical_models);
    }
    index.retained_keys()
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::edgion_plugins::{AiBackendSelectionConfig, AiProxyConfig, OnUnknownModel};
    use edgion_resources::resources::http_route::HTTPRouteFilterType;

    fn key(route_uid: &str, model: &str) -> AdaptiveTelemetryKey {
        AdaptiveTelemetryKey::new(
            route_uid.to_string(),
            "rule-a".to_string(),
            PolicyTargetKey {
                target_kind: PolicyTargetKind::EdgionBackend,
                namespace: "default".to_string(),
                name: "provider".to_string(),
            },
            model.to_string(),
        )
        .unwrap()
    }

    fn adaptive_entry(selection_type: AiBackendSelection, enable: bool) -> RequestFilterEntry {
        RequestFilterEntry::with_enable(
            EdgionPlugin::AiProxy(AiProxyConfig {
                backend_selection: AiBackendSelectionConfig { selection_type },
                inject_usage_tracking: true,
                on_unknown_model: OnUnknownModel::Reject,
                model_routes: Vec::new(),
                token_quota: None,
            }),
            enable,
        )
    }

    fn adaptive_route() -> HTTPRoute {
        serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {
                "name": "adaptive-route",
                "namespace": "route-ns"
            },
            "spec": {
                "rules": [{
                    "name": "primary",
                    "filters": [{
                        "type": "ExtensionRef",
                        "extensionRef": {
                            "group": "edgion.io",
                            "kind": "EdgionPlugins",
                            "name": "adaptive"
                        }
                    }],
                    "backendRefs": [{
                        "group": "edgion.io",
                        "kind": "EdgionBackend",
                        "name": "provider"
                    }],
                    "resolvedTerminalRouteUid": "route-uid",
                    "resolvedTerminalRuleIdentity": "name:primary"
                }]
            }
        }))
        .expect("adaptive route fixture must deserialize")
    }

    fn collect_for_test(route: HTTPRoute, adaptive_plugin_key: &str) -> HashSet<AdaptiveTelemetryKey> {
        collect_retained_keys(
            &[Arc::new(route)],
            |plugin_key| plugin_key == adaptive_plugin_key,
            |_| Some(vec!["model-a".to_string(), "model-b".to_string()]),
        )
    }

    fn route_with_dependencies(route_uid: &str, plugin_name: &str, target_name: &str) -> HTTPRoute {
        let mut route = adaptive_route();
        let rule = &mut route.spec.rules.as_mut().unwrap()[0];
        rule.resolved_terminal_route_uid = Some(route_uid.to_string());
        rule.filters.as_mut().unwrap()[0].extension_ref.as_mut().unwrap().name = plugin_name.to_string();
        rule.backend_refs.as_mut().unwrap()[0].name = target_name.to_string();
        route
    }

    fn recompute_test_routes(
        index: &mut AdaptiveContributionIndex,
        route_keys: impl IntoIterator<Item = String>,
        adaptive_plugins: &HashSet<String>,
        models: &HashMap<PolicyTargetKey, Vec<String>>,
    ) {
        let mut plugin_uses_adaptive = |key: &str| adaptive_plugins.contains(key);
        let mut logical_models = |target: &PolicyTargetKey| models.get(target).cloned();
        for route_key in route_keys {
            index.recompute_route(&route_key, &mut plugin_uses_adaptive, &mut logical_models);
        }
    }

    #[test]
    fn current_bucket_is_excluded_until_next_tick_boundary() {
        let state = AdaptiveTelemetryState::new();
        state.tick(100);
        state.record_success_ttft(100, 100);
        state.record_charged_tokens(100, 7);
        state.tick(100);
        assert_eq!(state.snapshot(), AdaptiveSnapshot::default());

        state.tick(101);
        assert_eq!(
            state.snapshot(),
            AdaptiveSnapshot {
                latency_p50_micros: Some(100),
                latency_samples: 1,
                charged_tokens_60s: 7,
            }
        );
    }

    #[test]
    fn p50_uses_successes_only_and_failure_charge_still_counts() {
        let state = AdaptiveTelemetryState::new();
        state.tick(10);
        for value in [1_000, 100, 1_500, 250, 500] {
            state.record_success_ttft(10, value);
        }
        // A failed attempt records only its charged-token accounting.
        state.record_charged_tokens(10, 11);
        state.tick(11);
        assert_eq!(state.snapshot().latency_p50_micros, Some(500));
        assert_eq!(state.snapshot().latency_samples, 5);
        assert_eq!(state.snapshot().charged_tokens_60s, 11);
    }

    #[test]
    fn latency_and_usage_windows_include_only_complete_buckets() {
        let state = AdaptiveTelemetryState::new();
        state.tick(10);
        state.record_charged_tokens(10, 1);
        state.tick(40);
        state.record_success_ttft(40, 100);
        state.record_charged_tokens(40, 4);
        state.tick(69);
        state.record_success_ttft(69, 250);
        state.record_charged_tokens(69, 9);

        state.tick(70);
        let at_70 = state.snapshot();
        assert_eq!(at_70.latency_samples, 2); // seconds 40 and 69
        assert_eq!(at_70.latency_p50_micros, Some(100));
        assert_eq!(at_70.charged_tokens_60s, 14); // seconds 10, 40, and 69

        state.record_success_ttft(70, 500);
        state.record_charged_tokens(70, 10);
        state.tick(71);
        let at_71 = state.snapshot();
        assert_eq!(at_71.latency_samples, 2); // seconds 69 and 70
        assert_eq!(at_71.latency_p50_micros, Some(250));
        assert_eq!(at_71.charged_tokens_60s, 23); // second 10 aged out
    }

    #[test]
    fn token_arithmetic_saturates_and_latency_count_is_bounded() {
        let state = AdaptiveTelemetryState::new();
        state.tick(1);
        state.record_charged_tokens(1, u64::MAX);
        state.record_charged_tokens(1, 1);
        state.tick(2);
        assert_eq!(state.snapshot().charged_tokens_60s, u64::MAX);
    }

    #[test]
    fn equivalent_key_reuses_state_remove_hides_but_old_arc_survives() {
        let store = AdaptiveTelemetryStore::new();
        let key = key("uid-a", "model-a");
        let first = store.get_or_create(key.clone());
        let equivalent = store.get_or_create(key.clone());
        assert!(Arc::ptr_eq(&first, &equivalent));

        first.tick(1);
        first.record_success_ttft(1, 100);
        assert!(store.remove(&key));
        assert!(store.get(&key).is_none());
        first.tick(2);
        assert_eq!(first.snapshot().latency_p50_micros, Some(100));

        let recreated = store.get_or_create(key);
        assert!(!Arc::ptr_eq(&first, &recreated));
        assert_eq!(recreated.snapshot(), AdaptiveSnapshot::default());
    }

    #[test]
    fn count_and_keys_track_telemetry_lifecycle() {
        let store = AdaptiveTelemetryStore::new();
        assert_eq!(store.count(), 0);
        assert!(store.keys().is_empty());

        let key = key("uid-a", "model-a");
        store.reconcile_keys(&HashSet::from([key.clone()]));
        assert_eq!(store.count(), 1);
        assert_eq!(
            store.keys(),
            vec![format!(
                "{}/{}/{}/{}",
                key.terminal_route_uid, key.rule_name, key.target, key.logical_model
            )]
        );

        assert!(store.remove(&key));
        assert_eq!(store.count(), 0);
        assert!(store.keys().is_empty());
    }

    #[test]
    fn concurrent_cold_reconcile_reuses_one_published_arc() {
        let store = Arc::new(AdaptiveTelemetryStore::new());
        let key = key("uid-a", "model-a");
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let threads: Vec<_> = (0..16)
            .map(|_| {
                let store = Arc::clone(&store);
                let key = key.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    store.reconcile_keys(&HashSet::from([key.clone()]));
                    store.get(&key).unwrap()
                })
            })
            .collect();
        let states: Vec<_> = threads.into_iter().map(|thread| thread.join().unwrap()).collect();
        let published = store.get(&key).expect("same key must be published");
        assert!(states.iter().all(|state| Arc::ptr_eq(state, &published)));
        assert_eq!(store.count(), 1);
    }

    #[test]
    fn cleanup_retains_only_normalized_keys() {
        let store = AdaptiveTelemetryStore::new();
        let keep = key("uid-a", "model-a");
        let remove = key("uid-a", "model-b");
        let kept_state = store.get_or_create(keep.clone());
        let removed_state = store.get_or_create(remove);
        store.retain_keys(&HashSet::from([keep.clone()]));
        assert_eq!(store.count(), 1);
        assert!(Arc::ptr_eq(&kept_state, &store.get(&keep).unwrap()));
        removed_state.tick(1);
        removed_state.record_charged_tokens(1, 3);
        removed_state.tick(2);
        assert_eq!(removed_state.snapshot().charged_tokens_60s, 3);
    }

    #[test]
    fn reconcile_prebuilds_state_and_request_lookup_never_mutates_snapshot() {
        let store = AdaptiveTelemetryStore::new();
        let key = key("uid-a", "model-a");
        store.reconcile_keys(&HashSet::from([key.clone()]));
        let before = store.states.load_full();
        let state = store.get(&key).expect("cold reconcile must prebuild state");
        for _ in 0..100 {
            assert!(Arc::ptr_eq(&state, &store.get(&key).unwrap()));
        }
        let after = store.states.load_full();

        assert!(Arc::ptr_eq(&before, &after));
    }

    #[test]
    fn target_cleanup_removes_all_models_and_preserves_other_targets() {
        let store = AdaptiveTelemetryStore::new();
        let first = key("route-a", "model-a");
        let second = key("route-b", "model-b");
        let mut other = key("route-c", "model-c");
        other.target.name = "other-provider".to_string();
        let captured = store.get_or_create(first.clone());
        store.get_or_create(second.clone());
        store.get_or_create(other.clone());

        captured.tick(10);
        captured.record_success_ttft(10, 100);
        captured.record_charged_tokens(10, 3);
        captured.tick(11);

        assert_eq!(store.remove_target(&first.target), 2);
        assert!(store.get(&first).is_none());
        assert!(store.get(&second).is_none());
        assert!(store.get(&other).is_some());
        assert_eq!(
            captured.snapshot(),
            AdaptiveSnapshot {
                latency_p50_micros: Some(100),
                latency_samples: 1,
                charged_tokens_60s: 3,
            }
        );

        captured.record_success_ttft(11, 250);
        captured.record_charged_tokens(11, 4);
        captured.tick(12);
        assert_eq!(captured.snapshot().latency_samples, 2);
        assert_eq!(captured.snapshot().charged_tokens_60s, 7);
    }

    #[test]
    fn key_rejects_non_ai_target_and_empty_identity_parts() {
        let target = key("uid", "model").target;
        assert!(AdaptiveTelemetryKey::new("".into(), "rule".into(), target.clone(), "model".into()).is_none());
        assert!(AdaptiveTelemetryKey::new("uid".into(), "".into(), target.clone(), "model".into()).is_none());
        assert!(AdaptiveTelemetryKey::new("uid".into(), "rule".into(), target.clone(), "".into()).is_none());

        let mut target = target;
        target.target_kind = PolicyTargetKind::Service;
        assert!(AdaptiveTelemetryKey::new("uid".into(), "rule".into(), target, "model".into()).is_none());
    }

    #[test]
    fn adaptive_plugin_predicate_requires_enabled_adaptive_ai_proxy() {
        assert!(!request_plugins_use_adaptive_selection(None));
        assert!(!request_plugins_use_adaptive_selection(Some(&[adaptive_entry(
            AiBackendSelection::LowestLatency,
            false,
        )])));
        for selection_type in [
            AiBackendSelection::Weighted,
            AiBackendSelection::Priority,
            AiBackendSelection::CostWeighted,
        ] {
            assert!(!request_plugins_use_adaptive_selection(Some(&[adaptive_entry(
                selection_type,
                true,
            )])));
        }
        for selection_type in [AiBackendSelection::LowestLatency, AiBackendSelection::LowestUsage] {
            assert!(request_plugins_use_adaptive_selection(Some(&[adaptive_entry(
                selection_type,
                true,
            )])));
        }
    }

    #[test]
    fn retained_keys_match_route_plugin_target_and_catalog_models() {
        let retained = collect_for_test(adaptive_route(), "route-ns/adaptive");
        let target = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "route-ns".to_string(),
            name: "provider".to_string(),
        };
        let expected: HashSet<_> = ["model-a", "model-b"]
            .into_iter()
            .map(|model| {
                AdaptiveTelemetryKey::new(
                    "route-uid".to_string(),
                    "name:primary".to_string(),
                    target.clone(),
                    model.to_string(),
                )
                .unwrap()
            })
            .collect();
        assert_eq!(retained, expected);

        let mut delegated = adaptive_route();
        delegated.spec.rules.as_mut().unwrap()[0].filters.as_mut().unwrap()[0]
            .extension_ref
            .as_mut()
            .unwrap()
            .resolved_namespace = Some("child-ns".to_string());
        assert_eq!(collect_for_test(delegated, "child-ns/adaptive"), expected);

        let mut cross_namespace = adaptive_route();
        cross_namespace.spec.rules.as_mut().unwrap()[0]
            .backend_refs
            .as_mut()
            .unwrap()[0]
            .namespace = Some("provider-ns".to_string());
        let explicit_target = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "provider-ns".to_string(),
            name: "provider".to_string(),
        };
        let explicit_retained = collect_retained_keys(
            &[Arc::new(cross_namespace)],
            |plugin_key| plugin_key == "route-ns/adaptive",
            |target| {
                assert_eq!(target, &explicit_target);
                Some(vec!["model-a".to_string()])
            },
        );
        assert_eq!(
            explicit_retained,
            HashSet::from([AdaptiveTelemetryKey::new(
                "route-uid".to_string(),
                "name:primary".to_string(),
                explicit_target,
                "model-a".to_string(),
            )
            .unwrap(),])
        );
    }

    #[test]
    fn retained_keys_fail_closed_for_ineligible_route_shapes() {
        let mut cases = Vec::new();

        let mut non_extension = adaptive_route();
        non_extension.spec.rules.as_mut().unwrap()[0].filters.as_mut().unwrap()[0].filter_type =
            HTTPRouteFilterType::RequestHeaderModifier;
        cases.push(non_extension);

        let mut wrong_group = adaptive_route();
        wrong_group.spec.rules.as_mut().unwrap()[0].filters.as_mut().unwrap()[0]
            .extension_ref
            .as_mut()
            .unwrap()
            .group = "example.io".to_string();
        cases.push(wrong_group);

        let mut wrong_kind = adaptive_route();
        wrong_kind.spec.rules.as_mut().unwrap()[0].filters.as_mut().unwrap()[0]
            .extension_ref
            .as_mut()
            .unwrap()
            .kind = "OtherPlugins".to_string();
        cases.push(wrong_kind);

        let mut unresolved_route = adaptive_route();
        unresolved_route.spec.rules.as_mut().unwrap()[0].resolved_terminal_route_uid = None;
        cases.push(unresolved_route);

        let mut non_ai_backend = adaptive_route();
        let backend = &mut non_ai_backend.spec.rules.as_mut().unwrap()[0]
            .backend_refs
            .as_mut()
            .unwrap()[0];
        backend.group = None;
        backend.kind = Some("Service".to_string());
        cases.push(non_ai_backend);

        for route in cases {
            assert!(collect_for_test(route, "route-ns/adaptive").is_empty());
        }

        assert!(collect_for_test(adaptive_route(), "route-ns/other-plugin").is_empty());
        assert!(collect_retained_keys(
            &[Arc::new(adaptive_route())],
            |plugin_key| plugin_key == "route-ns/adaptive",
            |_| None,
        )
        .is_empty());
    }

    #[test]
    fn contribution_index_self_heals_when_route_arrives_before_dependencies() {
        let route = route_with_dependencies("uid-a", "adaptive", "provider");
        let facts = RouteAdaptiveFacts::from_route(&route);
        let target = facts.targets.iter().next().unwrap().clone();
        let mut index = AdaptiveContributionIndex::default();
        index.attach_route("route-a".to_string(), facts);
        for position in 0..64 {
            index.attach_route(format!("ordinary-{position}"), RouteAdaptiveFacts::default());
        }

        let mut adaptive_plugins = HashSet::new();
        let mut models = HashMap::new();
        let all_routes: Vec<_> = index.routes.keys().cloned().collect();
        recompute_test_routes(&mut index, all_routes, &adaptive_plugins, &models);
        assert!(index.retained_keys().is_empty());
        assert!(index.route_contributions.is_empty());
        assert_eq!(
            index.plugin_routes["route-ns/adaptive"],
            HashSet::from(["route-a".to_string()])
        );
        assert_eq!(index.target_routes[&target], HashSet::from(["route-a".to_string()]));

        adaptive_plugins.insert("route-ns/adaptive".to_string());
        let plugin_routes = index.plugin_routes["route-ns/adaptive"].clone();
        recompute_test_routes(&mut index, plugin_routes, &adaptive_plugins, &models);
        assert!(index.retained_keys().is_empty());
        assert!(index.route_contributions.is_empty());

        models.insert(target, vec!["model-a".to_string(), "model-b".to_string()]);
        recompute_test_routes(&mut index, ["route-a".to_string()], &adaptive_plugins, &models);
        assert_eq!(index.retained_keys().len(), 2);
    }

    #[test]
    fn contribution_index_updates_only_changed_route_and_removes_stale_facts() {
        let route_a = route_with_dependencies("uid-a", "adaptive-a", "provider-a");
        let route_b = route_with_dependencies("uid-b", "adaptive-b", "provider-b");
        let facts_a = RouteAdaptiveFacts::from_route(&route_a);
        let facts_b = RouteAdaptiveFacts::from_route(&route_b);
        let models: HashMap<_, _> = facts_a
            .targets
            .iter()
            .chain(facts_b.targets.iter())
            .cloned()
            .map(|target| (target, vec!["model".to_string()]))
            .collect();
        let adaptive_plugins = HashSet::from(["route-ns/adaptive-a".to_string(), "route-ns/adaptive-b".to_string()]);
        let mut index = AdaptiveContributionIndex::default();
        index.attach_route("route-a".to_string(), facts_a);
        index.attach_route("route-b".to_string(), facts_b);
        recompute_test_routes(
            &mut index,
            ["route-a".to_string(), "route-b".to_string()],
            &adaptive_plugins,
            &models,
        );
        let route_b_keys = index.route_contributions["route-b"].clone();

        index.detach_route("route-a");
        index.attach_route(
            "route-a".to_string(),
            RouteAdaptiveFacts::from_route(&route_with_dependencies("uid-a-next", "adaptive-a", "provider-a")),
        );
        recompute_test_routes(&mut index, ["route-a".to_string()], &adaptive_plugins, &models);
        assert_eq!(index.route_contributions["route-b"], route_b_keys);
        assert!(index
            .retained_keys()
            .iter()
            .any(|key| key.terminal_route_uid == "uid-a-next"));
        assert!(!index
            .retained_keys()
            .iter()
            .any(|key| key.terminal_route_uid == "uid-a"));

        index.detach_route("route-a");
        assert_eq!(index.retained_keys(), route_b_keys);
        assert!(!index.plugin_routes.contains_key("route-ns/adaptive-a"));
    }

    #[test]
    fn duplicate_route_contributions_survive_until_last_source_is_removed() {
        let facts = RouteAdaptiveFacts::from_route(&adaptive_route());
        let target = facts.targets.iter().next().unwrap().clone();
        let adaptive_plugins = HashSet::from(["route-ns/adaptive".to_string()]);
        let models = HashMap::from([(target, vec!["model-a".to_string()])]);
        let mut index = AdaptiveContributionIndex::default();
        index.attach_route("parent-a".to_string(), facts.clone());
        index.attach_route("parent-b".to_string(), facts);
        recompute_test_routes(
            &mut index,
            ["parent-a".to_string(), "parent-b".to_string()],
            &adaptive_plugins,
            &models,
        );
        assert_eq!(index.retained_keys().len(), 1);

        index.detach_route("parent-a");
        assert_eq!(index.retained_keys().len(), 1);
        index.detach_route("parent-b");
        assert!(index.retained_keys().is_empty());
    }

    #[test]
    fn selection_sequence_is_bounded_and_explores_every_twentieth_plan() {
        let store = AdaptiveTelemetryStore::new();
        let sequence: Vec<_> = (0..21).map(|_| store.next_selection_sequence()).collect();
        assert_eq!(sequence, (0..20).chain(std::iter::once(0)).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn global_ticker_stops_when_auxiliary_shutdown_is_signaled() {
        let store = Arc::new(AdaptiveTelemetryStore::new());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(run_adaptive_telemetry_ticker(store, shutdown_rx));
        shutdown_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("ticker must honor shutdown")
            .expect("ticker task must not panic");
    }

    #[test]
    fn concurrent_recording_is_atomic_and_boundary_rotation_isolated() {
        let state = AdaptiveTelemetryState::new();
        state.tick(5);
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let state = Arc::clone(&state);
                std::thread::spawn(move || {
                    for _ in 0..1_000 {
                        state.record_success_ttft(5, 100);
                        state.record_charged_tokens(5, 2);
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        state.tick(6);
        assert_eq!(state.snapshot().latency_samples, 8_000);
        assert_eq!(state.snapshot().latency_p50_micros, Some(100));
        assert_eq!(state.snapshot().charged_tokens_60s, 16_000);

        // A late write to the complete second cannot contaminate the new slot.
        state.record_success_ttft(5, 1);
        state.record_charged_tokens(5, 1);
        state.tick(7);
        assert_eq!(state.snapshot().latency_samples, 8_000);
        assert_eq!(state.snapshot().charged_tokens_60s, 16_000);
    }
}
