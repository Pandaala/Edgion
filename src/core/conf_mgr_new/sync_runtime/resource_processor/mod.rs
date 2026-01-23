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

// Re-export utility functions from old conf_mgr (still shared)
pub use crate::core::conf_mgr::conf_center::sync_runtime::resource_processor::{
    find_secret, format_secret_key, make_resource_key,
};
