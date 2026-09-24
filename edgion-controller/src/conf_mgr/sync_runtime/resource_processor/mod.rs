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
pub mod credential;
pub mod credential_source_state;
pub mod edgion_config_data_utils;
pub mod frontend_ca_ref;
pub mod gateway_route_index;
mod handler;
pub mod handlers;
pub mod listener_port_manager;
pub mod namespace_store;
mod processor;
pub mod proxy_protocol_listener_policy;
pub mod raw_store;
pub mod ref_grant;
pub mod ref_manager;
pub mod region_route_table;
pub mod route_delegation_index;
pub mod secret_utils;
pub mod service_ref;
pub mod status_diff;
pub mod status_observation;
pub mod status_ownership;
mod status_pipeline;
pub mod status_utils;
pub(crate) mod unknown_fields_stamp;

pub use context::HandlerContext;
pub use credential::{ResolveError, SecretMaterialResolver};
pub use handler::{ProcessResult, ProcessorHandler};
pub use processor::{extract_status_value, ProcessorObj, ResourceProcessor, WorkItemResult};
pub use raw_store::{KubeRawStore, LocalRawStore, RawStore};
// NOTE: `status_utils::condition_unknown` is intentionally NOT re-exported —
// no in-tree caller uses it because `EdgionAcmeStatus` uses upstream
// `k8s_openapi::Condition` (not our local `types::common::Condition`) and has
// its own `k8s_condition_unknown` helper.
pub use status_utils::{
    accepted_condition, condition_false, condition_reasons, condition_true, condition_types, resolved_refs_condition,
    set_parent_conditions_full, update_condition, AcceptedError, ResolvedRefsError,
};

pub use handlers::{
    BackendTlsPolicyHandler, ConfigMapHandler, EdgionAcmeHandler, EdgionBackendHandler,
    EdgionBackendTrafficPolicyHandler, EdgionConfigDataHandler, EdgionGatewayConfigHandler, EdgionPluginsHandler,
    EdgionStreamPluginsHandler, EdgionTlsHandler, EndpointSliceHandler, GatewayClassHandler, GatewayHandler,
    GrpcRouteHandler, HttpRouteHandler, LinkSysHandler, ReferenceGrantHandler, SecretHandler, ServiceHandler,
    TcpRouteHandler, TlsRouteHandler, UdpRouteHandler,
};

pub use ref_manager::{BidirectionalRefManager, RefManagerStats, RefValue, ResourceRef};

// Note: get_secret / get_global_secret_store are kept for Gateway-side fallback use.
// Controller code should use runtime.secret_store instead.
pub use secret_utils::{get_global_secret_store, get_secret, get_secret_by_name, SecretRefManager, SecretStore};

pub use ref_grant::{
    is_cross_ns_ref_allowed, run_post_cache_ready_revalidations, trigger_full_cross_ns_revalidation,
    trigger_gateway_route_revalidation, trigger_gateway_secret_revalidation, validate_grpc_route_if_enabled,
    validate_http_route_if_enabled, validate_tcp_route_if_enabled, validate_tls_route_if_enabled,
    validate_udp_route_if_enabled, CrossNamespaceRefManager, CrossNamespaceValidator, CrossNsRevalidationListener,
    ReferenceGrantChangedEvent, ReferenceGrantStore, RevalidationListener,
};

pub use listener_port_manager::{make_conflict_key, ConflictKey, ListenerPortManager, ListenerRef, PortKey};
pub use proxy_protocol_listener_policy::{
    ProxyProtocolListenerContribution, ProxyProtocolListenerPolicyManager, ProxyProtocolPolicyDeclaration,
};

pub use attached_route_tracker::AttachedRouteTracker;

pub use gateway_route_index::GatewayRouteIndex;

pub use route_delegation_index::RouteDelegationIndex;

pub use service_ref::{new_service_ref_manager, ServiceRefManager};

pub use backend_tls_policy_ref::{make_target_conflict_key, BackendTLSPolicyState};

pub use namespace_store::NamespaceStore;

pub use edgion_config_data_utils::EdgionConfigDataStore;

// region_route_table: empty module; no types exported.

// ============================================================================
// Utility functions
// ============================================================================

use kube::Resource;

/// Format secret key from namespace and name
pub fn format_secret_key(namespace: Option<&String>, name: &str) -> String {
    match namespace {
        Some(ns) => format!("{}/{}", ns, name),
        None => name.to_string(),
    }
}

/// Prefix that distinguishes ConfigMap material dependencies from Secrets.
pub(crate) const CONFIG_MAP_REF_KEY_PREFIX: &str = "ConfigMap/";

/// Format a ConfigMap dependency key without colliding with a same-name Secret.
pub(crate) fn format_configmap_ref_key(namespace: Option<&String>, name: &str) -> String {
    format!("{CONFIG_MAP_REF_KEY_PREFIX}{}", format_secret_key(namespace, name))
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
