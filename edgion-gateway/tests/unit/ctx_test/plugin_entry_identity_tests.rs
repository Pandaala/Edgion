use super::CurrentPluginEntryIdentity;

#[test]
fn preserves_bounded_name_and_alias() {
    let identity = CurrentPluginEntryIdentity::new("AiProxy", Some("edge.ai-1")).unwrap();
    assert_eq!(identity.name(), "AiProxy");
    assert_eq!(identity.alias(), Some("edge.ai-1"));
    assert_eq!(CurrentPluginEntryIdentity::new("AiProxy", None).unwrap().alias(), None);
}

#[test]
fn rejects_empty_or_overlong_identity_parts() {
    assert!(CurrentPluginEntryIdentity::new("", None).is_none());
    assert!(CurrentPluginEntryIdentity::new(&"x".repeat(37), None).is_none());
    assert!(CurrentPluginEntryIdentity::new("AiProxy", Some(&"x".repeat(33))).is_none());
}
