//! Wire coverage for Guardrail failure-policy on mixed Content-Encoding.

use std::time::Instant;

use crate::framework::{env_host_or, TestCase, TestContext, TestResult, TestSuite};
use serde::Deserialize;
use serde_json::json;

pub struct GuardrailEncodingTestSuite;

const FAIL_OPEN_HOST: &str = "guardrail-encoding-fail-open.example.com";
const FAIL_CLOSE_HOST: &str = "guardrail-encoding-fail-close.example.com";
const PATH: &str = "/v1/chat/completions?mock_mode=guardrail_encoded";

#[derive(Debug, Deserialize)]
struct GuardrailObservation {
    calls: u64,
}

fn fixture_hosts(ctx: &TestContext) -> Vec<String> {
    if std::env::var("EDGION_TEST_K8S_MODE")
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
    {
        if let Ok(ips) = std::env::var("EDGION_TEST_BACKEND_ENDPOINT_IPS") {
            let hosts = ips
                .split(',')
                .map(str::trim)
                .filter(|host| !host.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            if !hosts.is_empty() {
                return hosts;
            }
        }
    }
    vec![env_host_or("EDGION_TEST_GUARDRAIL_FIXTURE_HOST", &ctx.target_host)]
}

fn fixture_url(host: &str, path: &str) -> String {
    format!("http://{host}:30001{path}")
}

async fn reset_observations(ctx: &TestContext) -> Result<(), String> {
    for host in fixture_hosts(ctx) {
        let response = ctx
            .http_client
            .post(fixture_url(&host, "/guardrail/reset"))
            .send()
            .await
            .map_err(|error| format!("failed to reset Guardrail observations on {host}: {error}"))?;
        if response.status() != reqwest::StatusCode::NO_CONTENT {
            return Err(format!("Guardrail reset on {host} returned {}", response.status()));
        }
    }
    Ok(())
}

async fn observed_pass_calls(ctx: &TestContext) -> Result<u64, String> {
    let mut calls = 0;
    for host in fixture_hosts(ctx) {
        let response = ctx
            .http_client
            .get(fixture_url(&host, "/guardrail/observation/pass"))
            .send()
            .await
            .map_err(|error| format!("failed to query Guardrail observation on {host}: {error}"))?;
        let observation: GuardrailObservation = response
            .json()
            .await
            .map_err(|error| format!("invalid Guardrail observation from {host}: {error}"))?;
        calls += observation.calls;
    }
    Ok(calls)
}

async fn request_encoded(ctx: &TestContext, host: &str, trace_id: Option<&str>) -> Result<reqwest::Response, String> {
    let mut request = ctx
        .http_client
        .post(format!("{}{PATH}", ctx.http_url()))
        .header("host", host)
        .header("connection", "close")
        .header("accept-encoding", "identity")
        .json(&json!({
            "model": "fixture-model",
            "stream": true,
            "messages": [{"role": "user", "content": "fixture prompt"}]
        }));
    if let Some(trace_id) = trace_id {
        request = request
            .header(
                "traceparent",
                crate::access_log_client::traceparent_for_marker(trace_id),
            )
            .header("access_log", "test_store");
    }
    request
        .send()
        .await
        .map_err(|error| format!("Guardrail encoding request failed: {error}"))
}

fn mixed_identity_and_gzip(headers: &reqwest::header::HeaderMap) -> bool {
    let joined = headers
        .get_all(reqwest::header::CONTENT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join(",");
    joined.contains("identity") && joined.contains("gzip")
}

fn guardrail_summaries(entry: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(stages) = entry["stage_logs"].as_array() {
        for stage in stages {
            if let Some(resources) = stage["edgion_plugins"].as_array() {
                for resource in resources {
                    if let Some(logs) = resource["logs"].as_array() {
                        for log in logs {
                            if let Some(items) = log["log"].as_array() {
                                lines.extend(
                                    items
                                        .iter()
                                        .filter_map(|item| item.as_str())
                                        .filter(|line| line.starts_with("guardrail "))
                                        .map(ToOwned::to_owned),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    lines
}

async fn assert_zero_decision_calls(ctx: &TestContext) -> Result<(), String> {
    let calls = observed_pass_calls(ctx).await?;
    if calls == 0 {
        Ok(())
    } else {
        Err(format!(
            "encoding rejection called the decision service {calls} time(s)"
        ))
    }
}

async fn assert_aggregate_summary(ctx: &TestContext, trace_id: &str, outcome: &str) -> Result<(), String> {
    let entry = ctx
        .access_log_client()
        .get_access_log_with_retry(trace_id, 15, 200)
        .await
        .map_err(|error| format!("access log not found: {error}"))?;
    let summaries = guardrail_summaries(&entry.data);
    let expected_prefix = format!(
        "guardrail windows=0 passed=0 replaced=0 input_bytes=0 output_bytes=0 outcome={outcome} reason=response-not-applicable"
    );
    if summaries.len() == 1 && summaries[0].starts_with(&expected_prefix) {
        Ok(())
    } else {
        Err(format!("expected one {outcome} encoding summary, got {summaries:?}"))
    }
}

impl GuardrailEncodingTestSuite {
    fn mixed_encoding_fail_open_releases_original() -> TestCase {
        TestCase::new(
            "mixed_encoding_fail_open_releases_original",
            "Repeated identity+gzip Content-Encoding under fail-open releases original SSE",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(error) = reset_observations(&ctx).await {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    let trace_id = format!("guardrail-encoding-open-{}", uuid::Uuid::new_v4());
                    let response = match request_encoded(&ctx, FAIL_OPEN_HOST, Some(&trace_id)).await {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if response.status() != reqwest::StatusCode::OK {
                        return TestResult::failed(start.elapsed(), format!("expected 200, got {}", response.status()));
                    }
                    if !mixed_identity_and_gzip(response.headers()) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "expected mixed identity+gzip Content-Encoding, got {:?}",
                                response.headers().get_all(reqwest::header::CONTENT_ENCODING)
                            ),
                        );
                    }
                    let body = match response.bytes().await {
                        Ok(body) => body,
                        Err(error) => return TestResult::failed(start.elapsed(), format!("body read: {error}")),
                    };
                    if !body
                        .windows(b"stream-fixture".len())
                        .any(|window| window == b"stream-fixture")
                        || !body.windows(b"[DONE]".len()).any(|window| window == b"[DONE]")
                    {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("fail-open lost original SSE bytes: {}", String::from_utf8_lossy(&body)),
                        );
                    }
                    if let Err(error) = assert_zero_decision_calls(&ctx).await {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    if let Err(error) = assert_aggregate_summary(&ctx, &trace_id, "fail-open").await {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "fail-open released original SSE without semantic processing".to_string(),
                    )
                })
            },
        )
    }

    fn mixed_encoding_fail_close_returns_503() -> TestCase {
        TestCase::new(
            "mixed_encoding_fail_close_returns_503",
            "Repeated identity+gzip Content-Encoding under fail-close returns 503 before SSE",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(error) = reset_observations(&ctx).await {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    let trace_id = format!("guardrail-encoding-close-{}", uuid::Uuid::new_v4());
                    let response = match request_encoded(&ctx, FAIL_CLOSE_HOST, Some(&trace_id)).await {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if response.status() != reqwest::StatusCode::SERVICE_UNAVAILABLE {
                        return TestResult::failed(start.elapsed(), format!("expected 503, got {}", response.status()));
                    }
                    let body = match response.bytes().await {
                        Ok(body) => body,
                        Err(error) => return TestResult::failed(start.elapsed(), format!("body read: {error}")),
                    };
                    if body
                        .windows(b"stream-fixture".len())
                        .any(|window| window == b"stream-fixture")
                        || body.windows(b"[DONE]".len()).any(|window| window == b"[DONE]")
                    {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "fail-close leaked original SSE bytes: {}",
                                String::from_utf8_lossy(&body)
                            ),
                        );
                    }
                    if let Err(error) = assert_zero_decision_calls(&ctx).await {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    if let Err(error) = assert_aggregate_summary(&ctx, &trace_id, "fail-close").await {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "fail-close returned 503 without semantic processing".to_string(),
                    )
                })
            },
        )
    }
}

impl TestSuite for GuardrailEncodingTestSuite {
    fn name(&self) -> &str {
        "GuardrailEncoding"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::mixed_encoding_fail_open_releases_original(),
            Self::mixed_encoding_fail_close_returns_503(),
        ]
    }
}
