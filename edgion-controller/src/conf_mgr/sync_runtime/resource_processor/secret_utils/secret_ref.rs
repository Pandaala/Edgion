//! Credential Material Reference Manager
//!
//! Type alias over the generic `BidirectionalRefManager` for tracking
//! Secret/ConfigMap → Resource dependencies.

use super::super::ref_manager::{BidirectionalRefManager, ResourceRef};

/// Manages Secret and ConfigMap material references and dependencies.
///
/// - Secret key: `namespace/name`
/// - ConfigMap key: `ConfigMap/namespace/name`
/// - Forward: `material_key → Set<ResourceRef>`
/// - Reverse: `resource_key → Set<material_key>`
pub type SecretRefManager = BidirectionalRefManager<ResourceRef>;

/// Create a `SecretRefManager` with the canonical component name.
pub fn new_secret_ref_manager() -> SecretRefManager {
    SecretRefManager::with_component("secret_ref_manager")
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/secret_utils/secret_ref_test.rs"]
mod tests;
