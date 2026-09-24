use super::*;

fn config_with(
    consecutive_errors: u32,
    consecutive_gateway_errors: Option<u32>,
    consecutive_local_origin_failures: Option<u32>,
    ejection_time: &str,
    max_ejection_time: Option<&str>,
) -> OutlierDetectionConfig {
    OutlierDetectionConfig {
        consecutive_errors,
        consecutive_gateway_errors,
        consecutive_local_origin_failures,
        ejection_time: ejection_time.to_string(),
        max_ejection_time: max_ejection_time.map(str::to_string),
        max_ejection_percent: default_max_ejection_percent(),
    }
}

#[test]
fn gateway_errors_defaults_to_consecutive_errors() {
    let config = config_with(2, None, None, &default_ejection_time(), None).normalize();
    assert_eq!(config.consecutive_errors, 2);
    assert_eq!(config.consecutive_gateway_errors, 2); // inherits, not literal 5
    assert_eq!(config.consecutive_local_origin_failures, 2); // inherits gateway -> errors
}

#[test]
fn local_origin_failures_defaults_to_gateway_errors() {
    // Only the gateway threshold is set; local-origin inherits it (back-compat:
    // connect/proxy errors keep counting at the same threshold they used when they
    // shared the gateway bucket).
    let config = config_with(
        default_consecutive_errors(),
        Some(4),
        None,
        &default_ejection_time(),
        None,
    )
    .normalize();
    assert_eq!(config.consecutive_gateway_errors, 4);
    assert_eq!(config.consecutive_local_origin_failures, 4);
}

#[test]
fn local_origin_failures_explicit_overrides_inheritance() {
    let config = config_with(
        default_consecutive_errors(),
        Some(8),
        Some(2),
        &default_ejection_time(),
        None,
    )
    .normalize();
    assert_eq!(config.consecutive_gateway_errors, 8);
    assert_eq!(config.consecutive_local_origin_failures, 2);
}

#[test]
fn zero_local_origin_failures_rejected() {
    let config = OutlierDetectionResolved {
        consecutive_local_origin_failures: 0,
        ..OutlierDetectionResolved::default()
    };
    assert!(config.validate().is_some());
}

#[test]
fn max_ejection_time_defaults_to_max_5m_ejection() {
    let config = config_with(default_consecutive_errors(), None, None, "10m", None).normalize();
    assert_eq!(config.max_ejection_seconds, 600); // max(300, 600)
}

#[test]
fn max_ejection_time_clamps_up_not_rejects() {
    // Explicit max < ejection must be clamped up, NOT cause validation to fail.
    let config = config_with(default_consecutive_errors(), None, None, "1m", Some("10s")).normalize();
    assert_eq!(config.max_ejection_seconds, 60);
}

#[test]
fn invalid_ejection_time_rejected_by_validation() {
    // Bare integers (the pre-GEP-2257 form) and sub-second durations are
    // rejected by get_validation_error, not silently defaulted.
    let config = config_with(default_consecutive_errors(), None, None, "30", None);
    assert!(config.get_validation_error().is_some());
    let config = config_with(default_consecutive_errors(), None, None, "500ms", None);
    assert!(config.get_validation_error().is_some());
    let config = config_with(default_consecutive_errors(), None, None, "30s", Some("bogus"));
    assert!(config.get_validation_error().is_some());
    let config = config_with(default_consecutive_errors(), None, None, "30s", Some("5m"));
    assert!(config.get_validation_error().is_none());
}

#[test]
fn invalid_ejection_time_normalize_falls_back_to_defaults() {
    // Lenient admission can leak an unvalidated string into normalize;
    // it must fall back to the documented defaults, not panic.
    let config = config_with(default_consecutive_errors(), None, None, "bogus", None).normalize();
    assert_eq!(config.ejection_seconds, 30);
    assert_eq!(config.max_ejection_seconds, 300);
}

#[test]
fn defaults_all_fields() {
    let config = OutlierDetectionResolved::default();
    assert_eq!(config.consecutive_errors, 5);
    assert_eq!(config.consecutive_gateway_errors, 5);
    assert_eq!(config.consecutive_local_origin_failures, 5);
    assert_eq!(config.ejection_seconds, 30);
    assert_eq!(config.max_ejection_seconds, 300);
    assert_eq!(config.max_ejection_percent, 50);
    assert!(config.validate().is_none());
}

#[test]
fn zero_consecutive_errors_rejected() {
    let config = OutlierDetectionResolved {
        consecutive_errors: 0,
        ..OutlierDetectionResolved::default()
    };
    assert!(config.validate().is_some());
}
