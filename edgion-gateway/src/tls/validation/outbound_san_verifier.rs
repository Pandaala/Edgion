//! Outbound SAN-pinning `ServerCertVerifier`.
//!
//! Wraps a base rustls verifier (webpki / CA chain) and, on success, extracts
//! end-entity SANs from the DER certificate and matches them against the
//! operator-configured `policy_sans` (BackendTLSPolicy `validation.subjectAltNames`
//! or the equivalent outbound-TLS field).
//!
//! On no-match the verifier returns a `rustls::Error` so the handshake aborts.
//! The caller (Task 3 / Task 4) maps the rustls error to
//! `EdgionStatus::OutboundTlsSanMismatch` in its handshake-error path.
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
                Arc::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("SAN mismatch: {}", result.reason),
                )),
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
mod tests {
    use super::*;
    use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};
    use rcgen::{CertificateParams, KeyPair, SanType};

    // -----------------------------------------------------------------------
    // Test stub: inner verifier that always returns Ok.
    // -----------------------------------------------------------------------

    struct AlwaysOkVerifier;

    impl std::fmt::Debug for AlwaysOkVerifier {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("AlwaysOkVerifier")
        }
    }

    impl ServerCertVerifier for AlwaysOkVerifier {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, RustlsError> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, RustlsError> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, RustlsError> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            vec![]
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_cert_der(dns_sans: &[&str]) -> CertificateDer<'static> {
        let san_types: Vec<SanType> = dns_sans
            .iter()
            .map(|s| SanType::DnsName((*s).try_into().expect("valid DNS SAN")))
            .collect();
        let key_pair = KeyPair::generate().expect("rcgen key generation failed");
        let mut params = CertificateParams::new(vec![]).expect("rcgen params failed");
        params.subject_alt_names = san_types;
        let cert = params.self_signed(&key_pair).expect("rcgen self-sign failed");
        CertificateDer::from(cert.der().to_vec())
    }

    fn hostname_san(h: &str) -> SubjectAltName {
        SubjectAltName {
            san_type: SubjectAltNameType::Hostname,
            hostname: Some(h.to_string()),
            uri: None,
        }
    }

    fn make_server_name() -> ServerName<'static> {
        ServerName::try_from("idp.example.com").expect("valid server name")
    }

    fn make_now() -> UnixTime {
        UnixTime::now()
    }

    // -----------------------------------------------------------------------
    // Test (a): inner Ok + matching SAN → Ok
    // -----------------------------------------------------------------------

    #[test]
    fn inner_ok_matching_san_returns_ok() {
        let cert_der = make_cert_der(&["idp.example.com"]);
        let verifier = OutboundSanVerifier::new(Arc::new(AlwaysOkVerifier), vec![hostname_san("idp.example.com")]);

        let result = verifier.verify_server_cert(&cert_der, &[], &make_server_name(), &[], make_now());

        assert!(result.is_ok(), "expected Ok, got: {:?}", result);
    }

    // -----------------------------------------------------------------------
    // Test (b): inner Ok + non-matching SAN → Err containing "SAN mismatch"
    // -----------------------------------------------------------------------

    #[test]
    fn inner_ok_non_matching_san_returns_err() {
        let cert_der = make_cert_der(&["idp.example.com"]);
        let verifier = OutboundSanVerifier::new(Arc::new(AlwaysOkVerifier), vec![hostname_san("other.example.com")]);

        let result = verifier.verify_server_cert(&cert_der, &[], &make_server_name(), &[], make_now());

        assert!(result.is_err(), "expected Err, got Ok");
        let err_str = format!("{:?}", result.unwrap_err());
        assert!(
            err_str.contains("SAN mismatch"),
            "error should contain 'SAN mismatch', got: {}",
            err_str
        );
    }

    // -----------------------------------------------------------------------
    // Test (c): empty policy_sans → delegates to inner only (no SAN constraint)
    // -----------------------------------------------------------------------

    #[test]
    fn empty_policy_delegates_to_inner_only() {
        let cert_der = make_cert_der(&["idp.example.com"]);
        let verifier = OutboundSanVerifier::new(Arc::new(AlwaysOkVerifier), vec![]);

        let result = verifier.verify_server_cert(&cert_der, &[], &make_server_name(), &[], make_now());

        assert!(result.is_ok(), "empty policy_sans should pass, got: {:?}", result);
    }
}
