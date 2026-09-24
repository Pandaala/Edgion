//! Shared TLS certificate/key validation helpers.
//!
//! `check_key_matches_cert` is a pure crypto/PEM validator (no proxy runtime
//! state) used by both the control plane (resource ingest validation) and the
//! data plane (TLS config validation). It lives in `common` so the controller
//! does not need to reach into the gateway module for it.
//!
//! `validate_cert`, `CertValidationResult`, and `CertValidationError` perform
//! full EdgionTls certificate validation and also live here so the controller
//! can call them directly (the crate DAG is resources → common → {gateway,
//! controller}; the controller must not depend on the gateway).
//!
//! Both `validate_cert` and `check_key_matches_cert` are pure, side-effect-free
//! functions — they emit no tracing calls. Advisory warnings for deprecated or
//! unimplemented config fields (deprecated TLS version, reserved
//! revocation/OCSP schema, no-SAN CN fallback) are emitted by a caller-side
//! helper in the gateway.

use rustls::pki_types::{pem::PemObject, PrivateKeyDer};
use std::fmt;
use x509_parser::pem::Pem;
use x509_parser::prelude::*;

use edgion_algo::matcher::rfc6125_hostname::matches;
use edgion_resources::EdgionTls;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaBundleError {
    Empty,
    TooLarge,
    TooManyCertificates,
    InvalidUtf8,
    UnexpectedContent,
    TruncatedPem,
    InvalidPem,
    NonCertificatePem,
    InvalidCertificate,
}

/// Immutable canonical CA bundle shared by Controller validation and Gateway
/// cold-path trust compilation. Only complete CERTIFICATE PEM blocks are
/// accepted; any non-whitespace prefix, suffix, private-key block, truncated
/// block, or invalid X.509 DER rejects the whole bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalCaBundle {
    pub certificates_der: Vec<Vec<u8>>,
}

/// Ordered serving certificate chain. The first certificate is always the
/// leaf whose public key must match the configured private key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrictCertificateChain {
    pub certificates_der: Vec<Vec<u8>>,
    /// Leaf validity bounds compiled to Unix seconds for allocation-free
    /// handshake checks.
    pub leaf_not_before_unix: i64,
    pub leaf_not_after_unix: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivateKeyFormat {
    Pkcs8,
    RsaPkcs1,
    EcSec1,
}

/// Strictly parsed private key. DER is retained so a TLS backend can compile
/// the key once on its configuration cold path.
pub struct StrictPrivateKey {
    pub der: PrivateKeyDer<'static>,
    pub format: PrivateKeyFormat,
    public_key_spki_der: Vec<u8>,
}

pub const MAX_CA_REFERENCE_COUNT: usize = 16;
pub const MAX_CA_BUNDLE_BYTES_PER_REF: usize = 1024 * 1024;
pub const MAX_CA_BUNDLE_BYTES_AGGREGATE: usize = 1024 * 1024;
pub const MAX_CA_CERTIFICATES_PER_REF: usize = 1024;
pub const MAX_CA_CERTIFICATES_AGGREGATE: usize = 1024;
pub const MAX_PRIVATE_KEY_BYTES: usize = 1024 * 1024;
pub const MAX_SERVING_CERTIFICATE_REFS: usize = 64;

fn parse_ordered_certificate_pem(input: &[u8]) -> Result<Vec<Vec<u8>>, CaBundleError> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";

    if input.len() > MAX_CA_BUNDLE_BYTES_PER_REF {
        return Err(CaBundleError::TooLarge);
    }
    let text = std::str::from_utf8(input).map_err(|_| CaBundleError::InvalidUtf8)?;
    let mut remaining = text;
    let mut certificates_der = Vec::new();
    loop {
        remaining = remaining.trim_start_matches(char::is_whitespace);
        if remaining.is_empty() {
            break;
        }
        if !remaining.starts_with(BEGIN) {
            return Err(CaBundleError::UnexpectedContent);
        }
        let end_offset = remaining.find(END).ok_or(CaBundleError::TruncatedPem)? + END.len();
        let block = &remaining[..end_offset];
        let (rest, pem) = x509_parser::pem::parse_x509_pem(block.as_bytes()).map_err(|_| CaBundleError::InvalidPem)?;
        if !rest.iter().all(u8::is_ascii_whitespace) || pem.label != "CERTIFICATE" {
            return Err(CaBundleError::NonCertificatePem);
        }
        let (der_rest, _) = X509Certificate::from_der(&pem.contents).map_err(|_| CaBundleError::InvalidCertificate)?;
        if !der_rest.is_empty() {
            return Err(CaBundleError::InvalidCertificate);
        }
        if certificates_der.len() >= MAX_CA_CERTIFICATES_PER_REF {
            return Err(CaBundleError::TooManyCertificates);
        }
        certificates_der.push(pem.contents.clone());
        remaining = &remaining[end_offset..];
    }
    if certificates_der.is_empty() {
        return Err(CaBundleError::Empty);
    }
    Ok(certificates_der)
}

pub fn parse_canonical_ca_bundle(input: &[u8]) -> Result<CanonicalCaBundle, CaBundleError> {
    let mut certificates_der = parse_ordered_certificate_pem(input)?;
    certificates_der.sort();
    certificates_der.dedup();
    Ok(CanonicalCaBundle { certificates_der })
}

/// Strict complete certificate-chain parser for serving certificates.
/// Unlike the CA-named entry point, this communicates that leaf/intermediate
/// chains are accepted and does not assign trust-anchor semantics.
pub fn parse_strict_certificate_chain(input: &[u8]) -> Result<StrictCertificateChain, CaBundleError> {
    let certificates_der = parse_ordered_certificate_pem(input)?;
    let (_, leaf) = X509Certificate::from_der(&certificates_der[0]).map_err(|_| CaBundleError::InvalidCertificate)?;
    let leaf_not_before_unix = leaf.validity().not_before.timestamp();
    let leaf_not_after_unix = leaf.validity().not_after.timestamp();
    Ok(StrictCertificateChain {
        certificates_der,
        leaf_not_before_unix,
        leaf_not_after_unix,
    })
}

pub fn parse_strict_private_key_pem(input: &[u8]) -> Result<StrictPrivateKey, KeyCertMatchError> {
    if input.len() > MAX_PRIVATE_KEY_BYTES {
        return Err(KeyCertMatchError::KeyParse(
            "private key exceeds the byte limit".to_string(),
        ));
    }
    let text = std::str::from_utf8(input)
        .map_err(|_| KeyCertMatchError::KeyParse("private key is not UTF-8 PEM".to_string()))?;
    let trimmed = text.trim();
    let labels = ["PRIVATE KEY", "RSA PRIVATE KEY", "EC PRIVATE KEY"];
    let Some(label) = labels
        .iter()
        .find(|label| trimmed.starts_with(&format!("-----BEGIN {label}-----")))
    else {
        return Err(KeyCertMatchError::KeyParse(
            "private key PEM has an unsupported or missing BEGIN marker".to_string(),
        ));
    };
    let end = format!("-----END {label}-----");
    let Some(end_offset) = trimmed.find(&end) else {
        return Err(KeyCertMatchError::KeyParse("private key PEM is truncated".to_string()));
    };
    if !trimmed[end_offset + end.len()..].trim().is_empty() {
        return Err(KeyCertMatchError::KeyParse(
            "private key PEM contains trailing content".to_string(),
        ));
    }
    let der = PrivateKeyDer::from_pem_slice(trimmed.as_bytes())
        .map_err(|_| KeyCertMatchError::KeyParse("private key PEM is invalid".to_string()))?;
    let format = match &der {
        PrivateKeyDer::Pkcs8(_) => PrivateKeyFormat::Pkcs8,
        PrivateKeyDer::Pkcs1(_) => PrivateKeyFormat::RsaPkcs1,
        PrivateKeyDer::Sec1(_) => PrivateKeyFormat::EcSec1,
        _ => {
            return Err(KeyCertMatchError::KeyParse(
                "private key PEM format is unsupported".to_string(),
            ))
        }
    };
    let signing_key = rustls::crypto::aws_lc_rs::sign::any_supported_type(&der)
        .map_err(|_| KeyCertMatchError::KeyParse("private key PEM is invalid or unsupported".to_string()))?;
    let public_key_spki_der = signing_key
        .public_key()
        .ok_or_else(|| KeyCertMatchError::KeyParse("private key public key is unavailable".to_string()))?
        .as_ref()
        .to_vec();
    Ok(StrictPrivateKey {
        der,
        format,
        public_key_spki_der,
    })
}

// ---------------------------------------------------------------------------
// KeyCertMatchError + check_key_matches_cert (original helpers)
// ---------------------------------------------------------------------------

/// Error returned when a certificate/private-key pair fails to match.
///
/// The `Display` strings intentionally mirror the corresponding
/// `CertValidationError` variants so that existing warning/error messages
/// are unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyCertMatchError {
    /// Certificate PEM parsing failed
    CertParse(String),
    /// Private key PEM parsing failed
    KeyParse(String),
    /// Private key does not match certificate public key
    Mismatch(String),
}

impl fmt::Display for KeyCertMatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CertParse(msg) => write!(f, "Certificate parse error: {}", msg),
            Self::KeyParse(msg) => write!(f, "Private key parse error: {}", msg),
            Self::Mismatch(msg) => write!(f, "Key mismatch: {}", msg),
        }
    }
}

impl std::error::Error for KeyCertMatchError {}

/// Verify that a PEM private key matches the public key in a PEM certificate.
pub fn check_key_matches_cert(cert_pem: &str, key_pem: &str) -> Result<(), KeyCertMatchError> {
    let chain = parse_strict_certificate_chain(cert_pem.as_bytes())
        .map_err(|error| KeyCertMatchError::CertParse(format!("strict certificate chain rejected: {error:?}")))?;
    let private_key = parse_strict_private_key_pem(key_pem.as_bytes())?;
    let (_, cert) = X509Certificate::from_der(&chain.certificates_der[0])
        .map_err(|e| KeyCertMatchError::CertParse(format!("X.509 parse error: {}", e)))?;
    let cert_pubkey_spki_der = cert.public_key().raw;

    if cert_pubkey_spki_der != private_key.public_key_spki_der {
        return Err(KeyCertMatchError::Mismatch(
            "Private key does not match certificate public key".to_string(),
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// CertValidationResult
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// CertValidationError
// ---------------------------------------------------------------------------

/// Certificate validation errors
#[derive(Debug, Clone, PartialEq, Eq)]
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

impl std::error::Error for CertValidationError {}

impl CertValidationError {
    /// Returns a stable, PascalCase discriminant string for this error variant.
    ///
    /// The returned value is identical across all calls for the same variant
    /// regardless of field contents — it never contains wall-clock values or
    /// any data whose ordering is non-deterministic (e.g. HashMap iteration
    /// order). The controller's status semantic-diff compares condition
    /// `message` and `reason` verbatim; a varying value would cause an
    /// infinite status-write loop.
    pub fn status_reason(&self) -> &'static str {
        match self {
            Self::SecretNotFound => "SecretNotFound",
            Self::CertNotFound => "CertNotFound",
            Self::KeyNotFound => "KeyNotFound",
            Self::CertParseError(_) => "CertParseError",
            Self::KeyParseError(_) => "KeyParseError",
            Self::CertExpired { .. } => "CertExpired",
            Self::SanMismatch { .. } => "SanMismatch",
            Self::NoHostsDeclared => "NoHostsDeclared",
            Self::InvalidSanPattern { .. } => "InvalidSanPattern",
            Self::KeyMismatch(_) => "KeyMismatch",
            Self::MtlsCaSecretRefRequired => "MtlsCaSecretRefRequired",
            Self::MtlsCaCertNotFound => "MtlsCaCertNotFound",
            Self::MtlsCaCertParseError(_) => "MtlsCaCertParseError",
            Self::MtlsVerifyDepthOutOfRange(_) => "MtlsVerifyDepthOutOfRange",
            Self::MtlsInvalidSanPattern(_) => "MtlsInvalidSanPattern",
            Self::MtlsInvalidCnPattern(_) => "MtlsInvalidCnPattern",
            Self::ChainCertExpired { .. } => "ChainCertExpired",
            Self::ChainCertError { .. } => "ChainCertError",
        }
    }

    /// Returns a deterministic, human-readable message suitable for writing
    /// into a Kubernetes status condition `message` field.
    ///
    /// # Determinism invariant
    ///
    /// This method MUST NOT include any wall-clock value (e.g.
    /// `CertExpired.current`) or any value whose string representation is
    /// non-deterministic (e.g. HashMap iteration order). The controller's
    /// status semantic-diff compares `message` verbatim across reconcile
    /// cycles; a varying message would trigger a status write on every
    /// reconcile, causing an infinite status-write loop.
    ///
    /// For variants that are already deterministic, this delegates to
    /// `Display`. For `CertExpired`, the `current` field (wall-clock) is
    /// intentionally omitted — only the certificate's own validity window
    /// (`not_before`/`not_after`) is included.
    pub fn status_message(&self) -> String {
        match self {
            Self::CertExpired {
                not_before, not_after, ..
            } => {
                format!(
                    "Certificate expired or not yet valid (valid: {} - {})",
                    not_before, not_after
                )
            }
            // INVARIANT: any Display arm that gains a non-deterministic source
            // (clock, HashMap/HashSet iteration, RNG) MUST also add an explicit
            // arm here that omits it — otherwise status_message silently inherits
            // the non-determinism and reintroduces the status-write hot loop.
            other => other.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// From<KeyCertMatchError> for CertValidationError
// ---------------------------------------------------------------------------

impl From<KeyCertMatchError> for CertValidationError {
    fn from(e: KeyCertMatchError) -> Self {
        match e {
            KeyCertMatchError::CertParse(msg) => CertValidationError::CertParseError(msg),
            KeyCertMatchError::KeyParse(msg) => CertValidationError::KeyParseError(msg),
            KeyCertMatchError::Mismatch(msg) => CertValidationError::KeyMismatch(msg),
        }
    }
}

// ---------------------------------------------------------------------------
// validate_cert
//
// Pure, side-effect-free — no tracing calls. Advisory warnings for deprecated
// TLS version, reserved revocation/OCSP schema, and the no-SAN CN fallback are
// emitted by a caller-side helper in the gateway.
// ---------------------------------------------------------------------------

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

    if tls.spec.secret.is_none() {
        errors.push(CertValidationError::SecretNotFound);
        return CertValidationResult::invalid(errors);
    }

    let Ok(cert_pem) = tls.cert_pem() else {
        errors.push(CertValidationError::CertNotFound);
        return CertValidationResult::invalid(errors);
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
            errors.push(CertValidationError::from(e));
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

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Parse leaf X.509 certificate from PEM string.
/// Returns (leaf_der_data, not_before_unix_ts, not_after_unix_ts)
fn parse_leaf_cert(pem_str: &str) -> Result<(Vec<u8>, i64, i64), String> {
    let pem = Pem::iter_from_buffer(pem_str.as_bytes())
        .next()
        .ok_or_else(|| "No PEM block found".to_string())?
        .map_err(|e| format!("PEM parse error: {}", e))?;

    let der_data = pem.contents.clone();
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

    // CN fallback only when certificate has no SAN extension at all (legacy cert compat).
    // Advisory warning about this deprecated fallback is emitted by a caller-side helper
    // in the gateway.
    if !has_san_extension {
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
            san_ip_list.contains(&declared_ip)
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
    san_list.iter().any(|san| matches(san, declared))
}

/// Whether a declared host is a valid SAN pattern under RFC 6125 §6.4.3 / RFC 5280 §4.2.1.6.
///
/// A host with no `*` is always valid. A wildcard host is valid only when it is exactly
/// `*.<suffix>` where `<suffix>` is non-empty and contains no further `*` — i.e. a single,
/// left-most-label asterisk. Multi-label wildcards (`*.*.example.com`), partial-label
/// wildcards (`foo*.example.com`), and non-leftmost wildcards (`a.*.example.com`) are
/// rejected; these are exactly the patterns the RFC 6125 matcher cannot match.
fn is_valid_san_pattern(host: &str) -> bool {
    if !host.contains('*') {
        return true;
    }
    match host.strip_prefix("*.") {
        Some(suffix) => !suffix.is_empty() && !suffix.contains('*'),
        None => false,
    }
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
            if let Some(err) = edgion_resources::resources::allowed_san::allowed_san_config_error(san) {
                errors.push(CertValidationError::MtlsInvalidSanPattern(err));
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

// Unit tests for this module live in `../tests/unit/tls_validation_test.rs`.
#[cfg(test)]
#[path = "../tests/unit/tls_validation_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/unit/tls_validation_test/status_determinism_tests.rs"]
mod status_determinism_tests;

#[cfg(test)]
#[path = "../tests/unit/tls_validation_test/canonical_ca_bundle_tests.rs"]
mod canonical_ca_bundle_tests;
