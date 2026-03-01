use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Instant;

pub struct HealthCheckTransitionTestSuite;

const TEST_HOST: &str = "hc-transition.example.com";
const TEST_NAMESPACE: &str = "edgion-default";
const TEST_SERVICE: &str = "hc-transition";
const HC_ANNOTATION_KEY: &str = "edgion.io/health-check";

impl HealthCheckTransitionTestSuite {
    fn unhealthy_hc_annotation() -> String {
        r#"active:
  type: http
  path: /status/503
  interval: 1s
  timeout: 1s
  healthyThreshold: 1
  unhealthyThreshold: 1
  expectedStatuses:
    - 200
"#
        .to_string()
    }

    async fn send_status(ctx: &TestContext) -> Result<u16, String> {
        let url = format!("{}/health", ctx.http_url());
        let resp = ctx
            .http_client
            .get(&url)
            .header("Host", TEST_HOST)
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;
        Ok(resp.status().as_u16())
    }

    async fn wait_for_status(
        ctx: &TestContext,
        expect: u16,
        retries: usize,
        delay_ms: u64,
    ) -> Result<Vec<u16>, String> {
        let mut observed = Vec::new();
        for _ in 0..retries {
            let status = Self::send_status(ctx).await?;
            observed.push(status);
            if status == expect {
                return Ok(observed);
            }
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        }
        Ok(observed)
    }

    async fn update_service_health_check(ctx: &TestContext, annotation_value: &str) -> Result<(), String> {
        let admin_url = format!(
            "{}/api/v1/namespaced/Service/{}/{}",
            ctx.admin_api_url(),
            TEST_NAMESPACE,
            TEST_SERVICE
        );

        let current: Value = ctx
            .http_client
            .get(&admin_url)
            .send()
            .await
            .map_err(|e| format!("Failed to fetch Service: {}", e))?
            .error_for_status()
            .map_err(|e| format!("Service fetch status error: {}", e))?
            .json()
            .await
            .map_err(|e| format!("Failed to decode Service JSON: {}", e))?;

        let mut updated = current;
        if updated.get("metadata").is_none() {
            updated["metadata"] = json!({});
        }
        if updated["metadata"].get("annotations").is_none() {
            updated["metadata"]["annotations"] = json!({});
        }
        updated["metadata"]["annotations"][HC_ANNOTATION_KEY] = Value::String(annotation_value.to_string());

        ctx.http_client
            .put(&admin_url)
            .header("Content-Type", "application/json")
            .body(serde_json::to_string(&updated).map_err(|e| format!("Failed to encode updated Service JSON: {}", e))?)
            .send()
            .await
            .map_err(|e| format!("Failed to update Service: {}", e))?
            .error_for_status()
            .map_err(|e| format!("Service update status error: {}", e))?;

        Ok(())
    }

    fn test_health_check_transitions_to_unhealthy() -> TestCase {
        TestCase::new(
            "health_check_transitions_to_unhealthy",
            "Backend starts healthy, then Service annotation update makes HC fail and route returns 503",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Step 1: verify initial healthy state
                    let initial = match Self::wait_for_status(&ctx, 200, 10, 500).await {
                        Ok(statuses) => statuses,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    if !initial.contains(&200) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected initial 200 before transition, observed={:?}", initial),
                        );
                    }

                    // Step 2: update Service annotation to failing HC path
                    if let Err(e) = Self::update_service_health_check(&ctx, &Self::unhealthy_hc_annotation()).await {
                        return TestResult::failed(start.elapsed(), e);
                    }

                    // Step 3: wait until backend is marked unhealthy and selection returns 503
                    let transitioned = match Self::wait_for_status(&ctx, 503, 24, 500).await {
                        Ok(statuses) => statuses,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    if transitioned.last() == Some(&503) || transitioned.contains(&503) {
                        return TestResult::passed_with_message(
                            start.elapsed(),
                            format!(
                                "Transition observed: initial={:?}, after_update={:?}",
                                initial, transitioned
                            ),
                        );
                    }

                    TestResult::failed(
                        start.elapsed(),
                        format!("Expected transition to 503, observed after update={:?}", transitioned),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for HealthCheckTransitionTestSuite {
    fn name(&self) -> &str {
        "HealthCheckTransition"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_health_check_transitions_to_unhealthy()]
    }
}
