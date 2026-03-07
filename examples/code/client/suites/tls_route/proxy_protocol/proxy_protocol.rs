// TLSRoute Proxy Protocol v2 Test Suite
//
// Tests that PP2 header with AUTHORITY TLV (SNI hostname) is correctly
// sent to the backend when edgion.io/proxy-protocol: "v2" is set.
//
// The backend is a PP2-aware TCP server (test_server --tcp-pp2-port 30012)
// that parses the PP2 header using the `proxy-header` crate and returns
// structured JSON with parsed fields for assertion.
//
// Required config files (in examples/test/conf/TLSRoute/ProxyProtocol/):
// - 01_Gateway.yaml             # Gateway with tls-pp2 listener on port 31281
// - 02_EdgionTls.yaml           # Wildcard certificate for *.pp2.example.com
// - 03_TLSRoute_pp2.yaml        # TLSRoute with proxy-protocol: v2 → test-tcp-pp2:30012
// - Service_test-tcp-pp2.yaml   # PP2-aware backend service
// - EndpointSlice_test-tcp-pp2.yaml
//
// The negative test (test_no_pp2_without_annotation) uses Basic suite's
// Gateway on port 31280, so TLSRoute/Basic config must also be loaded.
//
// Port allocation (from ports.json):
// - 31281 (tls_pp2): PP2-enabled TLS route (from "TLSRoute/ProxyProtocol")
// - 31280 (tls_basic): non-PP2 route (from "TLSRoute/Basic")

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::tls_route::basic::basic::make_tls_connector;
use async_trait::async_trait;
use rustls::pki_types::ServerName;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct TlsProxyProtocolTestSuite;

impl TlsProxyProtocolTestSuite {
    /// Test: PP2 header with AUTHORITY TLV is sent to backend and correctly parsed.
    fn test_pp2_header_parsed() -> TestCase {
        TestCase::new(
            "tls_route_pp2_header_parsed",
            "Backend should receive and parse PP2 header with correct AUTHORITY",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:31281", ctx.target_host);
                    let connector = make_tls_connector();
                    let sni_hostname = "test-443.pp2.example.com";
                    let sni = ServerName::try_from(sni_hostname).unwrap();

                    let tcp_stream = match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                        Ok(Ok(s)) => s,
                        Ok(Err(e)) => {
                            return TestResult::failed(start.elapsed(), format!("TCP connect failed: {}", e));
                        }
                        Err(_) => {
                            return TestResult::failed(start.elapsed(), "TCP connect timed out".to_string());
                        }
                    };

                    let mut tls_stream = match connector.connect(sni, tcp_stream).await {
                        Ok(s) => s,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("TLS handshake failed: {}", e));
                        }
                    };

                    // Send a small marker so the gateway flushes PP2 + app data to backend
                    if let Err(e) = tls_stream.write_all(b"PP2-CHECK").await {
                        return TestResult::failed(start.elapsed(), format!("Write failed: {}", e));
                    }

                    // Read the PP2-aware server response
                    let mut buf = vec![0u8; 4096];
                    let n = match tokio::time::timeout(Duration::from_secs(5), tls_stream.read(&mut buf)).await {
                        Ok(Ok(n)) if n > 0 => n,
                        Ok(Ok(_)) => {
                            return TestResult::failed(start.elapsed(), "Connection closed (0 bytes)".to_string());
                        }
                        Ok(Err(e)) => {
                            return TestResult::failed(start.elapsed(), format!("Read failed: {}", e));
                        }
                        Err(_) => {
                            return TestResult::failed(start.elapsed(), "Read timed out".to_string());
                        }
                    };

                    let response_str = String::from_utf8_lossy(&buf[..n]);

                    // The PP2 server returns JSON; find the JSON object boundary
                    let json_str = if let Some(end) = response_str.find('}') {
                        &response_str[..=end]
                    } else {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("No JSON in response: {}", response_str),
                        );
                    };

                    let parsed: serde_json::Value = match serde_json::from_str(json_str) {
                        Ok(v) => v,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Failed to parse JSON response: {} (raw: {})", e, json_str),
                            );
                        }
                    };

                    // Verify PP2 was detected
                    if parsed.get("pp2") != Some(&serde_json::Value::Bool(true)) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("PP2 not detected by backend: {}", json_str),
                        );
                    }

                    // Verify AUTHORITY TLV contains the SNI hostname
                    let authority = parsed.get("authority").and_then(|v| v.as_str()).unwrap_or("");
                    if authority != sni_hostname {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "AUTHORITY TLV mismatch: expected '{}', got '{}'",
                                sni_hostname, authority
                            ),
                        );
                    }

                    // Verify source address is present
                    let src_addr = parsed.get("src_addr").and_then(|v| v.as_str()).unwrap_or("");
                    if src_addr.is_empty() || src_addr == "local" {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("PP2 source address missing or local: {}", json_str),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "PP2 parsed: authority={}, src={}, dst={}, header_len={}",
                            authority,
                            src_addr,
                            parsed.get("dst_addr").and_then(|v| v.as_str()).unwrap_or("?"),
                            parsed.get("pp2_header_len").and_then(|v| v.as_u64()).unwrap_or(0),
                        ),
                    )
                })
            },
        )
    }

    /// Test: Non-PP2 route on same gateway should NOT send PP2 header.
    /// Uses the basic TLSRoute (port 31280) which has no PP2 annotation.
    fn test_no_pp2_without_annotation() -> TestCase {
        TestCase::new(
            "tls_route_no_pp2_without_annotation",
            "TLSRoute without PP2 annotation should not send PP2 header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:31280", ctx.target_host);
                    let connector = make_tls_connector();
                    let sni = ServerName::try_from("test-443.sandbox.example.com").unwrap();

                    let tcp_stream = match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                        Ok(Ok(s)) => s,
                        Ok(Err(e)) => {
                            return TestResult::failed(start.elapsed(), format!("TCP connect failed: {}", e));
                        }
                        Err(_) => {
                            return TestResult::failed(start.elapsed(), "TCP connect timed out".to_string());
                        }
                    };

                    let mut tls_stream = match connector.connect(sni, tcp_stream).await {
                        Ok(s) => s,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("TLS handshake failed: {}", e));
                        }
                    };

                    let test_data = b"NO-PP2-TEST";
                    if let Err(e) = tls_stream.write_all(test_data).await {
                        return TestResult::failed(start.elapsed(), format!("Write failed: {}", e));
                    }

                    let mut buf = vec![0u8; 4096];
                    match tokio::time::timeout(Duration::from_secs(3), tls_stream.read(&mut buf)).await {
                        Ok(Ok(n)) if n > 0 => {
                            // Basic echo server should return exactly what we sent (no PP2 prefix)
                            if &buf[..n] == test_data {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "No PP2 header — plain echo matches sent data".to_string(),
                                )
                            } else {
                                let pp2_sig: &[u8] = &[0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A];
                                if n >= 12 && buf[..12] == *pp2_sig {
                                    TestResult::failed(
                                        start.elapsed(),
                                        "PP2 header detected on non-PP2 route!".to_string(),
                                    )
                                } else {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!("Echo response received ({} bytes), no PP2 header", n),
                                    )
                                }
                            }
                        }
                        Ok(Ok(_)) => TestResult::failed(start.elapsed(), "Connection closed (0 bytes)".to_string()),
                        Ok(Err(e)) => TestResult::failed(start.elapsed(), format!("Read failed: {}", e)),
                        Err(_) => TestResult::failed(start.elapsed(), "Read timed out".to_string()),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for TlsProxyProtocolTestSuite {
    fn name(&self) -> &str {
        "TLSRoute Proxy Protocol v2"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_pp2_header_parsed(),
            Self::test_no_pp2_without_annotation(),
        ]
    }
}
