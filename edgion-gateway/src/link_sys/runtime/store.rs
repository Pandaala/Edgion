//! Global store for LinkSys resources.
//!
//! Manages all LinkSys resources (Webhook, Redis, Etcd, etc.) and dispatches
//! configuration changes to the appropriate sub-module managers.
//! Follows the same ArcSwap pattern as PluginStore.
//!
//! Redis, Etcd, Elasticsearch, Kafka, and OTLP persistent runners live in
//! their provider runtime stores.
//! Callers use `get_*_client("namespace/name")` for the published small handle (D14).

use arc_swap::ArcSwap;
use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};

use edgion_resources::resources::link_sys::LinkSys;

use crate::link_sys::credential;
use crate::link_sys::providers::elasticsearch::{EsApply, EsLinkClient};
use crate::link_sys::providers::etcd::{EtcdApply, EtcdLinkClient};
use crate::link_sys::providers::kafka::{KafkaApply, KafkaLinkClient};
use crate::link_sys::providers::otlp::{OtlpApply, OtlpLinkClient};
use crate::link_sys::providers::redis::{RedisApply, RedisLinkClient};
use crate::link_sys::providers::webhook::{
    fingerprint_webhook, get_webhook_manager, WebhookLinkClient, WebhookManager,
};
use crate::link_sys::runtime::apply::RuntimeFingerprint;
use crate::link_sys::runtime::runner::{ApplyOwner, PersistentRunner};
use crate::link_sys::LinkSysHealth;
use crate::observe::metrics::registry::record_link_sys_health;

// ============================================================
// Type aliases
// ============================================================

type LinkSysMap = HashMap<String, LinkSys>;

static LINK_SYS_SOURCE_EVENT_LOCK: Mutex<()> = Mutex::new(());
static NEXT_LINK_SYS_DISPATCH_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static LINK_SYS_DISPATCH_TAIL: Mutex<Option<tokio::task::JoinHandle<()>>> = Mutex::new(None);

/// Reserve async admission order while the caller holds the source-event lock.
/// The Webhook mutation guard already serializes generic dispatch, but its stale
/// fence must not discard an earlier delta just because a later task polls first.
/// One tail owns the pending task chain; completed predecessors are released.
fn spawn_ordered_dispatch(
    handle: &tokio::runtime::Handle,
    tail: &Mutex<Option<tokio::task::JoinHandle<()>>>,
    dispatch: impl std::future::Future<Output = ()> + Send + 'static,
) {
    let mut tail = tail.lock();
    let previous = tail.take();
    *tail = Some(handle.spawn(dispatch_after(previous, dispatch)));
}

async fn dispatch_after(
    previous: Option<tokio::task::JoinHandle<()>>,
    dispatch: impl std::future::Future<Output = ()>,
) {
    if let Some(previous) = previous {
        if previous.await.is_err() {
            tracing::warn!(reason = "dispatch_failed", "Previous LinkSys dispatch did not complete");
        }
    }
    dispatch.await;
}

fn reserve_dispatch_sequence(counter: &AtomicU64) -> Option<u64> {
    counter
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| current.checked_add(1))
        .ok()
        .and_then(|previous| previous.checked_add(1))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DispatchSequenceExhausted;

// ============================================================
// LinkSysStore
// ============================================================

pub struct LinkSysStore {
    resources: ArcSwap<LinkSysMap>,
}

impl Default for LinkSysStore {
    fn default() -> Self {
        Self::new()
    }
}

impl LinkSysStore {
    pub fn new() -> Self {
        Self {
            resources: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Get a LinkSys resource by key (namespace/name)
    pub fn get(&self, key: &str) -> Option<LinkSys> {
        let map = self.resources.load();
        map.get(key).cloned()
    }

    /// Check if a LinkSys resource exists
    pub fn contains(&self, key: &str) -> bool {
        let map = self.resources.load();
        map.contains_key(key)
    }

    /// Get total count of LinkSys resources
    pub fn count(&self) -> usize {
        let map = self.resources.load();
        map.len()
    }

    /// Replace all LinkSys resources atomically (full sync).
    ///
    /// Dispatches each resource to the appropriate sub-module manager
    /// based on its SystemConfig type.
    pub fn replace_all(&self, data: HashMap<String, LinkSys>) {
        if self
            .replace_all_with_dispatch(
                data,
                &NEXT_LINK_SYS_DISPATCH_SEQUENCE,
                &LINK_SYS_SOURCE_EVENT_LOCK,
                |sequence, dispatch_data| {
                    if let Ok(handle) = tokio::runtime::Handle::try_current() {
                        spawn_ordered_dispatch(&handle, &LINK_SYS_DISPATCH_TAIL, async move {
                            dispatch_full_set(sequence, &dispatch_data, false).await;
                        });
                    } else {
                        // Reachable only outside a tokio runtime (currently: the four sync
                        // unit tests that exercise the resource map only). Production
                        // callers all run inside tokio; if this fires there, provider
                        // clients were never built and `get_*_client` will look like
                        // "config not synced yet" — surface it instead of failing silently.
                        tracing::error!("LinkSysStore::replace_all called outside a tokio runtime — dispatch skipped");
                    }
                },
            )
            .is_err()
        {
            tracing::error!("LinkSys dispatch sequence exhausted; full set rejected");
        }
    }

    /// Update LinkSys resources atomically (incremental sync).
    ///
    /// Dispatches changes to the appropriate sub-module managers.
    pub fn update(&self, add_or_update: HashMap<String, LinkSys>, remove: &HashSet<String>) {
        if self
            .update_with_dispatch(
                add_or_update,
                remove,
                &NEXT_LINK_SYS_DISPATCH_SEQUENCE,
                &LINK_SYS_SOURCE_EVENT_LOCK,
                |sequence, dispatch_data, remove_set| {
                    if let Ok(handle) = tokio::runtime::Handle::try_current() {
                        spawn_ordered_dispatch(&handle, &LINK_SYS_DISPATCH_TAIL, async move {
                            dispatch_partial_update(sequence, &dispatch_data, &remove_set).await;
                        });
                    } else {
                        // See `replace_all` for the rationale — this branch is only hit by
                        // sync unit tests today; logging makes any future outside-runtime
                        // regression visible rather than presenting as "config not synced".
                        tracing::error!("LinkSysStore::update called outside a tokio runtime — dispatch skipped");
                    }
                },
            )
            .is_err()
        {
            tracing::error!("LinkSys dispatch sequence exhausted; partial update rejected");
        }
    }

    /// Rebuild every runtime client from the current authoritative snapshot.
    ///
    /// LinkSys clients resolve inherited process-wide settings when they are
    /// constructed. A change to one of those settings therefore needs a fresh
    /// dispatch even though the LinkSys resources themselves did not change.
    pub fn rebuild_clients(&self) {
        if self
            .rebuild_clients_with_dispatch(
                &NEXT_LINK_SYS_DISPATCH_SEQUENCE,
                &LINK_SYS_SOURCE_EVENT_LOCK,
                |sequence, dispatch_data| {
                    if let Ok(handle) = tokio::runtime::Handle::try_current() {
                        spawn_ordered_dispatch(&handle, &LINK_SYS_DISPATCH_TAIL, async move {
                            dispatch_full_set(sequence, &dispatch_data, true).await;
                        });
                    } else {
                        tracing::error!(
                            "LinkSysStore::rebuild_clients called outside a tokio runtime — dispatch skipped"
                        );
                    }
                },
            )
            .is_err()
        {
            tracing::error!("LinkSys dispatch sequence exhausted; client rebuild rejected");
        }
    }

    fn publish_source_event(
        &self,
        counter: &AtomicU64,
        source_lock: &Mutex<()>,
        build_snapshot: impl FnOnce(&LinkSysMap) -> Arc<LinkSysMap>,
        create_dispatch_task: impl FnOnce(u64),
    ) -> Result<u64, DispatchSequenceExhausted> {
        let _source_guard = source_lock.lock();
        let sequence = reserve_dispatch_sequence(counter).ok_or(DispatchSequenceExhausted)?;
        let current = self.resources.load_full();
        let next = build_snapshot(current.as_ref());

        // Publication and task creation share the same synchronous source-event
        // critical section. Async polling order may differ, but sequence order
        // always reflects the authoritative source entry order.
        self.resources.store(next);
        create_dispatch_task(sequence);
        Ok(sequence)
    }

    fn replace_all_with_dispatch(
        &self,
        data: HashMap<String, LinkSys>,
        counter: &AtomicU64,
        source_lock: &Mutex<()>,
        create_dispatch_task: impl FnOnce(u64, Arc<LinkSysMap>),
    ) -> Result<u64, DispatchSequenceExhausted> {
        // Wrap once and share the exact authoritative snapshot with both the
        // ArcSwap publication and the dispatch task.
        let data = Arc::new(data);
        let dispatch_data = Arc::clone(&data);
        self.publish_source_event(
            counter,
            source_lock,
            |_| Arc::clone(&data),
            |sequence| create_dispatch_task(sequence, dispatch_data),
        )
    }

    fn update_with_dispatch(
        &self,
        add_or_update: HashMap<String, LinkSys>,
        remove: &HashSet<String>,
        counter: &AtomicU64,
        source_lock: &Mutex<()>,
        create_dispatch_task: impl FnOnce(u64, Arc<LinkSysMap>, HashSet<String>),
    ) -> Result<u64, DispatchSequenceExhausted> {
        // Wrap once and share with the dispatch task to avoid a second deep clone
        // of the delta (each LinkSys carries nested credential structs).
        let add_or_update = Arc::new(add_or_update);
        let dispatch_data = Arc::clone(&add_or_update);
        let remove_set = remove.clone();
        self.publish_source_event(
            counter,
            source_lock,
            |current| {
                let mut new_map = current.clone();
                for key in remove {
                    new_map.remove(key);
                }
                for (key, ls) in add_or_update.iter() {
                    new_map.insert(key.clone(), ls.clone());
                }
                Arc::new(new_map)
            },
            |sequence| create_dispatch_task(sequence, dispatch_data, remove_set),
        )
    }

    fn rebuild_clients_with_dispatch(
        &self,
        counter: &AtomicU64,
        source_lock: &Mutex<()>,
        create_dispatch_task: impl FnOnce(u64, Arc<LinkSysMap>),
    ) -> Result<u64, DispatchSequenceExhausted> {
        let _source_guard = source_lock.lock();
        let sequence = reserve_dispatch_sequence(counter).ok_or(DispatchSequenceExhausted)?;
        let snapshot = self.resources.load_full();

        // Reserve and enqueue under the same lock used by LinkSys source
        // events. This makes the dependency-triggered rebuild part of their
        // total order without republishing or cloning the source snapshot.
        create_dispatch_task(sequence, snapshot);
        Ok(sequence)
    }
}

// ============================================================
// Generic per-provider runtime store (ArcSwap for lock-free reads)
// ============================================================

/// Generic ArcSwap-backed runtime store shared by every LinkSys provider.
///
/// Holds a map of "namespace/name" -> Arc<Client> for one provider type.
/// ArcSwap gives lock-free concurrent reads; `insert` / `remove` use `rcu`
/// so concurrent dispatch tasks cannot lose an update, and `replace_all`
/// performs a single atomic swap. There is intentionally **no** trait bound on
/// `C`: this is pure storage and never touches the client's lifecycle, so each
/// provider's static is a one-line declaration (adding a new provider no longer
/// means copying the insert/remove/replace_all bodies).
struct ProviderRuntimeStore<C> {
    map: ArcSwap<HashMap<String, Arc<C>>>,
}

impl<C> ProviderRuntimeStore<C> {
    fn new() -> Self {
        Self {
            map: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Look up a client by key ("namespace/name").
    fn get(&self, key: &str) -> Option<Arc<C>> {
        self.map.load().get(key).cloned()
    }

    /// All registered client keys.
    fn list(&self) -> Vec<String> {
        self.map.load().keys().cloned().collect()
    }

    /// Number of clients currently in the store.
    fn count(&self) -> usize {
        self.map.load().len()
    }

    /// Whether a key is currently present.
    fn contains(&self, key: &str) -> bool {
        self.map.load().contains_key(key)
    }

    /// Snapshot of all live clients (used by health checks).
    fn values(&self) -> Vec<Arc<C>> {
        self.map.load().values().cloned().collect()
    }

    /// Full `Arc` snapshot of the current map (used to carry over clients on full-set).
    fn load_full(&self) -> Arc<HashMap<String, Arc<C>>> {
        self.map.load_full()
    }

    /// Insert a client into the store.
    fn insert(&self, key: String, client: Arc<C>) {
        // `rcu` retries the closure on CAS contention, so the load-clone-insert-store
        // is atomic against concurrent dispatch tasks (no lost update). The closure
        // is `FnMut` and may run more than once, hence the per-iteration clones.
        self.map.rcu(|current| {
            let mut new_map = (**current).clone();
            new_map.insert(key.clone(), client.clone());
            new_map
        });
    }

    /// Remove a client from the store, returning the old client (if any).
    fn remove(&self, key: &str) -> Option<Arc<C>> {
        // Fast path: skip the clone when the key is clearly absent. A concurrent
        // insert after this check is a separate entry we legitimately do not remove.
        if !self.map.load().contains_key(key) {
            return None;
        }
        // `rcu` returns the map as it was immediately before the successful swap, so
        // `old.get(key)` is exactly the client this call removed.
        let old = self.map.rcu(|current| {
            let mut new_map = (**current).clone();
            new_map.remove(key);
            new_map
        });
        old.get(key).cloned()
    }

    /// Replace the whole map atomically, returning the previous contents.
    fn replace_all(&self, new_map: HashMap<String, Arc<C>>) -> HashMap<String, Arc<C>> {
        let old = self.map.swap(Arc::new(new_map));
        (*old).clone()
    }
}

fn record_current_health<O: ApplyOwner>(
    store: &ProviderRuntimeStore<PersistentRunner<O>>,
    healthy: impl Fn(&O::Client) -> bool,
) {
    for (name, runner) in store.map.load().iter() {
        if let Some(client) = runner.current() {
            record_link_sys_health(name, healthy(&client));
        }
    }
}

#[cfg(test)]
mod published_health_tests {
    use super::*;
    use crate::link_sys::runtime::apply::{ConnectionCoverage, RuntimeFingerprint};
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};

    struct HealthOwner;

    struct HealthClient {
        healthy: AtomicBool,
    }

    #[async_trait::async_trait]
    impl ApplyOwner for HealthOwner {
        type Client = HealthClient;

        fn fingerprint(_: &LinkSys) -> anyhow::Result<RuntimeFingerprint> {
            unreachable!()
        }

        fn fingerprint_live(_: &Self::Client) -> RuntimeFingerprint {
            unreachable!()
        }

        fn coverage(_: &Self::Client) -> ConnectionCoverage {
            unreachable!()
        }

        fn build(_: &str, _: &LinkSys) -> anyhow::Result<Arc<Self::Client>> {
            unreachable!()
        }

        async fn init(_: &Self::Client) -> anyhow::Result<()> {
            unreachable!()
        }

        async fn shutdown(_: Arc<Self::Client>) {}

        fn mark_retired(_: &Self::Client) {}

        fn label() -> &'static str {
            "health-test"
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn old_probe_result_cannot_overwrite_published_health_gauge() {
        let name = "test/link".to_string();
        let store = ProviderRuntimeStore::new();
        let old_client = Arc::new(HealthClient {
            healthy: AtomicBool::new(true),
        });
        let old = PersistentRunner::<HealthOwner>::spawn_with_published(name.clone(), old_client.clone());
        store.insert(name.clone(), old.clone());

        let new = PersistentRunner::<HealthOwner>::spawn_with_published(
            name.clone(),
            Arc::new(HealthClient {
                healthy: AtomicBool::new(true),
            }),
        );
        store.insert(name.clone(), new.clone());

        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || {
            record_current_health(&store, |client| client.healthy.load(Ordering::Relaxed));
            assert!(crate::link_sys::runtime::health_probe::apply_health_result(
                &old_client.healthy,
                &name,
                "test",
                &AtomicU64::new(0),
                false,
                false,
            ));
        });
        let snapshot = snapshotter.snapshot().into_vec();
        assert!(snapshot.iter().any(|(key, _, _, value)| {
            key.key().name() == crate::observe::metrics::registry::names::LINK_SYS_HEALTH
                && key
                    .key()
                    .labels()
                    .any(|label| label.key() == "link" && label.value() == name)
                && matches!(value, DebugValue::Gauge(level) if *level == 1.0)
        }));

        old.begin_delete();
        new.begin_delete();
        old.await_join().await;
        new.await_join().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn published_etcd_client_is_sampled_without_network_init() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let name = "published-health/etcd-without-init".to_string();
        let config = serde_json::from_value(serde_json::json!({
            "endpoints": ["http://127.0.0.1:2379"]
        }))
        .expect("test Etcd config must deserialize");
        let client = Arc::new(
            EtcdLinkClient::from_config(&name, &config, &credential::CredentialState::NotRequired)
                .expect("Etcd construction must not connect"),
        );
        seed_published_etcd(name.clone(), client);

        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, record_published_link_sys_health);
        let snapshot = snapshotter.snapshot().into_vec();

        let runner = ETCD_RUNTIME.remove(&name).expect("test Etcd runner must be registered");
        runner.begin_delete();
        runner.await_join().await;

        let matching: Vec<_> = snapshot
            .iter()
            .filter(|(key, _, _, _)| {
                key.key().name() == crate::observe::metrics::registry::names::LINK_SYS_HEALTH
                    && key
                        .key()
                        .labels()
                        .any(|label| label.key() == "link" && label.value() == name)
            })
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "Etcd must publish one health gauge for its unique key"
        );
        assert!(matches!(matching[0].3, DebugValue::Gauge(level) if level == 0.0));
    }
}

/// Sample the same published clients returned to Gateway data-plane callers.
/// Retired and unpublished generations never supply this gauge's value.
pub(crate) fn record_published_link_sys_health() {
    record_current_health(&REDIS_RUNTIME, RedisLinkClient::healthy);
    record_current_health(&ETCD_RUNTIME, EtcdLinkClient::healthy);
    record_current_health(&ES_RUNTIME, EsLinkClient::healthy);
    record_current_health(&KAFKA_RUNTIME, KafkaLinkClient::healthy);
    record_current_health(&OTLP_RUNTIME, OtlpLinkClient::healthy);
}

fn retain_persistent_runners(
    apply_lock: &Mutex<()>,
    present: &HashSet<String>,
    list: impl FnOnce() -> Vec<String>,
    mut delete_locked: impl FnMut(&str),
    retain_pending: impl FnOnce(&HashSet<String>),
) {
    let _guard = apply_lock.lock();
    for key in list() {
        if !present.contains(&key) {
            delete_locked(&key);
        }
    }
    retain_pending(present);
}

// ============================================================
// LinkSys instance cap (process-global, aggregated from EGC)
// ============================================================

/// Default `linkSys.maxInstancesPerKind`. Must mirror the schema default and
/// `DEFAULT_LINK_SYS_MAX_INSTANCES_PER_KIND` in `conf_handler_impl.rs`.
const DEFAULT_LINK_SYS_MAX_INSTANCES_PER_KIND: usize = 200;

/// Inclusive upper bound. Must mirror `LINK_SYS_MAX_INSTANCES_PER_KIND_LIMIT`
/// in resources / conf_handler.
const LINK_SYS_MAX_INSTANCES_PER_KIND_LIMIT: usize = 10_000;

static LINK_SYS_MAX_INSTANCES_PER_KIND: AtomicUsize = AtomicUsize::new(DEFAULT_LINK_SYS_MAX_INSTANCES_PER_KIND);

/// Read the current per-kind instance cap.
pub(crate) fn get_link_sys_max_instances_per_kind() -> usize {
    LINK_SYS_MAX_INSTANCES_PER_KIND.load(Ordering::Relaxed)
}

/// Set the per-kind instance cap (called when EdgionGatewayConfig is updated).
pub(crate) fn configure_link_sys_max_instances_per_kind(n: usize) {
    let n = n.clamp(1, LINK_SYS_MAX_INSTANCES_PER_KIND_LIMIT);
    LINK_SYS_MAX_INSTANCES_PER_KIND.store(n, Ordering::Relaxed);
    tracing::info!(cap = n, "LinkSys max instances per kind updated");
}

// ============================================================
// Redis runtime store
// ============================================================

/// Global runtime store for Redis persistent runners, keyed by "namespace/name".
static REDIS_RUNTIME: LazyLock<ProviderRuntimeStore<PersistentRunner<RedisApply>>> =
    LazyLock::new(ProviderRuntimeStore::new);
static REDIS_DELETING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
type RedisPendingApply = HashMap<String, (Arc<LinkSys>, bool)>;
static REDIS_PENDING: LazyLock<Mutex<RedisPendingApply>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static REDIS_APPLY_LOCK: Mutex<()> = Mutex::new(());

/// Get a Redis client by LinkSys name ("namespace/name").
///
/// Returns None until the runner has successfully published a generation (D14).
pub fn get_redis_client(name: &str) -> Option<Arc<RedisLinkClient>> {
    REDIS_RUNTIME.get(name).and_then(|runner| runner.current())
}

/// List all registered Redis runner keys (including connecting, unpublished keys).
pub fn list_redis_clients() -> Vec<String> {
    REDIS_RUNTIME.list()
}

/// Number of Redis runners currently in the runtime (D10: runner keys, not published clients).
pub fn redis_client_count() -> usize {
    REDIS_RUNTIME.count()
}

/// True when a runner is registered for this key, including connecting / unpublished.
pub fn redis_runner_exists(name: &str) -> bool {
    REDIS_RUNTIME.get(name).is_some()
}

/// Health check all published Redis clients. Connecting runners with an empty
/// small-handle slot are skipped (D10).
pub async fn health_check_all_redis() -> Vec<LinkSysHealth> {
    let clients: Vec<_> = REDIS_RUNTIME
        .values()
        .into_iter()
        .filter_map(|runner| runner.current())
        .collect();
    futures::future::join_all(clients.into_iter().map(|c| async move { c.health_status().await })).await
}

fn apply_redis(key: &str, ls: &LinkSys, forced: bool) {
    let _guard = REDIS_APPLY_LOCK.lock();
    apply_redis_locked(key, ls, forced);
}

fn apply_redis_locked(key: &str, ls: &LinkSys, forced: bool) {
    let (ready, tls_ok, auth) = credential_readiness(ls, credential::basic_state);
    if !ready {
        if REDIS_DELETING.lock().contains(key) {
            REDIS_PENDING.lock().remove(key);
            tracing::info!(
                key,
                label = "Redis",
                auth_kind = auth.kind(),
                tls_ready = tls_ok,
                existing = false,
                "LinkSys not ready; dropping deferred Redis apply"
            );
            return;
        }
        if let Some(runner) = REDIS_RUNTIME.get(key) {
            tracing::info!(
                key,
                label = "Redis",
                auth_kind = auth.kind(),
                tls_ready = tls_ok,
                existing = true,
                "LinkSys not ready; keeping existing client"
            );
            runner.post_not_ready();
        } else {
            tracing::info!(
                key,
                label = "Redis",
                auth_kind = auth.kind(),
                tls_ready = tls_ok,
                existing = false,
                "LinkSys not ready; skipping client build"
            );
        }
        return;
    }
    if REDIS_DELETING.lock().contains(key) {
        REDIS_PENDING
            .lock()
            .insert(key.to_string(), (Arc::new(ls.clone()), forced));
        tracing::info!(
            key,
            label = "Redis",
            "deferring Redis apply until previous runner finishes joining"
        );
        return;
    }
    let runner = match REDIS_RUNTIME.get(key) {
        Some(runner) => runner,
        None => {
            // Live runners plus draining joiners. Join replay and dispatch
            // share REDIS_APPLY_LOCK so check-then-insert is atomic.
            let in_flight = REDIS_RUNTIME.count() + REDIS_DELETING.lock().len();
            let cap = get_link_sys_max_instances_per_kind();
            if in_flight >= cap {
                tracing::warn!(
                    key,
                    label = "Redis",
                    current = in_flight,
                    cap,
                    "LinkSys instance cap reached; refusing new Redis runner"
                );
                return;
            }
            let runner = PersistentRunner::<RedisApply>::spawn(key.to_string());
            REDIS_RUNTIME.insert(key.to_string(), runner.clone());
            runner
        }
    };
    runner.post_ready(Arc::new(ls.clone()), forced);
}

fn delete_redis_runner(key: &str) {
    let _guard = REDIS_APPLY_LOCK.lock();
    delete_redis_runner_locked(key);
}

fn delete_redis_runner_locked(key: &str) {
    REDIS_PENDING.lock().remove(key);
    let Some(runner) = REDIS_RUNTIME.remove(key) else {
        return;
    };
    runner.begin_delete();
    REDIS_DELETING.lock().insert(key.to_string());
    let deleted_key = key.to_string();
    tokio::spawn(async move {
        runner.await_join().await;
        let _guard = REDIS_APPLY_LOCK.lock();
        REDIS_DELETING.lock().remove(&deleted_key);
        let pending = REDIS_PENDING.lock().remove(&deleted_key);
        if let Some((ls, forced)) = pending {
            if REDIS_RUNTIME.get(&deleted_key).is_none() {
                apply_redis_locked(&deleted_key, &ls, forced);
            }
        }
    });
}

fn retain_redis_runners(present: &HashSet<String>) {
    retain_persistent_runners(
        &REDIS_APPLY_LOCK,
        present,
        || REDIS_RUNTIME.list(),
        delete_redis_runner_locked,
        |present| REDIS_PENDING.lock().retain(|k, _| present.contains(k)),
    );
}

#[cfg(test)]
fn seed_published_redis(key: String, client: Arc<RedisLinkClient>) {
    let runner = PersistentRunner::<RedisApply>::spawn_with_published(key.clone(), client);
    REDIS_RUNTIME.insert(key, runner);
}

// ============================================================
// Etcd runtime store
// ============================================================

/// Global runtime store for Etcd persistent runners, keyed by "namespace/name".
static ETCD_RUNTIME: LazyLock<ProviderRuntimeStore<PersistentRunner<EtcdApply>>> =
    LazyLock::new(ProviderRuntimeStore::new);
static ETCD_DELETING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
type EtcdPendingApply = HashMap<String, (Arc<LinkSys>, bool)>;
static ETCD_PENDING: LazyLock<Mutex<EtcdPendingApply>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static ETCD_APPLY_LOCK: Mutex<()> = Mutex::new(());

/// Get an Etcd client by LinkSys name ("namespace/name").
///
/// Returns None until the runner has successfully published a generation (D14).
pub fn get_etcd_client(name: &str) -> Option<Arc<EtcdLinkClient>> {
    ETCD_RUNTIME.get(name).and_then(|runner| runner.current())
}

/// List all registered Etcd runner keys (including connecting, unpublished keys).
pub fn list_etcd_clients() -> Vec<String> {
    ETCD_RUNTIME.list()
}

/// Number of Etcd runners currently in the runtime (D10: runner keys, not published clients).
pub fn etcd_client_count() -> usize {
    ETCD_RUNTIME.count()
}

/// True when a runner is registered for this key, including connecting / unpublished.
pub fn etcd_runner_exists(name: &str) -> bool {
    ETCD_RUNTIME.get(name).is_some()
}

/// Health check all published Etcd clients. Connecting runners with an empty
/// small-handle slot are skipped (D10).
pub async fn health_check_all_etcd() -> Vec<LinkSysHealth> {
    let clients: Vec<_> = ETCD_RUNTIME
        .values()
        .into_iter()
        .filter_map(|runner| runner.current())
        .collect();
    futures::future::join_all(clients.into_iter().map(|c| async move { c.health_status().await })).await
}

fn apply_etcd(key: &str, ls: &LinkSys, forced: bool) {
    let _guard = ETCD_APPLY_LOCK.lock();
    apply_etcd_locked(key, ls, forced);
}

fn apply_etcd_locked(key: &str, ls: &LinkSys, forced: bool) {
    let (ready, tls_ok, auth) = credential_readiness(ls, credential::basic_state);
    if !ready {
        if ETCD_DELETING.lock().contains(key) {
            ETCD_PENDING.lock().remove(key);
            tracing::info!(
                key,
                label = "Etcd",
                auth_kind = auth.kind(),
                tls_ready = tls_ok,
                existing = false,
                "LinkSys not ready; dropping deferred Etcd apply"
            );
            return;
        }
        if let Some(runner) = ETCD_RUNTIME.get(key) {
            tracing::info!(
                key,
                label = "Etcd",
                auth_kind = auth.kind(),
                tls_ready = tls_ok,
                existing = true,
                "LinkSys not ready; keeping existing client"
            );
            runner.post_not_ready();
        } else {
            tracing::info!(
                key,
                label = "Etcd",
                auth_kind = auth.kind(),
                tls_ready = tls_ok,
                existing = false,
                "LinkSys not ready; skipping client build"
            );
        }
        return;
    }
    if ETCD_DELETING.lock().contains(key) {
        ETCD_PENDING
            .lock()
            .insert(key.to_string(), (Arc::new(ls.clone()), forced));
        tracing::info!(
            key,
            label = "Etcd",
            "deferring Etcd apply until previous runner finishes joining"
        );
        return;
    }
    let runner = match ETCD_RUNTIME.get(key) {
        Some(runner) => runner,
        None => {
            let in_flight = ETCD_RUNTIME.count() + ETCD_DELETING.lock().len();
            let cap = get_link_sys_max_instances_per_kind();
            if in_flight >= cap {
                tracing::warn!(
                    key,
                    label = "Etcd",
                    current = in_flight,
                    cap,
                    "LinkSys instance cap reached; refusing new Etcd runner"
                );
                return;
            }
            let runner = PersistentRunner::<EtcdApply>::spawn(key.to_string());
            ETCD_RUNTIME.insert(key.to_string(), runner.clone());
            runner
        }
    };
    runner.post_ready(Arc::new(ls.clone()), forced);
}

fn delete_etcd_runner(key: &str) {
    let _guard = ETCD_APPLY_LOCK.lock();
    delete_etcd_runner_locked(key);
}

fn delete_etcd_runner_locked(key: &str) {
    ETCD_PENDING.lock().remove(key);
    let Some(runner) = ETCD_RUNTIME.remove(key) else {
        return;
    };
    runner.begin_delete();
    ETCD_DELETING.lock().insert(key.to_string());
    let deleted_key = key.to_string();
    tokio::spawn(async move {
        runner.await_join().await;
        let _guard = ETCD_APPLY_LOCK.lock();
        ETCD_DELETING.lock().remove(&deleted_key);
        let pending = ETCD_PENDING.lock().remove(&deleted_key);
        if let Some((ls, forced)) = pending {
            if ETCD_RUNTIME.get(&deleted_key).is_none() {
                apply_etcd_locked(&deleted_key, &ls, forced);
            }
        }
    });
}

fn retain_etcd_runners(present: &HashSet<String>) {
    retain_persistent_runners(
        &ETCD_APPLY_LOCK,
        present,
        || ETCD_RUNTIME.list(),
        delete_etcd_runner_locked,
        |present| ETCD_PENDING.lock().retain(|k, _| present.contains(k)),
    );
}

#[cfg(test)]
fn seed_published_etcd(key: String, client: Arc<EtcdLinkClient>) {
    let runner = PersistentRunner::<EtcdApply>::spawn_with_published(key.clone(), client);
    ETCD_RUNTIME.insert(key, runner);
}

// ============================================================
// Elasticsearch runtime store
// ============================================================

/// Global runtime store for Elasticsearch persistent runners, keyed by "namespace/name".
static ES_RUNTIME: LazyLock<ProviderRuntimeStore<PersistentRunner<EsApply>>> = LazyLock::new(ProviderRuntimeStore::new);
static ES_DELETING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
type EsPendingApply = HashMap<String, (Arc<LinkSys>, bool)>;
static ES_PENDING: LazyLock<Mutex<EsPendingApply>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static ES_APPLY_LOCK: Mutex<()> = Mutex::new(());

/// Get an Elasticsearch client by LinkSys name ("namespace/name").
///
/// Returns None until the runner has successfully published a generation (D14).
pub fn get_es_client(name: &str) -> Option<Arc<EsLinkClient>> {
    ES_RUNTIME.get(name).and_then(|runner| runner.current())
}

/// List all registered Elasticsearch runner keys (including connecting, unpublished keys).
pub fn list_es_clients() -> Vec<String> {
    ES_RUNTIME.list()
}

/// Number of Elasticsearch runners currently in the runtime (D10: runner keys, not published clients).
pub fn es_client_count() -> usize {
    ES_RUNTIME.count()
}

/// True when a runner is registered for this key, including connecting / unpublished.
pub fn es_runner_exists(name: &str) -> bool {
    ES_RUNTIME.get(name).is_some()
}

/// Health check all published Elasticsearch clients. Connecting runners with an empty
/// small-handle slot are skipped (D10).
pub async fn health_check_all_es() -> Vec<LinkSysHealth> {
    let clients: Vec<_> = ES_RUNTIME
        .values()
        .into_iter()
        .filter_map(|runner| runner.current())
        .collect();
    futures::future::join_all(clients.into_iter().map(|c| async move { c.health_status().await })).await
}

fn apply_es(key: &str, ls: &LinkSys, forced: bool) {
    let _guard = ES_APPLY_LOCK.lock();
    apply_es_locked(key, ls, forced);
}

fn apply_es_locked(key: &str, ls: &LinkSys, forced: bool) {
    let (ready, tls_ok, auth) = credential_readiness(ls, credential::resolved_state);
    if !ready {
        if ES_DELETING.lock().contains(key) {
            ES_PENDING.lock().remove(key);
            tracing::info!(
                key,
                label = "Elasticsearch",
                auth_kind = auth.kind(),
                tls_ready = tls_ok,
                existing = false,
                "LinkSys not ready; dropping deferred Elasticsearch apply"
            );
            return;
        }
        if let Some(runner) = ES_RUNTIME.get(key) {
            tracing::info!(
                key,
                label = "Elasticsearch",
                auth_kind = auth.kind(),
                tls_ready = tls_ok,
                existing = true,
                "LinkSys not ready; keeping existing client"
            );
            runner.post_not_ready();
        } else {
            tracing::info!(
                key,
                label = "Elasticsearch",
                auth_kind = auth.kind(),
                tls_ready = tls_ok,
                existing = false,
                "LinkSys not ready; skipping client build"
            );
        }
        return;
    }
    if ES_DELETING.lock().contains(key) {
        ES_PENDING
            .lock()
            .insert(key.to_string(), (Arc::new(ls.clone()), forced));
        tracing::info!(
            key,
            label = "Elasticsearch",
            "deferring Elasticsearch apply until previous runner finishes joining"
        );
        return;
    }
    let runner = match ES_RUNTIME.get(key) {
        Some(runner) => runner,
        None => {
            let in_flight = ES_RUNTIME.count() + ES_DELETING.lock().len();
            let cap = get_link_sys_max_instances_per_kind();
            if in_flight >= cap {
                tracing::warn!(
                    key,
                    label = "Elasticsearch",
                    current = in_flight,
                    cap,
                    "LinkSys instance cap reached; refusing new Elasticsearch runner"
                );
                return;
            }
            let runner = PersistentRunner::<EsApply>::spawn(key.to_string());
            ES_RUNTIME.insert(key.to_string(), runner.clone());
            runner
        }
    };
    runner.post_ready(Arc::new(ls.clone()), forced);
}

fn delete_es_runner(key: &str) {
    let _guard = ES_APPLY_LOCK.lock();
    delete_es_runner_locked(key);
}

fn delete_es_runner_locked(key: &str) {
    ES_PENDING.lock().remove(key);
    let Some(runner) = ES_RUNTIME.remove(key) else {
        return;
    };
    runner.begin_delete();
    ES_DELETING.lock().insert(key.to_string());
    let deleted_key = key.to_string();
    tokio::spawn(async move {
        runner.await_join().await;
        let _guard = ES_APPLY_LOCK.lock();
        ES_DELETING.lock().remove(&deleted_key);
        let pending = ES_PENDING.lock().remove(&deleted_key);
        if let Some((ls, forced)) = pending {
            if ES_RUNTIME.get(&deleted_key).is_none() {
                apply_es_locked(&deleted_key, &ls, forced);
            }
        }
    });
}

fn retain_es_runners(present: &HashSet<String>) {
    retain_persistent_runners(
        &ES_APPLY_LOCK,
        present,
        || ES_RUNTIME.list(),
        delete_es_runner_locked,
        |present| ES_PENDING.lock().retain(|k, _| present.contains(k)),
    );
}

#[cfg(test)]
fn seed_published_es(key: String, client: Arc<EsLinkClient>) {
    let runner = PersistentRunner::<EsApply>::spawn_with_published(key.clone(), client);
    ES_RUNTIME.insert(key, runner);
}

// ============================================================
// Kafka runtime store
// ============================================================

/// Global runtime store for Kafka persistent runners, keyed by "namespace/name".
static KAFKA_RUNTIME: LazyLock<ProviderRuntimeStore<PersistentRunner<KafkaApply>>> =
    LazyLock::new(ProviderRuntimeStore::new);
static KAFKA_DELETING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
type KafkaPendingApply = HashMap<String, (Arc<LinkSys>, bool)>;
static KAFKA_PENDING: LazyLock<Mutex<KafkaPendingApply>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static KAFKA_APPLY_LOCK: Mutex<()> = Mutex::new(());

/// Get a Kafka client by LinkSys name ("namespace/name").
///
/// Returns None until the runner has successfully published a generation (D14).
pub fn get_kafka_client(name: &str) -> Option<Arc<KafkaLinkClient>> {
    KAFKA_RUNTIME.get(name).and_then(|runner| runner.current())
}

/// List all registered Kafka runner keys (including connecting, unpublished keys).
pub fn list_kafka_clients() -> Vec<String> {
    KAFKA_RUNTIME.list()
}

/// Number of Kafka runners currently in the runtime (D10: runner keys, not published clients).
pub fn kafka_client_count() -> usize {
    KAFKA_RUNTIME.count()
}

/// True when a runner is registered for this key, including connecting / unpublished.
pub fn kafka_runner_exists(name: &str) -> bool {
    KAFKA_RUNTIME.get(name).is_some()
}

fn apply_kafka(key: &str, ls: &LinkSys, forced: bool) {
    let _guard = KAFKA_APPLY_LOCK.lock();
    apply_kafka_locked(key, ls, forced);
}

fn apply_kafka_locked(key: &str, ls: &LinkSys, forced: bool) {
    let (ready, tls_ok, auth) = credential_readiness(ls, credential::resolved_state);
    if !ready {
        if KAFKA_DELETING.lock().contains(key) {
            KAFKA_PENDING.lock().remove(key);
            tracing::info!(
                key,
                label = "Kafka",
                auth_kind = auth.kind(),
                tls_ready = tls_ok,
                existing = false,
                "LinkSys not ready; dropping deferred Kafka apply"
            );
            return;
        }
        if let Some(runner) = KAFKA_RUNTIME.get(key) {
            tracing::info!(
                key,
                label = "Kafka",
                auth_kind = auth.kind(),
                tls_ready = tls_ok,
                existing = true,
                "LinkSys not ready; keeping existing client"
            );
            runner.post_not_ready();
        } else {
            tracing::info!(
                key,
                label = "Kafka",
                auth_kind = auth.kind(),
                tls_ready = tls_ok,
                existing = false,
                "LinkSys not ready; skipping client build"
            );
        }
        return;
    }
    if KAFKA_DELETING.lock().contains(key) {
        KAFKA_PENDING
            .lock()
            .insert(key.to_string(), (Arc::new(ls.clone()), forced));
        tracing::info!(
            key,
            label = "Kafka",
            "deferring Kafka apply until previous runner finishes joining"
        );
        return;
    }
    let runner = match KAFKA_RUNTIME.get(key) {
        Some(runner) => runner,
        None => {
            let in_flight = KAFKA_RUNTIME.count() + KAFKA_DELETING.lock().len();
            let cap = get_link_sys_max_instances_per_kind();
            if in_flight >= cap {
                tracing::warn!(
                    key,
                    label = "Kafka",
                    current = in_flight,
                    cap,
                    "LinkSys instance cap reached; refusing new Kafka runner"
                );
                return;
            }
            let runner = PersistentRunner::<KafkaApply>::spawn(key.to_string());
            KAFKA_RUNTIME.insert(key.to_string(), runner.clone());
            runner
        }
    };
    runner.post_ready(Arc::new(ls.clone()), forced);
}

fn delete_kafka_runner(key: &str) {
    let _guard = KAFKA_APPLY_LOCK.lock();
    delete_kafka_runner_locked(key);
}

fn delete_kafka_runner_locked(key: &str) {
    KAFKA_PENDING.lock().remove(key);
    let Some(runner) = KAFKA_RUNTIME.remove(key) else {
        return;
    };
    runner.begin_delete();
    KAFKA_DELETING.lock().insert(key.to_string());
    let deleted_key = key.to_string();
    tokio::spawn(async move {
        runner.await_join().await;
        let _guard = KAFKA_APPLY_LOCK.lock();
        KAFKA_DELETING.lock().remove(&deleted_key);
        let pending = KAFKA_PENDING.lock().remove(&deleted_key);
        if let Some((ls, forced)) = pending {
            if KAFKA_RUNTIME.get(&deleted_key).is_none() {
                apply_kafka_locked(&deleted_key, &ls, forced);
            }
        }
    });
}

fn retain_kafka_runners(present: &HashSet<String>) {
    retain_persistent_runners(
        &KAFKA_APPLY_LOCK,
        present,
        || KAFKA_RUNTIME.list(),
        delete_kafka_runner_locked,
        |present| KAFKA_PENDING.lock().retain(|k, _| present.contains(k)),
    );
}

#[cfg(test)]
fn seed_published_kafka(key: String, client: Arc<KafkaLinkClient>) {
    let runner = PersistentRunner::<KafkaApply>::spawn_with_published(key.clone(), client);
    KAFKA_RUNTIME.insert(key, runner);
}

/// Global runtime store for OTLP persistent runners, keyed by "namespace/name".
static OTLP_RUNTIME: LazyLock<ProviderRuntimeStore<PersistentRunner<OtlpApply>>> =
    LazyLock::new(ProviderRuntimeStore::new);
static OTLP_CLEANUP: LazyLock<Mutex<tokio::task::JoinSet<()>>> =
    LazyLock::new(|| Mutex::new(tokio::task::JoinSet::new()));
static OTLP_DELETING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
type OtlpPendingApply = HashMap<String, (Arc<LinkSys>, bool)>;
static OTLP_PENDING: LazyLock<Mutex<OtlpPendingApply>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static OTLP_APPLY_LOCK: Mutex<()> = Mutex::new(());
static OTLP_SHUTDOWN: Mutex<Option<Shared<BoxFuture<'static, ()>>>> = Mutex::new(None);
static OTLP_STOPPING: AtomicBool = AtomicBool::new(false);

/// Returns None until the runner publishes an exporter after an empty Export.
pub fn get_otlp_client(name: &str) -> Option<Arc<OtlpLinkClient>> {
    OTLP_RUNTIME.get(name).and_then(|runner| runner.current())
}

/// List registered runner keys, including connecting and unpublished runners.
pub fn list_otlp_clients() -> Vec<String> {
    OTLP_RUNTIME.list()
}

/// Number of live runner keys, not the number of published exporters.
pub fn otlp_client_count() -> usize {
    OTLP_RUNTIME.count()
}

fn apply_otlp(key: &str, resource: &LinkSys, forced: bool) {
    let _guard = OTLP_APPLY_LOCK.lock();
    apply_otlp_locked(key, resource, forced);
}

fn apply_otlp_locked(key: &str, resource: &LinkSys, forced: bool) {
    if OTLP_STOPPING.load(Ordering::Acquire) {
        return;
    }
    let (ready, tls_ok, auth) = credential_readiness(resource, credential::resolved_state);
    if !ready {
        if OTLP_DELETING.lock().contains(key) {
            OTLP_PENDING.lock().remove(key);
            tracing::info!(
                key,
                label = "OTLP",
                auth_kind = auth.kind(),
                tls_ready = tls_ok,
                existing = false,
                "LinkSys not ready; dropping deferred OTLP apply"
            );
            return;
        }
        if let Some(runner) = OTLP_RUNTIME.get(key) {
            tracing::info!(
                key,
                label = "OTLP",
                auth_kind = auth.kind(),
                tls_ready = tls_ok,
                existing = true,
                "LinkSys not ready; keeping existing client"
            );
            runner.post_not_ready();
        } else {
            tracing::info!(
                key,
                label = "OTLP",
                auth_kind = auth.kind(),
                tls_ready = tls_ok,
                existing = false,
                "LinkSys not ready; skipping client build"
            );
        }
        return;
    }
    if OTLP_DELETING.lock().contains(key) {
        OTLP_PENDING
            .lock()
            .insert(key.to_string(), (Arc::new(resource.clone()), forced));
        tracing::info!(
            key,
            label = "OTLP",
            "deferring OTLP apply until previous runner finishes joining"
        );
        return;
    }
    let runner = match OTLP_RUNTIME.get(key) {
        Some(runner) => runner,
        None => {
            let in_flight = OTLP_RUNTIME.count() + OTLP_DELETING.lock().len();
            let cap = get_link_sys_max_instances_per_kind();
            if in_flight >= cap {
                tracing::warn!(
                    key,
                    label = "OTLP",
                    current = in_flight,
                    cap,
                    "LinkSys instance cap reached; refusing new OTLP runner"
                );
                return;
            }
            let runner = PersistentRunner::<OtlpApply>::spawn(key.to_string());
            OTLP_RUNTIME.insert(key.to_string(), runner.clone());
            runner
        }
    };
    runner.post_ready(Arc::new(resource.clone()), forced);
}

fn delete_otlp_runner(key: &str) {
    let _guard = OTLP_APPLY_LOCK.lock();
    delete_otlp_runner_locked(key);
}

fn delete_otlp_runner_locked(key: &str) {
    OTLP_PENDING.lock().remove(key);
    let Some(runner) = OTLP_RUNTIME.remove(key) else {
        return;
    };
    runner.begin_delete();
    OTLP_DELETING.lock().insert(key.to_string());
    let deleted_key = key.to_string();
    let mut cleanup = OTLP_CLEANUP.lock();
    reap_finished_otlp_cleanup(&mut cleanup);
    cleanup.spawn(async move {
        runner.await_join().await;
        let _guard = OTLP_APPLY_LOCK.lock();
        OTLP_DELETING.lock().remove(&deleted_key);
        let pending = OTLP_PENDING.lock().remove(&deleted_key);
        if !OTLP_STOPPING.load(Ordering::Acquire) {
            if let Some((resource, forced)) = pending {
                if OTLP_RUNTIME.get(&deleted_key).is_none() {
                    apply_otlp_locked(&deleted_key, &resource, forced);
                }
            }
        }
    });
}

fn retain_otlp_runners(present: &HashSet<String>) {
    retain_persistent_runners(
        &OTLP_APPLY_LOCK,
        present,
        || OTLP_RUNTIME.list(),
        delete_otlp_runner_locked,
        |present| OTLP_PENDING.lock().retain(|key, _| present.contains(key)),
    );
}

#[cfg(test)]
pub(crate) fn seed_published_otlp(key: String, client: Arc<OtlpLinkClient>) {
    let runner = PersistentRunner::<OtlpApply>::spawn_with_published(key.clone(), client);
    OTLP_RUNTIME.insert(key, runner);
}

fn reap_finished_otlp_cleanup(cleanup: &mut tokio::task::JoinSet<()>) {
    while let Some(result) = cleanup.try_join_next() {
        if result.is_err() {
            tracing::warn!(system = "otlp", reason = "shutdown_failed", "OTLP cleanup task failed");
        }
    }
}

/// Includes removed runners whose probe and SDK cleanup have not joined yet.
pub fn otlp_retiring_client_count() -> usize {
    let mut cleanup = OTLP_CLEANUP.lock();
    reap_finished_otlp_cleanup(&mut cleanup);
    drop(cleanup);
    OTLP_DELETING.lock().len()
}

/// Cancel all OTLP runners and join cleanup before Tokio IO is dropped.
pub async fn shutdown_otlp_clients() {
    let completion = {
        let mut shutdown = OTLP_SHUTDOWN.lock();
        shutdown
            .get_or_insert_with(|| {
                let mut cleanup = {
                    let _guard = OTLP_APPLY_LOCK.lock();
                    OTLP_STOPPING.store(true, Ordering::Release);
                    for key in OTLP_RUNTIME.list() {
                        delete_otlp_runner_locked(&key);
                    }
                    std::mem::replace(&mut *OTLP_CLEANUP.lock(), tokio::task::JoinSet::new())
                };
                let task = tokio::spawn(async move {
                    while let Some(result) = cleanup.join_next().await {
                        if result.is_err() {
                            tracing::warn!(system = "otlp", reason = "shutdown_failed", "OTLP cleanup task failed");
                        }
                    }
                });
                async move {
                    if task.await.is_err() {
                        tracing::warn!(system = "otlp", reason = "shutdown_failed", "OTLP process drain failed");
                    }
                }
                .boxed()
                .shared()
            })
            .clone()
    };
    completion.await;
}

// ============================================================
// Webhook runtime store
// ============================================================
//
// Webhook does NOT expose a typed `get_webhook_client` API — the runtime
// store is internal-only, used by dispatch to track per-key Arc<WebhookLinkClient>
// instances so they can be shut down on remove. Plugins / other consumers
// continue to interact with WebhookManager via existing APIs.

static WEBHOOK_RUNTIME: LazyLock<ProviderRuntimeStore<WebhookLinkClient>> = LazyLock::new(ProviderRuntimeStore::new);

// ============================================================
// Generic dispatch helpers
// ============================================================

/// Compute the credential-readiness gate shared by every non-Webhook provider
/// arm in both dispatch functions: a provider is buildable only when its auth
/// material resolves (`is_buildable()`) AND its TLS material (if any) is ready.
///
/// Centralizing the `is_buildable() && tls_ok` formula keeps `dispatch_full_set`
/// and `dispatch_partial_update` in lock-step and prevents a future provider arm
/// from silently dropping the `tls_ok` term. Returns `(ready, tls_ok, auth)`;
/// `tls_ok` and `auth` are returned because the readiness-log helpers want
/// `tls_ok` / `auth.kind()` and the build closures borrow `&auth`.
///
/// Webhook is intentionally NOT routed through this helper — it gates on auth
/// only (`tls_ok` hard-coded true; TLS is validated inside the manager).
fn credential_readiness<M>(
    ls: &LinkSys,
    auth_fn: fn(&LinkSys) -> credential::CredentialState<M>,
) -> (bool, bool, credential::CredentialState<M>) {
    let tls_ok = credential::tls_ready(ls);
    let auth = auth_fn(ls);
    let ready = auth.is_buildable() && tls_ok;
    (ready, tls_ok, auth)
}

/// Webhook readiness for `${secretRef:...}`: every referenced secret must be
/// present in the controller-carried `resolved_secrets` map. An unresolved ref
/// (missing/invalid Secret, or a not-yet-granted cross-namespace reference)
/// makes the webhook not-ready (fail-closed) — never serve with an empty
/// credential.
fn webhook_secrets_ready(config: &edgion_resources::resources::link_sys::webhook::WebhookServiceConfig) -> bool {
    config.unresolved_secret_refs().is_empty()
}

fn webhook_fingerprint_or_unspecified(ls: &LinkSys) -> RuntimeFingerprint {
    fingerprint_webhook(ls).unwrap_or_else(|_| RuntimeFingerprint::unspecified())
}

fn webhook_can_reuse(existing: &WebhookLinkClient, ls: &LinkSys, forced: bool) -> bool {
    if forced {
        return false;
    }
    let incoming = webhook_fingerprint_or_unspecified(ls);
    let live = existing.runtime_fingerprint();
    live.equal_runtime(&incoming) || live.rv_skip_allows_reuse(&incoming)
}

/// `dispatch_partial_update` case: when not ready, keep the current state
/// (the old Arc in the live store keeps serving).
/// Returns `true` to indicate handled; the caller should `continue`.
fn preserve_if_not_ready<C>(
    key: &str,
    label: &'static str,
    auth_kind: &str,
    tls_ok: bool,
    ready: bool,
    runtime: &ProviderRuntimeStore<C>,
) -> bool {
    if ready {
        return false;
    }
    let existing = runtime.contains(key);
    tracing::info!(
        key,
        label,
        auth_kind,
        tls_ready = tls_ok,
        existing,
        "LinkSys not ready; {} client",
        if existing { "keeping existing" } else { "skipping" }
    );
    true
}

// ============================================================
// Dispatch to sub-module managers
// ============================================================

/// Dispatch a full set of LinkSys resources to sub-module managers.
async fn dispatch_full_set(sequence: u64, data: &HashMap<String, LinkSys>, forced: bool) {
    dispatch_full_set_with(sequence, data, get_webhook_manager(), &WEBHOOK_RUNTIME, forced).await;
}

async fn dispatch_full_set_with(
    sequence: u64,
    data: &HashMap<String, LinkSys>,
    webhook_manager: &WebhookManager,
    webhook_runtime: &ProviderRuntimeStore<WebhookLinkClient>,
    forced: bool,
) {
    let webhook_mutation = match webhook_manager.begin_mutation(sequence).await {
        Ok(mutation) => mutation,
        Err(stale) => {
            tracing::info!(
                sequence = stale.sequence,
                latest_sequence = stale.latest_accepted_sequence,
                "Skipping stale LinkSys full-set dispatch"
            );
            return;
        }
    };

    // Snapshot current runtime maps so that we can carry over clients for
    // resources whose credentials are not yet resolved.
    let old_webhook_snapshot = webhook_runtime.load_full();

    let mut new_webhook_map: HashMap<String, Arc<WebhookLinkClient>> = HashMap::new();

    let mut present_redis: HashSet<String> = HashSet::new();
    let mut present_etcd: HashSet<String> = HashSet::new();
    let mut present_es: HashSet<String> = HashSet::new();
    let mut present_kafka: HashSet<String> = HashSet::new();
    let mut present_otlp: HashSet<String> = HashSet::new();

    for (key, ls) in data {
        match &ls.spec.config {
            edgion_resources::resources::link_sys::SystemConfig::Redis(_) => {
                present_redis.insert(key.clone());
            }
            edgion_resources::resources::link_sys::SystemConfig::Etcd(_) => {
                present_etcd.insert(key.clone());
            }
            edgion_resources::resources::link_sys::SystemConfig::Elasticsearch(_) => {
                present_es.insert(key.clone());
            }
            edgion_resources::resources::link_sys::SystemConfig::Kafka(_) => {
                present_kafka.insert(key.clone());
            }
            edgion_resources::resources::link_sys::SystemConfig::Otlp(_) => {
                present_otlp.insert(key.clone());
            }
            _ => {}
        }
    }

    // Remove runners for omitted or changed kinds before any new kind can apply.
    retain_redis_runners(&present_redis);
    retain_etcd_runners(&present_etcd);
    retain_es_runners(&present_es);
    retain_kafka_runners(&present_kafka);
    retain_otlp_runners(&present_otlp);

    for (key, ls) in data {
        match &ls.spec.config {
            edgion_resources::resources::link_sys::SystemConfig::Webhook(config) => {
                // Gate on credential readiness: a webhook whose `auth.secretRef`
                // is configured but whose Secret is not yet injected (Pending) or
                // is malformed (Invalid) must not be registered — otherwise
                // `inject_auth` would silently send an unauthenticated request.
                // Mirrors the Elasticsearch arm below.
                let auth = credential::resolved_state(ls);
                let ready = auth.is_buildable() && webhook_secrets_ready(config);
                if !ready {
                    if let Some(existing) = old_webhook_snapshot.get(key) {
                        tracing::info!(
                            key,
                            label = "Webhook",
                            auth_kind = auth.kind(),
                            tls_ready = true,
                            "LinkSys not ready; keeping existing client"
                        );
                        new_webhook_map.insert(key.clone(), existing.clone());
                    } else {
                        tracing::info!(
                            key,
                            label = "Webhook",
                            auth_kind = auth.kind(),
                            tls_ready = true,
                            "LinkSys not ready; skipping client build"
                        );
                    }
                    continue;
                }
                if let Some(existing) = old_webhook_snapshot.get(key) {
                    if webhook_can_reuse(existing, ls, forced) {
                        new_webhook_map.insert(key.clone(), existing.clone());
                        continue;
                    }
                }
                match webhook_mutation.prepare(key, config.clone(), ls.metadata.namespace.as_deref()) {
                    Ok(prepared) => {
                        new_webhook_map.insert(
                            key.clone(),
                            Arc::new(WebhookLinkClient::from_prepared(
                                prepared,
                                webhook_fingerprint_or_unspecified(ls),
                            )),
                        );
                    }
                    Err(e) => {
                        tracing::error!(key, label = "Webhook", error = %e, "prepare failed");
                        if let Some(existing) = old_webhook_snapshot.get(key) {
                            new_webhook_map.insert(key.clone(), existing.clone());
                        }
                    }
                }
            }
            edgion_resources::resources::link_sys::SystemConfig::Redis(_) => {
                apply_redis(key, ls, forced);
            }
            edgion_resources::resources::link_sys::SystemConfig::Etcd(_) => {
                apply_etcd(key, ls, forced);
            }
            edgion_resources::resources::link_sys::SystemConfig::Elasticsearch(_) => {
                apply_es(key, ls, forced);
            }
            edgion_resources::resources::link_sys::SystemConfig::Kafka(_) => {
                apply_kafka(key, ls, forced);
            }
            // DNS resolver, not a runtime link client — nothing to register in the link store.
            edgion_resources::resources::link_sys::SystemConfig::HttpDns(_) => {}
            // Controller-only credential acquisition metadata. The Gateway receives
            // only the resolved plugin credential and never contacts the issuer.
            edgion_resources::resources::link_sys::SystemConfig::CredentialSource(_) => {}
            edgion_resources::resources::link_sys::SystemConfig::Otlp(_) => {
                apply_otlp(key, ls, forced);
            }
        }
    }

    let prepared_webhooks = new_webhook_map
        .iter()
        .map(|(key, client)| (key.clone(), client.prepared_entry()))
        .collect();
    if let Err(e) = webhook_mutation.commit_full(prepared_webhooks).await {
        tracing::error!(sequence, error = %e, "Webhook full-set commit rejected");
        return;
    }
    webhook_runtime.replace_all(new_webhook_map);
}

/// Dispatch incremental changes to sub-module managers.
async fn dispatch_partial_update(sequence: u64, add_or_update: &HashMap<String, LinkSys>, remove: &HashSet<String>) {
    dispatch_partial_update_with(sequence, add_or_update, remove, get_webhook_manager(), &WEBHOOK_RUNTIME).await;
}

async fn dispatch_partial_update_with(
    sequence: u64,
    add_or_update: &HashMap<String, LinkSys>,
    remove: &HashSet<String>,
    webhook_manager: &WebhookManager,
    webhook_runtime: &ProviderRuntimeStore<WebhookLinkClient>,
) {
    let webhook_mutation = match webhook_manager.begin_mutation(sequence).await {
        Ok(mutation) => mutation,
        Err(stale) => {
            tracing::info!(
                sequence = stale.sequence,
                latest_sequence = stale.latest_accepted_sequence,
                "Skipping stale LinkSys partial dispatch"
            );
            return;
        }
    };

    // Handle add/update. Kind change is Delete(old persistent kind) + Create
    // in the same dispatch (D7).
    for (key, ls) in add_or_update {
        if !matches!(
            &ls.spec.config,
            edgion_resources::resources::link_sys::SystemConfig::Redis(_)
        ) {
            delete_redis_runner(key);
        }
        if !matches!(
            &ls.spec.config,
            edgion_resources::resources::link_sys::SystemConfig::Etcd(_)
        ) {
            delete_etcd_runner(key);
        }
        if !matches!(
            &ls.spec.config,
            edgion_resources::resources::link_sys::SystemConfig::Elasticsearch(_)
        ) {
            delete_es_runner(key);
        }
        if !matches!(
            &ls.spec.config,
            edgion_resources::resources::link_sys::SystemConfig::Kafka(_)
        ) {
            delete_kafka_runner(key);
        }
        if !matches!(
            &ls.spec.config,
            edgion_resources::resources::link_sys::SystemConfig::Otlp(_)
        ) {
            delete_otlp_runner(key);
        }
        match &ls.spec.config {
            edgion_resources::resources::link_sys::SystemConfig::Webhook(config) => {
                // Gate on credential readiness so a webhook whose secretRef is
                // configured but whose Secret is not yet injected (Pending) or is
                // malformed (Invalid) is not registered with an unresolved
                // credential — otherwise inject_auth would silently send an
                // unauthenticated request. Mirrors the Elasticsearch arm.
                let auth = credential::resolved_state(ls);
                let tls_ok = true; // Webhook TLS material is validated inside build_webhook_client.
                let ready = auth.is_buildable() && webhook_secrets_ready(config);
                if preserve_if_not_ready(key, "Webhook", auth.kind(), tls_ok, ready, webhook_runtime) {
                    continue;
                }
                if let Some(existing) = webhook_runtime.get(key) {
                    if webhook_can_reuse(&existing, ls, false) {
                        continue;
                    }
                }
                match webhook_mutation.prepare(key, config.clone(), ls.metadata.namespace.as_deref()) {
                    Err(e) => tracing::error!(key, label = "Webhook", error = %e, "prepare failed"),
                    Ok(prepared) => {
                        let client = Arc::new(WebhookLinkClient::from_prepared(
                            prepared,
                            webhook_fingerprint_or_unspecified(ls),
                        ));
                        if let Err(e) = webhook_mutation.commit_one(key, client.prepared_entry()).await {
                            tracing::error!(key, label = "Webhook", error = %e, "commit failed");
                        } else {
                            webhook_runtime.insert(key.clone(), client);
                        }
                    }
                }
            }
            edgion_resources::resources::link_sys::SystemConfig::Redis(_) => {
                apply_redis(key, ls, false);
            }
            edgion_resources::resources::link_sys::SystemConfig::Etcd(_) => {
                apply_etcd(key, ls, false);
            }
            edgion_resources::resources::link_sys::SystemConfig::Elasticsearch(_) => {
                apply_es(key, ls, false);
            }
            edgion_resources::resources::link_sys::SystemConfig::Kafka(_) => {
                apply_kafka(key, ls, false);
            }
            edgion_resources::resources::link_sys::SystemConfig::HttpDns(_) => {}
            // Controller-only credential acquisition metadata.
            edgion_resources::resources::link_sys::SystemConfig::CredentialSource(_) => {}
            edgion_resources::resources::link_sys::SystemConfig::Otlp(_) => {
                // Switching a resource to OTLP must not leave its previous
                // provider callable under the same key.
                webhook_mutation.commit_remove(key).await;
                webhook_runtime.remove(key);
                apply_otlp(key, ls, false);
            }
        }
    }

    // Handle remove — uniform across all providers
    for key in remove {
        webhook_mutation.commit_remove(key).await;
        webhook_runtime.remove(key);
        delete_redis_runner(key);
        delete_etcd_runner(key);
        delete_es_runner(key);
        delete_kafka_runner(key);
        delete_otlp_runner(key);
    }
}

// ============================================================
// Global singleton
// ============================================================

static GLOBAL_LINK_SYS_STORE: LazyLock<Arc<LinkSysStore>> = LazyLock::new(|| Arc::new(LinkSysStore::new()));

/// Get the global LinkSys store
pub fn get_global_link_sys_store() -> Arc<LinkSysStore> {
    GLOBAL_LINK_SYS_STORE.clone()
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
pub(crate) static I5_DISPATCH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Serializes tests that mutate `LINK_SYS_MAX_INSTANCES_PER_KIND` with
/// `recompute_global_config`, which writes the same atomic. Reentrant so a
/// test can hold it across `full_set` / `partial_update`.
#[cfg(test)]
pub(crate) static LINK_SYS_CAP_TEST_LOCK: parking_lot::ReentrantMutex<()> = parking_lot::ReentrantMutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    type DispatchTaskSlot = Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>;

    #[test]
    fn full_set_retain_holds_apply_lock_through_runtime_and_pending_cleanup() {
        let apply_lock = Mutex::new(());
        let present = HashSet::from(["ns/kept".to_string()]);
        let mut deleted = Vec::new();
        let mut pending = HashSet::from(["ns/kept".to_string(), "ns/omitted".to_string()]);

        retain_persistent_runners(
            &apply_lock,
            &present,
            || {
                assert!(
                    apply_lock.try_lock().is_none(),
                    "runtime enumeration must hold the apply lock"
                );
                vec!["ns/omitted".to_string(), "ns/kept".to_string()]
            },
            |key| {
                assert!(
                    apply_lock.try_lock().is_none(),
                    "runtime deletion must hold the apply lock"
                );
                deleted.push(key.to_string());
            },
            |present| {
                assert!(
                    apply_lock.try_lock().is_none(),
                    "pending pruning must hold the apply lock"
                );
                pending.retain(|key| present.contains(key));
            },
        );

        assert_eq!(deleted, ["ns/omitted"]);
        assert_eq!(pending, present);
    }

    fn next_dispatch_sequence() -> u64 {
        reserve_dispatch_sequence(&NEXT_LINK_SYS_DISPATCH_SEQUENCE)
            .expect("test dispatch sequence must remain available")
    }

    fn create_test_webhook_linksys(namespace: &str, name: &str) -> LinkSys {
        create_test_webhook_linksys_with_port(namespace, name, 8080)
    }

    fn create_test_webhook_linksys_with_port(namespace: &str, name: &str, port: i32) -> LinkSys {
        use edgion_resources::resources::link_sys::webhook::{
            ExternalHttpConnection, WebhookServiceConfig, WebhookServiceRef, WebhookTarget,
        };
        use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};

        LinkSys {
            metadata: kube::core::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: LinkSysSpec {
                unknown_fields: Vec::new(),
                config: SystemConfig::Webhook(WebhookServiceConfig {
                    conn: ExternalHttpConnection {
                        target: WebhookTarget {
                            service: WebhookServiceRef {
                                kind: None,
                                name: Some("resolve-svc".to_string()),
                                namespace: Some(namespace.to_string()),
                                port: Some(port),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            },
            status: None,
        }
    }

    fn spawn_partial_dispatch(
        manager: Arc<WebhookManager>,
        runtime: Arc<ProviderRuntimeStore<WebhookLinkClient>>,
        gate: Option<tokio::sync::oneshot::Receiver<()>>,
        started: Option<tokio::sync::oneshot::Sender<()>>,
        slot: DispatchTaskSlot,
    ) -> impl FnOnce(u64, Arc<LinkSysMap>, HashSet<String>) {
        move |sequence, data, remove| {
            let task = tokio::spawn(async move {
                if let Some(started) = started {
                    started.send(()).expect("dispatch start receiver remains alive");
                }
                if let Some(gate) = gate {
                    gate.await.expect("dispatch gate sender remains alive");
                }
                dispatch_partial_update_with(sequence, &data, &remove, &manager, &runtime).await;
            });
            *slot.lock() = Some(task);
        }
    }

    fn spawn_full_dispatch(
        manager: Arc<WebhookManager>,
        runtime: Arc<ProviderRuntimeStore<WebhookLinkClient>>,
        gate: Option<tokio::sync::oneshot::Receiver<()>>,
        slot: DispatchTaskSlot,
    ) -> impl FnOnce(u64, Arc<LinkSysMap>) {
        move |sequence, data| {
            let task = tokio::spawn(async move {
                if let Some(gate) = gate {
                    gate.await.expect("dispatch gate sender remains alive");
                }
                dispatch_full_set_with(sequence, &data, &manager, &runtime, false).await;
            });
            *slot.lock() = Some(task);
        }
    }

    async fn await_dispatch_task(slot: &DispatchTaskSlot) {
        let task = slot.lock().take().expect("source entry creates one dispatch task");
        task.await.expect("dispatch task completes without panic");
    }

    fn source_webhook_port(store: &LinkSysStore, key: &str) -> i32 {
        use edgion_resources::resources::link_sys::SystemConfig;

        let resource = store.get(key).expect("source snapshot contains webhook");
        let SystemConfig::Webhook(config) = resource.spec.config else {
            panic!("source snapshot entry must remain a webhook");
        };
        config
            .conn
            .target
            .service
            .port
            .expect("test webhook has a service port")
    }

    async fn assert_callable_webhook_port(
        manager: &WebhookManager,
        runtime: &ProviderRuntimeStore<WebhookLinkClient>,
        key: &str,
        expected_port: i32,
    ) {
        use crate::plugins::http::common::outbound::target::ResolvedTarget;

        let bookkeeping = runtime.get(key).expect("runtime bookkeeping contains webhook");
        let callable = manager.get(key).await.expect("manager contains callable webhook");
        assert!(Arc::ptr_eq(bookkeeping.prepared_entry().client(), &callable));
        match &callable.resolved_target {
            ResolvedTarget::Service { port, .. } => assert_eq!(*port, Some(expected_port)),
            other => panic!("expected Service target, got {other:?}"),
        }
    }

    #[test]
    fn test_full_set() {
        let store = LinkSysStore::new();

        let mut data = HashMap::new();
        data.insert(
            "default/webhook1".to_string(),
            create_test_webhook_linksys("default", "webhook1"),
        );
        data.insert(
            "default/webhook2".to_string(),
            create_test_webhook_linksys("default", "webhook2"),
        );

        store.replace_all(data);

        assert!(store.contains("default/webhook1"));
        assert!(store.contains("default/webhook2"));
        assert!(!store.contains("default/webhook3"));
        assert_eq!(store.count(), 2);
    }

    #[test]
    fn test_partial_update_add() {
        let store = LinkSysStore::new();

        let mut add = HashMap::new();
        add.insert(
            "default/webhook1".to_string(),
            create_test_webhook_linksys("default", "webhook1"),
        );

        store.update(add, &HashSet::new());

        assert!(store.contains("default/webhook1"));
    }

    #[test]
    fn test_partial_update_remove() {
        let store = LinkSysStore::new();

        let mut data = HashMap::new();
        data.insert(
            "default/webhook1".to_string(),
            create_test_webhook_linksys("default", "webhook1"),
        );
        store.replace_all(data);

        let mut remove = HashSet::new();
        remove.insert("default/webhook1".to_string());
        store.update(HashMap::new(), &remove);

        assert!(!store.contains("default/webhook1"));
    }

    #[test]
    fn test_get() {
        let store = LinkSysStore::new();

        let mut data = HashMap::new();
        data.insert(
            "default/webhook1".to_string(),
            create_test_webhook_linksys("default", "webhook1"),
        );
        store.replace_all(data);

        let ls = store.get("default/webhook1");
        assert!(ls.is_some());

        let ls = store.get("default/nonexistent");
        assert!(ls.is_none());
    }

    #[test]
    fn dependency_rebuild_dispatches_current_snapshot_without_republishing_it() {
        let store = LinkSysStore::new();
        let source_lock = Mutex::new(());
        let counter = AtomicU64::new(0);
        let key = "default/rebuild".to_string();
        let source = Arc::new(HashMap::from([(
            key.clone(),
            create_test_webhook_linksys_with_port("default", "rebuild", 8181),
        )]));
        store.resources.store(Arc::clone(&source));

        let dispatched = Mutex::new(None);
        let sequence = store
            .rebuild_clients_with_dispatch(&counter, &source_lock, |sequence, snapshot| {
                *dispatched.lock() = Some((sequence, snapshot));
            })
            .expect("dependency rebuild reserves a dispatch sequence");

        let (dispatched_sequence, dispatched_snapshot) = dispatched
            .into_inner()
            .expect("dependency rebuild creates one dispatch task");
        assert_eq!(sequence, 1);
        assert_eq!(dispatched_sequence, 1);
        assert!(Arc::ptr_eq(&source, &dispatched_snapshot));
        assert!(Arc::ptr_eq(&source, &store.resources.load_full()));
        assert_eq!(source_webhook_port(&store, &key), 8181);
    }

    #[test]
    fn source_entry_sequence_overflow_preserves_snapshot_and_skips_task_creation() {
        let store = LinkSysStore::new();
        let source_lock = Mutex::new(());
        let counter = AtomicU64::new(u64::MAX - 1);
        let task_creations = AtomicUsize::new(0);
        let stable_key = "ordered/stable".to_string();

        let sequence = store
            .replace_all_with_dispatch(
                HashMap::from([(
                    stable_key.clone(),
                    create_test_webhook_linksys_with_port("ordered", "stable", 8080),
                )]),
                &counter,
                &source_lock,
                |sequence, _| {
                    assert_eq!(sequence, u64::MAX);
                    task_creations.fetch_add(1, Ordering::Relaxed);
                },
            )
            .expect("u64::MAX is the final accepted sequence");
        assert_eq!(sequence, u64::MAX);
        let before = store.resources.load_full();

        let result = store.update_with_dispatch(
            HashMap::from([(
                "ordered/rejected".to_string(),
                create_test_webhook_linksys_with_port("ordered", "rejected", 9090),
            )]),
            &HashSet::new(),
            &counter,
            &source_lock,
            |_, _, _| {
                task_creations.fetch_add(1, Ordering::Relaxed);
            },
        );

        assert_eq!(result, Err(DispatchSequenceExhausted));
        let after = store.resources.load_full();
        assert!(Arc::ptr_eq(&before, &after));
        assert_eq!(task_creations.load(Ordering::Relaxed), 1);
        assert_eq!(counter.load(Ordering::Acquire), u64::MAX);
        assert_eq!(source_webhook_port(&store, &stable_key), 8080);
        assert!(!store.contains("ordered/rejected"));
    }

    async fn assert_newer_source_event_wins(older_full: bool, newer_full: bool) {
        let _global_guard = I5_DISPATCH_LOCK.lock().await;
        let store = LinkSysStore::new();
        let source_lock = Mutex::new(());
        let counter = AtomicU64::new(0);
        let manager = Arc::new(WebhookManager::new_for_test());
        let runtime = Arc::new(ProviderRuntimeStore::new());
        let key = "ordered/interleaving".to_string();
        let older = HashMap::from([(
            key.clone(),
            create_test_webhook_linksys_with_port("ordered", "interleaving", 8081),
        )]);
        let newer = HashMap::from([(
            key.clone(),
            create_test_webhook_linksys_with_port("ordered", "interleaving", 8082),
        )]);
        let (release_older_tx, release_older_rx) = tokio::sync::oneshot::channel();
        let older_slot: DispatchTaskSlot = Arc::new(Mutex::new(None));
        let newer_slot: DispatchTaskSlot = Arc::new(Mutex::new(None));

        let older_sequence = if older_full {
            store.replace_all_with_dispatch(
                older,
                &counter,
                &source_lock,
                spawn_full_dispatch(
                    manager.clone(),
                    runtime.clone(),
                    Some(release_older_rx),
                    older_slot.clone(),
                ),
            )
        } else {
            store.update_with_dispatch(
                older,
                &HashSet::new(),
                &counter,
                &source_lock,
                spawn_partial_dispatch(
                    manager.clone(),
                    runtime.clone(),
                    Some(release_older_rx),
                    None,
                    older_slot.clone(),
                ),
            )
        }
        .expect("older source event is accepted");

        let newer_sequence = if newer_full {
            store.replace_all_with_dispatch(
                newer,
                &counter,
                &source_lock,
                spawn_full_dispatch(manager.clone(), runtime.clone(), None, newer_slot.clone()),
            )
        } else {
            store.update_with_dispatch(
                newer,
                &HashSet::new(),
                &counter,
                &source_lock,
                spawn_partial_dispatch(manager.clone(), runtime.clone(), None, None, newer_slot.clone()),
            )
        }
        .expect("newer source event is accepted");

        assert_eq!(older_sequence, 1);
        assert_eq!(newer_sequence, 2);
        await_dispatch_task(&newer_slot).await;
        release_older_tx
            .send(())
            .expect("older dispatch still waits on its gate");
        await_dispatch_task(&older_slot).await;

        assert_eq!(source_webhook_port(&store, &key), 8082);
        assert_callable_webhook_port(&manager, &runtime, &key, 8082).await;
    }

    #[tokio::test]
    async fn source_entry_partial_partial_reversed_polling_keeps_newer_state() {
        assert_newer_source_event_wins(false, false).await;
    }

    #[tokio::test]
    async fn source_entry_older_full_newer_partial_keeps_newer_state() {
        assert_newer_source_event_wins(true, false).await;
    }

    #[tokio::test]
    async fn source_entry_older_partial_newer_full_keeps_newer_state() {
        assert_newer_source_event_wins(false, true).await;
    }

    #[tokio::test]
    async fn source_entry_older_guard_makes_newer_dispatch_wait_then_win() {
        use edgion_resources::resources::link_sys::SystemConfig;

        let store = LinkSysStore::new();
        let source_lock = Mutex::new(());
        let counter = AtomicU64::new(0);
        let manager = Arc::new(WebhookManager::new_for_test());
        let runtime = Arc::new(ProviderRuntimeStore::new());
        let key = "ordered/waiting".to_string();
        let (older_acquired_tx, older_acquired_rx) = tokio::sync::oneshot::channel();
        let (release_older_tx, release_older_rx) = tokio::sync::oneshot::channel();
        let older_slot: DispatchTaskSlot = Arc::new(Mutex::new(None));
        let older_task_slot = older_slot.clone();
        let older_manager = manager.clone();
        let older_runtime = runtime.clone();

        let older_sequence = store
            .update_with_dispatch(
                HashMap::from([(
                    key.clone(),
                    create_test_webhook_linksys_with_port("ordered", "waiting", 8081),
                )]),
                &HashSet::new(),
                &counter,
                &source_lock,
                move |sequence, data, _| {
                    let task = tokio::spawn(async move {
                        let mutation = older_manager
                            .begin_mutation(sequence)
                            .await
                            .expect("older transition acquires the manager guard");
                        older_acquired_tx
                            .send(())
                            .expect("older-acquired receiver remains alive");
                        release_older_rx.await.expect("older release sender remains alive");
                        let (key, resource) = data.iter().next().expect("test delta has one webhook");
                        let SystemConfig::Webhook(config) = &resource.spec.config else {
                            panic!("test delta must contain a webhook");
                        };
                        let prepared = mutation
                            .prepare(key, config.clone(), resource.metadata.namespace.as_deref())
                            .expect("older webhook prepares");
                        let client = Arc::new(WebhookLinkClient::from_prepared(
                            prepared,
                            webhook_fingerprint_or_unspecified(resource),
                        ));
                        mutation
                            .commit_one(key, client.prepared_entry())
                            .await
                            .expect("older webhook commits");
                        older_runtime.insert(key.clone(), client);
                    });
                    *older_task_slot.lock() = Some(task);
                },
            )
            .expect("older source event is accepted");
        assert_eq!(older_sequence, 1);
        older_acquired_rx.await.expect("older task reports guard acquisition");

        let (newer_started_tx, newer_started_rx) = tokio::sync::oneshot::channel();
        let newer_slot: DispatchTaskSlot = Arc::new(Mutex::new(None));
        let newer_sequence = store
            .update_with_dispatch(
                HashMap::from([(
                    key.clone(),
                    create_test_webhook_linksys_with_port("ordered", "waiting", 8082),
                )]),
                &HashSet::new(),
                &counter,
                &source_lock,
                spawn_partial_dispatch(
                    manager.clone(),
                    runtime.clone(),
                    None,
                    Some(newer_started_tx),
                    newer_slot.clone(),
                ),
            )
            .expect("newer source event is accepted");
        assert_eq!(newer_sequence, 2);
        newer_started_rx.await.expect("newer task begins dispatch");
        tokio::task::yield_now().await;
        assert!(
            !newer_slot
                .lock()
                .as_ref()
                .expect("newer task was created")
                .is_finished(),
            "newer dispatch must wait while the older manager guard is held"
        );

        release_older_tx.send(()).expect("older task still waits for release");
        await_dispatch_task(&older_slot).await;
        await_dispatch_task(&newer_slot).await;

        assert_eq!(source_webhook_port(&store, &key), 8082);
        assert_callable_webhook_port(&manager, &runtime, &key, 8082).await;
    }

    #[test]
    fn ordered_otlp_type_change_removes_foreign_clients_before_unrelated_delta() {
        let _guard = I5_DISPATCH_LOCK.blocking_lock();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            for full in [false, true] {
                let manager = Arc::new(WebhookManager::new_for_test());
                let webhooks = Arc::new(ProviderRuntimeStore::new());
                let source = LinkSysStore::new();
                let source_lock = Mutex::new(());
                let counter = AtomicU64::new(1);
                let tail = Mutex::new(None);
                let key = "ordered/transition".to_owned();
                let old = create_test_webhook_linksys("ordered", "transition");
                dispatch_partial_update_with(
                    1,
                    &HashMap::from([(key.clone(), old)]),
                    &HashSet::new(),
                    &manager,
                    &webhooks,
                )
                .await;
                assert!(manager.get(&key).await.is_some());

                // Constructors do not connect. Populate every foreign typed map
                // without starting any networking or provider background worker.
                let redis =
                    serde_json::from_value(serde_json::json!({"endpoints": ["redis://127.0.0.1:6379"]})).unwrap();
                let etcd = serde_json::from_value(serde_json::json!({"endpoints": ["http://127.0.0.1:2379"]})).unwrap();
                let es = serde_json::from_value(serde_json::json!({"endpoints": ["http://127.0.0.1:9200"]})).unwrap();
                let kafka = serde_json::from_value(serde_json::json!({"brokers": ["127.0.0.1:9092"]})).unwrap();
                seed_published_redis(
                    key.clone(),
                    Arc::new(
                        RedisLinkClient::from_config(&key, &redis, &credential::CredentialState::NotRequired).unwrap(),
                    ),
                );
                seed_published_etcd(
                    key.clone(),
                    Arc::new(
                        EtcdLinkClient::from_config(&key, &etcd, &credential::CredentialState::NotRequired).unwrap(),
                    ),
                );
                seed_published_es(
                    key.clone(),
                    Arc::new(EsLinkClient::from_config(&key, &es, &credential::CredentialState::NotRequired).unwrap()),
                );
                seed_published_kafka(
                    key.clone(),
                    Arc::new(
                        KafkaLinkClient::from_config(&key, &kafka, &credential::CredentialState::NotRequired).unwrap(),
                    ),
                );

                let otlp: LinkSys = serde_json::from_value(serde_json::json!({
                    "apiVersion": "edgion.io/v1", "kind": "LinkSys",
                    "metadata": {"namespace": "ordered", "name": "transition"},
                    "spec": {"type": "otlp", "config": {"endpoint": "https://collector.example.test",
                        "auth": {"secretRef": {"name": "missing"}}}}
                }))
                .unwrap();
                let (release, gate) = tokio::sync::oneshot::channel();
                let earlier_manager = manager.clone();
                let earlier_webhooks = webhooks.clone();
                let enqueue = |sequence, data: Arc<LinkSysMap>| {
                    spawn_ordered_dispatch(&tokio::runtime::Handle::current(), &tail, async move {
                        gate.await.unwrap();
                        if full {
                            dispatch_full_set_with(sequence, &data, &earlier_manager, &earlier_webhooks, false).await;
                        } else {
                            dispatch_partial_update_with(
                                sequence,
                                &data,
                                &HashSet::new(),
                                &earlier_manager,
                                &earlier_webhooks,
                            )
                            .await;
                        }
                    });
                };
                let data = HashMap::from([(key.clone(), otlp)]);
                if full {
                    source
                        .replace_all_with_dispatch(data, &counter, &source_lock, enqueue)
                        .unwrap();
                } else {
                    source
                        .update_with_dispatch(data, &HashSet::new(), &counter, &source_lock, |sequence, data, _| {
                            enqueue(sequence, data)
                        })
                        .unwrap();
                }
                assert!(
                    OTLP_RUNTIME.get(&key).is_none(),
                    "OTLP apply must wait for the ordered generic dispatch"
                );
                let unrelated = HashMap::from([(
                    "ordered/unrelated".to_owned(),
                    create_test_webhook_linksys("ordered", "unrelated"),
                )]);
                let previous = tail.lock().take();
                let mut later = std::pin::pin!(dispatch_after(previous, async {
                    dispatch_partial_update_with(3, &unrelated, &HashSet::new(), &manager, &webhooks).await;
                }));
                // Force the later task to poll before the earlier task can enter
                // begin_mutation. Without the predecessor wait it discards N.
                assert!(futures::poll!(&mut later).is_pending());
                assert!(manager.get("ordered/unrelated").await.is_none());
                release.send(()).unwrap();
                later.await;
                assert!(get_redis_client(&key).is_none());
                assert!(get_etcd_client(&key).is_none());
                assert!(get_es_client(&key).is_none());
                assert!(get_kafka_client(&key).is_none());
                assert!(webhooks.get(&key).is_none());
                assert!(manager.get(&key).await.is_none());
                assert!(manager.get("ordered/unrelated").await.is_some());
                assert!(
                    OTLP_RUNTIME.get(&key).is_none(),
                    "unresolved OTLP auth must not create a runner"
                );

                // A later recreation must survive the earlier type cleanup.
                let recreated = HashMap::from([(
                    key.clone(),
                    create_test_webhook_linksys_with_port("ordered", "transition", 8181),
                )]);
                dispatch_partial_update_with(4, &recreated, &HashSet::new(), &manager, &webhooks).await;
                assert_callable_webhook_port(&manager, &webhooks, &key, 8181).await;
            }
        });
    }

    #[tokio::test]
    async fn ordered_dispatch_releases_successor_after_predecessor_failure() {
        let previous = tokio::spawn(async { panic!("test predecessor failure") });
        let ran = AtomicBool::new(false);
        dispatch_after(Some(previous), async {
            ran.store(true, Ordering::Relaxed);
        })
        .await;
        assert!(ran.load(Ordering::Relaxed));
    }

    // ============================================================
    // Credential-readiness-gated dispatch tests
    //
    // These tests mutate the process-global REDIS_RUNTIME. A full-set test
    // running concurrently with a partial-update test would wipe the map the
    // latter seeded, producing flaky failures. Serialize them through a
    // shared test Mutex, also used by webhook consumer tests. Keys are namespaced under `i5-ns/` to avoid
    // colliding with any future non-dispatch test.
    // ============================================================

    /// How the injected Secret (if any) is populated — controls the
    /// `CredentialState` that `basic_state(ls)` will compute.
    #[derive(Clone, Copy)]
    enum InjectedSecret {
        /// No Secret injected → `CredentialState::Pending`.
        Missing,
        /// Secret with a valid `password` key → `CredentialState::Resolved`.
        Valid,
        /// Secret present but missing the `password` key → the
        /// `BasicAuthResolver::parse` fails, producing
        /// `CredentialState::Invalid(InvalidReason::ParseFailed(_))`.
        MalformedForInvalid,
    }

    fn create_redis_linksys(namespace: &str, name: &str, inject: InjectedSecret) -> LinkSys {
        use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
        use edgion_resources::resources::link_sys::redis::{RedisAuth, RedisClientConfig};
        use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};
        use k8s_openapi::api::core::v1::Secret;
        use k8s_openapi::ByteString;
        use std::collections::BTreeMap;

        let secret = match inject {
            InjectedSecret::Missing => None,
            InjectedSecret::Valid => {
                let mut data = BTreeMap::new();
                data.insert("password".to_string(), ByteString(b"pw".to_vec()));
                Some(Secret {
                    data: Some(data),
                    ..Default::default()
                })
            }
            InjectedSecret::MalformedForInvalid => {
                let mut data = BTreeMap::new();
                data.insert("not-password".to_string(), ByteString(b"garbage".to_vec()));
                Some(Secret {
                    data: Some(data),
                    ..Default::default()
                })
            }
        };

        LinkSys {
            metadata: kube::core::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: LinkSysSpec {
                unknown_fields: Vec::new(),
                config: SystemConfig::Redis(RedisClientConfig {
                    endpoints: vec!["redis://127.0.0.1:6379".to_string()],
                    auth: Some(RedisAuth {
                        slot: SecretSlot {
                            secret_ref: SecretObjectReference {
                                group: None,
                                kind: None,
                                name: "creds".to_string(),
                                namespace: None,
                            },
                            secret,
                        },
                    }),
                    db: None,
                    timeout: None,
                    pool: None,
                    topology: None,
                    tls: None,
                }),
            },
            status: None,
        }
    }

    /// Full-set dispatch must not build a client for a LinkSys whose credential
    /// Secret has not been injected yet.
    #[tokio::test]
    async fn dispatch_full_set_skips_build_when_credential_not_ready() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/pending-full-set".to_string();
        let ls = create_redis_linksys("i5-ns", "pending-full-set", InjectedSecret::Missing);
        let mut data = HashMap::new();
        data.insert(key.clone(), ls);

        dispatch_full_set(next_dispatch_sequence(), &data, false).await;

        assert!(
            get_redis_client(&key).is_none(),
            "no client should be built for a pending-credential LinkSys"
        );
    }

    /// When a previously-published client exists and a subsequent full-set carries
    /// the same resource with pending credentials (Secret temporarily gone),
    /// the existing client is preserved rather than torn down.
    #[tokio::test]
    async fn dispatch_full_set_preserves_existing_client_on_pending_update() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/preserve-full-set".to_string();

        let ready = create_redis_linksys("i5-ns", "preserve-full-set", InjectedSecret::Valid);
        seed_published_redis(key.clone(), RedisLinkClient::from_linksys(&key, &ready).unwrap());
        let first = get_redis_client(&key).expect("seeded client should be published");

        let pending = create_redis_linksys("i5-ns", "preserve-full-set", InjectedSecret::Missing);
        let mut data = HashMap::new();
        data.insert(key.clone(), pending);
        dispatch_full_set(next_dispatch_sequence(), &data, false).await;

        let second = get_redis_client(&key).expect("client should be preserved on pending update");
        assert!(
            Arc::ptr_eq(&first, &second),
            "the preserved client must be the same Arc instance"
        );

        delete_redis_and_wait(&key).await;
    }

    /// Partial-update dispatch must not build a client on an add with pending
    /// credentials.
    #[tokio::test]
    async fn dispatch_partial_update_skips_build_when_credential_not_ready() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/pending-partial-add".to_string();
        let ls = create_redis_linksys("i5-ns", "pending-partial-add", InjectedSecret::Missing);
        let mut add = HashMap::new();
        add.insert(key.clone(), ls);

        dispatch_partial_update(next_dispatch_sequence(), &add, &HashSet::new()).await;

        assert!(
            get_redis_client(&key).is_none(),
            "no client should be built for a pending-credential partial add"
        );
    }

    /// Partial-update on an existing key that turns pending preserves the old
    /// client.
    #[tokio::test]
    async fn dispatch_partial_update_preserves_existing_client_on_pending_update() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/preserve-partial".to_string();

        let ready = create_redis_linksys("i5-ns", "preserve-partial", InjectedSecret::Valid);
        seed_published_redis(key.clone(), RedisLinkClient::from_linksys(&key, &ready).unwrap());
        let first = get_redis_client(&key).expect("seeded client should be published");

        let pending = create_redis_linksys("i5-ns", "preserve-partial", InjectedSecret::Missing);
        let mut add = HashMap::new();
        add.insert(key.clone(), pending);
        dispatch_partial_update(next_dispatch_sequence(), &add, &HashSet::new()).await;

        let second = get_redis_client(&key).expect("client should be preserved on pending update");
        assert!(
            Arc::ptr_eq(&first, &second),
            "the preserved client must be the same Arc instance"
        );

        delete_redis_and_wait(&key).await;
    }

    #[tokio::test]
    async fn dispatch_full_set_relist_reuses_runner_and_published_client() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/relist-reuse".to_string();
        let ready = create_redis_linksys("i5-ns", "relist-reuse", InjectedSecret::Valid);
        seed_published_redis(key.clone(), RedisLinkClient::from_linksys(&key, &ready).unwrap());
        let runner_first = REDIS_RUNTIME.get(&key).expect("seeded runner");
        let client_first = get_redis_client(&key).expect("seeded client");

        let mut data = HashMap::new();
        data.insert(key.clone(), ready);
        dispatch_full_set(next_dispatch_sequence(), &data, false).await;

        let runner_second = REDIS_RUNTIME.get(&key).expect("relist keeps runner");
        assert!(
            Arc::ptr_eq(&runner_first, &runner_second),
            "full-set relist must not spawn a second runner"
        );
        let client_second = get_redis_client(&key).expect("relist keeps published client");
        assert!(
            Arc::ptr_eq(&client_first, &client_second),
            "equal fingerprint relist must Reuse the small handle"
        );

        delete_redis_and_wait(&key).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dispatch_full_set_removes_old_kind_before_applying_new_kind() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/full-set-redis-to-kafka".to_string();
        let redis = create_redis_linksys("i5-ns", "full-set-redis-to-kafka", InjectedSecret::Valid);
        seed_published_redis(key.clone(), RedisLinkClient::from_linksys(&key, &redis).unwrap());
        assert!(get_redis_client(&key).is_some());

        // Hold the new provider's admission lock to observe the old provider
        // before Kafka can accept its apply, regardless of worker scheduling.
        let (lock_held_tx, lock_held_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let kafka_blocker = tokio::task::spawn_blocking(move || {
            let _guard = KAFKA_APPLY_LOCK.lock();
            lock_held_tx.send(()).expect("test waits for Kafka lock");
            release_rx.recv().expect("test releases Kafka lock");
        });
        lock_held_rx.await.expect("Kafka admission lock is held");
        let kafka = create_kafka_linksys("i5-ns", "full-set-redis-to-kafka");
        let dispatch = tokio::spawn(async move {
            dispatch_full_set(next_dispatch_sequence(), &HashMap::from([(key, kafka)]), false).await;
        });
        let key = "i5-ns/full-set-redis-to-kafka";
        let removed_before_kafka = tokio::time::timeout(tokio::time::Duration::from_secs(2), async {
            while REDIS_RUNTIME.get(key).is_some() {
                tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
            }
        })
        .await
        .is_ok();
        let old_client_removed = get_redis_client(key).is_none();
        release_tx.send(()).expect("Kafka blocker remains alive");
        kafka_blocker.await.expect("Kafka blocker completes");
        dispatch.await.expect("full-set dispatch completes");
        assert!(
            removed_before_kafka,
            "old Redis runner must be removed before Kafka apply"
        );
        assert!(
            old_client_removed,
            "old Redis client must be unavailable before Kafka apply"
        );

        delete_redis_and_wait(key).await;
        delete_kafka_and_wait(key).await;
    }

    #[tokio::test]
    async fn apply_redis_replays_last_write_after_deleting_window() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/pending-replay".to_string();
        let ls = create_redis_linksys("i5-ns", "pending-replay", InjectedSecret::Valid);
        REDIS_DELETING.lock().insert(key.clone());
        apply_redis(&key, &ls, false);
        assert!(
            REDIS_RUNTIME.get(&key).is_none(),
            "deleting window must not spawn a second runner"
        );
        let pending = REDIS_PENDING.lock().remove(&key);
        REDIS_DELETING.lock().remove(&key);
        let (pending_ls, forced) = pending.expect("last write during delete must be retained");
        if REDIS_RUNTIME.get(&key).is_none() {
            apply_redis(&key, &pending_ls, forced);
        }
        assert!(
            REDIS_RUNTIME.get(&key).is_some(),
            "join-complete replay must spawn the deferred Redis runner"
        );
        delete_redis_and_wait(&key).await;
    }

    #[tokio::test]
    async fn delete_redis_runner_drops_pending_so_kind_change_cannot_resurrect() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/pending-kind-change".to_string();
        let ls = create_redis_linksys("i5-ns", "pending-kind-change", InjectedSecret::Valid);
        REDIS_DELETING.lock().insert(key.clone());
        apply_redis(&key, &ls, false);
        assert!(REDIS_PENDING.lock().contains_key(&key));
        REDIS_DELETING.lock().remove(&key);
        delete_redis_and_wait(&key).await;
        assert!(
            REDIS_PENDING.lock().get(&key).is_none(),
            "kind-change delete must drop a deferred Redis apply"
        );
        assert!(REDIS_RUNTIME.get(&key).is_none());
    }

    #[tokio::test]
    async fn apply_redis_not_ready_during_delete_drops_pending() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/pending-not-ready".to_string();
        let ready = create_redis_linksys("i5-ns", "pending-not-ready", InjectedSecret::Valid);
        let pending = create_redis_linksys("i5-ns", "pending-not-ready", InjectedSecret::Missing);
        REDIS_DELETING.lock().insert(key.clone());
        apply_redis(&key, &ready, false);
        assert!(REDIS_PENDING.lock().contains_key(&key));
        apply_redis(&key, &pending, false);
        assert!(
            REDIS_PENDING.lock().get(&key).is_none(),
            "NotReady during delete must drop a deferred Redis apply"
        );
        REDIS_DELETING.lock().remove(&key);
    }

    #[tokio::test]
    async fn apply_redis_spawn_lists_unpublished_runner() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/spawn-unpublished".to_string();
        let ls = create_redis_linksys("i5-ns", "spawn-unpublished", InjectedSecret::Valid);
        let mut add = HashMap::new();
        add.insert(key.clone(), ls.clone());
        dispatch_partial_update(next_dispatch_sequence(), &add, &HashSet::new()).await;

        let runner_first = REDIS_RUNTIME.get(&key).expect("ready dispatch spawns a runner");
        assert!(
            get_redis_client(&key).is_none(),
            "get stays None until the first successful init"
        );
        assert!(
            list_redis_clients().iter().any(|listed| listed == &key),
            "D10: list includes connecting runner keys"
        );

        dispatch_partial_update(next_dispatch_sequence(), &add, &HashSet::new()).await;
        let runner_second = REDIS_RUNTIME.get(&key).expect("relist keeps runner");
        assert!(
            Arc::ptr_eq(&runner_first, &runner_second),
            "second ready dispatch must not spawn a second runner"
        );

        delete_redis_and_wait(&key).await;
    }

    async fn wait_deleting_idle(deleting: &parking_lot::Mutex<HashSet<String>>, key: &str) {
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(2);
        while deleting.lock().contains(key) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "delete join did not finish for {key}"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
        }
    }

    async fn delete_redis_and_wait(key: &str) {
        delete_redis_runner(key);
        wait_deleting_idle(&REDIS_DELETING, key).await;
    }

    async fn delete_etcd_and_wait(key: &str) {
        delete_etcd_runner(key);
        wait_deleting_idle(&ETCD_DELETING, key).await;
    }

    async fn delete_es_and_wait(key: &str) {
        delete_es_runner(key);
        wait_deleting_idle(&ES_DELETING, key).await;
    }

    async fn delete_kafka_and_wait(key: &str) {
        delete_kafka_runner(key);
        wait_deleting_idle(&KAFKA_DELETING, key).await;
    }

    async fn delete_otlp_and_wait(key: &str) {
        delete_otlp_runner(key);
        wait_deleting_idle(&OTLP_DELETING, key).await;
        otlp_retiring_client_count();
    }

    struct InstanceCapReset(usize);
    impl Drop for InstanceCapReset {
        fn drop(&mut self) {
            configure_link_sys_max_instances_per_kind(self.0);
        }
    }

    #[tokio::test]
    async fn apply_redis_refuses_new_key_at_instance_cap() {
        let _cap = LINK_SYS_CAP_TEST_LOCK.lock();
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let _reset = InstanceCapReset(get_link_sys_max_instances_per_kind());
        configure_link_sys_max_instances_per_kind(1);

        let live = "i5-ns/cap-live".to_string();
        let extra = "i5-ns/cap-extra".to_string();
        let live_ls = create_redis_linksys("i5-ns", "cap-live", InjectedSecret::Valid);
        let extra_ls = create_redis_linksys("i5-ns", "cap-extra", InjectedSecret::Valid);

        apply_redis(&live, &live_ls, false);
        assert!(REDIS_RUNTIME.get(&live).is_some(), "first key must spawn");

        apply_redis(&extra, &extra_ls, false);
        assert!(
            REDIS_RUNTIME.get(&extra).is_none(),
            "new key must be refused at the instance cap"
        );

        apply_redis(&live, &live_ls, false);
        assert!(
            REDIS_RUNTIME.get(&live).is_some(),
            "existing key updates must not be blocked by the cap"
        );

        delete_redis_and_wait(&live).await;
        delete_redis_and_wait(&extra).await;
    }

    #[tokio::test]
    async fn apply_redis_counts_deleting_joiners_against_instance_cap() {
        let _cap = LINK_SYS_CAP_TEST_LOCK.lock();
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let _reset = InstanceCapReset(get_link_sys_max_instances_per_kind());
        configure_link_sys_max_instances_per_kind(1);

        let extra = "i5-ns/cap-deleting-extra".to_string();
        let extra_ls = create_redis_linksys("i5-ns", "cap-deleting-extra", InjectedSecret::Valid);
        REDIS_DELETING
            .lock()
            .insert("i5-ns/cap-deleting-placeholder".to_string());
        apply_redis(&extra, &extra_ls, false);
        assert!(
            REDIS_RUNTIME.get(&extra).is_none(),
            "draining joiners must count toward the instance cap"
        );
        REDIS_DELETING.lock().remove("i5-ns/cap-deleting-placeholder");
        delete_redis_and_wait(&extra).await;
    }

    fn create_etcd_linksys(namespace: &str, name: &str, inject: InjectedSecret) -> LinkSys {
        use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
        use edgion_resources::resources::link_sys::etcd::{EtcdAuth, EtcdClientConfig};
        use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};
        use k8s_openapi::api::core::v1::Secret;
        use k8s_openapi::ByteString;
        use std::collections::BTreeMap;

        let secret = match inject {
            InjectedSecret::Missing => None,
            InjectedSecret::Valid => {
                let mut data = BTreeMap::new();
                data.insert("password".to_string(), ByteString(b"pw".to_vec()));
                Some(Secret {
                    data: Some(data),
                    ..Default::default()
                })
            }
            InjectedSecret::MalformedForInvalid => {
                let mut data = BTreeMap::new();
                data.insert("not-password".to_string(), ByteString(b"garbage".to_vec()));
                Some(Secret {
                    data: Some(data),
                    ..Default::default()
                })
            }
        };

        LinkSys {
            metadata: kube::core::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: LinkSysSpec {
                unknown_fields: Vec::new(),
                config: SystemConfig::Etcd(EtcdClientConfig {
                    endpoints: vec!["http://127.0.0.1:2379".to_string()],
                    auth: Some(EtcdAuth {
                        slot: SecretSlot {
                            secret_ref: SecretObjectReference {
                                group: None,
                                kind: None,
                                name: "creds".to_string(),
                                namespace: None,
                            },
                            secret,
                        },
                    }),
                    tls: None,
                    timeout: None,
                    keep_alive: None,
                    namespace: None,
                    auto_sync_interval: None,
                    max_call_send_size: None,
                    max_call_recv_size: None,
                    user_agent: None,
                    reject_old_cluster: None,
                    observability: None,
                }),
            },
            status: None,
        }
    }

    fn seed_etcd_from_ls(key: String, ls: &LinkSys) {
        use crate::link_sys::runtime::runner::ApplyOwner;
        seed_published_etcd(key.clone(), EtcdApply::build(&key, ls).expect("etcd seed"));
    }

    #[tokio::test]
    async fn dispatch_full_set_preserves_existing_etcd_client_on_pending_update() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/etcd-preserve-full-set".to_string();

        let ready = create_etcd_linksys("i5-ns", "etcd-preserve-full-set", InjectedSecret::Valid);
        seed_etcd_from_ls(key.clone(), &ready);
        let first = get_etcd_client(&key).expect("seeded etcd client should be published");

        let pending = create_etcd_linksys("i5-ns", "etcd-preserve-full-set", InjectedSecret::Missing);
        let mut data = HashMap::new();
        data.insert(key.clone(), pending);
        dispatch_full_set(next_dispatch_sequence(), &data, false).await;

        let second = get_etcd_client(&key).expect("etcd client should be preserved on pending update");
        assert!(
            Arc::ptr_eq(&first, &second),
            "the preserved etcd client must be the same Arc instance"
        );

        delete_etcd_and_wait(&key).await;
    }

    #[tokio::test]
    async fn dispatch_full_set_relist_reuses_etcd_runner_and_published_client() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/etcd-relist-reuse".to_string();
        let ready = create_etcd_linksys("i5-ns", "etcd-relist-reuse", InjectedSecret::Valid);
        seed_etcd_from_ls(key.clone(), &ready);
        let runner_first = ETCD_RUNTIME.get(&key).expect("seeded etcd runner");
        let client_first = get_etcd_client(&key).expect("seeded etcd client");

        let mut data = HashMap::new();
        data.insert(key.clone(), ready);
        dispatch_full_set(next_dispatch_sequence(), &data, false).await;

        let runner_second = ETCD_RUNTIME.get(&key).expect("relist keeps etcd runner");
        assert!(
            Arc::ptr_eq(&runner_first, &runner_second),
            "full-set relist must not spawn a second etcd runner"
        );
        let client_second = get_etcd_client(&key).expect("relist keeps published etcd client");
        assert!(
            Arc::ptr_eq(&client_first, &client_second),
            "equal fingerprint relist must Reuse the etcd small handle"
        );

        delete_etcd_and_wait(&key).await;
    }

    #[tokio::test]
    async fn apply_etcd_replays_last_write_after_deleting_window() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/etcd-pending-replay".to_string();
        let ls = create_etcd_linksys("i5-ns", "etcd-pending-replay", InjectedSecret::Valid);
        ETCD_DELETING.lock().insert(key.clone());
        apply_etcd(&key, &ls, false);
        assert!(
            ETCD_RUNTIME.get(&key).is_none(),
            "deleting window must not spawn a second etcd runner"
        );
        let pending = ETCD_PENDING.lock().remove(&key);
        ETCD_DELETING.lock().remove(&key);
        let (pending_ls, forced) = pending.expect("last write during delete must be retained");
        if ETCD_RUNTIME.get(&key).is_none() {
            apply_etcd(&key, &pending_ls, forced);
        }
        assert!(
            ETCD_RUNTIME.get(&key).is_some(),
            "join-complete replay must spawn the deferred Etcd runner"
        );
        delete_etcd_and_wait(&key).await;
    }

    #[tokio::test]
    async fn delete_etcd_runner_drops_pending_so_kind_change_cannot_resurrect() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/etcd-pending-kind-change".to_string();
        let ls = create_etcd_linksys("i5-ns", "etcd-pending-kind-change", InjectedSecret::Valid);
        ETCD_DELETING.lock().insert(key.clone());
        apply_etcd(&key, &ls, false);
        assert!(ETCD_PENDING.lock().contains_key(&key));
        ETCD_DELETING.lock().remove(&key);
        delete_etcd_and_wait(&key).await;
        assert!(
            ETCD_PENDING.lock().get(&key).is_none(),
            "kind-change delete must drop a deferred Etcd apply"
        );
        assert!(ETCD_RUNTIME.get(&key).is_none());
    }

    #[tokio::test]
    async fn apply_etcd_not_ready_during_delete_drops_pending() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/etcd-pending-not-ready".to_string();
        let ready = create_etcd_linksys("i5-ns", "etcd-pending-not-ready", InjectedSecret::Valid);
        let pending = create_etcd_linksys("i5-ns", "etcd-pending-not-ready", InjectedSecret::Missing);
        ETCD_DELETING.lock().insert(key.clone());
        apply_etcd(&key, &ready, false);
        assert!(ETCD_PENDING.lock().contains_key(&key));
        apply_etcd(&key, &pending, false);
        assert!(
            ETCD_PENDING.lock().get(&key).is_none(),
            "NotReady during delete must drop a deferred Etcd apply"
        );
        ETCD_DELETING.lock().remove(&key);
    }

    #[tokio::test]
    async fn apply_etcd_spawn_lists_unpublished_runner() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/etcd-spawn-unpublished".to_string();
        let ls = create_etcd_linksys("i5-ns", "etcd-spawn-unpublished", InjectedSecret::Valid);
        let mut add = HashMap::new();
        add.insert(key.clone(), ls.clone());
        dispatch_partial_update(next_dispatch_sequence(), &add, &HashSet::new()).await;

        let runner_first = ETCD_RUNTIME.get(&key).expect("ready dispatch spawns an etcd runner");
        assert!(
            get_etcd_client(&key).is_none(),
            "get stays None until the first successful etcd init"
        );
        assert!(
            list_etcd_clients().iter().any(|listed| listed == &key),
            "D10: list includes connecting etcd runner keys"
        );
        assert!(etcd_runner_exists(&key));

        dispatch_partial_update(next_dispatch_sequence(), &add, &HashSet::new()).await;
        let runner_second = ETCD_RUNTIME.get(&key).expect("relist keeps etcd runner");
        assert!(
            Arc::ptr_eq(&runner_first, &runner_second),
            "second ready dispatch must not spawn a second etcd runner"
        );

        delete_etcd_and_wait(&key).await;
    }

    #[tokio::test]
    async fn apply_etcd_refuses_new_key_at_instance_cap() {
        let _cap = LINK_SYS_CAP_TEST_LOCK.lock();
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let _reset = InstanceCapReset(get_link_sys_max_instances_per_kind());
        configure_link_sys_max_instances_per_kind(1);

        let live = "i5-ns/etcd-cap-live".to_string();
        let extra = "i5-ns/etcd-cap-extra".to_string();
        let live_ls = create_etcd_linksys("i5-ns", "etcd-cap-live", InjectedSecret::Valid);
        let extra_ls = create_etcd_linksys("i5-ns", "etcd-cap-extra", InjectedSecret::Valid);

        apply_etcd(&live, &live_ls, false);
        assert!(ETCD_RUNTIME.get(&live).is_some(), "first etcd key must spawn");

        apply_etcd(&extra, &extra_ls, false);
        assert!(
            ETCD_RUNTIME.get(&extra).is_none(),
            "new etcd key must be refused at the instance cap"
        );

        apply_etcd(&live, &live_ls, false);
        assert!(
            ETCD_RUNTIME.get(&live).is_some(),
            "existing etcd key updates must not be blocked by the cap"
        );

        delete_etcd_runner(&live);
        delete_etcd_runner(&extra);
    }

    #[tokio::test]
    async fn apply_etcd_counts_deleting_joiners_against_instance_cap() {
        let _cap = LINK_SYS_CAP_TEST_LOCK.lock();
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let _reset = InstanceCapReset(get_link_sys_max_instances_per_kind());
        configure_link_sys_max_instances_per_kind(1);

        let extra = "i5-ns/etcd-cap-deleting-extra".to_string();
        let extra_ls = create_etcd_linksys("i5-ns", "etcd-cap-deleting-extra", InjectedSecret::Valid);
        ETCD_DELETING
            .lock()
            .insert("i5-ns/etcd-cap-deleting-placeholder".to_string());
        apply_etcd(&extra, &extra_ls, false);
        assert!(
            ETCD_RUNTIME.get(&extra).is_none(),
            "draining etcd joiners must count toward the instance cap"
        );
        ETCD_DELETING.lock().remove("i5-ns/etcd-cap-deleting-placeholder");
        delete_etcd_and_wait(&extra).await;
    }

    fn create_es_linksys(namespace: &str, name: &str, inject: InjectedSecret) -> LinkSys {
        use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
        use edgion_resources::resources::link_sys::elasticsearch::{ElasticsearchAuth, ElasticsearchClientConfig};
        use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};
        use k8s_openapi::api::core::v1::Secret;
        use k8s_openapi::ByteString;
        use std::collections::BTreeMap;

        let secret = match inject {
            InjectedSecret::Missing => None,
            InjectedSecret::Valid => {
                let mut data = BTreeMap::new();
                data.insert("password".to_string(), ByteString(b"pw".to_vec()));
                Some(Secret {
                    data: Some(data),
                    ..Default::default()
                })
            }
            InjectedSecret::MalformedForInvalid => {
                let mut data = BTreeMap::new();
                data.insert("not-password".to_string(), ByteString(b"garbage".to_vec()));
                Some(Secret {
                    data: Some(data),
                    ..Default::default()
                })
            }
        };

        LinkSys {
            metadata: kube::core::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: LinkSysSpec {
                unknown_fields: Vec::new(),
                config: SystemConfig::Elasticsearch(ElasticsearchClientConfig {
                    endpoints: vec!["http://127.0.0.1:9200".to_string()],
                    auth: Some(ElasticsearchAuth::Basic {
                        slot: SecretSlot {
                            secret_ref: SecretObjectReference {
                                group: None,
                                kind: None,
                                name: "creds".to_string(),
                                namespace: None,
                            },
                            secret,
                        },
                    }),
                    tls: None,
                    timeout: None,
                    pool: None,
                    bulk: None,
                    index: None,
                }),
            },
            status: None,
        }
    }

    fn seed_es_from_ls(key: String, ls: &LinkSys) {
        use crate::link_sys::runtime::runner::ApplyOwner;
        seed_published_es(key.clone(), EsApply::build(&key, ls).expect("es seed"));
    }

    #[tokio::test]
    async fn dispatch_full_set_relist_reuses_es_runner_and_published_client() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/es-relist-reuse".to_string();
        let ready = create_es_linksys("i5-ns", "es-relist-reuse", InjectedSecret::Valid);
        seed_es_from_ls(key.clone(), &ready);
        let runner_first = ES_RUNTIME.get(&key).expect("seeded es runner");
        let client_first = get_es_client(&key).expect("seeded es client");

        let mut data = HashMap::new();
        data.insert(key.clone(), ready);
        dispatch_full_set(next_dispatch_sequence(), &data, false).await;

        let runner_second = ES_RUNTIME.get(&key).expect("relist keeps es runner");
        assert!(
            Arc::ptr_eq(&runner_first, &runner_second),
            "full-set relist must not spawn a second es runner"
        );
        let client_second = get_es_client(&key).expect("relist keeps published es client");
        assert!(
            Arc::ptr_eq(&client_first, &client_second),
            "equal fingerprint relist must Reuse the es small handle"
        );

        delete_es_and_wait(&key).await;
    }

    #[tokio::test]
    async fn apply_es_spawn_lists_unpublished_runner() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/es-spawn-unpublished".to_string();
        let ls = create_es_linksys("i5-ns", "es-spawn-unpublished", InjectedSecret::Valid);
        let mut add = HashMap::new();
        add.insert(key.clone(), ls.clone());
        dispatch_partial_update(next_dispatch_sequence(), &add, &HashSet::new()).await;

        let runner_first = ES_RUNTIME.get(&key).expect("ready dispatch spawns an es runner");
        assert!(
            get_es_client(&key).is_none(),
            "get stays None until the first successful es init"
        );
        assert!(es_runner_exists(&key));

        dispatch_partial_update(next_dispatch_sequence(), &add, &HashSet::new()).await;
        let runner_second = ES_RUNTIME.get(&key).expect("relist keeps es runner");
        assert!(
            Arc::ptr_eq(&runner_first, &runner_second),
            "second ready dispatch must not spawn a second es runner"
        );

        delete_es_and_wait(&key).await;
    }

    #[tokio::test]
    async fn apply_es_refuses_new_key_at_instance_cap() {
        let _cap = LINK_SYS_CAP_TEST_LOCK.lock();
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let _reset = InstanceCapReset(get_link_sys_max_instances_per_kind());
        configure_link_sys_max_instances_per_kind(1);

        let live = "i5-ns/es-cap-live".to_string();
        let extra = "i5-ns/es-cap-extra".to_string();
        let live_ls = create_es_linksys("i5-ns", "es-cap-live", InjectedSecret::Valid);
        let extra_ls = create_es_linksys("i5-ns", "es-cap-extra", InjectedSecret::Valid);

        apply_es(&live, &live_ls, false);
        assert!(ES_RUNTIME.get(&live).is_some(), "first es key must spawn");

        apply_es(&extra, &extra_ls, false);
        assert!(
            ES_RUNTIME.get(&extra).is_none(),
            "new es key must be refused at the instance cap"
        );

        apply_es(&live, &live_ls, false);
        assert!(
            ES_RUNTIME.get(&live).is_some(),
            "existing es key updates must not be blocked by the cap"
        );

        delete_es_and_wait(&live).await;
        delete_es_and_wait(&extra).await;
    }

    fn create_kafka_linksys(namespace: &str, name: &str) -> LinkSys {
        use edgion_resources::resources::link_sys::kafka::KafkaClientConfig;
        use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};

        LinkSys {
            metadata: kube::core::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: LinkSysSpec {
                unknown_fields: Vec::new(),
                config: SystemConfig::Kafka(KafkaClientConfig {
                    brokers: vec!["127.0.0.1:9092".to_string()],
                    sasl: None,
                    tls: None,
                    channel_size: None,
                    max_topics: None,
                    max_pending_records: None,
                    max_pending_bytes: None,
                    linger_ms: None,
                }),
            },
            status: None,
        }
    }

    fn create_kafka_linksys_sasl(namespace: &str, name: &str, inject: InjectedSecret) -> LinkSys {
        use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
        use edgion_resources::resources::link_sys::kafka::{KafkaClientConfig, KafkaSaslConfig};
        use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};
        use k8s_openapi::api::core::v1::Secret;
        use k8s_openapi::ByteString;
        use std::collections::BTreeMap;

        let secret = match inject {
            InjectedSecret::Missing => None,
            InjectedSecret::Valid => {
                let mut data = BTreeMap::new();
                data.insert("password".to_string(), ByteString(b"pw".to_vec()));
                Some(Secret {
                    data: Some(data),
                    ..Default::default()
                })
            }
            InjectedSecret::MalformedForInvalid => {
                let mut data = BTreeMap::new();
                data.insert("not-password".to_string(), ByteString(b"garbage".to_vec()));
                Some(Secret {
                    data: Some(data),
                    ..Default::default()
                })
            }
        };

        LinkSys {
            metadata: kube::core::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: LinkSysSpec {
                unknown_fields: Vec::new(),
                config: SystemConfig::Kafka(KafkaClientConfig {
                    brokers: vec!["127.0.0.1:9092".to_string()],
                    sasl: Some(KafkaSaslConfig {
                        username: "alice".to_string(),
                        password: SecretSlot {
                            secret_ref: SecretObjectReference {
                                group: None,
                                kind: None,
                                name: "creds".to_string(),
                                namespace: None,
                            },
                            secret,
                        },
                    }),
                    tls: None,
                    channel_size: None,
                    max_topics: None,
                    max_pending_records: None,
                    max_pending_bytes: None,
                    linger_ms: None,
                }),
            },
            status: None,
        }
    }

    fn seed_kafka_from_ls(key: String, ls: &LinkSys) {
        use crate::link_sys::runtime::runner::ApplyOwner;
        seed_published_kafka(key.clone(), KafkaApply::build(&key, ls).expect("kafka seed"));
    }

    #[tokio::test]
    async fn dispatch_full_set_relist_reuses_kafka_runner_and_published_client() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/kafka-relist-reuse".to_string();
        let ready = create_kafka_linksys("i5-ns", "kafka-relist-reuse");
        seed_kafka_from_ls(key.clone(), &ready);
        let runner_first = KAFKA_RUNTIME.get(&key).expect("seeded kafka runner");
        let client_first = get_kafka_client(&key).expect("seeded kafka client");

        let mut data = HashMap::new();
        data.insert(key.clone(), ready);
        dispatch_full_set(next_dispatch_sequence(), &data, false).await;

        let runner_second = KAFKA_RUNTIME.get(&key).expect("relist keeps kafka runner");
        assert!(
            Arc::ptr_eq(&runner_first, &runner_second),
            "full-set relist must not spawn a second kafka runner"
        );
        let client_second = get_kafka_client(&key).expect("relist keeps published kafka client");
        assert!(
            Arc::ptr_eq(&client_first, &client_second),
            "equal fingerprint relist must Reuse the kafka small handle"
        );

        delete_kafka_and_wait(&key).await;
    }

    #[tokio::test]
    async fn apply_kafka_spawn_lists_unpublished_runner() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/kafka-spawn-unpublished".to_string();
        let ls = create_kafka_linksys("i5-ns", "kafka-spawn-unpublished");
        let mut add = HashMap::new();
        add.insert(key.clone(), ls.clone());
        dispatch_partial_update(next_dispatch_sequence(), &add, &HashSet::new()).await;

        let runner_first = KAFKA_RUNTIME.get(&key).expect("ready dispatch spawns a kafka runner");
        assert!(
            get_kafka_client(&key).is_none(),
            "get stays None until the first successful kafka init"
        );
        assert!(
            list_kafka_clients().iter().any(|listed| listed == &key),
            "D10: list includes connecting kafka runner keys"
        );
        assert!(kafka_runner_exists(&key));

        dispatch_partial_update(next_dispatch_sequence(), &add, &HashSet::new()).await;
        let runner_second = KAFKA_RUNTIME.get(&key).expect("relist keeps kafka runner");
        assert!(
            Arc::ptr_eq(&runner_first, &runner_second),
            "second ready dispatch must not spawn a second kafka runner"
        );

        delete_kafka_and_wait(&key).await;
    }

    #[tokio::test]
    async fn apply_kafka_refuses_new_key_at_instance_cap() {
        let _cap = LINK_SYS_CAP_TEST_LOCK.lock();
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let _reset = InstanceCapReset(get_link_sys_max_instances_per_kind());
        configure_link_sys_max_instances_per_kind(1);

        let live = "i5-ns/kafka-cap-live".to_string();
        let extra = "i5-ns/kafka-cap-extra".to_string();
        let live_ls = create_kafka_linksys("i5-ns", "kafka-cap-live");
        let extra_ls = create_kafka_linksys("i5-ns", "kafka-cap-extra");

        apply_kafka(&live, &live_ls, false);
        assert!(KAFKA_RUNTIME.get(&live).is_some(), "first kafka key must spawn");

        apply_kafka(&extra, &extra_ls, false);
        assert!(
            KAFKA_RUNTIME.get(&extra).is_none(),
            "new kafka key must be refused at the instance cap"
        );

        apply_kafka(&live, &live_ls, false);
        assert!(
            KAFKA_RUNTIME.get(&live).is_some(),
            "existing kafka key updates must not be blocked by the cap"
        );

        delete_kafka_and_wait(&live).await;
        delete_kafka_and_wait(&extra).await;
    }

    #[tokio::test]
    async fn apply_kafka_not_ready_during_delete_drops_pending() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/kafka-pending-not-ready".to_string();
        let ready = create_kafka_linksys_sasl("i5-ns", "kafka-pending-not-ready", InjectedSecret::Valid);
        let pending = create_kafka_linksys_sasl("i5-ns", "kafka-pending-not-ready", InjectedSecret::Missing);
        KAFKA_DELETING.lock().insert(key.clone());
        apply_kafka(&key, &ready, false);
        assert!(KAFKA_PENDING.lock().contains_key(&key));
        apply_kafka(&key, &pending, false);
        assert!(
            KAFKA_PENDING.lock().get(&key).is_none(),
            "NotReady during delete must drop a deferred Kafka apply"
        );
        KAFKA_DELETING.lock().remove(&key);
    }

    #[tokio::test]
    async fn retain_kafka_runners_drops_pending_for_absent_deleting_key() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/kafka-retain-pending".to_string();
        let ls = create_kafka_linksys("i5-ns", "kafka-retain-pending");
        KAFKA_DELETING.lock().insert(key.clone());
        apply_kafka(&key, &ls, false);
        assert!(KAFKA_PENDING.lock().contains_key(&key));
        retain_kafka_runners(&HashSet::new());
        assert!(
            KAFKA_PENDING.lock().get(&key).is_none(),
            "full-set retain must drop a deferred Kafka apply when the key is absent"
        );
        KAFKA_DELETING.lock().remove(&key);
    }

    fn create_otlp_linksys(namespace: &str, name: &str, auth: InjectedSecret) -> LinkSys {
        use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
        use edgion_resources::resources::link_sys::otlp::{OtlpAuth, OtlpClientConfig};
        use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};
        use k8s_openapi::api::core::v1::Secret;
        use k8s_openapi::ByteString;
        use std::collections::BTreeMap;

        let secret = match auth {
            InjectedSecret::Missing => None,
            InjectedSecret::Valid => Some(Secret {
                data: Some(BTreeMap::from([(
                    "bearerToken".to_string(),
                    ByteString(b"token".to_vec()),
                )])),
                ..Default::default()
            }),
            InjectedSecret::MalformedForInvalid => Some(Secret {
                data: Some(BTreeMap::from([("wrong".to_string(), ByteString(b"value".to_vec()))])),
                ..Default::default()
            }),
        };

        LinkSys {
            metadata: kube::core::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: LinkSysSpec {
                unknown_fields: Vec::new(),
                config: SystemConfig::Otlp(OtlpClientConfig {
                    endpoint: "http://192.0.2.1:4317".to_string(),
                    timeout_ms: 10_000,
                    auth: Some(OtlpAuth {
                        slot: SecretSlot {
                            secret_ref: SecretObjectReference {
                                group: None,
                                kind: None,
                                name: "auth".to_string(),
                                namespace: None,
                            },
                            secret,
                        },
                    }),
                    tls: None,
                }),
            },
            status: None,
        }
    }

    fn seed_otlp_from_ls(key: String, ls: &LinkSys) {
        use crate::link_sys::runtime::runner::ApplyOwner;

        let client = OtlpApply::build(&key, ls).expect("OTLP seed");
        client.publish_lazy_test_pipeline().expect("OTLP lazy pipeline");
        seed_published_otlp(key, client);
    }

    #[tokio::test]
    async fn dispatch_full_set_relist_reuses_otlp_runner_and_published_client() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/otlp-relist-reuse".to_string();
        let ready = create_otlp_linksys("i5-ns", "otlp-relist-reuse", InjectedSecret::Valid);
        seed_otlp_from_ls(key.clone(), &ready);
        let runner_first = OTLP_RUNTIME.get(&key).expect("seeded OTLP runner");
        let client_first = get_otlp_client(&key).expect("seeded OTLP client");

        dispatch_full_set(next_dispatch_sequence(), &HashMap::from([(key.clone(), ready)]), false).await;

        let runner_second = OTLP_RUNTIME.get(&key).expect("relist keeps OTLP runner");
        assert!(Arc::ptr_eq(&runner_first, &runner_second));
        let client_second = get_otlp_client(&key).expect("relist keeps OTLP client");
        assert!(
            Arc::ptr_eq(&client_first, &client_second),
            "equal fingerprint relist must reuse the OTLP exporter"
        );
        delete_otlp_and_wait(&key).await;
    }

    #[tokio::test]
    async fn apply_otlp_lists_unpublished_runner_and_get_stays_none() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/otlp-unpublished".to_string();
        let ready = create_otlp_linksys("i5-ns", "otlp-unpublished", InjectedSecret::Valid);
        apply_otlp(&key, &ready, false);

        assert!(OTLP_RUNTIME.get(&key).is_some());
        assert!(list_otlp_clients().iter().any(|listed| listed == &key));
        assert!(
            get_otlp_client(&key).is_none(),
            "D14: get stays None until empty Export succeeds"
        );
        delete_otlp_and_wait(&key).await;
    }

    #[tokio::test]
    async fn apply_otlp_counts_deleting_joiners_toward_instance_cap() {
        let _cap = LINK_SYS_CAP_TEST_LOCK.lock();
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let _reset = InstanceCapReset(get_link_sys_max_instances_per_kind());
        configure_link_sys_max_instances_per_kind(1);
        let deleting = "i5-ns/otlp-cap-deleting".to_string();
        let extra = "i5-ns/otlp-cap-extra".to_string();
        OTLP_DELETING.lock().insert(deleting.clone());

        let ready = create_otlp_linksys("i5-ns", "otlp-cap-extra", InjectedSecret::Valid);
        apply_otlp(&extra, &ready, false);
        assert!(
            OTLP_RUNTIME.get(&extra).is_none(),
            "deleting OTLP joiners must consume instance capacity"
        );
        OTLP_DELETING.lock().remove(&deleting);
    }

    #[tokio::test]
    async fn apply_otlp_not_ready_during_delete_drops_pending() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/otlp-pending-not-ready".to_string();
        let ready = create_otlp_linksys("i5-ns", "otlp-pending-not-ready", InjectedSecret::Valid);
        let pending = create_otlp_linksys("i5-ns", "otlp-pending-not-ready", InjectedSecret::Missing);
        OTLP_DELETING.lock().insert(key.clone());
        apply_otlp(&key, &ready, false);
        assert!(OTLP_PENDING.lock().contains_key(&key));
        apply_otlp(&key, &pending, false);
        assert!(
            OTLP_PENDING.lock().get(&key).is_none(),
            "NotReady during delete must drop a deferred OTLP apply"
        );
        OTLP_DELETING.lock().remove(&key);
    }

    /// An `Invalid` credential state must also block client builds, symmetric
    /// to the `Pending` case. Both share `is_buildable() == false`, but a
    /// separate test defends against a future refactor that accidentally treats
    /// them differently at the dispatch layer.
    #[tokio::test]
    async fn dispatch_full_set_skips_build_when_credential_invalid() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/invalid-full-set".to_string();
        let ls = create_redis_linksys("i5-ns", "invalid-full-set", InjectedSecret::MalformedForInvalid);
        let mut data = HashMap::new();
        data.insert(key.clone(), ls);

        dispatch_full_set(next_dispatch_sequence(), &data, false).await;

        assert!(
            get_redis_client(&key).is_none(),
            "no client should be built for an invalid-credential LinkSys"
        );
    }

    /// Partial-update add with `Invalid` credential state must also be filtered
    /// out.
    #[tokio::test]
    async fn dispatch_partial_update_skips_build_when_credential_invalid() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/invalid-partial-add".to_string();
        let ls = create_redis_linksys("i5-ns", "invalid-partial-add", InjectedSecret::MalformedForInvalid);
        let mut add = HashMap::new();
        add.insert(key.clone(), ls);

        dispatch_partial_update(next_dispatch_sequence(), &add, &HashSet::new()).await;

        assert!(
            get_redis_client(&key).is_none(),
            "no client should be built for an invalid-credential partial add"
        );
    }

    #[test]
    fn webhook_with_unresolved_secret_ref_is_not_ready() {
        use edgion_resources::resources::link_sys::webhook::{
            CustomField, CustomSource, ListFields, WebhookOutboundRequest, WebhookServiceConfig,
        };
        let mut config = WebhookServiceConfig::default();
        config.conn.target.service.name = Some("svc".to_string());
        config.conn.target.service.port = Some(8080);
        config.request = WebhookOutboundRequest {
            headers: ListFields {
                origin: vec![],
                custom: vec![CustomField {
                    name: "Authorization".to_string(),
                    source: CustomSource::Template("Bearer ${secretRef:ns/tok:token}".to_string()),
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        // resolved_secrets is None → the ref is unresolved.
        assert!(!config.unresolved_secret_refs().is_empty());
        assert!(
            !webhook_secrets_ready(&config),
            "must be not-ready when refs unresolved"
        );
    }

    #[tokio::test]
    async fn dispatch_full_set_relist_reuses_webhook_registration() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "i5-ns/webhook-relist-reuse".to_string();
        let ls = create_test_webhook_linksys("i5-ns", "webhook-relist-reuse");
        let mut data = HashMap::new();
        data.insert(key.clone(), ls);
        dispatch_full_set(next_dispatch_sequence(), &data, false).await;

        let first = WEBHOOK_RUNTIME.get(&key).expect("first webhook publish");
        let first_entry = first.prepared_entry();
        dispatch_full_set(next_dispatch_sequence(), &data, false).await;
        let second = WEBHOOK_RUNTIME.get(&key).expect("relist webhook");
        assert!(
            Arc::ptr_eq(&first, &second),
            "equal fingerprint relist must keep the webhook runtime Arc"
        );
        let second_entry = second.prepared_entry();
        assert!(
            Arc::ptr_eq(first_entry.registration(), second_entry.registration()),
            "equal fingerprint relist must not prepare a new webhook registration"
        );

        let mut remove = HashSet::new();
        remove.insert(key);
        dispatch_partial_update(next_dispatch_sequence(), &HashMap::new(), &remove).await;
    }

    #[tokio::test]
    async fn dispatch_partial_update_replace_webhook_keeps_runtime_arc_swap() {
        use edgion_resources::resources::link_sys::webhook::WebhookServiceConfig;
        use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};

        // Share a global lock with other dispatch tests to serialize. Without
        // holding the lock, their `dispatch_full_set` calls would wipe the key
        // inserted by this test via an empty new_webhook_map through
        // `WEBHOOK_RUNTIME.replace_all`.
        let _guard = I5_DISPATCH_LOCK.lock().await;

        // Use a unique key to also avoid interference with other tests' add/remove paths.
        let key = "t13-ns/wh-arc-swap".to_string();

        let make_ls = |port: i32| {
            use edgion_resources::resources::link_sys::webhook::{
                ExternalHttpConnection, WebhookServiceRef, WebhookTarget,
            };
            LinkSys {
                metadata: kube::core::ObjectMeta {
                    namespace: Some("t13-ns".into()),
                    name: Some("wh-arc-swap".into()),
                    ..Default::default()
                },
                spec: LinkSysSpec {
                    unknown_fields: Vec::new(),
                    config: SystemConfig::Webhook(WebhookServiceConfig {
                        conn: ExternalHttpConnection {
                            target: WebhookTarget {
                                service: WebhookServiceRef {
                                    kind: None,
                                    name: Some("webhook-svc".to_string()),
                                    namespace: Some("t13-ns".to_string()),
                                    port: Some(port),
                                    ..Default::default()
                                },
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                },
                status: None,
            }
        };

        // Add
        let mut add = HashMap::new();
        add.insert(key.clone(), make_ls(8080));
        dispatch_partial_update(next_dispatch_sequence(), &add, &HashSet::new()).await;
        // The prepared commit publishes the manager before atomically updating
        // WEBHOOK_RUNTIME, so the Arc is visible when dispatch returns.
        let arc1 = WEBHOOK_RUNTIME.get(&key);
        assert!(arc1.is_some(), "first dispatch must populate WEBHOOK_RUNTIME");
        let manager1 = get_webhook_manager()
            .get(&key)
            .await
            .expect("first dispatch must populate WebhookManager");
        assert!(
            Arc::ptr_eq(arc1.as_ref().unwrap().prepared_entry().client(), &manager1),
            "runtime bookkeeping and callable manager must publish the same client"
        );

        // Update (different port) — a new Arc should replace the old one.
        // Equal-fingerprint relists skip prepare; a rebuild still prepare/commits.
        let mut update = HashMap::new();
        update.insert(key.clone(), make_ls(8081));
        dispatch_partial_update(next_dispatch_sequence(), &update, &HashSet::new()).await;
        let arc2 = WEBHOOK_RUNTIME.get(&key);
        assert!(arc2.is_some(), "after update, WEBHOOK_RUNTIME must still hold a client");
        assert!(
            !Arc::ptr_eq(arc1.as_ref().unwrap(), arc2.as_ref().unwrap()),
            "update must produce a fresh Arc"
        );
        let manager2 = get_webhook_manager()
            .get(&key)
            .await
            .expect("updated manager registration must remain callable");
        assert!(
            Arc::ptr_eq(arc2.as_ref().unwrap().prepared_entry().client(), &manager2),
            "manager and runtime must agree after replacement"
        );
        assert!(
            !Arc::ptr_eq(&manager1, &manager2),
            "replacement must atomically publish a new callable client"
        );

        // Remove
        let mut remove = HashSet::new();
        remove.insert(key.clone());
        dispatch_partial_update(next_dispatch_sequence(), &HashMap::new(), &remove).await;
        assert!(WEBHOOK_RUNTIME.get(&key).is_none(), "remove must clear WEBHOOK_RUNTIME");
        assert!(
            get_webhook_manager().get(&key).await.is_none(),
            "remove must clear the callable manager entry"
        );
    }
}

#[cfg(test)]
mod runtime_stats_tests {
    use super::*;

    #[test]
    fn redis_count_starts_at_zero_or_existing() {
        // Globals may carry state from other tests; just exercise the API.
        let _: usize = redis_client_count();
        let _: usize = etcd_client_count();
        let _: usize = es_client_count();
    }

    #[test]
    fn empty_otlp_runner_store_reports_no_residue() {
        let store: ProviderRuntimeStore<PersistentRunner<OtlpApply>> = ProviderRuntimeStore::new();
        assert_eq!(store.count(), 0);
        assert!(store.list().is_empty());
    }
}
