use super::*;

fn check(s: &str) -> Option<String> {
    validate_alias_str(s)
}

#[test]
fn accepts_letters_digits_dot_dash_underscore_within_32_bytes() {
    assert!(check("a").is_none());
    assert!(check("auth-internal").is_none());
    assert!(check("auth_v2.1").is_none());
    assert!(check("ABC.def-ghi_123").is_none());
    assert!(check(&"a".repeat(32)).is_none());
}

#[test]
fn rejects_empty_string() {
    let err = check("").expect("empty alias must be rejected");
    assert!(err.contains("[a-zA-Z0-9._-]"), "{err}");
}

#[test]
fn rejects_over_32_bytes() {
    let err = check(&"a".repeat(33)).expect("33-char alias must be rejected");
    assert!(err.contains("[a-zA-Z0-9._-]"), "{err}");
}

#[test]
fn rejects_disallowed_chars() {
    for bad in [
        "space here",
        "slash/here",
        // Multi-byte UTF-8 must be rejected (validator allows ASCII alphanumerics + . _ - only)
        "caf\u{e9}",
        "with\nnewline",
        "quote\"here",
        "/leading",
    ] {
        assert!(check(bad).is_some(), "must reject {bad:?}");
    }
}

#[test]
fn entry_with_alias_serde_roundtrip() {
    use crate::resources::edgion_plugins::EdgionPlugin;
    use crate::resources::plugin_configs::CorsConfig;

    let yaml = r#"
alias: auth-internal
type: Cors
config: {}
"#;
    let entry: PluginEntry = serde_yaml::from_str(yaml).expect("yaml parse");
    assert_eq!(entry.alias.as_deref(), Some("auth-internal"));
    assert!(matches!(entry.plugin, EdgionPlugin::Cors(_)));

    // Round-trip retains the field
    let json = serde_json::to_value(&entry).unwrap();
    assert_eq!(json["alias"], serde_json::json!("auth-internal"));

    // No alias -> field absent in serialized form
    let entry_no_alias = PluginEntry::new(EdgionPlugin::Cors(CorsConfig::default()));
    let json = serde_json::to_value(&entry_no_alias).unwrap();
    assert!(json.get("alias").is_none(), "alias must be omitted when None: {json}");
}

#[test]
fn entry_validate_alias_returns_error_for_invalid() {
    use crate::resources::edgion_plugins::EdgionPlugin;
    use crate::resources::plugin_configs::CorsConfig;

    let mut entry = PluginEntry::new(EdgionPlugin::Cors(CorsConfig::default()));
    assert_eq!(entry.validate_alias(), None);

    entry.alias = Some("bad/alias".to_string());
    let err = entry.validate_alias().expect("must reject");
    assert!(err.contains("[a-zA-Z0-9._-]"), "{err}");

    entry.alias = Some("ok-name_v1.2".to_string());
    assert_eq!(entry.validate_alias(), None);
}
