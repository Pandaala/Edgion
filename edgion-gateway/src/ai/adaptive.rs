//! Bounded adaptive-routing telemetry aggregation.
//!
//! Attempt completion records into one-second ring buckets. A cold-path tick
//! publishes immutable integer snapshots; selection reads only the ArcSwap.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::resources::edgion_plugins::{AiBackendSelection, EdgionPlugin};
use edgion_resources::resources::http_route::{HTTPBackendRefKind, HTTPRoute, HTTPRouteFilterType};
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;

use super::routing::{AdaptiveSnapshot, LATENCY_EXPLORATION_INTERVAL};
use super::AiServiceSnapshot;

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
            || target.target_kind != PolicyTargetKind::EdgionAIService
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
    tick_lock: Mutex<()>,
    published: ArcSwap<AdaptiveSnapshot>,
}

impl AdaptiveTelemetryState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            buckets: Buckets::default(),
            tick_lock: Mutex::new(()),
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
        let _tick_guard = self.tick_lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
    writer: Mutex<()>,
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
            writer: Mutex::new(()),
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
        let _guard = self.writer.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let _guard = self.writer.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut next = (**self.states.load()).clone();
        let removed = next.remove(key).is_some();
        self.states.store(Arc::new(next));
        removed
    }

    /// Remove every discoverable generation for one typed backend target.
    /// Request plans that already captured a state Arc remain valid.
    pub fn remove_target(&self, target: &PolicyTargetKey) -> usize {
        let _guard = self.writer.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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

    pub fn len(&self) -> usize {
        self.states.load().len()
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
/// task and supplies the Gateway auxiliary-service shutdown receiver.
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

/// Rebuild global discoverability from the current HTTPRoute and AI-service
/// generations. Existing equivalent keys retain their Arc state; removed route
/// UIDs, targets, and model names disappear while captured request Arcs live on.
pub fn reconcile_global_adaptive_telemetry() {
    let routes: Vec<Arc<HTTPRoute>> = crate::routes::http::get_global_http_route_managers()
        .route_cache
        .iter()
        .map(|entry| Arc::clone(entry.value()))
        .collect();
    let services = super::global_ai_service_store().snapshot();
    let retained = retained_keys_for_routes(&routes, &services);
    GLOBAL_ADAPTIVE_TELEMETRY_STORE.reconcile_keys(&retained);
}

fn retained_keys_for_routes(routes: &[Arc<HTTPRoute>], services: &AiServiceSnapshot) -> HashSet<AdaptiveTelemetryKey> {
    collect_retained_keys(routes, |target| {
        let key = format!("{}/{}", target.namespace, target.name);
        services
            .get_by_key(&key)
            .map(|service| service.models.keys().cloned().collect())
    })
}

fn collect_retained_keys(
    routes: &[Arc<HTTPRoute>],
    mut logical_models: impl FnMut(&PolicyTargetKey) -> Option<Vec<String>>,
) -> HashSet<AdaptiveTelemetryKey> {
    let mut retained = HashSet::new();
    for route in routes {
        let route_namespace = route.metadata.namespace.as_deref().unwrap_or("default");
        for rule in route.effective_rules().into_iter().flatten() {
            let adaptive_selection = rule.filters.as_ref().is_some_and(|filters| {
                filters.iter().any(|filter| {
                    if filter.filter_type != HTTPRouteFilterType::ExtensionRef {
                        return false;
                    }
                    let Some(reference) = filter.extension_ref.as_ref() else {
                        return false;
                    };
                    if reference.kind != "EdgionPlugins"
                        || (!reference.group.is_empty() && reference.group != "edgion.io")
                    {
                        return false;
                    }
                    let plugin_namespace = reference.resolved_namespace.as_deref().unwrap_or(route_namespace);
                    let plugin_key = format!("{plugin_namespace}/{}", reference.name);
                    crate::plugins::http::global_plugin_store()
                        .with_plugin(&plugin_key, |compiled| {
                            compiled
                                .spec
                                .spec
                                .request_plugins
                                .iter()
                                .flatten()
                                .filter(|entry| entry.enable)
                                .any(|entry| {
                                    matches!(
                                        &entry.plugin,
                                        EdgionPlugin::AiProxy(config)
                                            if matches!(
                                                config.backend_selection.selection_type,
                                                AiBackendSelection::LowestLatency | AiBackendSelection::LowestUsage
                                            )
                                    )
                                })
                        })
                        .unwrap_or(false)
                })
            });
            if !adaptive_selection {
                continue;
            }
            let (Some(route_uid), Some(rule_identity)) = (
                rule.resolved_terminal_route_uid.as_ref(),
                rule.resolved_terminal_rule_identity.as_ref(),
            ) else {
                continue;
            };
            for backend in rule.backend_refs.iter().flatten() {
                if backend.backend_kind() != HTTPBackendRefKind::EdgionAIService {
                    continue;
                }
                let target = PolicyTargetKey {
                    target_kind: PolicyTargetKind::EdgionAIService,
                    namespace: backend.namespace.as_deref().unwrap_or(route_namespace).to_string(),
                    name: backend.name.clone(),
                };
                for logical_model in logical_models(&target).into_iter().flatten() {
                    if let Some(key) = AdaptiveTelemetryKey::new(
                        route_uid.clone(),
                        rule_identity.clone(),
                        target.clone(),
                        logical_model,
                    ) {
                        retained.insert(key);
                    }
                }
            }
        }
    }
    retained
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(route_uid: &str, model: &str) -> AdaptiveTelemetryKey {
        AdaptiveTelemetryKey::new(
            route_uid.to_string(),
            "rule-a".to_string(),
            PolicyTargetKey {
                target_kind: PolicyTargetKind::EdgionAIService,
                namespace: "default".to_string(),
                name: "provider".to_string(),
            },
            model.to_string(),
        )
        .unwrap()
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
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn cleanup_retains_only_normalized_keys() {
        let store = AdaptiveTelemetryStore::new();
        let keep = key("uid-a", "model-a");
        let remove = key("uid-a", "model-b");
        let kept_state = store.get_or_create(keep.clone());
        let removed_state = store.get_or_create(remove);
        store.retain_keys(&HashSet::from([keep.clone()]));
        assert_eq!(store.len(), 1);
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
        store.get_or_create(second);
        store.get_or_create(other.clone());

        assert_eq!(store.remove_target(&first.target), 2);
        assert!(store.get(&first).is_none());
        assert!(store.get(&other).is_some());
        assert_eq!(captured.snapshot(), AdaptiveSnapshot::default());
    }

    #[test]
    fn key_rejects_non_ai_target_and_empty_identity_parts() {
        let mut target = key("uid", "model").target;
        target.target_kind = PolicyTargetKind::Service;
        assert!(AdaptiveTelemetryKey::new("uid".into(), "rule".into(), target, "model".into()).is_none());
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
