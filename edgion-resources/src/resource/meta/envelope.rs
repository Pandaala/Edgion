//! Resource envelope extraction
//!
//! Pulls the `kind`, `namespace`, and `name` identity fields out of raw
//! YAML/JSON resource documents before they have been typed into a specific
//! schema. Used by the controller's admin API and file-system storage, and by
//! the file watcher startup/reload scan and the CLI's apply/delete commands to dispatch on `kind` ahead of typed
//! deserialization.

/// Resource metadata extracted from YAML/JSON content.
///
/// This is the pre-typing "envelope header" of a Kubernetes-style document.
/// It is intentionally distinct from `kube::core::ObjectMeta` (which does not
/// carry `kind`) and from typed resource wrappers (which require knowing the
/// schema up front).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceMetadata {
    pub kind: Option<String>,
    pub namespace: Option<String>,
    pub name: Option<String>,
}

/// Extract `kind`, `namespace`, and `name` from YAML or JSON content without
/// requiring a specific type.
///
/// Tries YAML first, then falls back to JSON. Returns `None` if neither parses
/// or if the document has no `metadata` key. The value need not be a mapping;
/// this extraction is not typed resource validation. Individual fields inside the
/// returned `ResourceMetadata` may still be `None` when absent.
pub fn extract_resource_metadata(content: &str) -> Option<ResourceMetadata> {
    let value: yaml_serde::Value = match yaml_serde::from_str(content) {
        Ok(v) => v,
        Err(_) => serde_json::from_str(content).ok()?,
    };

    let kind = value.get("kind").and_then(|v| v.as_str()).map(|s| s.to_string());

    let metadata = value.get("metadata")?;

    let namespace = metadata
        .get("namespace")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let name = metadata.get("name").and_then(|v| v.as_str()).map(|s| s.to_string());

    Some(ResourceMetadata { kind, namespace, name })
}

#[cfg(test)]
#[path = "../../../tests/unit/resource/meta/envelope_test.rs"]
mod tests;
