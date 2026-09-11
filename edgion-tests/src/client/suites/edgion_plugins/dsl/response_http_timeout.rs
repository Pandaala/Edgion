//! Response-stage async HTTP suspension and configured timeout enforcement.

use crate::framework::{TestCase, TestContext, TestResult};
use std::time::{Duration, Instant};

pub(super) fn response_http_timeout() -> TestCase {
    TestCase::new(
        "dsl_response_http_timeout",
        "Response-stage HTTP calls complete after suspension and honor the configured timeout cap",
        |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let result = check_response_http_timeout(&ctx).await;
                let elapsed = Instant::now().saturating_duration_since(start);
                match result {
                    Ok(()) => TestResult::passed(elapsed),
                    Err(error) => TestResult::failed(elapsed, error),
                }
            })
        },
    )
}

async fn check_response_http_timeout(ctx: &TestContext) -> Result<(), String> {
    let mut response = ctx
        .http_client
        .get(format!("{}/health", ctx.edgion_plugins_url()))
        .header("Host", "dsl-response-http-timeout.example.com")
        // Bound a broken bridge without relying on a tight wall-clock assertion.
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|error| format!("Response-stage HTTP request failed: {error}"))?;

    if response.status().as_u16() != 200 {
        return Err(format!("Expected proxied HTTP 200, got {}", response.status()));
    }
    for (header, expected) in [
        ("x-dsl-response-http-control", "ok"),
        ("x-dsl-response-http-result", "timeout"),
    ] {
        let actual = response.headers().get(header).and_then(|value| value.to_str().ok());
        if actual != Some(expected) {
            return Err(format!("Expected {header}={expected:?}, got {actual:?}"));
        }
    }

    // Verify the complete original /health body, regardless of chunk boundaries,
    // without aggregating a response or accepting a successful header-only read.
    let expected_body = b"OK";
    let mut received = 0usize;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("Proxied health response body failed: {error}"))?
    {
        if chunk.len() > expected_body.len() - received
            || chunk.as_ref() != &expected_body[received..received + chunk.len()]
        {
            return Err("Proxied health response body changed after response-stage HTTP calls".to_string());
        }
        received += chunk.len();
    }
    if received != expected_body.len() {
        return Err("Proxied health response body ended before the complete OK payload".to_string());
    }
    Ok(())
}
