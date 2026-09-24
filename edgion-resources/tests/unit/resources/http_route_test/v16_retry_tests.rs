use super::HTTPRouteRetry;

#[test]
fn retry_attempts_and_codes_follow_v16_experimental_admission() {
    let retry = |attempts, codes| HTTPRouteRetry {
        attempts,
        backoff: None,
        codes,
    };
    assert!(retry(Some(1), Some(vec![500, 502, 503, 504]))
        .validation_error()
        .is_none());
    assert!(retry(Some(0), None).validation_error().unwrap().contains("at least 1"));
    assert!(retry(Some(-1), None).validation_error().is_some());
    assert!(retry(None, Some(vec![503, 503]))
        .validation_error()
        .unwrap()
        .contains("unique"));
    assert!(retry(None, Some(vec![399])).validation_error().is_some());
}
