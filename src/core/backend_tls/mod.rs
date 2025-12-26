//! Backend TLS Policy module
//!
//! This module provides functionality for managing BackendTLSPolicy resources.

pub mod backend_tls_policy_store;

pub use backend_tls_policy_store::{get_global_backend_tls_policy_store, BackendTLSPolicyStore};

