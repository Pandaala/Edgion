use super::*;

fn hostname_san(hostname: &str) -> SubjectAltName {
    SubjectAltName {
        san_type: SubjectAltNameType::Hostname,
        hostname: Some(hostname.to_string()),
        uri: None,
    }
}

fn uri_san(uri: &str) -> SubjectAltName {
    SubjectAltName {
        san_type: SubjectAltNameType::URI,
        hostname: None,
        uri: Some(uri.to_string()),
    }
}

#[test]
fn test_empty_policy_passes() {
    let cert_sans = vec!["example.com".to_string()];
    let result = validate_backend_sans(&cert_sans, &[]);
    assert!(result.passed);
}

#[test]
fn test_hostname_exact_match() {
    let cert_sans = vec!["backend.example.com".to_string()];
    let policy = vec![hostname_san("backend.example.com")];
    let result = validate_backend_sans(&cert_sans, &policy);
    assert!(result.passed);
    assert_eq!(result.reason, "certificate hostname SAN matched policy");
}

#[test]
fn test_hostname_wildcard_match() {
    let cert_sans = vec!["backend.example.com".to_string()];
    let policy = vec![hostname_san("*.example.com")];
    let result = validate_backend_sans(&cert_sans, &policy);
    assert!(result.passed);
}

#[test]
fn test_hostname_no_match() {
    let cert_sans = vec!["other.example.com".to_string()];
    let policy = vec![hostname_san("backend.example.com")];
    let result = validate_backend_sans(&cert_sans, &policy);
    assert!(!result.passed);
}

#[test]
fn test_uri_exact_match() {
    let cert_sans = vec!["uri:spiffe://cluster.local/ns/default/sa/backend".to_string()];
    let policy = vec![uri_san("spiffe://cluster.local/ns/default/sa/backend")];
    let result = validate_backend_sans(&cert_sans, &policy);
    assert!(result.passed);
    assert_eq!(result.reason, "certificate URI SAN matched policy");
}

#[test]
fn test_uri_no_match() {
    let cert_sans = vec!["uri:spiffe://cluster.local/ns/default/sa/other".to_string()];
    let policy = vec![uri_san("spiffe://cluster.local/ns/default/sa/backend")];
    let result = validate_backend_sans(&cert_sans, &policy);
    assert!(!result.passed);
    assert_eq!(result.reason, "no certificate SAN matched policy");
}

#[test]
fn test_mixed_san_any_match() {
    let cert_sans = vec![
        "other.example.com".to_string(),
        "uri:spiffe://cluster.local/ns/default/sa/backend".to_string(),
    ];
    let policy = vec![
        hostname_san("backend.example.com"),
        uri_san("spiffe://cluster.local/ns/default/sa/backend"),
    ];
    let result = validate_backend_sans(&cert_sans, &policy);
    assert!(result.passed, "should pass because URI matches");
}

#[test]
fn test_skip_ip_and_email_entries() {
    let cert_sans = vec!["IP:10.0.0.1".to_string(), "email:admin@example.com".to_string()];
    let policy = vec![hostname_san("IP:10.0.0.1")];
    let result = validate_backend_sans(&cert_sans, &policy);
    assert!(
        !result.passed,
        "IP and email entries should be skipped for hostname matching"
    );
}

#[test]
fn test_skip_other_name_entries_for_hostname() {
    let cert_sans = vec!["otherName:1.3.6.1.4.1.311.20.2.3=backend.example.com".to_string()];
    let policy = vec![hostname_san("backend.example.com")];
    let result = validate_backend_sans(&cert_sans, &policy);
    assert!(
        !result.passed,
        "otherName display strings must not satisfy hostname pinning"
    );
}

#[test]
fn test_matches_hostname_exact_and_wildcard() {
    use edgion_algo::matcher::rfc6125_hostname::matches;
    assert!(matches("foo.example.com", "foo.example.com"));
    assert!(matches("*.example.com", "foo.example.com"));
    assert!(!matches("*.example.com", "foo.bar.example.com"));
    assert!(!matches("*.example.com", "example.com"));
}
