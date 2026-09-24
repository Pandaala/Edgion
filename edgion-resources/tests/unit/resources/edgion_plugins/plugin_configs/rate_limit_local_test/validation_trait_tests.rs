use super::*;
use crate::resources::edgion_plugins::PluginValidation;

#[test]
fn validation_error_returns_field_error_without_validate_call() {
    let config = RateLimitLocalConfig {
        rate: 0,
        ..Default::default()
    };
    // Should report an error about rate, without any validate() call.
    assert!(config.validation_error().is_some(), "rate=0 must be invalid");
}

#[test]
fn validation_error_does_not_treat_clamp_as_error() {
    let config = RateLimitLocalConfig {
        skew_tolerance: 5.0, // Will be clamped at runtime, NOT an error.
        ..Default::default()
    };
    assert!(
        config.validation_error().is_none(),
        "skew_tolerance clamp must not surface as preparse error"
    );
}

#[test]
fn rate_field_has_minimum_1_in_json_schema() {
    let schema = serde_json::to_value(schemars::schema_for!(RateLimitLocalConfig)).unwrap();
    let minimum = schema["properties"]["rate"]["minimum"]
        .as_f64()
        .expect("rate field must have a numeric minimum in JSON Schema");
    assert_eq!(minimum, 1.0, "rate minimum should be 1");
}

#[test]
fn reject_status_field_has_range_in_json_schema() {
    let schema = serde_json::to_value(schemars::schema_for!(RateLimitLocalConfig)).unwrap();
    let props = &schema["properties"]["rejectStatus"];
    assert_eq!(
        props["minimum"].as_f64(),
        Some(200.0),
        "rejectStatus minimum should be 200"
    );
    assert_eq!(
        props["maximum"].as_f64(),
        Some(599.0),
        "rejectStatus maximum should be 599"
    );
}
