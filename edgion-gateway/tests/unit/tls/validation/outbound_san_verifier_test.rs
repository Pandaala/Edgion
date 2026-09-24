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

// -----------------------------------------------------------------------
// Detector tests
// -----------------------------------------------------------------------

fn san_rustls_error(reason: &str) -> RustlsError {
    RustlsError::InvalidCertificate(CertificateError::Other(OtherError(Arc::new(SanMismatchError {
        reason: reason.to_string(),
    }))))
}

/// A foreign error type that only exposes the marker through `Display`.
/// Exercises the step-4 substring fallback.
#[derive(Debug)]
struct OpaqueError(String);

impl std::fmt::Display for OpaqueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for OpaqueError {}

#[test]
fn detector_matches_bare_typed_error() {
    let err = SanMismatchError {
        reason: "declared [idp.example.com], actual [other.example.com]".to_string(),
    };
    assert_eq!(
        outbound_san_mismatch_reason(&err).as_deref(),
        Some("declared [idp.example.com], actual [other.example.com]")
    );
}

#[test]
fn detector_matches_through_rustls_error() {
    let err = san_rustls_error("no match");
    assert_eq!(outbound_san_mismatch_reason(&err).as_deref(), Some("no match"));
}

/// Regression guard for the `io::Error::get_ref()` hop.
///
/// `std::io::Error`'s `Error::source()` returns the *inner* error's source and
/// skips the inner error itself, so a plain `source()` walk never sees the
/// wrapped `rustls::Error`. tokio-rustls wraps handshake failures exactly this
/// way. Deleting the `get_ref()` hop turns only this test red.
#[test]
fn detector_matches_through_io_error_wrapper() {
    let io_err = std::io::Error::new(std::io::ErrorKind::InvalidData, san_rustls_error("wrapped"));
    assert_eq!(outbound_san_mismatch_reason(&io_err).as_deref(), Some("wrapped"));
}

#[test]
fn detector_bounds_nested_io_error_wrappers() {
    let mut err = std::io::Error::new(std::io::ErrorKind::InvalidData, san_rustls_error("too deep"));
    for _ in 0..MAX_CHAIN_DEPTH {
        err = std::io::Error::new(std::io::ErrorKind::InvalidData, err);
    }

    assert_eq!(outbound_san_mismatch_reason(&err), None);
}

#[test]
fn detector_falls_back_to_display_substring() {
    let err = OpaqueError("connect error: SAN mismatch: fallback reason".to_string());
    assert_eq!(outbound_san_mismatch_reason(&err).as_deref(), Some("fallback reason"));
}

#[test]
fn detector_rejects_unrelated_certificate_error() {
    let err = RustlsError::InvalidCertificate(CertificateError::Expired);
    assert_eq!(outbound_san_mismatch_reason(&err), None);
}

#[test]
fn detector_rejects_unrelated_error() {
    let err = OpaqueError("connection refused".to_string());
    assert_eq!(outbound_san_mismatch_reason(&err), None);
}

/// The chain walk is depth-bounded. The marker is planted deeper than
/// `MAX_CHAIN_DEPTH`, so a correct implementation gives up before reaching it.
/// A test with the marker absent would pass even with no bound at all.
#[test]
fn detector_stops_at_max_chain_depth() {
    #[derive(Debug)]
    struct Link {
        msg: String,
        next: Option<Box<Link>>,
    }

    impl std::fmt::Display for Link {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.msg)
        }
    }

    impl std::error::Error for Link {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.next
                .as_ref()
                .map(|b| b.as_ref() as &(dyn std::error::Error + 'static))
        }
    }

    // Deepest link carries the marker; 32 plain links sit above it.
    let mut head = Link {
        msg: "SAN mismatch: too deep to reach".to_string(),
        next: None,
    };
    for _ in 0..32 {
        head = Link {
            msg: "link".to_string(),
            next: Some(Box::new(head)),
        };
    }

    assert_eq!(
        outbound_san_mismatch_reason(&head),
        None,
        "the walk must abandon the chain at MAX_CHAIN_DEPTH"
    );
}
