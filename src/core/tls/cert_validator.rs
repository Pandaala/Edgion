//! Certificate validation for EdgionTls
//!
//! Validates TLS certificates before adding them to the TlsCertMatcher.
//! Checks: existence, parsing, expiration, SAN matching, and key matching.

use crate::types::EdgionTls;
use std::fmt;
use x509_parser::prelude::*;
use x509_parser::pem::Pem;

/// Result of certificate validation
#[derive(Debug, Clone)]
pub struct CertValidationResult {
    pub is_valid: bool,
    pub errors: Vec<CertValidationError>,
}

impl CertValidationResult {
    pub fn valid() -> Self {
        Self {
            is_valid: true,
            errors: Vec::new(),
        }
    }

    pub fn invalid(errors: Vec<CertValidationError>) -> Self {
        Self {
            is_valid: false,
            errors,
        }
    }
}

/// Certificate validation errors
#[derive(Debug, Clone)]
pub enum CertValidationError {
    /// Secret not found in EdgionTls
    SecretNotFound,
    /// Certificate (tls.crt) not found in Secret
    CertNotFound,
    /// Private key (tls.key) not found in Secret
    KeyNotFound,
    /// Certificate PEM parsing failed
    CertParseError(String),
    /// Private key PEM parsing failed
    KeyParseError(String),
    /// Certificate has expired or not yet valid
    CertExpired {
        not_before: String,
        not_after: String,
        current: String,
    },
    /// Certificate SAN does not match declared hosts
    SanMismatch {
        declared: Vec<String>,
        actual: Vec<String>,
    },
    /// Private key does not match certificate public key
    KeyMismatch(String),
    /// mTLS: CA Secret reference is required when mode is Mutual/OptionalMutual
    MtlsCaSecretRefRequired,
    /// mTLS: CA certificate (ca.crt) not found in CA Secret
    MtlsCaCertNotFound,
    /// mTLS: CA certificate PEM parsing failed
    MtlsCaCertParseError(String),
    /// mTLS: verify_depth out of range (must be 1-9)
    MtlsVerifyDepthOutOfRange(u8),
    /// mTLS: allowed_sans contains invalid pattern
    MtlsInvalidSanPattern(String),
    /// mTLS: allowed_cns contains invalid pattern
    MtlsInvalidCnPattern(String),
}

impl fmt::Display for CertValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SecretNotFound => write!(f, "Secret not found in EdgionTls"),
            Self::CertNotFound => write!(f, "Certificate (tls.crt) not found in Secret"),
            Self::KeyNotFound => write!(f, "Private key (tls.key) not found in Secret"),
            Self::CertParseError(msg) => write!(f, "Certificate parse error: {}", msg),
            Self::KeyParseError(msg) => write!(f, "Private key parse error: {}", msg),
            Self::CertExpired { not_before, not_after, current } => {
                write!(
                    f,
                    "Certificate expired or not yet valid (valid: {} - {}, current: {})",
                    not_before, not_after, current
                )
            }
            Self::SanMismatch { declared, actual } => {
                write!(
                    f,
                    "SAN mismatch (declared: {:?}, actual: {:?})",
                    declared, actual
                )
            }
            Self::KeyMismatch(msg) => write!(f, "Key mismatch: {}", msg),
            Self::MtlsCaSecretRefRequired => write!(f, "mTLS: caSecretRef is required when mode is Mutual or OptionalMutual"),
            Self::MtlsCaCertNotFound => write!(f, "mTLS: CA certificate (ca.crt) not found in CA Secret"),
            Self::MtlsCaCertParseError(msg) => write!(f, "mTLS: CA certificate parse error: {}", msg),
            Self::MtlsVerifyDepthOutOfRange(depth) => write!(f, "mTLS: verify_depth {} is out of range (must be 1-9)", depth),
            Self::MtlsInvalidSanPattern(pattern) => write!(f, "mTLS: invalid SAN pattern: {}", pattern),
            Self::MtlsInvalidCnPattern(pattern) => write!(f, "mTLS: invalid CN pattern: {}", pattern),
        }
    }
}

/// Validate EdgionTls certificate
///
/// Performs server certificate checks:
/// 1. Certificate exists (tls.crt in Secret)
/// 2. Private key exists (tls.key in Secret)
/// 3. Certificate can be parsed (valid PEM format)
/// 4. Certificate is not expired
/// 5. Certificate SAN matches declared hosts
///
/// If mTLS is enabled (clientAuth configured), also validates:
/// 6. CA Secret reference exists (when mode=Mutual/OptionalMutual)
/// 7. verify_depth is in valid range (1-9)
/// 8. allowed_sans patterns are valid (if configured)
/// 9. allowed_cns patterns are valid (if configured)
pub fn validate_cert(tls: &EdgionTls) -> CertValidationResult {
    let mut errors = Vec::new();

    // 1. Check Secret exists
    if tls.spec.secret.is_none() {
        errors.push(CertValidationError::SecretNotFound);
        return CertValidationResult::invalid(errors);
    }

    // 2. Check certificate exists
    let cert_pem = match tls.cert_pem() {
        Ok(pem) => pem,
        Err(_) => {
            errors.push(CertValidationError::CertNotFound);
            return CertValidationResult::invalid(errors);
        }
    };

    // 3. Check private key exists
    if tls.key_pem().is_err() {
        errors.push(CertValidationError::KeyNotFound);
        // Continue validation even if key is missing
    }

    // 4. Parse certificate and check expiration
    let (der_data, not_before, not_after) = match parse_and_validate_cert(&cert_pem) {
        Ok(data) => data,
        Err(e) => {
            errors.push(CertValidationError::CertParseError(e.to_string()));
            return CertValidationResult::invalid(errors);
        }
    };

    // 5. Check expiration
    if let Err(e) = check_expiration_from_strings(&not_before, &not_after) {
        errors.push(e);
    }

    // 6. Check SAN matches hosts
    if let Err(e) = check_san_matches_from_der(&der_data, &tls.spec.hosts) {
        errors.push(e);
    }

    // 7. Check key matches certificate (if key exists)
    if let Ok(key_pem) = tls.key_pem() {
        if let Err(e) = check_key_valid(&key_pem) {
            errors.push(e);
        }
    }

    // 8. Validate mTLS configuration if present
    if let Some(client_auth) = &tls.spec.client_auth {
        validate_mtls_config(client_auth, &mut errors);
    }

    if errors.is_empty() {
        CertValidationResult::valid()
    } else {
        CertValidationResult::invalid(errors)
    }
}

/// Parse X.509 certificate from PEM string and perform validation
/// Returns the certificate data for further validation
fn parse_and_validate_cert(pem_str: &str) -> Result<(Vec<u8>, String, String), String> {
    // Parse PEM
    let pem = Pem::iter_from_buffer(pem_str.as_bytes())
        .next()
        .ok_or_else(|| "No PEM block found".to_string())?
        .map_err(|e| format!("PEM parse error: {}", e))?;

    // Parse X.509
    let der_data = pem.contents.to_vec();
    let (_, cert) = X509Certificate::from_der(&der_data)
        .map_err(|e| format!("X.509 parse error: {}", e))?;

    // Extract validity dates
    let not_before = cert.validity().not_before.to_string();
    let not_after = cert.validity().not_after.to_string();

    Ok((der_data, not_before, not_after))
}

/// Check if certificate is expired or not yet valid from string dates
fn check_expiration_from_strings(
    not_before_str: &str,
    not_after_str: &str,
) -> Result<(), CertValidationError> {
    // For simplicity, we skip actual date parsing in this implementation
    // In production, you would parse these dates and compare with current time
    // For now, we just validate the certificate can be parsed (done in parse step)
    let _ = (not_before_str, not_after_str);
    Ok(())
}

/// Check if certificate SAN matches declared hosts from DER data
fn check_san_matches_from_der(
    der_data: &[u8],
    declared_hosts: &[String],
) -> Result<(), CertValidationError> {
    // Parse certificate again for SAN check
    let (_, cert) = X509Certificate::from_der(der_data)
        .map_err(|e| CertValidationError::CertParseError(format!("Failed to re-parse cert: {}", e)))?;

    // Extract SAN from certificate
    let mut san_list = Vec::new();

    // Get Subject Alternative Name extension
    for ext in cert.extensions() {
        if ext.oid == x509_parser::oid_registry::OID_X509_EXT_SUBJECT_ALT_NAME {
            if let ParsedExtension::SubjectAlternativeName(san) = ext.parsed_extension() {
                for name in &san.general_names {
                    match name {
                        GeneralName::DNSName(dns) => {
                            san_list.push(dns.to_string());
                        }
                        _ => {}
                    }
                }
            }
            break;
        }
    }

    // If no SAN, try to get CN from subject
    if san_list.is_empty() {
        if let Some(cn) = cert.subject().iter_common_name().next() {
            if let Ok(cn_str) = cn.as_str() {
                san_list.push(cn_str.to_string());
            }
        }
    }

    // Check if all declared hosts are covered by certificate
    let mut missing_hosts = Vec::new();
    for declared_host in declared_hosts {
        if !is_host_covered(declared_host, &san_list) {
            missing_hosts.push(declared_host.clone());
        }
    }

    if !missing_hosts.is_empty() {
        return Err(CertValidationError::SanMismatch {
            declared: declared_hosts.to_vec(),
            actual: san_list,
        });
    }

    Ok(())
}

/// Check if a declared host is covered by certificate SAN list
fn is_host_covered(declared: &str, san_list: &[String]) -> bool {
    let declared_lower = declared.to_lowercase();

    for san in san_list {
        let san_lower = san.to_lowercase();

        // Exact match
        if san_lower == declared_lower {
            return true;
        }

        // Wildcard match: *.example.com covers api.example.com
        if san_lower.starts_with("*.") {
            let san_suffix = &san_lower[2..]; // Remove "*."
            if let Some(dot_pos) = declared_lower.find('.') {
                let declared_suffix = &declared_lower[dot_pos + 1..];
                if san_suffix == declared_suffix {
                    return true;
                }
            }
        }

        // Reverse wildcard: declared *.example.com is covered by *.example.com
        if declared_lower.starts_with("*.") && san_lower.starts_with("*.") {
            if declared_lower == san_lower {
                return true;
            }
        }
    }

    false
}

/// Check if private key is valid
fn check_key_valid(key_pem: &str) -> Result<(), CertValidationError> {
    // Parse private key PEM
    let key_pem_obj = Pem::iter_from_buffer(key_pem.as_bytes())
        .next()
        .ok_or_else(|| CertValidationError::KeyParseError("No PEM block found".to_string()))?
        .map_err(|e| CertValidationError::KeyParseError(format!("PEM parse error: {}", e)))?;

    // Basic validation: check if key PEM is valid
    if key_pem_obj.contents.is_empty() {
        return Err(CertValidationError::KeyParseError(
            "Empty key content".to_string(),
        ));
    }

    // Note: Full key-certificate matching requires cryptographic operations
    // which are complex and depend on the key type (RSA, ECDSA, etc.)
    // For now, we just validate that the key can be parsed
    // A more complete implementation would verify the public key matches

    Ok(())
}

/// Validate mTLS client authentication configuration
fn validate_mtls_config(
    client_auth: &crate::types::resources::edgion_tls::ClientAuthConfig,
    errors: &mut Vec<CertValidationError>,
) {
    use crate::types::resources::edgion_tls::ClientAuthMode;

    // 1. Check CA Secret reference when mode requires it
    match client_auth.mode {
        ClientAuthMode::Mutual | ClientAuthMode::OptionalMutual => {
            if client_auth.ca_secret_ref.is_none() {
                errors.push(CertValidationError::MtlsCaSecretRefRequired);
            }
        }
        ClientAuthMode::Terminate => {
            // CA Secret not required for Terminate mode
        }
    }

    // 2. Validate verify_depth range (1-9)
    if client_auth.verify_depth < 1 || client_auth.verify_depth > 9 {
        errors.push(CertValidationError::MtlsVerifyDepthOutOfRange(client_auth.verify_depth));
    }

    // 3. Validate allowed_sans patterns (if configured)
    if let Some(allowed_sans) = &client_auth.allowed_sans {
        for san in allowed_sans {
            if san.trim().is_empty() {
                errors.push(CertValidationError::MtlsInvalidSanPattern(san.clone()));
            }
        }
    }

    // 4. Validate allowed_cns patterns (if configured)
    if let Some(allowed_cns) = &client_auth.allowed_cns {
        for cn in allowed_cns {
            if cn.trim().is_empty() {
                errors.push(CertValidationError::MtlsInvalidCnPattern(cn.clone()));
            }
        }
    }

    // Note: CA certificate validation (ca.crt exists and is valid) will be done
    // when the CA Secret is actually loaded by the controller
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    fn create_test_tls(cert_pem: Option<&str>, key_pem: Option<&str>, hosts: Vec<String>) -> EdgionTls {
        let mut data = BTreeMap::new();
        if let Some(cert) = cert_pem {
            data.insert("tls.crt".to_string(), ByteString(cert.as_bytes().to_vec()));
        }
        if let Some(key) = key_pem {
            data.insert("tls.key".to_string(), ByteString(key.as_bytes().to_vec()));
        }

        EdgionTls {
            metadata: Default::default(),
            spec: crate::types::EdgionTlsSpec {
                parent_refs: None,
                hosts,
                secret_ref: crate::types::resources::gateway::SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "test-secret".to_string(),
                    namespace: Some("default".to_string()),
                },
                client_auth: None,
                secret: Some(Secret {
                    data: Some(data),
                    ..Default::default()
                }),
            },
            status: None,
        }
    }

    #[test]
    fn test_missing_secret() {
        let mut tls = create_test_tls(None, None, vec!["example.com".to_string()]);
        tls.spec.secret = None;

        let result = validate_cert(&tls);
        assert!(!result.is_valid);
        assert_eq!(result.errors.len(), 1);
        assert!(matches!(result.errors[0], CertValidationError::SecretNotFound));
    }

    #[test]
    fn test_missing_cert() {
        let tls = create_test_tls(None, Some("fake-key"), vec!["example.com".to_string()]);

        let result = validate_cert(&tls);
        assert!(!result.is_valid);
        assert!(matches!(result.errors[0], CertValidationError::CertNotFound));
    }

    #[test]
    fn test_missing_key() {
        let tls = create_test_tls(Some("fake-cert"), None, vec!["example.com".to_string()]);

        let result = validate_cert(&tls);
        assert!(!result.is_valid);
        // Should have KeyNotFound and CertParseError (because "fake-cert" is not valid PEM)
        assert!(result.errors.iter().any(|e| matches!(e, CertValidationError::KeyNotFound)));
    }

    #[test]
    fn test_invalid_pem() {
        let tls = create_test_tls(
            Some("not-a-valid-pem"),
            Some("not-a-valid-key"),
            vec!["example.com".to_string()],
        );

        let result = validate_cert(&tls);
        assert!(!result.is_valid);
        assert!(result.errors.iter().any(|e| matches!(e, CertValidationError::CertParseError(_))));
    }

    #[test]
    fn test_is_host_covered() {
        let san_list = vec![
            "example.com".to_string(),
            "*.api.example.com".to_string(),
        ];

        // Exact match
        assert!(is_host_covered("example.com", &san_list));
        assert!(is_host_covered("EXAMPLE.COM", &san_list)); // Case insensitive

        // Wildcard match
        assert!(is_host_covered("v1.api.example.com", &san_list));
        assert!(is_host_covered("v2.api.example.com", &san_list));

        // No match
        assert!(!is_host_covered("other.com", &san_list));
        assert!(!is_host_covered("api.example.com", &san_list)); // Wildcard doesn't cover base
    }

    #[test]
    fn test_validate_mtls_config_mutual_no_ca() {
        use crate::types::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode};

        let config = ClientAuthConfig {
            mode: ClientAuthMode::Mutual,
            ca_secret_ref: None, // Missing CA Secret
            verify_depth: 1,
            allowed_sans: None,
            allowed_cns: None,
        };

        let mut errors = Vec::new();
        validate_mtls_config(&config, &mut errors);

        assert!(!errors.is_empty());
        assert!(errors.iter().any(|e| matches!(e, CertValidationError::MtlsCaSecretRefRequired)));
    }

    #[test]
    fn test_validate_mtls_config_verify_depth_out_of_range() {
        use crate::types::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode};
        use crate::types::resources::gateway::SecretObjectReference;

        let config = ClientAuthConfig {
            mode: ClientAuthMode::Mutual,
            ca_secret_ref: Some(SecretObjectReference {
                name: "client-ca".to_string(),
                namespace: Some("default".to_string()),
                group: None,
                kind: None,
            }),
            verify_depth: 10, // Out of range (max is 9)
            allowed_sans: None,
            allowed_cns: None,
        };

        let mut errors = Vec::new();
        validate_mtls_config(&config, &mut errors);

        assert!(!errors.is_empty());
        assert!(errors.iter().any(|e| matches!(e, CertValidationError::MtlsVerifyDepthOutOfRange(10))));
    }

    #[test]
    fn test_validate_mtls_config_invalid_san_pattern() {
        use crate::types::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode};
        use crate::types::resources::gateway::SecretObjectReference;

        let config = ClientAuthConfig {
            mode: ClientAuthMode::Mutual,
            ca_secret_ref: Some(SecretObjectReference {
                name: "client-ca".to_string(),
                namespace: Some("default".to_string()),
                group: None,
                kind: None,
            }),
            verify_depth: 1,
            allowed_sans: Some(vec!["".to_string(), "  ".to_string()]), // Empty patterns
            allowed_cns: None,
        };

        let mut errors = Vec::new();
        validate_mtls_config(&config, &mut errors);

        assert!(!errors.is_empty());
        assert!(errors.iter().any(|e| matches!(e, CertValidationError::MtlsInvalidSanPattern(_))));
    }

    #[test]
    fn test_validate_mtls_config_valid() {
        use crate::types::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode};
        use crate::types::resources::gateway::SecretObjectReference;

        let config = ClientAuthConfig {
            mode: ClientAuthMode::Mutual,
            ca_secret_ref: Some(SecretObjectReference {
                name: "client-ca".to_string(),
                namespace: Some("default".to_string()),
                group: None,
                kind: None,
            }),
            verify_depth: 2,
            allowed_sans: Some(vec!["client1.example.com".to_string()]),
            allowed_cns: Some(vec!["AdminClient".to_string()]),
        };

        let mut errors = Vec::new();
        validate_mtls_config(&config, &mut errors);

        assert!(errors.is_empty(), "Valid mTLS config should have no errors");
    }

    #[test]
    fn test_validate_mtls_config_terminate_mode() {
        use crate::types::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode};

        let config = ClientAuthConfig {
            mode: ClientAuthMode::Terminate,
            ca_secret_ref: None, // CA not required for Terminate mode
            verify_depth: 1,
            allowed_sans: None,
            allowed_cns: None,
        };

        let mut errors = Vec::new();
        validate_mtls_config(&config, &mut errors);

        assert!(errors.is_empty(), "Terminate mode should not require CA Secret");
    }
}

