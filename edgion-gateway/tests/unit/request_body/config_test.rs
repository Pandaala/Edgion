use super::*;

#[test]
fn crd_scalar_defaults_resolve_to_builtin_runtime_policy() {
    let crd: serde_json::Value = serde_yaml::from_str(include_str!(
        "../../../../config/crd/edgion-crd/edgion_gateway_config_crd.yaml"
    ))
    .expect("committed Gateway config CRD must parse");
    let versions = crd["spec"]["versions"].as_array().expect("CRD versions exist");
    let mut checked = 0;
    for version in versions.iter().filter(|version| version["served"] == true) {
        let properties = version["schema"]["openAPIV3Schema"]["properties"]["spec"]["properties"]["requestBody"]
            ["properties"]
            .as_object()
            .expect("request-body schema properties exist");
        // An explicit specimen, not an admission/defaulting simulator.
        let defaults: serde_json::Map<String, serde_json::Value> = properties
            .iter()
            .filter_map(|(name, field)| field.get("default").map(|value| (name.clone(), value.clone())))
            .collect();
        let config: RequestBodyConfig =
            serde_json::from_value(serde_json::Value::Object(defaults)).expect("CRD scalar defaults must deserialize");
        assert!(config.validate().is_empty());
        assert_eq!(
            ResolvedRequestBodyConfig::resolve(&config),
            ResolvedRequestBodyConfig::default(),
            "{}: CRD defaults must agree with the Gateway runtime fallback",
            version["name"]
        );
        checked += 1;
    }
    assert!(checked > 0, "at least one served version must be checked");
}

#[test]
fn defaults_preserve_existing_limits() {
    let resolved = ResolvedRequestBodyConfig::resolve(&RequestBodyConfig::default());
    assert_eq!(resolved.memory_buffer_bytes, 128 * 1024);
    assert_eq!(resolved.max_body_size_bytes, 32 * 1024 * 1024);
    assert_eq!(resolved.storage_operation_timeout, Duration::from_secs(5));
}

#[test]
fn business_maxima_override_defaults() {
    let config = RequestBodyConfig {
        max_memory_buffer_size: Some("1MiB".to_string()),
        max_body_size: Some("64MiB".to_string()),
        ..Default::default()
    };
    let resolved = ResolvedRequestBodyConfig::resolve(&config);
    assert_eq!(resolved.memory_buffer_bytes, 1024 * 1024);
    assert_eq!(resolved.max_body_size_bytes, 64 * 1024 * 1024);
}

#[test]
fn disabled_policy_disables_capture() {
    let config = RequestBodyConfig {
        enabled: false,
        ..Default::default()
    };
    assert!(!ResolvedRequestBodyConfig::resolve(&config).enabled);
}

#[test]
fn memory_never_exceeds_body_cap() {
    let config = RequestBodyConfig {
        max_memory_buffer_size: Some("2MiB".to_string()),
        max_body_size: Some("1MiB".to_string()),
        ..Default::default()
    };
    let resolved = ResolvedRequestBodyConfig::resolve(&config);
    assert_eq!(resolved.memory_buffer_bytes, 1024 * 1024);
}

#[test]
fn storage_operation_timeout_resolves_and_invalid_values_fail_safe() {
    let configured = RequestBodyConfig {
        storage_operation_timeout: "750ms".to_string(),
        ..Default::default()
    };
    assert_eq!(
        ResolvedRequestBodyConfig::resolve(&configured).storage_operation_timeout,
        Duration::from_millis(750)
    );

    for value in ["0s", "invalid"] {
        let invalid = RequestBodyConfig {
            storage_operation_timeout: value.to_string(),
            ..Default::default()
        };
        assert_eq!(
            ResolvedRequestBodyConfig::resolve(&invalid).storage_operation_timeout,
            DEFAULT_STORAGE_OPERATION_TIMEOUT
        );
    }
}
