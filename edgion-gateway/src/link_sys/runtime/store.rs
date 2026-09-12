//! Global store for LinkSys resources.
//!
//! Manages all LinkSys resources (Webhook, Redis, Etcd, etc.) and dispatches
//! configuration changes to the appropriate sub-module managers.
//! Follows the same ArcSwap pattern as PluginStore.
//!
//! Redis runtime clients are stored in a separate ArcSwap store for typed access:
//! callers use `get_redis_client("namespace/name")` to obtain a ready-to-use client.

use arc_swap::ArcSwap;
use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

use edgion_resources::resources::link_sys::LinkSys;

use crate::link_sys::credential;
use crate::link_sys::lifecycle::LinkSysLifecycle;
use crate::link_sys::providers::elasticsearch::EsLinkClient;
use crate::link_sys::providers::etcd::EtcdLinkClient;
use crate::link_sys::providers::kafka::KafkaLinkClient;
use crate::link_sys::providers::otlp::OtlpLinkClient;
use crate::link_sys::providers::redis::RedisLinkClient;
use crate::link_sys::providers::webhook::{get_webhook_manager, WebhookLinkClient, WebhookManager};
use crate::link_sys::LinkSysHealth;

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
                        dispatch_otlp_full(&dispatch_data);
                        spawn_ordered_dispatch(&handle, &LINK_SYS_DISPATCH_TAIL, async move {
                            dispatch_full_set(sequence, &dispatch_data).await;
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
                        dispatch_otlp_partial(&dispatch_data, &remove_set);
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
                        dispatch_otlp_full(&dispatch_data);
                        spawn_ordered_dispatch(&handle, &LINK_SYS_DISPATCH_TAIL, async move {
                            dispatch_full_set(sequence, &dispatch_data).await;
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

// ============================================================
// Redis runtime store
// ============================================================

/// Global runtime store for Redis clients, keyed by "namespace/name".
static REDIS_RUNTIME: LazyLock<ProviderRuntimeStore<RedisLinkClient>> = LazyLock::new(ProviderRuntimeStore::new);

/// Get a Redis client by LinkSys name ("namespace/name").
///
/// This is the primary API for plugins and other callers.
/// Returns None if the LinkSys resource doesn't exist or isn't a Redis type.
pub fn get_redis_client(name: &str) -> Option<Arc<RedisLinkClient>> {
    REDIS_RUNTIME.get(name)
}

/// List all registered Redis client names.
pub fn list_redis_clients() -> Vec<String> {
    REDIS_RUNTIME.list()
}

/// Number of Redis link clients currently in the runtime.
pub fn redis_client_count() -> usize {
    REDIS_RUNTIME.count()
}

/// Health check all registered Redis clients.
pub async fn health_check_all_redis() -> Vec<LinkSysHealth> {
    let clients = REDIS_RUNTIME.values();
    futures::future::join_all(clients.into_iter().map(|c| async move { c.health_status().await })).await
}

// ============================================================
// Etcd runtime store
// ============================================================

/// Global runtime store for Etcd clients, keyed by "namespace/name".
static ETCD_RUNTIME: LazyLock<ProviderRuntimeStore<EtcdLinkClient>> = LazyLock::new(ProviderRuntimeStore::new);

/// Get an Etcd client by LinkSys name ("namespace/name").
///
/// This is the primary API for plugins and other callers.
/// Returns None if the LinkSys resource doesn't exist or isn't an Etcd type.
pub fn get_etcd_client(name: &str) -> Option<Arc<EtcdLinkClient>> {
    ETCD_RUNTIME.get(name)
}

/// List all registered Etcd client names.
pub fn list_etcd_clients() -> Vec<String> {
    ETCD_RUNTIME.list()
}

/// Number of Etcd link clients currently in the runtime.
pub fn etcd_client_count() -> usize {
    ETCD_RUNTIME.count()
}

/// Health check all registered Etcd clients.
pub async fn health_check_all_etcd() -> Vec<LinkSysHealth> {
    let clients = ETCD_RUNTIME.values();
    futures::future::join_all(clients.into_iter().map(|c| async move { c.health_status().await })).await
}

// ============================================================
// Elasticsearch runtime store
// ============================================================

/// Global runtime store for Elasticsearch clients, keyed by "namespace/name".
static ES_RUNTIME: LazyLock<ProviderRuntimeStore<EsLinkClient>> = LazyLock::new(ProviderRuntimeStore::new);

/// Get an Elasticsearch client by LinkSys name ("namespace/name").
///
/// This is the primary API for DataSender and other callers.
/// Returns None if the LinkSys resource doesn't exist or isn't an Elasticsearch type.
pub fn get_es_client(name: &str) -> Option<Arc<EsLinkClient>> {
    ES_RUNTIME.get(name)
}

/// List all registered Elasticsearch client names.
pub fn list_es_clients() -> Vec<String> {
    ES_RUNTIME.list()
}

/// Number of Elasticsearch link clients currently in the runtime.
pub fn es_client_count() -> usize {
    ES_RUNTIME.count()
}

/// Health check all registered Elasticsearch clients.
pub async fn health_check_all_es() -> Vec<LinkSysHealth> {
    let clients = ES_RUNTIME.values();
    futures::future::join_all(clients.into_iter().map(|c| async move { c.health_status().await })).await
}

// ============================================================
// Kafka runtime store
// ============================================================

/// Global runtime store for Kafka clients, keyed by "namespace/name".
static KAFKA_RUNTIME: LazyLock<ProviderRuntimeStore<KafkaLinkClient>> = LazyLock::new(ProviderRuntimeStore::new);

/// Get a Kafka client by LinkSys name ("namespace/name").
///
/// This is the primary API for DataSender and other callers.
/// Returns None if the LinkSys resource doesn't exist or isn't a Kafka type.
pub fn get_kafka_client(name: &str) -> Option<Arc<KafkaLinkClient>> {
    KAFKA_RUNTIME.get(name)
}

// OTLP keeps one stable owner per key. Connection generations are prepared and
// published inside that owner, so a failed update cannot evict working service.
static OTLP_RUNTIME: LazyLock<ProviderRuntimeStore<OtlpLinkClient>> = LazyLock::new(ProviderRuntimeStore::new);
static OTLP_CLEANUP: LazyLock<Mutex<tokio::task::JoinSet<()>>> =
    LazyLock::new(|| Mutex::new(tokio::task::JoinSet::new()));
// A removed key cannot create another owner until its entire previous owner
// has joined. Desired recreations remain in the authoritative source snapshot.
static OTLP_RETIRING_KEYS: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
static OTLP_SHUTDOWN: Mutex<Option<Shared<BoxFuture<'static, ()>>>> = Mutex::new(None);
static OTLP_STOPPING: AtomicBool = AtomicBool::new(false);

pub fn get_otlp_client(name: &str) -> Option<Arc<OtlpLinkClient>> {
    OTLP_RUNTIME.get(name)
}

pub fn list_otlp_clients() -> Vec<String> {
    OTLP_RUNTIME.list()
}

pub fn otlp_client_count() -> usize {
    OTLP_RUNTIME.count()
}

fn configure_otlp(key: &str, resource: &LinkSys) {
    if OTLP_STOPPING.load(Ordering::Acquire) || OTLP_RETIRING_KEYS.lock().contains(key) {
        return;
    }
    let edgion_resources::resources::link_sys::SystemConfig::Otlp(config) = &resource.spec.config else {
        return;
    };
    // FS/process inputs do not inherit Kubernetes API name validation. Never
    // put the raw key in a diagnostic until the client has validated it.
    let client = match OTLP_RUNTIME.get(key) {
        Some(client) => client,
        None => match OtlpLinkClient::new(key.to_owned()) {
            Ok(client) => {
                OTLP_RUNTIME.insert(key.to_owned(), client.clone());
                client
            }
            Err(_) => {
                tracing::warn!(system = "otlp", reason = "invalid_config", "OTLP provider rejected");
                return;
            }
        },
    };
    if resource.key_name_str() != key || !resource.validate_config().is_empty() {
        client.invalidate_pending();
        tracing::warn!(
            link = key,
            system = "otlp",
            reason = "invalid_config",
            "OTLP update rejected"
        );
        return;
    }
    if let Err(reason) = client.update(config) {
        tracing::warn!(
            link = key,
            system = "otlp",
            reason,
            "OTLP update retained last-known-good generation"
        );
    }
}

/// Called under LINK_SYS_SOURCE_EVENT_LOCK, before source dispatch can reorder.
fn remove_otlp(key: &str) {
    let Some(client) = OTLP_RUNTIME.get(key) else {
        return;
    };
    if let Err(reason) = client.begin_shutdown() {
        tracing::warn!(link = key, system = "otlp", reason, "OTLP shutdown registration failed");
    }
    OTLP_RETIRING_KEYS.lock().insert(key.to_owned());
    OTLP_RUNTIME.remove(key);
    let key = key.to_owned();
    OTLP_CLEANUP.lock().spawn(async move {
        if let Err(reason) = client.shutdown().await {
            tracing::warn!(link = key, system = "otlp", reason, "OTLP shutdown failed");
        }
        let _source = LINK_SYS_SOURCE_EVENT_LOCK.lock();
        OTLP_RETIRING_KEYS.lock().remove(&key);
        if !OTLP_STOPPING.load(Ordering::Acquire) {
            if let Some(resource) = get_global_link_sys_store().get(&key) {
                configure_otlp(&key, &resource);
            }
        }
    });
}

/// Includes removed owners whose actual probe/SDK cleanup has not joined yet.
pub fn otlp_retiring_client_count() -> usize {
    let mut cleanup = OTLP_CLEANUP.lock();
    while let Some(result) = cleanup.try_join_next() {
        if result.is_err() {
            tracing::warn!(system = "otlp", reason = "shutdown_failed", "OTLP cleanup task failed");
        }
    }
    drop(cleanup);
    OTLP_RETIRING_KEYS.lock().len()
}

// These synchronous desired-state transitions run inside the existing source
// event critical section. Generic dispatch follows the same source order, while
// OTLP desired state and cancellation are visible before any async task polls.
fn dispatch_otlp_full(data: &HashMap<String, LinkSys>) {
    if OTLP_STOPPING.load(Ordering::Acquire) {
        return;
    }
    let mut present = HashSet::new();
    for (key, resource) in data {
        if matches!(
            resource.spec.config,
            edgion_resources::resources::link_sys::SystemConfig::Otlp(_)
        ) {
            present.insert(key.as_str());
            configure_otlp(key, resource);
        }
    }
    for key in OTLP_RUNTIME.list() {
        if !present.contains(key.as_str()) {
            remove_otlp(&key);
        }
    }
    otlp_retiring_client_count();
}

fn dispatch_otlp_partial(data: &HashMap<String, LinkSys>, remove: &HashSet<String>) {
    if OTLP_STOPPING.load(Ordering::Acquire) {
        return;
    }
    // Match source-map semantics: removal first, add/update wins an overlapping key.
    for key in remove {
        remove_otlp(key);
    }
    for (key, resource) in data {
        if matches!(
            resource.spec.config,
            edgion_resources::resources::link_sys::SystemConfig::Otlp(_)
        ) {
            configure_otlp(key, resource);
        } else {
            remove_otlp(key);
        }
    }
    otlp_retiring_client_count();
}

/// Fence source events and retain the entire drain task, including when a caller
/// stops awaiting it. Runtime shutdown must await this before dropping Tokio IO.
pub async fn shutdown_otlp_clients() {
    let completion = {
        let mut shutdown = OTLP_SHUTDOWN.lock();
        shutdown
            .get_or_insert_with(|| {
                let mut cleanup = {
                    let _source = LINK_SYS_SOURCE_EVENT_LOCK.lock();
                    OTLP_STOPPING.store(true, Ordering::Release);
                    for key in OTLP_RUNTIME.list() {
                        remove_otlp(&key);
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

/// Helper shared by the remove / disable paths: if there is an old client,
/// spawn its shutdown.
///
/// Note: this helper always calls `shutdown` and ignores `SHUTDOWN_ON_REPLACE`.
/// That const only controls the *replace* case (the old Arc on update); it
/// does not affect a real removal. Webhook uses its ordered specialized path
/// instead of this generic lifecycle helper.
fn spawn_shutdown_if_present<C: LinkSysLifecycle>(client: Option<Arc<C>>, label: &'static str) {
    if let Some(c) = client {
        tokio::spawn(async move {
            if let Err(e) = c.shutdown().await {
                tracing::warn!(label, error = %e, "shutdown failed");
            }
        });
    }
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

/// Build a new client and swap it into the store; init the new client in
/// the background and (if applicable) shut down the old client in the
/// background. Used by `dispatch_partial_update`.
///
/// Everything inside is dispatched via `tokio::spawn` and the function
/// itself does not await, so it is a synchronous `fn`. Callers must invoke
/// it from a tokio runtime context.
///
/// **Key invariant**: reading `old` (`runtime.get(key)`) is read-only;
/// afterwards `runtime.insert` performs a single atomic `ArcSwap` swap of the
/// new map (where that key already points to the new client). This way
/// `get_*_client(key)` always sees either the old or the new client —
/// **there is never a window where it is None**. If we instead did `remove`
/// followed by `insert`, the intermediate store would temporarily drop the key.
fn spawn_and_swap_partial<C, F>(key: &str, label: &'static str, build: F, runtime: &ProviderRuntimeStore<C>)
where
    C: LinkSysLifecycle,
    F: FnOnce() -> anyhow::Result<C>,
{
    match build() {
        Err(e) => tracing::error!(key, label, error = %e, "from_config failed"),
        Ok(client) => {
            let client = Arc::new(client);
            let old = runtime.get(key);
            runtime.insert(key.to_string(), client.clone());

            // New client: always init in the background
            let c = client.clone();
            let k = key.to_string();
            tokio::spawn(async move {
                if let Err(e) = c.init().await {
                    tracing::error!(label, key = %k, error = %e, "init failed");
                }
            });

            // Old client: shut down only when the provider declares SHUTDOWN_ON_REPLACE
            if let Some(old_client) = old {
                if C::SHUTDOWN_ON_REPLACE {
                    tokio::spawn(async move {
                        if let Err(e) = old_client.shutdown().await {
                            tracing::warn!(label, error = %e, "shutdown old client failed");
                        }
                    });
                }
                // Otherwise the Arc is dropped and the refcount is reclaimed naturally; manager state is maintained by the new init's upsert
            }
        }
    }
}

/// `dispatch_full_set` case: when not ready, carry the old Arc over into new_map.
/// Returns `true` to indicate handled; the caller should `continue`.
fn carry_over_if_not_ready<C>(
    key: &str,
    label: &'static str,
    auth_kind: &str,
    tls_ok: bool,
    ready: bool,
    old_snapshot: &HashMap<String, Arc<C>>,
    new_map: &mut HashMap<String, Arc<C>>,
    preserved: &mut HashSet<String>,
) -> bool {
    if ready {
        return false;
    }
    if let Some(existing) = old_snapshot.get(key) {
        tracing::info!(
            key,
            label,
            auth_kind,
            tls_ready = tls_ok,
            "LinkSys not ready; keeping existing client"
        );
        new_map.insert(key.to_string(), existing.clone());
        preserved.insert(key.to_string());
    } else {
        tracing::info!(
            key,
            label,
            auth_kind,
            tls_ready = tls_ok,
            "LinkSys not ready; skipping client build"
        );
    }
    true
}

/// `dispatch_full_set` case: add the new client to new_map and init in the background.
/// If construction fails, nothing is added (which matches "removed from the map" semantics).
fn register_for_full_set<C: LinkSysLifecycle>(
    key: &str,
    label: &'static str,
    build: impl FnOnce() -> anyhow::Result<C>,
    new_map: &mut HashMap<String, Arc<C>>,
) {
    match build() {
        Err(e) => tracing::error!(key, label, error = %e, "from_config failed"),
        Ok(client) => {
            let client = Arc::new(client);
            new_map.insert(key.to_string(), client.clone());
            let k = key.to_string();
            tokio::spawn(async move {
                if let Err(e) = client.init().await {
                    tracing::error!(label, key = %k, error = %e, "init failed");
                }
            });
        }
    }
}

/// At the end of `dispatch_full_set`, atomically swap each provider's store;
/// for evicted old Arcs (when the provider declares SHUTDOWN_ON_REPLACE),
/// shut them down concurrently in the background.
fn swap_and_shutdown_olds<C: LinkSysLifecycle>(
    label: &'static str,
    new_map: HashMap<String, Arc<C>>,
    preserved: HashSet<String>,
    runtime: &ProviderRuntimeStore<C>,
) {
    let old = runtime.replace_all(new_map);
    if !C::SHUTDOWN_ON_REPLACE || old.is_empty() {
        return;
    }
    tokio::spawn(async move {
        for (k, client) in old {
            if preserved.contains(&k) {
                continue; // carry-over Arc is the same one in new_map; do not shut down
            }
            if let Err(e) = client.shutdown().await {
                tracing::warn!(label, key = %k, error = %e, "shutdown old client failed");
            }
        }
    });
}

// ============================================================
// Dispatch to sub-module managers
// ============================================================

/// Dispatch a full set of LinkSys resources to sub-module managers.
async fn dispatch_full_set(sequence: u64, data: &HashMap<String, LinkSys>) {
    dispatch_full_set_with(sequence, data, get_webhook_manager(), &WEBHOOK_RUNTIME).await;
}

async fn dispatch_full_set_with(
    sequence: u64,
    data: &HashMap<String, LinkSys>,
    webhook_manager: &WebhookManager,
    webhook_runtime: &ProviderRuntimeStore<WebhookLinkClient>,
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
    let old_redis_snapshot = REDIS_RUNTIME.load_full();
    let old_etcd_snapshot = ETCD_RUNTIME.load_full();
    let old_es_snapshot = ES_RUNTIME.load_full();
    let old_kafka_snapshot = KAFKA_RUNTIME.load_full();

    let mut new_webhook_map: HashMap<String, Arc<WebhookLinkClient>> = HashMap::new();
    let mut new_redis_map: HashMap<String, Arc<RedisLinkClient>> = HashMap::new();
    let mut new_etcd_map: HashMap<String, Arc<EtcdLinkClient>> = HashMap::new();
    let mut new_es_map: HashMap<String, Arc<EsLinkClient>> = HashMap::new();
    let mut new_kafka_map: HashMap<String, Arc<KafkaLinkClient>> = HashMap::new();

    let mut preserved_redis_keys: HashSet<String> = HashSet::new();
    let mut preserved_etcd_keys: HashSet<String> = HashSet::new();
    let mut preserved_es_keys: HashSet<String> = HashSet::new();
    let mut preserved_kafka_keys: HashSet<String> = HashSet::new();

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
                match webhook_mutation.prepare(key, config.clone(), ls.metadata.namespace.as_deref()) {
                    Ok(prepared) => {
                        new_webhook_map.insert(key.clone(), Arc::new(WebhookLinkClient::from_prepared(prepared)));
                    }
                    Err(e) => {
                        tracing::error!(key, label = "Webhook", error = %e, "prepare failed");
                        if let Some(existing) = old_webhook_snapshot.get(key) {
                            new_webhook_map.insert(key.clone(), existing.clone());
                        }
                    }
                }
            }
            edgion_resources::resources::link_sys::SystemConfig::Redis(redis_config) => {
                let (ready, tls_ok, auth) = credential_readiness(ls, credential::basic_state);
                if carry_over_if_not_ready(
                    key,
                    "Redis",
                    auth.kind(),
                    tls_ok,
                    ready,
                    &*old_redis_snapshot,
                    &mut new_redis_map,
                    &mut preserved_redis_keys,
                ) {
                    continue;
                }
                register_for_full_set(
                    key,
                    "Redis",
                    || RedisLinkClient::from_config(key, redis_config, &auth),
                    &mut new_redis_map,
                );
            }
            edgion_resources::resources::link_sys::SystemConfig::Etcd(etcd_config) => {
                let (ready, tls_ok, auth) = credential_readiness(ls, credential::basic_state);
                if carry_over_if_not_ready(
                    key,
                    "Etcd",
                    auth.kind(),
                    tls_ok,
                    ready,
                    &*old_etcd_snapshot,
                    &mut new_etcd_map,
                    &mut preserved_etcd_keys,
                ) {
                    continue;
                }
                register_for_full_set(
                    key,
                    "Etcd",
                    || EtcdLinkClient::from_config(key, etcd_config, &auth),
                    &mut new_etcd_map,
                );
            }
            edgion_resources::resources::link_sys::SystemConfig::Elasticsearch(es_config) => {
                let (ready, tls_ok, auth) = credential_readiness(ls, credential::resolved_state);
                if carry_over_if_not_ready(
                    key,
                    "Elasticsearch",
                    auth.kind(),
                    tls_ok,
                    ready,
                    &*old_es_snapshot,
                    &mut new_es_map,
                    &mut preserved_es_keys,
                ) {
                    continue;
                }
                register_for_full_set(
                    key,
                    "Elasticsearch",
                    || EsLinkClient::from_config(key, es_config, &auth),
                    &mut new_es_map,
                );
            }
            edgion_resources::resources::link_sys::SystemConfig::Kafka(kafka_config) => {
                let (ready, tls_ok, auth) = credential_readiness(ls, credential::resolved_state);
                if carry_over_if_not_ready(
                    key,
                    "Kafka",
                    auth.kind(),
                    tls_ok,
                    ready,
                    &*old_kafka_snapshot,
                    &mut new_kafka_map,
                    &mut preserved_kafka_keys,
                ) {
                    continue;
                }
                register_for_full_set(
                    key,
                    "Kafka",
                    || KafkaLinkClient::from_config(key, kafka_config, &auth),
                    &mut new_kafka_map,
                );
            }
            // DNS resolver, not a runtime link client — nothing to register in the link store.
            edgion_resources::resources::link_sys::SystemConfig::HttpDns(_) => {}
            // Controller-only credential acquisition metadata. The Gateway receives
            // only the resolved plugin credential and never contacts the issuer.
            edgion_resources::resources::link_sys::SystemConfig::CredentialSource(_) => {}
            // OTLP desired state was applied synchronously at source entry.
            edgion_resources::resources::link_sys::SystemConfig::Otlp(_) => {}
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

    // Atomic swap + background shutdown of evicted old Arcs, gated by SHUTDOWN_ON_REPLACE
    swap_and_shutdown_olds("Redis", new_redis_map, preserved_redis_keys, &REDIS_RUNTIME);
    swap_and_shutdown_olds("Etcd", new_etcd_map, preserved_etcd_keys, &ETCD_RUNTIME);
    swap_and_shutdown_olds("Elasticsearch", new_es_map, preserved_es_keys, &ES_RUNTIME);
    swap_and_shutdown_olds("Kafka", new_kafka_map, preserved_kafka_keys, &KAFKA_RUNTIME);
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

    // Handle add/update
    for (key, ls) in add_or_update {
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
                match webhook_mutation.prepare(key, config.clone(), ls.metadata.namespace.as_deref()) {
                    Err(e) => tracing::error!(key, label = "Webhook", error = %e, "prepare failed"),
                    Ok(prepared) => {
                        let client = Arc::new(WebhookLinkClient::from_prepared(prepared));
                        if let Err(e) = webhook_mutation.commit_one(key, client.prepared_entry()).await {
                            tracing::error!(key, label = "Webhook", error = %e, "commit failed");
                        } else {
                            webhook_runtime.insert(key.clone(), client);
                        }
                    }
                }
            }
            edgion_resources::resources::link_sys::SystemConfig::Redis(redis_config) => {
                let (ready, tls_ok, auth) = credential_readiness(ls, credential::basic_state);
                if preserve_if_not_ready(key, "Redis", auth.kind(), tls_ok, ready, &REDIS_RUNTIME) {
                    continue;
                }
                spawn_and_swap_partial(
                    key,
                    "Redis",
                    || RedisLinkClient::from_config(key, redis_config, &auth),
                    &REDIS_RUNTIME,
                );
            }
            edgion_resources::resources::link_sys::SystemConfig::Etcd(etcd_config) => {
                let (ready, tls_ok, auth) = credential_readiness(ls, credential::basic_state);
                if preserve_if_not_ready(key, "Etcd", auth.kind(), tls_ok, ready, &ETCD_RUNTIME) {
                    continue;
                }
                spawn_and_swap_partial(
                    key,
                    "Etcd",
                    || EtcdLinkClient::from_config(key, etcd_config, &auth),
                    &ETCD_RUNTIME,
                );
            }
            edgion_resources::resources::link_sys::SystemConfig::Elasticsearch(es_config) => {
                let (ready, tls_ok, auth) = credential_readiness(ls, credential::resolved_state);
                if preserve_if_not_ready(key, "Elasticsearch", auth.kind(), tls_ok, ready, &ES_RUNTIME) {
                    continue;
                }
                spawn_and_swap_partial(
                    key,
                    "Elasticsearch",
                    || EsLinkClient::from_config(key, es_config, &auth),
                    &ES_RUNTIME,
                );
            }
            edgion_resources::resources::link_sys::SystemConfig::Kafka(kafka_config) => {
                let (ready, tls_ok, auth) = credential_readiness(ls, credential::resolved_state);
                if preserve_if_not_ready(key, "Kafka", auth.kind(), tls_ok, ready, &KAFKA_RUNTIME) {
                    continue;
                }
                spawn_and_swap_partial(
                    key,
                    "Kafka",
                    || KafkaLinkClient::from_config(key, kafka_config, &auth),
                    &KAFKA_RUNTIME,
                );
            }
            // DNS resolver, not a runtime link client — nothing to register in the link store.
            edgion_resources::resources::link_sys::SystemConfig::HttpDns(_) => {}
            // Controller-only credential acquisition metadata.
            edgion_resources::resources::link_sys::SystemConfig::CredentialSource(_) => {}
            edgion_resources::resources::link_sys::SystemConfig::Otlp(_) => {
                // Switching a resource to OTLP must not leave its previous
                // provider callable under the same key.
                webhook_mutation.commit_remove(key).await;
                webhook_runtime.remove(key);
                spawn_shutdown_if_present(REDIS_RUNTIME.remove(key), "Redis");
                spawn_shutdown_if_present(ETCD_RUNTIME.remove(key), "Etcd");
                spawn_shutdown_if_present(ES_RUNTIME.remove(key), "Elasticsearch");
                spawn_shutdown_if_present(KAFKA_RUNTIME.remove(key), "Kafka");
            }
        }
    }

    // Handle remove — uniform across all providers
    for key in remove {
        webhook_mutation.commit_remove(key).await;
        webhook_runtime.remove(key);
        spawn_shutdown_if_present(REDIS_RUNTIME.remove(key), "Redis");
        spawn_shutdown_if_present(ETCD_RUNTIME.remove(key), "Etcd");
        spawn_shutdown_if_present(ES_RUNTIME.remove(key), "Elasticsearch");
        spawn_shutdown_if_present(KAFKA_RUNTIME.remove(key), "Kafka");
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
pub(crate) static I5_DISPATCH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    type DispatchTaskSlot = Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>;

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
                dispatch_full_set_with(sequence, &data, &manager, &runtime).await;
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
        let _global_guard = I5_DISPATCH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
                        let client = Arc::new(WebhookLinkClient::from_prepared(prepared));
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
        let _guard = I5_DISPATCH_LOCK.lock().unwrap_or_else(|error| error.into_inner());
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
                REDIS_RUNTIME.insert(
                    key.clone(),
                    Arc::new(
                        RedisLinkClient::from_config(&key, &redis, &credential::CredentialState::NotRequired).unwrap(),
                    ),
                );
                ETCD_RUNTIME.insert(
                    key.clone(),
                    Arc::new(
                        EtcdLinkClient::from_config(&key, &etcd, &credential::CredentialState::NotRequired).unwrap(),
                    ),
                );
                ES_RUNTIME.insert(
                    key.clone(),
                    Arc::new(EsLinkClient::from_config(&key, &es, &credential::CredentialState::NotRequired).unwrap()),
                );
                KAFKA_RUNTIME.insert(
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
                    if full {
                        dispatch_otlp_full(&data);
                    } else {
                        dispatch_otlp_partial(&data, &HashSet::new());
                    }
                    spawn_ordered_dispatch(&tokio::runtime::Handle::current(), &tail, async move {
                        gate.await.unwrap();
                        if full {
                            dispatch_full_set_with(sequence, &data, &earlier_manager, &earlier_webhooks).await;
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
                let owner = get_otlp_client(&key).unwrap();
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
                assert!(Arc::ptr_eq(&owner, &get_otlp_client(&key).unwrap()));

                // A later recreation must survive the earlier type cleanup.
                let recreated = HashMap::from([(
                    key.clone(),
                    create_test_webhook_linksys_with_port("ordered", "transition", 8181),
                )]);
                dispatch_otlp_partial(&recreated, &HashSet::new());
                dispatch_partial_update_with(4, &recreated, &HashSet::new(), &manager, &webhooks).await;
                assert_callable_webhook_port(&manager, &webhooks, &key, 8181).await;
                owner.shutdown().await.unwrap();
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
                    retry: None,
                    topology: None,
                    tls: None,
                    observability: None,
                }),
            },
            status: None,
        }
    }

    /// Full-set dispatch must not build a client for a LinkSys whose credential
    /// Secret has not been injected yet.
    #[tokio::test]
    async fn dispatch_full_set_skips_build_when_credential_not_ready() {
        let _guard = I5_DISPATCH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let key = "i5-ns/pending-full-set".to_string();
        let ls = create_redis_linksys("i5-ns", "pending-full-set", InjectedSecret::Missing);
        let mut data = HashMap::new();
        data.insert(key.clone(), ls);

        dispatch_full_set(next_dispatch_sequence(), &data).await;

        assert!(
            get_redis_client(&key).is_none(),
            "no client should be built for a pending-credential LinkSys"
        );
    }

    /// When a previously-built client exists and a subsequent full-set carries
    /// the same resource with pending credentials (Secret temporarily gone),
    /// the existing client is preserved rather than torn down.
    #[tokio::test]
    async fn dispatch_full_set_preserves_existing_client_on_pending_update() {
        let _guard = I5_DISPATCH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let key = "i5-ns/preserve-full-set".to_string();

        // Seed with a ready LinkSys so a client is inserted.
        let ready = create_redis_linksys("i5-ns", "preserve-full-set", InjectedSecret::Valid);
        let mut data = HashMap::new();
        data.insert(key.clone(), ready);
        dispatch_full_set(next_dispatch_sequence(), &data).await;
        let first = get_redis_client(&key).expect("client should be built when ready");

        // Second full-set for the same key but with pending credentials.
        let pending = create_redis_linksys("i5-ns", "preserve-full-set", InjectedSecret::Missing);
        let mut data = HashMap::new();
        data.insert(key.clone(), pending);
        dispatch_full_set(next_dispatch_sequence(), &data).await;

        let second = get_redis_client(&key).expect("client should be preserved on pending update");
        assert!(
            Arc::ptr_eq(&first, &second),
            "the preserved client must be the same Arc instance"
        );

        // Cleanup so the global store doesn't carry state into other tests.
        let _ = REDIS_RUNTIME.remove(&key);
    }

    /// Partial-update dispatch must not build a client on an add with pending
    /// credentials.
    #[tokio::test]
    async fn dispatch_partial_update_skips_build_when_credential_not_ready() {
        let _guard = I5_DISPATCH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _guard = I5_DISPATCH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let key = "i5-ns/preserve-partial".to_string();

        // Seed
        let ready = create_redis_linksys("i5-ns", "preserve-partial", InjectedSecret::Valid);
        let mut add = HashMap::new();
        add.insert(key.clone(), ready);
        dispatch_partial_update(next_dispatch_sequence(), &add, &HashSet::new()).await;
        let first = get_redis_client(&key).expect("client should be built when ready");

        // Pending update
        let pending = create_redis_linksys("i5-ns", "preserve-partial", InjectedSecret::Missing);
        let mut add = HashMap::new();
        add.insert(key.clone(), pending);
        dispatch_partial_update(next_dispatch_sequence(), &add, &HashSet::new()).await;

        let second = get_redis_client(&key).expect("client should be preserved on pending update");
        assert!(
            Arc::ptr_eq(&first, &second),
            "the preserved client must be the same Arc instance"
        );

        let _ = REDIS_RUNTIME.remove(&key);
    }

    /// An `Invalid` credential state must also block client builds, symmetric
    /// to the `Pending` case. Both share `is_buildable() == false`, but a
    /// separate test defends against a future refactor that accidentally treats
    /// them differently at the dispatch layer.
    #[tokio::test]
    async fn dispatch_full_set_skips_build_when_credential_invalid() {
        let _guard = I5_DISPATCH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let key = "i5-ns/invalid-full-set".to_string();
        let ls = create_redis_linksys("i5-ns", "invalid-full-set", InjectedSecret::MalformedForInvalid);
        let mut data = HashMap::new();
        data.insert(key.clone(), ls);

        dispatch_full_set(next_dispatch_sequence(), &data).await;

        assert!(
            get_redis_client(&key).is_none(),
            "no client should be built for an invalid-credential LinkSys"
        );
    }

    /// Partial-update add with `Invalid` credential state must also be filtered
    /// out.
    #[tokio::test]
    async fn dispatch_partial_update_skips_build_when_credential_invalid() {
        let _guard = I5_DISPATCH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
    async fn dispatch_partial_update_replace_webhook_keeps_runtime_arc_swap() {
        use edgion_resources::resources::link_sys::webhook::WebhookServiceConfig;
        use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};

        // Share a global lock with other dispatch tests to serialize. Without
        // holding the lock, their `dispatch_full_set` calls would wipe the key
        // inserted by this test via an empty new_webhook_map through
        // `WEBHOOK_RUNTIME.replace_all`.
        let _guard = I5_DISPATCH_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // Use a unique key to also avoid interference with other tests' add/remove paths.
        let key = "t13-ns/wh-arc-swap".to_string();

        let make_ls = || {
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
                                    port: Some(8080),
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
        add.insert(key.clone(), make_ls());
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

        // Update (different config) — a new Arc should replace the old one.
        // Webhook replacement is manager-atomic and does not use generic shutdown.
        let mut update = HashMap::new();
        update.insert(key.clone(), make_ls());
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
    fn empty_otlp_owner_store_reports_no_residue() {
        let store: ProviderRuntimeStore<OtlpLinkClient> = ProviderRuntimeStore::new();
        assert_eq!(store.count(), 0);
        assert!(store.list().is_empty());
    }

    fn pending_otlp(name: &str) -> LinkSys {
        serde_json::from_value(serde_json::json!({
            "apiVersion": "edgion.io/v1", "kind": "LinkSys",
            "metadata": {"namespace": "otlp-store-test", "name": name},
            "spec": {"type": "otlp", "config": {
                "endpoint": "https://collector.example.test:4317",
                "auth": {"secretRef": {"name": "missing-auth"}}
            }}
        }))
        .unwrap()
    }

    #[test]
    fn otlp_full_partial_invalid_type_change_and_remove_keep_owner_scope() {
        let _guard = I5_DISPATCH_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let store = get_global_link_sys_store();
            let first_key = "otlp-store-test/first".to_owned();
            let second_key = "otlp-store-test/second".to_owned();
            let initial = HashMap::from([
                (first_key.clone(), pending_otlp("first")),
                (second_key.clone(), pending_otlp("second")),
            ]);
            store.replace_all(initial.clone());
            let first = get_otlp_client(&first_key).unwrap();
            let second = get_otlp_client(&second_key).unwrap();
            assert!(!first.healthy());
            assert!(!second.healthy());

            let mut invalid = pending_otlp("first");
            let edgion_resources::resources::link_sys::SystemConfig::Otlp(config) = &mut invalid.spec.config else {
                panic!("test fixture is OTLP");
            };
            config.endpoint = "http://private-sentinel@collector".into();
            store.update(HashMap::from([(first_key.clone(), invalid)]), &HashSet::new());
            assert!(Arc::ptr_eq(&first, &get_otlp_client(&first_key).unwrap()));
            assert!(Arc::ptr_eq(&second, &get_otlp_client(&second_key).unwrap()));

            // Full-set absence removes only that owner; pending credentials retain
            // the other owner without ever opening a socket in this unit test.
            store.replace_all(HashMap::from([(first_key.clone(), pending_otlp("first"))]));
            assert!(get_otlp_client(&second_key).is_none());
            assert!(Arc::ptr_eq(&first, &get_otlp_client(&first_key).unwrap()));

            let other: LinkSys = serde_json::from_value(serde_json::json!({
                "apiVersion": "edgion.io/v1", "kind": "LinkSys",
                "metadata": {"namespace": "otlp-store-test", "name": "first"},
                "spec": {"type": "httpdns", "config": {"urlTemplate": "https://dns.example.test/{domain}"}}
            }))
            .unwrap();
            store.update(HashMap::from([(first_key.clone(), other)]), &HashSet::new());
            assert!(get_otlp_client(&first_key).is_none());
            assert!(!first.healthy());
            store.update(initial, &HashSet::new());
            store.update(HashMap::new(), &HashSet::from([first_key.clone(), second_key.clone()]));
            assert!(get_otlp_client(&first_key).is_none());
            assert!(get_otlp_client(&second_key).is_none());
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while otlp_retiring_client_count() != 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("removed owners must finish cleanup");
        });
    }

    #[test]
    fn otlp_source_entries_precede_ordered_unrelated_async_dispatch() {
        let _guard = I5_DISPATCH_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let store = get_global_link_sys_store();
            let first_key = "otlp-store-test/source-first".to_owned();
            let second_key = "otlp-store-test/source-second".to_owned();
            let first = HashMap::from([(first_key.clone(), pending_otlp("source-first"))]);
            let second = HashMap::from([(second_key.clone(), pending_otlp("source-second"))]);
            store.update(first.clone(), &HashSet::new());
            store.update(second.clone(), &HashSet::new());
            // No async task has been polled yet on this current-thread runtime.
            // Both desired-state entries must already exist at source publication.
            let first_owner = get_otlp_client(&first_key).expect("earlier key is configured synchronously");
            let second_owner = get_otlp_client(&second_key).expect("unrelated later key is configured synchronously");
            let tail = LINK_SYS_DISPATCH_TAIL.lock().take().unwrap();
            tail.await.unwrap();
            assert!(Arc::ptr_eq(&first_owner, &get_otlp_client(&first_key).unwrap()));
            assert!(Arc::ptr_eq(&second_owner, &get_otlp_client(&second_key).unwrap()));
            store.rebuild_clients();
            assert!(Arc::ptr_eq(&first_owner, &get_otlp_client(&first_key).unwrap()));
            store.update(HashMap::new(), &HashSet::from([first_key.clone(), second_key.clone()]));
            assert!(get_otlp_client(&first_key).is_none());
            assert!(!first_owner.healthy());
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while otlp_retiring_client_count() != 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("removed owners must finish cleanup");
        });
    }
}
