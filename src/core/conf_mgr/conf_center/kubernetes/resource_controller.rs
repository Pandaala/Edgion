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
//! - Go operator-style workqueue with deduplication and retry
//! - Graceful shutdown support via ShutdownSignal

use anyhow::Result;
use futures::StreamExt;
use kube::runtime::watcher::Event;
use kube::runtime::reflector::{ObjectRef, Store};
use kube::runtime::{reflector, watcher};
use kube::{Api, Client, Resource, ResourceExt};
use serde::de::DeserializeOwned;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;
use super::metrics::{controller_metrics, InitSyncTimer};
use super::namespace::NamespaceWatchMode;
use super::shutdown::ShutdownSignal;
use super::workqueue::Workqueue;
use crate::core::conf_sync::conf_server::ConfigServer;
use crate::core::conf_sync::traits::ResourceChange;

/// Type alias for the apply function that handles InitAdd and runtime events
pub type ApplyFn<K> = Arc<dyn Fn(&ConfigServer, ResourceChange, K) + Send + Sync>;

/// Type alias for the optional filter function
pub type FilterFn<K> = Arc<dyn Fn(&K) -> bool + Send + Sync>;

/// Generic ResourceController that encapsulates the complete 1-8 flow for a single resource type
///
/// Uses a single watcher + reflector stream:
/// - Init phase: InitApply events are applied directly as InitAdd
/// - Runtime phase: Apply events are enqueued, Delete events are handled directly
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
                NamespaceWatchMode::SingleNamespace(ns) => {
                    Api::namespaced(self.client.clone(), ns)
                }
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
    /// 4. Process runtime events (WATCH phase) - enqueue Apply, handle Delete directly
    async fn run_with_api(self, api: Api<K>) -> Result<()> {
        let kind = self.kind;

        // Record controller started
        controller_metrics().controller_started();

        tracing::info!(
            component = "resource_controller",
            kind = kind,
            "Starting independent ResourceController with single watcher"
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

        // Track init phase
        let mut init_timer = Some(InitSyncTimer::start(kind));
        let mut init_count = 0;
        let mut init_done = false;

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
                            tracing::debug!(
                                component = "resource_controller",
                                kind = kind,
                                "Init phase started"
                            );
                        }
                        Event::InitApply(obj) => {
                            // Init phase - apply directly (store already updated by reflector)
                            if passes_filters(&obj, &self.namespace_filter, &self.filter_fn) {
                                (self.apply_fn)(
                                    &self.config_server,
                                    ResourceChange::InitAdd,
                                    obj,
                                );
                                init_count += 1;
                            }
                        }
                        Event::InitDone => {
                            // Init phase complete
                            let init_duration = init_timer.take()
                                .map(|t| t.complete(init_count))
                                .unwrap_or(0.0);
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

                            // Spawn worker for runtime phase
                            spawn_worker(
                                queue.clone(),
                                store.clone(),
                                self.config_server.clone(),
                                self.apply_fn.clone(),
                                self.filter_fn.clone(),
                                self.namespace_filter.clone(),
                                kind,
                                self.shutdown_signal.clone(),
                            );

                            tracing::info!(
                                component = "resource_controller",
                                kind = kind,
                                "Step 7-8: Worker started, processing runtime events"
                            );
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
                                    (self.apply_fn)(
                                        &self.config_server,
                                        ResourceChange::InitAdd,
                                        obj,
                                    );
                                    init_count += 1;
                                }
                            } else {
                                // Runtime phase - enqueue for worker
                                // Store is already updated by reflector
                                if passes_filters(&obj, &self.namespace_filter, &self.filter_fn) {
                                    let key = make_resource_key(&obj);
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
                                // Runtime phase - handle delete directly
                                // Store is already updated by reflector (object removed)
                                if passes_filters(&obj, &self.namespace_filter, &self.filter_fn) {
                                    let name = obj.name_any();
                                    let namespace = obj.namespace().unwrap_or_default();
                                    tracing::info!(
                                        component = "resource_controller",
                                        kind = kind,
                                        name = %name,
                                        namespace = %namespace,
                                        "Applying EventDelete"
                                    );
                                    (self.apply_fn)(
                                        &self.config_server,
                                        ResourceChange::EventDelete,
                                        obj,
                                    );
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

        // Record controller stopped
        controller_metrics().controller_stopped();

        tracing::warn!(
            component = "resource_controller",
            kind = kind,
            "Controller stopped"
        );
        Ok(())
    }
}

/// Spawn worker task for processing workqueue items
fn spawn_worker<K>(
    queue: Arc<Workqueue>,
    store: Store<K>,
    config_server: Arc<ConfigServer>,
    apply_fn: ApplyFn<K>,
    filter_fn: Option<FilterFn<K>>,
    namespace_filter: Option<Vec<String>>,
    kind: &'static str,
    shutdown_signal: Option<ShutdownSignal>,
)
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

        tracing::info!(
            component = "resource_controller",
            kind = kind,
            "Worker task ended"
        );
    });
}

/// Process a work item from the queue
fn process_work_item<K>(
    key: &str,
    store: &Store<K>,
    namespace_filter: &Option<Vec<String>>,
    filter_fn: &Option<FilterFn<K>>,
    config_server: &ConfigServer,
    apply_fn: &ApplyFn<K>,
    kind: &'static str,
)
where
    K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
{
    // Parse key to ObjectRef
    let obj_ref = parse_resource_key::<K>(key);

    // Get current state from store (already updated by reflector)
    match store.get(&obj_ref) {
        Some(obj) => {
            // Object exists in store -> Update
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
                "Applying EventUpdate from worker"
            );
            (apply_fn)(config_server, ResourceChange::EventUpdate, (*obj).clone());
        }
        None => {
            // Object not in store -> Already deleted
            // Delete is handled directly in the event loop
            tracing::trace!(
                component = "resource_controller",
                kind = kind,
                key = %key,
                "Object not found in store, likely already deleted"
            );
        }
    }
}

/// Check if resource passes namespace and custom filters
fn passes_filters<K>(
    obj: &K,
    namespace_filter: &Option<Vec<String>>,
    filter_fn: &Option<FilterFn<K>>,
) -> bool
where
    K: Resource + Clone,
{
    // Namespace filter for MultipleNamespaces mode
    let ns_ok = match namespace_filter {
        Some(allowed_ns) => {
            let resource_ns = obj.namespace().unwrap_or_default();
            allowed_ns.iter().any(|ns| ns == &resource_ns)
        }
        None => true,
    };

    // Custom filter
    let filter_ok = filter_fn.as_ref().map_or(true, |f| f(obj));

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
        )
    }
}
