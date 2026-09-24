//! Generic ResourceController for Kubernetes resources
//!
//! Each ResourceController runs a completely independent lifecycle:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────────┐
//! │                    ResourceController<K> Independent Flow                    │
//! ├─────────────────────────────────────────────────────────────────────────────┤
//! │  Step 1: Create Store (Reflector)                                          │
//! │  Step 2-6: Handle Init phase (LIST + InitApply + Ready)                    │
//! │  Step 7-8: Handle Runtime phase (WATCH + Workqueue)                        │
//! └─────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! Key features:
//! - Each resource type runs completely independently
//! - One watcher per configured namespace, or one cluster-wide watcher
//! - Each source commits its native snapshot independently
//! - One complete bootstrap replay, then all runtime changes reconcile through one workqueue
//! - Graceful shutdown support via ShutdownSignal

use super::controller_metrics;
use super::leader_election::LeaderHandle;
use super::namespace::NamespaceWatchMode;
use super::watch_source::{SourceState, WatchScope};
use super::InitSyncTimer;
use super::ShutdownSignal;
use crate::conf_mgr::sync_runtime::resource_processor::{
    make_resource_key,
    raw_store::RawStore,
    status_ownership::{ownership_dependency_kinds, ownership_dependency_signature},
    KubeRawStore, ResourceProcessor, WorkItemResult,
};
use anyhow::Result;
use edgion_resources::ResourceMeta;
use futures::{Stream, StreamExt};
use kube::api::{Patch, PatchParams};
use kube::runtime::watcher;
use kube::runtime::watcher::Event;
use kube::runtime::WatchStreamExt;
use kube::{Api, Client, Resource, ResourceExt};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::hash::Hash;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::{
    sync::CancellationToken,
    task::{AbortOnDropHandle, TaskTracker},
};

fn canonical_http_route_event(event: Event<edgion_resources::HTTPRouteBeta>) -> Event<edgion_resources::HTTPRoute> {
    match event {
        Event::Init => Event::Init,
        Event::InitApply(route) => Event::InitApply(route.into()),
        Event::InitDone => Event::InitDone,
        Event::Apply(route) => Event::Apply(route.into()),
        Event::Delete(route) => Event::Delete(route.into()),
    }
}

fn canonical_gateway_event(event: Event<edgion_resources::GatewayBeta>) -> Event<edgion_resources::Gateway> {
    match event {
        Event::Init => Event::Init,
        Event::InitApply(gateway) => Event::InitApply(gateway.into()),
        Event::InitDone => Event::InitDone,
        Event::Apply(gateway) => Event::Apply(gateway.into()),
        Event::Delete(gateway) => Event::Delete(gateway.into()),
    }
}

fn canonical_gateway_class_event(
    event: Event<edgion_resources::GatewayClassBeta>,
) -> Event<edgion_resources::GatewayClass> {
    match event {
        Event::Init => Event::Init,
        Event::InitApply(class) => Event::InitApply(class.into()),
        Event::InitDone => Event::InitDone,
        Event::Apply(class) => Event::Apply(class.into()),
        Event::Delete(class) => Event::Delete(class.into()),
    }
}

fn canonical_reference_grant_event(
    event: Event<edgion_resources::ReferenceGrantBeta>,
) -> Event<edgion_resources::ReferenceGrant> {
    match event {
        Event::Init => Event::Init,
        Event::InitApply(grant) => Event::InitApply(grant.into()),
        Event::InitDone => Event::InitDone,
        Event::Apply(grant) => Event::Apply(grant.into()),
        Event::Delete(grant) => Event::Delete(grant.into()),
    }
}

/// Generic ResourceController that encapsulates the complete lifecycle for a single resource type
///
/// Source-owned native stores publish complete LISTs. The first committed union
/// is replayed once; later commits and watch changes enqueue keys for one worker.
pub struct ResourceController<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
{
    kind: &'static str,
    client: Client,
    watcher_config: watcher::Config,

    // API creation based on scope
    api_scope: ApiScope,

    // Resource processor (holds cache, workqueue, handler)
    processor: Arc<ResourceProcessor<K>>,

    /// Namespace filter for MultipleNamespaces mode
    namespace_filter: Option<Vec<String>>,

    // Graceful shutdown signal
    shutdown_signal: Option<ShutdownSignal>,

    cancel: CancellationToken,
    tracker: TaskTracker,

    /// Leader handle for gating status writes (Phase 2 HA support)
    leader_handle: Option<LeaderHandle>,
    status_version: Option<&'static str>,
}

/// API scope for resource (namespaced or cluster-scoped)
#[derive(Clone)]
pub enum ApiScope {
    /// Namespaced resource with watch mode
    Namespaced(NamespaceWatchMode),
    /// Cluster-scoped resource
    ClusterScoped,
}

type NamespacedWatcher<K> = Pin<Box<dyn Stream<Item = watcher::Result<Event<K>>> + Send>>;

fn canonical_legacy_event<S, K>(event: Event<S>) -> Event<K>
where
    S: Into<K>,
{
    match event {
        Event::Init => Event::Init,
        Event::InitApply(resource) => Event::InitApply(resource.into()),
        Event::InitDone => Event::InitDone,
        Event::Apply(resource) => Event::Apply(resource.into()),
        Event::Delete(resource) => Event::Delete(resource.into()),
    }
}

struct ActiveController;
impl ActiveController {
    fn start() -> Self {
        controller_metrics().controller_started();
        Self
    }
}
impl Drop for ActiveController {
    fn drop(&mut self) {
        controller_metrics().controller_stopped();
    }
}

impl ApiScope {
    /// Get the namespace filter for MultipleNamespaces mode
    pub fn namespace_filter(&self) -> Option<Vec<String>> {
        match self {
            ApiScope::Namespaced(NamespaceWatchMode::MultipleNamespaces(ns)) => Some(ns.clone()),
            _ => None,
        }
    }
}

fn canonical_reference_grant_stream(
    stream: NamespacedWatcher<edgion_resources::ReferenceGrantBeta>,
) -> NamespacedWatcher<edgion_resources::ReferenceGrant> {
    Box::pin(stream.map(|event| event.map(canonical_reference_grant_event)))
}

impl ResourceController<edgion_resources::HTTPRoute> {
    pub async fn run_http_route_beta(self) -> Result<()> {
        use edgion_resources::HTTPRouteBeta;
        let client = self.client.clone();
        let config = self.watcher_config.clone();
        let stream = |api| -> NamespacedWatcher<edgion_resources::HTTPRoute> {
            Box::pin(
                watcher(api, config.clone())
                    .default_backoff()
                    .map(|event| event.map(canonical_http_route_event)),
            )
        };
        let sources = match &self.api_scope {
            ApiScope::Namespaced(NamespaceWatchMode::AllNamespaces) => {
                vec![(WatchScope::AllNamespaces, stream(Api::<HTTPRouteBeta>::all(client)))]
            }
            ApiScope::Namespaced(NamespaceWatchMode::SingleNamespace(ns)) => {
                vec![(
                    WatchScope::Namespace(ns.clone()),
                    stream(Api::<HTTPRouteBeta>::namespaced(client, ns)),
                )]
            }
            ApiScope::Namespaced(NamespaceWatchMode::MultipleNamespaces(namespaces)) => {
                let mut names = namespaces.clone();
                names.sort();
                names.dedup();
                names
                    .into_iter()
                    .map(|ns| {
                        let watched = stream(Api::<HTTPRouteBeta>::namespaced(client.clone(), &ns));
                        (WatchScope::Namespace(ns), watched)
                    })
                    .collect()
            }
            ApiScope::ClusterScoped => anyhow::bail!("Cannot run cluster-scoped HTTPRoute"),
        };
        self.run_sources(sources).await
    }
}

impl ResourceController<edgion_resources::Gateway> {
    pub async fn run_gateway_beta(self) -> Result<()> {
        use edgion_resources::GatewayBeta;
        let client = self.client.clone();
        let config = self.watcher_config.clone();
        let stream = |api| -> NamespacedWatcher<edgion_resources::Gateway> {
            Box::pin(
                watcher(api, config.clone())
                    .default_backoff()
                    .map(|event| event.map(canonical_gateway_event)),
            )
        };
        let sources = match &self.api_scope {
            ApiScope::Namespaced(NamespaceWatchMode::AllNamespaces) => {
                vec![(WatchScope::AllNamespaces, stream(Api::<GatewayBeta>::all(client)))]
            }
            ApiScope::Namespaced(NamespaceWatchMode::SingleNamespace(ns)) => {
                vec![(
                    WatchScope::Namespace(ns.clone()),
                    stream(Api::<GatewayBeta>::namespaced(client, ns)),
                )]
            }
            ApiScope::Namespaced(NamespaceWatchMode::MultipleNamespaces(namespaces)) => {
                let mut names = namespaces.clone();
                names.sort();
                names.dedup();
                names
                    .into_iter()
                    .map(|ns| {
                        let watched = stream(Api::<GatewayBeta>::namespaced(client.clone(), &ns));
                        (WatchScope::Namespace(ns), watched)
                    })
                    .collect()
            }
            ApiScope::ClusterScoped => anyhow::bail!("Cannot run cluster-scoped Gateway"),
        };
        self.run_sources(sources).await
    }
}

impl ResourceController<edgion_resources::GatewayClass> {
    pub async fn run_gateway_class_beta(self) -> Result<()> {
        let stream: NamespacedWatcher<edgion_resources::GatewayClass> = Box::pin(
            watcher(
                Api::<edgion_resources::GatewayClassBeta>::all(self.client.clone()),
                self.watcher_config.clone(),
            )
            .default_backoff()
            .map(|event| event.map(canonical_gateway_class_event)),
        );
        self.run_sources(vec![(WatchScope::Cluster, stream)]).await
    }
}

impl ResourceController<edgion_resources::ReferenceGrant> {
    /// Watch the selected v1beta1 transport and convert before the canonical raw store.
    pub async fn run_reference_grant_beta(self) -> Result<()> {
        use edgion_resources::ReferenceGrantBeta;
        let namespaces = match &self.api_scope {
            ApiScope::Namespaced(NamespaceWatchMode::AllNamespaces) => {
                let stream = canonical_reference_grant_stream(Box::pin(
                    watcher(
                        Api::<ReferenceGrantBeta>::all(self.client.clone()),
                        self.watcher_config.clone(),
                    )
                    .default_backoff(),
                ));
                return self.run_sources(vec![(WatchScope::AllNamespaces, stream)]).await;
            }
            ApiScope::Namespaced(NamespaceWatchMode::SingleNamespace(ns)) => vec![ns.clone()],
            ApiScope::Namespaced(NamespaceWatchMode::MultipleNamespaces(namespaces)) => {
                let mut names = namespaces.clone();
                names.sort();
                names.dedup();
                names
            }
            ApiScope::ClusterScoped => anyhow::bail!("Cannot run cluster-scoped ReferenceGrant"),
        };
        let sources = namespaces
            .into_iter()
            .map(|ns| {
                let stream = canonical_reference_grant_stream(Box::pin(
                    watcher(
                        Api::<ReferenceGrantBeta>::namespaced(self.client.clone(), &ns),
                        self.watcher_config.clone(),
                    )
                    .default_backoff(),
                ));
                (WatchScope::Namespace(ns), stream)
            })
            .collect();
        self.run_sources(sources).await
    }
}

impl<K> ResourceController<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
{
    /// Create a new ResourceController
    pub fn new(
        kind: &'static str,
        client: Client,
        processor: Arc<ResourceProcessor<K>>,
        api_scope: ApiScope,
        watcher_config: watcher::Config,
    ) -> Self {
        let namespace_filter = api_scope.namespace_filter();

        // Set namespace filter on processor if applicable
        if let Some(ref filter) = namespace_filter {
            processor.set_namespace_filter(filter.clone());
        }

        Self {
            kind,
            client,
            watcher_config,
            api_scope,
            processor,
            namespace_filter,
            shutdown_signal: None,
            cancel: CancellationToken::new(),
            tracker: TaskTracker::new(),
            leader_handle: None,
            status_version: None,
        }
    }

    /// Set shutdown signal
    pub fn with_shutdown(mut self, signal: ShutdownSignal) -> Self {
        self.shutdown_signal = Some(signal);
        self
    }

    pub(super) fn with_epoch(mut self, cancel: CancellationToken, tracker: TaskTracker) -> Self {
        self.cancel = cancel;
        self.tracker = tracker;
        self
    }

    pub fn with_status_version(mut self, version: &'static str) -> Self {
        self.status_version = Some(version);
        self
    }

    /// Set leader handle for gating status writes
    pub fn with_leader_handle(mut self, handle: LeaderHandle) -> Self {
        self.leader_handle = Some(handle);
        self
    }

    /// Construct fixed, nonoverlapping watch sources.
    pub async fn run_namespaced(self) -> Result<()>
    where
        K: Resource<Scope = kube::core::NamespaceResourceScope>,
    {
        let namespaces = match &self.api_scope {
            ApiScope::Namespaced(NamespaceWatchMode::AllNamespaces) => {
                let stream = self.watch(Api::all(self.client.clone()));
                return self.run_sources(vec![(WatchScope::AllNamespaces, stream)]).await;
            }
            ApiScope::Namespaced(NamespaceWatchMode::SingleNamespace(ns)) => vec![ns.clone()],
            ApiScope::Namespaced(NamespaceWatchMode::MultipleNamespaces(namespaces)) => {
                let mut names = namespaces.clone();
                names.sort();
                names.dedup();
                names
            }
            ApiScope::ClusterScoped => anyhow::bail!("Cannot run cluster-scoped resource as namespaced"),
        };
        let sources = namespaces
            .into_iter()
            .map(|ns| {
                let stream = self.watch(Api::namespaced(self.client.clone(), &ns));
                (WatchScope::Namespace(ns), stream)
            })
            .collect();
        self.run_sources(sources).await
    }

    /// Watch one selected legacy endpoint and convert events into the canonical queue.
    pub async fn run_legacy_namespaced<S>(self) -> Result<()>
    where
        K: Resource<Scope = kube::core::NamespaceResourceScope>,
        S: Resource<Scope = kube::core::NamespaceResourceScope, DynamicType = ()>
            + Clone
            + Debug
            + DeserializeOwned
            + Into<K>
            + Send
            + Sync
            + 'static,
    {
        let client = self.client.clone();
        let config = self.watcher_config.clone();
        let stream = |api| -> NamespacedWatcher<K> {
            Box::pin(
                watcher(api, config.clone())
                    .default_backoff()
                    .map(|event| event.map(canonical_legacy_event)),
            )
        };
        let sources = match &self.api_scope {
            ApiScope::Namespaced(NamespaceWatchMode::AllNamespaces) => {
                vec![(WatchScope::AllNamespaces, stream(Api::<S>::all(client)))]
            }
            ApiScope::Namespaced(NamespaceWatchMode::SingleNamespace(ns)) => {
                vec![(
                    WatchScope::Namespace(ns.clone()),
                    stream(Api::<S>::namespaced(client, ns)),
                )]
            }
            ApiScope::Namespaced(NamespaceWatchMode::MultipleNamespaces(namespaces)) => {
                let mut names = namespaces.clone();
                names.sort();
                names.dedup();
                names
                    .into_iter()
                    .map(|ns| {
                        let watched = stream(Api::<S>::namespaced(client.clone(), &ns));
                        (WatchScope::Namespace(ns), watched)
                    })
                    .collect()
            }
            ApiScope::ClusterScoped => anyhow::bail!("Cannot run a cluster-scoped legacy Route source"),
        };
        self.run_sources(sources).await
    }

    pub async fn run_cluster_scoped(self) -> Result<()>
    where
        K: Resource<Scope = kube::core::ClusterResourceScope>,
    {
        let stream = self.watch(Api::all(self.client.clone()));
        self.run_sources(vec![(WatchScope::Cluster, stream)]).await
    }

    fn watch(&self, api: Api<K>) -> NamespacedWatcher<K> {
        // Backoff belongs to each source; a failing source must not stall healthy ones.
        Box::pin(watcher(api, self.watcher_config.clone()).default_backoff())
    }

    async fn run_sources(self, inputs: Vec<(WatchScope, NamespacedWatcher<K>)>) -> Result<()> {
        anyhow::ensure!(!inputs.is_empty(), "No required watch sources");
        let mut sources = Vec::new();
        let mut streams = Vec::new();
        for (index, (scope, stream)) in inputs.into_iter().enumerate() {
            sources.push(SourceState::<K>::new(scope));
            // select_all silently removes EOF; retain an explicit terminal event.
            streams.push(Box::pin(
                stream
                    .map(move |event| (index, Some(event)))
                    .chain(futures::stream::once(async move { (index, None) })),
            ));
        }
        let raw: Arc<dyn RawStore<K>> = if sources.len() == 1 {
            Arc::new(KubeRawStore::new(sources[0].store.clone()))
        } else {
            let stores = sources
                .iter()
                .map(|source| match &source.scope {
                    WatchScope::Namespace(ns) => Ok((ns.clone(), source.store.clone())),
                    _ => Err(anyhow::anyhow!("Overlapping watch source scopes")),
                })
                .collect::<Result<_>>()?;
            Arc::new(KubeRawStore::namespaced(stores))
        };
        self.processor.set_raw_store(raw.clone());
        crate::observe::status_metrics::register_status_renderer::<K>(self.kind, raw.clone());
        crate::observe::unknown_fields_registry::register_unknown_fields_source::<K>(self.kind, raw.clone());
        let mut stream = futures::stream::select_all(streams);
        let worker_cancel = self.cancel.child_token();
        let mut worker: Option<AbortOnDropHandle<()>> = None;
        let mut shutdown = self.shutdown_signal.clone();
        let _active = ActiveController::start();
        let processing = async {
            let mut timer = None;
            let mut running = false;
            loop {
                let next = tokio::select! {
                    event = stream.next() => event,
                    result = async { worker.as_mut().unwrap().await }, if worker.is_some() => {
                        anyhow::bail!("Required resource worker stopped: {:?}", result);
                    }
                };
                let Some((index, Some(event))) = next else {
                    anyhow::bail!("Required watch source ended");
                };
                let event = match event {
                    Ok(event) => event,
                    Err(error) => {
                        tracing::warn!(component = "resource_controller", kind = self.kind, source = index,
                            error = %error, "Watch source error; retaining committed snapshot");
                        continue;
                    }
                };
                if matches!(&event, Event::Init) && timer.is_none() && !running {
                    timer = Some(InitSyncTimer::start(self.kind));
                }
                let tracks_ownership = matches!(self.kind, "GatewayClass" | "Gateway" | "HTTPRoute" | "GRPCRoute");
                // WATCH echoes read one key; only a complete LIST compares the source snapshot.
                let changed_key = match &event {
                    Event::Apply(obj) | Event::Delete(obj) => Some(make_resource_key(obj)),
                    _ => None,
                };
                let list_commit = matches!(&event, Event::InitDone);
                let compare_ownership = tracks_ownership && (list_commit || changed_key.is_some());
                let signatures = || {
                    let objects = match changed_key.as_deref() {
                        Some(key) => raw.get(key).into_iter().collect(),
                        None => raw.snapshot(),
                    };
                    objects
                        .into_iter()
                        .filter_map(|obj| {
                            let value = serde_json::to_value(obj.as_ref()).ok()?;
                            Some((
                                make_resource_key(obj.as_ref()),
                                ownership_dependency_signature(self.kind, &value)?,
                            ))
                        })
                        .collect::<BTreeMap<_, _>>()
                };
                let before = compare_ownership.then(&signatures);
                let was_committed = sources[index].ever_committed;
                let keys = sources[index].apply(event)?;
                let first_commit = !was_committed && sources[index].ever_committed;
                if compare_ownership && (first_commit || before.as_ref() != Some(&signatures())) {
                    self.requeue_ownership_dependents().await;
                }
                if running {
                    for key in keys {
                        // The outer cancellation abandons the epoch, including a partial delivery.
                        tokio::select! {
                            _ = self.processor.on_event_key(key) => {},
                            result = async { worker.as_mut().unwrap().await } => {
                                anyhow::bail!("Required resource worker stopped: {:?}", result);
                            }
                        }
                    }
                } else if sources.iter().all(|source| source.ever_committed) {
                    // No source is polled during replay: raw self/cross-key lookups see the complete union.
                    let snapshot = raw.snapshot();
                    self.processor.on_init();
                    let mut count = 0;
                    for object in snapshot {
                        if run_init_apply_once(
                            (*object).clone(),
                            &self.processor,
                            &self.client,
                            &self.api_scope,
                            &self.leader_handle,
                            self.status_version,
                            &self.namespace_filter,
                            self.kind,
                            "init",
                        )
                        .await
                        {
                            count += 1;
                        }
                    }
                    if let Some(timer) = timer.take() {
                        timer.complete(count);
                    }
                    self.processor.on_init_done();
                    worker = Some(AbortOnDropHandle::new(spawn_worker(
                        self.processor.clone(),
                        raw.clone(),
                        self.kind,
                        self.shutdown_signal.clone(),
                        self.client.clone(),
                        self.api_scope.clone(),
                        self.leader_handle.clone(),
                        self.status_version,
                        worker_cancel.clone(),
                        &self.tracker,
                    )));
                    running = true;
                }
            }
            #[allow(unreachable_code)]
            Ok(())
        };
        let result = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => Ok(()),
            _ = async {
                if let Some(signal) = &mut shutdown { signal.wait().await; }
                else { futures::future::pending::<()>().await; }
            } => Ok(()),
            result = processing => result,
        };
        worker_cancel.cancel();
        if let Some(mut handle) = worker {
            if !handle.is_finished() && tokio::time::timeout(Duration::from_secs(5), &mut handle).await.is_err() {
                handle.abort();
                let _ = handle.await;
            }
        }
        drop(sources);
        result
    }

    /// Source identity events bypass publication filters but still use the sole workqueue.
    async fn requeue_ownership_dependents(&self) {
        let kinds = ownership_dependency_kinds(self.kind);
        for kind in kinds {
            if let Some(processor) = self.processor.runtime().processor_registry.get(kind) {
                for key in processor.raw_keys() {
                    processor.requeue(key).await;
                }
            }
        }
    }
}

// ============================================================================
// Worker functions
// ============================================================================

/// Spawn worker task for processing workqueue items
///
/// Worker implements Go operator-style reconciliation:
/// - Dequeue key from workqueue
/// - Check store (K8s state) and call processor.process_work_item()
/// - Persist status to K8s API when status changes
#[allow(clippy::too_many_arguments)]
fn spawn_worker<K>(
    processor: Arc<ResourceProcessor<K>>,
    store: Arc<dyn RawStore<K>>,
    kind: &'static str,
    shutdown_signal: Option<ShutdownSignal>,
    client: Client,
    api_scope: ApiScope,
    leader_handle: Option<LeaderHandle>,
    status_version: Option<&'static str>,
    cancel: CancellationToken,
    tracker: &TaskTracker,
) -> JoinHandle<()>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
{
    let workqueue = processor.workqueue();

    tracker.spawn(async move {
        let processing = async {
            // Move shutdown_signal once outside the loop
            let mut shutdown = shutdown_signal;

            loop {
                let item = match &mut shutdown {
                    Some(signal) => {
                        tokio::select! {
                            item = workqueue.dequeue() => item,
                            _ = signal.wait() => {
                                tracing::info!(
                                    component = "resource_controller",
                                    kind = kind,
                                    "Worker received shutdown signal"
                                );
                                break;
                            }
                        }
                    }
                    None => workqueue.dequeue().await,
                };

                match item {
                    Some(work_item) => {
                        // Parse key to ObjectRef and get from store
                        let store_obj = store.get(&work_item.key).map(|arc| (*arc).clone());
                        // Fence the write against the input snapshot, not handler-mutated metadata.
                        let resource_version = store_obj.as_ref().and_then(|obj| obj.meta().resource_version.clone());

                        {
                            // K8s mode: pass None for existing_status_json (status is already in store_obj from K8s API)
                            let result = processor
                                .process_work_item(&work_item.key, store_obj, None, work_item.trigger_chain.clone())
                                .await;

                            if let WorkItemResult::Processed {
                                obj,
                                status_changed,
                                persisted_status,
                            } = result
                            {
                                let can_write_status = leader_handle.as_ref().is_none_or(|h| h.is_leader());
                                if status_changed && can_write_status {
                                    let name = obj.meta().name.as_deref().unwrap_or("");
                                    let namespace = obj.meta().namespace.as_deref();

                                    match persisted_status {
                                        Some(status_value) => {
                                            if let Err(e) = persist_k8s_status_version::<K>(
                                                &client,
                                                &api_scope,
                                                status_version,
                                                namespace,
                                                name,
                                                resource_version.as_deref(),
                                                &status_value,
                                            )
                                            .await
                                            {
                                                if e.is_not_found() {
                                                    tracing::info!(
                                                        component = "resource_controller",
                                                        kind = kind,
                                                        key = %work_item.key,
                                                        "Resource already deleted, skipping status persist"
                                                    );
                                                } else {
                                                    tracing::warn!(
                                                        component = "resource_controller",
                                                        kind = kind,
                                                        key = %work_item.key,
                                                        error = %e,
                                                        "Failed to persist status to K8s API"
                                                    );
                                                }
                                            }
                                        }
                                        None => {
                                            tracing::warn!(
                                                component = "resource_controller",
                                                kind = kind,
                                                key = %work_item.key,
                                                "Status changed but failed to extract status value for persistence"
                                            );
                                        }
                                    }
                                }
                            }
                            // Note: For WorkItemResult::Deleted, K8s handles status cleanup
                            // automatically when the resource is deleted
                        }

                        workqueue.done(&work_item.key);
                    }
                    None => {
                        tracing::warn!(
                            component = "resource_controller",
                            kind = kind,
                            "Workqueue closed, stopping worker"
                        );
                        break;
                    }
                }
            }
        };
        tokio::select! { _ = processing => {}, _ = cancel.cancelled() => {} }
        tracing::info!(component = "resource_controller", kind = kind, "Worker task ended");
    })
}

#[derive(Debug, thiserror::Error)]
enum StatusWriteError {
    #[error("Cannot persist Kubernetes status without the input resourceVersion; wait for a valid source snapshot")]
    MissingResourceVersion,
    #[error(transparent)]
    Kubernetes(#[from] kube::Error),
}

impl StatusWriteError {
    fn is_not_found(&self) -> bool {
        matches!(self, Self::Kubernetes(kube::Error::Api(status)) if status.code == 404)
    }
}

/// Persist status with an atomic precondition on the raw input resourceVersion.
///
/// Uses DynamicObject to avoid the Scope type constraint issue.
/// The `ApiResource` uses the selected GatewayClass transport when supplied;
/// other kinds use K's `#[kube]`-derived GVK and plural metadata. All resource
/// kinds inherit the guard. A conflict ends this attempt; recovery uses
/// subsequent source events/requeues, never a fresh version attached to stale status.
#[cfg(test)]
async fn persist_k8s_status<K>(
    client: &Client,
    api_scope: &ApiScope,
    namespace: Option<&str>,
    name: &str,
    resource_version: Option<&str>,
    status_value: &serde_json::Value,
) -> Result<(), StatusWriteError>
where
    K: Resource + Clone + Debug + DeserializeOwned + Serialize,
    K::DynamicType: Default,
{
    persist_k8s_status_version::<K>(client, api_scope, None, namespace, name, resource_version, status_value).await
}

async fn persist_k8s_status_version<K>(
    client: &Client,
    api_scope: &ApiScope,
    status_version: Option<&str>,
    namespace: Option<&str>,
    name: &str,
    resource_version: Option<&str>,
    status_value: &serde_json::Value,
) -> Result<(), StatusWriteError>
where
    K: Resource + Clone + Debug + DeserializeOwned + Serialize,
    K::DynamicType: Default,
{
    use kube::core::DynamicObject;
    use kube::discovery::ApiResource;

    edgion_resources::gateway_api_support::assert_canonical_type_identity::<K>(&K::kind(&K::DynamicType::default()));
    let dt = K::DynamicType::default();
    let api_resource = ApiResource::from_gvk_with_plural(
        &kube::core::GroupVersionKind {
            group: K::group(&dt).to_string(),
            version: status_version.unwrap_or(&K::version(&dt)).to_string(),
            kind: K::kind(&dt).to_string(),
        },
        &K::plural(&dt),
    );

    let kind_str = K::kind(&dt).to_string();

    let result = async {
        let resource_version = resource_version
            .filter(|version| !version.is_empty())
            .ok_or(StatusWriteError::MissingResourceVersion)?;
        let patch = serde_json::json!({
            "metadata": { "resourceVersion": resource_version },
            "status": status_value,
        });
        let params = PatchParams::default();

        match api_scope {
            ApiScope::Namespaced(_) => {
                let ns = namespace.unwrap_or("default");
                let api: Api<DynamicObject> = Api::namespaced_with(client.clone(), ns, &api_resource);
                api.patch_status(name, &params, &Patch::Merge(&patch)).await?;
            }
            ApiScope::ClusterScoped => {
                let api: Api<DynamicObject> = Api::all_with(client.clone(), &api_resource);
                api.patch_status(name, &params, &Patch::Merge(&patch)).await?;
            }
        }
        Ok::<(), StatusWriteError>(())
    }
    .await;

    match &result {
        Ok(_) => {
            crate::conf_mgr::sync_runtime::record_status_write(&kind_str, "kubernetes");
            tracing::info!(
                component = "resource_controller",
                kind = %kind_str,
                name = name,
                namespace = namespace,
                "Persisted status to K8s API"
            );
        }
        Err(_) => {
            crate::conf_mgr::sync_runtime::record_status_write_error(&kind_str, "kubernetes");
        }
    }

    result
}

/// Check if resource passes namespace filter
fn passes_namespace_filter<K>(obj: &K, namespace_filter: &Option<Vec<String>>) -> bool
where
    K: Resource + Clone,
{
    match namespace_filter {
        Some(allowed_ns) => match obj.namespace() {
            Some(resource_ns) => allowed_ns.iter().any(|ns| ns == &resource_ns),
            None => {
                tracing::warn!(
                    name = %obj.name_any(),
                    "Namespaced resource missing namespace, skipping"
                );
                false
            }
        },
        None => true,
    }
}

/// Process one object through the init-apply pipeline.
///
/// Applies namespace filter, calls `processor.on_init_apply`, and persists status to K8s
/// if it changed and this node is the leader.
///
/// Returns `true` when the object was fully processed (caller should increment `init_count`).
#[allow(clippy::too_many_arguments)]
async fn run_init_apply_once<K>(
    obj: K,
    processor: &Arc<ResourceProcessor<K>>,
    client: &Client,
    api_scope: &ApiScope,
    leader_handle: &Option<LeaderHandle>,
    status_version: Option<&'static str>,
    namespace_filter: &Option<Vec<String>>,
    kind: &'static str,
    phase: &'static str,
) -> bool
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
{
    if !passes_namespace_filter(&obj, namespace_filter) {
        return false;
    }

    let resource_version = obj.meta().resource_version.clone();
    let WorkItemResult::Processed {
        obj,
        status_changed,
        persisted_status,
    } = processor.on_init_apply(obj, None).await
    else {
        return false;
    };

    let can_write_status = leader_handle.as_ref().is_none_or(|h| h.is_leader());
    if status_changed && can_write_status {
        let name = obj.meta().name.as_deref().unwrap_or("");
        let namespace = obj.meta().namespace.as_deref();
        if let Some(status_value) = persisted_status {
            if let Err(e) = persist_k8s_status_version::<K>(
                client,
                api_scope,
                status_version,
                namespace,
                name,
                resource_version.as_deref(),
                &status_value,
            )
            .await
            {
                if e.is_not_found() {
                    tracing::info!(
                        component = "resource_controller",
                        kind = kind,
                        name = %name,
                        phase = phase,
                        "Resource already deleted, skipping status persist during init phase"
                    );
                } else {
                    tracing::warn!(
                        component = "resource_controller",
                        kind = kind,
                        name = %name,
                        phase = phase,
                        error = %e,
                        "Failed to persist status during init phase"
                    );
                }
            }
        }
    }

    true
}

// Status persistence regression tests live in `tests/status_persistence_test.rs`.
#[cfg(test)]
#[path = "tests/status_persistence_test.rs"]
mod status_persistence_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_watch_events_enter_the_canonical_lifecycle() {
        use edgion_resources::resources::TLSRouteAlpha3;
        let source: TLSRouteAlpha3 = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1alpha3",
            "kind": "TLSRoute",
            "metadata": {"name": "route", "namespace": "default", "resourceVersion": "17"},
            "spec": {"hostnames": ["example.com"], "rules": [{"backendRefs": [{"name": "backend", "port": 443}]}]},
            "status": {"parents": []}
        }))
        .unwrap();
        assert!(matches!(
            canonical_legacy_event::<TLSRouteAlpha3, edgion_resources::TLSRoute>(Event::Init),
            Event::Init
        ));
        assert!(matches!(
            canonical_legacy_event::<TLSRouteAlpha3, edgion_resources::TLSRoute>(Event::InitDone),
            Event::InitDone
        ));
        for event in [
            Event::InitApply(source.clone()),
            Event::Apply(source.clone()),
            Event::Delete(source.clone()),
        ] {
            let canonical: Event<edgion_resources::TLSRoute> = canonical_legacy_event(event);
            let (Event::InitApply(route) | Event::Apply(route) | Event::Delete(route)) = canonical else {
                unreachable!();
            };
            assert_eq!(route.metadata.resource_version.as_deref(), Some("17"));
            assert_eq!(route.spec.hostnames.as_ref().unwrap(), &vec!["example.com".to_string()]);
            assert!(route.status.is_some());
        }
    }

    #[test]
    fn gateway_class_beta_events_keep_one_canonical_lifecycle() {
        use edgion_resources::{GatewayClassBeta, GatewayClassBetaSpec};
        let class = GatewayClassBeta {
            metadata: kube::api::ObjectMeta {
                name: Some("edge".into()),
                resource_version: Some("17".into()),
                ..Default::default()
            },
            spec: GatewayClassBetaSpec {
                controller_name: "example.com/edge".into(),
                description: None,
                parameters_ref: None,
            },
            status: None,
        };
        assert!(matches!(canonical_gateway_class_event(Event::Init), Event::Init));
        assert!(matches!(
            canonical_gateway_class_event(Event::InitDone),
            Event::InitDone
        ));
        for event in [
            Event::InitApply(class.clone()),
            Event::Apply(class.clone()),
            Event::Delete(class),
        ] {
            let canonical = canonical_gateway_class_event(event);
            let (Event::InitApply(obj) | Event::Apply(obj) | Event::Delete(obj)) = canonical else {
                unreachable!();
            };
            assert_eq!(obj.metadata.resource_version.as_deref(), Some("17"));
            assert_eq!(obj.spec.controller_name, "example.com/edge");
        }
    }
    use crate::conf_mgr::sync_runtime::resource_processor::{
        HandlerContext, ProcessResult, ProcessorHandler, ProcessorObj,
    };
    use crate::conf_mgr::ControllerRuntime;
    use k8s_openapi::api::core::v1::ConfigMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::mpsc;

    #[test]
    fn beta_watch_events_enter_canonical_store_as_v1() {
        use edgion_resources::resources::{ReferenceGrantBetaSpec, ReferenceGrantFrom, ReferenceGrantTo};
        let beta = edgion_resources::ReferenceGrantBeta {
            metadata: kube::api::ObjectMeta {
                namespace: Some("backend".into()),
                name: Some("allow".into()),
                resource_version: Some("17".into()),
                ..Default::default()
            },
            spec: ReferenceGrantBetaSpec {
                from: vec![ReferenceGrantFrom {
                    group: "gateway.networking.k8s.io".into(),
                    kind: "HTTPRoute".into(),
                    namespace: "app".into(),
                }],
                to: vec![ReferenceGrantTo {
                    group: "".into(),
                    kind: "Service".into(),
                    name: Some("api".into()),
                }],
            },
        };
        for event in [
            Event::InitApply(beta.clone()),
            Event::Apply(beta.clone()),
            Event::Delete(beta),
        ] {
            let canonical = canonical_reference_grant_event(event);
            let (Event::InitApply(grant) | Event::Apply(grant) | Event::Delete(grant)) = canonical else {
                panic!("object event must remain an object event");
            };
            assert_eq!(grant.metadata.resource_version.as_deref(), Some("17"));
            assert_eq!(grant.spec.from[0].namespace, "app");
            assert_eq!(grant.spec.to[0].name.as_deref(), Some("api"));
            assert_eq!(<edgion_resources::ReferenceGrant as Resource>::version(&()), "v1");
        }
    }

    #[derive(Default)]
    struct GrantEvents(parking_lot::Mutex<Vec<std::collections::HashSet<String>>>);

    #[async_trait::async_trait]
    impl crate::conf_mgr::sync_runtime::resource_processor::ref_grant::events::RevalidationListener for GrantEvents {
        async fn on_reference_grant_changed(
            &self,
            event: &crate::conf_mgr::sync_runtime::resource_processor::ref_grant::events::ReferenceGrantChangedEvent,
        ) {
            self.0.lock().push(event.affected_namespaces.clone());
        }
    }

    #[derive(Default)]
    struct CountingGrantHandler {
        inner: crate::conf_mgr::sync_runtime::resource_processor::ReferenceGrantHandler,
        init_done: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ProcessorHandler<edgion_resources::ReferenceGrant> for CountingGrantHandler {
        async fn parse(
            &self,
            obj: edgion_resources::ReferenceGrant,
            ctx: &HandlerContext,
        ) -> ProcessResult<edgion_resources::ReferenceGrant> {
            self.inner.parse(obj, ctx).await
        }
        async fn on_change(&self, obj: &edgion_resources::ReferenceGrant, ctx: &HandlerContext) {
            self.inner.on_change(obj, ctx).await;
        }
        async fn on_delete(&self, obj: &edgion_resources::ReferenceGrant, ctx: &HandlerContext) {
            self.inner.on_delete(obj, ctx).await;
        }
        fn on_init_done(&self, _ctx: &HandlerContext) {
            self.init_done.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn beta_grant(from_namespace: &str) -> edgion_resources::ReferenceGrantBeta {
        use edgion_resources::resources::{ReferenceGrantBetaSpec, ReferenceGrantFrom, ReferenceGrantTo};
        edgion_resources::ReferenceGrantBeta {
            metadata: kube::api::ObjectMeta {
                namespace: Some("backend".into()),
                name: Some("allow".into()),
                resource_version: Some("17".into()),
                ..Default::default()
            },
            spec: ReferenceGrantBetaSpec {
                from: vec![ReferenceGrantFrom {
                    group: "gateway.networking.k8s.io".into(),
                    kind: "HTTPRoute".into(),
                    namespace: from_namespace.into(),
                }],
                to: vec![ReferenceGrantTo {
                    group: "".into(),
                    kind: "Service".into(),
                    name: Some("api".into()),
                }],
            },
        }
    }

    #[tokio::test]
    async fn beta_watch_stream_reconciles_canonical_raw_store_and_relist_without_status() {
        let runtime = Arc::new(ControllerRuntime::new());
        let events = Arc::new(GrantEvents::default());
        runtime.ref_grant_dispatcher.register_listener(events.clone());
        let handler = Arc::new(CountingGrantHandler::default());
        let processor = Arc::new(ResourceProcessor::new(
            "ReferenceGrant",
            32,
            handler.clone(),
            runtime.clone(),
        ));
        let requests = Arc::new(AtomicUsize::new(0));
        let captured = requests.clone();
        let client = Client::new(
            tower::service_fn(move |_request: http::Request<kube::client::Body>| {
                let captured = captured.clone();
                async move {
                    captured.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, std::convert::Infallible>(http::Response::new(kube::client::Body::empty()))
                }
            }),
            "default",
        );
        let cancel = CancellationToken::new();
        let tracker = TaskTracker::new();
        let controller = ResourceController::new(
            "ReferenceGrant",
            client,
            processor.clone(),
            ApiScope::Namespaced(NamespaceWatchMode::AllNamespaces),
            watcher::Config::default(),
        )
        .with_epoch(cancel.clone(), tracker.clone());
        let (tx, beta_stream) = source_channel::<edgion_resources::ReferenceGrantBeta>();
        let task = tracker.spawn(controller.run_sources(vec![(
            WatchScope::AllNamespaces,
            canonical_reference_grant_stream(beta_stream),
        )]));
        for event in [Event::Init, Event::InitApply(beta_grant("app-a")), Event::InitDone] {
            tx.send(Ok(event)).await.unwrap();
        }
        eventually(|| processor.get("backend/allow").is_some() && !events.0.lock().is_empty()).await;
        let raw = processor.get_raw_any("backend/allow").unwrap();
        let raw = raw.downcast::<edgion_resources::ReferenceGrant>().unwrap();
        assert_eq!(raw.spec.from[0].namespace, "app-a");
        assert_eq!(handler.init_done.load(Ordering::SeqCst), 1);
        assert_eq!(tracker.len(), 2, "one source driver and one resource worker");

        tx.send(Ok(Event::Apply(beta_grant("app-b")))).await.unwrap();
        eventually(|| {
            processor
                .get("backend/allow")
                .is_some_and(|grant| grant.spec.from[0].namespace == "app-b")
                && events.0.lock().len() >= 3
        })
        .await;
        let updates = events.0.lock().clone();
        assert!(updates[1..].iter().any(|event| event.contains("app-a")));
        assert!(updates[1..].iter().any(|event| event.contains("app-b")));

        tx.send(Ok(Event::Delete(beta_grant("app-b")))).await.unwrap();
        eventually(|| processor.get("backend/allow").is_none() && events.0.lock().len() >= 4).await;
        tx.send(Ok(Event::Apply(beta_grant("app-c")))).await.unwrap();
        eventually(|| {
            processor
                .get("backend/allow")
                .is_some_and(|grant| grant.spec.from[0].namespace == "app-c")
        })
        .await;
        tx.send(Err(watcher::Error::NoResourceVersion)).await.unwrap();
        for event in [
            Event::Init,
            Event::InitApply(beta_grant("app-d")),
            Event::InitDone,
            Event::InitDone,
        ] {
            tx.send(Ok(event)).await.unwrap();
        }
        eventually(|| {
            processor
                .get("backend/allow")
                .is_some_and(|grant| grant.spec.from[0].namespace == "app-d")
        })
        .await;
        assert!(events.0.lock().iter().any(|event| event.contains("app-c")));
        assert!(events.0.lock().iter().any(|event| event.contains("app-d")));
        assert_eq!(handler.init_done.load(Ordering::SeqCst), 1);
        assert_eq!(tracker.len(), 2, "relist must not spawn another worker");
        assert_eq!(
            requests.load(Ordering::SeqCst),
            0,
            "ReferenceGrant has no status endpoint"
        );
        cancel.cancel();
        task.await.unwrap().unwrap();
        tracker.close();
        tracker.wait().await;
        assert!(processor.raw_keys().is_empty());
    }

    struct RecordingHandler {
        completed: AtomicUsize,
        applied: AtomicUsize,
        deleted: AtomicUsize,
        processor: parking_lot::Mutex<Option<std::sync::Weak<ResourceProcessor<ConfigMap>>>>,
        bootstrap_keys: parking_lot::Mutex<Vec<String>>,
        block: tokio::sync::Semaphore,
        entered: AtomicUsize,
    }

    impl Default for RecordingHandler {
        fn default() -> Self {
            Self {
                completed: AtomicUsize::new(0),
                applied: AtomicUsize::new(0),
                deleted: AtomicUsize::new(0),
                processor: parking_lot::Mutex::new(None),
                bootstrap_keys: parking_lot::Mutex::new(Vec::new()),
                block: tokio::sync::Semaphore::new(0),
                entered: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProcessorHandler<ConfigMap> for RecordingHandler {
        fn filter(&self, obj: &ConfigMap, _ctx: &HandlerContext) -> bool {
            obj.metadata
                .labels
                .as_ref()
                .is_none_or(|labels| !labels.contains_key("excluded"))
        }
        async fn parse(&self, obj: ConfigMap, _ctx: &HandlerContext) -> ProcessResult<ConfigMap> {
            if obj
                .metadata
                .labels
                .as_ref()
                .is_some_and(|labels| labels.contains_key("panic"))
            {
                panic!("Injected worker failure");
            }
            if obj
                .metadata
                .labels
                .as_ref()
                .is_some_and(|labels| labels.contains_key("block"))
            {
                self.entered.fetch_add(1, Ordering::SeqCst);
                self.block.acquire().await.unwrap().forget();
            }
            ProcessResult::Continue(obj)
        }
        async fn on_change(&self, obj: &ConfigMap, _ctx: &HandlerContext) {
            if obj
                .metadata
                .labels
                .as_ref()
                .is_some_and(|labels| labels.contains_key("saturate"))
            {
                let processor = self.processor.lock().as_ref().unwrap().upgrade().unwrap();
                // Synthetic pressure probe of the existing delayed queue, not a production hook.
                for index in 0..12000 {
                    processor
                        .workqueue()
                        .enqueue_after(
                            format!("a/dependency-{index}"),
                            Duration::ZERO,
                            crate::conf_mgr::sync_runtime::workqueue::TriggerChain::default(),
                        )
                        .await;
                }
            }
            if self.completed.load(Ordering::SeqCst) == 0 {
                let processor = self.processor.lock().as_ref().unwrap().upgrade().unwrap();
                let expected = self.bootstrap_keys.lock();
                for key in expected.iter() {
                    assert!(
                        processor.get_raw_any(key).is_some(),
                        "missing complete bootstrap key {key}"
                    );
                }
            }
            self.applied.fetch_add(1, Ordering::SeqCst);
        }
        async fn on_delete(&self, _obj: &ConfigMap, _ctx: &HandlerContext) {
            self.deleted.fetch_add(1, Ordering::SeqCst);
        }
        fn on_init_done(&self, _ctx: &HandlerContext) {
            self.completed.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn object(ns: &str, name: &str) -> ConfigMap {
        let mut obj = ConfigMap::default();
        obj.metadata.name = Some(name.into());
        obj.metadata.namespace = Some(ns.into());
        obj
    }

    fn client() -> Client {
        Client::new(
            tower::service_fn(|_: http::Request<kube::client::Body>| async {
                Ok::<_, std::convert::Infallible>(http::Response::new(kube::client::Body::from(b"{}".to_vec())))
            }),
            "default",
        )
    }

    fn channel() -> (
        mpsc::Sender<watcher::Result<Event<ConfigMap>>>,
        NamespacedWatcher<ConfigMap>,
    ) {
        source_channel()
    }

    fn source_channel<K: Send + 'static>() -> (mpsc::Sender<watcher::Result<Event<K>>>, NamespacedWatcher<K>) {
        let (tx, rx) = mpsc::channel(32);
        (
            tx,
            Box::pin(futures::stream::unfold(rx, |mut rx| async {
                rx.recv().await.map(|event| (event, rx))
            })),
        )
    }

    async fn send(tx: &mpsc::Sender<watcher::Result<Event<ConfigMap>>>, events: Vec<Event<ConfigMap>>) {
        for event in events {
            tx.send(Ok(event)).await.unwrap();
        }
    }

    async fn eventually(mut predicate: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(30), async {
            while !predicate() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    fn setup() -> (
        ResourceController<ConfigMap>,
        Arc<ResourceProcessor<ConfigMap>>,
        Arc<RecordingHandler>,
        CancellationToken,
        TaskTracker,
    ) {
        let runtime = Arc::new(ControllerRuntime::new());
        let handler = Arc::new(RecordingHandler::default());
        let processor = Arc::new(ResourceProcessor::new("ConfigMap", 10000, handler.clone(), runtime));
        *handler.processor.lock() = Some(Arc::downgrade(&processor));
        let cancel = CancellationToken::new();
        let tracker = TaskTracker::new();
        let controller = ResourceController::new(
            "ConfigMap",
            client(),
            processor.clone(),
            ApiScope::Namespaced(NamespaceWatchMode::AllNamespaces),
            watcher::Config::default(),
        )
        .with_epoch(cancel.clone(), tracker.clone());
        (controller, processor, handler, cancel, tracker)
    }

    #[tokio::test]
    async fn namespace_watchers_expose_one_union_initialization_sequence() {
        let (controller, processor, handler, cancel, tracker) = setup();
        *handler.bootstrap_keys.lock() = vec!["a/old".into(), "a/added".into(), "b/kept".into(), "a/filtered".into()];
        let (a, stream_a) = channel();
        let (b, stream_b) = channel();
        let task = tracker.spawn(controller.run_sources(vec![
            (WatchScope::Namespace("a".into()), stream_a),
            (WatchScope::Namespace("b".into()), stream_b),
        ]));
        let mut filtered = object("a", "filtered");
        filtered.metadata.labels = Some(std::collections::BTreeMap::from([("excluded".into(), "true".into())]));
        send(
            &a,
            vec![
                Event::Init,
                Event::InitApply(object("a", "abandoned")),
                Event::Init,
                Event::InitApply(object("a", "old")),
                Event::InitApply(object("a", "removed")),
                Event::InitApply(filtered),
                Event::InitDone,
            ],
        )
        .await;
        eventually(|| processor.get_raw_any("a/old").is_some()).await;
        assert_eq!(handler.applied.load(Ordering::SeqCst), 0);
        assert!(processor.get_raw_any("a/abandoned").is_none());
        let mut updated = object("a", "old");
        updated.metadata.uid = Some("updated-before-bootstrap".into());
        send(
            &a,
            vec![
                Event::Apply(updated),
                Event::Apply(object("a", "added")),
                Event::Delete(object("a", "removed")),
                Event::Init,
                Event::InitApply(object("a", "new")),
            ],
        )
        .await;
        eventually(|| processor.get_raw_any("a/added").is_some() && processor.get_raw_any("a/removed").is_none()).await;
        assert_eq!(handler.applied.load(Ordering::SeqCst), 0);
        send(
            &b,
            vec![Event::Init, Event::InitApply(object("b", "kept")), Event::InitDone],
        )
        .await;
        eventually(|| handler.completed.load(Ordering::SeqCst) == 1).await;
        assert_eq!(
            processor.get("a/old").unwrap().metadata.uid.as_deref(),
            Some("updated-before-bootstrap")
        );
        assert!(processor.get("a/added").is_some());
        assert!(processor.get("a/removed").is_none());
        assert!(processor.get("a/new").is_none());
        assert!(processor.get("a/filtered").is_none());
        send(&a, vec![Event::InitDone, Event::InitDone]).await;
        eventually(|| processor.get("a/new").is_some() && processor.get("a/old").is_none()).await;
        assert!(processor.get("b/kept").is_some());
        send(&a, vec![Event::Init, Event::InitDone]).await;
        eventually(|| processor.get("a/new").is_none()).await;
        assert!(processor.get("b/kept").is_some());
        assert_eq!(handler.completed.load(Ordering::SeqCst), 1);
        cancel.cancel();
        task.await.unwrap().unwrap();
        tracker.close();
        tracker.wait().await;
        assert!(processor.raw_keys().is_empty());
    }

    #[tokio::test]
    async fn relist_larger_than_queue_capacity_converges_without_reinitialization() {
        let (controller, processor, handler, cancel, tracker) = setup();
        let (tx, stream) = channel();
        let task = tracker.spawn(controller.run_sources(vec![(WatchScope::AllNamespaces, stream)]));
        assert!(super::super::WorkqueueConfig::default().capacity < 5100);
        tx.send(Ok(Event::Init)).await.unwrap();
        // Bootstrap ingress must never fill the pre-worker queue.
        for index in 0..5100 {
            tx.send(Ok(Event::InitApply(object("a", &format!("item-{index}")))))
                .await
                .unwrap();
        }
        tx.send(Ok(Event::InitDone)).await.unwrap();
        eventually(|| handler.completed.load(Ordering::SeqCst) == 1).await;
        assert_eq!(processor.cache().list_keys().len(), 5100);
        send(&tx, vec![Event::Init, Event::InitDone]).await;
        eventually(|| processor.cache().list_keys().is_empty()).await;
        assert_eq!(handler.deleted.load(Ordering::SeqCst), 5100);
        assert_eq!(handler.completed.load(Ordering::SeqCst), 1);
        cancel.cancel();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn dirty_requeue_reads_latest_committed_object_and_eligibility_loss_deletes_cached_generation() {
        let (controller, processor, handler, cancel, tracker) = setup();
        let (tx, stream) = channel();
        let task = tracker.spawn(controller.run_sources(vec![(WatchScope::AllNamespaces, stream)]));
        send(
            &tx,
            vec![Event::Init, Event::InitApply(object("a", "x")), Event::InitDone],
        )
        .await;
        eventually(|| handler.completed.load(Ordering::SeqCst) == 1).await;
        let mut blocked = object("a", "x");
        blocked.metadata.labels = Some(std::collections::BTreeMap::from([("block".into(), "true".into())]));
        send(&tx, vec![Event::Apply(blocked)]).await;
        eventually(|| handler.entered.load(Ordering::SeqCst) == 1).await;
        let mut replacement = object("a", "x");
        replacement.metadata.uid = Some("replacement".into());
        replacement.metadata.labels = Some(std::collections::BTreeMap::from([("excluded".into(), "true".into())]));
        send(&tx, vec![Event::Init, Event::InitApply(replacement), Event::InitDone]).await;
        eventually(|| {
            processor
                .get_raw_any("a/x")
                .unwrap()
                .downcast::<ConfigMap>()
                .unwrap()
                .metadata
                .uid
                .as_deref()
                == Some("replacement")
        })
        .await;
        handler.block.add_permits(1);
        eventually(|| processor.get("a/x").is_none()).await;
        assert!(processor.get_raw_any("a/x").is_some());
        assert_eq!(handler.deleted.load(Ordering::SeqCst), 1);
        cancel.cancel();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn required_source_eof_and_worker_panic_are_terminal_and_drop_clears_raw_state() {
        for panic_worker in [false, true] {
            let (controller, processor, handler, _cancel, tracker) = setup();
            let (tx, stream) = channel();
            let task = tracker.spawn(controller.run_sources(vec![(WatchScope::AllNamespaces, stream)]));
            send(
                &tx,
                vec![Event::Init, Event::InitApply(object("a", "x")), Event::InitDone],
            )
            .await;
            eventually(|| handler.completed.load(Ordering::SeqCst) == 1).await;
            if panic_worker {
                let mut obj = object("a", "x");
                obj.metadata.labels = Some(std::collections::BTreeMap::from([("panic".into(), "true".into())]));
                send(&tx, vec![Event::Apply(obj)]).await;
            } else {
                drop(tx);
            }
            assert!(tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap()
                .is_err());
            tracker.close();
            tracker.wait().await;
            assert!(processor.raw_keys().is_empty());
        }
    }

    #[tokio::test]
    async fn cancellation_during_replay_releases_store_and_does_not_start_worker() {
        let (controller, processor, handler, cancel, tracker) = setup();
        let (tx, stream) = channel();
        let task = tracker.spawn(controller.run_sources(vec![(WatchScope::AllNamespaces, stream)]));
        let mut obj = object("a", "x");
        obj.metadata.labels = Some(std::collections::BTreeMap::from([("block".into(), "true".into())]));
        send(&tx, vec![Event::Init, Event::InitApply(obj), Event::InitDone]).await;
        eventually(|| handler.entered.load(Ordering::SeqCst) == 1).await;
        cancel.cancel();
        task.await.unwrap().unwrap();
        tracker.close();
        tracker.wait().await;
        assert!(processor.raw_keys().is_empty());
        assert_eq!(handler.completed.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn secret_accumulator_commits_once_and_relist_removes_only_its_source() {
        use crate::conf_mgr::sync_runtime::resource_processor::SecretHandler;
        use k8s_openapi::api::core::v1::Secret;
        let runtime = Arc::new(ControllerRuntime::new());
        let processor = Arc::new(ResourceProcessor::new(
            "Secret",
            100,
            Arc::new(SecretHandler::new()),
            runtime.clone(),
        ));
        let cancel = CancellationToken::new();
        let tracker = TaskTracker::new();
        let controller = ResourceController::new(
            "Secret",
            client(),
            processor.clone(),
            ApiScope::Namespaced(NamespaceWatchMode::AllNamespaces),
            watcher::Config::default(),
        )
        .with_epoch(cancel.clone(), tracker.clone());
        let (a, sa) = source_channel();
        let (b, sb) = source_channel();
        let task = tracker.spawn(controller.run_sources(vec![
            (WatchScope::Namespace("a".into()), sa),
            (WatchScope::Namespace("b".into()), sb),
        ]));
        let secret = |ns: &str, name: &str| Secret {
            metadata: object(ns, name).metadata,
            ..Default::default()
        };
        for event in [
            Event::Init,
            Event::InitApply(secret("a", "abandoned")),
            Event::Init,
            Event::InitApply(secret("a", "old")),
            Event::InitDone,
        ] {
            a.send(Ok(event)).await.unwrap();
        }
        eventually(|| processor.get_raw_any("a/old").is_some()).await;
        assert_eq!(runtime.secret_store.count(), 0);
        for event in [Event::Init, Event::InitApply(secret("b", "kept")), Event::InitDone] {
            b.send(Ok(event)).await.unwrap();
        }
        eventually(|| processor.is_ready()).await;
        assert_eq!(runtime.secret_store.keys(), vec!["a/old", "b/kept"]);
        for event in [Event::Init, Event::InitApply(secret("a", "new")), Event::InitDone] {
            a.send(Ok(event)).await.unwrap();
        }
        eventually(|| {
            runtime.secret_store.get(Some("a"), "new").is_some() && runtime.secret_store.get(Some("a"), "old").is_none()
        })
        .await;
        assert!(runtime.secret_store.get(Some("b"), "kept").is_some());
        assert!(processor.is_ready());
        cancel.cancel();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn identical_rv_relist_retries_failed_status_through_the_existing_worker() {
        use k8s_openapi::api::core::v1::{LoadBalancerIngress, LoadBalancerStatus, Service, ServiceStatus};
        struct StatusHandler;
        #[async_trait::async_trait]
        impl ProcessorHandler<Service> for StatusHandler {
            fn update_status(&self, obj: &mut Service, _: &HandlerContext, _: &[String]) {
                obj.status = Some(ServiceStatus {
                    load_balancer: Some(LoadBalancerStatus {
                        ingress: Some(vec![LoadBalancerIngress {
                            ip: Some("192.0.2.1".into()),
                            ..Default::default()
                        }]),
                    }),
                    ..Default::default()
                });
            }
        }
        let attempts = Arc::new(AtomicUsize::new(0));
        let captured = attempts.clone();
        let transport = tower::service_fn(move |request: http::Request<kube::client::Body>| {
            let attempt = captured.fetch_add(1, Ordering::SeqCst);
            async move {
                assert_eq!(request.method(), http::Method::PATCH);
                assert_eq!(request.uri().path(), "/api/v1/namespaces/a/services/backend/status");
                let (status, value) = if attempt == 0 {
                    (
                        503,
                        serde_json::json!({"kind":"Status","apiVersion":"v1","status":"Failure","message":"Unavailable","reason":"ServiceUnavailable","code":503}),
                    )
                } else {
                    (
                        200,
                        serde_json::json!({"kind":"Service","apiVersion":"v1","metadata":{"name":"backend","namespace":"a"}}),
                    )
                };
                Ok::<_, std::convert::Infallible>(
                    http::Response::builder()
                        .status(status)
                        .body(kube::client::Body::from(serde_json::to_vec(&value).unwrap()))
                        .unwrap(),
                )
            }
        });
        let processor = Arc::new(ResourceProcessor::new(
            "Service",
            100,
            Arc::new(StatusHandler),
            Arc::new(ControllerRuntime::new()),
        ));
        let cancel = CancellationToken::new();
        let tracker = TaskTracker::new();
        let controller = ResourceController::new(
            "Service",
            Client::new(transport, "default"),
            processor.clone(),
            ApiScope::Namespaced(NamespaceWatchMode::AllNamespaces),
            watcher::Config::default(),
        )
        .with_epoch(cancel.clone(), tracker.clone());
        let (tx, stream) = source_channel();
        let task = tracker.spawn(controller.run_sources(vec![(WatchScope::AllNamespaces, stream)]));
        let mut service = Service {
            metadata: object("a", "backend").metadata,
            ..Default::default()
        };
        service.metadata.resource_version = Some("same-rv".into());
        for event in [Event::Init, Event::InitApply(service.clone()), Event::InitDone] {
            tx.send(Ok(event)).await.unwrap();
        }
        eventually(|| processor.is_ready()).await;
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        for event in [Event::Init, Event::InitApply(service), Event::InitDone] {
            tx.send(Ok(event)).await.unwrap();
        }
        eventually(|| attempts.load(Ordering::SeqCst) == 2).await;
        assert!(processor.is_ready());
        cancel.cancel();
        task.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn source_backoff_does_not_stall_other_sources_or_cancellation() {
        let (controller, processor, handler, cancel, tracker) = setup();
        let (a, sa) = channel();
        let (b, sb) = channel();
        let task = tracker.spawn(controller.run_sources(vec![
            (WatchScope::Namespace("a".into()), Box::pin(sa.default_backoff())),
            (WatchScope::Namespace("b".into()), sb),
        ]));
        send(
            &a,
            vec![Event::Init, Event::InitApply(object("a", "old")), Event::InitDone],
        )
        .await;
        eventually(|| processor.get_raw_any("a/old").is_some()).await;
        a.send(Err(watcher::Error::NoResourceVersion)).await.unwrap();
        send(&a, vec![Event::Init, Event::InitDone]).await;
        send(
            &b,
            vec![Event::Init, Event::InitApply(object("b", "kept")), Event::InitDone],
        )
        .await;
        eventually(|| handler.completed.load(Ordering::SeqCst) == 1).await;
        assert!(processor.get("a/old").is_some());
        assert!(processor.get("b/kept").is_some());
        cancel.cancel();
        task.await.unwrap().unwrap();
        assert!(processor.raw_keys().is_empty());
    }

    #[tokio::test]
    async fn cancellation_of_blocked_resync_and_forced_abort_drain_the_owned_worker() {
        for forced_abort in [false, true] {
            let (controller, processor, handler, cancel, tracker) = setup();
            let (tx, stream) = channel();
            let task = tracker.spawn(controller.run_sources(vec![(WatchScope::AllNamespaces, stream)]));
            send(&tx, vec![Event::Init, Event::InitDone]).await;
            eventually(|| handler.completed.load(Ordering::SeqCst) == 1).await;
            let mut blocked = object("a", "blocked");
            blocked.metadata.labels = Some(std::collections::BTreeMap::from([("block".into(), "true".into())]));
            send(&tx, vec![Event::Apply(blocked)]).await;
            eventually(|| handler.entered.load(Ordering::SeqCst) == 1).await;
            tx.send(Ok(Event::Init)).await.unwrap();
            for index in 0..5100 {
                tx.send(Ok(Event::InitApply(object("a", &format!("item-{index}")))))
                    .await
                    .unwrap();
            }
            tx.send(Ok(Event::InitDone)).await.unwrap();
            eventually(|| {
                processor.workqueue().metrics().get_depth() > super::super::WorkqueueConfig::default().capacity as u64
            })
            .await;
            assert_eq!(processor.raw_keys().len(), 5100);
            if forced_abort {
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
            } else {
                cancel.cancel();
                task.await.unwrap().unwrap();
            }
            tracker.close();
            tokio::time::timeout(Duration::from_secs(5), tracker.wait())
                .await
                .unwrap();
            assert!(processor.raw_keys().is_empty());
            assert_eq!(handler.completed.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn route_uid_recreation_relist_replaces_real_handler_parent_dependencies() {
        use crate::conf_mgr::sync_runtime::resource_processor::HttpRouteHandler;
        use edgion_resources::prelude_resources::{Gateway, HTTPRoute};
        struct GatewayFixture;
        #[async_trait::async_trait]
        impl ProcessorHandler<Gateway> for GatewayFixture {}
        let runtime = Arc::new(ControllerRuntime::new());
        let gateway = Arc::new(ResourceProcessor::new(
            "Gateway",
            100,
            Arc::new(GatewayFixture),
            runtime.clone(),
        ));
        runtime.processor_registry.register(gateway.clone());
        let processor = Arc::new(ResourceProcessor::new(
            "HTTPRoute",
            100,
            Arc::new(HttpRouteHandler::new("edgion.io/gateway-controller".into())),
            runtime.clone(),
        ));
        runtime.processor_registry.register(processor.clone());
        let cancel = CancellationToken::new();
        let tracker = TaskTracker::new();
        let controller = ResourceController::new(
            "HTTPRoute",
            client(),
            processor.clone(),
            ApiScope::Namespaced(NamespaceWatchMode::AllNamespaces),
            watcher::Config::default(),
        )
        .with_epoch(cancel.clone(), tracker.clone());
        let (tx, stream) = source_channel();
        let task = tracker.spawn(controller.run_sources(vec![(WatchScope::AllNamespaces, stream)]));
        let route = |parent: &str, uid: &str| {
            serde_json::from_value::<HTTPRoute>(serde_json::json!({
                "metadata":{"name":"route","namespace":"a","uid":uid},"spec":{"parentRefs":[{"name":parent}]}
            }))
            .unwrap()
        };
        for event in [Event::Init, Event::InitApply(route("old", "uid1")), Event::InitDone] {
            tx.send(Ok(event)).await.unwrap();
        }
        eventually(|| processor.is_ready()).await;
        let first = tokio::time::timeout(Duration::from_secs(5), gateway.workqueue().dequeue())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.key, "a/old");
        for event in [Event::Init, Event::InitApply(route("new", "uid2")), Event::InitDone] {
            tx.send(Ok(event)).await.unwrap();
        }
        eventually(|| {
            processor
                .get("a/route")
                .is_some_and(|route| route.metadata.uid.as_deref() == Some("uid2"))
        })
        .await;
        assert!(runtime.gateway_route_index.get_routes_for_gateway("a/old").is_empty());
        assert_eq!(runtime.gateway_route_index.get_routes_for_gateway("a/new").len(), 1);
        let mut requeued = std::collections::HashSet::new();
        while requeued.len() < 2 {
            requeued.insert(
                tokio::time::timeout(Duration::from_secs(5), gateway.workqueue().dequeue())
                    .await
                    .unwrap()
                    .unwrap()
                    .key,
            );
        }
        assert_eq!(
            requeued,
            std::collections::HashSet::from(["a/old".into(), "a/new".into()])
        );
        cancel.cancel();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn synthetic_pre_worker_delayed_requeue_saturation_remains_cancellable() {
        let (controller, processor, handler, cancel, tracker) = setup();
        let capacity = super::super::WorkqueueConfig::default().capacity as u64;
        // Deliberately seed the pre-existing queue pressure before source ingestion.
        // This models the scheduler boundary; it is not a real production hook claim.
        for index in 0..capacity {
            processor.on_event_key(format!("a/prior-{index}")).await;
        }
        processor
            .workqueue()
            .enqueue_after(
                "a/prior-delayed".into(),
                Duration::ZERO,
                crate::conf_mgr::sync_runtime::workqueue::TriggerChain::default(),
            )
            .await;
        eventually(|| processor.workqueue().metrics().get_depth() > capacity).await;
        let (tx, stream) = channel();
        let task = tracker.spawn(controller.run_sources(vec![(WatchScope::AllNamespaces, stream)]));
        let mut obj = object("a", "pressure");
        obj.metadata.labels = Some(std::collections::BTreeMap::from([("saturate".into(), "true".into())]));
        send(&tx, vec![Event::Init, Event::InitApply(obj), Event::InitDone]).await;
        eventually(|| {
            processor.workqueue().metrics().get_depth() > capacity
                && processor.workqueue().metrics().delayed_total.load(Ordering::SeqCst) >= capacity + 2
        })
        .await;
        assert_eq!(handler.completed.load(Ordering::SeqCst), 0);
        assert!(!processor.is_ready());
        cancel.cancel();
        task.await.unwrap().unwrap();
        tracker.close();
        tracker.wait().await;
        assert!(processor.raw_keys().is_empty());
    }
}
