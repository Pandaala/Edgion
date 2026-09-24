use super::*;

#[test]
fn ref_denied_absent_round_trips_and_skips_serialization() {
    let r = EdgionConfigDataRef {
        name: "x".into(),
        namespace: None,
        ref_denied: None,
    };
    let json = serde_json::to_string(&r).unwrap();
    // ref_denied is skipped when None
    assert!(!json.contains("refDenied"), "got: {json}");
    let back: EdgionConfigDataRef = serde_json::from_str(&json).unwrap();
    assert_eq!(back, r);
}

#[test]
fn ref_denied_present_serializes_and_round_trips() {
    let r = EdgionConfigDataRef {
        name: "x".into(),
        namespace: Some("team-b".into()),
        ref_denied: Some(crate::resources::common::RefDenied {
            target_namespace: "team-b".into(),
            target_name: "x".into(),
            reason: Some("NoMatchingReferenceGrant".into()),
        }),
    };
    let json = serde_json::to_string(&r).unwrap();
    assert!(json.contains("refDenied"), "got: {json}");
    let back: EdgionConfigDataRef = serde_json::from_str(&json).unwrap();
    assert_eq!(back, r);
}

#[test]
fn old_payload_without_ref_denied_deserializes() {
    let back: EdgionConfigDataRef = serde_json::from_str(r#"{"name":"x"}"#).unwrap();
    assert_eq!(
        back,
        EdgionConfigDataRef {
            name: "x".into(),
            namespace: None,
            ref_denied: None
        }
    );
}
