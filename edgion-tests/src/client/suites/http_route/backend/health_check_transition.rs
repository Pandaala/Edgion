use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use serde_json::Value;
use std::time::Instant;

pub struct HealthCheckTransitionTestSuite;

const TEST_HOST: &str = "hc-transition.example.com";
const TEST_NAMESPACE: &str = "edgion-default";
// The active health check lives on an EdgionBackendTrafficPolicy targeting the
// hc-transition Service. The transition is driven by editing the policy's probe
// path (/health -> /status/503), not the Service.
const POLICY_KIND: &str = "EdgionBackendTrafficPolicy";
const POLICY_NAME: &str = "hc-transition-policy";
const HEALTHY_PATH: &str = "/health";
const UNHEALTHY_PATH: &str = "/status/503";

impl HealthCheckTransitionTestSuite {
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

    /// Fetch the EdgionBackendTrafficPolicy, rewrite its active-health-check
    /// probe path, and PUT it back. Switching the path between a healthy
    /// (200) and an always-503 endpoint drives the backend up/down without
    /// touching the Service.
    async fn update_policy_health_check(ctx: &TestContext, probe_path: &str) -> Result<(), String> {
        let admin_url = format!(
            "{}/api/v1/namespaced/{}/{}/{}",
            ctx.admin_api_url(),
            POLICY_KIND,
            TEST_NAMESPACE,
            POLICY_NAME
        );

        let mut get_req = ctx.http_client.get(&admin_url);
        if let Some(ref token) = ctx.admin_token {
            get_req = get_req.header("Authorization", format!("Bearer {}", token));
        }
        let current: Value = get_req
            .send()
            .await
            .map_err(|e| format!("Failed to fetch policy: {}", e))?
            .error_for_status()
            .map_err(|e| format!("Policy fetch status error: {}", e))?
            .json()
            .await
            .map_err(|e| format!("Failed to decode policy JSON: {}", e))?;

        let mut updated = current;
        let active = updated
            .get_mut("spec")
            .and_then(|s| s.get_mut("healthCheck"))
            .and_then(|h| h.get_mut("active"))
            .ok_or_else(|| "policy is missing spec.healthCheck.active".to_string())?;
        active["path"] = Value::String(probe_path.to_string());

        let mut put_req = ctx
            .http_client
            .put(&admin_url)
            .header("Content-Type", "application/json")
            // Status updates may advance the Kubernetes resourceVersion after
            // this suite's GET. This transition intentionally requests the
            // Admin API's last-writer-wins update contract; the policy must
            // still exist, but its status is not an operator precondition.
            .header("If-Match", "*")
            .body(serde_json::to_string(&updated).map_err(|e| format!("Failed to encode updated policy JSON: {}", e))?);
        if let Some(ref token) = ctx.admin_token {
            put_req = put_req.header("Authorization", format!("Bearer {}", token));
        }
        put_req
            .send()
            .await
            .map_err(|e| format!("Failed to update policy: {}", e))?
            .error_for_status()
            .map_err(|e| format!("Policy update status error: {}", e))?;

        Ok(())
    }

    fn test_health_check_transitions_to_unhealthy() -> TestCase {
        TestCase::new(
            "health_check_transitions_to_unhealthy",
            "Backend starts healthy, then a policy healthCheck edit makes HC fail and route returns 503",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Previous runs may have intentionally switched this policy
                    // to an unhealthy HC path. Reset first so skip-prepare reruns
                    // start from the same state as a fresh prepare.
                    if let Err(e) = Self::update_policy_health_check(&ctx, HEALTHY_PATH).await {
                        return TestResult::failed(start.elapsed(), e);
                    }

                    // Step 1: verify initial healthy state
                    let initial = match Self::wait_for_status(&ctx, 200, 24, 500).await {
                        Ok(statuses) => statuses,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    if !initial.contains(&200) {
                        let _ = Self::update_policy_health_check(&ctx, HEALTHY_PATH).await;
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected initial 200 before transition, observed={:?}", initial),
                        );
                    }

                    // Step 2: update the policy's health-check probe to a failing path
                    if let Err(e) = Self::update_policy_health_check(&ctx, UNHEALTHY_PATH).await {
                        return TestResult::failed(start.elapsed(), e);
                    }

                    // Step 3: wait until backend is marked unhealthy and selection returns 503
                    let transitioned = match Self::wait_for_status(&ctx, 503, 24, 500).await {
                        Ok(statuses) => statuses,
                        Err(e) => {
                            let _ = Self::update_policy_health_check(&ctx, HEALTHY_PATH).await;
                            return TestResult::failed(start.elapsed(), e);
                        }
                    };
                    if transitioned.last() == Some(&503) || transitioned.contains(&503) {
                        let _ = Self::update_policy_health_check(&ctx, HEALTHY_PATH).await;
                        return TestResult::passed_with_message(
                            start.elapsed(),
                            format!(
                                "Transition observed: initial={:?}, after_update={:?}",
                                initial, transitioned
                            ),
                        );
                    }

                    let _ = Self::update_policy_health_check(&ctx, HEALTHY_PATH).await;
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

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/HealthCheckTransition")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_health_check_transitions_to_unhealthy()]
    }
}
