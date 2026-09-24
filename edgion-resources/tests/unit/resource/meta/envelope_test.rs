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
