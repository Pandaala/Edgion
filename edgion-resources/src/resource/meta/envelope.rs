//! Resource envelope extraction
//!
//! Pulls the `kind`, `namespace`, and `name` identity fields out of raw
//! YAML/JSON resource documents before they have been typed into a specific
//! schema. Used by the controller's admin API and file-system storage, and by
//! the CLI's apply/delete commands to dispatch on `kind` ahead of typed
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
/// or if the document has no `metadata` object. Individual fields inside the
/// returned `ResourceMetadata` may still be `None` when absent.
pub fn extract_resource_metadata(content: &str) -> Option<ResourceMetadata> {
    let value: serde_yaml::Value = match serde_yaml::from_str(content) {
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
mod tests {
    use super::*;

    #[test]
    fn extract_resource_metadata_from_yaml() {
        let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: test-route
  namespace: default
spec:
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /
"#;
        let metadata = extract_resource_metadata(yaml).unwrap();
        assert_eq!(metadata.kind, Some("HTTPRoute".to_string()));
        assert_eq!(metadata.namespace, Some("default".to_string()));
        assert_eq!(metadata.name, Some("test-route".to_string()));
    }

    #[test]
    fn extract_resource_metadata_from_json() {
        let json = r#"{
  "apiVersion": "gateway.networking.k8s.io/v1",
  "kind": "GatewayClass",
  "metadata": {
    "name": "test-class"
  }
}"#;
        let metadata = extract_resource_metadata(json).unwrap();
        assert_eq!(metadata.kind, Some("GatewayClass".to_string()));
        assert_eq!(metadata.namespace, None);
        assert_eq!(metadata.name, Some("test-class".to_string()));
    }

    #[test]
    fn extract_resource_metadata_missing_fields() {
        let yaml = r#"
apiVersion: v1
kind: Service
metadata:
  name: test-service
"#;
        let metadata = extract_resource_metadata(yaml).unwrap();
        assert_eq!(metadata.kind, Some("Service".to_string()));
        assert_eq!(metadata.namespace, None);
        assert_eq!(metadata.name, Some("test-service".to_string()));
    }

    #[test]
    fn extract_resource_metadata_invalid_content() {
        let invalid = "not yaml or json content";
        assert!(extract_resource_metadata(invalid).is_none());
    }
}
