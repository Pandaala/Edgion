use super::*;

fn parse(extra: &str) -> GuardrailConfig {
    serde_yaml::from_str(&format!(
        "target:\n  url: https://guard.example\nrequest:\n  path:\n    template: /v1/decision\n{extra}"
    ))
    .unwrap()
}

#[test]
fn valid_minimal_config_and_defaults() {
    let config = parse("");
    assert_eq!(config.window_bytes, 4096);
    assert_eq!(config.overlap_bytes, 256);
    assert_eq!(config.max_concurrent, 64);
    assert!(!config.hold_first_window);
    assert_eq!(config.head_hold_timeout_ms, 5_000);
    assert!(!config.fail_open);
    assert!(config.validation_error().is_none());
}

#[test]
fn plugin_variant_is_request_only() {
    let plugin: crate::resources::edgion_plugins::EdgionPlugin = serde_yaml::from_str(
            "type: Guardrail\nconfig:\n  target:\n    url: https://guard.example\n  request:\n    path:\n      template: /v1/decision\n",
        )
        .unwrap();
    assert_eq!(plugin.type_name(), "Guardrail");
    assert!(plugin.validation_error().is_none());
    let stages = crate::resources::edgion_plugins::applicable_stages_by_name(plugin.type_name());
    assert!(stages.request);
    assert!(!stages.upstream_response_filter);
    assert!(!stages.upstream_response_body_filter);
    assert!(!stages.upstream_response);
}

#[test]
fn accepts_trusted_and_secret_header_templates() {
    let config = parse(
            "  headers:\n    custom:\n      - name: Authorization\n        template: 'Bearer ${secretRef:edgion/guard:key}'\n      - name: X-Route\n        template: '${matchedHttpRoute:name}-${ctx:tenant}'\n",
        );
    assert!(config.validation_error().is_none());
    assert_eq!(
        config.request.collect_secret_ref_names(),
        vec!["edgion/guard:key".to_string()]
    );
}

#[test]
fn rejects_replay_and_operator_owned_protocol_fields() {
    let retry = parse("retry:\n  maxRetries: 1\n");
    assert!(retry.validation_error().unwrap().contains("retry"));

    let timeout = parse("timeoutMsTemplate: '${ctx:timeout}'\n");
    assert!(timeout.validation_error().unwrap().contains("timeoutMsTemplate"));

    let method = parse("  method:\n    template: POST\n");
    assert!(method.validation_error().unwrap().contains("method"));

    let body = parse("  body:\n    type: raw\n    template: nope\n");
    assert!(body.validation_error().unwrap().contains("body"));
}

#[test]
fn rejects_fail_open_while_response_head_hold_is_mandatory() {
    let config = parse("holdFirstWindow: true\nfailOpen: true\n");
    assert!(config.validation_error().unwrap().contains("failOpen"));

    let legacy_fail_open = parse("failOpen: true\n");
    assert!(legacy_fail_open.validation_error().is_none());
}

#[test]
fn rejects_client_controlled_templates_and_out_of_range_bounds() {
    let header = parse("  headers:\n    custom:\n      - name: X-Unsafe\n        template: '${header:X-User}'\n");
    assert!(header.validation_error().unwrap().contains("client-controlled"));

    let bounds = parse("windowBytes: 0\nmaxConcurrent: 1025\n");
    assert!(bounds.validation_error().unwrap().contains("windowBytes"));
}

#[test]
fn resolved_secrets_are_absent_from_schema_and_redacted() {
    let schema = schemars::schema_for!(GuardrailConfig);
    let value = serde_json::to_value(schema).unwrap();
    let properties = value.get("properties").unwrap().as_object().unwrap();
    assert!(!properties.contains_key("resolvedSecrets"));

    let mut config = parse("");
    config.resolved_secrets = Some(ResolvedSecrets(std::collections::BTreeMap::from([(
        "edgion/guard:key".to_string(),
        "secret-value".to_string(),
    )])));
    let serialized = {
        let _guard = crate::redact::AdminRedactGuard::new();
        serde_json::to_string(&config).unwrap()
    };
    assert!(serialized.contains("[redacted]"));
    assert!(!serialized.contains("secret-value"));
}
