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
        let _active = controller_metrics().active_controller();
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

// Status persistence regression tests live in `tests/unit/conf_mgr/conf_center/kubernetes/resource_controller_test/status_persistence_tests.rs`.
#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/conf_center/kubernetes/resource_controller_test/status_persistence_tests.rs"]
mod status_persistence_tests;

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/conf_center/kubernetes/resource_controller_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../tests/support/conf_mgr/conf_center/kubernetes/resource_controller.rs"]
mod test_support;
#[cfg(test)]
use test_support::*;
