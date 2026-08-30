use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const HOST: &str = "condition-refs-dynamic.example.com";
const INITIAL_TOKEN: &str = "initial-token";
const ROTATED_TOKEN: &str = "rotated-token";

async fn request(ctx: &TestContext, token: &str) -> Result<(u16, String, Option<String>, Option<String>), String> {
    let response = ctx
        .http_client
        .post(format!("http://{}:31180/echo", ctx.target_host))
        .header("host", HOST)
        .header("X-Body-Token", token)
        .header("X-Dye-Token", token)
        .body("abcdef")
        .send()
        .await
        .map_err(|error| format!("Condition reference request failed: {error}"))?;
    let status = response.status().as_u16();
    let dye = response
        .headers()
        .get("X-Condition-Ref-Dye")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body_marker = response
        .headers()
        .get("X-Condition-Ref-Body")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = response
        .text()
        .await
        .map_err(|error| format!("Condition reference response body failed: {error}"))?;
    Ok((status, body, dye, body_marker))
}

async fn assert_active_token(ctx: &TestContext, active: &str, stale: &str) -> TestResult {
    let start = Instant::now();
    let active_result = request(ctx, active).await;
    let stale_result = request(ctx, stale).await;
    match (active_result, stale_result) {
        (Ok((200, _, active_dye, active_body)), Ok((500, stale_body, stale_dye, stale_marker)))
            if active_dye.as_deref() == Some("matched")
                && active_body.as_deref() == Some("matched")
                && stale_body == "Request body processing failed"
                && stale_dye.is_none()
                && stale_marker.is_none() =>
        {
            TestResult::passed(start.elapsed())
        }
        (active_result, stale_result) => TestResult::failed(
            start.elapsed(),
            format!(
                "Unexpected active/stale condition reference results: active={active_result:?}, stale={stale_result:?}"
            ),
        ),
    }
}

pub struct ConditionRefsDynamicInitialTestSuite;

impl TestSuite for ConditionRefsDynamicInitialTestSuite {
    fn name(&self) -> &str {
        "Condition References Dynamic - Initial"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "condition_refs_dynamic_initial",
            "Initial Secret value enables body and Dye conditions",
            |ctx| Box::pin(async move { assert_active_token(&ctx, INITIAL_TOKEN, ROTATED_TOKEN).await }),
        )]
    }
}

pub struct ConditionRefsDynamicUpdatedTestSuite;

impl TestSuite for ConditionRefsDynamicUpdatedTestSuite {
    fn name(&self) -> &str {
        "Condition References Dynamic - Updated"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "condition_refs_dynamic_updated",
            "Rotated Secret value replaces stale body and Dye matches",
            |ctx| Box::pin(async move { assert_active_token(&ctx, ROTATED_TOKEN, INITIAL_TOKEN).await }),
        )]
    }
}

pub struct ConditionRefsDynamicDeletedTestSuite;

impl TestSuite for ConditionRefsDynamicDeletedTestSuite {
    fn name(&self) -> &str {
        "Condition References Dynamic - Deleted"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "condition_refs_dynamic_deleted",
            "Deleting the shared condition Secret fails the plugin resource closed",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match request(&ctx, INITIAL_TOKEN).await {
                        Ok((500, body, dye, body_marker))
                            if body.contains("plugin configuration error")
                                && dye.is_none()
                                && body_marker.is_none() =>
                        {
                            TestResult::passed(start.elapsed())
                        }
                        result => TestResult::failed(
                            start.elapsed(),
                            format!("Expected generic 500 without Dye after Secret deletion, got {result:?}"),
                        ),
                    }
                })
            },
        )]
    }
}
