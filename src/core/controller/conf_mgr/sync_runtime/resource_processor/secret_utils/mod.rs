//! Secret utilities for resource processing
//!
//! This module provides:
//! - `SecretRefManager`: Manages references between Secrets and dependent resources
//! - `SecretStore`: Global store for Secret data (used by TLS callbacks)

mod secret_ref;
mod secret_store;

pub use secret_ref::{new_secret_ref_manager, SecretRefManager};
pub use secret_store::{
    get_global_secret_store, get_secret, get_secret_by_name, replace_all_secrets, update_secrets, SecretStore,
};

// Re-export from the generic ref_manager module for backward compatibility
pub use super::ref_manager::{RefManagerStats, ResourceRef};
