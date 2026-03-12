//! Service Reference Manager
//!
//! Tracks which Route resources reference which Services as backends.
//! When a Service changes, dependent routes are requeued so their
//! ResolvedRefs status gets re-evaluated.
//!
//! Uses the same `BidirectionalRefManager` as SecretRefManager.

use std::sync::LazyLock;

use super::secret_utils::SecretRefManager;

type ServiceRefManager = SecretRefManager;

static GLOBAL_SERVICE_REF_MANAGER: LazyLock<ServiceRefManager> = LazyLock::new(ServiceRefManager::new);

/// Get the global ServiceRefManager instance
pub fn get_service_ref_manager() -> &'static ServiceRefManager {
    &GLOBAL_SERVICE_REF_MANAGER
}
