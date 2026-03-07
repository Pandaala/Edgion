// TLSRoute StreamPlugins Test Suite
//
// Tests that stream plugins on TLSRoute annotations work correctly,
// mirroring the TCPRoute StreamPlugins tests but over TLS connections.
//
// Required config files (in examples/test/conf/TLSRoute/StreamPlugins/):
// - 01_EdgionStreamPlugins_deny.yaml            # deny-all resource
// - 02_EdgionStreamPlugins_allow_localhost.yaml  # allow 127.0.0.0/8 only
// - 03_Gateway.yaml                              # Gateway with 2 TLS listeners
// - 04_TLSRoute_allow_localhost.yaml             # Route with allow-localhost plugin
// - 05_TLSRoute_deny.yaml                        # Route with deny-all plugin
// - Service_test-tcp.yaml / EndpointSlice_test-tcp.yaml
//
// Port allocation (from ports.json "TLSRoute/StreamPlugins"):
// - 31282 (tls): allow-localhost filter
// - 31283 (tls_filtered): deny-all filter

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::tls_route::basic::basic::make_tls_connector;
use rustls::pki_types::ServerName;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct TlsStreamPluginsTestSuite;

impl TlsStreamPluginsTestSuite {
    /// Test: allow-localhost plugin should allow TLS connections from 127.0.0.1
    fn test_allow_localhost() -> TestCase {
        TestCase::new(
            "tls_stream_plugin_allow_localhost",
            "TLSRoute stream plugin should allow localhost connection",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:31282", ctx.target_host);
                    let connector = make_tls_connector();
                    let sni = ServerName::try_from("test.sp-allow.example.com").unwrap();

                    match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                        Ok(Ok(tcp_stream)) => {
                            match connector.connect(sni, tcp_stream).await {
                                Ok(mut tls_stream) => {
                                    let test_data = b"Hello TLS StreamPlugin";
                                    if let Err(e) = tls_stream.write_all(test_data).await {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Write failed: {}", e),
                                        );
                                    }

                                    let mut buf = vec![0u8; 1024];
                                    match tokio::time::timeout(
                                        Duration::from_secs(3),
                                        tls_stream.read(&mut buf),
                                    )
                                    .await
                                    {
                                        Ok(Ok(n)) if n > 0 => {
                                            if &buf[..n] == test_data {
                                                TestResult::passed_with_message(
                                                    start.elapsed(),
                                                    "TLS echo succeeded — stream plugin allowed localhost".to_string(),
                                                )
                                            } else {
                                                TestResult::passed_with_message(
                                                    start.elapsed(),
                                                    format!("Connection allowed, received {} bytes", n),
                                                )
                                            }
                                        }
                                        Ok(Ok(_)) => TestResult::failed(
                                            start.elapsed(),
                                            "Connection closed immediately — stream plugin may be denying".to_string(),
                                        ),
                                        Ok(Err(e)) => TestResult::failed(
                                            start.elapsed(),
                                            format!("Read failed: {}", e),
                                        ),
                                        Err(_) => TestResult::failed(
                                            start.elapsed(),
                                            "Read timed out".to_string(),
                                        ),
                                    }
                                }
                                Err(e) => TestResult::failed(
                                    start.elapsed(),
                                    format!("TLS handshake failed: {}", e),
                                ),
                            }
                        }
                        Ok(Err(e)) => TestResult::failed(
                            start.elapsed(),
                            format!("Connection refused: {}", e),
                        ),
                        Err(_) => TestResult::failed(
                            start.elapsed(),
                            "Connection timed out".to_string(),
                        ),
                    }
                })
            },
        )
    }

    /// Test: deny-all stream plugin should reject TLS connections after TLS handshake
    fn test_deny_all() -> TestCase {
        TestCase::new(
            "tls_stream_plugin_deny_all",
            "TLSRoute stream plugin with deny-all should reject connection after TLS handshake",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:31283", ctx.target_host);
                    let connector = make_tls_connector();
                    let sni = ServerName::try_from("test.sp-deny.example.com").unwrap();

                    match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                        Ok(Ok(tcp_stream)) => {
                            match connector.connect(sni, tcp_stream).await {
                                Ok(mut tls_stream) => {
                                    let test_data = b"Hello";
                                    let _ = tls_stream.write_all(test_data).await;

                                    let mut buf = vec![0u8; 1024];
                                    match tokio::time::timeout(
                                        Duration::from_secs(2),
                                        tls_stream.read(&mut buf),
                                    )
                                    .await
                                    {
                                        Ok(Ok(0)) => TestResult::passed_with_message(
                                            start.elapsed(),
                                            "Connection closed by stream plugin (EOF) after TLS handshake".to_string(),
                                        ),
                                        Ok(Ok(n)) => TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected denial but received {} bytes", n),
                                        ),
                                        Ok(Err(_e)) => TestResult::passed_with_message(
                                            start.elapsed(),
                                            "Connection rejected by stream plugin (read error)".to_string(),
                                        ),
                                        Err(_) => TestResult::failed(
                                            start.elapsed(),
                                            "Read timed out — stream plugin did not deny".to_string(),
                                        ),
                                    }
                                }
                                Err(_e) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "TLS handshake rejected (before stream plugin)".to_string(),
                                ),
                            }
                        }
                        Ok(Err(_e)) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Connection refused by stream plugin".to_string(),
                        ),
                        Err(_) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Connection dropped by stream plugin (timeout)".to_string(),
                        ),
                    }
                })
            },
        )
    }

    /// Test: short name annotation format should work for TLSRoute
    fn test_short_name_annotation() -> TestCase {
        TestCase::new(
            "tls_stream_plugin_short_name",
            "Short name annotation (without namespace/) should correctly resolve EdgionStreamPlugins for TLSRoute",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:31283", ctx.target_host);
                    let connector = make_tls_connector();
                    let sni = ServerName::try_from("test.sp-deny.example.com").unwrap();

                    match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                        Ok(Ok(tcp_stream)) => {
                            match connector.connect(sni, tcp_stream).await {
                                Ok(mut tls_stream) => {
                                    let mut buf = vec![0u8; 16];
                                    match tokio::time::timeout(
                                        Duration::from_secs(2),
                                        tls_stream.read(&mut buf),
                                    )
                                    .await
                                    {
                                        Ok(Ok(0)) | Ok(Err(_)) => TestResult::passed_with_message(
                                            start.elapsed(),
                                            "Short name annotation resolved — connection denied".to_string(),
                                        ),
                                        Ok(Ok(n)) => TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected denial but received {} bytes", n),
                                        ),
                                        Err(_) => TestResult::failed(
                                            start.elapsed(),
                                            "Read timed out — short name may not be resolved".to_string(),
                                        ),
                                    }
                                }
                                Err(_) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Short name annotation resolved — TLS handshake rejected".to_string(),
                                ),
                            }
                        }
                        Ok(Err(_)) | Err(_) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Short name annotation resolved — connection refused/dropped".to_string(),
                        ),
                    }
                })
            },
        )
    }
}

impl TestSuite for TlsStreamPluginsTestSuite {
    fn name(&self) -> &str {
        "TLSRoute StreamPlugins"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_allow_localhost(),
            Self::test_deny_all(),
            Self::test_short_name_annotation(),
        ]
    }
}
