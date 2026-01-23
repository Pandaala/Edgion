//! Generic ResourceController for Kubernetes resources
//!
//! Each ResourceController runs a **completely independent** 1-8 flow:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────────┐
//! │                    ResourceController<K> Independent Flow                    │
//! ├─────────────────────────────────────────────────────────────────────────────┤
//! │  Step 1: Create Store                                                       │
//! │  Step 2-6: Reflector stream handles Init phase (LIST + InitAdd + Ready)     │
//! │  Step 7-8: Reflector stream handles Runtime phase (WATCH + Workqueue)       │
//! └─────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! Key benefits:
//! - Each resource type runs completely independently
//! - Single watcher connection per resource type (efficient)
//! - Reflector ensures store is updated before events are processed (correct ordering)
//! - Go operator-style workqueue: ALL events enqueue key, worker decides update/delete
//! - Graceful shutdown support via ShutdownSignal
//! - Unified `process_resource` function for both Init and Runtime phases

use super::metrics::{controller_metrics, InitSyncTimer};
use super::namespace::NamespaceWatchMode;
use super::resource_processor::{
    make_resource_key, process_resource, process_resource_delete, ProcessConfig, ProcessContext, RequeueRegistry,
    ResourceProcessor, SecretRefManager,
};
use super::shutdown::ShutdownSignal;
use super::workqueue::Workqueue;
use crate::core::conf_sync::conf_server::ConfigServer;
use anyhow::Result;
use futures::StreamExt;
use kube::runtime::reflector::{ObjectRef, Store};
use kube::runtime::watcher::Event;
use kube::runtime::{reflector, watcher};
use kube::{Api, Client, Resource, ResourceExt};
use serde::de::DeserializeOwned;
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
}

// ============================================================================
// Legacy types (kept for backward compatibility, will be deprecated)
// ============================================================================

/// Type alias for the apply function that handles InitAdd and runtime events
#[allow(dead_code)]
pub type ApplyFn<K> = Arc<dyn Fn(&ConfigServer, crate::core::conf_sync::traits::ResourceChange, K) + Send + Sync>;

/// Type alias for the optional filter function
#[allow(dead_code)]
pub type FilterFn<K> = Arc<dyn Fn(&K) -> bool + Send + Sync>;

/// Type alias for the get function that retrieves object from ConfigServer cache
/// Used by worker to get deleted object for EventDelete (replaces pending_deletes)
#[allow(dead_code)]
pub type GetFn<K> = Arc<dyn Fn(&ConfigServer, &str) -> Option<K> + Send + Sync>;

// ============================================================================
// ResourceController with Processor support
// ============================================================================

/// Generic ResourceController that encapsulates the complete 1-8 flow for a single resource type
///
/// Uses a single watcher + reflector stream with Go operator-style workqueue:
/// - Init phase: InitApply events are processed directly (no workqueue)
/// - Runtime phase: ALL events (Apply/Delete) enqueue key only, worker decides update/delete
///   - Worker checks store vs ConfigServer cache: store has → process, cache has but store doesn't → delete
pub struct ResourceController<K, P>
where
    K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
    P: ResourceProcessor<K> + 'static,
{
    kind: &'static str,
    client: Client,
    config_server: Arc<ConfigServer>,
    watcher_config: watcher::Config,

    // API creation based on scope
    api_scope: ApiScope,

    // Resource processor
    processor: Arc<P>,

    // Processing configuration
    process_config: ProcessConfig,

    // RequeueRegistry for cross-resource requeue
    requeue_registry: Arc<RequeueRegistry>,

    // SecretRefManager for secret reference tracking
    secret_ref_manager: Arc<SecretRefManager>,

    /// Namespace filter for MultipleNamespaces mode
    namespace_filter: Option<Vec<String>>,

    // Graceful shutdown signal
    shutdown_signal: Option<ShutdownSignal>,

    /// Optional relink signal sender for notifying when 410 Gone is detected
    relink_signal: Option<RelinkSignalSender>,

    /// Phantom data for type parameter K
    _marker: std::marker::PhantomData<K>,
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

impl<K, P> ResourceController<K, P>
where
    K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
    P: ResourceProcessor<K> + 'static,
{
    /// Create a new ResourceController with Processor
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kind: &'static str,
        client: Client,
        config_server: Arc<ConfigServer>,
        watcher_config: watcher::Config,
        api_scope: ApiScope,
        processor: P,
        process_config: ProcessConfig,
        requeue_registry: Arc<RequeueRegistry>,
        secret_ref_manager: Arc<SecretRefManager>,
        shutdown_signal: Option<ShutdownSignal>,
        relink_signal: Option<RelinkSignalSender>,
    ) -> Self {
        // Extract namespace filter from api_scope for MultipleNamespaces mode
        let namespace_filter = api_scope.namespace_filter();

        Self {
            kind,
            client,
            config_server,
            watcher_config,
            api_scope,
            processor: Arc::new(processor),
            process_config,
            requeue_registry,
            secret_ref_manager,
            namespace_filter,
            shutdown_signal,
            relink_signal,
            _marker: std::marker::PhantomData,
        }
    }

    /// Run the complete 1-8 flow independently for namespaced resources
    pub async fn run_namespaced(self) -> Result<()>
    where
        K: Resource<Scope = kube::core::NamespaceResourceScope>,
    {
        let api = match &self.api_scope {
            ApiScope::Namespaced(watch_mode) => match watch_mode {
                NamespaceWatchMode::AllNamespaces => Api::all(self.client.clone()),
                NamespaceWatchMode::SingleNamespace(ns) => Api::namespaced(self.client.clone(), ns),
                NamespaceWatchMode::MultipleNamespaces(_) => Api::all(self.client.clone()),
            },
            ApiScope::ClusterScoped => {
                unreachable!("run_namespaced called with ClusterScoped scope - use run_cluster_scoped instead")
            }
        };
        self.run_with_api(api).await
    }

    /// Run for cluster-scoped resources
    pub async fn run_cluster_scoped(self) -> Result<()>
    where
        K: Resource<Scope = kube::core::ClusterResourceScope>,
    {
        let api = Api::all(self.client.clone());
        self.run_with_api(api).await
    }

    /// Internal run implementation with provided API
    ///
    /// Uses a single watcher + reflector stream for both init and runtime phases:
    /// 1. Create store and reflector
    /// 2. Process Init events (LIST phase) - process directly (unified logic)
    /// 3. Mark cache ready after InitDone
    /// 4. Process runtime events (WATCH phase) - enqueue key, worker processes
    async fn run_with_api(self, api: Api<K>) -> Result<()> {
        let kind = self.kind;

        // Record controller started
        controller_metrics().controller_started();

        tracing::info!(
            component = "resource_controller",
            kind = kind,
            "Starting independent ResourceController with Go operator-style workqueue"
        );

        // Step 1: Create Store
        let (store, writer) = reflector::store();
        tracing::debug!(
            component = "resource_controller",
            kind = kind,
            "Step 1: Created reflector store"
        );

        // Create single watcher + reflector stream
        let watcher_stream = watcher(api, self.watcher_config.clone());
        let mut stream = Box::pin(reflector(writer, watcher_stream));

        // Create workqueue for runtime phase
        let queue = Arc::new(Workqueue::with_defaults(kind));

        // Track init phase
        let mut init_timer = Some(InitSyncTimer::start(kind));
        let mut init_count = 0;
        let mut init_done = false;

        // Worker handle for graceful shutdown
        let mut worker_handle: Option<JoinHandle<()>> = None;

        tracing::debug!(
            component = "resource_controller",
            kind = kind,
            "Step 2: Starting reflector stream (Init phase)"
        );

        // Main event loop - handles both init and runtime phases
        loop {
            let event = if let Some(ref mut shutdown) = self.shutdown_signal.clone() {
                tokio::select! {
                    event = stream.next() => event,
                    _ = shutdown.wait() => {
                        tracing::info!(
                            component = "resource_controller",
                            kind = kind,
                            "Received shutdown signal"
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
                            }
                        }
                        Event::InitApply(obj) => {
                            // Init phase: process directly using unified process_resource
                            let ctx = ProcessContext::new(
                                &self.config_server,
                                self.process_config.metadata_filter.as_ref(),
                                self.namespace_filter.as_ref(),
                                &self.requeue_registry,
                                &self.secret_ref_manager,
                            );

                            if process_resource(obj, &*self.processor, &ctx, true, kind) {
                                init_count += 1;
                            }
                        }
                        Event::InitDone => {
                            let init_duration = init_timer.take().map(|t| t.complete(init_count)).unwrap_or(0.0);
                            tracing::info!(
                                component = "resource_controller",
                                kind = kind,
                                count = init_count,
                                duration_secs = init_duration,
                                "Init phase complete (Step 5: Resources processed)"
                            );

                            // Mark cache ready
                            self.config_server.set_cache_ready_by_kind(kind);
                            tracing::info!(
                                component = "resource_controller",
                                kind = kind,
                                "Step 6: Cache marked ready, entering runtime phase"
                            );

                            init_done = true;

                            // Spawn worker for runtime phase
                            worker_handle = Some(spawn_worker(
                                queue.clone(),
                                store.clone(),
                                self.config_server.clone(),
                                self.processor.clone(),
                                self.process_config.clone(),
                                self.namespace_filter.clone(),
                                self.requeue_registry.clone(),
                                self.secret_ref_manager.clone(),
                                kind,
                                self.shutdown_signal.clone(),
                            ));

                            tracing::info!(
                                component = "resource_controller",
                                kind = kind,
                                "Step 7-8: Worker started, processing runtime events via workqueue"
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
                                let ctx = ProcessContext::new(
                                    &self.config_server,
                                    self.process_config.metadata_filter.as_ref(),
                                    self.namespace_filter.as_ref(),
                                    &self.requeue_registry,
                                    &self.secret_ref_manager,
                                );

                                if process_resource(obj, &*self.processor, &ctx, true, kind) {
                                    init_count += 1;
                                }
                            } else {
                                // Runtime phase - enqueue key for worker
                                if self.processor.filter(&obj) && passes_namespace_filter(&obj, &self.namespace_filter)
                                {
                                    let key = make_resource_key(&obj);
                                    queue.enqueue(key).await;
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
                                if self.processor.filter(&obj) && passes_namespace_filter(&obj, &self.namespace_filter)
                                {
                                    let key = make_resource_key(&obj);
                                    queue.enqueue(key).await;
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
        tracing::warn!(component = "resource_controller", kind = kind, "Controller stopped");
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
/// - Check store (K8s state) vs ConfigServer cache (our state)
/// - If store has object → process with unified logic
/// - If store doesn't have but cache has → delete
/// - If neither has → skip
#[allow(clippy::too_many_arguments)]
fn spawn_worker<K, P>(
    queue: Arc<Workqueue>,
    store: Store<K>,
    config_server: Arc<ConfigServer>,
    processor: Arc<P>,
    process_config: ProcessConfig,
    namespace_filter: Option<Vec<String>>,
    requeue_registry: Arc<RequeueRegistry>,
    secret_ref_manager: Arc<SecretRefManager>,
    kind: &'static str,
    shutdown_signal: Option<ShutdownSignal>,
) -> JoinHandle<()>
where
    K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
    P: ResourceProcessor<K> + 'static,
{
    // Register workqueue to RequeueRegistry for cross-resource requeue
    requeue_registry.register(kind, queue.clone());

    tokio::spawn(async move {
        loop {
            let item = if let Some(ref mut shutdown) = shutdown_signal.clone() {
                tokio::select! {
                    item = queue.dequeue() => item,
                    _ = shutdown.wait() => {
                        tracing::info!(
                            component = "resource_controller",
                            kind = kind,
                            "Worker received shutdown signal"
                        );
                        break;
                    }
                }
            } else {
                queue.dequeue().await
            };

            match item {
                Some(work_item) => {
                    // Create ProcessContext for this work item
                    let ctx = ProcessContext::new(
                        &config_server,
                        process_config.metadata_filter.as_ref(),
                        namespace_filter.as_ref(),
                        &requeue_registry,
                        &secret_ref_manager,
                    );

                    process_work_item(&work_item.key, &store, &*processor, &ctx, kind);
                    queue.done(&work_item.key);
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

/// Process a work item from the queue (Go operator-style reconciliation)
fn process_work_item<K, P>(key: &str, store: &Store<K>, processor: &P, ctx: &ProcessContext, kind: &'static str)
where
    K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
    P: ResourceProcessor<K>,
{
    // Parse key to ObjectRef
    let obj_ref = parse_resource_key::<K>(key);

    // Get current state from store (K8s state)
    let store_obj = store.get(&obj_ref);

    // Get current state from ConfigServer cache (our state)
    let cache_obj = processor.get(ctx.config_server, key);

    match (store_obj, cache_obj) {
        (Some(obj), _) => {
            // Object exists in store → process
            process_resource((*obj).clone(), processor, ctx, false, kind);
        }
        (None, Some(cached_obj)) => {
            // Object not in store but exists in cache → Delete
            process_resource_delete(cached_obj, processor, ctx, kind);
        }
        (None, None) => {
            // Not in store and not in cache → already processed
            tracing::trace!(
                component = "resource_controller",
                kind = kind,
                key = %key,
                "Object not found in store or cache, skipping (already processed)"
            );
        }
    }
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

// ============================================================================
// Builder
// ============================================================================

/// Builder for ResourceController with Processor support
pub struct ResourceControllerBuilder<K, P = ()>
where
    K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
{
    kind: &'static str,
    api_scope: Option<ApiScope>,
    processor: Option<P>,
    process_config: Option<ProcessConfig>,
    requeue_registry: Option<Arc<RequeueRegistry>>,
    secret_ref_manager: Option<Arc<SecretRefManager>>,
    shutdown_signal: Option<ShutdownSignal>,
    relink_signal: Option<RelinkSignalSender>,
    _marker: std::marker::PhantomData<K>,
}

impl<K> ResourceControllerBuilder<K, ()>
where
    K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
{
    /// Create a new builder
    pub fn new(kind: &'static str) -> Self {
        Self {
            kind,
            api_scope: None,
            processor: None,
            process_config: None,
            requeue_registry: None,
            secret_ref_manager: None,
            shutdown_signal: None,
            relink_signal: None,
            _marker: std::marker::PhantomData,
        }
    }
}

impl<K, P> ResourceControllerBuilder<K, P>
where
    K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
{
    /// Set namespaced scope with watch mode
    pub fn namespaced(mut self, watch_mode: NamespaceWatchMode) -> Self {
        self.api_scope = Some(ApiScope::Namespaced(watch_mode));
        self
    }

    /// Set cluster-scoped
    pub fn cluster_scoped(mut self) -> Self {
        self.api_scope = Some(ApiScope::ClusterScoped);
        self
    }

    /// Set shutdown signal for graceful shutdown
    pub fn with_shutdown(mut self, signal: ShutdownSignal) -> Self {
        self.shutdown_signal = Some(signal);
        self
    }

    /// Set relink signal sender for 410 Gone detection
    pub fn with_relink_signal(mut self, signal: RelinkSignalSender) -> Self {
        self.relink_signal = Some(signal);
        self
    }

    /// Set the resource processor
    pub fn with_processor<P2: ResourceProcessor<K>>(self, processor: P2) -> ResourceControllerBuilder<K, P2> {
        ResourceControllerBuilder {
            kind: self.kind,
            api_scope: self.api_scope,
            processor: Some(processor),
            process_config: self.process_config,
            requeue_registry: self.requeue_registry,
            secret_ref_manager: self.secret_ref_manager,
            shutdown_signal: self.shutdown_signal,
            relink_signal: self.relink_signal,
            _marker: std::marker::PhantomData,
        }
    }

    /// Set the processing configuration
    pub fn with_process_config(mut self, config: ProcessConfig) -> Self {
        self.process_config = Some(config);
        self
    }

    /// Set the RequeueRegistry for cross-resource requeue
    pub fn with_requeue_registry(mut self, registry: Arc<RequeueRegistry>) -> Self {
        self.requeue_registry = Some(registry);
        self
    }

    /// Set the SecretRefManager for secret reference tracking
    pub fn with_secret_ref_manager(mut self, manager: Arc<SecretRefManager>) -> Self {
        self.secret_ref_manager = Some(manager);
        self
    }
}

impl<K, P> ResourceControllerBuilder<K, P>
where
    K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
    P: ResourceProcessor<K> + 'static,
{
    /// Build the ResourceController
    pub fn build(
        self,
        client: Client,
        config_server: Arc<ConfigServer>,
        watcher_config: watcher::Config,
    ) -> ResourceController<K, P> {
        ResourceController::new(
            self.kind,
            client,
            config_server,
            watcher_config,
            self.api_scope.expect("API scope must be set"),
            self.processor.expect("Processor must be set"),
            self.process_config.unwrap_or_default(),
            self.requeue_registry.expect("RequeueRegistry must be set"),
            self.secret_ref_manager.expect("SecretRefManager must be set"),
            self.shutdown_signal,
            self.relink_signal,
        )
    }
}
