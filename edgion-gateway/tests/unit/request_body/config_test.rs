use super::*;
use edgion_resources::resources::edgion_gateway_config::RequestBodyStorageConfig;

#[test]
fn crd_scalar_defaults_resolve_to_builtin_runtime_policy() {
    fn defaults(schema: &serde_json::Value) -> serde_json::Value {
        let fields = schema["properties"].as_object().unwrap();
        serde_json::Value::Object(
            fields
                .iter()
                .filter_map(|(name, field)| {
                    field
                        .get("default")
                        .cloned()
                        .or_else(|| field.get("properties").map(|_| defaults(field)))
                        .map(|value| (name.clone(), value))
                })
                .collect(),
        )
    }
    let crd: serde_json::Value = yaml_serde::from_str(include_str!(
        "../../../../config/crd/edgion-crd/edgion_gateway_config_crd.yaml"
    ))
    .unwrap();
    let mut checked = 0;
    for version in crd["spec"]["versions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["served"] == true)
    {
        let schema = &version["schema"]["openAPIV3Schema"]["properties"]["spec"]["properties"]["requestBody"];
        let config: RequestBodyConfig = serde_json::from_value(defaults(schema)).unwrap();
        assert!(config.validate().is_empty());
        assert_eq!(
            ResolvedRequestBodyConfig::resolve(&config),
            ResolvedRequestBodyConfig::default()
        );
        checked += 1;
    }
    assert!(checked > 0);
}

#[test]
fn defaults_preserve_existing_limits() {
    let resolved = ResolvedRequestBodyConfig::resolve(&RequestBodyConfig::default());
    assert_eq!(resolved.memory_buffer_bytes, 128 * 1024);
    assert_eq!(resolved.max_body_size_bytes, 32 * 1024 * 1024);
    assert_eq!(resolved.capture_limit(), resolved.max_body_size_bytes);
    assert!(resolved.spill_to_disk);
    assert_eq!(resolved.storage_operation_timeout, Duration::from_secs(5));
}

#[test]
fn capture_capacity_is_independent_of_hard_admission() {
    let mut config = RequestBodyConfig {
        max_body_size: "4MiB".to_string(),
        storage: RequestBodyStorageConfig {
            memory_limit: "1MiB".to_string(),
            max_buffered_size: Some("2MiB".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    let spill = ResolvedRequestBodyConfig::resolve(&config);
    assert_eq!(spill.capture_limit(), 2 * 1024 * 1024);
    config.storage.spill_to_disk = false;
    let memory = ResolvedRequestBodyConfig::resolve(&config);
    assert_eq!(memory.capture_limit(), 1024 * 1024);
    assert_eq!(memory.max_body_size_bytes, 4 * 1024 * 1024);
    assert_eq!(spill.capture_limit(), 2 * 1024 * 1024);
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
fn invalid_non_admitted_policy_still_bounds_capacity() {
    let mut config = RequestBodyConfig {
        max_body_size: "1MiB".to_string(),
        ..Default::default()
    };
    config.storage.memory_limit = "3MiB".to_string();
    config.storage.max_buffered_size = Some("2MiB".to_string());
    let resolved = ResolvedRequestBodyConfig::resolve(&config);
    assert_eq!(resolved.memory_buffer_bytes, 1024 * 1024);
    assert_eq!(resolved.capture_limit(), 1024 * 1024);
}

#[test]
fn storage_operation_timeout_resolves_and_invalid_values_fail_safe() {
    for (value, expected) in [
        ("750ms", Duration::from_millis(750)),
        ("0s", DEFAULT_STORAGE_OPERATION_TIMEOUT),
        ("invalid", DEFAULT_STORAGE_OPERATION_TIMEOUT),
    ] {
        let mut config = RequestBodyConfig::default();
        config.storage.operation_timeout = value.to_string();
        assert_eq!(
            ResolvedRequestBodyConfig::resolve(&config).storage_operation_timeout,
            expected
        );
    }
}
