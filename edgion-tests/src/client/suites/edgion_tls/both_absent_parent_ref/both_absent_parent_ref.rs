// Validates that an EdgionTls resource with parentRefs containing ONLY
// name + namespace (no sectionName, no port) correctly attaches its
// certificate to all listeners of the referenced Gateway.
//
// Required config files (in edgion-tests/integration/conf/EdgionTls/BothAbsentParentRef/):
// - Gateway.yaml         # HTTPS Gateway listener on port 31289
// - EdgionTls.yaml       # EdgionTls with parentRef (name+ns only)
// - HTTPRoute.yaml       # Route for both-absent-tls.example.com
// - Service.yaml         # Backend service
// - EndpointSlice.yaml   # Backend endpoint
//
// Port allocation (from ports.json "EdgionTls/BothAbsentParentRef"):
// - 31289 (https): HTTPS with EdgionTls certificate

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::{Duration, Instant};

pub struct EdgionTlsBothAbsentParentRefTestSuite;

const ORIGINAL_GATEWAY: &str = r#"apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: edgiontls-both-absent-gw
  namespace: edgion-test
spec:
  gatewayClassName: public-gateway
  listeners:
    - name: https
      protocol: HTTPS
      port: 31289
      hostname: "both-absent-tls.example.com"
      allowedRoutes:
        namespaces:
          from: All"#;

const RENAMED_GATEWAY: &str = r#"apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: edgiontls-both-absent-gw
  namespace: edgion-test
spec:
  gatewayClassName: public-gateway
  listeners:
    - name: https-updated
      protocol: HTTPS
      port: 31289
      hostname: "both-absent-tls.example.com"
      allowedRoutes:
        namespaces:
          from: All"#;

async fn https_works(ctx: &TestContext) -> bool {
    let host = ctx.http_host.as_deref().unwrap_or(&ctx.target_host);
    let url = format!("https://{}:{}/health", host, ctx.https_port);
    ctx.http_client
        .get(url)
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

async fn wait_for_https(ctx: &TestContext) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if https_works(ctx).await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

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

    /// Rename the existing listener without changing its physical key, then
    /// verify both Controller and Gateway projections refresh and HTTPS stays
    /// attached through the broad parentRef.
    fn test_gateway_listener_update_requeue() -> TestCase {
        TestCase::new(
            "edgiontls_both_absent_gateway_listener_update_requeue",
            "Renaming a stable Gateway listener should requeue EdgionTls and preserve HTTPS",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(error) = ctx
                        .wait_for_listener_projection(
                            "EdgionTls",
                            "edgion-test",
                            "edgiontls-both-absent-cert",
                            "resolvedListenerAttachments",
                            "https",
                            Duration::from_secs(8),
                        )
                        .await
                    {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Initial listener projection is missing: {error}"),
                        );
                    }
                    if !wait_for_https(&ctx).await {
                        return TestResult::failed(start.elapsed(), "Baseline HTTPS request failed".to_string());
                    }

                    if let Err(error) = ctx.apply_yaml(RENAMED_GATEWAY).await {
                        return TestResult::failed(start.elapsed(), format!("Failed to rename listener: {error}"));
                    }
                    let renamed_result = ctx
                        .wait_for_listener_projection(
                            "EdgionTls",
                            "edgion-test",
                            "edgiontls-both-absent-cert",
                            "resolvedListenerAttachments",
                            "https-updated",
                            Duration::from_secs(8),
                        )
                        .await;
                    let renamed_traffic_ok = renamed_result.is_ok() && wait_for_https(&ctx).await;

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
                            "HTTPS failed after the broad parentRef moved to the renamed listener".to_string(),
                        );
                    }
                    if let Err(error) = ctx
                        .wait_for_listener_projection(
                            "EdgionTls",
                            "edgion-test",
                            "edgiontls-both-absent-cert",
                            "resolvedListenerAttachments",
                            "https",
                            Duration::from_secs(8),
                        )
                        .await
                    {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Restored projection did not converge: {error}"),
                        );
                    }
                    if !wait_for_https(&ctx).await {
                        return TestResult::failed(start.elapsed(), "HTTPS failed after listener restore".to_string());
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "Stable listener rename requeued EdgionTls in Controller and Gateway; HTTPS stayed routable"
                            .to_string(),
                    )
                })
            },
        )
        .with_timeout(Duration::from_secs(40))
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
            Self::test_gateway_listener_update_requeue(),
        ]
    }
}
