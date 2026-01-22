//! Resource Processor Module
//!
//! Defines the `ResourceProcessor` trait and related types for unified resource processing.
//! Each resource type has its own processor that implements this trait.
//!
//! ## Design
//!
//! - `ResourceProcessor<K>`: Trait for resource-specific processing logic
//! - `ProcessContext`: Context for processor methods, provides access to ConfigServer, RequeueRegistry
//! - `ProcessConfig`: Configuration for processing (metadata filter, etc.)
//! - `RequeueRegistry`: Cross-resource requeue mechanism
//!
//! ## Usage
//!
//! Both Init phase and Runtime phase use the same `process_resource` function,
//! ensuring consistent processing logic.

mod backend_tls_policy;
mod edgion_gateway_config;
mod edgion_plugins;
mod edgion_stream_plugins;
mod edgion_tls;
mod endpoint_slice;
mod endpoints;
mod gateway;
mod gateway_class;
mod grpc_route;
mod http_route;
mod link_sys;
mod plugin_metadata;
mod reference_grant;
mod secret;
mod service;
mod tcp_route;
mod tls_route;
mod udp_route;

pub use backend_tls_policy::BackendTlsPolicyProcessor;
pub use edgion_gateway_config::EdgionGatewayConfigProcessor;
pub use edgion_plugins::EdgionPluginsProcessor;
pub use edgion_stream_plugins::EdgionStreamPluginsProcessor;
pub use edgion_tls::EdgionTlsProcessor;
pub use endpoint_slice::EndpointSliceProcessor;
pub use endpoints::EndpointsProcessor;
pub use gateway::GatewayProcessor;
pub use gateway_class::GatewayClassProcessor;
pub use grpc_route::GrpcRouteProcessor;
pub use http_route::HttpRouteProcessor;
pub use link_sys::LinkSysProcessor;
pub use plugin_metadata::PluginMetadataProcessor;
pub use reference_grant::ReferenceGrantProcessor;
pub use secret::SecretProcessor;
pub use service::ServiceProcessor;
pub use tcp_route::TcpRouteProcessor;
pub use tls_route::TlsRouteProcessor;
pub use udp_route::UdpRouteProcessor;

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use k8s_openapi::api::core::v1::Secret;
use kube::Resource;

use crate::core::conf_mgr::MetadataFilterConfig;
use crate::core::conf_sync::conf_server::{ConfigServer, SecretRefManager};
use crate::core::conf_sync::types::ListData;

use super::workqueue::Workqueue;

/// Resource processing result
#[derive(Debug)]
pub enum ProcessResult<K> {
    /// Continue processing, resource may have been modified
    Continue(K),
    /// Skip this resource (e.g., filtered, validation failed)
    Skip { reason: String },
}

/// Resource processor trait
///
/// Each resource type implements this trait to define its specific processing logic.
/// The trait provides default implementations for common operations.
pub trait ResourceProcessor<K>: Send + Sync
where
    K: Resource + Clone + Send + Sync + 'static,
{
    /// Resource type name (used for logging, metrics, RequeueRegistry)
    fn kind(&self) -> &'static str;

    /// Resource filtering (called before enqueue and in process_resource)
    fn filter(&self, _obj: &K) -> bool {
        true
    }

    /// Clean metadata (remove blocked_annotations and managedFields)
    /// Default implementation uses clean_metadata utility
    fn clean_metadata(&self, obj: &mut K, ctx: &ProcessContext) {
        if let Some(config) = ctx.metadata_filter {
            crate::core::utils::clean_metadata(obj, config);
        }
    }

    /// Resource parsing/preprocessing
    /// - Parse Secret references
    /// - Register to SecretRefManager
    /// - Other resource-specific logic
    fn parse(&self, obj: K, ctx: &ProcessContext) -> ProcessResult<K>;

    /// Save resource to cache
    /// Uses apply_change(EventUpdate) internally, but doesn't go through old apply_xxx_change logic
    fn save(&self, cs: &ConfigServer, obj: K);

    /// Remove resource from cache
    /// Uses apply_change(EventDelete) internally
    fn remove(&self, cs: &ConfigServer, key: &str);

    /// Cleanup operation on delete (e.g., clear SecretRefManager references)
    fn on_delete(&self, _obj: &K, _ctx: &ProcessContext) {}

    /// Additional processing after resource change (e.g., Secret's cascading requeue)
    fn on_change(&self, _obj: &K, _ctx: &ProcessContext) {}

    /// Get cached object from ConfigServer (used for delete detection)
    fn get(&self, cs: &ConfigServer, key: &str) -> Option<K>;
}

/// Context for processor methods
pub struct ProcessContext<'a> {
    pub config_server: &'a ConfigServer,
    pub metadata_filter: Option<&'a MetadataFilterConfig>,
    pub namespace_filter: Option<&'a Vec<String>>,
    pub requeue_registry: &'a RequeueRegistry,
}

impl<'a> ProcessContext<'a> {
    /// Create a new ProcessContext
    pub fn new(
        config_server: &'a ConfigServer,
        metadata_filter: Option<&'a MetadataFilterConfig>,
        namespace_filter: Option<&'a Vec<String>>,
        requeue_registry: &'a RequeueRegistry,
    ) -> Self {
        Self {
            config_server,
            metadata_filter,
            namespace_filter,
            requeue_registry,
        }
    }

    /// Get Secret cache list
    pub fn list_secrets(&self) -> ListData<Secret> {
        self.config_server.secrets.list_owned()
    }

    /// Get SecretRefManager reference
    pub fn secret_ref_manager(&self) -> &SecretRefManager {
        &self.config_server.secret_ref_manager
    }

    /// Get RequeueRegistry reference (for triggering other resource reprocessing)
    pub fn requeue_registry(&self) -> &RequeueRegistry {
        self.requeue_registry
    }
}

/// Processing configuration passed during spawn
#[derive(Clone, Default)]
pub struct ProcessConfig {
    pub metadata_filter: Option<MetadataFilterConfig>,
}

/// Cross-resource requeue registry
///
/// Allows one resource's Processor to trigger reprocessing of another resource
/// by enqueueing keys to their respective workqueues.
pub struct RequeueRegistry {
    /// kind -> workqueue
    queues: RwLock<HashMap<&'static str, Arc<Workqueue>>>,
}

impl RequeueRegistry {
    /// Create a new RequeueRegistry
    pub fn new() -> Self {
        Self {
            queues: RwLock::new(HashMap::new()),
        }
    }

    /// Register a resource's workqueue (called when ResourceController starts)
    pub fn register(&self, kind: &'static str, queue: Arc<Workqueue>) {
        self.queues.write().unwrap().insert(kind, queue);
        tracing::debug!(
            component = "requeue_registry",
            kind = kind,
            "Registered workqueue for kind"
        );
    }

    /// Enqueue key to the specified resource's workqueue
    pub fn enqueue(&self, kind: &str, key: String) {
        let queue = {
            let queues = self.queues.read().unwrap();
            queues.get(kind).cloned()
        };

        if let Some(queue) = queue {
            let key_for_log = key.clone();

            tokio::spawn(async move {
                queue.enqueue(key).await;
            });

            tracing::debug!(
                component = "requeue_registry",
                target_kind = kind,
                key = %key_for_log,
                "Enqueued cross-resource requeue"
            );
        } else {
            tracing::warn!(
                component = "requeue_registry",
                target_kind = kind,
                "Cannot enqueue: workqueue not registered for kind"
            );
        }
    }
}

impl Default for RequeueRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Helper functions
// ============================================================================

/// Format secret key from namespace and name
pub fn format_secret_key(namespace: Option<&String>, name: &str) -> String {
    match namespace {
        Some(ns) => format!("{}/{}", ns, name),
        None => name.to_string(),
    }
}

/// Find a secret in the cache list
pub fn find_secret<'a>(
    secret_list: &'a ListData<Secret>,
    namespace: Option<&String>,
    name: &str,
) -> Option<&'a Secret> {
    secret_list
        .data
        .iter()
        .find(|s| s.metadata.namespace.as_ref() == namespace && s.metadata.name.as_deref() == Some(name))
}

/// Create a resource key from object: "namespace/name" or "name" for cluster-scoped
pub fn make_resource_key<K>(obj: &K) -> String
where
    K: Resource,
{
    let name = obj.meta().name.as_deref().unwrap_or("");
    match obj.meta().namespace.as_ref() {
        Some(ns) => format!("{}/{}", ns, name),
        None => name.to_string(),
    }
}
