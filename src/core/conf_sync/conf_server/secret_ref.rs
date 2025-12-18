//! Secret Reference Manager
//! 
//! Manages the reference relationship between Secrets and resources that depend on them.
//! Uses a bidirectional index for efficient lookups and updates.

use crate::types::ResourceKind;
use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

/// Represents a resource that references a Secret
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceRef {
    pub kind: ResourceKind,
    pub namespace: Option<String>,
    pub name: String,
}

impl ResourceRef {
    /// Create a new ResourceRef
    pub fn new(kind: ResourceKind, namespace: Option<String>, name: String) -> Self {
        Self {
            kind,
            namespace,
            name,
        }
    }

    /// Generate a unique key for this resource
    /// Format: "kind/namespace/name" or "kind//name" (for cluster-scoped)
    pub fn key(&self) -> String {
        match &self.namespace {
            Some(ns) => format!("{:?}/{}/{}", self.kind, ns, self.name),
            None => format!("{:?}//{}", self.kind, self.name),
        }
    }

    /// Create ResourceRef from a key string
    pub fn from_key(key: &str) -> Option<Self> {
        let parts: Vec<&str> = key.splitn(3, '/').collect();
        if parts.len() < 3 {
            return None;
        }

        let kind = match parts[0] {
            "EdgionTls" => ResourceKind::EdgionTls,
            _ => return None,
        };

        let namespace = if parts[1].is_empty() {
            None
        } else {
            Some(parts[1].to_string())
        };

        Some(Self {
            kind,
            namespace,
            name: parts[2].to_string(),
        })
    }
}

/// Manages Secret references and dependencies
pub struct SecretRefManager {
    /// Forward index: Secret key -> Resources that reference it
    refs: RwLock<HashMap<String, HashSet<ResourceRef>>>,

    /// Reverse index: Resource key -> Secret keys it depends on
    dependencies: RwLock<HashMap<String, HashSet<String>>>,
}

impl SecretRefManager {
    /// Create a new SecretRefManager
    pub fn new() -> Self {
        Self {
            refs: RwLock::new(HashMap::new()),
            dependencies: RwLock::new(HashMap::new()),
        }
    }

    /// Add a reference: resource depends on secret
    /// This is idempotent - adding the same reference multiple times is safe
    pub fn add_ref(&self, secret_key: String, resource_ref: ResourceRef) {
        let resource_key = resource_ref.key();

        // Add to forward index
        {
            let mut refs = self.refs.write().unwrap();
            refs.entry(secret_key.clone())
                .or_insert_with(HashSet::new)
                .insert(resource_ref.clone());
        }

        // Add to reverse index
        {
            let mut deps = self.dependencies.write().unwrap();
            deps.entry(resource_key.clone())
                .or_insert_with(HashSet::new)
                .insert(secret_key.clone());
        }

        tracing::debug!(
            secret_key = %secret_key,
            resource = %resource_key,
            "Added secret reference"
        );
    }

    /// Remove a specific reference
    pub fn remove_ref(&self, secret_key: &str, resource_ref: &ResourceRef) {
        let resource_key = resource_ref.key();

        // Remove from forward index
        {
            let mut refs = self.refs.write().unwrap();
            if let Some(resource_set) = refs.get_mut(secret_key) {
                resource_set.remove(resource_ref);
                if resource_set.is_empty() {
                    refs.remove(secret_key);
                }
            }
        }

        // Remove from reverse index
        {
            let mut deps = self.dependencies.write().unwrap();
            if let Some(secret_set) = deps.get_mut(&resource_key) {
                secret_set.remove(secret_key);
                if secret_set.is_empty() {
                    deps.remove(&resource_key);
                }
            }
        }

        tracing::debug!(
            secret_key = %secret_key,
            resource = %resource_key,
            "Removed secret reference"
        );
    }

    /// Get all resources that reference a specific Secret
    pub fn get_refs(&self, secret_key: &str) -> Vec<ResourceRef> {
        let refs = self.refs.read().unwrap();
        refs.get(secret_key)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Get all Secrets that a resource depends on
    pub fn get_dependencies(&self, resource_key: &str) -> Vec<String> {
        let deps = self.dependencies.read().unwrap();
        deps.get(resource_key)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Clear all references for a resource (when resource is deleted)
    /// Returns the list of Secret keys that were referenced
    pub fn clear_resource_refs(&self, resource_ref: &ResourceRef) -> Vec<String> {
        let resource_key = resource_ref.key();

        // Get all secrets this resource depends on
        let secret_keys = {
            let mut deps = self.dependencies.write().unwrap();
            deps.remove(&resource_key).unwrap_or_default()
        };

        // Remove this resource from all those secrets' reference lists
        {
            let mut refs = self.refs.write().unwrap();
            for secret_key in &secret_keys {
                if let Some(resource_set) = refs.get_mut(secret_key) {
                    resource_set.remove(resource_ref);
                    if resource_set.is_empty() {
                        refs.remove(secret_key);
                    }
                }
            }
        }

        if !secret_keys.is_empty() {
            tracing::info!(
                resource = %resource_key,
                secret_count = secret_keys.len(),
                "Cleared all secret references for resource"
            );
        }

        secret_keys.into_iter().collect()
    }

    /// Get statistics about the reference manager
    pub fn stats(&self) -> RefManagerStats {
        let refs = self.refs.read().unwrap();
        let deps = self.dependencies.read().unwrap();

        RefManagerStats {
            secret_count: refs.len(),
            resource_count: deps.len(),
            total_references: refs.values().map(|set| set.len()).sum(),
        }
    }
}

impl Default for SecretRefManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics about the reference manager
#[derive(Debug, Clone)]
pub struct RefManagerStats {
    /// Number of unique Secrets being referenced
    pub secret_count: usize,
    /// Number of unique resources with dependencies
    pub resource_count: usize,
    /// Total number of reference relationships
    pub total_references: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_ref_key() {
        let ref1 = ResourceRef::new(
            ResourceKind::EdgionTls,
            Some("default".to_string()),
            "my-tls".to_string(),
        );
        assert_eq!(ref1.key(), "EdgionTls/default/my-tls");

        let ref2 = ResourceRef::new(
            ResourceKind::EdgionTls,
            None,
            "cluster-tls".to_string(),
        );
        assert_eq!(ref2.key(), "EdgionTls//cluster-tls");
    }

    #[test]
    fn test_add_and_get_ref() {
        let manager = SecretRefManager::new();
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
    fn test_remove_ref() {
        let manager = SecretRefManager::new();
        let resource = ResourceRef::new(
            ResourceKind::EdgionTls,
            Some("default".to_string()),
            "my-tls".to_string(),
        );

        manager.add_ref("default/my-cert".to_string(), resource.clone());
        manager.remove_ref("default/my-cert", &resource);

        let refs = manager.get_refs("default/my-cert");
        assert_eq!(refs.len(), 0);

        let deps = manager.get_dependencies(&resource.key());
        assert_eq!(deps.len(), 0);
    }

    #[test]
    fn test_clear_resource_refs() {
        let manager = SecretRefManager::new();
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

        let refs1 = manager.get_refs("default/cert1");
        let refs2 = manager.get_refs("default/cert2");
        assert_eq!(refs1.len(), 0);
        assert_eq!(refs2.len(), 0);
    }

    #[test]
    fn test_multiple_resources_same_secret() {
        let manager = SecretRefManager::new();
        
        let res1 = ResourceRef::new(
            ResourceKind::EdgionTls,
            Some("default".to_string()),
            "tls1".to_string(),
        );
        let res2 = ResourceRef::new(
            ResourceKind::EdgionTls,
            Some("default".to_string()),
            "tls2".to_string(),
        );

        manager.add_ref("default/my-cert".to_string(), res1.clone());
        manager.add_ref("default/my-cert".to_string(), res2.clone());

        let refs = manager.get_refs("default/my-cert");
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn test_idempotent_add() {
        let manager = SecretRefManager::new();
        let resource = ResourceRef::new(
            ResourceKind::EdgionTls,
            Some("default".to_string()),
            "my-tls".to_string(),
        );

        manager.add_ref("default/my-cert".to_string(), resource.clone());
        manager.add_ref("default/my-cert".to_string(), resource.clone());
        manager.add_ref("default/my-cert".to_string(), resource.clone());

        let refs = manager.get_refs("default/my-cert");
        assert_eq!(refs.len(), 1);
    }
}

