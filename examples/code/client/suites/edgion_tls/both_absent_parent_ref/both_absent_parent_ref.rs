// EdgionTls BothAbsentParentRef Test Suite
//
// Validates that an EdgionTls resource with parentRefs containing ONLY
// name + namespace (no sectionName, no port) correctly attaches its
// certificate to all listeners of the referenced Gateway.
//
// Required config files (in examples/test/conf/EdgionTls/BothAbsentParentRef/):
// - Gateway.yaml         # HTTPS Gateway listener on port 31290
// - EdgionTls.yaml       # EdgionTls with parentRef (name+ns only)
// - HTTPRoute.yaml       # Route for both-absent-tls.example.com
// - Service.yaml         # Backend service
// - EndpointSlice.yaml   # Backend endpoint
//
// Port allocation (from ports.json "EdgionTls/BothAbsentParentRef"):
// - 31290 (https): HTTPS with EdgionTls certificate

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct EdgionTlsBothAbsentParentRefTestSuite;

impl EdgionTlsBothAbsentParentRefTestSuite {
    fn test_https_with_both_absent_parentref() -> TestCase {
        TestCase::new(
            "edgiontls_both_absent_parentref_https",
            "EdgionTls with both-absent parentRef should serve HTTPS via resolved listener port",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let host = ctx.http_host.as_deref().unwrap_or(&ctx.target_host);
                    let url = format!("https://{}:{}/health", host, ctx.https_port);

                    match ctx.http_client.get(&url).send().await {
                        Ok(response) => {
                            let status = response.status();
                            if status.is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "HTTPS with both-absent parentRef succeeded. Status: {}",
                                        status
                                    ),
                                )
                            } else {
                                match response.text().await {
                                    Ok(body) => TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "HTTPS failed — cert may not be attached. Status: {}, Body: {}",
                                            status, body
                                        ),
                                    ),
                                    Err(e) => TestResult::failed(
                                        start.elapsed(),
                                        format!("Status: {}, Error reading body: {}", status, e),
                                    ),
                                }
                            }
                        }
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "HTTPS request failed — EdgionTls may not have resolved ports with both-absent parentRef: {}",
                                e
                            ),
                        ),
                    }
                })
            },
        )
    }

    /// Test: delete Gateway, re-apply it, verify HTTPS recovers.
    /// Validates that EdgionTls's gateway_route_index registration
    /// allows Gateway changes to trigger EdgionTls requeue.
    fn test_gateway_requeue_cycle() -> TestCase {
        TestCase::new(
            "edgiontls_both_absent_gateway_requeue",
            "Delete and re-apply Gateway should requeue EdgionTls and restore HTTPS",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let host = ctx.http_host.as_deref().unwrap_or(&ctx.target_host);

                    // Phase 1: baseline
                    let baseline_url = format!("https://{}:{}/health", host, ctx.https_port);
                    let baseline_ok = ctx.http_client.get(&baseline_url).send().await.is_ok();
                    if !baseline_ok {
                        return TestResult::failed(
                            start.elapsed(),
                            "Baseline failed — HTTPS not working before Gateway delete".to_string(),
                        );
                    }

                    // Phase 2: delete Gateway
                    if let Err(e) = ctx
                        .delete_resource("Gateway", "edgion-test", "edgiontls-both-absent-gw")
                        .await
                    {
                        return TestResult::failed(start.elapsed(), format!("Failed to delete Gateway: {}", e));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

                    // Phase 3: re-apply Gateway
                    let gateway_yaml = r#"apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: edgiontls-both-absent-gw
  namespace: edgion-test
spec:
  gatewayClassName: public-gateway
  listeners:
    - name: https
      protocol: HTTPS
      port: 31290
      hostname: "both-absent-tls.example.com"
      allowedRoutes:
        namespaces:
          from: All"#;

                    if let Err(e) = ctx.apply_yaml(gateway_yaml).await {
                        return TestResult::failed(start.elapsed(), format!("Failed to re-apply Gateway: {}", e));
                    }

                    // Phase 4: wait for requeue
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

                    // Phase 5: verify HTTPS works again (retry)
                    let url = format!("https://{}:{}/health", host, ctx.https_port);
                    let mut recovered = false;
                    for attempt in 0..5 {
                        if let Ok(resp) = ctx.http_client.get(&url).send().await {
                            if resp.status().is_success() {
                                recovered = true;
                                break;
                            }
                        }
                        if attempt < 4 {
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        }
                    }

                    if recovered {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "Gateway delete + re-apply triggered EdgionTls requeue, HTTPS restored".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            "HTTPS did not recover after Gateway re-apply — EdgionTls requeue may not work with both-absent parentRefs".to_string(),
                        )
                    }
                })
            },
        )
    }

    fn test_https_echo() -> TestCase {
        TestCase::new(
            "edgiontls_both_absent_parentref_echo",
            "EdgionTls with both-absent parentRef should handle HTTPS echo request",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let host = ctx.http_host.as_deref().unwrap_or(&ctx.target_host);
                    let url = format!("https://{}:{}/echo", host, ctx.https_port);

                    match ctx.http_client.get(&url).send().await {
                        Ok(response) => {
                            let status = response.status();
                            if status.is_success() {
                                TestResult::passed_with_message(start.elapsed(), format!("Status: {}", status))
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected success, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("HTTPS request failed: {}", e)),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for EdgionTlsBothAbsentParentRefTestSuite {
    fn name(&self) -> &str {
        "EdgionTls BothAbsentParentRef"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_https_with_both_absent_parentref(),
            Self::test_https_echo(),
            Self::test_gateway_requeue_cycle(),
        ]
    }
}
