//! Service Reference Manager
//!
//! Tracks which Route resources reference which Services as backends.
//! When a Service changes, dependent routes are requeued so their
//! ResolvedRefs status gets re-evaluated.
//!
//! Uses the same `BidirectionalRefManager` as SecretRefManager.

use super::secret_utils::SecretRefManager;

pub type ServiceRefManager = SecretRefManager;

/// Create a new `ServiceRefManager` with the canonical component name.
pub fn new_service_ref_manager() -> ServiceRefManager {
    ServiceRefManager::new()
}
