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

use crate::config::conf_center::MetadataFilterConfig;
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
/// use crate::conf_mgr::metadata_filter::clean_metadata;
/// use crate::config::conf_center::MetadataFilterConfig;
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
#[path = "../../tests/unit/conf_mgr/metadata_filter_test.rs"]
mod tests;
