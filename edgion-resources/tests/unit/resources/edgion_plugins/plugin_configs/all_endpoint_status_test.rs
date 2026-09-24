use super::*;

#[test]
fn test_default_values() {
    let config = AllEndpointStatusConfig::default();
    assert_eq!(config.timeout_ms, 2000);
    assert_eq!(config.wall_timeout_ms, 10_000);
    assert_eq!(config.max_endpoints, 20);
    assert_eq!(config.max_body_size, 16 * 1024);
    assert_eq!(config.concurrency_limit, 10);
    assert!(!config.include_response_headers);
    assert!(config.method_override.is_none());
    assert!(config.path_override.is_none());
}

#[test]
fn test_timeout_clamped_to_5s() {
    let config = AllEndpointStatusConfig {
        timeout_ms: 10000,
        ..Default::default()
    };
    assert_eq!(config.effective_timeout_ms(), 5000);
}

#[test]
fn test_wall_timeout_clamped_to_10s() {
    let config = AllEndpointStatusConfig {
        wall_timeout_ms: 30000,
        ..Default::default()
    };
    assert_eq!(config.effective_wall_timeout_ms(), 10000);
}

#[test]
fn test_max_endpoints_clamped_to_hard_cap() {
    let config = AllEndpointStatusConfig {
        max_endpoints: 500,
        ..Default::default()
    };
    // global_max = 100, hard cap = 50 → min(500, 100, 50) = 50
    assert_eq!(config.effective_max_endpoints(100), 50);
}

#[test]
fn test_max_endpoints_clamped_to_global() {
    let config = AllEndpointStatusConfig {
        max_endpoints: 40,
        ..Default::default()
    };
    // global_max = 20, hard cap = 50 → min(40, 20, 50) = 20
    assert_eq!(config.effective_max_endpoints(20), 20);
}

#[test]
fn test_max_endpoints_plugin_value_lowest() {
    let config = AllEndpointStatusConfig {
        max_endpoints: 15,
        ..Default::default()
    };
    // global_max = 30, hard cap = 50 → min(15, 30, 50) = 15
    assert_eq!(config.effective_max_endpoints(30), 15);
}

#[test]
fn test_max_body_size_clamped() {
    let config = AllEndpointStatusConfig {
        max_body_size: 1024 * 1024,
        ..Default::default()
    }; // 1MB
    assert_eq!(config.effective_max_body_size(), 16 * 1024);
}

// --- validation tests ---

#[test]
fn test_validation_default_config_is_valid() {
    let config = AllEndpointStatusConfig::default();
    assert!(config.get_validation_error().is_none());
}

#[test]
fn test_validation_timeout_ms_zero() {
    let config = AllEndpointStatusConfig {
        timeout_ms: 0,
        ..Default::default()
    };
    assert_eq!(config.get_validation_error(), Some("timeoutMs must be > 0".to_string()));
}

#[test]
fn test_validation_wall_timeout_ms_zero() {
    let config = AllEndpointStatusConfig {
        wall_timeout_ms: 0,
        ..Default::default()
    };
    assert_eq!(
        config.get_validation_error(),
        Some("wallTimeoutMs must be > 0".to_string())
    );
}

#[test]
fn test_validation_max_endpoints_zero() {
    let config = AllEndpointStatusConfig {
        max_endpoints: 0,
        ..Default::default()
    };
    assert_eq!(
        config.get_validation_error(),
        Some("maxEndpoints must be > 0".to_string())
    );
}

#[test]
fn test_validation_concurrency_limit_zero() {
    let config = AllEndpointStatusConfig {
        concurrency_limit: 0,
        ..Default::default()
    };
    assert_eq!(
        config.get_validation_error(),
        Some("concurrencyLimit must be > 0".to_string())
    );
}

#[test]
fn test_validation_path_override_missing_slash() {
    let config = AllEndpointStatusConfig {
        path_override: Some("health".to_string()),
        ..Default::default()
    };
    assert_eq!(
        config.get_validation_error(),
        Some("pathOverride must start with '/'".to_string()),
    );
}

#[test]
fn test_validation_path_override_with_slash_is_valid() {
    let config = AllEndpointStatusConfig {
        path_override: Some("/health".to_string()),
        ..Default::default()
    };
    assert!(config.get_validation_error().is_none());
}
