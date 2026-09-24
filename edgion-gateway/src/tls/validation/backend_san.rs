//! Backend SAN validation for upstream TLS connections.
//!
//! Validates server certificate SANs against BackendTLSPolicy SubjectAltName requirements.

use edgion_algo::matcher::rfc6125_hostname::matches;
use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};
use std::fmt;

/// Result of backend SAN validation.
pub struct BackendSanValidationResult {
    pub passed: bool,
    pub reason: &'static str,
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
            reason: "no SubjectAltName constraints specified",
        };
    }

    for policy_san in policy_sans {
        match policy_san.san_type {
            SubjectAltNameType::Hostname => {
                if let Some(ref hostname_pattern) = policy_san.hostname {
                    for cert_san in cert_sans {
                        if edgion_common::allowed_san::has_typed_san_display_prefix(cert_san) {
                            continue;
                        }
                        if matches(hostname_pattern, cert_san) {
                            return BackendSanValidationResult {
                                passed: true,
                                reason: "certificate hostname SAN matched policy",
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
                                    reason: "certificate URI SAN matched policy",
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
        reason: "no certificate SAN matched policy",
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/tls/validation/backend_san_test.rs"]
mod tests;
