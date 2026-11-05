use super::*;
#[test]
fn source_precondition_is_canonical_and_includes_unknown_fields_and_identity() {
    let a: serde_json::Value =
        serde_json::from_str(r#"{"metadata":{"uid":"u","resourceVersion":"1"},"spec":{"unknown":{"a":1,"b":2}}}"#)
            .unwrap();
    let b: serde_json::Value =
        serde_json::from_str(r#"{"spec":{"unknown":{"b":2,"a":1}},"metadata":{"resourceVersion":"1","uid":"u"}}"#)
            .unwrap();
    let expected = SourcePrecondition::from_value(&a).unwrap();
    assert_eq!(expected, SourcePrecondition::from_value(&b).unwrap());
    assert!(expected.verify(&yaml_serde::to_string(&b).unwrap()).is_ok());
    for pointer in ["/spec/unknown/a", "/metadata/uid", "/metadata/resourceVersion"] {
        let mut changed = b.clone();
        *changed.pointer_mut(pointer).unwrap() = serde_json::json!("changed");
        assert!(matches!(
            expected.verify(&changed.to_string()),
            Err(ConfWriterError::Conflict(_))
        ));
    }
}
