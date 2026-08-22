use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::edgion_plugins::hmac_test_utils::build_hmac_headers;
use std::time::Instant;

const HOST: &str = "gac-hmac-dynamic.example.com";
const USERNAME: &str = "dynamic-policy-user";
const INITIAL_SECRET: &[u8] = b"dynamic-policy-initial-secret-at-least-32-bytes";
const ROTATED_SECRET: &[u8] = b"dynamic-policy-rotated-secret-at-least-32-bytes";

async fn status(ctx: &TestContext, secret: &[u8]) -> reqwest::Result<u16> {
    let signed = build_hmac_headers(USERNAME, secret, "GET", "/health", None, HOST, None);
    ctx.http_client
        .get(format!("http://{}:31180/health", ctx.target_host))
        .header("host", HOST)
        .header("date", signed.date)
        .header("authorization", signed.authorization)
        .send()
        .await
        .map(|response| response.status().as_u16())
}

async fn assert_statuses(ctx: &TestContext, initial: u16, rotated: u16) -> TestResult {
    let start = Instant::now();
    match (status(ctx, INITIAL_SECRET).await, status(ctx, ROTATED_SECRET).await) {
        (Ok(actual_initial), Ok(actual_rotated)) if (actual_initial, actual_rotated) == (initial, rotated) => {
            TestResult::passed(start.elapsed())
        }
        (Ok(actual_initial), Ok(actual_rotated)) => TestResult::failed(
            start.elapsed(),
            format!("Expected initial/rotated status {initial}/{rotated}, got {actual_initial}/{actual_rotated}"),
        ),
        (initial_error, rotated_error) => TestResult::failed(
            start.elapsed(),
            format!("Request failure: initial={initial_error:?}, rotated={rotated_error:?}"),
        ),
    }
}

pub struct RequestAccessPolicyHmacDynamicInitialTestSuite;

impl TestSuite for RequestAccessPolicyHmacDynamicInitialTestSuite {
    fn name(&self) -> &str {
        "RequestAccessPolicy HMAC Dynamic - Initial"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "request_access_policy_hmac_dynamic_initial",
            "Initial Secret accepts only the initial HMAC key",
            |ctx| Box::pin(async move { assert_statuses(&ctx, 200, 403).await }),
        )]
    }
}

pub struct RequestAccessPolicyHmacDynamicUpdatedTestSuite;

impl TestSuite for RequestAccessPolicyHmacDynamicUpdatedTestSuite {
    fn name(&self) -> &str {
        "RequestAccessPolicy HMAC Dynamic - Updated"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "request_access_policy_hmac_dynamic_updated",
            "Rotated Secret rejects the stale HMAC key and accepts the replacement",
            |ctx| Box::pin(async move { assert_statuses(&ctx, 403, 200).await }),
        )]
    }
}
