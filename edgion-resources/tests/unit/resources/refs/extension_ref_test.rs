use super::*;

#[test]
fn required_group_is_not_defaulted_or_lost_during_serialization() {
    let reference: LocalObjectReference = serde_json::from_value(serde_json::json!({
        "group": "", "kind": "Secret", "name": "material"
    }))
    .unwrap();
    assert_eq!(serde_json::to_value(reference).unwrap()["group"], "");
    for value in [
        serde_json::json!({"kind": "EdgionPlugins", "name": "policy"}),
        serde_json::json!({"group": null, "kind": "EdgionPlugins", "name": "policy"}),
    ] {
        assert!(serde_json::from_value::<LocalObjectReference>(value).is_err());
    }
}

fn reference(group: &str, kind: &str) -> LocalObjectReference {
    LocalObjectReference {
        group: group.to_string(),
        kind: kind.to_string(),
        name: "plugins".to_string(),
        resolved_namespace: None,
    }
}

#[test]
fn pairing_and_supported_target_match_gateway_api() {
    let supported = reference("edgion.io", "EdgionPlugins");
    assert!(matches!(validate_extension_ref_filter(false, None), Ok(None)));
    assert!(matches!(
        validate_extension_ref_filter(false, Some(&supported)),
        Err(ExtensionRefValidationError::UnexpectedConfiguration)
    ));
    assert!(matches!(
        validate_extension_ref_filter(true, None),
        Err(ExtensionRefValidationError::MissingConfiguration)
    ));
    assert_eq!(
        validate_extension_ref_filter(true, Some(&supported))
            .expect("edgion.io/EdgionPlugins must be supported")
            .map(|reference| reference.name.as_str()),
        Some("plugins")
    );

    for unsupported in [
        reference("", "EdgionPlugins"),
        reference("other.io", "EdgionPlugins"),
        reference("edgion.io", "Secret"),
    ] {
        assert!(matches!(
            validate_extension_ref_filter(true, Some(&unsupported)),
            Err(ExtensionRefValidationError::UnsupportedTarget)
        ));
    }
}
