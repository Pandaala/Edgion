//! Enhanced Resource Processor Module
//!
//! This module provides an enhanced `ResourceProcessor<T>` that:
//! - Holds `ServerCache<T>` for resource storage
//! - Manages workqueue for event processing
//! - Provides lifecycle methods (on_init, on_apply, on_delete, etc.)
//!
//! ## Design
//!
//! - `ResourceProcessor<T>`: Core struct holding cache, workqueue, and handler
//! - `ProcessorHandler<T>`: Trait for resource-specific processing logic
//! - `ProcessorObj`: Object-safe trait for registry management
//! - `HandlerContext`: Context passed to handler methods

mod context;
mod handler;
pub mod handlers;
mod processor;
pub mod secret_utils;

pub use context::HandlerContext;
pub use handler::{ProcessResult, ProcessorHandler};
pub use processor::{ProcessorObj, ResourceProcessor};

// Re-export handlers
pub use handlers::{
    BackendTlsPolicyHandler, EdgionGatewayConfigHandler, EdgionPluginsHandler, EdgionStreamPluginsHandler,
    EdgionTlsHandler, EndpointSliceHandler, EndpointsHandler, GatewayClassHandler, GatewayHandler, GrpcRouteHandler,
    HttpRouteHandler, LinkSysHandler, PluginMetadataHandler, ReferenceGrantHandler, SecretHandler, ServiceHandler,
    TcpRouteHandler, TlsRouteHandler, UdpRouteHandler,
};

// Re-export secret utilities from local module
pub use secret_utils::{
    get_global_secret_store, get_secret, get_secret_by_name, replace_all_secrets, update_secrets, RefManagerStats,
    ResourceRef, SecretRefManager, SecretStore,
};

// ============================================================================
// Utility functions (previously in old conf_mgr)
// ============================================================================

use kube::Resource;

/// Format secret key from namespace and name
pub fn format_secret_key(namespace: Option<&String>, name: &str) -> String {
    match namespace {
        Some(ns) => format!("{}/{}", ns, name),
        None => name.to_string(),
    }
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
