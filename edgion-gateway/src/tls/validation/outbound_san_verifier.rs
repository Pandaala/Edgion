//! Outbound SAN-pinning `ServerCertVerifier`.
//!
//! Wraps a base rustls verifier (webpki / CA chain) and, on success, extracts
//! end-entity SANs from the DER certificate and matches them against the
//! operator-configured `policy_sans` (BackendTLSPolicy `validation.subjectAltNames`
//! or the equivalent outbound-TLS field).
//!
//! On no-match the verifier returns a `rustls::Error` so the handshake aborts.
//! The caller (Task 3 / Task 4) maps the rustls error to
//! `EdgionErrorCode::OutboundTlsSanMismatch` in its handshake-error path.
//!
//! # Performance note
//! `verify_server_cert` is called once per TLS handshake.  `x509-parser` is
//! invoked only on the single end-entity certificate, never on intermediates.

use std::sync::Arc;

use edgion_resources::resources::backend_tls_policy::SubjectAltName;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, DigitallySignedStruct, Error as RustlsError, OtherError, SignatureScheme};

use super::backend_san::validate_backend_sans;

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Extract Subject Alternative Names from a DER-encoded certificate.
///
/// Returns a `Vec<String>` whose entries follow the same conventions as the
/// BoringSSL-backed `extract_sans` in `cert_extractor.rs`:
/// - DNS names: bare string, e.g. `"idp.example.com"`
/// - URI names: `"uri:<value>"`, e.g. `"uri:spiffe://cluster.local/ns/x/sa/y"`
/// - IPv4 addresses: `"IP:a.b.c.d"`
/// - IPv6 addresses: `"IP:<compressed-ipv6>"` (RFC-5952 form via `Ipv6Addr`)
///
/// `validate_backend_sans` skips entries that start with `IP:` / `uri:` /
/// `email:` when matching `Hostname` policy SANs, so the prefixes are safe.
fn extract_sans_from_rustls_cert(der: &CertificateDer<'_>) -> Result<Vec<String>, x509_parser::error::X509Error> {
    use x509_parser::prelude::*;

    let (_, cert) = X509Certificate::from_der(der.as_ref())?;
    let mut sans = Vec::new();

    if let Ok(Some(san_ext)) = cert.subject_alternative_name() {
        for gn in &san_ext.value.general_names {
            match gn {
                GeneralName::DNSName(dns) => {
                    sans.push(dns.to_string());
                }
                GeneralName::URI(uri) => {
                    sans.push(format!("uri:{}", uri));
                }
                GeneralName::IPAddress(ip) => match ip.len() {
                    4 => {
                        sans.push(format!("IP:{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]));
                    }
                    16 => {
                        let octets: [u8; 16] = match (*ip).try_into() {
                            Ok(arr) => arr,
                            Err(_) => continue,
                        };
                        sans.push(format!("IP:{}", std::net::Ipv6Addr::from(octets)));
                    }
                    _ => {}
                },
                GeneralName::RFC822Name(email) => {
                    sans.push(format!("email:{}", email));
                }
                _ => {}
            }
        }
    }

    Ok(sans)
}

// ---------------------------------------------------------------------------
// SanMismatchError + detector
// ---------------------------------------------------------------------------

/// Marker prefix carried by both `Display` and `Debug`. Callers outside this
/// module match on the typed error; the prefix exists so the last-resort
/// substring fallback in `outbound_san_mismatch_reason` can still recover the
/// reason when the typed identity is lost crossing a foreign error boundary.
const SAN_MISMATCH_PREFIX: &str = "SAN mismatch: ";

/// Bound on the `source()` walk. Real chains are 3-5 links deep.
const MAX_CHAIN_DEPTH: usize = 16;

/// Typed marker embedded in `rustls::Error::InvalidCertificate(CertificateError::Other(..))`
/// when outbound SAN pinning rejects a peer certificate.
///
/// `Debug` is hand-written, not derived, and renders identically to `Display`.
/// rustls's `CertificateError` `Display` falls through to `write!(f, "{other:?}")`,
/// so a derived `Debug` would emit `SanMismatchError { reason: "..." }` and the
/// `SAN_MISMATCH_PREFIX` marker would never appear in the rendered chain.
pub struct SanMismatchError {
    pub reason: String,
}

impl std::fmt::Display for SanMismatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", SAN_MISMATCH_PREFIX, self.reason)
    }
}

impl std::fmt::Debug for SanMismatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", SAN_MISMATCH_PREFIX, self.reason)
    }
}

impl std::error::Error for SanMismatchError {}

/// Recover the SAN-mismatch reason from an outbound error chain, if present.
///
/// Two full, depth-bounded passes over the chain, not one interleaved pass:
/// typed matches first (`probe_typed`), the `Display`-substring fallback
/// (`probe_fallback`) only if the entire chain fails every typed check.
///
/// This ordering matters. Some error types (e.g. `thiserror`'s
/// `#[error("... {source}")]`) render their whole nested chain into one
/// `Display` string, and `rustls::CertificateError::Other` has no dedicated
/// `Display` arm and falls back to `{other:?}` (`Other(OtherError(SAN
/// mismatch: ...))`, `Debug`, with wrapper parens). A single interleaved
/// pass would try the substring fallback on an *outer* node first, match the
/// marker inside that recursively-rendered text, and return a reason
/// contaminated with the inner nodes' wrapper syntax -- before the walk ever
/// reaches the deeper node where the clean typed downcast would have hit.
/// Trying every node's typed check before any node's fallback check
/// guarantees a clean match always wins over a contaminated one.
///
/// Returns an owned `String` because the fallback arm recovers the reason from a
/// `Display` rendering, which is a temporary nothing can borrow from. This is a
/// cold error path.
pub fn outbound_san_mismatch_reason(err: &(dyn std::error::Error + 'static)) -> Option<String> {
    match walk_typed_chain(err, MAX_CHAIN_DEPTH) {
        Ok(Some(reason)) => Some(reason),
        Ok(None) => walk_chain(err, probe_fallback),
        Err(DepthLimitReached) => None,
    }
}

struct DepthLimitReached;

/// Typed walk that includes `io::Error::get_ref()` without resetting the
/// caller's depth budget. `io::Error::source()` skips the wrapped error itself,
/// so its inner chain replaces the ordinary source hop instead of adding a
/// second traversal path.
fn walk_typed_chain(
    err: &(dyn std::error::Error + 'static),
    remaining: usize,
) -> Result<Option<String>, DepthLimitReached> {
    if remaining == 0 {
        return Err(DepthLimitReached);
    }

    if let Some(reason) = probe_typed(err) {
        return Ok(Some(reason));
    }

    if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
        return match io_err.get_ref() {
            Some(inner) => walk_typed_chain(inner, remaining - 1),
            None => Ok(None),
        };
    }

    match err.source() {
        Some(source) => walk_typed_chain(source, remaining - 1),
        None => Ok(None),
    }
}

/// Depth-bounded `source()` walk, applying `probe` at each node until it matches.
fn walk_chain(
    err: &(dyn std::error::Error + 'static),
    probe: fn(&(dyn std::error::Error + 'static)) -> Option<String>,
) -> Option<String> {
    let mut node: Option<&(dyn std::error::Error + 'static)> = Some(err);
    let mut depth = 0usize;

    while let Some(current) = node {
        if depth >= MAX_CHAIN_DEPTH {
            return None;
        }
        depth += 1;

        if let Some(reason) = probe(current) {
            return Some(reason);
        }
        node = current.source();
    }

    None
}

/// Typed-only checks. Never falls back to a `Display` rendering -- see
/// `outbound_san_mismatch_reason` for why that must stay a separate, later pass.
fn probe_typed(node: &(dyn std::error::Error + 'static)) -> Option<String> {
    // 1. The typed error itself.
    if let Some(san) = node.downcast_ref::<SanMismatchError>() {
        return Some(san.reason.clone());
    }

    // 2. Still inside the rustls error that the verifier returned.
    if let Some(RustlsError::InvalidCertificate(CertificateError::Other(other))) = node.downcast_ref::<RustlsError>() {
        if let Some(san) = other.0.downcast_ref::<SanMismatchError>() {
            return Some(san.reason.clone());
        }
    }

    None
}

/// Last resort: the typed identity was lost crossing a foreign boundary,
/// but the marker survives in the rendered text. Intentionally traverses only
/// `source()` and NOT `io::Error::get_ref()` children (unlike `probe_typed`), as
/// this fallback is unreachable for real SAN rejections (typed pass always matches).
/// Keeping `source()`-only avoids reintroducing reason-string contamination.
fn probe_fallback(node: &(dyn std::error::Error + 'static)) -> Option<String> {
    let rendered = node.to_string();
    rendered
        .find(SAN_MISMATCH_PREFIX)
        .map(|idx| rendered[idx + SAN_MISMATCH_PREFIX.len()..].to_string())
}

// ---------------------------------------------------------------------------
// OutboundSanVerifier
// ---------------------------------------------------------------------------

/// A rustls `ServerCertVerifier` that adds SAN-pinning on top of a base verifier.
///
/// Construct via [`OutboundSanVerifier::new`], wrapping a real webpki / CA-chain
/// verifier together with the operator-supplied `policy_sans`.  When
/// `policy_sans` is empty the verifier is functionally identical to the inner
/// verifier (no extra check).
#[derive(Debug)]
pub struct OutboundSanVerifier {
    inner: Arc<dyn ServerCertVerifier>,
    policy_sans: Vec<SubjectAltName>,
}

impl OutboundSanVerifier {
    /// Create a new `OutboundSanVerifier`.
    ///
    /// - `inner`: the base verifier that performs CA-chain / expiry /
    ///   hostname-SAN checks (e.g. a `WebPkiServerVerifier`).
    /// - `policy_sans`: operator-configured `subjectAltNames`; if empty, the
    ///   extra SAN-pinning step is skipped.
    pub fn new(inner: Arc<dyn ServerCertVerifier>, policy_sans: Vec<SubjectAltName>) -> Self {
        Self { inner, policy_sans }
    }
}

impl ServerCertVerifier for OutboundSanVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        // Step 1: delegate to inner (CA chain, expiry, hostname SAN).
        let verified = self
            .inner
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)?;

        // Step 2: skip extra check if no policy SANs configured.
        if self.policy_sans.is_empty() {
            return Ok(verified);
        }

        // Step 3: extract SANs from the end-entity DER cert.
        let cert_sans = extract_sans_from_rustls_cert(end_entity)
            .map_err(|e| RustlsError::InvalidCertificate(CertificateError::Other(OtherError(Arc::new(e)))))?;

        // Step 4: validate cert SANs against the operator policy.
        let result = validate_backend_sans(&cert_sans, &self.policy_sans);
        if !result.passed {
            return Err(RustlsError::InvalidCertificate(CertificateError::Other(OtherError(
                Arc::new(SanMismatchError {
                    reason: result.reason.to_string(),
                }),
            ))));
        }

        Ok(verified)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "../../../tests/unit/tls/validation/outbound_san_verifier_test.rs"]
mod tests;
