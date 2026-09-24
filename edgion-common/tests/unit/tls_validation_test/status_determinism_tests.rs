use super::*;

#[test]
fn cert_expired_status_message_is_deterministic_and_clockfree() {
    let err = CertValidationError::CertExpired {
        not_before: "2020-01-01T00:00:00Z".to_string(),
        not_after: "2020-02-01T00:00:00Z".to_string(),
        current: "2026-06-13T10:00:00Z".to_string(),
    };
    let m1 = err.status_message();
    let m2 = err.status_message();
    assert!(
        !m1.contains("2026-06-13"),
        "status_message must not contain current time: {m1}"
    );
    assert!(!m1.contains("current"), "status_message must not mention current: {m1}");
    assert_eq!(m1, m2, "status_message must be stable across calls");
    assert!(m1.contains("2020-01-01") && m1.contains("2020-02-01"));
    assert_eq!(err.status_reason(), "CertExpired");
}
