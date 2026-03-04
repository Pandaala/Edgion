//! Secret Reference Manager
//!
//! Type alias over the generic `BidirectionalRefManager` for tracking
//! Secret → Resource dependencies.

use super::super::ref_manager::{BidirectionalRefManager, ResourceRef};

/// Manages Secret references and dependencies.
///
/// - Forward: `secret_key → Set<ResourceRef>` (resources that depend on this secret)
/// - Reverse: `resource_key → Set<secret_key>` (secrets a resource depends on)
pub type SecretRefManager = BidirectionalRefManager<ResourceRef>;

/// Create a `SecretRefManager` with the canonical component name.
pub fn new_secret_ref_manager() -> SecretRefManager {
    SecretRefManager::with_component("secret_ref_manager")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ResourceKind;

    #[test]
    fn test_add_and_get_ref() {
        let manager = new_secret_ref_manager();
        let resource = ResourceRef::new(
            ResourceKind::EdgionTls,
            Some("default".to_string()),
            "my-tls".to_string(),
        );

        manager.add_ref("default/my-cert".to_string(), resource.clone());

        let refs = manager.get_refs("default/my-cert");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0], resource);

        let deps = manager.get_dependencies(&resource.key());
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0], "default/my-cert");
    }

    #[test]
    fn test_clear_resource_refs() {
        let manager = new_secret_ref_manager();
        let resource = ResourceRef::new(
            ResourceKind::EdgionTls,
            Some("default".to_string()),
            "my-tls".to_string(),
        );

        manager.add_ref("default/cert1".to_string(), resource.clone());
        manager.add_ref("default/cert2".to_string(), resource.clone());

        let cleared = manager.clear_resource_refs(&resource);
        assert_eq!(cleared.len(), 2);
        assert!(cleared.contains(&"default/cert1".to_string()));
        assert!(cleared.contains(&"default/cert2".to_string()));

        assert!(manager.get_refs("default/cert1").is_empty());
        assert!(manager.get_refs("default/cert2").is_empty());
    }

    #[test]
    fn test_idempotent_add() {
        let manager = new_secret_ref_manager();
        let resource = ResourceRef::new(
            ResourceKind::EdgionTls,
            Some("default".to_string()),
            "my-tls".to_string(),
        );

        manager.add_ref("default/my-cert".to_string(), resource.clone());
        manager.add_ref("default/my-cert".to_string(), resource.clone());
        manager.add_ref("default/my-cert".to_string(), resource.clone());

        assert_eq!(manager.get_refs("default/my-cert").len(), 1);
    }
}
