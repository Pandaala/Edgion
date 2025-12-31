//! ReferenceGrant module
//!
//! This module provides global storage and permission checking for ReferenceGrant resources.
//!
//! ReferenceGrants allow cross-namespace references in Gateway API by explicitly
//! defining trust relationships between namespaces.

mod store;
mod conf_handler_impl;

pub use store::{ReferenceGrantStore, get_global_reference_grant_store};
pub use conf_handler_impl::create_reference_grant_handler;
