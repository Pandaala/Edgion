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

pub mod attached_route_tracker;
pub mod backend_tls_policy_ref;
pub mod configmap_utils;
mod context;
pub mod gateway_route_index;
mod handler;
pub mod handlers;
pub mod listener_port_manager;
pub mod namespace_store;
mod processor;
pub mod raw_store;
pub mod ref_grant;
pub mod ref_manager;
pub mod secret_utils;
pub mod service_ref;
pub mod status_diff;
pub mod status_utils;

pub use context::HandlerContext;
pub use handler::{ProcessResult, ProcessorHandler};
pub use processor::{extract_status_value, ProcessorObj, ResourceProcessor, WorkItemResult};
pub use raw_store::{FsRawStore, KubeRawStore, RawStore};
pub use status_utils::{
    accepted_condition, condition_false, condition_reasons, condition_true, condition_types, now_rfc3339,
    resolved_refs_condition, set_parent_conditions, set_parent_conditions_full, update_condition, AcceptedError,
    ResolvedRefsError,
};

// Re-export handlers
pub use handlers::{
    BackendTlsPolicyAlpha3Handler, BackendTlsPolicyHandler, ConfigMapHandler, EdgionAcmeHandler,
    EdgionGatewayConfigHandler, EdgionPluginsHandler, EdgionStreamPluginsHandler, EdgionTlsHandler,
    EndpointSliceHandler, EndpointsHandler, GatewayClassHandler, GatewayHandler, GrpcRouteHandler, HttpRouteHandler,
    LinkSysHandler, PluginMetadataHandler, ReferenceGrantBetaHandler, ReferenceGrantHandler, SecretHandler,
    ServiceHandler, TcpRouteHandler, TlsRouteAlpha3Handler, TlsRouteHandler, UdpRouteHandler,
};

// Re-export generic ref_manager types
pub use ref_manager::{BidirectionalRefManager, RefManagerStats, RefValue, ResourceRef};

// Re-export secret utilities from local module
// Note: get_secret / get_global_secret_store are kept for Gateway-side fallback use.
// Controller code should use runtime.secret_store instead.
pub use secret_utils::{get_global_secret_store, get_secret, get_secret_by_name, SecretRefManager, SecretStore};

// Re-export ref_grant utilities
pub use ref_grant::{
    is_cross_ns_ref_allowed, run_post_cache_ready_revalidations, trigger_full_cross_ns_revalidation,
    trigger_gateway_route_revalidation, trigger_gateway_secret_revalidation, validate_grpc_route_if_enabled,
    validate_http_route_if_enabled, validate_tcp_route_if_enabled, validate_tls_route_if_enabled,
    validate_udp_route_if_enabled, CrossNamespaceRefManager, CrossNamespaceValidator, CrossNsResourceRef,
    CrossNsRevalidationListener, ReferenceGrantChangedEvent, ReferenceGrantStore, RevalidationListener,
};

// Re-export listener_port_manager utilities
pub use listener_port_manager::{make_conflict_key, ConflictKey, ListenerPortManager, ListenerRef, PortKey};

// Re-export attached_route_tracker utilities
pub use attached_route_tracker::{AttachedRouteTracker, Attachment, RouteRef};

// Re-export gateway_route_index utilities
pub use gateway_route_index::GatewayRouteIndex;

// Re-export service_ref utilities
pub use service_ref::{new_service_ref_manager, ServiceRefManager};

// Re-export backend_tls_policy_ref utilities
pub use backend_tls_policy_ref::{make_target_conflict_key, BackendTLSPolicyState};

// Re-export namespace_store utilities
pub use namespace_store::NamespaceStore;

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
