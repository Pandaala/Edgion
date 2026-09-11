//! Real request-to-response DSL context continuity and request isolation.

use crate::framework::{TestCase, TestContext, TestResult};
use std::time::Instant;

pub(super) fn request_response_context() -> TestCase {
    TestCase::new(
        "dsl_request_response_context",
        "Request-stage context reaches response-stage DSL and is isolated between sequential requests",
        |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let result = check_context_sequence(&ctx).await;
                let elapsed = Instant::now().saturating_duration_since(start);
                match result {
                    Ok(()) => TestResult::passed(elapsed),
                    Err(error) => TestResult::failed(elapsed, error),
                }
            })
        },
    )
}

async fn check_context_sequence(ctx: &TestContext) -> Result<(), String> {
    let url = format!("{}/test/dsl-request-response-context/api", ctx.edgion_plugins_url());
    // Keep the positive and negative requests in one case: suite scheduling must
    // not turn the isolation checks into independent requests with no predecessor.
    for (index, (write, expected)) in [
        (Some("alpha"), "response:phase-alpha"),
        (None, "missing"),
        (Some("beta"), "response:phase-beta"),
        (None, "missing"),
    ]
    .into_iter()
    .enumerate()
    {
        let step = index + 1;
        let mut request = ctx
            .http_client
            .get(&url)
            .header("Host", "dsl-request-response-context.example.com");
        if let Some(value) = write {
            request = request.header("X-Dsl-Context-Write", value);
        }
        let mut response = request
            .send()
            .await
            .map_err(|error| format!("Context sequence request {step} failed: {error}"))?;
        let status = response.status().as_u16();
        let actual = response
            .headers()
            .get("x-dsl-context-result")
            .and_then(|value| value.to_str().ok());
        if status != 200 || actual != Some(expected) {
            return Err(format!(
                "Context sequence request {step}: expected status=200 and marker={expected:?}, got status={status} and marker={actual:?}"
            ));
        }

        // Drain each small backend response without aggregation so the shared
        // client's connection can be reused before sending the next request.
        let mut received = 0usize;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| format!("Context sequence response {step} body failed: {error}"))?
        {
            received += chunk.len();
            if received > 64 * 1024 {
                return Err(format!(
                    "Context sequence response {step} exceeded the 64 KiB test body limit"
                ));
            }
        }
    }
    Ok(())
}
