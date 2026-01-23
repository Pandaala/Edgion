//! Secret utilities for resource processing
//!
//! This module provides:
//! - `SecretRefManager`: Manages references between Secrets and dependent resources
//! - `SecretStore`: Global store for Secret data (used by TLS callbacks)

mod secret_ref;
mod secret_store;

pub use secret_ref::{RefManagerStats, ResourceRef, SecretRefManager};
pub use secret_store::{
    get_global_secret_store, get_secret, get_secret_by_name, replace_all_secrets, update_secrets, SecretStore,
};
