use super::SecretObjectReference;

#[test]
fn deserialize_minimal() {
    let yaml = "name: my-secret";
    let s: SecretObjectReference = yaml_serde::from_str(yaml).unwrap();
    assert_eq!(s.name, "my-secret");
    assert_eq!(s.namespace, None);
    assert_eq!(s.group, None);
    assert_eq!(s.kind, None);
}

#[test]
fn deserialize_with_group_kind() {
    let yaml = "name: my-secret\ngroup: \"\"\nkind: Secret\nnamespace: credentials";
    let s: SecretObjectReference = yaml_serde::from_str(yaml).unwrap();
    assert_eq!(s.group.as_deref(), Some(""));
    assert_eq!(s.kind.as_deref(), Some("Secret"));
    assert_eq!(s.namespace.as_deref(), Some("credentials"));
}

#[test]
fn name_required() {
    let yaml = "namespace: credentials";
    assert!(yaml_serde::from_str::<SecretObjectReference>(yaml).is_err());
}

#[test]
fn rejects_unknown_fields() {
    let yaml = "name: my-secret\nunknownField: forbidden";
    assert!(yaml_serde::from_str::<SecretObjectReference>(yaml).is_err());
}

#[test]
fn effective_namespace_falls_back() {
    let s = SecretObjectReference {
        group: None,
        kind: None,
        name: "s".into(),
        namespace: None,
    };
    assert_eq!(s.effective_namespace("owner-ns"), "owner-ns");
}
