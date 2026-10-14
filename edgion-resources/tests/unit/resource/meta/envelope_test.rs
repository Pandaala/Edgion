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

#[test]
fn envelope_preserves_yaml_alias_and_tag_semantics() {
    for document in [
        "resourceType: &type HTTPRoute\nkind: *type\nmetadata: &meta {name: route}\nother: *meta",
        "kind: !!str HTTPRoute\nmetadata: {name: route}",
        "kind: !ResourceType HTTPRoute\nmetadata: {name: route}",
    ] {
        let metadata = extract_resource_metadata(document).unwrap();
        assert_eq!(metadata.kind.as_deref(), Some("HTTPRoute"));
        assert_eq!(metadata.name.as_deref(), Some("route"));
    }
}

#[test]
fn envelope_requires_metadata_presence_without_validating_its_shape() {
    assert!(extract_resource_metadata("kind: HTTPRoute").is_none());
    for value in ["null", "42", "[]", "{}"] {
        let metadata = extract_resource_metadata(&format!("kind: HTTPRoute\nmetadata: {value}")).unwrap();
        assert_eq!(metadata.kind.as_deref(), Some("HTTPRoute"));
        assert_eq!(metadata.name, None);
        assert_eq!(metadata.namespace, None);
    }
    for value in ["null", "42", "true", "[]", "{}"] {
        let metadata = extract_resource_metadata(&format!("kind: {value}\nmetadata: {{}}")).unwrap();
        assert_eq!(metadata.kind, None);
    }
}

#[test]
fn envelope_keeps_duplicate_key_parser_behavior() {
    assert!(extract_resource_metadata("kind: Service\nkind: HTTPRoute\nmetadata: {}").is_none());
    // The JSON fallback also targets serde_yaml::Value and rejects duplicates.
    assert!(extract_resource_metadata(r#"{"kind":"Service","kind":"HTTPRoute","metadata":{}}"#).is_none());
}
