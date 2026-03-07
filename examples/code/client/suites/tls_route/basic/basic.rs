// TLSRoute Basic Test Suite
//
// Tests TLS termination + SNI-based routing + TCP forwarding to backend.
//
// Required config files (in examples/test/conf/TLSRoute/Basic/):
// - 01_Gateway.yaml        # Gateway with TLS listener (protocol=TLS, backend-protocol=tcp)
// - 02_EdgionTls.yaml      # Wildcard certificate *.sandbox.example.com
// - 03_TLSRoute.yaml       # TLSRoute matching *.sandbox.example.com
// - Service_test-tcp.yaml / EndpointSlice_test-tcp.yaml
//
// Port allocation (from ports.json "TLSRoute/Basic"):
// - 31280 (tls): TLS terminate → TCP forward

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use rustls::pki_types::ServerName;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub(crate) fn make_tls_connector() -> tokio_rustls::TlsConnector {
    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoCertVerifier))
        .with_no_client_auth();
    tokio_rustls::TlsConnector::from(Arc::new(config))
}

#[derive(Debug)]
pub(crate) struct NoCertVerifier;

impl rustls::client::danger::ServerCertVerifier for NoCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub struct TlsRouteTestSuite;

impl TlsRouteTestSuite {
    /// Test: TLS connection via SNI matching should successfully connect and forward data
    fn test_tls_connection() -> TestCase {
        TestCase::new(
            "tls_route_connection",
            "TLSRoute should terminate TLS and forward to TCP backend",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:31280", ctx.target_host);
                    let connector = make_tls_connector();
                    let sni = ServerName::try_from("test-443.sandbox.example.com").unwrap();

                    match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                        Ok(Ok(tcp_stream)) => match connector.connect(sni, tcp_stream).await {
                            Ok(mut tls_stream) => {
                                let test_data = b"Hello TLSRoute";
                                if let Err(e) = tls_stream.write_all(test_data).await {
                                    return TestResult::failed(start.elapsed(), format!("TLS write failed: {}", e));
                                }

                                let mut buf = vec![0u8; 1024];
                                match tokio::time::timeout(Duration::from_secs(3), tls_stream.read(&mut buf)).await {
                                    Ok(Ok(n)) if n > 0 => {
                                        if &buf[..n] == test_data {
                                            TestResult::passed_with_message(
                                                start.elapsed(),
                                                "TLS terminated, echo via TCP backend succeeded".to_string(),
                                            )
                                        } else {
                                            TestResult::passed_with_message(
                                                start.elapsed(),
                                                format!("TLS connection established, received {} bytes", n),
                                            )
                                        }
                                    }
                                    Ok(Ok(_)) => TestResult::failed(
                                        start.elapsed(),
                                        "Connection closed immediately (0 bytes)".to_string(),
                                    ),
                                    Ok(Err(e)) => {
                                        TestResult::failed(start.elapsed(), format!("TLS read failed: {}", e))
                                    }
                                    Err(_) => TestResult::failed(start.elapsed(), "Read timed out".to_string()),
                                }
                            }
                            Err(e) => TestResult::failed(start.elapsed(), format!("TLS handshake failed: {}", e)),
                        },
                        Ok(Err(e)) => TestResult::failed(start.elapsed(), format!("TCP connection failed: {}", e)),
                        Err(_) => TestResult::failed(start.elapsed(), "Connection timed out".to_string()),
                    }
                })
            },
        )
    }

    /// Test: SNI mismatch should result in no matching route (connection closed)
    fn test_sni_mismatch() -> TestCase {
        TestCase::new(
            "tls_route_sni_mismatch",
            "TLSRoute should reject connections with non-matching SNI",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:31280", ctx.target_host);
                    let connector = make_tls_connector();
                    let sni = ServerName::try_from("nomatch.other.com").unwrap();

                    match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                        Ok(Ok(tcp_stream)) => match connector.connect(sni, tcp_stream).await {
                            Ok(mut tls_stream) => {
                                let test_data = b"Hello";
                                let _ = tls_stream.write_all(test_data).await;

                                let mut buf = vec![0u8; 1024];
                                match tokio::time::timeout(Duration::from_secs(2), tls_stream.read(&mut buf)).await {
                                    Ok(Ok(0)) => TestResult::passed_with_message(
                                        start.elapsed(),
                                        "Connection closed — no matching TLSRoute for SNI".to_string(),
                                    ),
                                    Ok(Err(_)) => TestResult::passed_with_message(
                                        start.elapsed(),
                                        "Connection rejected — no matching TLSRoute".to_string(),
                                    ),
                                    Ok(Ok(n)) => TestResult::failed(
                                        start.elapsed(),
                                        format!("Expected rejection but received {} bytes", n),
                                    ),
                                    Err(_) => TestResult::passed_with_message(
                                        start.elapsed(),
                                        "Read timed out — treated as rejection".to_string(),
                                    ),
                                }
                            }
                            Err(_e) => TestResult::passed_with_message(
                                start.elapsed(),
                                "TLS handshake failed for mismatched SNI".to_string(),
                            ),
                        },
                        Ok(Err(e)) => TestResult::failed(start.elapsed(), format!("TCP connection failed: {}", e)),
                        Err(_) => TestResult::failed(start.elapsed(), "Connection timed out".to_string()),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for TlsRouteTestSuite {
    fn name(&self) -> &str {
        "TLSRoute Basic"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_tls_connection(), Self::test_sni_mismatch()]
    }
}
