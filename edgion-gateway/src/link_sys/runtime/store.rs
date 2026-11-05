//! LinkSys source snapshots and the single typed runtime registry.
//!
//! Provider owners handle same-kind updates. Removal immediately hides the key
//! and signals the old owner; cleanup never writes back into the registry.

use arc_swap::ArcSwap;
use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};

use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};

use crate::link_sys::credential;
use crate::link_sys::providers::elasticsearch::{EsApply, EsLinkClient};
use crate::link_sys::providers::etcd::{EtcdApply, EtcdLinkClient};
use crate::link_sys::providers::kafka::{KafkaApply, KafkaLinkClient};
use crate::link_sys::providers::otlp::{OtlpApply, OtlpLinkClient};
use crate::link_sys::providers::redis::{RedisApply, RedisLinkClient};
use crate::link_sys::providers::webhook::{WebhookCandidate, WebhookLinkClient};
use crate::link_sys::runtime::runner::PersistentRunner;
use crate::link_sys::LinkSysHealth;
use crate::observe::metrics::registry::{record_link_sys_health, record_link_sys_redis_connections};

// ============================================================
// Type aliases
// ============================================================

type LinkSysMap = HashMap<String, LinkSys>;

// Orders source publication, runtime preparation, rebuilds, admission retries,
// and the synchronous part of OTLP shutdown. Readers never acquire this lock.
static LINK_SYS_SOURCE_EVENT_LOCK: Mutex<()> = Mutex::new(());

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
        self.replace_all_with_dispatch(data, &LINK_SYS_SOURCE_EVENT_LOCK, |dispatch_data| {
            if tokio::runtime::Handle::try_current().is_ok() {
                dispatch_full_set(&dispatch_data, false);
            } else {
                // Reachable only outside a tokio runtime (currently: the four sync
                // unit tests that exercise the resource map only). Production
                // callers all run inside tokio; if this fires there, provider
                // clients were never built and `get_*_client` will look like
                // "config not synced yet" — surface it instead of failing silently.
                tracing::error!("LinkSysStore::replace_all called outside a tokio runtime — dispatch skipped");
            }
        });
    }

    /// Update LinkSys resources atomically (incremental sync).
    ///
    /// Dispatches only changed keys, plus unadmitted keys when capacity is freed.
    pub fn update(&self, add_or_update: HashMap<String, LinkSys>, remove: &HashSet<String>) {
        self.update_with_dispatch(
            add_or_update,
            remove,
            &LINK_SYS_SOURCE_EVENT_LOCK,
            |dispatch_data, remove_set, snapshot| {
                if tokio::runtime::Handle::try_current().is_ok() {
                    dispatch_partial_update(&dispatch_data, &remove_set, &snapshot);
                } else {
                    // See `replace_all` for the rationale — this branch is only hit by
                    // sync unit tests today; logging makes any future outside-runtime
                    // regression visible rather than presenting as "config not synced".
                    tracing::error!("LinkSysStore::update called outside a tokio runtime — dispatch skipped");
                }
            },
        );
    }

    /// Rebuild every runtime client from the current authoritative snapshot.
    ///
    /// LinkSys clients resolve inherited process-wide settings when they are
    /// constructed. A change to one of those settings therefore needs a fresh
    /// dispatch even though the LinkSys resources themselves did not change.
    pub fn rebuild_clients(&self) {
        self.rebuild_clients_with_dispatch(&LINK_SYS_SOURCE_EVENT_LOCK, |dispatch_data| {
            if tokio::runtime::Handle::try_current().is_ok() {
                dispatch_full_set(&dispatch_data, true);
            } else {
                tracing::error!("LinkSysStore::rebuild_clients called outside a tokio runtime — dispatch skipped");
            }
        });
    }

    /// Reconsider unadmitted keys after a cap increase without rebuilding live clients.
    fn retry_admission(&self) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        self.rebuild_clients_with_dispatch(&LINK_SYS_SOURCE_EVENT_LOCK, |snapshot| {
            RUNTIMES.retry_admission(&snapshot);
        });
    }

    fn publish_source_event(
        &self,
        source_lock: &Mutex<()>,
        build_snapshot: impl FnOnce(&LinkSysMap) -> Arc<LinkSysMap>,
        dispatch: impl FnOnce(Arc<LinkSysMap>),
    ) {
        let _source_guard = source_lock.lock();
        let current = self.resources.load_full();
        let next = build_snapshot(current.as_ref());

        // Prepare and publish runtime handles within the source-event order.
        // Readers cannot observe the new raw generation before its preparation.
        dispatch(Arc::clone(&next));
        self.resources.store(next);
    }

    fn replace_all_with_dispatch(
        &self,
        data: HashMap<String, LinkSys>,
        source_lock: &Mutex<()>,
        dispatch: impl FnOnce(Arc<LinkSysMap>),
    ) {
        // Wrap once and share the exact authoritative snapshot with both the
        // ArcSwap publication and the synchronous dispatch.
        let data = Arc::new(data);
        let dispatch_data = Arc::clone(&data);
        self.publish_source_event(source_lock, |_| Arc::clone(&data), |_| dispatch(dispatch_data))
    }

    fn update_with_dispatch(
        &self,
        add_or_update: HashMap<String, LinkSys>,
        remove: &HashSet<String>,
        source_lock: &Mutex<()>,
        dispatch: impl FnOnce(Arc<LinkSysMap>, HashSet<String>, Arc<LinkSysMap>),
    ) {
        // Wrap once and share with the synchronous dispatch to avoid a second deep clone
        // of the delta (each LinkSys carries nested credential structs).
        let add_or_update = Arc::new(add_or_update);
        let dispatch_data = Arc::clone(&add_or_update);
        let remove_set = remove.clone();
        self.publish_source_event(
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
            |snapshot| dispatch(dispatch_data, remove_set, snapshot),
        )
    }

    fn rebuild_clients_with_dispatch(&self, source_lock: &Mutex<()>, dispatch: impl FnOnce(Arc<LinkSysMap>)) {
        let _source_guard = source_lock.lock();
        let snapshot = self.resources.load_full();

        // Dispatch under the same lock used by LinkSys source
        // events. This makes the dependency-triggered rebuild part of their
        // total order without republishing or cloning the source snapshot.
        dispatch(snapshot);
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
    HttpDns,
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
            SystemConfig::HttpDns(_) => Some(Self::HttpDns),
            SystemConfig::CredentialSource(_) | SystemConfig::Unsupported => None,
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
    HttpDns(Arc<edgion_common::dns::PreparedHttpDns>),
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
            ProviderRuntime::HttpDns(_) => ProviderKind::HttpDns,
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
            ProviderRuntime::HttpDns(_) => {}
        }
    }
}

impl Drop for RuntimeEntry {
    fn drop(&mut self) {
        self.stop();
    }
}

type RuntimeMap = HashMap<String, Arc<RuntimeEntry>>;
type ProviderCounts = [usize; 7];

/// Mutation-local admission counts: never a second published runtime authority.
fn count_providers(entries: &RuntimeMap) -> ProviderCounts {
    let mut counts = [0; 7];
    for entry in entries.values() {
        counts[entry.kind() as usize] += 1;
    }
    counts
}

/// One client authority for every provider. Writers perform no network IO here.
/// Callers hold LINK_SYS_SOURCE_EVENT_LOCK through each read-modify-publish.
struct RuntimeRegistry {
    entries: ArcSwap<RuntimeMap>,
}

impl RuntimeRegistry {
    fn new() -> Self {
        Self {
            entries: ArcSwap::from_pointee(HashMap::new()),
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

    fn apply(&self, changed: &LinkSysMap, remove: &HashSet<String>, snapshot: &LinkSysMap, full: bool, forced: bool) {
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

    fn retry_admission(&self, snapshot: &LinkSysMap) {
        let mut entries = (**self.entries.load()).clone();
        let mut counts = count_providers(&entries);
        admit_missing(&mut entries, &mut counts, snapshot);
        self.entries.store(Arc::new(entries));
    }

    #[cfg(test)]
    fn install(&self, key: &str, entry: Option<Arc<RuntimeEntry>>) {
        let _guard = LINK_SYS_SOURCE_EVENT_LOCK.lock();
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
    // A forced rebuild follows a global outbound TLS policy/material change.
    // Revoke the old Webhook publication even if the replacement is not ready or
    // fails preparation; it may carry credentials that are no longer permitted.
    if forced && kind == ProviderKind::Webhook {
        if let Some(old) = entries.remove(key) {
            old.stop();
            counts[kind as usize] -= 1;
        }
    }
    let existing = entries.get(key);
    let was_registered = existing.is_some();
    if kind != ProviderKind::Webhook
        && kind != ProviderKind::HttpDns
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
                        runner.post_not_ready(forced);
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
                        runner.post_not_ready(forced);
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
                        runner.post_not_ready(forced);
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
                        runner.post_not_ready(forced);
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
                        runner.post_not_ready(forced);
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
            let incoming = WebhookCandidate::from_link_sys(Arc::new(resource.clone()))
                .expect("the Webhook publication branch has the matching actual provider");
            if let Some(entry) = existing {
                if let ProviderRuntime::Webhook(client) = &entry.provider {
                    if incoming.can_reuse(client, forced) {
                        return;
                    }
                }
            }
            match incoming.prepare(key) {
                Ok(client) => {
                    let entry = RuntimeEntry::new(ProviderRuntime::Webhook(Arc::new(client)));
                    if let Some(old) = entries.insert(key.to_string(), entry) {
                        old.stop();
                    }
                }
                Err(error) => tracing::error!(key, label = "Webhook", error = %error, "LinkSys preparation failed"),
            }
        }
        SystemConfig::HttpDns(_) => {
            let incoming = edgion_common::dns::HttpDnsCandidate::from_link_sys(Arc::new(resource.clone()))
                .expect("the HTTPDNS publication branch has the matching actual provider");
            if !forced {
                if let Some(entry) = existing {
                    if let ProviderRuntime::HttpDns(prepared) = &entry.provider {
                        if incoming.matches(prepared) {
                            return;
                        }
                    }
                }
            }
            let prepared = incoming.prepare();
            if !prepared.is_ready() {
                tracing::warn!(
                    key,
                    label = "HttpDns",
                    "LinkSys preparation failed; using configured fallback"
                );
            }
            let entry = RuntimeEntry::new(ProviderRuntime::HttpDns(prepared));
            if let Some(old) = entries.insert(key.to_string(), entry) {
                old.stop();
            }
        }
        SystemConfig::CredentialSource(_) | SystemConfig::Unsupported => {}
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

/// The current HTTP DNS generation, including preparation-failed fallback states.
pub fn get_httpdns_client(name: &str) -> Option<Arc<edgion_common::dns::PreparedHttpDns>> {
    let entry = RUNTIMES.get(name)?;
    match &entry.provider {
        ProviderRuntime::HttpDns(prepared) => Some(Arc::clone(prepared)),
        _ => None,
    }
}

pub fn list_httpdns_clients() -> Vec<String> {
    RUNTIMES.keys(ProviderKind::HttpDns)
}

pub fn httpdns_client_count() -> usize {
    RUNTIMES.count(ProviderKind::HttpDns)
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
            ProviderRuntime::Redis(runner) => runner.current().map(|client| {
                let coverage = client.connection_coverage();
                record_link_sys_redis_connections(name, coverage.live, coverage.configured);
                client.healthy()
            }),
            ProviderRuntime::Etcd(runner) => runner.current().map(|client| client.healthy()),
            ProviderRuntime::Elasticsearch(runner) => runner.current().map(|client| client.healthy()),
            ProviderRuntime::Kafka(runner) => runner.current().map(|client| client.healthy()),
            ProviderRuntime::Otlp(runner) => runner.current().map(|client| client.healthy()),
            ProviderRuntime::Webhook(_) | ProviderRuntime::HttpDns(_) => None,
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
                    let _guard = LINK_SYS_SOURCE_EVENT_LOCK.lock();
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

fn dispatch_full_set(data: &LinkSysMap, forced: bool) {
    RUNTIMES.apply(data, &HashSet::new(), data, true, forced);
}

fn dispatch_partial_update(changed: &LinkSysMap, remove: &HashSet<String>, snapshot: &LinkSysMap) {
    RUNTIMES.apply(changed, remove, snapshot, false, false);
}

static GLOBAL_LINK_SYS_STORE: LazyLock<Arc<LinkSysStore>> = LazyLock::new(|| Arc::new(LinkSysStore::new()));

pub fn get_global_link_sys_store() -> Arc<LinkSysStore> {
    GLOBAL_LINK_SYS_STORE.clone()
}

#[cfg(test)]
include!("../../../tests/support/link_sys/runtime/store_test/999_static_item.rs");

#[cfg(test)]
include!("../../../tests/support/link_sys/runtime/store_test/1002_static_item.rs");

#[cfg(test)]
include!("../../../tests/support/link_sys/runtime/store_test/1005_function_item.rs");

#[cfg(test)]
include!("../../../tests/support/link_sys/runtime/store_test/1011_function_item.rs");

#[cfg(test)]
#[path = "../../../tests/unit/link_sys/runtime/store_test.rs"]
mod tests;
