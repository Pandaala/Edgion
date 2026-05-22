//! Certificate validation for EdgionTls
//!
//! Validates TLS certificates before adding them to the TlsCertMatcher.
//! Checks: existence, parsing, expiration, SAN matching, and key matching.

use crate::types::EdgionTls;
use std::fmt;
use x509_parser::pem::Pem;
use x509_parser::prelude::*;

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
    SanMismatch { declared: Vec<String>, actual: Vec<String> },
    /// Declared host is not a valid wildcard pattern (RFC 6125 §6.4.3 / RFC 5280 §4.2.1.6:
    /// only a single, left-most-label asterisk is permitted; multi-label wildcards such as
    /// `*.*.example.com` are explicitly forbidden)
    InvalidSanPattern { pattern: String },
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
    /// Chain certificate (intermediate CA) has expired or is not yet valid
    ChainCertExpired { index: usize, subject: String },
    /// Chain certificate parse/decode error
    ChainCertError { index: usize, detail: String },
}

impl fmt::Display for CertValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SecretNotFound => write!(f, "Secret not found in EdgionTls"),
            Self::CertNotFound => write!(f, "Certificate (tls.crt) not found in Secret"),
            Self::KeyNotFound => write!(f, "Private key (tls.key) not found in Secret"),
            Self::CertParseError(msg) => write!(f, "Certificate parse error: {}", msg),
            Self::KeyParseError(msg) => write!(f, "Private key parse error: {}", msg),
            Self::CertExpired {
                not_before,
                not_after,
                current,
            } => {
                write!(
                    f,
                    "Certificate expired or not yet valid (valid: {} - {}, current: {})",
                    not_before, not_after, current
                )
            }
            Self::SanMismatch { declared, actual } => {
                write!(f, "SAN mismatch (declared: {:?}, actual: {:?})", declared, actual)
            }
            Self::InvalidSanPattern { pattern } => {
                write!(
                    f,
                    "Pattern '{}' is not a valid wildcard per RFC 6125 §6.4.3 / RFC 5280 §4.2.1.6 \
                     (only a single leftmost asterisk is allowed)",
                    pattern
                )
            }
            Self::KeyMismatch(msg) => write!(f, "Key mismatch: {}", msg),
            Self::MtlsCaSecretRefRequired => {
                write!(f, "mTLS: caSecretRef is required when mode is Mutual or OptionalMutual")
            }
            Self::MtlsCaCertNotFound => write!(f, "mTLS: CA certificate (ca.crt) not found in CA Secret"),
            Self::MtlsCaCertParseError(msg) => write!(f, "mTLS: CA certificate parse error: {}", msg),
            Self::MtlsVerifyDepthOutOfRange(depth) => {
                write!(f, "mTLS: verify_depth {} is out of range (must be 1-9)", depth)
            }
            Self::MtlsInvalidSanPattern(pattern) => write!(f, "mTLS: invalid SAN pattern: {}", pattern),
            Self::MtlsInvalidCnPattern(pattern) => write!(f, "mTLS: invalid CN pattern: {}", pattern),
            Self::ChainCertExpired { index, subject } => {
                write!(
                    f,
                    "Chain certificate [{}] (subject: {}) is expired or not yet valid",
                    index, subject
                )
            }
            Self::ChainCertError { index, detail } => {
                write!(f, "Chain certificate [{}] error: {}", index, detail)
            }
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
    use crate::types::resources::edgion_tls::TlsVersion;

    let mut errors = Vec::new();

    // RFC 8996 formally deprecates TLS 1.0/1.1. We deliberately keep them as an
    // operator opt-in escape hatch (do NOT reject), but surface a config-time
    // warning so the downgrade is not silent. This runs on the config-load path
    // (not the data plane), once per apply.
    // See skills/04-review/rfc-divergence/tls-02-deprecated-tls-version-admission.md
    if let Some(v @ (TlsVersion::Tls10 | TlsVersion::Tls11)) = tls.spec.min_tls_version {
        tracing::warn!(
            component = "tls_validation",
            name = tls.metadata.name.as_deref().unwrap_or_default(),
            version = ?v,
            "EdgionTls min_tls_version is deprecated by RFC 8996; consider TLS 1.2+ \
             (connections still accepted — operator opt-in)"
        );
    }

    // `extend.revocationCheck` (RFC 5280 §6.3 CRL / RFC 6960 OCSP) and
    // `extend.ocspStapling` are reserved schema with no runtime implementation: the
    // gateway performs NO revocation lookups or OCSP stapling regardless of these
    // settings. We deliberately keep the fields (reserved for a future release; do NOT
    // hard-reject, which would take down a working TLS listener over a no-op field) but
    // surface a config-time warning so a compliance-minded operator does not silently
    // believe revocation is enforced. This runs on the config-load path (not the data
    // plane), once per apply.
    // See skills/04-review/rfc-divergence/tls-07-revocation-config-not-implemented.md
    if let Some(extend) = &tls.spec.extend {
        use crate::types::resources::edgion_tls::RevocationMode;
        let revocation_requested = extend
            .revocation_check
            .as_ref()
            .is_some_and(|rc| rc.mode != RevocationMode::Off);
        let stapling_requested = extend.ocsp_stapling.as_ref().is_some_and(|s| s.enabled);
        if revocation_requested || stapling_requested {
            tracing::warn!(
                component = "tls_validation",
                name = tls.metadata.name.as_deref().unwrap_or_default(),
                revocation_check = revocation_requested,
                ocsp_stapling = stapling_requested,
                "EdgionTls revocationCheck/ocspStapling is configured but NOT implemented; \
                 the gateway performs no OCSP/CRL revocation checks or OCSP stapling \
                 regardless of this setting (reserved schema; config still accepted)"
            );
        }
    }

    if tls.spec.secret.is_none() {
        errors.push(CertValidationError::SecretNotFound);
        return CertValidationResult::invalid(errors);
    }

    let cert_pem = match tls.cert_pem() {
        Ok(pem) => pem,
        Err(_) => {
            errors.push(CertValidationError::CertNotFound);
            return CertValidationResult::invalid(errors);
        }
    };

    if tls.key_pem().is_err() {
        errors.push(CertValidationError::KeyNotFound);
        // Continue validation even if key is missing
    }

    let (der_data, not_before_ts, not_after_ts) = match parse_leaf_cert(&cert_pem) {
        Ok(data) => data,
        Err(e) => {
            errors.push(CertValidationError::CertParseError(e));
            return CertValidationResult::invalid(errors);
        }
    };

    if let Err(e) = check_expiration(not_before_ts, not_after_ts) {
        errors.push(e);
    }

    check_chain_expiration(&cert_pem, &mut errors);

    // Detect structurally-invalid wildcard patterns up front. Without this they would fall
    // through to a generic SanMismatch, leaving the operator chasing a phantom certificate
    // problem instead of seeing that the wildcard syntax itself is unsupported. Only the
    // valid hosts are then checked against the certificate SAN.
    let (valid_hosts, invalid_patterns): (Vec<String>, Vec<String>) =
        tls.spec.hosts.iter().cloned().partition(|h| is_valid_san_pattern(h));
    for pattern in invalid_patterns {
        errors.push(CertValidationError::InvalidSanPattern { pattern });
    }

    // X509Certificate<'_> borrows from der_data, so we re-parse here rather than returning
    // the cert from parse_leaf_cert (which would require a self-referential struct).
    match X509Certificate::from_der(&der_data) {
        Ok((_, leaf_cert)) => {
            if let Err(e) = check_san_matches(&leaf_cert, &valid_hosts) {
                errors.push(e);
            }
        }
        Err(e) => {
            errors.push(CertValidationError::CertParseError(format!(
                "unexpected re-parse failure: {}",
                e
            )));
            return CertValidationResult::invalid(errors);
        }
    }

    if let Ok(key_pem) = tls.key_pem() {
        if let Err(e) = check_key_matches_cert(&cert_pem, &key_pem) {
            errors.push(e);
        }
    }

    if let Some(client_auth) = &tls.spec.client_auth {
        validate_mtls_config(client_auth, &mut errors);
    }

    if errors.is_empty() {
        CertValidationResult::valid()
    } else {
        CertValidationResult::invalid(errors)
    }
}

/// Parse leaf X.509 certificate from PEM string.
/// Returns (leaf_der_data, not_before_unix_ts, not_after_unix_ts)
fn parse_leaf_cert(pem_str: &str) -> Result<(Vec<u8>, i64, i64), String> {
    let pem = Pem::iter_from_buffer(pem_str.as_bytes())
        .next()
        .ok_or_else(|| "No PEM block found".to_string())?
        .map_err(|e| format!("PEM parse error: {}", e))?;

    let der_data = pem.contents.to_vec();
    let (_, cert) = X509Certificate::from_der(&der_data).map_err(|e| format!("X.509 parse error: {}", e))?;

    let not_before_ts = cert.validity().not_before.timestamp();
    let not_after_ts = cert.validity().not_after.timestamp();

    Ok((der_data, not_before_ts, not_after_ts))
}

/// Validate expiration of chain certificates (intermediate CAs) in a PEM bundle.
fn check_chain_expiration(pem_str: &str, errors: &mut Vec<CertValidationError>) {
    for (idx, pem_result) in Pem::iter_from_buffer(pem_str.as_bytes()).enumerate().skip(1) {
        let chain_pem = match pem_result {
            Ok(p) => p,
            Err(e) => {
                errors.push(CertValidationError::ChainCertError {
                    index: idx,
                    detail: format!("PEM parse error: {}", e),
                });
                continue;
            }
        };
        let (_, chain_cert) = match X509Certificate::from_der(&chain_pem.contents) {
            Ok(c) => c,
            Err(e) => {
                errors.push(CertValidationError::ChainCertError {
                    index: idx,
                    detail: format!("X.509 parse error: {}", e),
                });
                continue;
            }
        };

        let not_before = chain_cert.validity().not_before.timestamp();
        let not_after = chain_cert.validity().not_after.timestamp();

        if check_expiration(not_before, not_after).is_err() {
            errors.push(CertValidationError::ChainCertExpired {
                index: idx,
                subject: chain_cert.subject().to_string(),
            });
        }
    }
}

/// Check if certificate validity period covers current time
fn check_expiration(not_before_ts: i64, not_after_ts: i64) -> Result<(), CertValidationError> {
    use std::time::{SystemTime, UNIX_EPOCH};

    // unwrap_or_default(): if system clock is set before Unix epoch, duration_since returns Err
    // and we fall back to 0 (1970-01-01). Any cert with not_before > 0 will then be rejected,
    // which is the correct defensive behavior — a misconfigured clock should not allow certs through.
    // try_from/unwrap_or(i64::MAX): u64→i64 overflow only occurs after year ~292 billion; unreachable.
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(i64::MAX);

    // Both bounds are inclusive per RFC 5280: valid if not_before <= now <= not_after
    if now > not_after_ts || now < not_before_ts {
        let fmt = |ts: i64| {
            chrono::DateTime::from_timestamp(ts, 0)
                .map(|dt: chrono::DateTime<chrono::Utc>| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
                .unwrap_or_else(|| ts.to_string())
        };
        return Err(CertValidationError::CertExpired {
            not_before: fmt(not_before_ts),
            not_after: fmt(not_after_ts),
            current: fmt(now),
        });
    }

    Ok(())
}

/// Check if certificate SAN matches declared hosts
fn check_san_matches(cert: &X509Certificate, declared_hosts: &[String]) -> Result<(), CertValidationError> {
    let mut san_dns_list: Vec<String> = Vec::new();
    let mut san_ip_list: Vec<std::net::IpAddr> = Vec::new();
    let mut has_san_extension = false;

    for ext in cert.extensions() {
        if ext.oid == x509_parser::oid_registry::OID_X509_EXT_SUBJECT_ALT_NAME {
            has_san_extension = true;
            if let ParsedExtension::SubjectAlternativeName(san) = ext.parsed_extension() {
                for name in &san.general_names {
                    match name {
                        GeneralName::DNSName(dns) => {
                            san_dns_list.push(dns.to_string());
                        }
                        GeneralName::IPAddress(ip_bytes) => match ip_bytes.len() {
                            4 => {
                                san_ip_list.push(
                                    std::net::Ipv4Addr::new(ip_bytes[0], ip_bytes[1], ip_bytes[2], ip_bytes[3]).into(),
                                );
                            }
                            16 => {
                                if let Ok(octets) = <[u8; 16]>::try_from(*ip_bytes) {
                                    san_ip_list.push(std::net::Ipv6Addr::from(octets).into());
                                }
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
            }
            break;
        }
    }

    // CN fallback only when certificate has no SAN extension at all (legacy cert compat)
    if !has_san_extension {
        tracing::warn!(
            "Certificate for hosts {:?} has no SAN extension, falling back to CN matching. \
             This is deprecated per RFC 6125.",
            declared_hosts
        );
        if let Some(cn) = cert.subject().iter_common_name().next() {
            if let Ok(cn_str) = cn.as_str() {
                san_dns_list.push(cn_str.to_string());
            }
        }
    }

    // Check if all declared hosts are covered by certificate
    let mut missing_hosts = Vec::new();
    for declared_host in declared_hosts {
        let covered = if let Ok(declared_ip) = declared_host.parse::<std::net::IpAddr>() {
            // RFC 6125 §1.7.2: IP addresses use exact match, no wildcards
            san_ip_list.iter().any(|ip| *ip == declared_ip)
        } else {
            is_host_covered(declared_host, &san_dns_list)
        };
        if !covered {
            missing_hosts.push(declared_host.clone());
        }
    }

    if !missing_hosts.is_empty() {
        let mut all_sans = san_dns_list;
        all_sans.extend(san_ip_list.iter().map(|ip| format!("IP:{}", ip)));
        return Err(CertValidationError::SanMismatch {
            declared: declared_hosts.to_vec(),
            actual: all_sans,
        });
    }

    Ok(())
}

/// Check if a declared host is covered by certificate SAN list
fn is_host_covered(declared: &str, san_list: &[String]) -> bool {
    san_list.iter().any(|san| super::matches_host_pattern(declared, san))
}

/// Whether a declared host is a valid SAN pattern under RFC 6125 §6.4.3 / RFC 5280 §4.2.1.6.
///
/// A host with no `*` is always valid. A wildcard host is valid only when it is exactly
/// `*.<suffix>` where `<suffix>` is non-empty and contains no further `*` — i.e. a single,
/// left-most-label asterisk. Multi-label wildcards (`*.*.example.com`), partial-label
/// wildcards (`foo*.example.com`), and non-leftmost wildcards (`a.*.example.com`) are
/// rejected; these are exactly the patterns `matches_host_pattern` cannot match.
fn is_valid_san_pattern(host: &str) -> bool {
    if !host.contains('*') {
        return true;
    }
    match host.strip_prefix("*.") {
        Some(suffix) => !suffix.is_empty() && !suffix.contains('*'),
        None => false,
    }
}

/// Check if private key is valid and matches the certificate's public key
pub(crate) fn check_key_matches_cert(cert_pem: &str, key_pem: &str) -> Result<(), CertValidationError> {
    let key_pair = rcgen::KeyPair::from_pem(key_pem)
        .map_err(|e| CertValidationError::KeyParseError(format!("Failed to parse private key: {}", e)))?;

    let pem = Pem::iter_from_buffer(cert_pem.as_bytes())
        .next()
        .ok_or_else(|| CertValidationError::CertParseError("No PEM block found in cert".to_string()))?
        .map_err(|e| CertValidationError::CertParseError(format!("Cert PEM parse error: {}", e)))?;

    let (_, cert) = X509Certificate::from_der(&pem.contents)
        .map_err(|e| CertValidationError::CertParseError(format!("X.509 parse error: {}", e)))?;
    let cert_pubkey_raw = &cert.public_key().subject_public_key.data;
    let key_pubkey_raw = key_pair.public_key_raw();

    if cert_pubkey_raw.as_ref() != key_pubkey_raw {
        return Err(CertValidationError::KeyMismatch(
            "Private key does not match certificate public key".to_string(),
        ));
    }

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
    use crate::types::constants::secret_keys::tls::{CERT, KEY};
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    fn create_test_tls(cert_pem: Option<&str>, key_pem: Option<&str>, hosts: Vec<String>) -> EdgionTls {
        let mut data = BTreeMap::new();
        if let Some(cert) = cert_pem {
            data.insert(CERT.to_string(), ByteString(cert.as_bytes().to_vec()));
        }
        if let Some(key) = key_pem {
            data.insert(KEY.to_string(), ByteString(key.as_bytes().to_vec()));
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
                min_tls_version: None,
                ciphers: None,
                extend: None,
                secret: Some(Secret {
                    data: Some(data),
                    ..Default::default()
                }),
                resolved_listeners: None,
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
        assert!(result
            .errors
            .iter()
            .any(|e| matches!(e, CertValidationError::KeyNotFound)));
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
        assert!(result
            .errors
            .iter()
            .any(|e| matches!(e, CertValidationError::CertParseError(_))));
    }

    #[test]
    fn test_is_host_covered() {
        let san_list = vec!["example.com".to_string(), "*.api.example.com".to_string()];

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
    fn test_is_valid_san_pattern() {
        // No wildcard: always valid
        assert!(is_valid_san_pattern("example.com"));
        assert!(is_valid_san_pattern("api.staging.example.com"));

        // Single leftmost-label wildcard: valid
        assert!(is_valid_san_pattern("*.example.com"));
        assert!(is_valid_san_pattern("*.staging.example.com"));

        // Multi-label wildcard: invalid (RFC 6125 §6.4.3 explicitly forbidden)
        assert!(!is_valid_san_pattern("*.*.example.com"));
        assert!(!is_valid_san_pattern("*.*.*.example.com"));

        // Non-leftmost / partial-label wildcard: invalid
        assert!(!is_valid_san_pattern("a.*.example.com"));
        assert!(!is_valid_san_pattern("foo*.example.com"));

        // Degenerate wildcards: invalid
        assert!(!is_valid_san_pattern("*."));
        assert!(!is_valid_san_pattern("*"));
    }

    #[test]
    fn test_validate_cert_reports_invalid_san_pattern() {
        // A multi-star declared host must surface as InvalidSanPattern, not a generic
        // SanMismatch, so operators see the real problem (unsupported wildcard syntax).
        let (cert_pem, key_pem) = make_cert_with_sans(vec!["example.com"], vec![]);
        let tls = create_test_tls(
            Some(&cert_pem),
            Some(&key_pem),
            vec!["*.*.staging.example.com".to_string()],
        );

        let result = validate_cert(&tls);
        assert!(!result.is_valid, "multi-star host should be invalid");
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, CertValidationError::InvalidSanPattern { pattern } if pattern == "*.*.staging.example.com")),
            "should report InvalidSanPattern, got: {:?}",
            result.errors
        );
        // The invalid pattern must NOT also produce a SanMismatch for the same host.
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, CertValidationError::SanMismatch { .. })),
            "invalid pattern should not also report SanMismatch, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_validate_cert_single_wildcard_not_flagged() {
        // A valid single-label wildcard host that the cert covers must not be flagged.
        let (cert_pem, key_pem) = make_cert_with_sans(vec!["*.example.com"], vec![]);
        let tls = create_test_tls(Some(&cert_pem), Some(&key_pem), vec!["*.example.com".to_string()]);

        let result = validate_cert(&tls);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, CertValidationError::InvalidSanPattern { .. })),
            "valid single-label wildcard should not be flagged, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_validate_mtls_config_mutual_no_ca() {
        use crate::types::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode};

        let config = ClientAuthConfig {
            mode: ClientAuthMode::Mutual,
            ca_secret_ref: None, // Missing CA Secret
            verify_depth: 1,
            ca_secret: None,
            allowed_sans: None,
            allowed_cns: None,
        };

        let mut errors = Vec::new();
        validate_mtls_config(&config, &mut errors);

        assert!(!errors.is_empty());
        assert!(errors
            .iter()
            .any(|e| matches!(e, CertValidationError::MtlsCaSecretRefRequired)));
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
            ca_secret: None,
            allowed_sans: None,
            allowed_cns: None,
        };

        let mut errors = Vec::new();
        validate_mtls_config(&config, &mut errors);

        assert!(!errors.is_empty());
        assert!(errors
            .iter()
            .any(|e| matches!(e, CertValidationError::MtlsVerifyDepthOutOfRange(10))));
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
            ca_secret: None,
            allowed_sans: Some(vec!["".to_string(), "  ".to_string()]), // Empty patterns
            allowed_cns: None,
        };

        let mut errors = Vec::new();
        validate_mtls_config(&config, &mut errors);

        assert!(!errors.is_empty());
        assert!(errors
            .iter()
            .any(|e| matches!(e, CertValidationError::MtlsInvalidSanPattern(_))));
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
            ca_secret: None,
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
            ca_secret: None,
            allowed_sans: None,
            allowed_cns: None,
        };

        let mut errors = Vec::new();
        validate_mtls_config(&config, &mut errors);

        assert!(errors.is_empty(), "Terminate mode should not require CA Secret");
    }

    fn make_cert_with_validity(not_before_offset_days: i64, not_after_offset_days: i64) -> (String, String) {
        use rcgen::{CertificateParams, KeyPair};

        // Use absolute crate paths to avoid conflict with x509_parser::prelude::time module
        let now = ::time::OffsetDateTime::now_utc();
        let dur_before = ::time::Duration::days(not_before_offset_days);
        let dur_after = ::time::Duration::days(not_after_offset_days);

        let key_pair = KeyPair::generate().expect("rcgen key generation failed");
        let mut params = CertificateParams::new(vec!["example.com".to_string()]).expect("rcgen params failed");
        params.not_before = now + dur_before;
        params.not_after = now + dur_after;
        let cert = params.self_signed(&key_pair).expect("rcgen self-sign failed");
        (cert.pem(), key_pair.serialize_pem())
    }

    fn make_cert_with_sans(dns_names: Vec<&str>, ip_addrs: Vec<std::net::IpAddr>) -> (String, String) {
        use rcgen::{CertificateParams, KeyPair, SanType};

        let mut san_types: Vec<SanType> = dns_names
            .into_iter()
            .map(|name| SanType::DnsName(name.try_into().unwrap()))
            .collect();
        san_types.extend(ip_addrs.into_iter().map(SanType::IpAddress));

        let key_pair = KeyPair::generate().expect("key generation failed");
        let mut params = CertificateParams::new(vec![]).expect("params failed");
        params.subject_alt_names = san_types;
        let cert = params.self_signed(&key_pair).expect("self-sign failed");
        (cert.pem(), key_pair.serialize_pem())
    }

    fn make_cert_chain(
        leaf_not_before_days: i64,
        leaf_not_after_days: i64,
        intermediate_not_before_days: i64,
        intermediate_not_after_days: i64,
    ) -> (String, String) {
        use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};

        let now = ::time::OffsetDateTime::now_utc();

        // Generate intermediate CA
        let ca_key = KeyPair::generate().expect("CA key gen failed");
        let mut ca_params = CertificateParams::new(vec!["Edgion Test CA".to_string()]).expect("CA params failed");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.not_before = now + ::time::Duration::days(intermediate_not_before_days);
        ca_params.not_after = now + ::time::Duration::days(intermediate_not_after_days);
        let ca_cert = ca_params.self_signed(&ca_key).expect("CA self-sign failed");
        let ca_issuer = Issuer::from_params(&ca_params, &ca_key);

        // Generate leaf certificate signed by CA
        let leaf_key = KeyPair::generate().expect("leaf key gen failed");
        let mut leaf_params = CertificateParams::new(vec!["example.com".to_string()]).expect("leaf params failed");
        leaf_params.not_before = now + ::time::Duration::days(leaf_not_before_days);
        leaf_params.not_after = now + ::time::Duration::days(leaf_not_after_days);
        let leaf_cert = leaf_params.signed_by(&leaf_key, &ca_issuer).expect("leaf sign failed");

        // Chain: leaf + intermediate
        let chain_pem = format!("{}{}", leaf_cert.pem(), ca_cert.pem());
        (chain_pem, leaf_key.serialize_pem())
    }

    #[test]
    fn test_expired_cert_is_invalid() {
        // Cert expired yesterday
        let (cert_pem, key_pem) = make_cert_with_validity(-30, -1);
        let tls = create_test_tls(Some(&cert_pem), Some(&key_pem), vec!["example.com".to_string()]);

        let result = validate_cert(&tls);
        assert!(!result.is_valid, "expired cert should be invalid");
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, CertValidationError::CertExpired { .. })),
            "should report CertExpired, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_not_yet_valid_cert_is_invalid() {
        // Cert becomes valid tomorrow
        let (cert_pem, key_pem) = make_cert_with_validity(1, 365);
        let tls = create_test_tls(Some(&cert_pem), Some(&key_pem), vec!["example.com".to_string()]);

        let result = validate_cert(&tls);
        assert!(!result.is_valid, "not-yet-valid cert should be invalid");
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, CertValidationError::CertExpired { .. })),
            "should report CertExpired, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_valid_cert_passes_expiry_check() {
        // Cert valid since yesterday, expires in one year
        let (cert_pem, key_pem) = make_cert_with_validity(-1, 365);
        let tls = create_test_tls(Some(&cert_pem), Some(&key_pem), vec!["example.com".to_string()]);

        let result = validate_cert(&tls);
        // Only assert that there is no CertExpired error
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, CertValidationError::CertExpired { .. })),
            "valid cert should not report CertExpired, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_matching_key_cert_passes() {
        let (cert_pem, key_pem) = make_cert_with_validity(-1, 365);
        let result = check_key_matches_cert(&cert_pem, &key_pem);
        assert!(result.is_ok(), "matching cert/key should pass, got: {:?}", result);
    }

    #[test]
    fn test_mismatched_key_cert_fails() {
        let (cert_a, _key_a) = make_cert_with_validity(-1, 365);
        let (_cert_b, key_b) = make_cert_with_validity(-1, 365);

        let result = check_key_matches_cert(&cert_a, &key_b);
        assert!(result.is_err(), "mismatched cert/key should fail");
        assert!(
            matches!(result, Err(CertValidationError::KeyMismatch(_))),
            "should return KeyMismatch, got: {:?}",
            result
        );
    }

    #[test]
    fn test_validate_cert_rejects_mismatched_key() {
        let (cert_a, _key_a) = make_cert_with_validity(-1, 365);
        let (_cert_b, key_b) = make_cert_with_validity(-1, 365);

        let tls = create_test_tls(Some(&cert_a), Some(&key_b), vec!["example.com".to_string()]);
        let result = validate_cert(&tls);

        assert!(!result.is_valid, "mismatched cert/key should be invalid");
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, CertValidationError::KeyMismatch(_))),
            "should report KeyMismatch, got: {:?}",
            result.errors
        );
    }

    // --- IP SAN tests ---

    #[test]
    fn test_ip_san_ipv4_match() {
        let ip: std::net::IpAddr = "10.0.0.1".parse().unwrap();
        let (cert_pem, key_pem) = make_cert_with_sans(vec![], vec![ip]);
        let tls = create_test_tls(Some(&cert_pem), Some(&key_pem), vec!["10.0.0.1".to_string()]);

        let result = validate_cert(&tls);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, CertValidationError::SanMismatch { .. })),
            "IPv4 SAN should match, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_ip_san_ipv6_match() {
        let ip: std::net::IpAddr = "fd00::1".parse().unwrap();
        let (cert_pem, key_pem) = make_cert_with_sans(vec![], vec![ip]);
        let tls = create_test_tls(Some(&cert_pem), Some(&key_pem), vec!["fd00::1".to_string()]);

        let result = validate_cert(&tls);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, CertValidationError::SanMismatch { .. })),
            "IPv6 SAN should match, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_mixed_dns_and_ip_san() {
        let ip: std::net::IpAddr = "10.0.0.1".parse().unwrap();
        let (cert_pem, key_pem) = make_cert_with_sans(vec!["example.com"], vec![ip]);
        let tls = create_test_tls(
            Some(&cert_pem),
            Some(&key_pem),
            vec!["example.com".to_string(), "10.0.0.1".to_string()],
        );

        let result = validate_cert(&tls);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, CertValidationError::SanMismatch { .. })),
            "mixed DNS+IP SAN should both match, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_ip_host_not_matched_by_dns_san() {
        let (cert_pem, key_pem) = make_cert_with_sans(vec!["example.com"], vec![]);
        let tls = create_test_tls(Some(&cert_pem), Some(&key_pem), vec!["10.0.0.1".to_string()]);

        let result = validate_cert(&tls);
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, CertValidationError::SanMismatch { .. })),
            "IP host should not match DNS SAN, got: {:?}",
            result.errors
        );
    }

    // --- Certificate chain tests ---

    #[test]
    fn test_cert_chain_valid() {
        let (chain_pem, key_pem) = make_cert_chain(-1, 365, -30, 3650);
        let tls = create_test_tls(Some(&chain_pem), Some(&key_pem), vec!["example.com".to_string()]);

        let result = validate_cert(&tls);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, CertValidationError::CertParseError(_))),
            "valid chain should pass, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_cert_chain_expired_intermediate() {
        // Leaf valid, intermediate expired yesterday
        let (chain_pem, key_pem) = make_cert_chain(-1, 365, -3650, -1);
        let tls = create_test_tls(Some(&chain_pem), Some(&key_pem), vec!["example.com".to_string()]);

        let result = validate_cert(&tls);
        assert!(!result.is_valid, "expired intermediate should be invalid");
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, CertValidationError::ChainCertExpired { .. })),
            "should report ChainCertExpired, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_single_cert_still_works() {
        // Single cert (no chain) should still work as before
        let (cert_pem, key_pem) = make_cert_with_validity(-1, 365);
        let tls = create_test_tls(Some(&cert_pem), Some(&key_pem), vec!["example.com".to_string()]);

        let result = validate_cert(&tls);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, CertValidationError::CertParseError(_))),
            "single cert should still work, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_deprecated_min_tls_version_is_not_rejected() {
        use crate::types::resources::edgion_tls::TlsVersion;

        // RFC 8996 divergence: TLS 1.0/1.1 stay admissible as an operator opt-in
        // escape hatch. Setting them must emit a warn but must NOT invalidate the
        // resource. This test guards the "do not hard-reject" decision.
        let (cert_pem, key_pem) = make_cert_with_validity(-1, 365);
        let mut tls = create_test_tls(Some(&cert_pem), Some(&key_pem), vec!["example.com".to_string()]);
        tls.spec.min_tls_version = Some(TlsVersion::Tls10);

        let result = validate_cert(&tls);
        assert!(
            result.is_valid,
            "deprecated min_tls_version (TLS 1.0) must not be rejected, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_unimplemented_revocation_config_is_not_rejected() {
        use crate::types::resources::edgion_tls::{
            EdgionTlsExtend, OcspStaplingConfig, RevocationCheckConfig, RevocationMode,
        };

        // tls-07 divergence: revocationCheck/ocspStapling are reserved-but-unimplemented
        // schema. Setting them must emit a config-time warn but must NOT invalidate the
        // resource (hard-rejecting would take down a working TLS listener over a no-op
        // field). This test guards the "do not hard-reject" decision.
        let (cert_pem, key_pem) = make_cert_with_validity(-1, 365);
        let mut tls = create_test_tls(Some(&cert_pem), Some(&key_pem), vec!["example.com".to_string()]);
        tls.spec.extend = Some(EdgionTlsExtend {
            prefer_server_ciphers: false,
            ocsp_stapling: Some(OcspStaplingConfig {
                enabled: true,
                refresh_interval_seconds: None,
                fail_open: false,
            }),
            session_ticket: None,
            session_cache: None,
            revocation_check: Some(RevocationCheckConfig {
                mode: RevocationMode::Ocsp,
                fail_open: false,
            }),
            early_data: None,
        });

        let result = validate_cert(&tls);
        assert!(
            result.is_valid,
            "unimplemented revocationCheck/ocspStapling must not be rejected, got: {:?}",
            result.errors
        );
    }
}
