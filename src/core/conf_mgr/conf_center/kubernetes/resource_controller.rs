//! Generic ResourceController for Kubernetes resources
//!
//! Each ResourceController runs a **completely independent** 1-8 flow:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────────┐
//! │                    ResourceController<K> Independent Flow                    │
//! ├─────────────────────────────────────────────────────────────────────────────┤
//! │  Step 1: Create Store + Reflector                                           │
//! │  Step 2: Run Reflector (background)                                         │
//! │  Step 3: Wait for this store ready (only waits for itself)                  │
//! │  Step 4: Snapshot Store                                                      │
//! │  Step 5: Apply InitAdd for each resource                                    │
//! │  Step 6: Mark cache ready (InitDone)                                        │
//! │  Step 7-8: Start Controller + Reconcile Loop (immediately, no waiting)      │
//! └─────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! Key benefits:
//! - Each resource type runs completely independently
//! - No waiting for other resources to complete initialization
//! - Fault isolation: one resource failing doesn't affect others

use anyhow::Result;
use futures::StreamExt;
use kube::runtime::controller::Action;
use kube::runtime::{reflector, watcher, Controller};
use kube::{Api, Client, Resource};
use serde::de::DeserializeOwned;
use std::fmt::Debug;
use std::future::Future;
use std::hash::Hash;
use std::sync::Arc;

use super::context::ControllerContext;
use super::error::{error_policy, ReconcileError};
use super::namespace::NamespaceWatchMode;
use super::status::StatusStore;
use crate::core::conf_sync::conf_server::ConfigServer;
use crate::core::conf_sync::traits::ResourceChange;

/// Type alias for the apply function that handles InitAdd and runtime events
pub type ApplyFn<K> = Box<dyn Fn(&ConfigServer, ResourceChange, K) + Send + Sync>;

/// Type alias for the optional filter function
pub type FilterFn<K> = Box<dyn Fn(&K) -> bool + Send + Sync>;

/// Generic ResourceController that encapsulates the complete 1-8 flow for a single resource type
pub struct ResourceController<K, ReconcileFn, ReconcileFut>
where
    K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
    ReconcileFn: FnMut(Arc<K>, Arc<ControllerContext>) -> ReconcileFut + Send + 'static,
    ReconcileFut: Future<Output = Result<Action, ReconcileError>> + Send + 'static,
{
    kind: &'static str,
    client: Client,
    config_server: Arc<ConfigServer>,
    status_store: Arc<dyn StatusStore>,
    gateway_class_name: String,
    watcher_config: watcher::Config,

    // API creation based on scope
    api_scope: ApiScope,

    // Difference handling via closures
    apply_fn: ApplyFn<K>,
    filter_fn: Option<FilterFn<K>>,
    reconcile_fn: ReconcileFn,
}

/// API scope for resource (namespaced or cluster-scoped)
#[derive(Clone)]
pub enum ApiScope {
    /// Namespaced resource with watch mode
    Namespaced(NamespaceWatchMode),
    /// Cluster-scoped resource
    ClusterScoped,
}

impl<K, ReconcileFn, ReconcileFut> ResourceController<K, ReconcileFn, ReconcileFut>
where
    K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + 'static,
    K::DynamicType: Default + Eq + Hash + Clone + Debug + Unpin,
    ReconcileFn: FnMut(Arc<K>, Arc<ControllerContext>) -> ReconcileFut + Send + 'static,
    ReconcileFut: Future<Output = Result<Action, ReconcileError>> + Send + 'static,
{
    /// Create a new ResourceController
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kind: &'static str,
        client: Client,
        config_server: Arc<ConfigServer>,
        status_store: Arc<dyn StatusStore>,
        gateway_class_name: String,
        watcher_config: watcher::Config,
        api_scope: ApiScope,
        apply_fn: ApplyFn<K>,
        filter_fn: Option<FilterFn<K>>,
        reconcile_fn: ReconcileFn,
    ) -> Self {
        Self {
            kind,
            client,
            config_server,
            status_store,
            gateway_class_name,
            watcher_config,
            api_scope,
            apply_fn,
            filter_fn,
            reconcile_fn,
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
            ApiScope::ClusterScoped => Api::all(self.client.clone()),
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
    async fn run_with_api(self, api: Api<K>) -> Result<()> {
        let kind = self.kind;

        tracing::info!(
            component = "resource_controller",
            kind = kind,
            "Starting independent ResourceController"
        );

        // ==================== Initialization Phase (Steps 1-6) ====================

        // Step 1: Create Store + Reflector
        let (store, writer) = reflector::store();
        tracing::debug!(
            component = "resource_controller",
            kind = kind,
            "Step 1: Created reflector store"
        );

        // Step 2: Run Reflector (background task - performs LIST then WATCH)
        let watcher_stream = watcher(api.clone(), self.watcher_config.clone());
        tokio::spawn(
            reflector(writer, watcher_stream).for_each(|_| futures::future::ready(())),
        );
        tracing::debug!(
            component = "resource_controller",
            kind = kind,
            "Step 2: Started reflector in background"
        );

        // Step 3: Wait for this store to be ready (only waits for itself, not other resources)
        store
            .wait_until_ready()
            .await
            .map_err(|e| anyhow::anyhow!("{} store error: {}", kind, e))?;
        tracing::info!(
            component = "resource_controller",
            kind = kind,
            "Step 3: Store ready (initial LIST complete)"
        );

        // Step 4: Snapshot Store
        let snapshot = store.state();
        let total_count = snapshot.len();
        tracing::debug!(
            component = "resource_controller",
            kind = kind,
            count = total_count,
            "Step 4: Snapshot taken"
        );

        // Step 5: Apply InitAdd for each resource in snapshot
        let mut applied_count = 0;
        for resource in snapshot {
            // Apply filter if present
            let should_apply = self.filter_fn.as_ref().map_or(true, |f| f(&resource));

            if should_apply {
                (self.apply_fn)(
                    &self.config_server,
                    ResourceChange::InitAdd,
                    (*resource).clone(),
                );
                applied_count += 1;
            }
        }
        tracing::info!(
            component = "resource_controller",
            kind = kind,
            total = total_count,
            applied = applied_count,
            "Step 5: InitAdd applied"
        );

        // Step 6: Mark cache ready (triggers InitDone -> cache.set_ready())
        self.config_server.set_cache_ready_by_kind(kind);
        tracing::info!(
            component = "resource_controller",
            kind = kind,
            "Step 6: Cache marked ready, starting controller immediately"
        );

        // ==================== Runtime Phase (Steps 7-8) ====================
        // Starts immediately after init - no waiting for other resources

        // Step 7-8: Start Controller + Reconcile Loop
        let ctx = Arc::new(ControllerContext {
            config_server: self.config_server.clone(),
            status_store: self.status_store.clone(),
            gateway_class_name: self.gateway_class_name.clone(),
        });

        Controller::new(api, self.watcher_config.clone())
            .run(self.reconcile_fn, error_policy, ctx)
            .for_each(|res| async move {
                match res {
                    Ok((obj, _action)) => {
                        tracing::trace!(obj = ?obj, kind = kind, "Reconciled")
                    }
                    Err(e) => {
                        tracing::error!(error = %e, kind = kind, "Controller error")
                    }
                }
            })
            .await;

        tracing::warn!(
            component = "resource_controller",
            kind = kind,
            "Controller stopped"
        );
        Ok(())
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
        self.apply_fn = Some(Box::new(f));
        self
    }

    /// Set optional filter function (e.g., for filtering by gateway class)
    pub fn filter<F>(mut self, f: F) -> Self
    where
        F: Fn(&K) -> bool + Send + Sync + 'static,
    {
        self.filter_fn = Some(Box::new(f));
        self
    }

    /// Build the ResourceController with a reconcile function
    pub fn build<ReconcileFn, ReconcileFut>(
        self,
        client: Client,
        config_server: Arc<ConfigServer>,
        status_store: Arc<dyn StatusStore>,
        gateway_class_name: String,
        watcher_config: watcher::Config,
        reconcile_fn: ReconcileFn,
    ) -> ResourceController<K, ReconcileFn, ReconcileFut>
    where
        ReconcileFn: FnMut(Arc<K>, Arc<ControllerContext>) -> ReconcileFut + Send + 'static,
        ReconcileFut: Future<Output = Result<Action, ReconcileError>> + Send + 'static,
    {
        ResourceController::new(
            self.kind,
            client,
            config_server,
            status_store,
            gateway_class_name,
            watcher_config,
            self.api_scope.expect("API scope must be set"),
            self.apply_fn.expect("Apply function must be set"),
            self.filter_fn,
            reconcile_fn,
        )
    }
}
