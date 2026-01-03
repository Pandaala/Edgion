//! ReferenceGrant module
//!
//! This module provides global storage and permission checking for ReferenceGrant resources.
//!
//! ReferenceGrants allow cross-namespace references in Gateway API by explicitly
//! defining trust relationships between namespaces.

mod store;
mod conf_handler_impl;
mod validator;
pub mod events;

pub use store::{ReferenceGrantStore, get_global_reference_grant_store};
pub use conf_handler_impl::create_reference_grant_handler;
pub use validator::{
    CrossNamespaceValidator,
    validate_http_route_if_enabled,
    validate_grpc_route_if_enabled,
    validate_tcp_route_if_enabled,
    validate_udp_route_if_enabled,
    validate_tls_route_if_enabled,
};
pub use events::{ReferenceGrantChangedEvent, RevalidationListener, get_global_dispatcher};
