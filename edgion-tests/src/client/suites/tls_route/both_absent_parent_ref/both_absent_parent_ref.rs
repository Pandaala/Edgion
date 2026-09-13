// TLSRoute BothAbsentParentRef Test Suite
//
// Validates that a TLSRoute and EdgionTls with parentRefs containing ONLY
// name + namespace (no sectionName, no port) correctly attach to all
// listeners of the referenced Gateway.
//
// This exercises the Gateway API spec rule: when neither sectionName nor
// port is set, the resource should attach to ALL listeners of the Gateway.
//
// Required config files (in edgion-tests/integration/conf/TLSRoute/BothAbsentParentRef/):
// - 01_Gateway.yaml        # Gateway with TLS listener on port 31288
// - 02_EdgionTls.yaml      # EdgionTls with parentRef (name+ns only)
// - 03_TLSRoute.yaml       # TLSRoute with parentRef (name+ns only)
//
// Port allocation (from ports.json "TLSRoute/BothAbsentParentRef"):
// - 31288 (tls): TLS terminate → TCP forward

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::tls_route::basic::basic::make_tls_connector;
use async_trait::async_trait;
use rustls::pki_types::ServerName;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct TlsBothAbsentParentRefTestSuite;

const ORIGINAL_GATEWAY: &str = r#"apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: tls-route-both-absent-gw
  namespace: edgion-test
spec:
  gatewayClassName: public-gateway
  listeners:
    - name: tls-both-absent
      protocol: TLS
      port: 31288
      hostname: "*.both-absent.example.com"
      tls:
        mode: Terminate
        certificateRefs:
          - name: edge-tls
            namespace: edgion-test"#;

const RENAMED_GATEWAY: &str = r#"apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: tls-route-both-absent-gw
  namespace: edgion-test
spec:
  gatewayClassName: public-gateway
  listeners:
    - name: tls-both-absent-updated
      protocol: TLS
      port: 31288
      hostname: "*.both-absent.example.com"
      tls:
        mode: Terminate
        certificateRefs:
          - name: edge-tls
            namespace: edgion-test"#;

async fn tls_echo_works(ctx: &TestContext) -> bool {
    let addr = format!("{}:31288", ctx.target_host);
    let connector = make_tls_connector();
    let sni = ServerName::try_from("requeue.both-absent.example.com").unwrap();
    let Ok(Ok(tcp)) = tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(&addr)).await else {
        return false;
    };
    let Ok(mut tls) = connector.connect(sni, tcp).await else {
        return false;
    };
    let payload = b"Hello Requeue";
    if tls.write_all(payload).await.is_err() {
        return false;
    }
    let mut response = [0; 13];
    matches!(
        tokio::time::timeout(Duration::from_secs(2), tls.read_exact(&mut response)).await,
        Ok(Ok(_)) if response == *payload
    )
}

async fn wait_for_tls_echo(ctx: &TestContext) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if tls_echo_works(ctx).await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

impl TlsBothAbsentParentRefTestSuite {
    fn test_tls_connection_both_absent() -> TestCase {
        TestCase::new(
            "tls_both_absent_parentref_connection",
            "TLSRoute with both-absent parentRef should terminate TLS and forward to TCP backend",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:31288", ctx.target_host);
                    let connector = make_tls_connector();
                    let sni = ServerName::try_from("test.both-absent.example.com").unwrap();

                    match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                        Ok(Ok(tcp_stream)) => match connector.connect(sni, tcp_stream).await {
                            Ok(mut tls_stream) => {
                                let test_data = b"Hello BothAbsent";
                                if let Err(e) = tls_stream.write_all(test_data).await {
                                    return TestResult::failed(start.elapsed(), format!("TLS write failed: {}", e));
                                }

                                let mut buf = vec![0u8; 1024];
                                match tokio::time::timeout(Duration::from_secs(3), tls_stream.read(&mut buf)).await {
                                    Ok(Ok(n)) if n > 0 => {
                                        if &buf[..n] == test_data {
                                            TestResult::passed_with_message(
                                                start.elapsed(),
                                                "TLS terminated with both-absent parentRef, echo via TCP succeeded"
                                                    .to_string(),
                                            )
                                        } else {
                                            TestResult::passed_with_message(
                                                start.elapsed(),
                                                format!(
                                                    "TLS connection with both-absent parentRef established, received {} bytes",
                                                    n
                                                ),
                                            )
                                        }
                                    }
                                    Ok(Ok(_)) => TestResult::failed(
                                        start.elapsed(),
                                        "Connection closed immediately (0 bytes) — resolved_listeners may be empty"
                                            .to_string(),
                                    ),
                                    Ok(Err(e)) => {
                                        TestResult::failed(start.elapsed(), format!("TLS read failed: {}", e))
                                    }
                                    Err(_) => TestResult::failed(start.elapsed(), "Read timed out".to_string()),
                                }
                            }
                            Err(e) => TestResult::failed(
                                start.elapsed(),
                                format!(
                                    "TLS handshake failed (cert not resolved?) — both-absent parentRef may not have attached: {}",
                                    e
                                ),
                            ),
                        },
                        Ok(Err(e)) => TestResult::failed(start.elapsed(), format!("TCP connection failed: {}", e)),
                        Err(_) => TestResult::failed(start.elapsed(), "Connection timed out".to_string()),
                    }
                })
            },
        )
    }

    /// Rename the existing listener without changing its physical key, then
    /// verify both Controller and Gateway projections refresh and traffic stays
    /// attached through the broad parentRef.
    fn test_gateway_listener_update_requeue() -> TestCase {
        TestCase::new(
            "tls_both_absent_gateway_listener_update_requeue",
            "Renaming a stable Gateway listener should requeue TLSRoute and preserve TLS routing",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(error) = ctx
                        .wait_for_listener_projection(
                            "TLSRoute",
                            "edgion-test",
                            "tls-route-both-absent",
                            "resolvedListeners",
                            "tls-both-absent",
                            Duration::from_secs(8),
                        )
                        .await
                    {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Initial listener projection is missing: {error}"),
                        );
                    }
                    if !wait_for_tls_echo(&ctx).await {
                        return TestResult::failed(start.elapsed(), "Baseline TLS echo failed".to_string());
                    }

                    if let Err(error) = ctx.apply_yaml(RENAMED_GATEWAY).await {
                        return TestResult::failed(start.elapsed(), format!("Failed to rename listener: {error}"));
                    }
                    let renamed_result = ctx
                        .wait_for_listener_projection(
                            "TLSRoute",
                            "edgion-test",
                            "tls-route-both-absent",
                            "resolvedListeners",
                            "tls-both-absent-updated",
                            Duration::from_secs(8),
                        )
                        .await;
                    let renamed_traffic_ok = renamed_result.is_ok() && wait_for_tls_echo(&ctx).await;

                    if let Err(error) = ctx.apply_yaml(ORIGINAL_GATEWAY).await {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Failed to restore listener name: {error}"),
                        );
                    }
                    if let Err(error) = renamed_result {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Renamed projection did not converge: {error}"),
                        );
                    }
                    if !renamed_traffic_ok {
                        return TestResult::failed(
                            start.elapsed(),
                            "TLS echo failed after the broad parentRef moved to the renamed listener".to_string(),
                        );
                    }
                    if let Err(error) = ctx
                        .wait_for_listener_projection(
                            "TLSRoute",
                            "edgion-test",
                            "tls-route-both-absent",
                            "resolvedListeners",
                            "tls-both-absent",
                            Duration::from_secs(8),
                        )
                        .await
                    {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Restored projection did not converge: {error}"),
                        );
                    }
                    if !wait_for_tls_echo(&ctx).await {
                        return TestResult::failed(
                            start.elapsed(),
                            "TLS echo failed after listener restore".to_string(),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "Stable listener rename requeued TLSRoute in Controller and Gateway; TLS stayed routable"
                            .to_string(),
                    )
                })
            },
        )
        .with_timeout(Duration::from_secs(40))
    }

    fn test_sni_mismatch_both_absent() -> TestCase {
        TestCase::new(
            "tls_both_absent_parentref_sni_mismatch",
            "TLSRoute with both-absent parentRef should still reject non-matching SNI",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:31288", ctx.target_host);
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
impl TestSuite for TlsBothAbsentParentRefTestSuite {
    fn name(&self) -> &str {
        "TLSRoute BothAbsentParentRef"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_tls_connection_both_absent(),
            Self::test_sni_mismatch_both_absent(),
            Self::test_gateway_listener_update_requeue(),
        ]
    }
}
