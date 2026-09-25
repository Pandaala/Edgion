use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::status::helpers;
use crate::suites::tls_route::basic::basic::make_tls_connector;
use async_trait::async_trait;
use rustls::pki_types::ServerName;
use serde_json::Value;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const PORT: u16 = 31291;

pub struct TlsUnknownKindBackendTestSuite;
const ROUTE_NAMESPACE: &str = "edgion-test";

impl TlsUnknownKindBackendTestSuite {
    fn has_condition_with_reason(status: &Value, condition_type: &str, expected_status: &str, reason: &str) -> bool {
        if let Some(parents) = status["status"]["parents"].as_array() {
            for parent in parents {
                if let Some(conditions) = parent["conditions"].as_array() {
                    for cond in conditions {
                        if cond["type"].as_str() == Some(condition_type)
                            && cond["status"].as_str() == Some(expected_status)
                            && cond["reason"].as_str() == Some(reason)
                        {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    fn test_unknown_kind_only_rejected() -> TestCase {
        TestCase::new(
            "unknown_kind_only_rejected",
            "TLSRoute with only unknown-kind backendRef should reject traffic",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:{}", ctx.target_host, PORT);
                    let connector = make_tls_connector();
                    let sni = ServerName::try_from("unknown-kind.sandbox.example.com").unwrap();

                    let tcp = match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                        Ok(Ok(s)) => s,
                        Ok(Err(e)) => return TestResult::failed(start.elapsed(), format!("TCP connect failed: {e}")),
                        Err(_) => return TestResult::failed(start.elapsed(), "TCP connect timed out".to_string()),
                    };

                    match connector.connect(sni, tcp).await {
                        Ok(mut tls) => {
                            let _ = tls.write_all(b"must-not-route").await;
                            let mut buf = vec![0u8; 512];
                            match tokio::time::timeout(Duration::from_secs(2), tls.read(&mut buf)).await {
                                Ok(Ok(0)) | Ok(Err(_)) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Connection rejected as expected for unknown-kind-only backend".to_string(),
                                ),
                                Ok(Ok(n)) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected rejection but received {n} bytes"),
                                ),
                                Err(_) => {
                                    TestResult::failed(start.elapsed(), "Read timed out unexpectedly".to_string())
                                }
                            }
                        }
                        Err(_) => TestResult::passed_with_message(
                            start.elapsed(),
                            "TLS handshake failed as expected for unknown-kind-only backend".to_string(),
                        ),
                    }
                })
            },
        )
    }

    fn test_mixed_backend_keeps_invalid_share() -> TestCase {
        TestCase::new(
            "mixed_backend_keeps_invalid_share",
            "TLSRoute with equal-weight valid and unknown-kind backendRefs must fail the invalid share",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:{}", ctx.target_host, PORT);
                    let connector = make_tls_connector();
                    let payload = b"mixed-backend-echo";
                    const ATTEMPTS: usize = 20;
                    let mut echoed = 0usize;
                    let mut failed_closed = 0usize;

                    for _ in 0..ATTEMPTS {
                        let sni = ServerName::try_from("mixed-backend.sandbox.example.com").unwrap();
                        let tcp = match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                            Ok(Ok(s)) => s,
                            Ok(Err(e)) => {
                                return TestResult::failed(start.elapsed(), format!("TCP connect failed: {e}"))
                            }
                            Err(_) => return TestResult::failed(start.elapsed(), "TCP connect timed out".to_string()),
                        };

                        let Ok(mut tls) = connector.connect(sni, tcp).await else {
                            failed_closed += 1;
                            continue;
                        };

                        if tls.write_all(payload).await.is_err() {
                            failed_closed += 1;
                            continue;
                        }

                        let mut buf = vec![0u8; 1024];
                        match tokio::time::timeout(Duration::from_secs(2), tls.read(&mut buf)).await {
                            Ok(Ok(n)) if n > 0 => echoed += 1,
                            Ok(Ok(_)) | Ok(Err(_)) | Err(_) => failed_closed += 1,
                        }
                    }

                    if echoed == 0 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "All {ATTEMPTS} mixed-backend attempts failed closed; valid Service share is missing"
                            ),
                        );
                    }
                    if failed_closed == 0 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("All {ATTEMPTS} mixed-backend attempts echoed; unknown-kind weight was dropped"),
                        );
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "Mixed backends kept both shares: echoed={echoed} fail_closed={failed_closed} / {ATTEMPTS}"
                        ),
                    )
                })
            },
        )
    }

    fn test_unknown_kind_status_invalid_kind() -> TestCase {
        TestCase::new(
            "unknown_kind_status_invalid_kind",
            "TLSRoute with unknown backend kind should report ResolvedRefs=False(InvalidKind)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "TLSRoute", ROUTE_NAMESPACE, "tls-unknown-kind-only")
                        .await
                    {
                        Ok(route) => {
                            if !helpers::has_condition(&route, "Accepted", "True") {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected Accepted=True, got status: {}",
                                        serde_json::to_string_pretty(&route["status"]).unwrap_or_default()
                                    ),
                                );
                            }

                            if Self::has_condition_with_reason(&route, "ResolvedRefs", "False", "InvalidKind") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=False(InvalidKind) as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=False(InvalidKind), got status: {}",
                                        serde_json::to_string_pretty(&route["status"]).unwrap_or_default()
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for TlsUnknownKindBackendTestSuite {
    fn name(&self) -> &str {
        "TLSRoute UnknownKindBackend"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let mut cases = vec![Self::test_unknown_kind_status_invalid_kind()];
        if crate::suites::tls_route::tls_route_terminate_tests_enabled() {
            cases.extend([
                Self::test_unknown_kind_only_rejected(),
                Self::test_mixed_backend_keeps_invalid_share(),
            ]);
        } else {
            crate::suites::tls_route::report_terminate_deferred(
                "UnknownKindBackend/unknown_kind_only_rejected,mixed_backend_keeps_invalid_share",
            );
        }
        cases
    }
}
