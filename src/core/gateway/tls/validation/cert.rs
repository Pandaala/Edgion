//! Certificate validation for EdgionTls
//!
//! Validates TLS certificates before adding them to the TlsCertMatcher.
//! Checks: existence, parsing, expiration, SAN matching, and key matching.

use edgion_resources::EdgionTls;
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
    /// No hosts declared in `spec.hosts`; certificate SAN cannot be validated.
    /// Without a declared host, SAN matching would be vacuously satisfied, so this is
    /// reported as a configuration error (defense-in-depth; the CRD schema normally
    /// enforces non-empty hosts).
    NoHostsDeclared,
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
            Self::NoHostsDeclared => {
                write!(
                    f,
                    "No hosts declared in spec.hosts; certificate SAN cannot be validated"
                )
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
    use edgion_resources::resources::edgion_tls::TlsVersion;

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
        use edgion_resources::resources::edgion_tls::RevocationMode;
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

    // A genuinely empty `spec.hosts` is a latent misconfiguration: with no declared host,
    // SAN matching is vacuously satisfied (the loop in check_san_matches never runs and it
    // returns Ok). The CRD schema enforces non-empty hosts and the matcher skips no-host
    // certs, so this guard is defense-in-depth for a schema-bypass / future-optional-field
    // scenario. This is intentionally keyed on `spec.hosts`, NOT on `valid_hosts` below:
    // `valid_hosts` is also empty when every declared host is an invalid wildcard, but that
    // case is already reported via InvalidSanPattern and must not double-report here.
    if tls.spec.hosts.is_empty() {
        errors.push(CertValidationError::NoHostsDeclared);
    }

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

    // Check if all declared hosts are covered by certificate.
    // An empty `declared_hosts` here means every host in spec.hosts was an invalid wildcard
    // (already reported as InvalidSanPattern by the caller), so returning Ok avoids double-
    // reporting. A genuinely empty spec.hosts is caught upstream as NoHostsDeclared.
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
impl From<crate::core::common::tls_validation::KeyCertMatchError> for CertValidationError {
    fn from(e: crate::core::common::tls_validation::KeyCertMatchError) -> Self {
        use crate::core::common::tls_validation::KeyCertMatchError;
        match e {
            KeyCertMatchError::CertParse(msg) => CertValidationError::CertParseError(msg),
            KeyCertMatchError::KeyParse(msg) => CertValidationError::KeyParseError(msg),
            KeyCertMatchError::Mismatch(msg) => CertValidationError::KeyMismatch(msg),
        }
    }
}

pub(crate) fn check_key_matches_cert(cert_pem: &str, key_pem: &str) -> Result<(), CertValidationError> {
    crate::core::common::tls_validation::check_key_matches_cert(cert_pem, key_pem).map_err(CertValidationError::from)
}

/// Validate mTLS client authentication configuration
fn validate_mtls_config(
    client_auth: &edgion_resources::resources::edgion_tls::ClientAuthConfig,
    errors: &mut Vec<CertValidationError>,
) {
    use edgion_resources::resources::edgion_tls::ClientAuthMode;

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

// Unit tests for this module live in `tests/cert_test.rs`.
#[cfg(test)]
#[path = "tests/cert_test.rs"]
mod tests;
