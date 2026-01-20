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

use super::metrics::{controller_metrics, InitSyncTimer};
use super::namespace::NamespaceWatchMode;
use super::shutdown::ShutdownSignal;
use super::workqueue::Workqueue;
use crate::core::conf_sync::conf_server::ConfigServer;
use crate::core::conf_sync::traits::ResourceChange;
use anyhow::Result;
use dashmap::DashMap;
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

/// Type alias for the apply function that handles InitAdd and runtime events
pub type ApplyFn<K> = Arc<dyn Fn(&ConfigServer, ResourceChange, K) + Send + Sync>;

/// Type alias for the optional filter function
pub type FilterFn<K> = Arc<dyn Fn(&K) -> bool + Send + Sync>;

/// Generic ResourceController that encapsulates the complete 1-8 flow for a single resource type
///
/// Uses a single watcher + reflector stream with Go operator-style workqueue:
/// - Init phase: InitApply events are applied directly as InitAdd
/// - Runtime phase: ALL events (Apply/Delete) enqueue key only, worker decides update/delete
///   - Worker checks store: exists → EventUpdate, not exists → check pending_deletes → EventDelete
pub struct ResourceController<K>
where
    K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
{
    kind: &'static str,
    client: Client,
    config_server: Arc<ConfigServer>,
    watcher_config: watcher::Config,

    // API creation based on scope
    api_scope: ApiScope,

    // Difference handling via closures
    apply_fn: ApplyFn<K>,
    filter_fn: Option<FilterFn<K>>,
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
    K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
{
    /// Create a new ResourceController
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kind: &'static str,
        client: Client,
        config_server: Arc<ConfigServer>,
        watcher_config: watcher::Config,
        api_scope: ApiScope,
        apply_fn: ApplyFn<K>,
        filter_fn: Option<FilterFn<K>>,
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
            apply_fn,
            filter_fn,
            namespace_filter,
            shutdown_signal,
            relink_signal,
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
    /// 2. Process Init events (LIST phase) - apply InitAdd directly
    /// 3. Mark cache ready after InitDone
    /// 4. Process runtime events (WATCH phase) - ALL events enqueue key, worker decides
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
        // This stream will:
        // 1. Update store first (via writer)
        // 2. Then yield the event for processing
        let watcher_stream = watcher(api, self.watcher_config.clone());
        let mut stream = Box::pin(reflector(writer, watcher_stream));

        // Create workqueue for runtime phase
        let queue = Arc::new(Workqueue::with_defaults(kind));

        // Pending deletes: stores deleted objects temporarily for worker to process
        // Key: "namespace/name" or "name", Value: the deleted object
        let pending_deletes: Arc<DashMap<String, K>> = Arc::new(DashMap::new());

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
                                // Watcher reconnecting - this happens on 410 Gone or network issues
                                tracing::warn!(
                                    component = "resource_controller",
                                    kind = kind,
                                    "Watcher reconnecting (possible 410 Gone), starting re-sync via LIST"
                                );

                                // Send relink signal if configured
                                if let Some(ref signal) = self.relink_signal {
                                    let _ = signal.try_send(RelinkReason::WatcherReconnected);
                                    tracing::info!(
                                        component = "resource_controller",
                                        kind = kind,
                                        "Sent relink signal due to watcher reconnection"
                                    );
                                }
                            } else {
                                // First connection
                                tracing::debug!(component = "resource_controller", kind = kind, "Init phase started");
                            }
                        }
                        Event::InitApply(obj) => {
                            // Store already updated by reflector
                            if passes_filters(&obj, &self.namespace_filter, &self.filter_fn) {
                                // Init phase always applies directly (both first init and re-sync)
                                // Clear any stale pending delete for this key (only needed after first init)
                                if init_done {
                                    let key = make_resource_key(&obj);
                                    pending_deletes.remove(&key);
                                }
                                (self.apply_fn)(&self.config_server, ResourceChange::InitAdd, obj);
                                if !init_done {
                                    init_count += 1;
                                }
                            }
                        }
                        Event::InitDone => {
                            if !init_done {
                                // First init complete
                                let init_duration = init_timer.take().map(|t| t.complete(init_count)).unwrap_or(0.0);
                                tracing::info!(
                                    component = "resource_controller",
                                    kind = kind,
                                    count = init_count,
                                    duration_secs = init_duration,
                                    "Init phase complete (Step 5: InitAdd applied)"
                                );

                                // Mark cache ready
                                self.config_server.set_cache_ready_by_kind(kind);
                                tracing::info!(
                                    component = "resource_controller",
                                    kind = kind,
                                    "Step 6: Cache marked ready, entering runtime phase"
                                );

                                init_done = true;

                                // Spawn worker for runtime phase (only once) and save handle
                                worker_handle = Some(spawn_worker(
                                    queue.clone(),
                                    store.clone(),
                                    pending_deletes.clone(),
                                    self.config_server.clone(),
                                    self.apply_fn.clone(),
                                    self.filter_fn.clone(),
                                    self.namespace_filter.clone(),
                                    kind,
                                    self.shutdown_signal.clone(),
                                ));

                                tracing::info!(
                                    component = "resource_controller",
                                    kind = kind,
                                    "Step 7-8: Worker started, processing runtime events via workqueue"
                                );
                            } else {
                                // Watcher reconnected - worker already running
                                tracing::info!(
                                    component = "resource_controller",
                                    kind = kind,
                                    "Watcher reconnected, re-sync complete, worker already running"
                                );
                            }
                        }
                        Event::Apply(obj) => {
                            if !init_done {
                                // During init phase, this shouldn't happen but handle gracefully
                                tracing::warn!(
                                    component = "resource_controller",
                                    kind = kind,
                                    "Received Apply event during init phase, treating as InitAdd"
                                );
                                if passes_filters(&obj, &self.namespace_filter, &self.filter_fn) {
                                    (self.apply_fn)(&self.config_server, ResourceChange::InitAdd, obj);
                                    init_count += 1;
                                }
                            } else {
                                // Runtime phase - enqueue key for worker
                                // Store is already updated by reflector
                                if passes_filters(&obj, &self.namespace_filter, &self.filter_fn) {
                                    let key = make_resource_key(&obj);
                                    // Clear any stale pending delete for this key (object was recreated)
                                    pending_deletes.remove(&key);
                                    queue.enqueue(key).await;
                                }
                            }
                        }
                        Event::Delete(obj) => {
                            if !init_done {
                                // During init phase, delete shouldn't happen
                                tracing::warn!(
                                    component = "resource_controller",
                                    kind = kind,
                                    "Received Delete event during init phase, ignoring"
                                );
                            } else {
                                // Runtime phase - enqueue key for worker (Go operator style)
                                // Store is already updated by reflector (object removed)
                                if passes_filters(&obj, &self.namespace_filter, &self.filter_fn) {
                                    let key = make_resource_key(&obj);
                                    // Store the deleted object for worker to use
                                    pending_deletes.insert(key.clone(), obj);
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
                    // Watcher will reconnect automatically
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

        // Wait for worker task to finish gracefully (with timeout)
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

        // Record controller stopped
        controller_metrics().controller_stopped();

        tracing::warn!(component = "resource_controller", kind = kind, "Controller stopped");
        Ok(())
    }
}

/// Spawn worker task for processing workqueue items
///
/// Worker implements Go operator-style reconciliation:
/// - Dequeue key from workqueue
/// - Check store for current state
/// - If exists → EventUpdate with latest object from store
/// - If not exists → check pending_deletes → EventDelete with deleted object
///
/// Returns JoinHandle for graceful shutdown
#[allow(clippy::too_many_arguments)]
fn spawn_worker<K>(
    queue: Arc<Workqueue>,
    store: Store<K>,
    pending_deletes: Arc<DashMap<String, K>>,
    config_server: Arc<ConfigServer>,
    apply_fn: ApplyFn<K>,
    filter_fn: Option<FilterFn<K>>,
    namespace_filter: Option<Vec<String>>,
    kind: &'static str,
    shutdown_signal: Option<ShutdownSignal>,
) -> JoinHandle<()>
where
    K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
{
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
                    process_work_item(
                        &work_item.key,
                        &store,
                        &pending_deletes,
                        &namespace_filter,
                        &filter_fn,
                        &config_server,
                        &apply_fn,
                        kind,
                    );
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
///
/// Decision logic:
/// 1. Check store for current state
/// 2. If object exists in store → EventUpdate (use latest from store)
/// 3. If object not in store → check pending_deletes
///    - If in pending_deletes → EventDelete (use the deleted object)
///    - If not in pending_deletes → skip (already processed or never existed)
#[allow(clippy::too_many_arguments)]
fn process_work_item<K>(
    key: &str,
    store: &Store<K>,
    pending_deletes: &DashMap<String, K>,
    namespace_filter: &Option<Vec<String>>,
    filter_fn: &Option<FilterFn<K>>,
    config_server: &ConfigServer,
    apply_fn: &ApplyFn<K>,
    kind: &'static str,
) where
    K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
{
    // Parse key to ObjectRef
    let obj_ref = parse_resource_key::<K>(key);

    // Get current state from store (already updated by reflector)
    match store.get(&obj_ref) {
        Some(obj) => {
            // Object exists in store → EventUpdate
            // Clear any stale pending delete (object was recreated before we processed)
            pending_deletes.remove(key);

            if !passes_filters(&*obj, namespace_filter, filter_fn) {
                return;
            }

            let name = obj.name_any();
            let namespace = obj.namespace().unwrap_or_default();

            tracing::debug!(
                component = "resource_controller",
                kind = kind,
                name = %name,
                namespace = %namespace,
                "Applying EventUpdate from worker (object exists in store)"
            );
            (apply_fn)(config_server, ResourceChange::EventUpdate, (*obj).clone());
        }
        None => {
            // Object not in store → check pending_deletes
            if let Some((_, deleted_obj)) = pending_deletes.remove(key) {
                // Found in pending_deletes → EventDelete
                let name = deleted_obj.name_any();
                let namespace = deleted_obj.namespace().unwrap_or_default();

                tracing::debug!(
                    component = "resource_controller",
                    kind = kind,
                    name = %name,
                    namespace = %namespace,
                    "Applying EventDelete from worker (object removed from store)"
                );
                (apply_fn)(config_server, ResourceChange::EventDelete, deleted_obj);
            } else {
                // Not in store and not in pending_deletes → already processed or never existed
                tracing::trace!(
                    component = "resource_controller",
                    kind = kind,
                    key = %key,
                    "Object not found in store or pending_deletes, skipping (already processed)"
                );
            }
        }
    }
}

/// Check if resource passes namespace and custom filters
fn passes_filters<K>(obj: &K, namespace_filter: &Option<Vec<String>>, filter_fn: &Option<FilterFn<K>>) -> bool
where
    K: Resource + Clone,
{
    // Namespace filter for MultipleNamespaces mode
    let ns_ok = match namespace_filter {
        Some(allowed_ns) => {
            // For namespaced resources, namespace must exist
            match obj.namespace() {
                Some(resource_ns) => allowed_ns.iter().any(|ns| ns == &resource_ns),
                None => {
                    // Namespaced resource without namespace is invalid, skip it
                    tracing::warn!(
                        name = %obj.name_any(),
                        "Namespaced resource missing namespace, skipping"
                    );
                    false
                }
            }
        }
        None => true,
    };

    // Custom filter
    let filter_ok = filter_fn.as_ref().is_none_or(|f| f(obj));

    ns_ok && filter_ok
}

/// Create a resource key from object: "namespace/name" or "name" for cluster-scoped
fn make_resource_key<K>(obj: &K) -> String
where
    K: Resource,
{
    let name = obj.name_any();
    match obj.namespace() {
        Some(ns) => format!("{}/{}", ns, name),
        None => name,
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

/// Builder for ResourceController - provides a fluent API for configuration
pub struct ResourceControllerBuilder<K>
where
    K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
{
    kind: &'static str,
    api_scope: Option<ApiScope>,
    apply_fn: Option<ApplyFn<K>>,
    filter_fn: Option<FilterFn<K>>,
    shutdown_signal: Option<ShutdownSignal>,
    relink_signal: Option<RelinkSignalSender>,
    _marker: std::marker::PhantomData<K>,
}

impl<K> ResourceControllerBuilder<K>
where
    K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
{
    /// Create a new builder
    pub fn new(kind: &'static str) -> Self {
        Self {
            kind,
            api_scope: None,
            apply_fn: None,
            filter_fn: None,
            shutdown_signal: None,
            relink_signal: None,
            _marker: std::marker::PhantomData,
        }
    }

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

    /// Set the apply function for InitAdd and runtime events
    pub fn apply_with<F>(mut self, f: F) -> Self
    where
        F: Fn(&ConfigServer, ResourceChange, K) + Send + Sync + 'static,
    {
        self.apply_fn = Some(Arc::new(f));
        self
    }

    /// Set optional filter function (e.g., for filtering by gateway class)
    pub fn filter<F>(mut self, f: F) -> Self
    where
        F: Fn(&K) -> bool + Send + Sync + 'static,
    {
        self.filter_fn = Some(Arc::new(f));
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

    /// Build the ResourceController
    pub fn build(
        self,
        client: Client,
        config_server: Arc<ConfigServer>,
        watcher_config: watcher::Config,
    ) -> ResourceController<K> {
        ResourceController::new(
            self.kind,
            client,
            config_server,
            watcher_config,
            self.api_scope.expect("API scope must be set"),
            self.apply_fn.expect("Apply function must be set"),
            self.filter_fn,
            self.shutdown_signal,
            self.relink_signal,
        )
    }
}
