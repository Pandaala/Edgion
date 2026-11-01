use super::*;

#[test]
fn operator_update_preserves_server_fields_without_restoring_current_spec() {
    let current = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {
            "name": "route",
            "namespace": "default",
            "uid": "uid-1",
            "resourceVersion": "42",
            "generation": 7,
            "managedFields": [{"manager": "controller"}],
            "creationTimestamp": "2026-07-15T00:00:00Z",
            "deletionTimestamp": "2026-07-16T00:00:00Z",
            "ownerReferences": [{"name": "owner"}],
            "finalizers": ["edgion.io/finalizer"],
            "labels": {"old": "label"},
            "annotations": {"old": "annotation"}
        },
        "spec": {
            "hostnames": ["old.example"],
            "resolvedListeners": [{"name": "runtime-only"}]
        },
        "status": {"conditions": [{"type": "Accepted", "status": "True"}]}
    });
    let requested = serde_json::json!({
        "apiVersion": "spoofed/v9",
        "kind": "Gateway",
        "metadata": {
            "name": "renamed",
            "namespace": "other",
            "uid": "spoofed",
            "resourceVersion": "999",
            "finalizers": [],
            "labels": {"new": "label"}
        },
        "spec": {"hostnames": ["new.example"]},
        "status": {"conditions": []},
        "data": {"operator": "field"}
    });

    let merged = merge_operator_update_values(current, requested).expect("merge must succeed");

    assert_eq!(merged["apiVersion"], "gateway.networking.k8s.io/v1");
    assert_eq!(merged["kind"], "HTTPRoute");
    assert_eq!(merged["metadata"]["name"], "route");
    assert_eq!(merged["metadata"]["namespace"], "default");
    assert_eq!(merged["metadata"]["uid"], "uid-1");
    assert_eq!(merged["metadata"]["resourceVersion"], "42");
    assert_eq!(merged["metadata"]["generation"], 7);
    assert_eq!(merged["metadata"]["managedFields"][0]["manager"], "controller");
    assert_eq!(merged["metadata"]["creationTimestamp"], "2026-07-15T00:00:00Z");
    assert_eq!(merged["metadata"]["deletionTimestamp"], "2026-07-16T00:00:00Z");
    assert_eq!(merged["metadata"]["ownerReferences"][0]["name"], "owner");
    assert_eq!(merged["metadata"]["finalizers"][0], "edgion.io/finalizer");
    assert_eq!(merged["metadata"]["labels"]["new"], "label");
    assert!(merged["metadata"].get("annotations").is_none());
    assert_eq!(merged["spec"]["hostnames"][0], "new.example");
    assert!(merged["spec"].get("resolvedListeners").is_none());
    assert_eq!(merged["status"]["conditions"][0]["type"], "Accepted");
    assert_eq!(merged["data"]["operator"], "field");
}

#[test]
fn non_kubernetes_yaml_update_uses_generated_request_resource_version() {
    let current = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: route
  namespace: default
  resourceVersion: "42"
  uid: uid-1
spec:
  hostnames: [old.example]
"#;
    let requested = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: route
  namespace: default
  resourceVersion: "43"
spec:
  hostnames: [new.example]
"#;

    let merged = merge_operator_update_yaml(current, requested).expect("non-Kubernetes merge must succeed");
    let merged: Value = yaml_serde::from_str(&merged).expect("merged resource must remain valid YAML");
    assert_eq!(merged["metadata"]["resourceVersion"], "43");
    assert_eq!(merged["metadata"]["uid"], "uid-1");
    assert_eq!(merged["spec"]["hostnames"][0], "new.example");
}

#[test]
fn non_kubernetes_yaml_update_uses_requested_generation() {
    let current = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: gw
  namespace: default
  generation: 1
  uid: uid-1
spec:
  listeners: [{name: http}]
"#;
    let requested = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: gw
  namespace: default
  generation: 11
spec:
  listeners: [{name: http-v10}]
"#;

    let merged = merge_operator_update_yaml(current, requested).expect("non-Kubernetes merge must succeed");
    let merged: Value = yaml_serde::from_str(&merged).expect("merged resource must remain valid YAML");
    assert_eq!(merged["metadata"]["generation"], 11);
    assert_eq!(merged["metadata"]["uid"], "uid-1");
    assert_eq!(merged["spec"]["listeners"][0]["name"], "http-v10");
}

#[test]
fn non_kubernetes_yaml_update_retains_generation_when_request_omits_it() {
    let current = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: gw
  namespace: default
  generation: 7
spec:
  listeners: [{name: http}]
"#;
    let requested = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: gw
  namespace: default
spec:
  listeners: [{name: http-v1}]
"#;

    let merged = merge_operator_update_yaml(current, requested).expect("non-Kubernetes merge must succeed");
    let merged: Value = yaml_serde::from_str(&merged).expect("merged resource must remain valid YAML");
    assert_eq!(merged["metadata"]["generation"], 7);
}
