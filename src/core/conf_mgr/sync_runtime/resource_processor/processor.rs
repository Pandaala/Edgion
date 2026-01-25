//! ResourceProcessor - Core processor implementation
//!
//! ResourceProcessor<T> holds:
//! - ServerCache<T> for resource storage
//! - Workqueue for event processing
//! - ProcessorHandler<T> for resource-specific logic
//! - Configuration (metadata filter, namespace filter, etc.)

use std::fmt::Debug;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use kube::Resource;
use serde::de::DeserializeOwned;
use serde::Serialize;

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

use crate::core::conf_mgr::conf_center::MetadataFilterConfig;
use crate::core::conf_sync::conf_server::WatchObj;
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use crate::core::conf_sync::ServerCache;
use crate::types::ResourceMeta;

use super::context::HandlerContext;
use super::handler::{ProcessResult, ProcessorHandler};
use super::{make_resource_key, SecretRefManager};
use crate::core::conf_mgr::sync_runtime::workqueue::{Workqueue, WorkqueueConfig};

/// Extract status field from a resource as JSON string for comparison
///
/// Returns None if the object doesn't have a status field or serialization fails.
/// This is used to detect status changes without knowing the concrete status type.
fn extract_status_json<T: Serialize>(obj: &T) -> Option<String> {
    serde_json::to_value(obj)
        .ok()
        .and_then(|v| v.get("status").cloned())
        .and_then(|s| serde_json::to_string(&s).ok())
}

/// Extract status field from a resource as serde_json::Value
///
/// Returns the status as a JSON Value, or None if not present.
/// This is used for persisting status to FileSystem (.status files).
pub fn extract_status_value<T: Serialize>(obj: &T) -> Option<serde_json::Value> {
    serde_json::to_value(obj).ok().and_then(|v| v.get("status").cloned())
}

/// Object-safe trait for processor management
///
/// This trait allows storing different ResourceProcessor<T> types in a HashMap
/// and provides common operations without knowing the concrete type.
pub trait ProcessorObj: Send + Sync {
    /// Get the resource kind name
    fn kind(&self) -> &'static str;

    /// Get WatchObj for ConfigSyncServer registration
    fn as_watch_obj(&self) -> Arc<dyn WatchObj>;

    /// Enqueue a key for processing
    fn requeue(&self, key: String);

    /// Enqueue a key with delay
    fn requeue_after(&self, key: String, duration: Duration);

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

    /// Secret reference manager (shared across all processors)
    secret_ref_manager: Arc<SecretRefManager>,

    /// Processing handler (resource-specific logic)
    handler: Arc<dyn ProcessorHandler<K>>,

    /// Metadata filter configuration
    metadata_filter: RwLock<Option<Arc<MetadataFilterConfig>>>,

    /// Namespace filter
    namespace_filter: RwLock<Option<Arc<Vec<String>>>>,
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
        secret_ref_manager: Arc<SecretRefManager>,
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
            secret_ref_manager,
            handler,
            metadata_filter: RwLock::new(None),
            namespace_filter: RwLock::new(None),
        }
    }

    /// Create with custom workqueue config
    pub fn with_workqueue_config(
        kind: &'static str,
        capacity: usize,
        handler: Arc<dyn ProcessorHandler<K>>,
        secret_ref_manager: Arc<SecretRefManager>,
        workqueue_config: WorkqueueConfig,
    ) -> Self {
        let cache = Arc::new(ServerCache::new(capacity as u32));
        let workqueue = Arc::new(Workqueue::new(kind, workqueue_config));

        Self {
            kind,
            cache,
            workqueue,
            secret_ref_manager,
            handler,
            metadata_filter: RwLock::new(None),
            namespace_filter: RwLock::new(None),
        }
    }

    /// Set metadata filter configuration
    pub fn set_metadata_filter(&self, filter: MetadataFilterConfig) {
        *self.metadata_filter.write().unwrap() = Some(Arc::new(filter));
    }

    /// Set namespace filter
    pub fn set_namespace_filter(&self, namespaces: Vec<String>) {
        *self.namespace_filter.write().unwrap() = Some(Arc::new(namespaces));
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
    /// Returns true if resource was processed, false if filtered.
    ///
    /// Note: During init phase, we don't persist status because the resources
    /// are freshly loaded from K8s and their status is already up-to-date.
    pub fn on_init_apply(&self, obj: K) -> bool {
        let ctx = self.create_context();
        matches!(self.process_resource(obj, &ctx, true), WorkItemResult::Processed { .. })
    }

    /// Handle InitDone event (LIST completed)
    pub fn on_init_done(&self) {
        WatchObj::set_ready(self.cache.as_ref());
        tracing::info!(
            component = "resource_processor",
            kind = self.kind,
            "Init done, cache ready"
        );
    }

    /// Handle Apply event (enqueue for runtime processing)
    pub fn on_apply(&self, obj: &K) {
        let key = make_resource_key(obj);
        let workqueue = self.workqueue.clone();

        tokio::spawn(async move {
            workqueue.enqueue(key).await;
        });
    }

    /// Handle Delete event (enqueue for runtime processing)
    pub fn on_delete(&self, obj: &K) {
        let key = make_resource_key(obj);
        let workqueue = self.workqueue.clone();

        tokio::spawn(async move {
            workqueue.enqueue(key).await;
        });
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

    /// Save resource to cache
    pub fn save(&self, obj: K) {
        self.cache.apply_change(ResourceChange::EventUpdate, obj);
    }

    /// Remove resource from cache by key
    pub fn remove(&self, key: &str) {
        // Get the cached object first to properly delete
        if let Some(cached) = self.cache.get_by_key(key) {
            self.cache.apply_change(ResourceChange::EventDelete, cached);
        }
    }

    // ==================== Worker Processing ====================

    /// Process a single work item (called by worker loop)
    ///
    /// This compares the store state (from K8s) with cache state
    /// and determines whether to update or delete.
    ///
    /// Returns `WorkItemResult` indicating what action was taken and whether
    /// status needs to be persisted.
    pub fn process_work_item(&self, key: &str, store_obj: Option<K>) -> WorkItemResult<K> {
        let ctx = self.create_context();
        let cache_obj = self.get(key);

        match (store_obj, cache_obj) {
            (Some(obj), _) => {
                // Object exists in store -> process it
                self.process_resource(obj, &ctx, false)
            }
            (None, Some(cached)) => {
                // Object deleted from store but exists in cache -> delete
                self.process_delete(&cached, &ctx);
                WorkItemResult::Deleted { key: key.to_string() }
            }
            (None, None) => {
                // Both empty -> already processed, skip
                tracing::trace!(kind = self.kind, key = key, "Already processed, skipping");
                WorkItemResult::Skipped
            }
        }
    }

    // ==================== Internal Methods ====================

    /// Create handler context
    fn create_context(&self) -> HandlerContext {
        HandlerContext::new(
            self.secret_ref_manager.clone(),
            self.metadata_filter.read().unwrap().clone(),
            self.namespace_filter.read().unwrap().clone(),
        )
    }

    /// Process a resource through the handler pipeline
    fn process_resource(&self, obj: K, ctx: &HandlerContext, is_init: bool) -> WorkItemResult<K> {
        // Extract name/namespace early for logging (owned strings to avoid borrow issues)
        let name = obj.meta().name.clone().unwrap_or_default();
        let namespace = obj.meta().namespace.clone().unwrap_or_default();

        // 1. Check namespace filter
        if let Some(allowed_ns) = ctx.namespace_filter() {
            if !namespace.is_empty() && !allowed_ns.iter().any(|n| n == &namespace) {
                tracing::trace!(
                    kind = self.kind,
                    name = %name,
                    namespace = %namespace,
                    "Skipped by namespace filter"
                );
                return WorkItemResult::Skipped;
            }
        }

        // 2. Check handler filter
        if !self.handler.filter(&obj) {
            tracing::trace!(
                kind = self.kind,
                name = %name,
                namespace = %namespace,
                "Skipped by handler filter"
            );
            return WorkItemResult::Skipped;
        }

        // 3. Clean metadata
        // First apply context's metadata filter (removes managedFields, annotations, etc.)
        // Then let handler do any additional custom cleaning
        let mut obj = obj;
        ctx.clean_metadata(&mut obj);
        self.handler.clean_metadata(&mut obj, ctx);

        // 4. Validate (log warnings but continue)
        let warnings = self.handler.validate(&obj, ctx);
        for warning in &warnings {
            tracing::warn!(
                kind = self.kind,
                name = %name,
                namespace = %namespace,
                warning = %warning,
                "Resource validation warning"
            );
        }

        // 5. Parse/preprocess
        match self.handler.parse(obj, ctx) {
            ProcessResult::Continue(mut parsed_obj) => {
                // 6. Capture old status for comparison (serialize to JSON for comparison)
                let old_status = extract_status_json(&parsed_obj);

                // 7. Update status (handler sets Gateway API conditions)
                self.handler.update_status(&mut parsed_obj, ctx, &warnings);

                // 8. Check if status changed
                let new_status = extract_status_json(&parsed_obj);
                let status_changed = old_status != new_status;

                if status_changed {
                    tracing::trace!(
                        kind = self.kind,
                        name = %name,
                        namespace = %namespace,
                        "Status changed, will persist"
                    );
                }

                // 9. Call on_change
                if !is_init {
                    self.handler.on_change(&parsed_obj, ctx);
                }

                let phase = if is_init { "init" } else { "runtime" };
                tracing::debug!(
                    kind = self.kind,
                    name = %parsed_obj.meta().name.as_deref().unwrap_or(""),
                    namespace = %parsed_obj.meta().namespace.as_deref().unwrap_or(""),
                    phase = phase,
                    status_changed = status_changed,
                    "Resource processed and saving"
                );

                // 10. Save to cache
                // Use InitAdd during init phase (synchronous), EventUpdate at runtime (async)
                let obj_for_result = parsed_obj.clone();
                if is_init {
                    self.cache.apply_change(ResourceChange::InitAdd, parsed_obj);
                } else {
                    self.save(parsed_obj);
                }

                WorkItemResult::Processed {
                    obj: obj_for_result,
                    status_changed,
                }
            }
            ProcessResult::Skip { reason } => {
                tracing::debug!(
                    kind = self.kind,
                    reason = %reason,
                    "Resource skipped after parse"
                );
                WorkItemResult::Skipped
            }
        }
    }

    /// Process resource deletion
    fn process_delete(&self, cached_obj: &K, ctx: &HandlerContext) {
        let key = make_resource_key(cached_obj);

        // 1. Execute handler's delete cleanup
        self.handler.on_delete(cached_obj, ctx);

        // 2. Remove from cache
        self.remove(&key);

        tracing::debug!(
            kind = self.kind,
            key = %key,
            "Resource deleted from cache"
        );
    }
}

// Implement ProcessorObj for ResourceProcessor<K>
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

    fn requeue(&self, key: String) {
        let workqueue = self.workqueue.clone();
        tokio::spawn(async move {
            workqueue.enqueue(key).await;
        });
    }

    fn requeue_after(&self, key: String, duration: Duration) {
        let workqueue = self.workqueue.clone();
        tokio::spawn(async move {
            tokio::time::sleep(duration).await;
            workqueue.enqueue(key).await;
        });
    }

    fn is_ready(&self) -> bool {
        self.cache.is_ready()
    }

    fn set_ready(&self) {
        WatchObj::set_ready(self.cache.as_ref());
    }

    fn set_not_ready(&self) {
        WatchObj::set_not_ready(self.cache.as_ref());
    }

    fn clear(&self) {
        WatchObj::clear(self.cache.as_ref());
    }

    fn set_namespace_filter_vec(&self, filter: Option<Vec<String>>) {
        *self.namespace_filter.write().unwrap() = filter.map(Arc::new);
    }

    fn set_metadata_filter_config(&self, filter: Option<MetadataFilterConfig>) {
        *self.metadata_filter.write().unwrap() = filter.map(Arc::new);
    }

    fn workqueue(&self) -> Arc<Workqueue> {
        self.workqueue.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::conf_mgr::sync_runtime::resource_processor::handler::DefaultHandler;
    use kube::api::ObjectMeta;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TestResource {
        metadata: ObjectMeta,
        spec: String,
    }

    impl ResourceMeta for TestResource {
        fn get_version(&self) -> u64 {
            0
        }
        fn resource_kind() -> crate::types::ResourceKind {
            crate::types::ResourceKind::Unspecified
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
        let secret_ref_manager = Arc::new(SecretRefManager::new());
        let handler = Arc::new(DefaultHandler);

        let processor = ResourceProcessor::<TestResource>::new("TestResource", 100, handler, secret_ref_manager);

        assert_eq!(processor.kind(), "TestResource");
        assert!(!processor.is_ready());

        processor.set_ready();
        assert!(processor.is_ready());
    }

    #[tokio::test]
    async fn test_processor_init_apply() {
        let secret_ref_manager = Arc::new(SecretRefManager::new());
        let handler = Arc::new(DefaultHandler);

        let processor = ResourceProcessor::<TestResource>::new("TestResource", 100, handler, secret_ref_manager);

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

        let processed = processor.on_init_apply(resource.clone());
        assert!(processed);

        processor.on_init_done();
        assert!(processor.is_ready());

        // Check resource is in cache
        let cached = processor.get("default/test");
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().spec, "test spec");
    }
}
