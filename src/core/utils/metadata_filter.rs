//! Metadata filter utilities for Kubernetes resources
//!
//! This module provides functions to clean up K8s resource metadata,
//! removing unnecessary fields to reduce memory usage.
//!
//! ## Typical memory savings
//!
//! | Field | Size | Impact |
//! |-------|------|--------|
//! | `kubectl.kubernetes.io/last-applied-configuration` | ~KB | High - contains full JSON backup |
//! | `metadata.managedFields` | ~100B-KB | Medium - field management history |
//! | `meta.helm.sh/release-*` | ~10B each | Low - Helm metadata |
//!
//! For a typical HTTPRoute, this can reduce size from ~4KB to ~800B (~80% savings).

use crate::core::conf_mgr::MetadataFilterConfig;
use kube::Resource;

/// Clean metadata by removing blocked annotations and managedFields
///
/// This function modifies the resource in-place, removing:
/// - Annotations listed in `config.blocked_annotations`
/// - `managedFields` if `config.remove_managed_fields` is true
///
/// # Example
///
/// ```ignore
/// use crate::core::utils::clean_metadata;
/// use crate::core::conf_mgr::MetadataFilterConfig;
///
/// let config = MetadataFilterConfig::default();
/// let mut route: HTTPRoute = /* ... */;
/// clean_metadata(&mut route, &config);
/// ```
pub fn clean_metadata<T: Resource>(resource: &mut T, config: &MetadataFilterConfig) {
    let meta = resource.meta_mut();

    // Remove blocked annotations
    if let Some(ref mut annotations) = meta.annotations {
        for blocked in &config.blocked_annotations {
            annotations.remove(blocked);
        }
        // Clean up empty annotations map
        if annotations.is_empty() {
            meta.annotations = None;
        }
    }

    // Remove managedFields
    if config.remove_managed_fields {
        meta.managed_fields = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::ConfigMap;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ManagedFieldsEntry, ObjectMeta};
    use std::collections::BTreeMap;

    fn create_test_configmap() -> ConfigMap {
        let mut annotations = BTreeMap::new();
        annotations.insert(
            "kubectl.kubernetes.io/last-applied-configuration".to_string(),
            r#"{"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"test"}}"#.to_string(),
        );
        annotations.insert("meta.helm.sh/release-name".to_string(), "my-release".to_string());
        annotations.insert("meta.helm.sh/release-namespace".to_string(), "default".to_string());
        annotations.insert("custom-annotation".to_string(), "keep-me".to_string());

        ConfigMap {
            metadata: ObjectMeta {
                name: Some("test-configmap".to_string()),
                namespace: Some("default".to_string()),
                annotations: Some(annotations),
                managed_fields: Some(vec![ManagedFieldsEntry {
                    manager: Some("kubectl".to_string()),
                    operation: Some("Apply".to_string()),
                    ..Default::default()
                }]),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn test_clean_metadata_default_config() {
        let mut cm = create_test_configmap();
        let config = MetadataFilterConfig::default();

        clean_metadata(&mut cm, &config);

        // Check blocked annotations are removed
        let annotations = cm.metadata.annotations.as_ref().unwrap();
        assert!(!annotations.contains_key("kubectl.kubernetes.io/last-applied-configuration"));
        assert!(!annotations.contains_key("meta.helm.sh/release-name"));
        assert!(!annotations.contains_key("meta.helm.sh/release-namespace"));

        // Check custom annotation is preserved
        assert_eq!(annotations.get("custom-annotation"), Some(&"keep-me".to_string()));

        // Check managedFields is removed
        assert!(cm.metadata.managed_fields.is_none());
    }

    #[test]
    fn test_clean_metadata_custom_config() {
        let mut cm = create_test_configmap();
        let config = MetadataFilterConfig {
            blocked_annotations: vec!["custom-annotation".to_string()],
            remove_managed_fields: false,
        };

        clean_metadata(&mut cm, &config);

        // Check custom annotation is removed
        let annotations = cm.metadata.annotations.as_ref().unwrap();
        assert!(!annotations.contains_key("custom-annotation"));

        // Check default blocked annotations are preserved (not in config)
        assert!(annotations.contains_key("kubectl.kubernetes.io/last-applied-configuration"));

        // Check managedFields is preserved
        assert!(cm.metadata.managed_fields.is_some());
    }

    #[test]
    fn test_clean_metadata_empty_annotations_cleanup() {
        let mut annotations = BTreeMap::new();
        annotations.insert(
            "kubectl.kubernetes.io/last-applied-configuration".to_string(),
            "{}".to_string(),
        );

        let mut cm = ConfigMap {
            metadata: ObjectMeta {
                name: Some("test".to_string()),
                annotations: Some(annotations),
                ..Default::default()
            },
            ..Default::default()
        };

        let config = MetadataFilterConfig {
            blocked_annotations: vec!["kubectl.kubernetes.io/last-applied-configuration".to_string()],
            remove_managed_fields: true,
        };

        clean_metadata(&mut cm, &config);

        // Annotations map should be None when empty
        assert!(cm.metadata.annotations.is_none());
    }

    #[test]
    fn test_clean_metadata_no_annotations() {
        let mut cm = ConfigMap {
            metadata: ObjectMeta {
                name: Some("test".to_string()),
                annotations: None,
                managed_fields: Some(vec![]),
                ..Default::default()
            },
            ..Default::default()
        };

        let config = MetadataFilterConfig::default();

        // Should not panic
        clean_metadata(&mut cm, &config);

        assert!(cm.metadata.annotations.is_none());
        assert!(cm.metadata.managed_fields.is_none());
    }
}
