use super::*;

fn valid_config() -> RealIpConfig {
    serde_json::from_value(serde_json::json!({"trustedIpRefs": [{"name": "proxies"}]})).unwrap()
}

#[test]
fn test_default_config() {
    let config = RealIpConfig::default();
    assert!(config.trusted.ip_refs.is_empty());
    assert_eq!(config.real_ip_header, "X-Forwarded-For");
    assert!(config.recursive);
    assert_eq!(config.max_trusted_hops, None);
}

#[test]
fn test_empty_trusted_refs() {
    assert!(RealIpConfig::default()
        .validation_error()
        .unwrap()
        .contains("trustedIpRefs"));
}

#[test]
fn test_custom_header() {
    let mut config = valid_config();
    config.real_ip_header = "CF-Connecting-IP".into();
    config.recursive = false;
    assert!(config.validate_and_init().is_ok());
}

#[test]
fn test_serialization() {
    let config = valid_config();
    let json = serde_json::to_value(&config).unwrap();
    assert_eq!(json["trustedIpRefs"][0]["name"], "proxies");
    assert!(json.get("trustedIps").is_none());
    assert!(json.get("invalidInput").is_none());
    let restored: RealIpConfig = serde_json::from_value(json).unwrap();
    assert!(restored.validation_error().is_none());
}

#[test]
fn rejected_fields_preserve_invalidity_without_payload() {
    for key in ["trustedIps", "trustedIPRefs", "unexpected"] {
        for value in [
            serde_json::Value::Null,
            serde_json::json!([]),
            serde_json::json!([{"cidrs": ["10.0.0.0/8"]}]),
        ] {
            let mut raw = serde_json::json!({"trustedIpRefs": [{"name": "proxies"}], "invalidInput": false});
            raw[key] = value;
            let config: RealIpConfig = serde_json::from_value(raw).unwrap();
            assert!(config.trusted.invalid_input);
            assert!(config.validation_error().is_some());
            let transported = serde_json::to_value(&config).unwrap();
            assert!(transported.get(key).is_none());
            assert_eq!(transported["invalidInput"], true);
            let restored: RealIpConfig = serde_json::from_value(transported).unwrap();
            assert!(restored.validation_error().is_some());
        }
    }
    let forced: RealIpConfig =
        serde_json::from_value(serde_json::json!({"trustedIpRefs": [{"name": "proxies"}], "invalidInput": true}))
            .unwrap();
    assert!(forced.validation_error().is_some());
    assert!(valid_config().validation_error().is_none());
}
