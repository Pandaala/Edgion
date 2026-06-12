//! Secret utilities for resource processing
//!
//! This module provides:
//! - `SecretRefManager`: Manages references between Secrets and dependent resources
//! - `SecretStore`: Global store for Secret data (used by TLS callbacks)

mod secret_ref;

pub use secret_ref::{new_secret_ref_manager, SecretRefManager};
// The global Secret store now lives in `common::secret` (shared with the data
// plane). Re-export it from here to keep existing controller call sites stable.
pub use edgion_common::secret::{get_global_secret_store, get_secret, get_secret_by_name, SecretStore};

// Re-export from the generic ref_manager module for backward compatibility
pub use super::ref_manager::{RefManagerStats, ResourceRef};
