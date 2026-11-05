use super::*;

#[test]
fn resource_key_meta_is_constructible() {
    let _meta = ResourceKeyMeta {
        namespace: Some("ns".to_string()),
        name: "n".to_string(),
        creation_timestamp: None,
        labels: Default::default(),
        annotations: Default::default(),
        resource_version: Some("1".to_string()),
    };
    let _key = ResourceKey {
        kind: "HTTPRoute".to_string(),
        api_version: "gateway.networking.k8s.io/v1".to_string(),
        metadata: _meta,
    };
}

#[test]
fn list_result_default_param_is_conf_entry() {
    let r: ListResult = ListResult {
        items: Vec::<ConfEntry>::new(),
        continue_token: None,
    };
    assert!(r.items.is_empty());
}

#[test]
fn list_result_can_hold_resource_keys() {
    let r: ListResult<ResourceKey> = ListResult {
        items: Vec::new(),
        continue_token: Some("tok".to_string()),
    };
    assert_eq!(r.continue_token.as_deref(), Some("tok"));
}

#[test]
fn not_supported_displays() {
    let e = ConfWriterError::NotSupported("foo".to_string());
    assert_eq!(format!("{e}"), "Not supported: foo");
}

#[test]
fn stale_pagination_displays() {
    let e = ConfWriterError::StalePagination("token expired".to_string());
    assert_eq!(format!("{e}"), "Stale pagination: token expired");
}

/// Locks in the JSON wire shape: ResourceKey must emit `apiVersion`
/// (camelCase), matching the Dashboard TS interface and the K8s
/// convention. Without `#[serde(rename_all = "camelCase")]` serde
/// emits the raw `api_version`, which silently breaks consumers.
#[test]
fn resource_key_serializes_camel_case_fields() {
    let key = ResourceKey {
        kind: "HTTPRoute".to_string(),
        api_version: "gateway.networking.k8s.io/v1".to_string(),
        metadata: ResourceKeyMeta {
            name: "r".to_string(),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
    };
    let json = serde_json::to_string(&key).expect("serialize");
    assert!(
        json.contains("\"apiVersion\":\"gateway.networking.k8s.io/v1\""),
        "expected camelCase apiVersion, got: {json}"
    );
    assert!(
        !json.contains("api_version"),
        "must not leak snake_case api_version, got: {json}"
    );
}

#[test]
fn project_conf_entry_to_key_strips_spec_and_status() {
    let entry = ConfEntry {
        kind: "HTTPRoute".to_string(),
        namespace: Some("default".to_string()),
        name: "r1".to_string(),
        content: r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  rules: [{ matches: [{ path: { type: PathPrefix, value: /a } }] }]
status:
  parents: []
"#
        .to_string(),
    };
    let key = project_conf_entry_to_key("HTTPRoute", &entry).expect("project");
    assert_eq!(key.kind, "HTTPRoute");
    assert_eq!(key.api_version, "gateway.networking.k8s.io/v1");
    assert_eq!(key.metadata.name, "r1");
    assert_eq!(key.metadata.namespace.as_deref(), Some("default"));
    let json = serde_json::to_string(&key).expect("serialize");
    assert!(!json.contains("\"spec\""), "spec leaked: {json}");
    assert!(!json.contains("\"status\""), "status leaked: {json}");
}

#[test]
fn project_conf_entry_to_key_filters_last_applied_annotation() {
    let entry = ConfEntry {
        kind: "HTTPRoute".to_string(),
        namespace: Some("default".to_string()),
        name: "r1".to_string(),
        content: r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
  annotations:
    kubectl.kubernetes.io/last-applied-configuration: '{"huge":"yaml"}'
    edgion.io/enable-http: "true"
"#
        .to_string(),
    };
    let key = project_conf_entry_to_key("HTTPRoute", &entry).expect("project");
    assert!(
        !key.metadata
            .annotations
            .contains_key("kubectl.kubernetes.io/last-applied-configuration"),
        "last-applied leaked"
    );
    assert_eq!(
        key.metadata
            .annotations
            .get("edgion.io/enable-http")
            .map(String::as_str),
        Some("true"),
        "other annotations must pass through"
    );
}

#[test]
fn project_conf_entry_to_key_falls_back_to_entry_name_namespace() {
    let entry = ConfEntry {
        kind: "HTTPRoute".to_string(),
        namespace: Some("override-ns".to_string()),
        name: "override-name".to_string(),
        content: r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  labels:
    app: demo
"#
        .to_string(),
    };
    let key = project_conf_entry_to_key("HTTPRoute", &entry).expect("project");
    assert_eq!(key.metadata.name, "override-name");
    assert_eq!(key.metadata.namespace.as_deref(), Some("override-ns"));
}

#[test]
fn project_conf_entry_to_key_handles_invalid_content() {
    let entry = ConfEntry {
        kind: "HTTPRoute".to_string(),
        namespace: None,
        name: "x".to_string(),
        content: "::: not a valid document :::".to_string(),
    };
    assert!(project_conf_entry_to_key("HTTPRoute", &entry).is_none());
}

/// Direct test of the `list_keys_by_kind` default impl logic.
///
/// Mocking the full `CenterApi` trait (to test through async dispatch)
/// requires ~50 lines of `unimplemented!()` boilerplate per method, which
/// is more noise than signal. Instead this calls
/// [`project_conf_entry_list`] — the SAME helper the default impl invokes
/// — so the production and test code paths execute identical instructions.
/// If the default impl ever stops calling `project_conf_entry_list` (e.g.
/// switches to `.map().flatten()`), this test would not catch it; for
/// that level of regression guard, the integration scripts
/// `list-keys-{fs,etcd}.sh` exercise the trait dispatch end-to-end.
#[test]
fn list_keys_default_impl_rejects_invalid_source_instead_of_partial_success() {
    let entries = vec![
        ConfEntry {
            kind: "HTTPRoute".to_string(),
            namespace: Some("ns".to_string()),
            name: "good".to_string(),
            content: "apiVersion: v1\nkind: HTTPRoute\nmetadata:\n  name: good\n  namespace: ns\n".to_string(),
        },
        ConfEntry {
            kind: "HTTPRoute".to_string(),
            namespace: Some("ns".to_string()),
            name: "bad".to_string(),
            content: "::: not valid :::".to_string(),
        },
    ];
    let full: ListResult<ConfEntry> = ListResult {
        items: entries,
        continue_token: Some("tok-42".to_string()),
    };

    assert!(project_conf_entry_list("HTTPRoute", full.clone()).is_err());
    let valid = ListResult {
        items: vec![full.items[0].clone()],
        continue_token: full.continue_token,
    };
    let projected = project_conf_entry_list("HTTPRoute", valid).unwrap();
    assert_eq!(projected.items.len(), 1);
    assert_eq!(projected.items[0].metadata.name, "good");
    assert_eq!(projected.continue_token.as_deref(), Some("tok-42"));
}
