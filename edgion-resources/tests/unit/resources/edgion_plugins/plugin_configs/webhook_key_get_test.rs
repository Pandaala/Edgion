use super::*;

fn valid_var() -> WebhookVar {
    WebhookVar {
        name: "tenant_tier".to_string(),
        webhook_ref: "edgion-default/tenant-resolver".to_string(),
        path_override: None,
        extract: WebhookExtract::BodyText,
        default: None,
    }
}

fn valid_config() -> WebhookKeyGetConfig {
    WebhookKeyGetConfig {
        vars: vec![valid_var()],
    }
}

fn assert_check_matches_compile(config: &WebhookKeyGetConfig) {
    let check = config.check_fields();
    let mut compiled = config.clone();
    let compile_err = compiled.validate_and_compile().err();
    let mut validated = config.clone();
    let validate_err = validated.validate().err();
    assert_eq!(check, compile_err, "check_fields vs validate_and_compile");
    assert_eq!(check, validate_err, "check_fields vs validate");
    assert_eq!(config.validation_error(), check);
}

#[test]
fn valid_config_succeeds() {
    let config = valid_config();
    assert!(config.check_fields().is_none());
    assert_check_matches_compile(&config);
    let mut compiled = config.clone();
    assert!(compiled.validate_and_compile().is_ok());
}

#[test]
fn rejects_empty_vars() {
    let config = WebhookKeyGetConfig { vars: vec![] };
    assert_eq!(config.check_fields().as_deref(), Some("vars list is empty"));
    assert_check_matches_compile(&config);
}

#[test]
fn rejects_empty_name() {
    let mut config = valid_config();
    config.vars[0].name.clear();
    assert_eq!(config.check_fields().as_deref(), Some("vars[0]: name is empty"));
    assert_check_matches_compile(&config);
}

#[test]
fn rejects_webhook_ref_not_namespace_name() {
    let mut config = valid_config();
    config.vars[0].webhook_ref = "tenant-resolver".to_string();
    let err = config.check_fields().expect("must reject");
    assert!(err.contains("webhookRef"), "got: {err}");
    assert!(err.contains("namespace/name"), "got: {err}");
    assert_check_matches_compile(&config);
}

#[test]
fn rejects_webhook_ref_empty_parts() {
    let mut config = valid_config();
    config.vars[0].webhook_ref = "edgion-default/".to_string();
    let err = config.check_fields().expect("must reject");
    assert!(err.contains("webhookRef"), "got: {err}");
    assert_check_matches_compile(&config);
}

#[test]
fn rejects_webhook_ref_special_chars() {
    let mut config = valid_config();
    config.vars[0].webhook_ref = "default/webhook@svc".to_string();
    let err = config.check_fields().expect("must reject");
    assert!(err.contains("webhookRef"), "got: {err}");
    assert_check_matches_compile(&config);
}

#[test]
fn rejects_empty_extract_header_name() {
    let mut config = valid_config();
    config.vars[0].extract = WebhookExtract::Header { name: String::new() };
    let err = config.check_fields().expect("must reject");
    assert!(err.contains("extract header name is empty"), "got: {err}");
    assert_check_matches_compile(&config);
}

#[test]
fn rejects_empty_extract_cookie_name() {
    let mut config = valid_config();
    config.vars[0].extract = WebhookExtract::Cookie { name: String::new() };
    let err = config.check_fields().expect("must reject");
    assert!(err.contains("extract cookie name is empty"), "got: {err}");
    assert_check_matches_compile(&config);
}

#[test]
fn rejects_empty_extract_body_path() {
    let mut config = valid_config();
    config.vars[0].extract = WebhookExtract::Body { path: String::new() };
    let err = config.check_fields().expect("must reject");
    assert!(err.contains("extract body path is empty"), "got: {err}");
    assert_check_matches_compile(&config);
}

#[test]
fn body_text_extract_is_always_well_formed() {
    let mut config = valid_config();
    config.vars[0].extract = WebhookExtract::BodyText;
    assert!(config.check_fields().is_none());
    assert_check_matches_compile(&config);
}

#[test]
fn yaml_roundtrip_camel_case() {
    let yaml = r#"
vars:
  - name: tenant_tier
    webhookRef: "edgion-default/tenant-resolver"
    pathOverride: "/resolve/tier"
    extract:
      source: body
      path: "data.tier"
    default: "free"
"#;
    let config: WebhookKeyGetConfig = yaml_serde::from_str(yaml).unwrap();
    assert_eq!(config.vars.len(), 1);
    assert_eq!(config.vars[0].name, "tenant_tier");
    assert_eq!(config.vars[0].webhook_ref, "edgion-default/tenant-resolver");
    assert_eq!(config.vars[0].path_override.as_deref(), Some("/resolve/tier"));
    assert_eq!(
        config.vars[0].extract,
        WebhookExtract::Body {
            path: "data.tier".to_string()
        }
    );
    assert_eq!(config.vars[0].default.as_deref(), Some("free"));
    assert!(config.check_fields().is_none());
}

#[test]
fn test_webhook_extract_serde() {
    let yaml = r#"source: header
name: "X-Key""#;
    let extract: WebhookExtract = yaml_serde::from_str(yaml).unwrap();
    assert_eq!(
        extract,
        WebhookExtract::Header {
            name: "X-Key".to_string()
        }
    );

    let yaml = r#"source: body
path: "result.value""#;
    let extract: WebhookExtract = yaml_serde::from_str(yaml).unwrap();
    assert_eq!(
        extract,
        WebhookExtract::Body {
            path: "result.value".to_string()
        }
    );

    let yaml = "source: bodyText";
    let extract: WebhookExtract = yaml_serde::from_str(yaml).unwrap();
    assert_eq!(extract, WebhookExtract::BodyText);

    let yaml = r#"source: cookie
name: "sess""#;
    let extract: WebhookExtract = yaml_serde::from_str(yaml).unwrap();
    assert_eq!(
        extract,
        WebhookExtract::Cookie {
            name: "sess".to_string()
        }
    );
}
