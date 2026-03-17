// TLSRoute BothAbsentParentRef Test Suite
//
// Validates that a TLSRoute and EdgionTls with parentRefs containing ONLY
// name + namespace (no sectionName, no port) correctly attach to all
// listeners of the referenced Gateway.
//
// This exercises the Gateway API spec rule: when neither sectionName nor
// port is set, the resource should attach to ALL listeners of the Gateway.
//
// Required config files (in examples/test/conf/TLSRoute/BothAbsentParentRef/):
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
                                        "Connection closed immediately (0 bytes) — resolved_ports may be empty"
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

    /// Test: delete Gateway, verify TLS fails, re-apply Gateway, verify TLS recovers.
    /// This validates the requeue mechanism: when a Gateway is re-added after
    /// TLSRoute and EdgionTls already exist, they should be requeued and
    /// their resolved_ports re-resolved from the new Gateway's listeners.
    fn test_gateway_requeue_cycle() -> TestCase {
        TestCase::new(
            "tls_both_absent_gateway_requeue",
            "Delete and re-apply Gateway should trigger requeue and restore TLS routing",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Phase 1: baseline — TLS should work
                    let addr = format!("{}:31288", ctx.target_host);
                    let connector = make_tls_connector();
                    let sni = ServerName::try_from("requeue.both-absent.example.com").unwrap();

                    let baseline_ok = {
                        match tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(&addr)).await {
                            Ok(Ok(tcp)) => connector.connect(sni.clone(), tcp).await.is_ok(),
                            _ => false,
                        }
                    };
                    if !baseline_ok {
                        return TestResult::failed(
                            start.elapsed(),
                            "Baseline check failed — TLS not working before Gateway delete".to_string(),
                        );
                    }

                    // Phase 2: delete Gateway
                    if let Err(e) = ctx
                        .delete_resource("Gateway", "edgion-test", "tls-route-both-absent-gw")
                        .await
                    {
                        return TestResult::failed(start.elapsed(), format!("Failed to delete Gateway: {}", e));
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    // Phase 3: re-apply Gateway
                    let gateway_yaml = r#"apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: tls-route-both-absent-gw
  namespace: edgion-test
  annotations:
    edgion.io/backend-protocol: "tcp"
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

                    if let Err(e) = ctx.apply_yaml(gateway_yaml).await {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Failed to re-apply Gateway: {}", e),
                        );
                    }

                    // Phase 4: wait for requeue to propagate
                    tokio::time::sleep(Duration::from_secs(2)).await;

                    // Phase 5: verify TLS works again
                    let mut recovered = false;
                    for attempt in 0..5 {
                        match tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(&addr)).await {
                            Ok(Ok(tcp)) => {
                                if let Ok(mut tls_stream) = connector.connect(sni.clone(), tcp).await {
                                    let test_data = b"Hello Requeue";
                                    if tls_stream.write_all(test_data).await.is_ok() {
                                        let mut buf = vec![0u8; 1024];
                                        if let Ok(Ok(n)) = tokio::time::timeout(
                                            Duration::from_secs(2),
                                            tls_stream.read(&mut buf),
                                        )
                                        .await
                                        {
                                            if n > 0 {
                                                recovered = true;
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                        if attempt < 4 {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    }

                    if recovered {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "Gateway delete + re-apply triggered requeue, TLS restored".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            "TLS did not recover after Gateway re-apply — requeue may not be working for both-absent parentRefs".to_string(),
                        )
                    }
                })
            },
        )
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
            Self::test_gateway_requeue_cycle(),
        ]
    }
}
