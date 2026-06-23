//! Backend SAN validation for upstream TLS connections.
//!
//! Validates server certificate SANs against BackendTLSPolicy SubjectAltName requirements.

use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};
use std::fmt;

/// Result of backend SAN validation.
pub struct BackendSanValidationResult {
    pub passed: bool,
    pub reason: String,
}

impl fmt::Display for BackendSanValidationResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.passed {
            write!(f, "SAN validation passed: {}", self.reason)
        } else {
            write!(f, "SAN validation failed: {}", self.reason)
        }
    }
}

/// Validate certificate SANs against policy SubjectAltNames.
///
/// - Empty `policy_sans` → pass (no constraints).
/// - For Hostname type: match cert DNS SANs (entries without known prefixes) against policy hostname,
///   supporting wildcard patterns like `*.example.com`.
/// - For URI type: match cert SANs prefixed with `uri:` against policy URI (exact match).
/// - ANY match across all policy SANs → pass.
pub fn validate_backend_sans(cert_sans: &[String], policy_sans: &[SubjectAltName]) -> BackendSanValidationResult {
    if policy_sans.is_empty() {
        return BackendSanValidationResult {
            passed: true,
            reason: "no SubjectAltName constraints specified".to_string(),
        };
    }

    for policy_san in policy_sans {
        match policy_san.san_type {
            SubjectAltNameType::Hostname => {
                if let Some(ref hostname_pattern) = policy_san.hostname {
                    for cert_san in cert_sans {
                        // Skip entries with known prefixes (IP:, uri:, email:)
                        if cert_san.starts_with("IP:") || cert_san.starts_with("uri:") || cert_san.starts_with("email:")
                        {
                            continue;
                        }
                        if super::matches_host_pattern(cert_san, hostname_pattern) {
                            return BackendSanValidationResult {
                                passed: true,
                                reason: format!(
                                    "cert SAN '{}' matches policy hostname '{}'",
                                    cert_san, hostname_pattern
                                ),
                            };
                        }
                    }
                }
            }
            SubjectAltNameType::URI => {
                if let Some(ref uri) = policy_san.uri {
                    for cert_san in cert_sans {
                        if let Some(cert_uri) = cert_san.strip_prefix("uri:") {
                            if cert_uri == uri {
                                return BackendSanValidationResult {
                                    passed: true,
                                    reason: format!("cert SAN URI '{}' matches policy URI '{}'", cert_uri, uri),
                                };
                            }
                        }
                    }
                }
            }
        }
    }

    BackendSanValidationResult {
        passed: false,
        reason: format!(
            "no cert SAN matched any of the {} policy SubjectAltName(s)",
            policy_sans.len()
        ),
    }
}

#[cfg(test)]
mod tests {
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
    }

    #[test]
    fn test_uri_no_match() {
        let cert_sans = vec!["uri:spiffe://cluster.local/ns/default/sa/other".to_string()];
        let policy = vec![uri_san("spiffe://cluster.local/ns/default/sa/backend")];
        let result = validate_backend_sans(&cert_sans, &policy);
        assert!(!result.passed);
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
    fn test_matches_hostname_exact_and_wildcard() {
        use crate::tls::validation::matches_host_pattern;
        assert!(matches_host_pattern("foo.example.com", "foo.example.com"));
        assert!(matches_host_pattern("foo.example.com", "*.example.com"));
        assert!(!matches_host_pattern("foo.bar.example.com", "*.example.com"));
        assert!(!matches_host_pattern("example.com", "*.example.com"));
    }
}
