//! Global store for LinkSys resources.
//!
//! Manages all LinkSys resources (Webhook, Redis, Etcd, etc.) and dispatches
//! configuration changes to the appropriate sub-module managers.
//! Follows the same ArcSwap pattern as PluginStore.
//!
//! Redis runtime clients are stored in a separate ArcSwap store for typed access:
//! callers use `get_redis_client("namespace/name")` to obtain a ready-to-use client.

use arc_swap::ArcSwap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::LazyLock;

use edgion_resources::resources::link_sys::LinkSys;

use crate::link_sys::credential;
use crate::link_sys::lifecycle::LinkSysLifecycle;
use crate::link_sys::providers::elasticsearch::EsLinkClient;
use crate::link_sys::providers::etcd::EtcdLinkClient;
use crate::link_sys::providers::kafka::KafkaLinkClient;
use crate::link_sys::providers::redis::RedisLinkClient;
use crate::link_sys::providers::webhook::WebhookLinkClient;
use crate::link_sys::LinkSysHealth;

// ============================================================
// Type aliases
// ============================================================

type LinkSysMap = HashMap<String, LinkSys>;

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
        // Wrap once and share with the dispatch task to avoid cloning the entire map
        // (full_sync replays every LinkSys resource and used to copy the whole HashMap).
        let data = Arc::new(data);

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let data_for_dispatch = Arc::clone(&data);
            handle.spawn(async move {
                dispatch_full_set(&data_for_dispatch).await;
            });
        } else {
            // Reachable only outside a tokio runtime (currently: the four sync
            // unit tests that exercise the resource map only). Production
            // callers all run inside tokio; if this fires there, provider
            // clients were never built and `get_*_client` will look like
            // "config not synced yet" — surface it instead of failing silently.
            tracing::error!("LinkSysStore::replace_all called outside a tokio runtime — dispatch skipped");
        }

        self.resources.store(data);
    }

    /// Update LinkSys resources atomically (incremental sync).
    ///
    /// Dispatches changes to the appropriate sub-module managers.
    pub fn update(&self, add_or_update: HashMap<String, LinkSys>, remove: &HashSet<String>) {
        // Wrap once and share with the dispatch task to avoid a second deep clone of
        // `add_or_update` (each LinkSys carries nested credential structs).
        let add_or_update = Arc::new(add_or_update);

        let current = self.resources.load();
        let mut new_map: LinkSysMap = (**current).clone();

        for key in remove {
            new_map.remove(key);
        }
        for (key, ls) in add_or_update.iter() {
            new_map.insert(key.clone(), ls.clone());
        }

        // Dispatch changes to sub-module managers
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let dispatch_data = Arc::clone(&add_or_update);
            let remove_set = remove.clone();
            handle.spawn(async move {
                dispatch_partial_update(&dispatch_data, &remove_set).await;
            });
        } else {
            // See `replace_all` for the rationale — this branch is only hit by
            // sync unit tests today; logging makes any future outside-runtime
            // regression visible rather than presenting as "config not synced".
            tracing::error!("LinkSysStore::update called outside a tokio runtime — dispatch skipped");
        }

        self.resources.store(Arc::new(new_map));
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
/// does not affect *remove* -- on a real removal every provider needs cleanup
/// (including Webhook's `manager.remove`).
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
async fn dispatch_full_set(data: &HashMap<String, LinkSys>) {
    // Snapshot current runtime maps so that we can carry over clients for
    // resources whose credentials are not yet resolved.
    let old_webhook_snapshot = WEBHOOK_RUNTIME.load_full();
    let old_redis_snapshot = REDIS_RUNTIME.load_full();
    let old_etcd_snapshot = ETCD_RUNTIME.load_full();
    let old_es_snapshot = ES_RUNTIME.load_full();
    let old_kafka_snapshot = KAFKA_RUNTIME.load_full();

    let mut new_webhook_map: HashMap<String, Arc<WebhookLinkClient>> = HashMap::new();
    let mut new_redis_map: HashMap<String, Arc<RedisLinkClient>> = HashMap::new();
    let mut new_etcd_map: HashMap<String, Arc<EtcdLinkClient>> = HashMap::new();
    let mut new_es_map: HashMap<String, Arc<EsLinkClient>> = HashMap::new();
    let mut new_kafka_map: HashMap<String, Arc<KafkaLinkClient>> = HashMap::new();

    let mut preserved_webhook_keys: HashSet<String> = HashSet::new();
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
                if carry_over_if_not_ready(
                    key,
                    "Webhook",
                    auth.kind(),
                    // Webhook TLS material is validated inside the manager's
                    // build_webhook_client; only auth is gated here.
                    true,
                    ready,
                    &*old_webhook_snapshot,
                    &mut new_webhook_map,
                    &mut preserved_webhook_keys,
                ) {
                    continue;
                }
                register_for_full_set(
                    key,
                    "Webhook",
                    || {
                        Ok(WebhookLinkClient::new(
                            key.clone(),
                            config.clone(),
                            ls.metadata.namespace.clone(),
                        ))
                    },
                    &mut new_webhook_map,
                );
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
        }
    }

    // Atomic swap + background shutdown of evicted old Arcs, gated by SHUTDOWN_ON_REPLACE
    swap_and_shutdown_olds("Webhook", new_webhook_map, preserved_webhook_keys, &WEBHOOK_RUNTIME);
    swap_and_shutdown_olds("Redis", new_redis_map, preserved_redis_keys, &REDIS_RUNTIME);
    swap_and_shutdown_olds("Etcd", new_etcd_map, preserved_etcd_keys, &ETCD_RUNTIME);
    swap_and_shutdown_olds("Elasticsearch", new_es_map, preserved_es_keys, &ES_RUNTIME);
    swap_and_shutdown_olds("Kafka", new_kafka_map, preserved_kafka_keys, &KAFKA_RUNTIME);
}

/// Dispatch incremental changes to sub-module managers.
async fn dispatch_partial_update(add_or_update: &HashMap<String, LinkSys>, remove: &HashSet<String>) {
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
                if preserve_if_not_ready(key, "Webhook", auth.kind(), tls_ok, ready, &WEBHOOK_RUNTIME) {
                    continue;
                }
                spawn_and_swap_partial(
                    key,
                    "Webhook",
                    || {
                        Ok(WebhookLinkClient::new(
                            key.clone(),
                            config.clone(),
                            ls.metadata.namespace.clone(),
                        ))
                    },
                    &WEBHOOK_RUNTIME,
                );
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
        }
    }

    // Handle remove — uniform across all providers
    for key in remove {
        spawn_shutdown_if_present(WEBHOOK_RUNTIME.remove(key), "Webhook");
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
mod tests {
    use super::*;

    fn create_test_webhook_linksys(namespace: &str, name: &str) -> LinkSys {
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

    // ============================================================
    // Credential-readiness-gated dispatch tests
    //
    // These tests mutate the process-global REDIS_RUNTIME. A full-set test
    // running concurrently with a partial-update test would wipe the map the
    // latter seeded, producing flaky failures. Serialize them through a
    // module-local Mutex. Keys are namespaced under `i5-ns/` to avoid
    // colliding with any future non-dispatch test.
    // ============================================================

    use std::sync::Mutex as StdMutex;
    static I5_DISPATCH_LOCK: StdMutex<()> = StdMutex::new(());

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

        dispatch_full_set(&data).await;

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
        dispatch_full_set(&data).await;
        let first = get_redis_client(&key).expect("client should be built when ready");

        // Second full-set for the same key but with pending credentials.
        let pending = create_redis_linksys("i5-ns", "preserve-full-set", InjectedSecret::Missing);
        let mut data = HashMap::new();
        data.insert(key.clone(), pending);
        dispatch_full_set(&data).await;

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

        dispatch_partial_update(&add, &HashSet::new()).await;

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
        dispatch_partial_update(&add, &HashSet::new()).await;
        let first = get_redis_client(&key).expect("client should be built when ready");

        // Pending update
        let pending = create_redis_linksys("i5-ns", "preserve-partial", InjectedSecret::Missing);
        let mut add = HashMap::new();
        add.insert(key.clone(), pending);
        dispatch_partial_update(&add, &HashSet::new()).await;

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

        dispatch_full_set(&data).await;

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

        dispatch_partial_update(&add, &HashSet::new()).await;

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
        dispatch_partial_update(&add, &HashSet::new()).await;
        // spawn_and_swap_partial inserts into WEBHOOK_RUNTIME synchronously before
        // spawning background init, so the Arc is visible immediately after await.
        let arc1 = WEBHOOK_RUNTIME.get(&key);
        assert!(arc1.is_some(), "first dispatch must populate WEBHOOK_RUNTIME");

        // Update (different config) — a new Arc should replace the old one.
        // SHUTDOWN_ON_REPLACE is false for Webhook, so the old Arc is simply dropped.
        let mut update = HashMap::new();
        update.insert(key.clone(), make_ls());
        dispatch_partial_update(&update, &HashSet::new()).await;
        let arc2 = WEBHOOK_RUNTIME.get(&key);
        assert!(arc2.is_some(), "after update, WEBHOOK_RUNTIME must still hold a client");
        assert!(
            !Arc::ptr_eq(arc1.as_ref().unwrap(), arc2.as_ref().unwrap()),
            "update must produce a fresh Arc"
        );

        // Remove
        let mut remove = HashSet::new();
        remove.insert(key.clone());
        dispatch_partial_update(&HashMap::new(), &remove).await;
        assert!(WEBHOOK_RUNTIME.get(&key).is_none(), "remove must clear WEBHOOK_RUNTIME");
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
}
