use super::*;

fn config_with_header(name: &str) -> RealIpConfig {
    let mut config: RealIpConfig =
        serde_json::from_value(serde_json::json!({"trustedIpRefs": [{"name": "proxies"}]})).unwrap();
    config.real_ip_header = name.into();
    config
}

#[test]
fn test_plugin_validation_rejects_non_token_header_names() {
    for bad in [
        "",
        "   ",
        "X Forwarded For",
        "X-Forwarded-For:",
        " X-Real-IP ",
        "head(er)",
    ] {
        assert_eq!(
            config_with_header(bad).validation_error().as_deref(),
            Some("'real_ip_header' must be a valid HTTP header name")
        );
    }
}

#[test]
fn test_plugin_validation_rejects_oversized_header_name() {
    let oversized = "x".repeat(crate::gwapi_types::HEADER_NAME_MAX_LEN as usize + 1);
    assert!(config_with_header(&oversized).validation_error().is_some());
    let at_bound = "x".repeat(crate::gwapi_types::HEADER_NAME_MAX_LEN as usize);
    assert!(config_with_header(&at_bound).validation_error().is_none());
}

#[test]
fn test_plugin_validation_accepts_default_and_custom_header_names() {
    for ok in ["X-Forwarded-For", "CF-Connecting-IP", "True-Client-IP", "x_real.ip"] {
        assert!(config_with_header(ok).validation_error().is_none());
    }
}
