use super::*;

#[test]
fn serde_roundtrip_active_some() {
    let s = Selector {
        active: Some("lockdown".to_string()),
        description: Some("switch to lockdown".to_string()),
    };
    let j = serde_json::to_value(&s).unwrap();
    assert_eq!(j["active"], "lockdown");
    assert_eq!(j["description"], "switch to lockdown");
    let back: Selector = serde_json::from_value(j).unwrap();
    assert_eq!(back.active.as_deref(), Some("lockdown"));
}

#[test]
fn active_none_omitted_in_output() {
    let s = Selector {
        active: None,
        description: None,
    };
    let j = serde_json::to_value(&s).unwrap();
    // skip_serializing_if drops both fields → empty object.
    assert!(j.get("active").is_none());
    assert!(j.get("description").is_none());
}

#[test]
fn deserialize_empty_object_is_no_op_selector() {
    let s: Selector = serde_json::from_str("{}").unwrap();
    assert!(s.active.is_none());
    assert!(s.description.is_none());
}
