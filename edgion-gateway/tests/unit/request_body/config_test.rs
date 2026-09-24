use super::*;

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
