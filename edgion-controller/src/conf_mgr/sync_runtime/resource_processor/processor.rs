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
        /// Backend persistence payload; Kubernetes may include foreign observations.
        persisted_status: Option<serde_json::Value>,
    },
    /// Resource was deleted
    Deleted {
        /// The key of the deleted resource
        key: String,
    },
    /// Nothing to do (already processed or filtered)
    Skipped,
}

use crate::conf_mgr::conf_center::MetadataFilterConfig;
use crate::conf_mgr::ControllerRuntime;
use crate::conf_sync::conf_server::WatchObj;
use crate::conf_sync::ServerCache;
use edgion_common::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use edgion_resources::ResourceMeta;

use super::context::HandlerContext;
use super::handler::{ProcessResult, ProcessorHandler};
use super::make_resource_key;
use super::raw_store::RawStore;
use super::{status_observation, status_ownership, status_pipeline};
use crate::conf_mgr::sync_runtime::workqueue::{TriggerChain, Workqueue};

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
fn status_has_changed(
    kind: &str,
    old: &StatusExtractResult,
    new: &StatusExtractResult,
    resource_namespace: Option<&str>,
) -> bool {
    match (old, new) {
        (StatusExtractResult::Present(old_val), StatusExtractResult::Present(new_val)) => {
            super::status_diff::status_semantically_changed(kind, old_val, new_val, resource_namespace)
        }
        (StatusExtractResult::Empty, StatusExtractResult::Empty) => false,
        (StatusExtractResult::SerializationError, _) | (_, StatusExtractResult::SerializationError) => true,
        _ => true,
    }
}

/// Capture the ingest-only unknown-field sidecar carried by the four resource
/// kinds that support advisory unknown-field detection.
///
/// FileSystem/etcd parsing populates this state before status seeding. Because
/// the fields are intentionally `#[serde(skip)]`, the generic status merge
/// below must preserve them explicitly across its serde round trip.
fn capture_unknown_fields_sidecar<T: 'static>(obj: &T) -> Option<Vec<String>> {
    let obj = obj as &dyn Any;
    if let Some(resource) = obj.downcast_ref::<edgion_resources::EdgionPlugins>() {
        Some(resource.spec.unknown_fields.clone())
    } else if let Some(resource) = obj.downcast_ref::<edgion_resources::EdgionStreamPlugins>() {
        Some(resource.spec.unknown_fields.clone())
    } else if let Some(resource) = obj.downcast_ref::<edgion_resources::EdgionConfigData>() {
        Some(resource.spec.unknown_fields.clone())
    } else {
        obj.downcast_ref::<edgion_resources::LinkSys>()
            .map(|resource| resource.spec.unknown_fields.clone())
    }
}

fn restore_unknown_fields_sidecar<T: 'static>(obj: &mut T, unknown_fields: Option<Vec<String>>) {
    let Some(unknown_fields) = unknown_fields else {
        return;
    };
    let obj = obj as &mut dyn Any;
    if let Some(resource) = obj.downcast_mut::<edgion_resources::EdgionPlugins>() {
        resource.spec.unknown_fields = unknown_fields;
    } else if let Some(resource) = obj.downcast_mut::<edgion_resources::EdgionStreamPlugins>() {
        resource.spec.unknown_fields = unknown_fields;
    } else if let Some(resource) = obj.downcast_mut::<edgion_resources::EdgionConfigData>() {
        resource.spec.unknown_fields = unknown_fields;
    } else if let Some(resource) = obj.downcast_mut::<edgion_resources::LinkSys>() {
        resource.spec.unknown_fields = unknown_fields;
    }
}

/// Seed previously persisted status (FileSystem/Etcd `.status` sidecar) back
/// into the object before `update_status` runs.
///
/// Spec files carry no status, so without this every reconcile enters
/// `update_status` with empty conditions and `update_condition` can never
/// preserve `lastTransitionTime` — the transition-only LTT contract requires
/// the prior conditions to be present in the object (K8s mode gets this for
/// free from the watch object). Best-effort: on any roundtrip failure the
/// object is left unchanged (same as the pre-seed behavior).
fn seed_existing_status<T: Serialize + DeserializeOwned + 'static>(obj: &mut T, status_json: &str) {
    let status = match serde_json::from_str::<serde_json::Value>(status_json) {
        Ok(v) if !v.is_null() => v,
        _ => return,
    };
    // Never seed an ErrorStatus sidecar (parse-failure fallback): no handler
    // manages its condition type, so once seeded it would survive every
    // upsert-only update_status as a permanent ghost. Skipping the seed lets
    // the recovery pass rebuild a clean status that replaces the sidecar.
    let is_error_status_sidecar = status.get("conditions").and_then(|c| c.as_array()).is_some_and(|arr| {
        arr.iter().any(|c| {
            c.get("type").and_then(|t| t.as_str())
                == Some(crate::conf_mgr::conf_center::common::ERROR_STATUS_CONDITION_TYPE)
        })
    });
    if is_error_status_sidecar {
        return;
    }
    let unknown_fields = capture_unknown_fields_sidecar(obj);
    let Ok(mut obj_value) = serde_json::to_value(&*obj) else {
        return;
    };
    let Some(map) = obj_value.as_object_mut() else {
        return;
    };
    map.insert("status".to_string(), status);
    match serde_json::from_value::<T>(obj_value) {
        Ok(mut seeded) => {
            restore_unknown_fields_sidecar(&mut seeded, unknown_fields);
            *obj = seeded;
        }
        Err(e) => {
            tracing::debug!(error = %e, "Existing status not seedable into object; conditions start fresh");
        }
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

    /// Identity-only diagnostic view of committed raw resources, including filtered objects.
    fn raw_keys(&self) -> Vec<String>;

    /// Look up a processed resource from `ServerCache` by key.
    ///
    /// Unlike `get_raw_any`, this returns the handler-filtered object after
    /// `preparse`, `parse`, and status resolution. Use it for cross-kind reads
    /// that must observe Controller-resolved references and authorization results.
    /// Do not use it for an `on_change` self-lookup: the current object is saved
    /// only after `on_change` returns.
    ///
    /// The returned object may contain resolved secret material. Downcast and
    /// project only the required fields; never serialize it directly to an
    /// external response.
    fn get_cached_any(&self, key: &str) -> Option<Arc<dyn Any + Send + Sync>>;

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

    /// Serialize the processed cache snapshot. Unlike `get_raw_any`, this exposes
    /// Controller-derived fields such as delegated route `resolvedRules` and
    /// internal reference-denial markers used by read-only effective APIs.
    ///
    /// # Secret boundary — read before returning this to a client
    ///
    /// The output is **not redacted**. `crate::redact`'s `AdminRedactGuard` is a
    /// gateway-admin mechanism; the controller never installs it, so the
    /// `redact_opt` / `redact_required` field adapters here emit the *real*
    /// value. For kinds carrying controller-resolved secrets (`EdgionPlugins`
    /// with `openid_connect` / `ldap_auth` / `jwe_decrypt` configs, `EdgionTls`,
    /// `EdgionAcme`, `LinkSys`, ...) this snapshot therefore contains live
    /// credentials.
    ///
    /// Existing callers are safe because they project a fixed set of
    /// non-secret fields out of the snapshot rather than returning it.
    /// A caller that serializes the snapshot straight into a response — or adds
    /// a passthrough field — leaks credentials to any identity holding the
    /// endpoint's verb, including one explicitly denied `Secret` access.
    /// Project explicit fields, never pass the value through.
    fn list_cached_json(&self) -> Vec<serde_json::Value>;

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

    /// Resource cache
    cache: Arc<ServerCache<K>>,

    /// Work queue for runtime event processing
    workqueue: Arc<Workqueue>,

    /// Processing handler (resource-specific logic)
    handler: Arc<dyn ProcessorHandler<K>>,

    /// Metadata filter configuration
    metadata_filter: RwLock<Option<Arc<MetadataFilterConfig>>>,

    /// Namespace filter
    namespace_filter: RwLock<Option<Arc<[String]>>>,

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

    /// Inject the raw store backend.
    ///
    /// Called once during controller startup:
    /// - K8s mode: immediately after `reflector::store()`, before any events arrive.
    /// - FS mode: in `FileSystemResourceController::new()`.
    pub fn set_raw_store(&self, store: Arc<dyn RawStore<K>>) {
        *self.raw_store.write() = Some(store);
    }

    pub(crate) fn runtime(&self) -> &ControllerRuntime {
        &self.runtime
    }

    /// Set metadata filter configuration
    pub fn set_metadata_filter(&self, filter: MetadataFilterConfig) {
        *self.metadata_filter.write() = Some(Arc::new(filter));
    }

    /// Set namespace filter
    pub fn set_namespace_filter(&self, namespaces: Vec<String>) {
        *self.namespace_filter.write() = Some(namespaces.into());
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
        crate::conf_mgr::sync_runtime::resource_version::bump_resource_version_floor(rv);

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

    /// Wake any `wait_kinds_ready` waiter. Called by the etcd/FS `InitDone`
    /// handlers after `on_init_done()` so the Phase-1 gate observes readiness
    /// immediately instead of via the 1 s fallback poll. The K8s handlers
    /// deliberately do not call this — the constraint is to leave K8s unchanged.
    pub fn notify_ready(&self) {
        self.runtime.processor_registry.notify_ready();
    }

    /// Enqueue a watch-event key for the worker (apply and delete alike —
    /// the worker compares store state vs cache and decides update/delete).
    ///
    /// Workqueue discipline: ALL runtime events go through here; nothing
    /// outside the worker may call `process_resource`/`process_delete`.
    /// (Init is the sanctioned exception: `on_init_apply` runs pre-worker.)
    pub async fn on_event_key(&self, key: String) {
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

        // No `sv` field here: sync_version is drawn inside the store's write lock (see
        // ServerCache::push_event), so it does not exist yet and must not be predicted.
        // Drawing it early for span correlation is what opened the reordering window that
        // got legitimate events discarded as stale. The cache_server's "apply change" log
        // line reports the assigned version and correlates to this span via kind + name/ns.
        let span = tracing::info_span!(
            "process_resource",
            component = "resource_processor",
            kind = self.kind,
            name = %name,
            ns = %namespace,
            rv,
        );
        self.process_resource_inner(obj, ctx, is_init, existing_status_json)
            .instrument(span)
            .await
    }

    async fn process_resource_inner(
        &self,
        mut obj: K,
        ctx: &HandlerContext,
        is_init: bool,
        existing_status_json: Option<String>,
    ) -> WorkItemResult<K> {
        status_pipeline::clear_current_status(&mut obj);
        let namespace = obj.meta().namespace.clone().unwrap_or_default();

        // Eligibility can change without a delivered Delete (LIST replacement or key coalescing).
        let namespace_eligible = ctx
            .namespace_filter()
            .is_none_or(|allowed_ns| namespace.is_empty() || allowed_ns.iter().any(|n| n == &namespace));
        if !namespace_eligible || !self.handler.filter(&obj, ctx) {
            if let Some(cached) = self.cache.get_by_key(&make_resource_key(&obj)) {
                self.process_delete(&cached, ctx).await;
                return WorkItemResult::Deleted {
                    key: make_resource_key(&obj),
                };
            }
            return WorkItemResult::Skipped;
        }

        // 3. Clean metadata
        // First apply context's metadata filter (removes managedFields, annotations, etc.)
        // Then let handler do any additional custom cleaning
        let mut obj = obj;
        ctx.clean_metadata(&mut obj);
        self.handler.clean_metadata(&mut obj, ctx);

        // 3.5. Seed previously persisted status (FileSystem/Etcd `.status`
        // sidecar) into the object, so update_condition later sees the prior
        // conditions and preserves lastTransitionTime across reconciles.
        // This MUST run before preparse/parse: the seed is a serde roundtrip
        // that resets `#[serde(skip)]` runtime fields. The four ingest-only
        // unknown-field sidecars are preserved explicitly by
        // `seed_existing_status`; all other runtime fields are populated only
        // after this point by preparse/parse.
        if let Some(json) = existing_status_json.as_deref() {
            seed_existing_status(&mut obj, json);
        }

        // Capture the persisted status before any handler hook can mutate it.
        // In Kubernetes mode parse() may resolve references and update a
        // condition directly (for example LinkSys ResolvedRefs). Capturing
        // after parse() would compare the new status with itself and suppress
        // the required status write.
        let old_status = match existing_status_json.as_deref() {
            Some(json) => StatusExtractResult::Present(json.to_string()),
            None => extract_status_json(&obj),
        };

        if matches!(old_status, StatusExtractResult::SerializationError) {
            tracing::warn!("Failed to serialize old status for comparison");
        }

        let kubernetes = self.runtime.status_identity().is_some();
        let raw_resource = kubernetes.then(|| serde_json::to_value(&obj).ok()).flatten();
        let observed_status = raw_resource.as_ref().and_then(|raw| raw.get("status"));
        let observer = kubernetes
            .then(|| status_pipeline::observer_identity(&self.runtime))
            .flatten();
        let mut persistence_allowed = !kubernetes || raw_resource.is_some();
        if !kubernetes && status_pipeline::has_non_native_status(&obj) {
            // Native backends cannot attribute imported Kubernetes observations.
            // Recompute local diagnostics rather than passing an envelope to handlers.
            let _ = status_pipeline::replace_status(&mut obj, serde_json::Value::Null);
        }
        if kubernetes && status_pipeline::is_partitioned(self.kind) {
            let local = match (&observer, observed_status) {
                (Some(identity), Some(status)) => status_observation::project_status(self.kind, status, identity),
                (Some(_), None) => Ok(None),
                (None, _) => {
                    persistence_allowed = false;
                    Ok(None)
                }
            };
            let local = match local {
                Ok(local) => local.unwrap_or(serde_json::Value::Null),
                Err(error) => {
                    persistence_allowed = false;
                    tracing::warn!(component = "status_projection", kind = self.kind, error = %error,
                        "Invalid controller observations; skipping status persistence");
                    serde_json::Value::Null
                }
            };
            if status_pipeline::replace_status(&mut obj, local).is_err() {
                persistence_allowed = false;
                // Empty native state must replace an unprojected envelope before handlers.
                let _ = status_pipeline::replace_status(&mut obj, serde_json::Value::Null);
            }
        }

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
                // 7. Report whether the pre-handler status snapshot exists.
                tracing::debug!(
                    kind = self.kind,
                    old_status_present = matches!(old_status, StatusExtractResult::Present(_)),
                    "Status comparison: old_status captured before handler mutation"
                );

                // 8. Update status (handler sets Gateway API conditions)
                self.handler.update_status(&mut parsed_obj, ctx, &all_errors);
                let mut persisted_status = extract_status_value(&parsed_obj);
                if kubernetes && status_pipeline::is_partitioned(self.kind) {
                    persisted_status = match (&observer, persisted_status.as_ref()) {
                        (Some(identity), Some(local)) if persistence_allowed => {
                            let facts = status_pipeline::class_observations(&self.runtime, observed_status);
                            match status_observation::merge_status(self.kind, observed_status, local, identity, &facts)
                            {
                                Ok(status) => status,
                                Err(error) => {
                                    tracing::warn!(component = "status_projection", kind = self.kind, error = %error,
                                        "Controller observation cannot be persisted; local configuration retained");
                                    None
                                }
                            }
                        }
                        _ => None,
                    };
                } else if let (Some(raw), Some(computed)) = (raw_resource.as_ref(), persisted_status.as_ref()) {
                    if let Some((local, persisted)) = status_ownership::native_status_projection(
                        self.kind,
                        &namespace,
                        raw,
                        observed_status.unwrap_or(&serde_json::Value::Null),
                        computed,
                        &self.runtime,
                    ) {
                        if status_pipeline::replace_status(&mut parsed_obj, local).is_ok() {
                            persisted_status = Some(persisted);
                        } else {
                            persistence_allowed = false;
                        }
                    }
                }
                if !persistence_allowed {
                    persisted_status = None;
                }
                // The local decision is transported separately from the complete
                // API history. Gateway proof finalization consumes this projection.
                status_pipeline::capture_current_status(&mut parsed_obj);
                self.handler.finalize_transport(&mut parsed_obj, ctx);
                status_pipeline::filter_current_route_status(&mut parsed_obj);
                let complete_status = persisted_status.clone().unwrap_or_else(|| match &old_status {
                    StatusExtractResult::Present(status) => serde_json::from_str(status).unwrap_or_default(),
                    _ => serde_json::Value::Null,
                });
                if let Err(error) = status_pipeline::replace_status(&mut parsed_obj, complete_status) {
                    tracing::warn!(component = "status_projection", kind = self.kind, error = %error,
                        "Complete status could not be restored; skipping publication");
                    return WorkItemResult::Skipped;
                }

                // 9. Check if status changed
                let new_status = match persisted_status.as_ref() {
                    Some(status) => StatusExtractResult::Present(status.to_string()),
                    None => StatusExtractResult::Empty,
                };

                // Log serialization errors
                if matches!(new_status, StatusExtractResult::SerializationError) {
                    tracing::warn!("Failed to serialize new status for comparison");
                }

                let status_changed = persisted_status.is_some()
                    && status_has_changed(
                        self.kind,
                        &old_status,
                        &new_status,
                        parsed_obj.meta().namespace.as_deref(),
                    );

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
                        crate::conf_mgr::sync_runtime::record_status_write_skipped(self.kind);
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
                // Use InitAdd during init phase (synchronous), EventUpdate at runtime (async).
                // `None` means "assign sync_version at the store, under its write lock".
                let obj_for_result = parsed_obj.clone();
                if is_init {
                    self.cache.apply_change(ResourceChange::InitAdd, parsed_obj, None);
                    self.stats_dirty.store(true, Ordering::Relaxed);
                } else {
                    self.save(parsed_obj, None);
                }

                // Consumers of controller-owned proofs must observe the exact
                // object just published above, never the previous cache value.
                self.handler.after_publish(&obj_for_result, ctx).await;

                // Merge Patch must remove the retired native siblings when first
                // installing an envelope. Tombstones are wire-only, not local or
                // semantic-comparison state, and come only from validated fields.
                if kubernetes && status_pipeline::is_partitioned(self.kind) {
                    if let (Some(observed), Some(persisted)) = (
                        observed_status.filter(|status| status.get("controllers").is_none()),
                        persisted_status.as_mut(),
                    ) {
                        match status_observation::retired_native_fields(self.kind, observed) {
                            Ok(fields) => {
                                if let Some(map) = persisted.as_object_mut() {
                                    for field in fields {
                                        map.insert(field, serde_json::Value::Null);
                                    }
                                }
                            }
                            Err(_) => persisted_status = None,
                        }
                    }
                }

                WorkItemResult::Processed {
                    obj: obj_for_result,
                    status_changed: status_changed && persisted_status.is_some(),
                    persisted_status,
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
        // No `sv` field / no early allocation: the version is drawn inside the store's write
        // lock. This method is where the cache-residue incident originated -- a version drawn
        // here, then ~839us of on_delete work (clearing secret refs), then a store write that
        // a parallel task had already overtaken. The store then discarded this delete as
        // "stale" against a kind-wide high-water mark set by an unrelated Gateway.
        let span = tracing::info_span!(
            "process_delete",
            component = "resource_processor",
            kind = self.kind,
            key = %key,
        );

        async {
            // 1. Execute handler's delete cleanup
            self.handler.on_delete(cached_obj, ctx).await;

            // 2. Remove from cache (sync_version assigned at the store)
            self.remove(&key, None);

            // 3. Requeue consumers only after they can observe the absence of
            // this exact generation in ServerCache.
            self.handler.after_unpublish(cached_obj, ctx).await;

            tracing::debug!("Resource deleted from cache");
        }
        .instrument(span)
        .await
    }
}

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
        *self.namespace_filter.write() = filter.map(Arc::from);
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

    fn list_cached_json(&self) -> Vec<serde_json::Value> {
        self.cache
            .list_keys()
            .into_iter()
            .filter_map(|key| self.cache.get_by_key(&key))
            .filter_map(|resource| serde_json::to_value(&resource).ok())
            .collect()
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

    fn raw_keys(&self) -> Vec<String> {
        let store = self.raw_store.read().clone();
        store
            .map(|store| {
                store
                    .snapshot()
                    .iter()
                    .map(|obj| make_resource_key(obj.as_ref()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn get_cached_any(&self, key: &str) -> Option<Arc<dyn Any + Send + Sync>> {
        self.cache
            .get_by_key(key)
            .map(|resource| Arc::new(resource) as Arc<dyn Any + Send + Sync>)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kube::api::ObjectMeta;
    use serde::{Deserialize, Serialize};

    struct TestHandler;

    struct EligibilityHandler;
    #[async_trait::async_trait]
    impl ProcessorHandler<TestResource> for EligibilityHandler {
        fn filter(&self, obj: &TestResource, _ctx: &HandlerContext) -> bool {
            obj.spec != "excluded"
        }
    }

    #[tokio::test]
    async fn local_raw_replacement_losing_eligibility_unpublishes_without_removing_raw() {
        use super::super::raw_store::LocalRawStore;
        for namespace_filter in [false, true] {
            let processor = ResourceProcessor::new(
                "TestResource",
                10,
                Arc::new(EligibilityHandler),
                Arc::new(ControllerRuntime::new()),
            );
            let raw = Arc::new(LocalRawStore::new());
            processor.set_raw_store(raw.clone());
            let mut obj = TestResource {
                metadata: ObjectMeta {
                    name: Some("same-key".into()),
                    namespace: Some("a".into()),
                    uid: Some("first".into()),
                    ..Default::default()
                },
                spec: "accepted".into(),
                status: None,
            };
            raw.upsert("a/same-key".into(), Arc::new(obj.clone()));
            processor.on_init_apply(obj.clone(), None).await;
            processor.on_init_done();
            obj.metadata.uid = Some("second".into());
            if namespace_filter {
                processor.set_namespace_filter(vec!["b".into()]);
            } else {
                obj.spec = "excluded".into();
            }
            raw.upsert("a/same-key".into(), Arc::new(obj.clone()));
            let result = processor
                .process_work_item("a/same-key", Some(obj), Some("{}".into()), TriggerChain::default())
                .await;
            assert!(matches!(result, WorkItemResult::Deleted { .. }));
            assert!(processor.get("a/same-key").is_none());
            assert_eq!(
                processor
                    .get_raw_any("a/same-key")
                    .unwrap()
                    .downcast::<TestResource>()
                    .unwrap()
                    .metadata
                    .uid
                    .as_deref(),
                Some("second")
            );
        }
    }

    #[async_trait::async_trait]
    impl<K> ProcessorHandler<K> for TestHandler where K: kube::Resource + Clone + Send + Sync + 'static {}

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TestResource {
        metadata: ObjectMeta,
        spec: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<serde_json::Value>,
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

    struct PublishOrderHandler {
        processor: parking_lot::Mutex<Option<std::sync::Weak<ResourceProcessor<TestResource>>>>,
        observed_final_cache: std::sync::atomic::AtomicBool,
        observed_unpublished_cache: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl ProcessorHandler<TestResource> for PublishOrderHandler {
        fn update_status(&self, obj: &mut TestResource, _ctx: &HandlerContext, _validation_errors: &[String]) {
            obj.spec = "status-derived".to_string();
        }

        fn finalize_transport(&self, obj: &mut TestResource, _ctx: &HandlerContext) {
            assert_eq!(obj.spec, "status-derived");
            obj.spec = "final-transport".to_string();
        }

        async fn after_publish(&self, obj: &TestResource, _ctx: &HandlerContext) {
            let processor = self
                .processor
                .lock()
                .as_ref()
                .and_then(std::sync::Weak::upgrade)
                .expect("test processor must still be alive");
            let cached = processor
                .get(&obj.key_name())
                .expect("after_publish must observe cache");
            self.observed_final_cache.store(
                cached.spec == "final-transport" && cached.spec == obj.spec,
                std::sync::atomic::Ordering::Release,
            );
        }

        async fn after_unpublish(&self, obj: &TestResource, _ctx: &HandlerContext) {
            let processor = self
                .processor
                .lock()
                .as_ref()
                .and_then(std::sync::Weak::upgrade)
                .expect("test processor must still be alive");
            self.observed_unpublished_cache.store(
                processor.get(&obj.key_name()).is_none(),
                std::sync::atomic::Ordering::Release,
            );
        }
    }

    #[tokio::test]
    async fn final_transport_is_published_before_post_publish_consumers_run() {
        let handler = Arc::new(PublishOrderHandler {
            processor: parking_lot::Mutex::new(None),
            observed_final_cache: std::sync::atomic::AtomicBool::new(false),
            observed_unpublished_cache: std::sync::atomic::AtomicBool::new(false),
        });
        let processor = Arc::new(ResourceProcessor::<TestResource>::new(
            "TestResource",
            100,
            handler.clone(),
            Arc::new(ControllerRuntime::default()),
        ));
        *handler.processor.lock() = Some(Arc::downgrade(&processor));
        let resource = TestResource {
            metadata: ObjectMeta {
                name: Some("publish-order".to_string()),
                ..Default::default()
            },
            spec: "raw".to_string(),
            status: None,
        };

        let result = processor.on_init_apply(resource, None).await;
        let WorkItemResult::Processed { obj, .. } = result else {
            panic!("expected processed result")
        };
        assert_eq!(obj.spec, "final-transport");
        assert!(handler.observed_final_cache.load(std::sync::atomic::Ordering::Acquire));

        let result = processor
            .process_work_item(
                "publish-order",
                None,
                None,
                crate::conf_mgr::sync_runtime::workqueue::TriggerChain::default(),
            )
            .await;
        assert!(matches!(result, WorkItemResult::Deleted { .. }));
        assert!(handler
            .observed_unpublished_cache
            .load(std::sync::atomic::Ordering::Acquire));
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

    /// `notify_ready()` must wake a `wait_kinds_ready` waiter promptly, without
    /// relying on the ~1s fallback poll inside `wait_kinds_ready`.
    ///
    /// The waiter is given a 5s timeout (far longer than the 1s fallback poll)
    /// so a pass driven by the fallback poll would still take ~1s. We then wrap
    /// the join in a 500ms timeout: if `notify_ready()` did nothing, the waiter
    /// would only resolve via the fallback poll (~1s later) and this 500ms
    /// timeout would fire, failing the test. Only an actual wake from
    /// `notify_ready()` can resolve the waiter within the 500ms window.
    #[tokio::test]
    async fn notify_ready_wakes_wait_kinds_ready_promptly() {
        use std::time::Duration;

        let handler = Arc::new(TestHandler);
        let processor = Arc::new(ResourceProcessor::<TestResource>::new(
            "TestResource",
            100,
            handler,
            Arc::new(ControllerRuntime::default()),
        ));

        processor.runtime.processor_registry.register(processor.clone());

        let runtime = processor.runtime.clone();
        let kind = processor.kind;

        // Long timeout: if this waiter resolves quickly, it must be because
        // notify_ready() woke it, not the 1s fallback poll.
        let waiter = tokio::spawn(async move {
            runtime
                .processor_registry
                .wait_kinds_ready(&[kind], Duration::from_secs(5))
                .await
        });

        // Give the waiter a moment to register its `Notify::notified()` future.
        // `notify_waiters()` only wakes futures that are already registered, so
        // notifying before the waiter subscribes would be a silent no-op.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Mirror what `on_init_done()` does: mark the cache ready directly
        // (bypassing the `ProcessorObj::set_ready()` trait method, which
        // already calls `notify_ready()` itself), then call `notify_ready()`
        // separately so this test isolates its effect.
        WatchObj::set_ready(processor.cache.as_ref());
        processor.notify_ready();

        let resolved = tokio::time::timeout(Duration::from_millis(500), waiter)
            .await
            .expect("waiter did not resolve within 500ms; notify_ready() failed to wake it promptly")
            .expect("waiter task panicked");

        assert!(resolved, "wait_kinds_ready should report the kind as ready");
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
            status: None,
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
        let snapshot = ProcessorObj::list_cached_json(&processor);
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0]["spec"], "test spec");
    }

    #[tokio::test]
    async fn test_processor_init_apply_bumps_resource_version_floor() {
        use crate::conf_mgr::sync_runtime::resource_version::next_resource_version;

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
            status: None,
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
                current_status: None,
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
        crate::conf_mgr::sync_runtime::resource_processor::update_condition(
            &mut status.conditions,
            crate::conf_mgr::sync_runtime::resource_processor::accepted_condition(gc_modified.metadata.generation),
        );
        crate::conf_mgr::sync_runtime::resource_processor::update_condition(
            &mut status.conditions,
            crate::conf_mgr::sync_runtime::resource_processor::condition_true(
                "SupportedVersion",
                "SupportedVersion",
                "Gateway API version is supported",
                gc_modified.metadata.generation,
            ),
        );

        let new_status = extract_status_json(&gc_modified);

        let changed = status_has_changed("GatewayClass", &old_status, &new_status, None);
        assert!(changed, "Status should have changed after update_status");
    }

    struct ParseStatusHandler;

    #[async_trait::async_trait]
    impl ProcessorHandler<TestResource> for ParseStatusHandler {
        async fn parse(&self, mut obj: TestResource, _ctx: &HandlerContext) -> ProcessResult<TestResource> {
            obj.status = Some(serde_json::json!({
                "conditions": [{
                    "type": "ResolvedRefs",
                    "status": "True",
                    "reason": "ResolvedRefs",
                    "message": "All references resolved",
                    "observedGeneration": 1
                }]
            }));
            ProcessResult::Continue(obj)
        }
    }

    #[tokio::test]
    async fn parse_status_change_is_compared_against_persisted_status() {
        let processor = ResourceProcessor::<TestResource>::new(
            "TestResource",
            100,
            Arc::new(ParseStatusHandler),
            Arc::new(ControllerRuntime::default()),
        );
        let resource = TestResource {
            metadata: ObjectMeta {
                name: Some("status-change".to_string()),
                ..Default::default()
            },
            spec: "test spec".to_string(),
            status: Some(serde_json::json!({
                "conditions": [{
                    "type": "ResolvedRefs",
                    "status": "False",
                    "reason": "SecretNotFound",
                    "message": "Secret not found",
                    "observedGeneration": 1
                }]
            })),
        };

        let result = processor.on_init_apply(resource, None).await;
        match result {
            WorkItemResult::Processed {
                obj, status_changed, ..
            } => {
                assert!(status_changed, "parse-time status mutation must be persisted");
                assert_eq!(
                    obj.status
                        .as_ref()
                        .and_then(|status| status.pointer("/conditions/0/status"))
                        .and_then(serde_json::Value::as_str),
                    Some("True")
                );
            }
            other => panic!("expected processed result, got {other:?}"),
        }
    }

    #[test]
    fn seed_existing_status_enables_ltt_preservation() {
        use edgion_resources::prelude_resources::GatewayClass;

        // FS/Etcd mode: the spec file carries no status.
        let mut gc = GatewayClass {
            metadata: ObjectMeta {
                name: Some("test-gc".to_string()),
                generation: Some(2),
                ..Default::default()
            },
            spec: edgion_resources::resources::gateway_class::GatewayClassSpec {
                current_status: None,
                controller_name: "example.com/controller".to_string(),
                description: None,
                parameters_ref: None,
            },
            status: None,
        };

        let sidecar = r#"{"conditions":[{"type":"Accepted","status":"True","reason":"Accepted","message":"Resource accepted","lastTransitionTime":"2026-01-01T00:00:00Z","observedGeneration":1}]}"#;
        seed_existing_status(&mut gc, sidecar);

        let status = gc.status.as_mut().expect("status seeded from sidecar");
        assert_eq!(status.conditions.len(), 1);

        // A reconcile that does not flip the condition keeps the stored LTT
        // (while observedGeneration still advances).
        crate::conf_mgr::sync_runtime::resource_processor::update_condition(
            &mut status.conditions,
            crate::conf_mgr::sync_runtime::resource_processor::accepted_condition(gc.metadata.generation),
        );
        assert_eq!(status.conditions[0].last_transition_time, "2026-01-01T00:00:00Z");
        assert_eq!(status.conditions[0].observed_generation, Some(2));
    }

    #[test]
    fn seed_existing_status_preserves_all_unknown_field_sidecars() {
        use edgion_resources::resources::edgion_config_data::{
            ConfigDataVisibility, ConfigEntry, EdgionConfigDataSpec,
        };
        use edgion_resources::resources::edgion_stream_plugins::EdgionStreamPluginsSpec;
        use edgion_resources::resources::link_sys::httpdns::{HttpDnsFallback, HttpDnsResponseFormat};
        use edgion_resources::resources::link_sys::{HttpDnsResolverConfig, LinkSysSpec, SystemConfig};
        use edgion_resources::{EdgionConfigData, EdgionPlugins, EdgionStreamPlugins, LinkSys};

        let expected = vec!["config.typo".to_string()];
        let status = r#"{"conditions":[]}"#;

        let mut plugins = EdgionPlugins::new("plugins", Default::default());
        plugins.spec.unknown_fields = expected.clone();
        seed_existing_status(&mut plugins, status);
        assert_eq!(plugins.spec.unknown_fields, expected);

        let mut stream_plugins = EdgionStreamPlugins::new(
            "stream-plugins",
            EdgionStreamPluginsSpec {
                current_status: None,
                plugins: None,
                tls_route_plugins: None,
                unknown_fields: expected.clone(),
            },
        );
        seed_existing_status(&mut stream_plugins, status);
        assert_eq!(stream_plugins.spec.unknown_fields, expected);

        let mut config_data = EdgionConfigData::new(
            "config-data",
            EdgionConfigDataSpec {
                current_status: None,
                enable: true,
                active: None,
                visibility: ConfigDataVisibility::Namespace,
                data: ConfigEntry::Misc(serde_json::json!({})),
                unknown_fields: expected.clone(),
            },
        );
        seed_existing_status(&mut config_data, status);
        assert_eq!(config_data.spec.unknown_fields, expected);

        let mut link_sys = LinkSys::new(
            "link-sys",
            LinkSysSpec {
                current_status: None,
                config: SystemConfig::HttpDns(HttpDnsResolverConfig {
                    preset: None,
                    url_template: Some("https://resolver.example/dns?name={domain}".to_string()),
                    response: HttpDnsResponseFormat::default(),
                    fallback: HttpDnsFallback::default(),
                    connection: None,
                }),
                unknown_fields: expected.clone(),
            },
        );
        seed_existing_status(&mut link_sys, status);
        assert_eq!(link_sys.spec.unknown_fields, expected);
    }

    #[test]
    fn seed_existing_status_skips_error_status_sidecar() {
        use edgion_resources::prelude_resources::GatewayClass;

        let mut gc = GatewayClass {
            metadata: ObjectMeta {
                name: Some("test-gc".to_string()),
                ..Default::default()
            },
            spec: edgion_resources::resources::gateway_class::GatewayClassSpec {
                current_status: None,
                controller_name: "example.com/controller".to_string(),
                description: None,
                parameters_ref: None,
            },
            status: None,
        };

        // A resource that was briefly unparseable has an ErrorStatus sidecar;
        // on recovery it must NOT be seeded, or its unmanaged condition type
        // would ghost through every subsequent upsert-only update_status.
        let sidecar = r#"{"conditions":[{"type":"Ready","status":"False","reason":"ParseError","message":"bad yaml","lastTransitionTime":"2026-01-01T00:00:00Z"}]}"#;
        seed_existing_status(&mut gc, sidecar);
        assert!(gc.status.is_none(), "ErrorStatus sidecar must not be seeded");
    }

    #[test]
    fn seed_existing_status_ignores_unparseable_json() {
        use edgion_resources::prelude_resources::GatewayClass;

        let mut gc = GatewayClass {
            metadata: ObjectMeta {
                name: Some("test-gc".to_string()),
                ..Default::default()
            },
            spec: edgion_resources::resources::gateway_class::GatewayClassSpec {
                current_status: None,
                controller_name: "example.com/controller".to_string(),
                description: None,
                parameters_ref: None,
            },
            status: None,
        };

        seed_existing_status(&mut gc, "not-json{{");
        assert!(gc.status.is_none(), "object must be left unchanged on parse failure");
    }

    #[tokio::test]
    async fn on_event_key_enqueues_for_worker() {
        let processor = ResourceProcessor::<TestResource>::new(
            "TestResource",
            100,
            Arc::new(TestHandler),
            Arc::new(ControllerRuntime::default()),
        );

        processor.on_event_key("obj1".to_string()).await;

        let item = tokio::time::timeout(std::time::Duration::from_secs(2), processor.workqueue().dequeue())
            .await
            .expect("dequeue timed out")
            .expect("queue closed");
        assert_eq!(item.key, "obj1");
    }

    #[tokio::test]
    async fn delete_flows_through_worker_arm() {
        let processor = ResourceProcessor::<TestResource>::new(
            "TestResource",
            100,
            Arc::new(TestHandler),
            Arc::new(ControllerRuntime::default()),
        );
        // Seed cache with the object, as a prior apply would have.
        let obj = TestResource {
            metadata: ObjectMeta {
                name: Some("obj1".to_string()),
                ..Default::default()
            },
            spec: "v1".to_string(),
            status: None,
        };
        processor.save(obj, None);
        assert!(processor.get("obj1").is_some());

        // Delete event: enqueue the key, then drive the worker path manually.
        processor.on_event_key("obj1".to_string()).await;
        let item = tokio::time::timeout(std::time::Duration::from_secs(2), processor.workqueue().dequeue())
            .await
            .expect("dequeue timed out")
            .expect("queue closed");
        let result = processor
            .process_work_item(&item.key, None, None, item.trigger_chain.clone())
            .await;

        assert!(matches!(result, WorkItemResult::Deleted { .. }));
        assert!(processor.get("obj1").is_none());
    }
}
