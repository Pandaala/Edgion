//! Backend TLS Policy module
//!
//! This module provides functionality for managing BackendTLSPolicy resources.

pub mod backend_tls_policy_store;
pub mod conf_handler_impl;

pub use backend_tls_policy_store::{get_global_backend_tls_policy_store, BackendTLSPolicyStore};
pub use conf_handler_impl::create_backend_tls_policy_handler;

