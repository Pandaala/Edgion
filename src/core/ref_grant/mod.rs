//! ReferenceGrant module
//!
//! This module provides global storage and permission checking for ReferenceGrant resources.
//!
//! ReferenceGrants allow cross-namespace references in Gateway API by explicitly
//! defining trust relationships between namespaces.

mod conf_handler_impl;
pub mod cross_ns_ref_manager;
pub mod events;
mod revalidation_listener;
mod store;
mod validator;

pub use conf_handler_impl::create_reference_grant_handler;
pub use cross_ns_ref_manager::{get_global_cross_ns_ref_manager, CrossNamespaceRefManager, CrossNsResourceRef};
pub use events::{get_global_dispatcher, ReferenceGrantChangedEvent, RevalidationListener};
pub use revalidation_listener::CrossNsRevalidationListener;
pub use store::{get_global_reference_grant_store, ReferenceGrantStore};
pub use validator::{
    validate_grpc_route_if_enabled, validate_http_route_if_enabled, validate_tcp_route_if_enabled,
    validate_tls_route_if_enabled, validate_udp_route_if_enabled, CrossNamespaceValidator,
};
