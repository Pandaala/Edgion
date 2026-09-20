//! LinkSys source snapshots and the single typed runtime registry.
//!
//! Provider owners handle same-kind updates. Removal immediately hides the key
//! and signals the old owner; cleanup never writes back into the registry.

use arc_swap::ArcSwap;
use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};

use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};

use crate::link_sys::credential;
use crate::link_sys::providers::elasticsearch::{EsApply, EsLinkClient};
use crate::link_sys::providers::etcd::{EtcdApply, EtcdLinkClient};
use crate::link_sys::providers::kafka::{KafkaApply, KafkaLinkClient};
use crate::link_sys::providers::otlp::{OtlpApply, OtlpLinkClient};
use crate::link_sys::providers::redis::{RedisApply, RedisLinkClient};
use crate::link_sys::providers::webhook::{fingerprint_webhook, WebhookLinkClient};
use crate::link_sys::runtime::apply::RuntimeFingerprint;
use crate::link_sys::runtime::runner::PersistentRunner;
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
/// Every delta must run, including an earlier update to an unrelated key.
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
    /// Reconciles typed owners in the unified runtime registry.
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
    /// Dispatches only changed keys, plus unadmitted keys when capacity is freed.
    pub fn update(&self, add_or_update: HashMap<String, LinkSys>, remove: &HashSet<String>) {
        if self
            .update_with_dispatch(
                add_or_update,
                remove,
                &NEXT_LINK_SYS_DISPATCH_SEQUENCE,
                &LINK_SYS_SOURCE_EVENT_LOCK,
                |sequence, dispatch_data, remove_set, snapshot| {
                    if let Ok(handle) = tokio::runtime::Handle::try_current() {
                        spawn_ordered_dispatch(&handle, &LINK_SYS_DISPATCH_TAIL, async move {
                            dispatch_partial_update(sequence, &dispatch_data, &remove_set, &snapshot).await;
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

    /// Reconsider unadmitted keys after a cap increase without rebuilding live clients.
    fn retry_admission(&self) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if self
            .rebuild_clients_with_dispatch(
                &NEXT_LINK_SYS_DISPATCH_SEQUENCE,
                &LINK_SYS_SOURCE_EVENT_LOCK,
                |sequence, snapshot| {
                    spawn_ordered_dispatch(&handle, &LINK_SYS_DISPATCH_TAIL, async move {
                        RUNTIMES.retry_admission(sequence, &snapshot);
                    });
                },
            )
            .is_err()
        {
            tracing::error!("LinkSys dispatch sequence exhausted; admission retry rejected");
        }
    }

    fn publish_source_event(
        &self,
        counter: &AtomicU64,
        source_lock: &Mutex<()>,
        build_snapshot: impl FnOnce(&LinkSysMap) -> Arc<LinkSysMap>,
        create_dispatch_task: impl FnOnce(u64, Arc<LinkSysMap>),
    ) -> Result<u64, DispatchSequenceExhausted> {
        let _source_guard = source_lock.lock();
        let sequence = reserve_dispatch_sequence(counter).ok_or(DispatchSequenceExhausted)?;
        let current = self.resources.load_full();
        let next = build_snapshot(current.as_ref());

        // Publication and task creation share the same synchronous source-event
        // critical section. Async polling order may differ, but sequence order
        // always reflects the authoritative source entry order.
        self.resources.store(Arc::clone(&next));
        create_dispatch_task(sequence, next);
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
            |sequence, _| create_dispatch_task(sequence, dispatch_data),
        )
    }

    fn update_with_dispatch(
        &self,
        add_or_update: HashMap<String, LinkSys>,
        remove: &HashSet<String>,
        counter: &AtomicU64,
        source_lock: &Mutex<()>,
        create_dispatch_task: impl FnOnce(u64, Arc<LinkSysMap>, HashSet<String>, Arc<LinkSysMap>),
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
            |sequence, snapshot| create_dispatch_task(sequence, dispatch_data, remove_set, snapshot),
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProviderKind {
    Redis,
    Etcd,
    Elasticsearch,
    Kafka,
    Otlp,
    Webhook,
}

impl ProviderKind {
    fn of(config: &SystemConfig) -> Option<Self> {
        match config {
            SystemConfig::Redis(_) => Some(Self::Redis),
            SystemConfig::Etcd(_) => Some(Self::Etcd),
            SystemConfig::Elasticsearch(_) => Some(Self::Elasticsearch),
            SystemConfig::Kafka(_) => Some(Self::Kafka),
            SystemConfig::Otlp(_) => Some(Self::Otlp),
            SystemConfig::Webhook(_) => Some(Self::Webhook),
            SystemConfig::HttpDns(_) | SystemConfig::CredentialSource(_) => None,
        }
    }
}

enum ProviderRuntime {
    Redis(Arc<PersistentRunner<RedisApply>>),
    Etcd(Arc<PersistentRunner<EtcdApply>>),
    Elasticsearch(Arc<PersistentRunner<EsApply>>),
    Kafka(Arc<PersistentRunner<KafkaApply>>),
    Otlp(Arc<PersistentRunner<OtlpApply>>),
    Webhook(Arc<WebhookLinkClient>),
}

/// The registry owns this wrapper, while background tasks own only their runner.
/// Removing the wrapper signals cancellation even when the task holds its runner.
struct RuntimeEntry {
    provider: ProviderRuntime,
    stopped: AtomicBool,
}

impl RuntimeEntry {
    fn new(provider: ProviderRuntime) -> Arc<Self> {
        Arc::new(Self {
            provider,
            stopped: AtomicBool::new(false),
        })
    }

    fn kind(&self) -> ProviderKind {
        match &self.provider {
            ProviderRuntime::Redis(_) => ProviderKind::Redis,
            ProviderRuntime::Etcd(_) => ProviderKind::Etcd,
            ProviderRuntime::Elasticsearch(_) => ProviderKind::Elasticsearch,
            ProviderRuntime::Kafka(_) => ProviderKind::Kafka,
            ProviderRuntime::Otlp(_) => ProviderKind::Otlp,
            ProviderRuntime::Webhook(_) => ProviderKind::Webhook,
        }
    }

    fn stop(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        match &self.provider {
            ProviderRuntime::Redis(runner) => runner.begin_delete(),
            ProviderRuntime::Etcd(runner) => runner.begin_delete(),
            ProviderRuntime::Elasticsearch(runner) => runner.begin_delete(),
            ProviderRuntime::Kafka(runner) => runner.begin_delete(),
            ProviderRuntime::Otlp(runner) => {
                runner.begin_delete();
                // OTLP already has a process-exit SDK drain. This tracks completion
                // only: it never delays replacement or recreates a removed key.
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    let runner = Arc::clone(runner);
                    let mut cleanup = OTLP_CLEANUP.lock();
                    reap_finished_otlp_cleanup(&mut cleanup);
                    cleanup.spawn_on(async move { runner.await_join().await }, &handle);
                }
            }
            ProviderRuntime::Webhook(client) => client.retire(),
        }
    }
}

impl Drop for RuntimeEntry {
    fn drop(&mut self) {
        self.stop();
    }
}

type RuntimeMap = HashMap<String, Arc<RuntimeEntry>>;
type ProviderCounts = [usize; 6];

/// Mutation-local admission counts: never a second published runtime authority.
fn count_providers(entries: &RuntimeMap) -> ProviderCounts {
    let mut counts = [0; 6];
    for entry in entries.values() {
        counts[entry.kind() as usize] += 1;
    }
    counts
}

/// One client authority for every provider. Writers perform no network IO here.
/// The mutation lock also makes cap admission and source sequence checks atomic.
struct RuntimeRegistry {
    entries: ArcSwap<RuntimeMap>,
    sequence: Mutex<u64>,
}

impl RuntimeRegistry {
    fn new() -> Self {
        Self {
            entries: ArcSwap::from_pointee(HashMap::new()),
            sequence: Mutex::new(0),
        }
    }

    fn get(&self, key: &str) -> Option<Arc<RuntimeEntry>> {
        self.entries.load().get(key).cloned()
    }

    fn keys(&self, kind: ProviderKind) -> Vec<String> {
        self.entries
            .load()
            .iter()
            .filter(|(_, entry)| entry.kind() == kind)
            .map(|(key, _)| key.clone())
            .collect()
    }

    fn count(&self, kind: ProviderKind) -> usize {
        self.entries
            .load()
            .values()
            .filter(|entry| entry.kind() == kind)
            .count()
    }

    fn apply(
        &self,
        sequence: u64,
        changed: &LinkSysMap,
        remove: &HashSet<String>,
        snapshot: &LinkSysMap,
        full: bool,
        forced: bool,
    ) {
        let mut accepted = self.sequence.lock();
        if sequence <= *accepted {
            return;
        }
        *accepted = sequence;
        let mut entries = (**self.entries.load()).clone();
        let mut released = false;
        entries.retain(|key, entry| {
            let removed = if full {
                !snapshot.contains_key(key)
            } else {
                remove.contains(key)
            };
            let wrong_kind = changed
                .get(key)
                .is_some_and(|resource| ProviderKind::of(&resource.spec.config) != Some(entry.kind()));
            if removed || wrong_kind {
                entry.stop();
                released = true;
                false
            } else {
                true
            }
        });
        let mut counts = count_providers(&entries);
        for (key, resource) in changed {
            apply_entry(&mut entries, &mut counts, key, resource, forced);
        }
        if released {
            admit_missing(&mut entries, &mut counts, snapshot);
        }
        self.entries.store(Arc::new(entries));
    }

    fn retry_admission(&self, sequence: u64, snapshot: &LinkSysMap) {
        let mut accepted = self.sequence.lock();
        if sequence <= *accepted {
            return;
        }
        *accepted = sequence;
        let mut entries = (**self.entries.load()).clone();
        let mut counts = count_providers(&entries);
        admit_missing(&mut entries, &mut counts, snapshot);
        self.entries.store(Arc::new(entries));
    }

    #[cfg(test)]
    fn install(&self, key: &str, entry: Option<Arc<RuntimeEntry>>) {
        let _guard = self.sequence.lock();
        let mut entries = (**self.entries.load()).clone();
        if let Some(old) = entries.remove(key) {
            old.stop();
        }
        if let Some(entry) = entry {
            entries.insert(key.to_string(), entry);
        }
        self.entries.store(Arc::new(entries));
    }
}

impl Drop for RuntimeRegistry {
    fn drop(&mut self) {
        for entry in self.entries.load().values() {
            entry.stop();
        }
    }
}

static RUNTIMES: LazyLock<RuntimeRegistry> = LazyLock::new(RuntimeRegistry::new);

fn admit_missing(entries: &mut RuntimeMap, counts: &mut ProviderCounts, snapshot: &LinkSysMap) {
    // Only unadmitted keys are retried. Existing runners must not receive a
    // synthetic replay that cancels their in-flight initialization.
    for (key, resource) in snapshot {
        if !entries.contains_key(key) {
            apply_entry(entries, counts, key, resource, false);
        }
    }
}

fn apply_entry(entries: &mut RuntimeMap, counts: &mut ProviderCounts, key: &str, resource: &LinkSys, forced: bool) {
    let Some(kind) = ProviderKind::of(&resource.spec.config) else {
        return;
    };
    if kind == ProviderKind::Otlp && OTLP_STOPPING.load(Ordering::Acquire) {
        return;
    }
    let existing = entries.get(key);
    let was_registered = existing.is_some();
    if kind != ProviderKind::Webhook
        && !was_registered
        && counts[kind as usize] >= get_link_sys_max_instances_per_kind()
    {
        // The source snapshot retains the desired configuration. Removal and
        // cap increases retry admission; there is no dropped desired-state queue.
        tracing::warn!(
            key,
            cap = get_link_sys_max_instances_per_kind(),
            "LinkSys instance cap reached; waiting for capacity"
        );
        return;
    }
    match &resource.spec.config {
        SystemConfig::Redis(_) => {
            let (ready, _, _) = credential_readiness(resource, credential::basic_state);
            if let Some(entry) = existing {
                if let ProviderRuntime::Redis(runner) = &entry.provider {
                    if ready {
                        runner.post_ready(Arc::new(resource.clone()), forced);
                    } else {
                        runner.post_not_ready();
                    }
                }
            } else if ready {
                let runner = PersistentRunner::<RedisApply>::spawn(key.to_string());
                runner.post_ready(Arc::new(resource.clone()), forced);
                entries.insert(key.to_string(), RuntimeEntry::new(ProviderRuntime::Redis(runner)));
            }
        }
        SystemConfig::Etcd(_) => {
            let (ready, _, _) = credential_readiness(resource, credential::basic_state);
            if let Some(entry) = existing {
                if let ProviderRuntime::Etcd(runner) = &entry.provider {
                    if ready {
                        runner.post_ready(Arc::new(resource.clone()), forced);
                    } else {
                        runner.post_not_ready();
                    }
                }
            } else if ready {
                let runner = PersistentRunner::<EtcdApply>::spawn(key.to_string());
                runner.post_ready(Arc::new(resource.clone()), forced);
                entries.insert(key.to_string(), RuntimeEntry::new(ProviderRuntime::Etcd(runner)));
            }
        }
        SystemConfig::Elasticsearch(_) => {
            let (ready, _, _) = credential_readiness(resource, credential::resolved_state);
            if let Some(entry) = existing {
                if let ProviderRuntime::Elasticsearch(runner) = &entry.provider {
                    if ready {
                        runner.post_ready(Arc::new(resource.clone()), forced);
                    } else {
                        runner.post_not_ready();
                    }
                }
            } else if ready {
                let runner = PersistentRunner::<EsApply>::spawn(key.to_string());
                runner.post_ready(Arc::new(resource.clone()), forced);
                entries.insert(
                    key.to_string(),
                    RuntimeEntry::new(ProviderRuntime::Elasticsearch(runner)),
                );
            }
        }
        SystemConfig::Kafka(_) => {
            let (ready, _, _) = credential_readiness(resource, credential::resolved_state);
            if let Some(entry) = existing {
                if let ProviderRuntime::Kafka(runner) = &entry.provider {
                    if ready {
                        runner.post_ready(Arc::new(resource.clone()), forced);
                    } else {
                        runner.post_not_ready();
                    }
                }
            } else if ready {
                let runner = PersistentRunner::<KafkaApply>::spawn(key.to_string());
                runner.post_ready(Arc::new(resource.clone()), forced);
                entries.insert(key.to_string(), RuntimeEntry::new(ProviderRuntime::Kafka(runner)));
            }
        }
        SystemConfig::Otlp(_) => {
            let (ready, _, _) = credential_readiness(resource, credential::resolved_state);
            if let Some(entry) = existing {
                if let ProviderRuntime::Otlp(runner) = &entry.provider {
                    if ready {
                        runner.post_ready(Arc::new(resource.clone()), forced);
                    } else {
                        runner.post_not_ready();
                    }
                }
            } else if ready {
                let runner = PersistentRunner::<OtlpApply>::spawn(key.to_string());
                runner.post_ready(Arc::new(resource.clone()), forced);
                entries.insert(key.to_string(), RuntimeEntry::new(ProviderRuntime::Otlp(runner)));
            }
        }
        SystemConfig::Webhook(config) => {
            if !credential::resolved_state(resource).is_buildable() || !webhook_secrets_ready(config) {
                return;
            }
            if let Some(entry) = existing {
                if let ProviderRuntime::Webhook(client) = &entry.provider {
                    if webhook_can_reuse(client, resource, forced) {
                        return;
                    }
                }
            }
            match WebhookLinkClient::prepare(
                key,
                config.clone(),
                resource.metadata.namespace.as_deref(),
                webhook_fingerprint_or_unspecified(resource),
            ) {
                Ok(client) => {
                    let entry = RuntimeEntry::new(ProviderRuntime::Webhook(Arc::new(client)));
                    if let Some(old) = entries.insert(key.to_string(), entry) {
                        old.stop();
                    }
                }
                Err(error) => tracing::error!(key, label = "Webhook", error = %error, "LinkSys preparation failed"),
            }
        }
        SystemConfig::HttpDns(_) | SystemConfig::CredentialSource(_) => {}
    }
    if !was_registered && entries.contains_key(key) {
        counts[kind as usize] += 1;
    }
}

const DEFAULT_LINK_SYS_MAX_INSTANCES_PER_KIND: usize = 200;
const LINK_SYS_MAX_INSTANCES_PER_KIND_LIMIT: usize = 10_000;
static LINK_SYS_MAX_INSTANCES_PER_KIND: AtomicUsize = AtomicUsize::new(DEFAULT_LINK_SYS_MAX_INSTANCES_PER_KIND);

pub(crate) fn get_link_sys_max_instances_per_kind() -> usize {
    LINK_SYS_MAX_INSTANCES_PER_KIND.load(Ordering::Relaxed)
}

/// The cap counts current registered owners, not retired handles held by callers.
pub(crate) fn configure_link_sys_max_instances_per_kind(n: usize) {
    let n = n.clamp(1, LINK_SYS_MAX_INSTANCES_PER_KIND_LIMIT);
    let previous = LINK_SYS_MAX_INSTANCES_PER_KIND.swap(n, Ordering::Relaxed);
    if n > previous {
        get_global_link_sys_store().retry_admission();
    }
    tracing::info!(cap = n, "LinkSys max instances per kind updated");
}

/// Return only the currently published Redis generation for this key.
pub fn get_redis_client(name: &str) -> Option<Arc<RedisLinkClient>> {
    let entry = RUNTIMES.get(name)?;
    match &entry.provider {
        ProviderRuntime::Redis(runner) => runner.current(),
        _ => None,
    }
}

pub fn list_redis_clients() -> Vec<String> {
    RUNTIMES.keys(ProviderKind::Redis)
}

pub fn redis_client_count() -> usize {
    RUNTIMES.count(ProviderKind::Redis)
}

pub fn redis_runner_exists(name: &str) -> bool {
    RUNTIMES
        .get(name)
        .is_some_and(|entry| entry.kind() == ProviderKind::Redis)
}

/// Return only the currently published Etcd generation for this key.
pub fn get_etcd_client(name: &str) -> Option<Arc<EtcdLinkClient>> {
    let entry = RUNTIMES.get(name)?;
    match &entry.provider {
        ProviderRuntime::Etcd(runner) => runner.current(),
        _ => None,
    }
}

pub fn list_etcd_clients() -> Vec<String> {
    RUNTIMES.keys(ProviderKind::Etcd)
}

pub fn etcd_client_count() -> usize {
    RUNTIMES.count(ProviderKind::Etcd)
}

pub fn etcd_runner_exists(name: &str) -> bool {
    RUNTIMES
        .get(name)
        .is_some_and(|entry| entry.kind() == ProviderKind::Etcd)
}

/// Return only the currently published Elasticsearch generation for this key.
pub fn get_es_client(name: &str) -> Option<Arc<EsLinkClient>> {
    let entry = RUNTIMES.get(name)?;
    match &entry.provider {
        ProviderRuntime::Elasticsearch(runner) => runner.current(),
        _ => None,
    }
}

pub fn list_es_clients() -> Vec<String> {
    RUNTIMES.keys(ProviderKind::Elasticsearch)
}

pub fn es_client_count() -> usize {
    RUNTIMES.count(ProviderKind::Elasticsearch)
}

pub fn es_runner_exists(name: &str) -> bool {
    RUNTIMES
        .get(name)
        .is_some_and(|entry| entry.kind() == ProviderKind::Elasticsearch)
}

/// Return only the currently published Kafka generation for this key.
pub fn get_kafka_client(name: &str) -> Option<Arc<KafkaLinkClient>> {
    let entry = RUNTIMES.get(name)?;
    match &entry.provider {
        ProviderRuntime::Kafka(runner) => runner.current(),
        _ => None,
    }
}

pub fn list_kafka_clients() -> Vec<String> {
    RUNTIMES.keys(ProviderKind::Kafka)
}

pub fn kafka_client_count() -> usize {
    RUNTIMES.count(ProviderKind::Kafka)
}

pub fn kafka_runner_exists(name: &str) -> bool {
    RUNTIMES
        .get(name)
        .is_some_and(|entry| entry.kind() == ProviderKind::Kafka)
}

/// Return only the currently published Otlp generation for this key.
pub fn get_otlp_client(name: &str) -> Option<Arc<OtlpLinkClient>> {
    let entry = RUNTIMES.get(name)?;
    match &entry.provider {
        ProviderRuntime::Otlp(runner) => runner.current(),
        _ => None,
    }
}

pub fn list_otlp_clients() -> Vec<String> {
    RUNTIMES.keys(ProviderKind::Otlp)
}

pub fn otlp_client_count() -> usize {
    RUNTIMES.count(ProviderKind::Otlp)
}

pub fn runtime_client_count() -> usize {
    RUNTIMES.entries.load().len()
}

pub fn list_runtime_clients() -> Vec<String> {
    RUNTIMES.entries.load().keys().cloned().collect()
}

pub fn get_webhook_client(name: &str) -> Option<Arc<WebhookLinkClient>> {
    let entry = RUNTIMES.get(name)?;
    match &entry.provider {
        ProviderRuntime::Webhook(client) => Some(Arc::clone(client)),
        _ => None,
    }
}

pub fn list_webhook_clients() -> Vec<String> {
    RUNTIMES.keys(ProviderKind::Webhook)
}

pub fn webhook_client_count() -> usize {
    RUNTIMES.count(ProviderKind::Webhook)
}

pub async fn health_check_all_redis() -> Vec<LinkSysHealth> {
    let clients: Vec<_> = RUNTIMES
        .entries
        .load()
        .values()
        .filter_map(|entry| match &entry.provider {
            ProviderRuntime::Redis(runner) => runner.current(),
            _ => None,
        })
        .collect();
    futures::future::join_all(
        clients
            .into_iter()
            .map(|client| async move { client.health_status().await }),
    )
    .await
}

pub async fn health_check_all_etcd() -> Vec<LinkSysHealth> {
    let clients: Vec<_> = RUNTIMES
        .entries
        .load()
        .values()
        .filter_map(|entry| match &entry.provider {
            ProviderRuntime::Etcd(runner) => runner.current(),
            _ => None,
        })
        .collect();
    futures::future::join_all(
        clients
            .into_iter()
            .map(|client| async move { client.health_status().await }),
    )
    .await
}

pub async fn health_check_all_es() -> Vec<LinkSysHealth> {
    let clients: Vec<_> = RUNTIMES
        .entries
        .load()
        .values()
        .filter_map(|entry| match &entry.provider {
            ProviderRuntime::Elasticsearch(runner) => runner.current(),
            _ => None,
        })
        .collect();
    futures::future::join_all(
        clients
            .into_iter()
            .map(|client| async move { client.health_status().await }),
    )
    .await
}

/// Health samples use exactly the same current generation as typed getters.
pub(crate) fn record_published_link_sys_health() {
    record_registry_health(&RUNTIMES);
}

fn record_registry_health(registry: &RuntimeRegistry) {
    for (name, entry) in registry.entries.load().iter() {
        let healthy = match &entry.provider {
            ProviderRuntime::Redis(runner) => runner.current().map(|client| client.healthy()),
            ProviderRuntime::Etcd(runner) => runner.current().map(|client| client.healthy()),
            ProviderRuntime::Elasticsearch(runner) => runner.current().map(|client| client.healthy()),
            ProviderRuntime::Kafka(runner) => runner.current().map(|client| client.healthy()),
            ProviderRuntime::Otlp(runner) => runner.current().map(|client| client.healthy()),
            ProviderRuntime::Webhook(_) => None,
        };
        if let Some(healthy) = healthy {
            record_link_sys_health(name, healthy);
        }
    }
}

static OTLP_CLEANUP: LazyLock<Mutex<tokio::task::JoinSet<()>>> =
    LazyLock::new(|| Mutex::new(tokio::task::JoinSet::new()));
static OTLP_SHUTDOWN: Mutex<Option<Shared<BoxFuture<'static, ()>>>> = Mutex::new(None);
static OTLP_STOPPING: AtomicBool = AtomicBool::new(false);

fn reap_finished_otlp_cleanup(cleanup: &mut tokio::task::JoinSet<()>) {
    while let Some(result) = cleanup.try_join_next() {
        if result.is_err() {
            tracing::warn!(system = "otlp", reason = "shutdown_failed", "OTLP cleanup task failed");
        }
    }
}

pub fn otlp_retiring_client_count() -> usize {
    let mut cleanup = OTLP_CLEANUP.lock();
    reap_finished_otlp_cleanup(&mut cleanup);
    cleanup.len()
}

/// Preserve the existing OTLP SDK process-exit drain, without gating ordinary CRUD.
pub async fn shutdown_otlp_clients() {
    let completion = {
        let mut shutdown = OTLP_SHUTDOWN.lock();
        shutdown
            .get_or_insert_with(|| {
                let mut cleanup = {
                    let _guard = RUNTIMES.sequence.lock();
                    OTLP_STOPPING.store(true, Ordering::Release);
                    let mut entries = (**RUNTIMES.entries.load()).clone();
                    entries.retain(|_, entry| {
                        if entry.kind() == ProviderKind::Otlp {
                            entry.stop();
                            false
                        } else {
                            true
                        }
                    });
                    RUNTIMES.entries.store(Arc::new(entries));
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

fn credential_readiness<M>(
    resource: &LinkSys,
    auth_fn: fn(&LinkSys) -> credential::CredentialState<M>,
) -> (bool, bool, credential::CredentialState<M>) {
    let tls_ok = credential::tls_ready(resource);
    let auth = auth_fn(resource);
    (auth.is_buildable() && tls_ok, tls_ok, auth)
}

fn webhook_secrets_ready(config: &edgion_resources::resources::link_sys::webhook::WebhookServiceConfig) -> bool {
    config.unresolved_secret_refs().is_empty()
}

fn webhook_fingerprint_or_unspecified(resource: &LinkSys) -> RuntimeFingerprint {
    fingerprint_webhook(resource).unwrap_or_else(|_| RuntimeFingerprint::unspecified())
}

fn webhook_can_reuse(existing: &WebhookLinkClient, resource: &LinkSys, forced: bool) -> bool {
    if forced {
        return false;
    }
    let incoming = webhook_fingerprint_or_unspecified(resource);
    let live = existing.runtime_fingerprint();
    live.equal_runtime(&incoming) || live.rv_skip_allows_reuse(&incoming)
}

async fn dispatch_full_set(sequence: u64, data: &LinkSysMap, forced: bool) {
    RUNTIMES.apply(sequence, data, &HashSet::new(), data, true, forced);
}

async fn dispatch_partial_update(sequence: u64, changed: &LinkSysMap, remove: &HashSet<String>, snapshot: &LinkSysMap) {
    RUNTIMES.apply(sequence, changed, remove, snapshot, false, false);
}

static GLOBAL_LINK_SYS_STORE: LazyLock<Arc<LinkSysStore>> = LazyLock::new(|| Arc::new(LinkSysStore::new()));

pub fn get_global_link_sys_store() -> Arc<LinkSysStore> {
    GLOBAL_LINK_SYS_STORE.clone()
}

#[cfg(test)]
pub(crate) static I5_DISPATCH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
pub(crate) static LINK_SYS_CAP_TEST_LOCK: parking_lot::ReentrantMutex<()> = parking_lot::ReentrantMutex::new(());

#[cfg(test)]
pub(crate) fn seed_published_otlp(key: String, client: Arc<OtlpLinkClient>) {
    let runner = PersistentRunner::<OtlpApply>::spawn_with_published(key.clone(), client);
    RUNTIMES.install(&key, Some(RuntimeEntry::new(ProviderRuntime::Otlp(runner))));
}

#[cfg(test)]
pub(crate) fn install_test_webhook(key: &str, client: Option<Arc<WebhookLinkClient>>) {
    RUNTIMES.install(
        key,
        client.map(|client| RuntimeEntry::new(ProviderRuntime::Webhook(client))),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link_sys::runtime::runner::ApplyOwner;
    use crate::plugins::http::common::outbound::target::ResolvedTarget;

    #[derive(Clone, Copy)]
    enum InjectedSecret {
        Missing,
        Valid,
        MalformedForInvalid,
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

    #[tokio::test(flavor = "current_thread")]
    async fn redis_relist_and_pending_update_preserve_published_generation() {
        let registry = RuntimeRegistry::new();
        let resource = create_redis_linksys("test", "link", InjectedSecret::Valid);
        let client = RedisApply::build("test/link", &resource).unwrap();
        let runner = PersistentRunner::<RedisApply>::spawn_with_published("test/link".to_string(), client.clone());
        registry.install(
            "test/link",
            Some(RuntimeEntry::new(ProviderRuntime::Redis(runner.clone()))),
        );
        let old = registry.get("test/link").unwrap();
        let data = one(resource);
        registry.apply(1, &data, &HashSet::new(), &data, true, false);
        assert!(Arc::ptr_eq(&old, &registry.get("test/link").unwrap()));
        assert!(Arc::ptr_eq(&client, &runner.current().unwrap()));
        let pending = one(create_redis_linksys("test", "link", InjectedSecret::Missing));
        registry.apply(2, &pending, &HashSet::new(), &pending, false, false);
        assert!(Arc::ptr_eq(&client, &runner.current().unwrap()));
        drop(registry);
        assert!(runner.current().is_none());
        runner.await_join().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn etcd_relist_and_pending_update_preserve_published_generation() {
        let registry = RuntimeRegistry::new();
        let resource = create_etcd_linksys("test", "link", InjectedSecret::Valid);
        let client = EtcdApply::build("test/link", &resource).unwrap();
        let runner = PersistentRunner::<EtcdApply>::spawn_with_published("test/link".to_string(), client.clone());
        registry.install(
            "test/link",
            Some(RuntimeEntry::new(ProviderRuntime::Etcd(runner.clone()))),
        );
        let old = registry.get("test/link").unwrap();
        let data = one(resource);
        registry.apply(1, &data, &HashSet::new(), &data, true, false);
        assert!(Arc::ptr_eq(&old, &registry.get("test/link").unwrap()));
        assert!(Arc::ptr_eq(&client, &runner.current().unwrap()));
        let pending = one(create_etcd_linksys("test", "link", InjectedSecret::Missing));
        registry.apply(2, &pending, &HashSet::new(), &pending, false, false);
        assert!(Arc::ptr_eq(&client, &runner.current().unwrap()));
        drop(registry);
        assert!(runner.current().is_none());
        runner.await_join().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn es_relist_and_pending_update_preserve_published_generation() {
        let registry = RuntimeRegistry::new();
        let resource = create_es_linksys("test", "link", InjectedSecret::Valid);
        let client = EsApply::build("test/link", &resource).unwrap();
        let runner = PersistentRunner::<EsApply>::spawn_with_published("test/link".to_string(), client.clone());
        registry.install(
            "test/link",
            Some(RuntimeEntry::new(ProviderRuntime::Elasticsearch(runner.clone()))),
        );
        let old = registry.get("test/link").unwrap();
        let data = one(resource);
        registry.apply(1, &data, &HashSet::new(), &data, true, false);
        assert!(Arc::ptr_eq(&old, &registry.get("test/link").unwrap()));
        assert!(Arc::ptr_eq(&client, &runner.current().unwrap()));
        let pending = one(create_es_linksys("test", "link", InjectedSecret::Missing));
        registry.apply(2, &pending, &HashSet::new(), &pending, false, false);
        assert!(Arc::ptr_eq(&client, &runner.current().unwrap()));
        drop(registry);
        assert!(runner.current().is_none());
        runner.await_join().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn kafka_relist_and_pending_update_preserve_published_generation() {
        let registry = RuntimeRegistry::new();
        let resource = create_kafka_linksys_sasl("test", "link", InjectedSecret::Valid);
        let client = KafkaApply::build("test/link", &resource).unwrap();
        let runner = PersistentRunner::<KafkaApply>::spawn_with_published("test/link".to_string(), client.clone());
        registry.install(
            "test/link",
            Some(RuntimeEntry::new(ProviderRuntime::Kafka(runner.clone()))),
        );
        let old = registry.get("test/link").unwrap();
        let data = one(resource);
        registry.apply(1, &data, &HashSet::new(), &data, true, false);
        assert!(Arc::ptr_eq(&old, &registry.get("test/link").unwrap()));
        assert!(Arc::ptr_eq(&client, &runner.current().unwrap()));
        let pending = one(create_kafka_linksys_sasl("test", "link", InjectedSecret::Missing));
        registry.apply(2, &pending, &HashSet::new(), &pending, false, false);
        assert!(Arc::ptr_eq(&client, &runner.current().unwrap()));
        drop(registry);
        assert!(runner.current().is_none());
        runner.await_join().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn otlp_relist_and_pending_update_preserve_published_generation() {
        let registry = RuntimeRegistry::new();
        let resource = create_otlp_linksys("test", "link", InjectedSecret::Valid);
        let client = OtlpApply::build("test/link", &resource).unwrap();
        let runner = PersistentRunner::<OtlpApply>::spawn_with_published("test/link".to_string(), client.clone());
        registry.install(
            "test/link",
            Some(RuntimeEntry::new(ProviderRuntime::Otlp(runner.clone()))),
        );
        let old = registry.get("test/link").unwrap();
        let data = one(resource);
        registry.apply(1, &data, &HashSet::new(), &data, true, false);
        assert!(Arc::ptr_eq(&old, &registry.get("test/link").unwrap()));
        assert!(Arc::ptr_eq(&client, &runner.current().unwrap()));
        let pending = one(create_otlp_linksys("test", "link", InjectedSecret::Missing));
        registry.apply(2, &pending, &HashSet::new(), &pending, false, false);
        assert!(Arc::ptr_eq(&client, &runner.current().unwrap()));
        drop(registry);
        assert!(runner.current().is_none());
        // OTLP stop owns its exit-drain join; registry deletion itself never waits.
    }

    #[tokio::test(flavor = "current_thread")]
    async fn old_probe_result_cannot_overwrite_published_health_gauge() {
        use metrics_util::debugging::{DebugValue, DebuggingRecorder};
        let registry = RuntimeRegistry::new();
        let resource = create_etcd_linksys("test", "link", InjectedSecret::Valid);
        let old_client = EtcdApply::build("test/link", &resource).unwrap();
        let old = PersistentRunner::<EtcdApply>::spawn_with_published("test/link".to_string(), old_client.clone());
        registry.install("test/link", Some(RuntimeEntry::new(ProviderRuntime::Etcd(old.clone()))));
        let new_client = EtcdApply::build("test/link", &resource).unwrap();
        let new = PersistentRunner::<EtcdApply>::spawn_with_published("test/link".to_string(), new_client);
        registry.install("test/link", Some(RuntimeEntry::new(ProviderRuntime::Etcd(new.clone()))));
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || {
            record_registry_health(&registry);
            let health = EtcdApply::published_health(&old_client).unwrap();
            crate::link_sys::runtime::health_probe::apply_health_result(
                health,
                "test/link",
                "etcd",
                &AtomicU64::new(0),
                true,
                false,
            );
        });
        assert!(snapshotter.snapshot().into_vec().iter().any(|(key, _, _, value)| {
            key.key().name() == crate::observe::metrics::registry::names::LINK_SYS_HEALTH
                && matches!(value, DebugValue::Gauge(level) if *level == 0.0)
        }));
        drop(registry);
        old.await_join().await;
        new.await_join().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn typed_getters_only_return_the_current_provider() {
        let _guard = I5_DISPATCH_LOCK.lock().await;
        let key = "test/link";
        let resources = resources(InjectedSecret::Valid);
        let entries = vec![
            RuntimeEntry::new(ProviderRuntime::Redis(
                PersistentRunner::<RedisApply>::spawn_with_published(
                    key.to_string(),
                    RedisApply::build(key, &resources[0]).unwrap(),
                ),
            )),
            RuntimeEntry::new(ProviderRuntime::Etcd(
                PersistentRunner::<EtcdApply>::spawn_with_published(
                    key.to_string(),
                    EtcdApply::build(key, &resources[1]).unwrap(),
                ),
            )),
            RuntimeEntry::new(ProviderRuntime::Elasticsearch(
                PersistentRunner::<EsApply>::spawn_with_published(
                    key.to_string(),
                    EsApply::build(key, &resources[2]).unwrap(),
                ),
            )),
            RuntimeEntry::new(ProviderRuntime::Kafka(
                PersistentRunner::<KafkaApply>::spawn_with_published(
                    key.to_string(),
                    KafkaApply::build(key, &resources[3]).unwrap(),
                ),
            )),
            RuntimeEntry::new(ProviderRuntime::Otlp(
                PersistentRunner::<OtlpApply>::spawn_with_published(
                    key.to_string(),
                    OtlpApply::build(key, &resources[4]).unwrap(),
                ),
            )),
            {
                let resource = &resources[5];
                let SystemConfig::Webhook(config) = &resource.spec.config else {
                    panic!("expected webhook")
                };
                RuntimeEntry::new(ProviderRuntime::Webhook(Arc::new(
                    WebhookLinkClient::prepare(
                        key,
                        config.clone(),
                        Some("test"),
                        webhook_fingerprint_or_unspecified(resource),
                    )
                    .unwrap(),
                )))
            },
        ];
        for (index, entry) in entries.into_iter().enumerate() {
            RUNTIMES.install(key, Some(entry));
            let visible = [
                get_redis_client(key).is_some(),
                get_etcd_client(key).is_some(),
                get_es_client(key).is_some(),
                get_kafka_client(key).is_some(),
                get_otlp_client(key).is_some(),
                get_webhook_client(key).is_some(),
            ];
            for (provider, callable) in visible.into_iter().enumerate() {
                assert_eq!(callable, provider == index);
            }
        }
        RUNTIMES.install(key, None);
        assert!(get_webhook_client(key).is_none());
        assert!(!list_runtime_clients().iter().any(|current| current == key));
    }

    fn source_webhook_port(store: &LinkSysStore, key: &str) -> i32 {
        let resource = store.get(key).expect("source contains webhook");
        let SystemConfig::Webhook(config) = resource.spec.config else {
            panic!("expected webhook")
        };
        config.conn.target.service.port.expect("test service has a port")
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
            |_, _, _, _| {
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

    fn resources(auth: InjectedSecret) -> Vec<LinkSys> {
        vec![
            create_redis_linksys("test", "link", auth),
            create_etcd_linksys("test", "link", auth),
            create_es_linksys("test", "link", auth),
            create_kafka_linksys_sasl("test", "link", auth),
            create_otlp_linksys("test", "link", auth),
            create_test_webhook_linksys("test", "link"),
        ]
    }

    fn one(resource: LinkSys) -> LinkSysMap {
        HashMap::from([("test/link".to_string(), resource)])
    }

    fn webhook_port(registry: &RuntimeRegistry, key: &str) -> i32 {
        let entry = registry.get(key).expect("webhook is registered");
        let ProviderRuntime::Webhook(client) = &entry.provider else {
            panic!("expected webhook")
        };
        match &client.managed_client().resolved_target {
            ResolvedTarget::Service { port, .. } => port.expect("test service has port"),
            other => panic!("expected service target, got {other:?}"),
        }
    }

    #[test]
    fn empty_unified_store_reports_no_residue() {
        let registry = RuntimeRegistry::new();
        assert!(registry.entries.load().is_empty());
        for resource in resources(InjectedSecret::Valid) {
            let kind = ProviderKind::of(&resource.spec.config).unwrap();
            assert_eq!(registry.count(kind), 0);
            assert!(registry.keys(kind).is_empty());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn every_provider_type_change_has_one_current_owner() {
        for old_resource in resources(InjectedSecret::Valid) {
            for new_resource in resources(InjectedSecret::Valid) {
                let registry = RuntimeRegistry::new();
                let old_data = one(old_resource.clone());
                registry.apply(1, &old_data, &HashSet::new(), &old_data, true, false);
                let old = registry.get("test/link").unwrap();
                let new_data = one(new_resource);
                registry.apply(2, &new_data, &HashSet::new(), &new_data, false, false);
                let current = registry.get("test/link").unwrap();
                assert_eq!(registry.entries.load().len(), 1);
                assert!(Some(current.kind()) == ProviderKind::of(&new_data["test/link"].spec.config));
                if old.kind() == current.kind() {
                    assert!(Arc::ptr_eq(&old, &current), "same kind reuses its owner");
                } else {
                    assert!(old.stopped.load(Ordering::Acquire), "old owner is cancelled");
                    assert!(!Arc::ptr_eq(&old, &current));
                }
                // Drop cancels every posted desired state before yielding; these
                // registry tests never initialize real network clients.
                drop(registry);
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn full_set_omission_and_delete_recreate_cannot_resurrect_old_owner() {
        for resource in resources(InjectedSecret::Valid) {
            let registry = RuntimeRegistry::new();
            let data = one(resource);
            registry.apply(1, &data, &HashSet::new(), &data, true, false);
            let old = registry.get("test/link").unwrap();
            registry.apply(2, &HashMap::new(), &HashSet::new(), &HashMap::new(), true, false);
            assert!(registry.entries.load().is_empty());
            assert!(old.stopped.load(Ordering::Acquire));
            registry.apply(3, &data, &HashSet::new(), &data, false, false);
            let new = registry.get("test/link").unwrap();
            assert!(!Arc::ptr_eq(&old, &new), "recreation is immediate and independent");
            old.stop();
            assert!(Arc::ptr_eq(&new, &registry.get("test/link").unwrap()));
            registry.apply(
                4,
                &HashMap::new(),
                &HashSet::from(["test/link".to_string()]),
                &HashMap::new(),
                false,
                false,
            );
            assert!(registry.entries.load().is_empty());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn metadata_only_kind_change_removes_previous_runtime() {
        let configurations = [
            SystemConfig::HttpDns(
                serde_json::from_value(serde_json::json!({
                    "preset": "tencent"
                }))
                .unwrap(),
            ),
            SystemConfig::CredentialSource(
                serde_json::from_value(serde_json::json!({
                    "provider": {
                        "type": "oauth2ClientCredentials",
                        "tokenEndpoint": "https://issuer.example/token",
                        "clientAuthentication": { "activeSecretRef": { "name": "bootstrap" } }
                    }
                }))
                .unwrap(),
            ),
        ];
        for config in configurations {
            let registry = RuntimeRegistry::new();
            let old_data = one(create_test_webhook_linksys("test", "link"));
            registry.apply(1, &old_data, &HashSet::new(), &old_data, true, false);
            let mut resource = old_data["test/link"].clone();
            resource.spec.config = config;
            let data = one(resource);
            registry.apply(2, &data, &HashSet::new(), &data, false, false);
            assert!(registry.get("test/link").is_none());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pending_and_invalid_credentials_never_admit_any_persistent_provider() {
        for auth in [InjectedSecret::Missing, InjectedSecret::MalformedForInvalid] {
            for resource in resources(auth).into_iter().take(5) {
                for full in [true, false] {
                    let registry = RuntimeRegistry::new();
                    let data = one(resource.clone());
                    registry.apply(1, &data, &HashSet::new(), &data, full, false);
                    assert!(registry.entries.load().is_empty());
                }
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pending_same_kind_retains_owner_but_changed_kind_removes_it() {
        let ready = resources(InjectedSecret::Valid);
        let pending = resources(InjectedSecret::Missing);
        for (index, resource) in ready.iter().take(5).enumerate() {
            let registry = RuntimeRegistry::new();
            let data = one(resource.clone());
            registry.apply(1, &data, &HashSet::new(), &data, true, false);
            let old = registry.get("test/link").unwrap();
            let pending_same = one(pending[index].clone());
            registry.apply(2, &pending_same, &HashSet::new(), &pending_same, false, false);
            assert!(Arc::ptr_eq(&old, &registry.get("test/link").unwrap()));
            let changed = one(pending[(index + 1) % 5].clone());
            registry.apply(3, &changed, &HashSet::new(), &changed, false, false);
            assert!(registry.get("test/link").is_none());
            assert!(old.stopped.load(Ordering::Acquire));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mixed_provider_admission_counts_are_independent_and_recomputed_after_removal() {
        let _cap_guard = LINK_SYS_CAP_TEST_LOCK.lock();
        let previous = LINK_SYS_MAX_INSTANCES_PER_KIND.swap(1, Ordering::Relaxed);
        let registry = RuntimeRegistry::new();
        let resources = resources(InjectedSecret::Valid);
        let mut snapshot = HashMap::new();
        for (index, resource) in resources.iter().enumerate() {
            snapshot.insert(format!("test/first-{index}"), resource.clone());
            snapshot.insert(format!("test/second-{index}"), resource.clone());
        }
        registry.apply(1, &snapshot, &HashSet::new(), &snapshot, true, false);
        let mut remove = HashSet::new();
        for resource in &resources {
            let kind = ProviderKind::of(&resource.spec.config).unwrap();
            assert_eq!(registry.count(kind), if kind == ProviderKind::Webhook { 2 } else { 1 });
            if kind != ProviderKind::Webhook {
                remove.extend(registry.keys(kind));
            }
        }
        for key in &remove {
            snapshot.remove(key);
        }
        registry.apply(2, &HashMap::new(), &remove, &snapshot, false, false);
        assert_eq!(registry.entries.load().len(), 7);
        for key in &remove {
            assert!(registry.get(key).is_none());
        }
        for resource in &resources {
            let kind = ProviderKind::of(&resource.spec.config).unwrap();
            assert_eq!(registry.count(kind), if kind == ProviderKind::Webhook { 2 } else { 1 });
        }
        LINK_SYS_MAX_INSTANCES_PER_KIND.store(previous, Ordering::Relaxed);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn capacity_release_retries_latest_unadmitted_configuration() {
        let _cap_guard = LINK_SYS_CAP_TEST_LOCK.lock();
        let previous = LINK_SYS_MAX_INSTANCES_PER_KIND.swap(1, Ordering::Relaxed);
        for resource in resources(InjectedSecret::Valid).into_iter().take(5) {
            let registry = RuntimeRegistry::new();
            let first = HashMap::from([("test/first".to_string(), resource.clone())]);
            registry.apply(1, &first, &HashSet::new(), &first, true, false);
            let changed = HashMap::from([("test/second".to_string(), resource.clone())]);
            let mut snapshot = first;
            snapshot.extend(changed.clone());
            registry.apply(2, &changed, &HashSet::new(), &snapshot, false, false);
            assert!(registry.get("test/first").is_some());
            assert!(registry.get("test/second").is_none());
            let mut latest = resource;
            latest.metadata.resource_version = Some("latest".to_string());
            match &mut latest.spec.config {
                SystemConfig::Redis(config) => config.endpoints = vec!["redis://127.0.0.1:6380".to_string()],
                SystemConfig::Etcd(config) => config.endpoints = vec!["http://127.0.0.1:2378".to_string()],
                SystemConfig::Elasticsearch(config) => config.endpoints = vec!["http://127.0.0.1:9201".to_string()],
                SystemConfig::Kafka(config) => config.brokers = vec!["127.0.0.1:9093".to_string()],
                SystemConfig::Otlp(config) => config.endpoint = "http://192.0.2.1:4318".to_string(),
                _ => panic!("test covers persistent providers only"),
            }
            let latest_delta = HashMap::from([("test/second".to_string(), latest.clone())]);
            snapshot.extend(latest_delta.clone());
            registry.apply(3, &latest_delta, &HashSet::new(), &snapshot, false, false);
            assert!(registry.get("test/second").is_none());
            snapshot.remove("test/first");
            registry.apply(
                4,
                &HashMap::new(),
                &HashSet::from(["test/first".to_string()]),
                &snapshot,
                false,
                false,
            );
            assert!(registry.get("test/first").is_none());
            assert!(
                registry.get("test/second").is_some(),
                "capacity release must recover without another apply"
            );
            let admitted = registry.get("test/second").unwrap();
            let desired = match &admitted.provider {
                ProviderRuntime::Redis(runner) => runner.desired_link_for_test(),
                ProviderRuntime::Etcd(runner) => runner.desired_link_for_test(),
                ProviderRuntime::Elasticsearch(runner) => runner.desired_link_for_test(),
                ProviderRuntime::Kafka(runner) => runner.desired_link_for_test(),
                ProviderRuntime::Otlp(runner) => runner.desired_link_for_test(),
                ProviderRuntime::Webhook(_) => panic!("test covers persistent providers only"),
            }
            .unwrap();
            assert_eq!(desired.metadata.resource_version.as_deref(), Some("latest"));
            assert_eq!(
                serde_json::to_value(&desired.spec).unwrap(),
                serde_json::to_value(&latest.spec).unwrap()
            );
        }
        LINK_SYS_MAX_INSTANCES_PER_KIND.store(previous, Ordering::Relaxed);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn capacity_increase_retries_without_reposting_existing_runner() {
        let _cap_guard = LINK_SYS_CAP_TEST_LOCK.lock();
        let previous = LINK_SYS_MAX_INSTANCES_PER_KIND.swap(1, Ordering::Relaxed);
        let registry = RuntimeRegistry::new();
        let resource = create_redis_linksys("test", "link", InjectedSecret::Valid);
        let first = HashMap::from([("test/first".to_string(), resource.clone())]);
        registry.apply(1, &first, &HashSet::new(), &first, true, false);
        let old = registry.get("test/first").unwrap();
        let mut snapshot = first;
        snapshot.insert("test/second".to_string(), resource);
        registry.retry_admission(2, &snapshot);
        assert!(registry.get("test/second").is_none());
        LINK_SYS_MAX_INSTANCES_PER_KIND.store(2, Ordering::Relaxed);
        registry.retry_admission(3, &snapshot);
        assert!(registry.get("test/second").is_some());
        assert!(Arc::ptr_eq(&old, &registry.get("test/first").unwrap()));
        LINK_SYS_MAX_INSTANCES_PER_KIND.store(previous, Ordering::Relaxed);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn omitted_capacity_waiter_is_not_recreated_after_release() {
        let _cap_guard = LINK_SYS_CAP_TEST_LOCK.lock();
        let previous = LINK_SYS_MAX_INSTANCES_PER_KIND.swap(1, Ordering::Relaxed);
        let registry = RuntimeRegistry::new();
        let resource = create_redis_linksys("test", "link", InjectedSecret::Valid);
        let first = HashMap::from([("test/first".to_string(), resource.clone())]);
        registry.apply(1, &first, &HashSet::new(), &first, true, false);
        let mut snapshot = first;
        snapshot.insert("test/second".to_string(), resource);
        registry.retry_admission(2, &snapshot);
        registry.apply(3, &HashMap::new(), &HashSet::new(), &HashMap::new(), true, false);
        registry.retry_admission(4, &HashMap::new());
        assert!(registry.entries.load().is_empty());
        LINK_SYS_MAX_INSTANCES_PER_KIND.store(previous, Ordering::Relaxed);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn webhook_relist_reuses_and_material_update_replaces_current_client() {
        let registry = RuntimeRegistry::new();
        let data = one(create_test_webhook_linksys("test", "link"));
        registry.apply(1, &data, &HashSet::new(), &data, true, false);
        let old = registry.get("test/link").unwrap();
        registry.apply(2, &data, &HashSet::new(), &data, true, false);
        assert!(Arc::ptr_eq(&old, &registry.get("test/link").unwrap()));
        let changed = one(create_test_webhook_linksys_with_port("test", "link", 9090));
        registry.apply(3, &changed, &HashSet::new(), &changed, false, false);
        assert!(old.stopped.load(Ordering::Acquire));
        assert_eq!(webhook_port(&registry, "test/link"), 9090);
        let current = registry.get("test/link").unwrap();
        registry.apply(4, &changed, &HashSet::new(), &changed, true, true);
        assert!(!Arc::ptr_eq(&current, &registry.get("test/link").unwrap()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn webhook_prepare_failure_preserves_same_kind_but_not_foreign_kind() {
        let registry = RuntimeRegistry::new();
        let data = one(create_test_webhook_linksys("test", "link"));
        registry.apply(1, &data, &HashSet::new(), &data, true, false);
        let old = registry.get("test/link").unwrap();
        let mut invalid = data["test/link"].clone();
        let SystemConfig::Webhook(config) = &mut invalid.spec.config else {
            panic!("expected webhook")
        };
        config.conn.target.service.port = None;
        let invalid = one(invalid);
        registry.apply(2, &invalid, &HashSet::new(), &invalid, false, false);
        assert!(Arc::ptr_eq(&old, &registry.get("test/link").unwrap()));
        let redis = one(create_redis_linksys("test", "link", InjectedSecret::Valid));
        registry.apply(3, &redis, &HashSet::new(), &redis, false, false);
        registry.apply(4, &invalid, &HashSet::new(), &invalid, false, false);
        assert!(registry.get("test/link").is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stale_full_or_partial_dispatch_cannot_replace_newer_current() {
        for full in [true, false] {
            let registry = RuntimeRegistry::new();
            let latest = one(create_test_webhook_linksys_with_port("test", "link", 9090));
            registry.apply(2, &latest, &HashSet::new(), &latest, full, false);
            let stale = one(create_test_webhook_linksys("test", "link"));
            registry.apply(1, &stale, &HashSet::new(), &stale, !full, false);
            assert_eq!(webhook_port(&registry, "test/link"), 9090);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ordered_dispatch_preserves_unrelated_deltas_and_captured_snapshot() {
        let source = LinkSysStore::new();
        let registry = Arc::new(RuntimeRegistry::new());
        let counter = AtomicU64::new(0);
        let source_lock = Mutex::new(());
        let tail = Mutex::new(None);
        let (release, gate) = tokio::sync::oneshot::channel();
        let first = HashMap::from([("test/first".to_string(), create_test_webhook_linksys("test", "first"))]);
        let captured = Mutex::new(None);
        source
            .update_with_dispatch(
                first,
                &HashSet::new(),
                &counter,
                &source_lock,
                |sequence, changed, remove, snapshot| {
                    *captured.lock() = Some(snapshot.clone());
                    let registry = registry.clone();
                    spawn_ordered_dispatch(&tokio::runtime::Handle::current(), &tail, async move {
                        gate.await.unwrap();
                        registry.apply(sequence, &changed, &remove, &snapshot, false, false);
                    });
                },
            )
            .unwrap();
        let second = HashMap::from([("test/second".to_string(), create_test_webhook_linksys("test", "second"))]);
        source
            .update_with_dispatch(
                second,
                &HashSet::new(),
                &counter,
                &source_lock,
                |sequence, changed, remove, snapshot| {
                    let registry = registry.clone();
                    spawn_ordered_dispatch(&tokio::runtime::Handle::current(), &tail, async move {
                        registry.apply(sequence, &changed, &remove, &snapshot, false, false);
                    });
                },
            )
            .unwrap();
        assert_eq!(
            captured.lock().as_ref().unwrap().len(),
            1,
            "captured snapshot must not observe later source events"
        );
        release.send(()).unwrap();
        let task = tail.lock().take().unwrap();
        task.await.unwrap();
        assert_eq!(
            registry.entries.load().len(),
            2,
            "later unrelated delta must not skip its predecessor"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ordered_dispatch_releases_successor_after_predecessor_failure() {
        let predecessor = tokio::spawn(async { panic!("injected predecessor failure") });
        let ran = Arc::new(AtomicBool::new(false));
        let observed = ran.clone();
        dispatch_after(
            Some(predecessor),
            async move { observed.store(true, Ordering::Release) },
        )
        .await;
        assert!(ran.load(Ordering::Acquire));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn published_etcd_client_is_sampled_without_network_init() {
        use metrics_util::debugging::{DebugValue, DebuggingRecorder};
        let registry = RuntimeRegistry::new();
        let resource = create_etcd_linksys("test", "link", InjectedSecret::Valid);
        let client = EtcdApply::build("test/link", &resource).unwrap();
        let runner = PersistentRunner::<EtcdApply>::spawn_with_published("test/link".to_string(), client);
        registry.install(
            "test/link",
            Some(RuntimeEntry::new(ProviderRuntime::Etcd(runner.clone()))),
        );
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || record_registry_health(&registry));
        assert!(snapshotter.snapshot().into_vec().iter().any(|(key, _, _, value)| {
            key.key().name() == crate::observe::metrics::registry::names::LINK_SYS_HEALTH
                && matches!(value, DebugValue::Gauge(level) if *level == 0.0)
        }));
        drop(registry);
        runner.await_join().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn last_registry_owner_drop_stops_runner_without_waiting_for_arc_drop() {
        let registry = RuntimeRegistry::new();
        let resource = create_etcd_linksys("test", "link", InjectedSecret::Valid);
        let client = EtcdApply::build("test/link", &resource).unwrap();
        let runner = PersistentRunner::<EtcdApply>::spawn_with_published("test/link".to_string(), client);
        registry.install(
            "test/link",
            Some(RuntimeEntry::new(ProviderRuntime::Etcd(runner.clone()))),
        );
        assert!(runner.current().is_some());
        drop(registry);
        assert!(runner.current().is_none());
        runner.await_join().await;
    }
}
