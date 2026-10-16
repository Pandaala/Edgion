//! Scoped negative TLS probes for the mTLS integration suites.
//!
//! TLS 1.2 makes client authentication terminal before connect returns. The
//! ordinary positive HTTP cases continue to exercise normal version negotiation.

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::WebPkiClientVerifier;
use rustls::sign::CertifiedKey;
use rustls::{AlertDescription, CertificateError, ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use std::fmt;
use std::future::Future;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use x509_parser::prelude::parse_x509_certificate;

pub(crate) const VALID_CLIENT: (&str, &str) = (
    "edgion-tests/certs/mtls/valid-client.crt",
    "edgion-tests/certs/mtls/valid-client.key",
);
pub(crate) const INVALID_CLIENT: (&str, &str) = (
    "edgion-tests/certs/mtls/invalid-client.crt",
    "edgion-tests/certs/mtls/invalid-client.key",
);
pub(crate) const NONMATCHING_CLIENT: (&str, &str) = (
    "edgion-tests/certs/mtls/nonmatching-client.crt",
    "edgion-tests/certs/mtls/nonmatching-client.key",
);
pub(crate) const MTLS_SERVER: &str = "edgion-tests/certs/mtls/mtls-server.crt";
pub(crate) const FRONTEND_SERVER: &str = "edgion-tests/certs/mtls/fv-server.crt";
pub(crate) const EDGE_SERVER: &str = "edgion-tests/certs/server.crt";
const CLIENT_CA: &str = "edgion-tests/certs/mtls/client-ca.crt";
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TlsOutcome {
    Complete,
    Alert {
        description: AlertDescription,
        server_verified: bool,
    },
    // Only a pre-TLS listener gate may use this, with exact invalid conditions.
    PeerClosed,
}

impl TlsOutcome {
    pub(crate) fn require_alert(self, expected: AlertDescription) -> Result<(), String> {
        if matches!(self, Self::Alert { description, .. } if description == expected) {
            Ok(())
        } else {
            Err(format!("Expected peer TLS alert {expected:?}, observed {self:?}"))
        }
    }

    pub(crate) fn require_authenticated_alert(self, expected: AlertDescription) -> Result<(), String> {
        self.require_alert(expected)?;
        if matches!(
            self,
            Self::Alert {
                server_verified: true,
                ..
            }
        ) {
            Ok(())
        } else {
            Err("Peer alert arrived before the expected server certificate was verified".into())
        }
    }

    pub(crate) fn require_complete(self) -> Result<(), String> {
        if self == Self::Complete {
            Ok(())
        } else {
            Err(format!("Positive TLS control did not complete: {self:?}"))
        }
    }

    pub(crate) fn require_peer_close(self) -> Result<(), String> {
        if self == Self::PeerClosed {
            Ok(())
        } else {
            Err(format!("Expected pre-TLS listener closure, observed {self:?}"))
        }
    }
}

fn read_fixture(path: &Path, kind: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|_| {
        format!(
            "Failed to read {kind} fixture at {}; prepare the test certificates",
            path.display()
        )
    })
}

fn parse_certificates(pem: &[u8]) -> Result<Vec<CertificateDer<'static>>, String> {
    let chain = CertificateDer::pem_slice_iter(pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Certificate fixture contains invalid PEM".to_string())?;
    if chain.is_empty() {
        return Err("Certificate fixture contains no certificate".to_string());
    }
    for certificate in &chain {
        let (remaining, parsed) = parse_x509_certificate(certificate.as_ref())
            .map_err(|_| "Certificate fixture contains invalid DER".to_string())?;
        if !remaining.is_empty() || !parsed.validity().is_valid() {
            return Err("Certificate fixture is malformed or outside its validity period".to_string());
        }
    }
    Ok(chain)
}

fn certificates(path: &Path) -> Result<Vec<CertificateDer<'static>>, String> {
    parse_certificates(&read_fixture(path, "certificate")?)
}

fn load_identity(
    cert_path: &Path,
    key_path: &Path,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), String> {
    let chain = certificates(cert_path)?;
    let key_pem = read_fixture(key_path, "private key")?;
    let key = PrivateKeyDer::from_pem_slice(&key_pem)
        .map_err(|_| "Private-key fixture contains invalid PEM or no supported key".to_string())?;
    let provider = rustls::crypto::ring::default_provider();
    let checked = CertifiedKey::from_der(chain.clone(), key.clone_key(), &provider)
        .map_err(|_| "Client identity has an invalid key or mismatched certificate/key".to_string())?;
    checked
        .keys_match()
        .map_err(|_| "Client certificate/private-key consistency could not be established".to_string())?;
    Ok((chain, key))
}

/// Validate the identity's chain independently before attributing a wire alert
/// to trust policy. UnknownIssuer is the sole acceptable untrusted precondition.
pub(crate) fn require_client_trust(cert_path: &str, trusted: bool) -> Result<(), String> {
    let chain = certificates(Path::new(cert_path))?;
    let mut roots = RootCertStore::empty();
    for ca in certificates(Path::new(CLIENT_CA))? {
        roots
            .add(ca)
            .map_err(|_| "Client CA fixture is not a usable trust anchor".to_string())?;
    }
    let verifier = WebPkiClientVerifier::builder_with_provider(
        Arc::new(roots),
        Arc::new(rustls::crypto::ring::default_provider()),
    )
    .build()
    .map_err(|_| "Failed to prepare client-certificate trust verifier".to_string())?;
    require_trust_result(
        verifier.verify_client_cert(&chain[0], &chain[1..], UnixTime::now()),
        trusted,
    )
}

fn require_trust_result(
    result: Result<rustls::server::danger::ClientCertVerified, rustls::Error>,
    trusted: bool,
) -> Result<(), String> {
    match (result, trusted) {
        (Ok(_), true) | (Err(rustls::Error::InvalidCertificate(CertificateError::UnknownIssuer)), false) => Ok(()),
        _ => Err("Client fixture did not satisfy its expected trust precondition".to_string()),
    }
}

pub(crate) fn require_dns_san(cert_path: &str, expected: &str, present: bool) -> Result<(), String> {
    let chain = certificates(Path::new(cert_path))?;
    let (_, certificate) =
        parse_x509_certificate(chain[0].as_ref()).map_err(|_| "Client certificate DER is invalid".to_string())?;
    let sans = certificate
        .subject_alternative_name()
        .map_err(|_| "Client certificate has malformed SAN extensions".to_string())?
        .ok_or_else(|| "Client certificate fixture has no SAN extension".to_string())?;
    let matches = sans
        .value
        .general_names
        .iter()
        .any(|name| matches!(name, x509_parser::extensions::GeneralName::DNSName(value) if *value == expected));
    if matches == present {
        Ok(())
    } else {
        Err("Client fixture did not satisfy its expected DNS SAN precondition".to_string())
    }
}

struct PinnedServer {
    certificate: CertificateDer<'static>,
    provider: Arc<CryptoProvider>,
    verified: AtomicBool,
    certificate_checked: AtomicBool,
}

impl fmt::Debug for PinnedServer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PinnedTestServer")
    }
}

impl ServerCertVerifier for PinnedServer {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if end_entity.as_ref() == self.certificate.as_ref() {
            self.certificate_checked.store(true, Ordering::Relaxed);
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let verified = verify_tls12_signature(
            message,
            certificate,
            signature,
            &self.provider.signature_verification_algorithms,
        )?;
        // Certificate bytes are public. Only a valid handshake signature after
        // the pin check demonstrates possession of the expected server key.
        self.verified.store(true, Ordering::Relaxed);
        Ok(verified)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let verified = verify_tls13_signature(
            message,
            certificate,
            signature,
            &self.provider.signature_verification_algorithms,
        )?;
        self.verified.store(true, Ordering::Relaxed);
        Ok(verified)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

pub(crate) struct TlsProbe {
    connector: TlsConnector,
    verifier: Arc<PinnedServer>,
    server_name: ServerName<'static>,
}

impl TlsProbe {
    pub(crate) fn new(hostname: &str, server_cert: &str, identity: Option<(&str, &str)>) -> Result<Self, String> {
        let server_name = ServerName::try_from(hostname.to_owned()).map_err(|_| "Probe SNI is invalid".to_string())?;
        let pin = certificates(Path::new(server_cert))?.remove(0);
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = Arc::new(PinnedServer {
            certificate: pin,
            provider: provider.clone(),
            verified: AtomicBool::new(false),
            certificate_checked: AtomicBool::new(false),
        });
        let builder = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS12])
            .map_err(|_| "Failed to configure TLS 1.2 probe".to_string())?
            .dangerous()
            .with_custom_certificate_verifier(verifier.clone());
        let mut config = match identity {
            Some((cert, key)) => {
                let (chain, key) = load_identity(Path::new(cert), Path::new(key))?;
                builder
                    .with_client_auth_cert(chain, key)
                    .map_err(|_| "Failed to configure probe client identity".to_string())?
            }
            None => builder.with_no_client_auth(),
        };
        config.resumption = rustls::client::Resumption::disabled();
        Ok(Self {
            connector: TlsConnector::from(Arc::new(config)),
            verifier,
            server_name,
        })
    }

    pub(crate) async fn run(self, host: &str, port: u16) -> Result<TlsOutcome, String> {
        // The future owns TCP and TLS. Timeout/cancellation drops the socket;
        // no probe-owned child/task survives. An already-running system DNS
        // lookup may outlive the caller because Tokio uses spawn_blocking.
        bounded_operation(
            async {
                let socket = TcpStream::connect((host, port))
                    .await
                    .map_err(|_| "TLS probe target resolution or TCP connect failed".to_string())?;
                match self.connector.connect(self.server_name.clone(), socket).await {
                    Ok(stream) => {
                        drop(stream);
                        Ok(TlsOutcome::Complete)
                    }
                    Err(error) => classify_tls_error(
                        &error,
                        self.verifier.verified.load(Ordering::Relaxed),
                        self.verifier.certificate_checked.load(Ordering::Relaxed),
                    ),
                }
            },
            PROBE_TIMEOUT,
        )
        .await
    }
}

async fn bounded_operation<F: Future<Output = Result<TlsOutcome, String>>>(
    operation: F,
    timeout: Duration,
) -> Result<TlsOutcome, String> {
    tokio::time::timeout(timeout, operation)
        .await
        .map_err(|_| "TLS probe operation deadline expired".to_string())?
}

fn classify_tls_error(
    error: &io::Error,
    server_verified: bool,
    certificate_checked: bool,
) -> Result<TlsOutcome, String> {
    if let Some(rustls::Error::AlertReceived(alert)) =
        error.get_ref().and_then(|inner| inner.downcast_ref::<rustls::Error>())
    {
        return Ok(TlsOutcome::Alert {
            description: *alert,
            server_verified,
        });
    }
    match error.kind() {
        io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset if !certificate_checked => {
            Ok(TlsOutcome::PeerClosed)
        }
        _ => Err("TLS probe failed without a supported peer rejection witness".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{generate_simple_self_signed, CertifiedKey as GeneratedKey};

    fn identity(dir: &Path, stem: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let GeneratedKey { cert, signing_key } =
            generate_simple_self_signed(vec!["client.example.com".into()]).unwrap();
        let cert_path = dir.join(format!("{stem}.crt"));
        let key_path = dir.join(format!("{stem}.key"));
        std::fs::write(&cert_path, cert.pem()).unwrap();
        std::fs::write(&key_path, signing_key.serialize_pem()).unwrap();
        (cert_path, key_path)
    }

    #[test]
    fn identity_preparation_rejects_missing_malformed_and_mismatched_material() {
        let dir = tempfile::tempdir().unwrap();
        let (cert, key) = identity(dir.path(), "first");
        let (_, other_key) = identity(dir.path(), "second");
        assert!(load_identity(&cert, &key).is_ok());
        assert!(load_identity(&dir.path().join("missing.crt"), &key).is_err());
        assert!(load_identity(&cert, &dir.path().join("missing.key")).is_err());
        assert!(load_identity(&cert, &other_key).is_err());
        std::fs::write(&key, "invalid private key").unwrap();
        assert!(load_identity(&cert, &key).is_err());
        for pem in [
            "",
            "not PEM",
            "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n",
            "-----BEGIN CERTIFICATE-----\n!\n-----END CERTIFICATE-----\n",
        ] {
            assert!(parse_certificates(pem.as_bytes()).is_err());
        }
    }

    #[test]
    fn typed_alerts_are_distinct_from_local_failures_and_peer_close() {
        let expected_alerts = [
            AlertDescription::HandshakeFailure,
            AlertDescription::UnknownCA,
            AlertDescription::InternalError,
        ];
        for observed in [
            AlertDescription::HandshakeFailure,
            AlertDescription::UnknownCA,
            AlertDescription::InternalError,
            AlertDescription::CertificateUnknown,
            AlertDescription::ProtocolVersion,
        ] {
            let error = io::Error::new(io::ErrorKind::InvalidData, rustls::Error::AlertReceived(observed));
            let outcome = classify_tls_error(&error, true, true).unwrap();
            for expected in expected_alerts {
                assert_eq!(outcome.require_alert(expected).is_ok(), observed == expected);
                assert_eq!(
                    outcome.require_authenticated_alert(expected).is_ok(),
                    observed == expected
                );
                for certificate_checked in [false, true] {
                    let early = classify_tls_error(&error, false, certificate_checked).unwrap();
                    assert!(early.require_authenticated_alert(expected).is_err());
                    assert_eq!(early.require_alert(expected).is_ok(), observed == expected);
                }
            }
            assert!(outcome.require_complete().is_err());
            assert!(outcome.require_peer_close().is_err());
        }
        for kind in [
            io::ErrorKind::ConnectionRefused,
            io::ErrorKind::TimedOut,
            io::ErrorKind::NotFound,
            io::ErrorKind::InvalidData,
        ] {
            assert!(classify_tls_error(&io::Error::new(kind, "unrelated failure"), false, false).is_err());
        }
        for kind in [io::ErrorKind::UnexpectedEof, io::ErrorKind::ConnectionReset] {
            let outcome = classify_tls_error(&io::Error::new(kind, "peer close"), false, false).unwrap();
            assert!(outcome.require_peer_close().is_ok());
            assert!(classify_tls_error(&io::Error::new(kind, "close after certificate"), false, true).is_err());
            assert!(outcome.require_alert(AlertDescription::HandshakeFailure).is_err());
        }
        let local = io::Error::new(
            io::ErrorKind::InvalidData,
            rustls::Error::InvalidCertificate(CertificateError::UnknownIssuer),
        );
        assert!(classify_tls_error(&local, false, false).is_err());
        assert!(TlsOutcome::Complete
            .require_alert(AlertDescription::HandshakeFailure)
            .is_err());
        assert!(TlsOutcome::Complete.require_peer_close().is_err());
    }

    #[tokio::test]
    async fn operation_deadline_is_a_failed_gate() {
        assert!(bounded_operation(std::future::pending(), Duration::from_millis(1))
            .await
            .unwrap_err()
            .contains("deadline"));
        assert!(
            bounded_operation(async { Err("TCP connect failed".into()) }, PROBE_TIMEOUT)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn deadline_and_caller_cancellation_drop_owned_operation_state() {
        struct DropWitness(Arc<AtomicBool>);
        impl Drop for DropWitness {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let witness = DropWitness(dropped.clone());
        let operation = async move {
            let _witness = witness;
            std::future::pending::<Result<TlsOutcome, String>>().await
        };
        assert!(bounded_operation(operation, Duration::from_millis(1)).await.is_err());
        assert!(dropped.load(Ordering::Relaxed));

        dropped.store(false, Ordering::Relaxed);
        let witness = DropWitness(dropped.clone());
        let operation = async move {
            let _witness = witness;
            std::future::pending::<Result<TlsOutcome, String>>().await
        };
        let mut bounded = Box::pin(bounded_operation(operation, PROBE_TIMEOUT));
        assert!(futures::poll!(bounded.as_mut()).is_pending());
        assert!(!dropped.load(Ordering::Relaxed));
        drop(bounded);
        assert!(dropped.load(Ordering::Relaxed));
    }

    #[test]
    fn only_unknown_issuer_establishes_the_untrusted_fixture_precondition() {
        for error in [
            CertificateError::UnknownIssuer,
            CertificateError::BadEncoding,
            CertificateError::ApplicationVerificationFailure,
        ] {
            let unknown = error == CertificateError::UnknownIssuer;
            assert_eq!(
                require_trust_result(Err(rustls::Error::InvalidCertificate(error)), false).is_ok(),
                unknown
            );
        }
        assert!(require_trust_result(Ok(rustls::server::danger::ClientCertVerified::assertion()), true).is_ok());
        assert!(require_trust_result(Ok(rustls::server::danger::ClientCertVerified::assertion()), false).is_err());
    }
    #[test]
    fn pinned_server_and_dns_san_preconditions_reject_wrong_fixtures() {
        let dir = tempfile::tempdir().unwrap();
        let (cert, _) = identity(dir.path(), "first");
        let (other, _) = identity(dir.path(), "second");
        let chain = certificates(&cert).unwrap();
        let different = certificates(&other).unwrap();
        let verifier = PinnedServer {
            certificate: chain[0].clone(),
            provider: Arc::new(rustls::crypto::ring::default_provider()),
            verified: AtomicBool::new(false),
            certificate_checked: AtomicBool::new(false),
        };
        let sni = ServerName::try_from("client.example.com").unwrap();
        assert!(verifier
            .verify_server_cert(&different[0], &[], &sni, &[], UnixTime::now())
            .is_err());
        assert!(!verifier.verified.load(Ordering::Relaxed));
        assert!(verifier
            .verify_server_cert(&chain[0], &[], &sni, &[], UnixTime::now())
            .is_ok());
        // A copied public certificate alone is not server authentication.
        assert!(verifier.certificate_checked.load(Ordering::Relaxed));
        assert!(!verifier.verified.load(Ordering::Relaxed));
        assert!(require_dns_san(cert.to_str().unwrap(), "client.example.com", true).is_ok());
        assert!(require_dns_san(cert.to_str().unwrap(), "client.example.com", false).is_err());
        assert!(require_dns_san(cert.to_str().unwrap(), "other.example.com", false).is_ok());
        assert!(require_dns_san(cert.to_str().unwrap(), "other.example.com", true).is_err());
    }

    #[test]
    fn client_certificate_validity_is_a_precondition() {
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(vec!["expired.example.com".into()]).unwrap();
        params.not_before = rcgen::date_time_ymd(2000, 1, 1);
        params.not_after = rcgen::date_time_ymd(2001, 1, 1);
        let certificate = params.self_signed(&key).unwrap();
        assert!(parse_certificates(certificate.pem().as_bytes()).is_err());
    }

    #[test]
    fn fixture_policies_preserve_trust_and_san_isolation_and_no_ca_terminate() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        for tree in ["integration", "k8s"] {
            let base = root.join(tree).join("conf/EdgionTls/mTLS");
            let read = |name: &str| -> serde_yaml::Value {
                serde_yaml::from_str(&std::fs::read_to_string(base.join(name)).unwrap()).unwrap()
            };
            let mutual = read("EdgionTls_edge_mtls-test-mutual.yaml");
            let san = read(if tree == "k8s" {
                "DynamicTest/initial/EdgionTls.yaml"
            } else {
                "EdgionTls_edge_mtls-test-san.yaml"
            });
            for policy in [&mutual, &san] {
                assert_eq!(policy["spec"]["clientAuth"]["mode"].as_str(), Some("Mutual"));
                assert_eq!(
                    policy["spec"]["clientAuth"]["caSecretRef"]["name"].as_str(),
                    Some("client-ca")
                );
                assert_eq!(policy["spec"]["secretRef"]["name"].as_str(), Some("mtls-server"));
            }
            assert_eq!(mutual["spec"]["parentRefs"], san["spec"]["parentRefs"]);
            assert!(mutual["spec"]["clientAuth"].get("allowedSans").is_none());
            assert!(mutual["spec"]["clientAuth"].get("allowedCns").is_none());
            assert_eq!(san["spec"]["hosts"][0].as_str(), Some("mtls-san.example.com"));
            let allowed = san["spec"]["clientAuth"]["allowedSans"].as_sequence().unwrap();
            assert_eq!(allowed.len(), 1);
            assert_eq!(allowed[0]["type"].as_str(), Some("DNS"));
            assert_eq!(allowed[0]["value"].as_str(), Some("client1.example.com"));
            assert!(san["spec"]["clientAuth"].get("allowedCns").is_none());
            let terminate = read("EdgionTls_edge_mtls-test-terminate.yaml");
            assert_eq!(terminate["spec"]["clientAuth"]["mode"].as_str(), Some("Terminate"));
            assert!(terminate["spec"]["clientAuth"].get("caSecretRef").is_none());
        }
    }
}
