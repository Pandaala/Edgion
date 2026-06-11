//! ResourceProcessor - Core processor implementation
//!
//! ResourceProcessor<T> holds:
//! - ServerCache<T> for resource storage
//! - Workqueue for event processing
//! - ProcessorHandler<T> for resource-specific logic
//! - Configuration (metadata filter, namespace filter, etc.)

use parking_lot::RwLock;
use std::any::Any;
use std::fmt::Debug;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use kube::Resource;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tracing::Instrument;

/// Result of processing a work item
///
/// Used by workers to determine if status needs to be persisted
#[derive(Debug)]
pub enum WorkItemResult<K> {
    /// Resource was processed successfully
    Processed {
        /// The processed object (with updated status)
        obj: K,
        /// Whether status changed and needs persistence
        status_changed: bool,
    },
    /// Resource was deleted
    Deleted {
        /// The key of the deleted resource
        key: String,
    },
    /// Nothing to do (already processed or filtered)
    Skipped,
}

use crate::core::common::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use crate::core::controller::conf_mgr::conf_center::MetadataFilterConfig;
use crate::core::controller::conf_mgr::ControllerRuntime;
use crate::core::controller::conf_sync::conf_server::WatchObj;
use crate::core::controller::conf_sync::ServerCache;
use edgion_resources::ResourceMeta;

use super::context::HandlerContext;
use super::handler::{ProcessResult, ProcessorHandler};
use super::make_resource_key;
use super::raw_store::RawStore;
use crate::core::controller::conf_mgr::sync_runtime::workqueue::{TriggerChain, Workqueue, WorkqueueConfig};

/// Result of status extraction
#[derive(Debug, PartialEq)]
enum StatusExtractResult {
    /// Status field present with serialized value
    Present(String),
    /// Status field is null or empty
    Empty,
    /// Serialization failed
    SerializationError,
}

impl StatusExtractResult {
    /// Short tag for debug logging that avoids printing the full status JSON
    /// (which may contain resource names in condition messages).
    fn digest_for_log(&self) -> String {
        match self {
            StatusExtractResult::Present(s) => format!("present({} bytes)", s.len()),
            StatusExtractResult::Empty => "empty".to_string(),
            StatusExtractResult::SerializationError => "error".to_string(),
        }
    }
}

/// Extract status field from a resource as JSON string for comparison
///
/// Returns the extraction result to distinguish between:
/// - Status field present with value
/// - Status field is null/empty
/// - Serialization error (should be logged)
fn extract_status_json<T: Serialize>(obj: &T) -> StatusExtractResult {
    match serde_json::to_value(obj) {
        Ok(value) => match value.get("status") {
            Some(status) if !status.is_null() => match serde_json::to_string(status) {
                Ok(s) => StatusExtractResult::Present(s),
                Err(_) => StatusExtractResult::SerializationError,
            },
            _ => StatusExtractResult::Empty,
        },
        Err(_) => StatusExtractResult::SerializationError,
    }
}

/// Check if status has changed using semantic comparison.
///
/// Uses `status_diff::status_semantically_changed` to ignore non-semantic
/// fields like `lastTransitionTime`, avoiding unnecessary K8s API writes.
fn status_has_changed(kind: &str, old: &StatusExtractResult, new: &StatusExtractResult) -> bool {
    match (old, new) {
        (StatusExtractResult::Present(old_val), StatusExtractResult::Present(new_val)) => {
            super::status_diff::status_semantically_changed(kind, old_val, new_val)
        }
        (StatusExtractResult::Empty, StatusExtractResult::Empty) => false,
        (StatusExtractResult::SerializationError, _) | (_, StatusExtractResult::SerializationError) => true,
        _ => true,
    }
}

/// Extract status field from a resource as serde_json::Value
///
/// Returns the status as a JSON Value, or None if not present.
/// This is used for persisting status to FileSystem (.status files).
pub fn extract_status_value<T: Serialize>(obj: &T) -> Option<serde_json::Value> {
    match serde_json::to_value(obj) {
        Ok(value) => value.get("status").cloned().filter(|s| !s.is_null()),
        Err(e) => {
            tracing::warn!(
                component = "resource_processor",
                error = %e,
                "Failed to serialize object for status extraction"
            );
            None
        }
    }
}

/// Object-safe trait for processor management
///
/// This trait allows storing different ResourceProcessor<T> types in a HashMap
/// and provides common operations without knowing the concrete type.
#[async_trait::async_trait]
pub trait ProcessorObj: Send + Sync {
    /// Get the resource kind name
    fn kind(&self) -> &'static str;

    /// Get WatchObj for ConfigSyncServer registration
    fn as_watch_obj(&self) -> Arc<dyn WatchObj>;

    /// Look up a single resource by key from the raw store (bypasses handler filter).
    ///
    /// Key format: `"namespace/name"` for namespaced resources, `"name"` for
    /// cluster-scoped resources.
    ///
    /// Returns the resource as a type-erased `Arc<dyn Any + Send + Sync>`, or `None` if:
    /// - The raw store has not been injected yet (before controller init), or
    /// - The resource does not exist in the raw store.
    ///
    /// Callers should downcast to the concrete type, e.g.:
    /// ```ignore
    /// let arc: Arc<Gateway> = processor.get_raw_any(&key)?.downcast().ok()?;
    /// ```
    fn get_raw_any(&self, key: &str) -> Option<Arc<dyn Any + Send + Sync>>;

    /// Enqueue a key for immediate processing (original events, init revalidation)
    async fn requeue(&self, key: String);

    /// Cross-resource requeue with trigger chain (goes through delay subsystem)
    async fn requeue_with_chain(&self, key: String, chain: TriggerChain);

    /// Check if cache is ready
    fn is_ready(&self) -> bool;

    /// Set cache to ready state
    fn set_ready(&self);

    /// Set cache to not ready state
    fn set_not_ready(&self);

    /// Clear cache data
    fn clear(&self);

    /// Set namespace filter for this processor
    fn set_namespace_filter_vec(&self, filter: Option<Vec<String>>);

    /// Set metadata filter for this processor
    fn set_metadata_filter_config(&self, filter: Option<MetadataFilterConfig>);

    /// Get the workqueue for this processor (for RequeueRegistry registration)
    fn workqueue(&self) -> Arc<Workqueue>;

    /// List all resource keys in cache
    fn list_keys(&self) -> Vec<String>;

    /// Number of resources currently held in cache. Must be O(1).
    fn store_size(&self) -> usize;

    /// Returns true (and clears the flag) if the processor's resource count has
    /// changed since the last call. Used by fed_sync stats coalescing to decide
    /// whether to push a fresh StatsReport.
    fn take_stats_dirty(&self) -> bool;

    /// Check if a resource exists in cache by key
    fn contains_key(&self, key: &str) -> bool;

    /// Requeue all keys in cache (used for leader status reconciliation)
    async fn requeue_all_keys(&self) -> usize;
}

/// Enhanced ResourceProcessor that holds ServerCache<T>
///
/// This processor manages the complete lifecycle of a resource type:
/// - Receives events from K8s watcher (via on_init_apply, on_apply, on_delete)
/// - Processes events through handler pipeline
/// - Stores results in ServerCache
/// - Provides WatchObj for gRPC streaming
pub struct ResourceProcessor<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
{
    /// Resource kind name
    kind: &'static str,

    /// Resource cache (previously in ConfigServer)
    cache: Arc<ServerCache<K>>,

    /// Work queue for runtime event processing
    workqueue: Arc<Workqueue>,

    /// Processing handler (resource-specific logic)
    handler: Arc<dyn ProcessorHandler<K>>,

    /// Metadata filter configuration
    metadata_filter: RwLock<Option<Arc<MetadataFilterConfig>>>,

    /// Namespace filter
    namespace_filter: RwLock<Option<Arc<Vec<String>>>>,

    /// Raw data source for cross-resource key lookups (bypasses handler filter).
    /// K8s mode: injected as KubeRawStore after reflector::store() call.
    /// FS/Etcd mode: injected as LocalRawStore on controller construction.
    raw_store: RwLock<Option<Arc<dyn RawStore<K>>>>,

    /// Shared controller runtime (stores, registries, etc.)
    runtime: Arc<ControllerRuntime>,

    /// Set whenever cache content changes; consumed by fed_sync stats push.
    /// Initialized to `true` so the first stats coalescing tick after
    /// fed_client startup emits an initial report.
    stats_dirty: Arc<AtomicBool>,
}

impl<K> ResourceProcessor<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
{
    /// Create a new ResourceProcessor
    pub fn new(
        kind: &'static str,
        capacity: usize,
        handler: Arc<dyn ProcessorHandler<K>>,
        runtime: Arc<ControllerRuntime>,
    ) -> Self {
        let cache = Arc::new(ServerCache::new(capacity as u32));
        let workqueue = Arc::new(Workqueue::with_defaults(kind));

        tracing::info!(
            component = "resource_processor",
            kind = kind,
            capacity = capacity,
            "Creating ResourceProcessor"
        );

        Self {
            kind,
            cache,
            workqueue,
            handler,
            metadata_filter: RwLock::new(None),
            namespace_filter: RwLock::new(None),
            raw_store: RwLock::new(None),
            runtime,
            stats_dirty: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Create with custom workqueue config
    pub fn with_workqueue_config(
        kind: &'static str,
        capacity: usize,
        handler: Arc<dyn ProcessorHandler<K>>,
        workqueue_config: WorkqueueConfig,
        runtime: Arc<ControllerRuntime>,
    ) -> Self {
        let cache = Arc::new(ServerCache::new(capacity as u32));
        let workqueue = Arc::new(Workqueue::new(kind, workqueue_config));

        Self {
            kind,
            cache,
            workqueue,
            handler,
            metadata_filter: RwLock::new(None),
            namespace_filter: RwLock::new(None),
            raw_store: RwLock::new(None),
            runtime,
            stats_dirty: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Inject the raw store backend.
    ///
    /// Called once during controller startup:
    /// - K8s mode: immediately after `reflector::store()`, before any events arrive.
    /// - FS mode: in `FileSystemResourceController::new()`.
    pub fn set_raw_store(&self, store: Arc<dyn RawStore<K>>) {
        *self.raw_store.write() = Some(store);
    }

    /// Set metadata filter configuration
    pub fn set_metadata_filter(&self, filter: MetadataFilterConfig) {
        *self.metadata_filter.write() = Some(Arc::new(filter));
    }

    /// Set namespace filter
    pub fn set_namespace_filter(&self, namespaces: Vec<String>) {
        *self.namespace_filter.write() = Some(Arc::new(namespaces));
    }

    /// Get workqueue reference (for external use like worker spawning)
    pub fn workqueue(&self) -> Arc<Workqueue> {
        self.workqueue.clone()
    }

    /// Get cache reference
    pub fn cache(&self) -> Arc<ServerCache<K>> {
        self.cache.clone()
    }

    // ==================== Lifecycle Methods ====================

    /// Handle Init event (LIST started)
    pub fn on_init(&self) {
        tracing::info!(component = "resource_processor", kind = self.kind, "Init started");
        self.cache.set_not_ready();
    }

    /// Handle InitApply event (process single resource from LIST)
    ///
    /// Called directly during init phase (not via workqueue).
    ///
    /// # Arguments
    /// * `obj` - Object from config center
    /// * `existing_status_json` - Existing status from config center
    ///   - K8s mode: pass None (status is already in obj from K8s API)
    ///   - FileSystem mode: pass content of .status file as JSON string
    ///
    /// Returns WorkItemResult for status persistence handling by caller.
    pub async fn on_init_apply(&self, obj: K, existing_status_json: Option<String>) -> WorkItemResult<K> {
        // Reconstruct the resource_version floor from persisted rv before init proceeds.
        let rv = edgion_resources::resource::meta::extract_version(obj.meta());
        crate::core::common::utils::bump_resource_version_floor(rv);

        let ctx = self.create_context(TriggerChain::default());
        self.process_resource(obj, &ctx, true, existing_status_json).await
    }

    /// Handle InitDone event (LIST completed)
    pub fn on_init_done(&self) {
        let ctx = self.create_context(TriggerChain::default());
        self.handler.on_init_done(&ctx);

        WatchObj::set_ready(self.cache.as_ref());
        tracing::info!(
            component = "resource_processor",
            kind = self.kind,
            "Init done, cache ready"
        );
    }

    /// Handle Apply event (enqueue for runtime processing)
    pub async fn on_apply(&self, obj: &K) {
        let key = make_resource_key(obj);
        self.workqueue.enqueue(key).await;
    }

    /// Handle Delete event (enqueue for runtime processing)
    pub async fn on_delete(&self, obj: &K) {
        let key = make_resource_key(obj);
        self.workqueue.enqueue(key).await;
    }

    // ==================== Cache Operations ====================

    /// Get resource by key
    pub fn get(&self, key: &str) -> Option<K> {
        self.cache.get_by_key(key)
    }

    /// List all resources
    pub fn list(&self) -> Vec<K> {
        self.cache.list_owned().data
    }

    /// Save resource to cache with an optional pre-assigned sync_version
    pub fn save(&self, obj: K, sv: Option<u64>) {
        self.cache.apply_change(ResourceChange::EventUpdate, obj, sv);
        self.stats_dirty.store(true, Ordering::Relaxed);
    }

    /// Remove resource from cache by key with an optional pre-assigned sync_version
    pub fn remove(&self, key: &str, sv: Option<u64>) {
        // Get the cached object first to properly delete
        if let Some(cached) = self.cache.get_by_key(key) {
            self.cache.apply_change(ResourceChange::EventDelete, cached, sv);
            self.stats_dirty.store(true, Ordering::Relaxed);
        }
    }

    // ==================== Worker Processing ====================

    /// Process a single work item (called by worker loop)
    ///
    /// This compares the store state (from K8s/FileSystem) with cache state
    /// and determines whether to update or delete.
    ///
    /// # Arguments
    /// * `key` - Resource key (namespace/name or just name)
    /// * `store_obj` - Object from config center (K8s store or file)
    /// * `existing_status_json` - Existing status from config center (for FileSystem: from .status file)
    ///   - K8s mode: pass None (status is already in store_obj)
    ///   - FileSystem mode: pass content of .status file as JSON string
    /// * `trigger_chain` - Cascade trigger chain from the WorkItem
    ///
    /// Returns `WorkItemResult` indicating what action was taken and whether
    /// status needs to be persisted.
    pub async fn process_work_item(
        &self,
        key: &str,
        store_obj: Option<K>,
        existing_status_json: Option<String>,
        trigger_chain: TriggerChain,
    ) -> WorkItemResult<K> {
        let extended_chain = trigger_chain.extend(self.kind, key);
        let ctx = self.create_context(extended_chain);
        let cache_obj = self.get(key);

        match (store_obj, cache_obj) {
            (Some(obj), _) => {
                // Object exists in store -> process it
                self.process_resource(obj, &ctx, false, existing_status_json).await
            }
            (None, Some(cached)) => {
                // Object deleted from store but exists in cache -> delete
                self.process_delete(&cached, &ctx).await;
                WorkItemResult::Deleted { key: key.to_string() }
            }
            (None, None) => {
                // Resource absent from both store and cache: already deleted and cleaned up.
                tracing::trace!(
                    kind = self.kind,
                    key = key,
                    "Resource not found in store or cache, skipping"
                );
                WorkItemResult::Skipped
            }
        }
    }

    // ==================== Internal Methods ====================

    /// Create handler context with trigger chain for cascade tracking
    fn create_context(&self, trigger_chain: TriggerChain) -> HandlerContext {
        HandlerContext::new(
            self.metadata_filter.read().clone(),
            self.namespace_filter.read().clone(),
            trigger_chain,
            self.workqueue.config().max_trigger_cycles,
            self.workqueue.config().max_trigger_depth,
            self.runtime.clone(),
        )
    }

    /// Process a resource through the handler pipeline
    ///
    /// # Arguments
    /// * `obj` - Object to process
    /// * `ctx` - Handler context
    /// * `is_init` - Whether this is during init phase
    /// * `existing_status_json` - Existing status from config center as JSON string
    ///   - K8s mode: None (status is in obj)
    ///   - FileSystem mode: content of .status file
    async fn process_resource(
        &self,
        obj: K,
        ctx: &HandlerContext,
        is_init: bool,
        existing_status_json: Option<String>,
    ) -> WorkItemResult<K> {
        // Extract name/namespace/version early for logging (owned strings to avoid borrow issues)
        let name = obj.meta().name.clone().unwrap_or_default();
        let namespace = obj.meta().namespace.clone().unwrap_or_default();
        let rv = edgion_resources::resource::meta::extract_version(obj.meta());

        // Generate sv early so the entire processing span carries it for telemetry correlation
        let sv = crate::core::common::utils::next_resource_version();
        let span = tracing::info_span!(
            "process_resource",
            component = "resource_processor",
            kind = self.kind,
            name = %name,
            ns = %namespace,
            rv,
            sv,
        );
        self.process_resource_inner(obj, ctx, is_init, existing_status_json, sv)
            .instrument(span)
            .await
    }

    async fn process_resource_inner(
        &self,
        obj: K,
        ctx: &HandlerContext,
        is_init: bool,
        existing_status_json: Option<String>,
        sv: u64,
    ) -> WorkItemResult<K> {
        let namespace = obj.meta().namespace.clone().unwrap_or_default();

        // 1. Check namespace filter
        if let Some(allowed_ns) = ctx.namespace_filter() {
            if !namespace.is_empty() && !allowed_ns.iter().any(|n| n == &namespace) {
                tracing::trace!("Skipped by namespace filter");
                return WorkItemResult::Skipped;
            }
        }

        // 2. Check handler filter
        if !self.handler.filter(&obj) {
            tracing::trace!("Skipped by handler filter");
            return WorkItemResult::Skipped;
        }

        // 3. Clean metadata
        // First apply context's metadata filter (removes managedFields, annotations, etc.)
        // Then let handler do any additional custom cleaning
        let mut obj = obj;
        ctx.clean_metadata(&mut obj);
        self.handler.clean_metadata(&mut obj, ctx);

        // 4. Validate (log warnings but continue)
        let mut all_errors = self.handler.validate(&obj, ctx);
        for error in &all_errors {
            tracing::warn!(error = %error, "Resource validation error");
        }

        // 5. Preparse (build runtime structures, validate configs)
        let preparse_errors = self.handler.preparse(&mut obj, ctx);
        for error in &preparse_errors {
            tracing::warn!(error = %error, "Resource preparse error");
        }
        all_errors.extend(preparse_errors);

        // 6. Parse/preprocess
        match self.handler.parse(obj, ctx).await {
            ProcessResult::Continue(mut parsed_obj) => {
                // 7. Determine old status for comparison
                // - If existing_status_json provided (FileSystem mode): use it
                // - Otherwise: extract from object (K8s mode - status is already in obj)
                let old_status = match existing_status_json {
                    Some(json) => StatusExtractResult::Present(json),
                    None => extract_status_json(&parsed_obj),
                };

                // Log serialization errors
                if matches!(old_status, StatusExtractResult::SerializationError) {
                    tracing::warn!("Failed to serialize old status for comparison");
                }

                tracing::debug!(
                    kind = self.kind,
                    old_status_present = matches!(old_status, StatusExtractResult::Present(_)),
                    "Status comparison: old_status captured before update_status()"
                );

                // 8. Update status (handler sets Gateway API conditions)
                self.handler.update_status(&mut parsed_obj, ctx, &all_errors);

                // 9. Check if status changed
                let new_status = extract_status_json(&parsed_obj);

                // Log serialization errors
                if matches!(new_status, StatusExtractResult::SerializationError) {
                    tracing::warn!("Failed to serialize new status for comparison");
                }

                let status_changed = status_has_changed(self.kind, &old_status, &new_status);

                if status_changed {
                    tracing::debug!(
                        kind = self.kind,
                        old = %old_status.digest_for_log(),
                        new = %new_status.digest_for_log(),
                        "Status semantically changed, will persist"
                    );
                } else {
                    if matches!(
                        (&old_status, &new_status),
                        (StatusExtractResult::Present(_), StatusExtractResult::Present(_))
                    ) {
                        crate::core::controller::conf_mgr::sync_runtime::record_status_write_skipped(self.kind);
                    }
                    tracing::debug!(
                        kind = self.kind,
                        old = %old_status.digest_for_log(),
                        new = %new_status.digest_for_log(),
                        "Status unchanged by semantic diff, skipping persist"
                    );
                }

                // 10. Call on_change
                //
                // Called in both init and runtime phases. During init, requeued
                // items are buffered by the workqueue and processed after init
                // completes, so cross-resource requeue (e.g., Gateway port
                // changes triggering TLSRoute requeue) works correctly even
                // when resources arrive out of order during the initial LIST.
                //
                // NOTE: on_change is intentionally called BEFORE save-to-cache (Step 11).
                // Handlers that perform cross-resource lookups inside on_change (e.g.
                // lookup_gateway) MUST use RawStore (via get_raw_any) rather than
                // ServerCache, because the current resource has not yet been written
                // to ServerCache at this point.
                self.handler.on_change(&parsed_obj, ctx).await;

                let phase = if is_init { "init" } else { "runtime" };
                tracing::debug!(
                    phase = phase,
                    status_changed = status_changed,
                    "Resource processed and saving"
                );

                // 11. Save to cache
                // Use InitAdd during init phase (synchronous), EventUpdate at runtime (async)
                let obj_for_result = parsed_obj.clone();
                if is_init {
                    self.cache.apply_change(ResourceChange::InitAdd, parsed_obj, Some(sv));
                    self.stats_dirty.store(true, Ordering::Relaxed);
                } else {
                    self.save(parsed_obj, Some(sv));
                }

                WorkItemResult::Processed {
                    obj: obj_for_result,
                    status_changed,
                }
            }
            ProcessResult::Skip { reason } => {
                tracing::debug!(reason = %reason, "Resource skipped after parse");
                WorkItemResult::Skipped
            }
        }
    }

    /// Process resource deletion
    async fn process_delete(&self, cached_obj: &K, ctx: &HandlerContext) {
        let key = make_resource_key(cached_obj);
        let sv = crate::core::common::utils::next_resource_version();
        let span = tracing::info_span!(
            "process_delete",
            component = "resource_processor",
            kind = self.kind,
            key = %key,
            sv,
        );

        async {
            // 1. Execute handler's delete cleanup
            self.handler.on_delete(cached_obj, ctx).await;

            // 2. Remove from cache
            self.remove(&key, Some(sv));

            tracing::debug!("Resource deleted from cache");
        }
        .instrument(span)
        .await
    }
}

// Implement ProcessorObj for ResourceProcessor<K>
#[async_trait::async_trait]
impl<K> ProcessorObj for ResourceProcessor<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
{
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn as_watch_obj(&self) -> Arc<dyn WatchObj> {
        self.cache.clone()
    }

    async fn requeue(&self, key: String) {
        self.workqueue.enqueue(key).await;
    }

    async fn requeue_with_chain(&self, key: String, chain: TriggerChain) {
        let delay = self.workqueue.config().default_requeue_delay;
        self.workqueue.enqueue_after(key, delay, chain).await;
    }

    fn is_ready(&self) -> bool {
        self.cache.is_ready()
    }

    fn set_ready(&self) {
        WatchObj::set_ready(self.cache.as_ref());
        self.runtime.processor_registry.notify_ready();
    }

    fn set_not_ready(&self) {
        WatchObj::set_not_ready(self.cache.as_ref());
    }

    fn clear(&self) {
        WatchObj::clear(self.cache.as_ref());
        self.stats_dirty.store(true, Ordering::Relaxed);
    }

    fn set_namespace_filter_vec(&self, filter: Option<Vec<String>>) {
        *self.namespace_filter.write() = filter.map(Arc::new);
    }

    fn set_metadata_filter_config(&self, filter: Option<MetadataFilterConfig>) {
        *self.metadata_filter.write() = filter.map(Arc::new);
    }

    fn workqueue(&self) -> Arc<Workqueue> {
        self.workqueue.clone()
    }

    fn list_keys(&self) -> Vec<String> {
        self.cache.list_keys()
    }

    fn store_size(&self) -> usize {
        self.cache.len()
    }

    fn take_stats_dirty(&self) -> bool {
        // swap-style: returns previous value while resetting to false.
        self.stats_dirty.swap(false, Ordering::Relaxed)
    }

    fn contains_key(&self, key: &str) -> bool {
        self.cache.get_by_key(key).is_some()
    }

    async fn requeue_all_keys(&self) -> usize {
        let keys = self.cache.list_keys();
        let count = keys.len();
        for key in keys {
            self.workqueue.enqueue(key).await;
        }
        count
    }

    fn get_raw_any(&self, key: &str) -> Option<Arc<dyn Any + Send + Sync>> {
        let raw_store = self.raw_store.read();
        let arc_k = raw_store.as_ref()?.get(key)?;
        Some(arc_k as Arc<dyn Any + Send + Sync>)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kube::api::ObjectMeta;
    use serde::{Deserialize, Serialize};

    struct TestHandler;

    #[async_trait::async_trait]
    impl<K> ProcessorHandler<K> for TestHandler where K: kube::Resource + Clone + Send + Sync + 'static {}

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TestResource {
        metadata: ObjectMeta,
        spec: String,
    }

    impl ResourceMeta for TestResource {
        fn get_version(&self) -> u64 {
            0
        }
        fn get_sync_version(&self) -> u64 {
            0
        }
        fn resource_kind() -> edgion_resources::ResourceKind {
            edgion_resources::ResourceKind::Unspecified
        }
        fn kind_name() -> &'static str {
            "TestResource"
        }
        fn key_name(&self) -> String {
            make_resource_key(self)
        }
    }

    impl kube::Resource for TestResource {
        type DynamicType = ();
        type Scope = kube::core::ClusterResourceScope;

        fn kind(_: &Self::DynamicType) -> std::borrow::Cow<'static, str> {
            "TestResource".into()
        }
        fn group(_: &Self::DynamicType) -> std::borrow::Cow<'static, str> {
            "test.example.com".into()
        }
        fn version(_: &Self::DynamicType) -> std::borrow::Cow<'static, str> {
            "v1".into()
        }
        fn plural(_: &Self::DynamicType) -> std::borrow::Cow<'static, str> {
            "testresources".into()
        }
        fn meta(&self) -> &ObjectMeta {
            &self.metadata
        }
        fn meta_mut(&mut self) -> &mut ObjectMeta {
            &mut self.metadata
        }
    }

    #[tokio::test]
    async fn test_processor_basic() {
        let handler = Arc::new(TestHandler);

        let processor = ResourceProcessor::<TestResource>::new(
            "TestResource",
            100,
            handler,
            Arc::new(ControllerRuntime::default()),
        );

        assert_eq!(processor.kind(), "TestResource");
        assert!(!processor.is_ready());

        processor.set_ready();
        assert!(processor.is_ready());
    }

    #[tokio::test]
    async fn test_processor_init_apply() {
        let handler = Arc::new(TestHandler);

        let processor = ResourceProcessor::<TestResource>::new(
            "TestResource",
            100,
            handler,
            Arc::new(ControllerRuntime::default()),
        );

        let resource = TestResource {
            metadata: ObjectMeta {
                name: Some("test".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: "test spec".to_string(),
        };

        processor.on_init();
        assert!(!processor.is_ready());

        let result = processor.on_init_apply(resource.clone(), None).await;
        assert!(matches!(result, WorkItemResult::Processed { .. }));

        processor.on_init_done();
        assert!(processor.is_ready());

        // Check resource is in cache
        let cached = processor.get("default/test");
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().spec, "test spec");
    }

    #[tokio::test]
    async fn test_processor_init_apply_bumps_resource_version_floor() {
        use crate::core::common::utils::next_resource_version;

        let handler = Arc::new(TestHandler);
        let processor = ResourceProcessor::<TestResource>::new(
            "TestResource",
            100,
            handler,
            Arc::new(ControllerRuntime::default()),
        );

        // Pick an rv that is guaranteed higher than the current process-wide floor.
        let baseline = next_resource_version();
        let high_rv = baseline + 1_000_000;

        let resource = TestResource {
            metadata: ObjectMeta {
                name: Some("test".to_string()),
                namespace: Some("default".to_string()),
                resource_version: Some(high_rv.to_string()),
                ..Default::default()
            },
            spec: "test spec".to_string(),
        };

        processor.on_init();
        let result = processor.on_init_apply(resource, None).await;
        assert!(matches!(result, WorkItemResult::Processed { .. }));

        let after = next_resource_version();
        assert!(
            after > high_rv,
            "on_init_apply should bump the floor above the resource's rv (after={}, high_rv={})",
            after,
            high_rv
        );
    }

    #[test]
    fn test_gateway_class_status_change_detection() {
        use edgion_resources::prelude_resources::GatewayClass;
        use edgion_resources::resources::common::Condition;
        use edgion_resources::resources::gateway_class::GatewayClassStatus;

        let gc = GatewayClass {
            metadata: ObjectMeta {
                name: Some("test-gc".to_string()),
                generation: Some(1),
                ..Default::default()
            },
            spec: edgion_resources::resources::gateway_class::GatewayClassSpec {
                controller_name: "example.com/controller".to_string(),
                description: None,
                parameters_ref: None,
            },
            status: Some(GatewayClassStatus {
                conditions: vec![Condition {
                    type_: "Accepted".to_string(),
                    status: "Unknown".to_string(),
                    reason: "Pending".to_string(),
                    message: "Waiting for controller".to_string(),
                    last_transition_time: "1970-01-01T00:00:00Z".to_string(),
                    observed_generation: None,
                }],
                supported_features: None,
            }),
        };

        let old_status = extract_status_json(&gc);

        let mut gc_modified = gc.clone();
        let status = gc_modified.status.get_or_insert_with(GatewayClassStatus::default);
        crate::core::controller::conf_mgr::sync_runtime::resource_processor::update_condition(
            &mut status.conditions,
            crate::core::controller::conf_mgr::sync_runtime::resource_processor::accepted_condition(
                gc_modified.metadata.generation,
            ),
        );
        crate::core::controller::conf_mgr::sync_runtime::resource_processor::update_condition(
            &mut status.conditions,
            crate::core::controller::conf_mgr::sync_runtime::resource_processor::condition_true(
                "SupportedVersion",
                "SupportedVersion",
                "Gateway API version is supported",
                gc_modified.metadata.generation,
            ),
        );

        let new_status = extract_status_json(&gc_modified);

        let changed = status_has_changed("GatewayClass", &old_status, &new_status);
        assert!(changed, "Status should have changed after update_status");
    }
}
