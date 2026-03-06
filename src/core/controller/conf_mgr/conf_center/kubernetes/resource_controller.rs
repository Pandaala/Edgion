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
//! - Single watcher connection per resource type (efficient)
//! - Processor handles all lifecycle events directly
//! - Go operator-style workqueue: ALL events enqueue key, worker decides update/delete
//! - Graceful shutdown support via ShutdownSignal

use super::controller_metrics;
use super::namespace::NamespaceWatchMode;
use super::InitSyncTimer;
use super::ShutdownSignal;
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    extract_status_value, ResourceProcessor, WorkItemResult,
};
use crate::types::ResourceMeta;
use anyhow::Result;
use futures::StreamExt;
use kube::api::{Patch, PatchParams};
use kube::runtime::reflector::{ObjectRef, Store};
use kube::runtime::watcher::Event;
use kube::runtime::{reflector, watcher};
use kube::{Api, Client, Resource, ResourceExt};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Signal sender for relink requests
/// When a ResourceController detects 410 Gone (re-LIST needed),
/// it sends a signal through this channel
pub type RelinkSignalSender = mpsc::Sender<RelinkReason>;

/// Reason for relink request
#[derive(Debug, Clone)]
pub enum RelinkReason {
    /// Watcher received 410 Gone error
    GoneError,
    /// Watcher reconnected (detected by Event::Init after init_done)
    WatcherReconnected,
    /// Manual reload requested via Admin API
    ReloadRequested,
}

/// Generic ResourceController that encapsulates the complete lifecycle for a single resource type
///
/// Uses a single watcher + reflector stream with Go operator-style workqueue:
/// - Init phase: InitApply events are processed directly via processor.on_init_apply()
/// - Runtime phase: ALL events (Apply/Delete) enqueue key, worker decides update/delete
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

    /// Optional relink signal sender for notifying when 410 Gone is detected
    relink_signal: Option<RelinkSignalSender>,
}

/// API scope for resource (namespaced or cluster-scoped)
#[derive(Clone)]
pub enum ApiScope {
    /// Namespaced resource with watch mode
    Namespaced(NamespaceWatchMode),
    /// Cluster-scoped resource
    ClusterScoped,
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
            relink_signal: None,
        }
    }

    /// Set shutdown signal
    pub fn with_shutdown(mut self, signal: ShutdownSignal) -> Self {
        self.shutdown_signal = Some(signal);
        self
    }

    /// Set relink signal sender
    pub fn with_relink_signal(mut self, sender: RelinkSignalSender) -> Self {
        self.relink_signal = Some(sender);
        self
    }

    /// Run for namespaced resources
    pub async fn run_namespaced(self) -> Result<()>
    where
        K: Resource<Scope = kube::core::NamespaceResourceScope>,
    {
        let api: Api<K> = match &self.api_scope {
            ApiScope::Namespaced(NamespaceWatchMode::AllNamespaces) => Api::all(self.client.clone()),
            ApiScope::Namespaced(NamespaceWatchMode::SingleNamespace(ns)) => Api::namespaced(self.client.clone(), ns),
            ApiScope::Namespaced(NamespaceWatchMode::MultipleNamespaces(_)) => {
                // For multiple namespaces, we watch all and filter in processing
                Api::all(self.client.clone())
            }
            ApiScope::ClusterScoped => {
                return Err(anyhow::anyhow!("Cannot run cluster-scoped resource as namespaced"));
            }
        };

        self.run_with_api(api).await
    }

    /// Run for cluster-scoped resources
    pub async fn run_cluster_scoped(self) -> Result<()>
    where
        K: Resource<Scope = kube::core::ClusterResourceScope>,
    {
        let api: Api<K> = Api::all(self.client.clone());
        self.run_with_api(api).await
    }

    /// Internal run with API
    async fn run_with_api(self, api: Api<K>) -> Result<()> {
        let kind = self.kind;

        controller_metrics().controller_started();
        tracing::info!(
            component = "resource_controller",
            kind = kind,
            "Starting ResourceController"
        );

        // Create store and reflector stream
        let (store, writer) = reflector::store();
        let watcher_stream = watcher(api, self.watcher_config.clone());
        let mut stream = Box::pin(reflector(writer, watcher_stream));

        let mut init_done = false;
        let mut init_count: usize = 0;
        let mut init_timer: Option<InitSyncTimer> = None;
        let mut worker_handle: Option<JoinHandle<()>> = None;
        let mut shutdown = self.shutdown_signal.clone();

        // Main event loop
        loop {
            let event = if let Some(ref mut signal) = shutdown {
                tokio::select! {
                    event = stream.next() => event,
                    _ = signal.wait() => {
                        tracing::info!(
                            component = "resource_controller",
                            kind = kind,
                            "Shutdown signal received, stopping controller"
                        );
                        break;
                    }
                }
            } else {
                stream.next().await
            };

            match event {
                Some(Ok(event)) => {
                    match event {
                        Event::Init => {
                            // Start of init phase (LIST)
                            if init_done {
                                // Watcher reconnecting - exit and let upper layer rebuild
                                tracing::warn!(
                                    component = "resource_controller",
                                    kind = kind,
                                    "Watcher reconnecting (possible 410 Gone), requesting full rebuild"
                                );

                                if let Some(ref signal) = self.relink_signal {
                                    let _ = signal.try_send(RelinkReason::WatcherReconnected);
                                    tracing::info!(
                                        component = "resource_controller",
                                        kind = kind,
                                        "Sent relink signal due to watcher reconnection, exiting"
                                    );
                                }
                                break;
                            } else {
                                tracing::debug!(component = "resource_controller", kind = kind, "Init phase started");
                                init_timer = Some(InitSyncTimer::start(kind));
                                self.processor.on_init();
                            }
                        }
                        Event::InitApply(obj) => {
                            // Init phase: process directly via processor
                            // K8s mode: pass None for existing_status_json (status is already in obj from K8s API)
                            if passes_namespace_filter(&obj, &self.namespace_filter) {
                                let result = self.processor.on_init_apply(obj, None);

                                // Handle status persistence (same as runtime)
                                if let WorkItemResult::Processed { obj, status_changed } = result {
                                    init_count += 1;
                                    if status_changed {
                                        if let Some(status_value) = extract_status_value(&obj) {
                                            let name = obj.meta().name.as_deref().unwrap_or("");
                                            let namespace = obj.meta().namespace.as_deref();

                                            // Persist status to K8s API
                                            if let Err(e) = persist_k8s_status::<K>(
                                                &self.client,
                                                &self.api_scope,
                                                namespace,
                                                name,
                                                &status_value,
                                            )
                                            .await
                                            {
                                                tracing::warn!(
                                                    component = "resource_controller",
                                                    kind = kind,
                                                    name = %name,
                                                    error = %e,
                                                    "Failed to persist status during init"
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Event::InitDone => {
                            let init_duration = init_timer.take().map(|t| t.complete(init_count)).unwrap_or(0.0);
                            tracing::info!(
                                component = "resource_controller",
                                kind = kind,
                                count = init_count,
                                duration_secs = init_duration,
                                "Init phase complete"
                            );

                            // Mark cache ready
                            self.processor.on_init_done();
                            init_done = true;

                            // Spawn worker for runtime phase
                            worker_handle = Some(spawn_worker(
                                self.processor.clone(),
                                store.clone(),
                                self.namespace_filter.clone(),
                                kind,
                                self.shutdown_signal.clone(),
                                self.client.clone(),
                                self.api_scope.clone(),
                            ));

                            tracing::info!(
                                component = "resource_controller",
                                kind = kind,
                                "Worker started, processing runtime events via workqueue"
                            );
                        }
                        Event::Apply(obj) => {
                            if !init_done {
                                // During init phase, treat as InitApply
                                tracing::warn!(
                                    component = "resource_controller",
                                    kind = kind,
                                    "Received Apply event during init phase, treating as InitApply"
                                );
                                if passes_namespace_filter(&obj, &self.namespace_filter) {
                                    let result = self.processor.on_init_apply(obj, None);

                                    if let WorkItemResult::Processed { obj, status_changed } = result {
                                        init_count += 1;
                                        if status_changed {
                                            if let Some(status_value) = extract_status_value(&obj) {
                                                let name = obj.meta().name.as_deref().unwrap_or("");
                                                let namespace = obj.meta().namespace.as_deref();

                                                if let Err(e) = persist_k8s_status::<K>(
                                                    &self.client,
                                                    &self.api_scope,
                                                    namespace,
                                                    name,
                                                    &status_value,
                                                )
                                                .await
                                                {
                                                    tracing::warn!(
                                                        component = "resource_controller",
                                                        kind = kind,
                                                        name = %name,
                                                        error = %e,
                                                        "Failed to persist status during apply-as-init"
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            } else {
                                // Runtime phase - enqueue key for worker
                                if passes_namespace_filter(&obj, &self.namespace_filter) {
                                    self.processor.on_apply(&obj);
                                }
                            }
                        }
                        Event::Delete(obj) => {
                            if !init_done {
                                tracing::warn!(
                                    component = "resource_controller",
                                    kind = kind,
                                    "Received Delete event during init phase, ignoring"
                                );
                            } else {
                                // Runtime phase - enqueue key for worker
                                if passes_namespace_filter(&obj, &self.namespace_filter) {
                                    self.processor.on_delete(&obj);
                                }
                            }
                        }
                    }
                }
                Some(Err(e)) => {
                    tracing::error!(
                        component = "resource_controller",
                        kind = kind,
                        error = %e,
                        "Watcher error"
                    );
                }
                None => {
                    tracing::warn!(
                        component = "resource_controller",
                        kind = kind,
                        "Watcher stream ended unexpectedly"
                    );
                    break;
                }
            }
        }

        // Wait for worker task to finish gracefully
        if let Some(handle) = worker_handle {
            tracing::info!(
                component = "resource_controller",
                kind = kind,
                "Waiting for worker task to finish..."
            );

            match tokio::time::timeout(Duration::from_secs(5), handle).await {
                Ok(Ok(())) => {
                    tracing::info!(
                        component = "resource_controller",
                        kind = kind,
                        "Worker task finished gracefully"
                    );
                }
                Ok(Err(e)) => {
                    tracing::error!(
                        component = "resource_controller",
                        kind = kind,
                        error = %e,
                        "Worker task panicked"
                    );
                }
                Err(_) => {
                    tracing::warn!(
                        component = "resource_controller",
                        kind = kind,
                        "Worker task did not finish within 5 seconds, aborting"
                    );
                }
            }
        }

        controller_metrics().controller_stopped();
        tracing::info!(
            component = "resource_controller",
            kind = kind,
            "ResourceController stopped"
        );

        Ok(())
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
fn spawn_worker<K>(
    processor: Arc<ResourceProcessor<K>>,
    store: Store<K>,
    namespace_filter: Option<Vec<String>>,
    kind: &'static str,
    shutdown_signal: Option<ShutdownSignal>,
    client: Client,
    api_scope: ApiScope,
) -> JoinHandle<()>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
{
    let workqueue = processor.workqueue();

    tokio::spawn(async move {
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
                    let obj_ref = parse_resource_key::<K>(&work_item.key);
                    let store_obj = store.get(&obj_ref).map(|arc| (*arc).clone());

                    // Check namespace filter for store object
                    let should_process = match &store_obj {
                        Some(obj) => passes_namespace_filter(obj, &namespace_filter),
                        None => true, // Always process deletes
                    };

                    if should_process {
                        // K8s mode: pass None for existing_status_json (status is already in store_obj from K8s API)
                        let result = processor.process_work_item(&work_item.key, store_obj, None, work_item.trigger_chain.clone());

                        // Persist status to K8s API when status changes
                        if let WorkItemResult::Processed { obj, status_changed } = result {
                            if status_changed {
                                let name = obj.meta().name.as_deref().unwrap_or("");
                                let namespace = obj.meta().namespace.as_deref();

                                match extract_status_value(&obj) {
                                    Some(status_value) => {
                                        if let Err(e) =
                                            persist_k8s_status::<K>(&client, &api_scope, namespace, name, &status_value)
                                                .await
                                        {
                                            tracing::warn!(
                                                component = "resource_controller",
                                                kind = kind,
                                                key = %work_item.key,
                                                error = %e,
                                                "Failed to persist status to K8s API"
                                            );
                                        }
                                    }
                                    None => {
                                        // Status changed but extraction failed - log warning
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

        tracing::info!(component = "resource_controller", kind = kind, "Worker task ended");
    })
}

/// Persist status to K8s API using JSON Merge Patch on status subresource
///
/// Uses DynamicObject to avoid the Scope type constraint issue.
/// The `ApiResource` is built from K's `#[kube]`-derived GVK + plural metadata.
async fn persist_k8s_status<K>(
    client: &Client,
    api_scope: &ApiScope,
    namespace: Option<&str>,
    name: &str,
    status_value: &serde_json::Value,
) -> Result<(), kube::Error>
where
    K: Resource + Clone + Debug + DeserializeOwned + Serialize,
    K::DynamicType: Default,
{
    use kube::core::DynamicObject;
    use kube::discovery::ApiResource;

    let dt = K::DynamicType::default();
    let api_resource = ApiResource::from_gvk_with_plural(
        &kube::core::GroupVersionKind {
            group: K::group(&dt).to_string(),
            version: K::version(&dt).to_string(),
            kind: K::kind(&dt).to_string(),
        },
        &K::plural(&dt),
    );

    let patch = serde_json::json!({ "status": status_value });
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

    tracing::trace!(
        component = "resource_controller",
        name = name,
        namespace = namespace,
        "Persisted status to K8s API"
    );

    Ok(())
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

/// Parse a resource key back to ObjectRef
fn parse_resource_key<K>(key: &str) -> ObjectRef<K>
where
    K: Resource,
    K::DynamicType: Default,
{
    if let Some((ns, name)) = key.split_once('/') {
        ObjectRef::<K>::new(name).within(ns)
    } else {
        ObjectRef::<K>::new(key)
    }
}
