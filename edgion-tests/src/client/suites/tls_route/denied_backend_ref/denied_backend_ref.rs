// TLSRoute DeniedBackendRef suite: cross-namespace Service backendRefs
// without a ReferenceGrant must fail closed in both proxy modes.
//
// Local fixtures use edgion-test -> backend. In Kubernetes, both Routes in
// edgion-default reference Services in edgion-backend:
// - 31956 (tls terminate): handshake with the gateway, then no upstream echo
// - 31957 (tls passthrough): ClientHello is not forwarded to the HTTPS backend
//
// Both backends are real. If the post-select gate is missing, terminate
// echoes and passthrough completes a backend handshake.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::status::helpers;
use crate::suites::tls_route::basic::basic::make_tls_connector;
use async_trait::async_trait;
use rustls::pki_types::ServerName;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const PASSTHROUGH_PORT: u16 = 31957;

pub struct TlsDeniedBackendRefTestSuite;

async fn assert_attached_and_denied(ctx: &TestContext, name: &str) -> Result<(), String> {
    helpers::wait_resource_status(
        ctx,
        "TLSRoute",
        helpers::l4_denied_backend_ref_namespace(),
        name,
        Duration::from_secs(10),
        |route| {
            route["status"]["parents"].as_array().is_some_and(|parents| {
                parents.iter().any(|parent| {
                    let conditions = parent["conditions"].as_array();
                    conditions.is_some_and(|conditions| {
                        conditions
                            .iter()
                            .any(|condition| condition["type"] == "Accepted" && condition["status"] == "True")
                            && conditions.iter().any(|condition| {
                                condition["type"] == "ResolvedRefs"
                                    && condition["status"] == "False"
                                    && condition["reason"] == "RefNotPermitted"
                            })
                    })
                })
            })
        },
    )
    .await
    .map(|_| ())
}

impl TlsDeniedBackendRefTestSuite {
    fn test_terminate_denied_ref_fail_closed() -> TestCase {
        TestCase::new(
            "terminate_denied_cross_ns_ref_fail_closed",
            "TLS terminate cross-ns backendRef without ReferenceGrant must not echo",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(error) = assert_attached_and_denied(&ctx, "tls-denied-ref-terminate").await {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("route attachment precondition failed: {error}"),
                        );
                    }
                    let addr = format!("{}:{}", ctx.target_host, ctx.tcp_port);
                    let connector = make_tls_connector();
                    let sni = ServerName::try_from("denied-terminate.sandbox.example.com").unwrap();
                    let payload = b"must-not-echo";

                    let tcp = match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                        Ok(Ok(s)) => s,
                        Ok(Err(e)) => return TestResult::failed(start.elapsed(), format!("TCP connect failed: {e}")),
                        Err(_) => return TestResult::failed(start.elapsed(), "TCP connect timed out".to_string()),
                    };

                    let mut tls = match connector.connect(sni, tcp).await {
                        Ok(s) => s,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("TLS handshake failed: {e}")),
                    };

                    if tls.write_all(payload).await.is_err() {
                        return TestResult::passed_with_message(
                            start.elapsed(),
                            "Terminate denied backendRef closed during write".to_string(),
                        );
                    }

                    let mut buf = vec![0u8; 64];
                    match tokio::time::timeout(Duration::from_secs(2), tls.read(&mut buf)).await {
                        Ok(Ok(0)) | Ok(Err(_)) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Terminate denied backendRef closed without echoing".to_string(),
                        ),
                        Ok(Ok(n)) => TestResult::failed(
                            start.elapsed(),
                            format!("Expected fail-close but received {n} bytes (gate missing?)"),
                        ),
                        Err(_) => TestResult::failed(
                            start.elapsed(),
                            "Read timed out; expected immediate fail-close".to_string(),
                        ),
                    }
                })
            },
        )
    }

    fn test_passthrough_denied_ref_fail_closed() -> TestCase {
        TestCase::new(
            "passthrough_denied_cross_ns_ref_fail_closed",
            "TLS passthrough cross-ns backendRef without ReferenceGrant must not reach the backend",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(error) = assert_attached_and_denied(&ctx, "tls-denied-ref-passthrough").await {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("route attachment precondition failed: {error}"),
                        );
                    }
                    let addr = format!("{}:{}", ctx.target_host, PASSTHROUGH_PORT);
                    let connector = make_tls_connector();
                    let sni = ServerName::try_from("denied-passthrough.sandbox.example.com").unwrap();

                    let tcp = match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                        Ok(Ok(s)) => s,
                        Ok(Err(e)) => return TestResult::failed(start.elapsed(), format!("TCP connect failed: {e}")),
                        Err(_) => return TestResult::failed(start.elapsed(), "TCP connect timed out".to_string()),
                    };

                    match connector.connect(sni, tcp).await {
                        Err(_) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Passthrough denied backendRef did not complete a backend handshake".to_string(),
                        ),
                        Ok(mut tls) => {
                            let request = b"GET /health HTTP/1.1\r\nHost: denied-passthrough.sandbox.example.com\r\nConnection: close\r\n\r\n";
                            let _ = tls.write_all(request).await;
                            let mut buf = vec![0u8; 512];
                            match tokio::time::timeout(Duration::from_secs(2), tls.read(&mut buf)).await {
                                Ok(Ok(n)) if n > 0 => TestResult::failed(
                                    start.elapsed(),
                                    format!("Passthrough reached a backend and read {n} bytes (gate missing?)"),
                                ),
                                Ok(Ok(_)) | Ok(Err(_)) | Err(_) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Passthrough denied backendRef produced no backend response".to_string(),
                                ),
                            }
                        }
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for TlsDeniedBackendRefTestSuite {
    fn name(&self) -> &str {
        "TLSRoute DeniedBackendRef"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let mut cases = vec![Self::test_passthrough_denied_ref_fail_closed()];
        if crate::suites::tls_route::tls_route_terminate_tests_enabled() {
            cases.push(Self::test_terminate_denied_ref_fail_closed());
        } else {
            crate::suites::tls_route::report_terminate_deferred(
                "DeniedBackendRef/terminate_denied_cross_ns_ref_fail_closed",
            );
        }
        cases
    }
}
