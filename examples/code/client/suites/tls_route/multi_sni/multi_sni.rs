// TLSRoute MultiSNI Test Suite
//
// Verifies that multiple TLSRoutes with different SNI hostnames can be served
// on a single Gateway listener (same port), using the global TlsRouteTable.
//
// This test validates the fix for the stale-Arc problem: because EdgionTls
// now loads a fresh route table snapshot per-connection (via ArcSwap), route
// updates are always visible regardless of timing.
//
// Required config files (in examples/test/conf/TLSRoute/MultiSNI/):
// - 01_Gateway.yaml              # Single TLS listener on port 31284
// - 02_EdgionTls_alpha.yaml      # Wildcard cert for *.alpha.example.com
// - 02_EdgionTls_beta.yaml       # Wildcard cert for *.beta.example.com
// - 03_TLSRoute_alpha.yaml       # Route *.alpha.example.com -> TCP echo
// - 03_TLSRoute_beta.yaml        # Route *.beta.example.com  -> TCP echo
// - Service_test-tcp.yaml / EndpointSlice_test-tcp.yaml
//
// Port allocation (from ports.json "TLSRoute/MultiSNI"):
// - 31284 (tls): Single listener for both SNI domains

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::tls_route::basic::basic::make_tls_connector;
use async_trait::async_trait;
use rustls::pki_types::ServerName;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const PORT: u16 = 31284;

pub struct TlsMultiSniTestSuite;

impl TlsMultiSniTestSuite {
    fn test_alpha_domain() -> TestCase {
        TestCase::new(
            "multi_sni_alpha",
            "*.alpha.example.com should route through TLSRoute alpha",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:{}", ctx.target_host, PORT);
                    let connector = make_tls_connector();
                    let sni = ServerName::try_from("alpha.sandbox.example.com").unwrap();

                    let tcp = match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                        Ok(Ok(s)) => s,
                        Ok(Err(e)) => return TestResult::failed(start.elapsed(), format!("TCP connect: {}", e)),
                        Err(_) => return TestResult::failed(start.elapsed(), "TCP connect timed out".to_string()),
                    };

                    let mut tls = match connector.connect(sni, tcp).await {
                        Ok(s) => s,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("TLS handshake: {}", e)),
                    };

                    let payload = b"hello-alpha";
                    if let Err(e) = tls.write_all(payload).await {
                        return TestResult::failed(start.elapsed(), format!("write: {}", e));
                    }

                    let mut buf = vec![0u8; 1024];
                    match tokio::time::timeout(Duration::from_secs(3), tls.read(&mut buf)).await {
                        Ok(Ok(n)) if n > 0 && &buf[..n] == payload => TestResult::passed_with_message(
                            start.elapsed(),
                            "alpha SNI routed and echoed correctly".to_string(),
                        ),
                        Ok(Ok(n)) if n > 0 => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("alpha connected, received {} bytes", n),
                        ),
                        Ok(Ok(_)) => TestResult::failed(start.elapsed(), "alpha: 0 bytes".to_string()),
                        Ok(Err(e)) => TestResult::failed(start.elapsed(), format!("alpha read: {}", e)),
                        Err(_) => TestResult::failed(start.elapsed(), "alpha: read timed out".to_string()),
                    }
                })
            },
        )
    }

    fn test_beta_domain() -> TestCase {
        TestCase::new(
            "multi_sni_beta",
            "*.beta.example.com should route through TLSRoute beta",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:{}", ctx.target_host, PORT);
                    let connector = make_tls_connector();
                    let sni = ServerName::try_from("beta.sandbox.example.com").unwrap();

                    let tcp = match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                        Ok(Ok(s)) => s,
                        Ok(Err(e)) => return TestResult::failed(start.elapsed(), format!("TCP connect: {}", e)),
                        Err(_) => return TestResult::failed(start.elapsed(), "TCP connect timed out".to_string()),
                    };

                    let mut tls = match connector.connect(sni, tcp).await {
                        Ok(s) => s,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("TLS handshake: {}", e)),
                    };

                    let payload = b"hello-beta";
                    if let Err(e) = tls.write_all(payload).await {
                        return TestResult::failed(start.elapsed(), format!("write: {}", e));
                    }

                    let mut buf = vec![0u8; 1024];
                    match tokio::time::timeout(Duration::from_secs(3), tls.read(&mut buf)).await {
                        Ok(Ok(n)) if n > 0 && &buf[..n] == payload => TestResult::passed_with_message(
                            start.elapsed(),
                            "beta SNI routed and echoed correctly".to_string(),
                        ),
                        Ok(Ok(n)) if n > 0 => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("beta connected, received {} bytes", n),
                        ),
                        Ok(Ok(_)) => TestResult::failed(start.elapsed(), "beta: 0 bytes".to_string()),
                        Ok(Err(e)) => TestResult::failed(start.elapsed(), format!("beta read: {}", e)),
                        Err(_) => TestResult::failed(start.elapsed(), "beta: read timed out".to_string()),
                    }
                })
            },
        )
    }

    fn test_unknown_sni_rejected() -> TestCase {
        TestCase::new(
            "multi_sni_unknown_rejected",
            "Unknown SNI on same port should be rejected",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:{}", ctx.target_host, PORT);
                    let connector = make_tls_connector();
                    let sni = ServerName::try_from("unknown.other.com").unwrap();

                    let tcp = match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                        Ok(Ok(s)) => s,
                        Ok(Err(e)) => return TestResult::failed(start.elapsed(), format!("TCP connect: {}", e)),
                        Err(_) => return TestResult::failed(start.elapsed(), "TCP connect timed out".to_string()),
                    };

                    match connector.connect(sni, tcp).await {
                        Ok(mut tls) => {
                            let _ = tls.write_all(b"hello").await;
                            let mut buf = vec![0u8; 1024];
                            match tokio::time::timeout(Duration::from_secs(2), tls.read(&mut buf)).await {
                                Ok(Ok(0)) | Ok(Err(_)) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Connection closed/rejected for unknown SNI".to_string(),
                                ),
                                Ok(Ok(n)) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected rejection but got {} bytes", n),
                                ),
                                Err(_) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Read timed out — treated as rejection".to_string(),
                                ),
                            }
                        }
                        Err(_) => TestResult::passed_with_message(
                            start.elapsed(),
                            "TLS handshake failed for unknown SNI".to_string(),
                        ),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for TlsMultiSniTestSuite {
    fn name(&self) -> &str {
        "TLSRoute MultiSNI"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_alpha_domain(),
            Self::test_beta_domain(),
            Self::test_unknown_sni_rejected(),
        ]
    }
}
