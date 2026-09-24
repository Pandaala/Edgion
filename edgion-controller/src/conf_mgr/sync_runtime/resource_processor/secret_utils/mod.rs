//! Secret utilities for resource processing
//!
//! This module provides:
//! - `SecretRefManager`: Manages kind-preserving Secret/ConfigMap material dependencies
//! - `SecretStore`: Global store for Secret data (used by TLS callbacks)

mod secret_ref;

pub use secret_ref::{new_secret_ref_manager, SecretRefManager};
// Global Secret store lives in edgion_common::secret (shared with the data plane);
// re-exported here for controller call sites.
pub use edgion_common::secret::{get_global_secret_store, get_secret, get_secret_by_name, SecretStore};
