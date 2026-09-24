use super::*;

#[test]
fn retry_new_fields_default() {
    let r: WebhookRetry = serde_yaml::from_str("maxRetries: 3\n").unwrap();
    assert!(matches!(r.backoff_policy, BackoffPolicy::Exponential));
    assert_eq!(r.max_delay_ms, 1000);
    assert!(r.jitter);
    assert!(!r.honor_retry_after);
    assert_eq!(r.retry_on_timeout, None); // per-consumer default resolved later
}

#[test]
fn retry_rejects_excessive_max_delay() {
    let r: WebhookRetry = serde_yaml::from_str("maxRetries: 1\nmaxDelayMs: 20000\n").unwrap();
    let err = validate_retry(&r).expect("20000 exceeds ceiling");
    assert!(err.contains("maxDelayMs"), "got: {err}");
}

#[test]
fn retry_rejects_zero_base_delay() {
    let r: WebhookRetry = serde_yaml::from_str("maxRetries: 1\nretryDelayMs: 0\n").unwrap();
    assert!(validate_retry(&r).is_some());
}

#[test]
fn retry_policy_parses_lowercase() {
    let r: WebhookRetry = serde_yaml::from_str("maxRetries: 1\nbackoffPolicy: fixed\n").unwrap();
    assert!(matches!(r.backoff_policy, BackoffPolicy::Fixed));
}

/// IMPORTANT 4 regression: `maxRetries: 200` + `maxDelayMs: 10000` would allow
/// ~33 minutes of inline `tokio::time::sleep` on a live data-plane request —
/// the very multi-minute stall `MAX_RETRY_BACKOFF_CEILING_MS`'s doc says must
/// be impossible. `maxRetries` itself must also be bounded.
#[test]
fn retry_rejects_excessive_max_retries() {
    let r: WebhookRetry = serde_yaml::from_str("maxRetries: 200\n").unwrap();
    let err = validate_retry(&r).expect("200 exceeds the maxRetries ceiling");
    assert!(err.contains("maxRetries"), "got: {err}");
}

#[test]
fn retry_accepts_max_retries_at_ceiling() {
    let r: WebhookRetry = serde_yaml::from_str(&format!("maxRetries: {MAX_RETRIES_CEILING}\n")).unwrap();
    assert!(validate_retry(&r).is_none());
}
