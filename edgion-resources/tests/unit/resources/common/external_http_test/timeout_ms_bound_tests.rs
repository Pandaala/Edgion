use super::*;

/// The static `timeoutMs` field must share the same upper bound as the
/// template-rendered timeout (`MAX_OUTBOUND_TIMEOUT_MS`). Without this, a
/// `timeoutMs: 600000` + `maxRetries: 10` config would be admitted and
/// stall the data-plane request inline for ~110 minutes.
#[test]
fn rejects_timeout_ms_above_ceiling() {
    let c: ExternalHttpConnection =
        serde_yaml::from_str("target:\n  url: https://api.example.com\ntimeoutMs: 120000\n").unwrap();
    let err = validate_connection_target(&c).expect("120000 exceeds MAX_OUTBOUND_TIMEOUT_MS");
    assert!(err.contains("timeoutMs"), "got: {err}");
}

#[test]
fn accepts_reasonable_timeout_ms() {
    let c: ExternalHttpConnection =
        serde_yaml::from_str("target:\n  url: https://api.example.com\ntimeoutMs: 5000\n").unwrap();
    assert!(validate_connection_target(&c).is_none());
}

#[test]
fn accepts_timeout_ms_at_ceiling() {
    let c: ExternalHttpConnection = serde_yaml::from_str(&format!(
        "target:\n  url: https://api.example.com\ntimeoutMs: {MAX_OUTBOUND_TIMEOUT_MS}\n"
    ))
    .unwrap();
    assert!(validate_connection_target(&c).is_none());
}
