//! Focused end-to-end coverage for bounded semantic SSE Guardrail decisions.

use std::time::Instant;

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use serde::Deserialize;
use serde_json::json;

pub struct GuardrailTestSuite;

const PASS_HOST: &str = "guardrail-pass.example.com";
const REPLACE_HOST: &str = "guardrail-replace.example.com";
const REJECT_HOST: &str = "guardrail-reject.example.com";
const MALFORMED_HOST: &str = "guardrail-malformed.example.com";
const TIMEOUT_HOST: &str = "guardrail-timeout.example.com";
const EXT_PROC_HOST: &str = "guardrail-ext-proc.example.com";
const PATH: &str = "/v1/chat/completions?mock_mode=guardrail_split";
const TWO_WINDOWS_PATH: &str = "/v1/chat/completions?mock_mode=guardrail_two_windows";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GuardrailObservation {
    calls: u64,
    total_windows: u64,
    last_window_count: usize,
    version_v1: bool,
    minimal_envelope: bool,
    sorted_unique_keys: bool,
    canonical_content_window: bool,
}

fn provider_observation() -> &'static str {
    r#"{"fixture_id":"openai-compatible-v1","auth_is_provider_key":true,"credential_generation":"a","auth_is_client_key":false,"host_is_expected":true,"path_has_prefix":true,"query_is_preserved":false,"model_is_canonical":true,"model_is_economy":false,"model_is_unknown":false,"stream_requested":true,"include_usage":true}"#
}

fn expected_pass_body() -> Vec<u8> {
    format!(
        "data: {{\"id\":\"stream-fixture\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"fixed \"}}}}],\"mock\":{}}}\n\n\
         data: {{\"id\":\"stream-fixture\",\"choices\":[],\"usage\":{{\"prompt_tokens\":11,\"completion_tokens\":7,\"total_tokens\":18}}}}\n\n\
         data: [DONE]\n\n",
        provider_observation()
    )
    .into_bytes()
}

fn expected_replace_body() -> Vec<u8> {
    let original = expected_pass_body();
    let marker = b"\"content\":\"fixed \"";
    let position = original
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("fixture content token must exist");
    let mut replaced = Vec::with_capacity(original.len());
    replaced.extend_from_slice(&original[..position]);
    replaced.extend_from_slice(b"\"content\":\"reviewed\"");
    replaced.extend_from_slice(&original[position + marker.len()..]);
    replaced
}

async fn request_with_trace(
    ctx: &TestContext,
    host: &str,
    trace_id: Option<&str>,
) -> Result<reqwest::Response, String> {
    request_at_path(ctx, host, PATH, trace_id).await
}

async fn request_at_path(
    ctx: &TestContext,
    host: &str,
    path: &str,
    trace_id: Option<&str>,
) -> Result<reqwest::Response, String> {
    let mut request = ctx
        .http_client
        .post(format!("{}{path}", ctx.http_url()))
        .header("host", host)
        .json(&json!({
            "model": "fixture-model",
            "stream": true,
            "messages": [{"role": "user", "content": "fixture prompt"}]
        }));
    if let Some(trace_id) = trace_id {
        request = request
            .header("x-trace-id", trace_id)
            .header("access_log", "test_store");
    }
    request
        .send()
        .await
        .map_err(|error| format!("Guardrail AI request failed: {error}"))
}

async fn request(ctx: &TestContext, host: &str) -> Result<reqwest::Response, String> {
    request_with_trace(ctx, host, None).await
}

fn fixture_url(ctx: &TestContext, path: &str) -> String {
    format!("http://{}:30001{path}", ctx.target_host)
}

async fn reset_observations(ctx: &TestContext) -> Result<(), String> {
    let response = ctx
        .http_client
        .post(fixture_url(ctx, "/guardrail/reset"))
        .send()
        .await
        .map_err(|error| format!("failed to reset Guardrail observations: {error}"))?;
    if response.status() != reqwest::StatusCode::NO_CONTENT {
        return Err(format!("Guardrail reset returned {}", response.status()));
    }
    Ok(())
}

async fn assert_one_canonical_call(ctx: &TestContext, action: &str) -> Result<(), String> {
    assert_canonical_calls(ctx, action, 1).await
}

async fn assert_canonical_calls(ctx: &TestContext, action: &str, expected_calls: u64) -> Result<(), String> {
    let response = ctx
        .http_client
        .get(fixture_url(ctx, &format!("/guardrail/observation/{action}")))
        .send()
        .await
        .map_err(|error| format!("failed to query Guardrail observation: {error}"))?;
    let observation: GuardrailObservation = response
        .json()
        .await
        .map_err(|error| format!("invalid Guardrail observation: {error}"))?;
    if observation.calls != expected_calls
        || observation.total_windows != expected_calls
        || observation.last_window_count != 1
        || !observation.version_v1
        || !observation.minimal_envelope
        || !observation.sorted_unique_keys
        || !observation.canonical_content_window
    {
        return Err(format!("unexpected Guardrail observation: {observation:?}"));
    }
    Ok(())
}

fn held_sse_leaked(delivered: &[u8]) -> bool {
    [b"fixed ".as_slice(), b"stream-fixture".as_slice(), b"[DONE]".as_slice()]
        .iter()
        .any(|marker| delivered.windows(marker.len()).any(|window| window == *marker))
}

impl GuardrailTestSuite {
    fn exact_body_case(
        name: &'static str,
        description: &'static str,
        host: &'static str,
        action: &'static str,
        expected: Vec<u8>,
    ) -> TestCase {
        TestCase::new(name, description, move |ctx: TestContext| {
            let expected = expected.clone();
            Box::pin(async move {
                let start = Instant::now();
                if let Err(error) = reset_observations(&ctx).await {
                    return TestResult::failed(start.elapsed(), error);
                }
                let response = match request(&ctx, host).await {
                    Ok(response) => response,
                    Err(error) => return TestResult::failed(start.elapsed(), error),
                };
                if response.status() != reqwest::StatusCode::OK {
                    return TestResult::failed(
                        start.elapsed(),
                        format!("expected 200 response head, got {}", response.status()),
                    );
                }
                let actual = match response.bytes().await {
                    Ok(body) => body,
                    Err(error) => {
                        return TestResult::failed(start.elapsed(), format!("failed to read SSE body: {error}"))
                    }
                };
                if actual.as_ref() == expected.as_slice() {
                    if let Err(error) = assert_one_canonical_call(&ctx, action).await {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    TestResult::passed_with_message(start.elapsed(), "SSE bytes matched exactly".to_string())
                } else {
                    TestResult::failed(
                        start.elapsed(),
                        format!(
                            "SSE bytes differed: expected {} bytes, got {} bytes; body={}",
                            expected.len(),
                            actual.len(),
                            String::from_utf8_lossy(&actual[..actual.len().min(500)])
                        ),
                    )
                }
            })
        })
    }

    fn pass_is_byte_identical() -> TestCase {
        Self::exact_body_case(
            "pass_is_byte_identical",
            "Pass releases the canonical provider SSE byte-identically",
            PASS_HOST,
            "pass",
            expected_pass_body(),
        )
    }

    fn replace_rewrites_only_semantic_token() -> TestCase {
        Self::exact_body_case(
            "replace_rewrites_only_semantic_token",
            "Replace changes only the reviewed semantic JSON string token",
            REPLACE_HOST,
            "replace",
            expected_replace_body(),
        )
    }

    fn malformed_fail_open_releases_original() -> TestCase {
        Self::exact_body_case(
            "malformed_fail_open_releases_original",
            "A malformed dependency response under fail-open releases original bytes",
            MALFORMED_HOST,
            "malformed",
            expected_pass_body(),
        )
    }

    fn policy_reject_is_not_failed_open() -> TestCase {
        TestCase::new(
            "policy_reject_is_not_failed_open",
            "A policy Reject discards held bytes even when failOpen is true",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(error) = reset_observations(&ctx).await {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    let mut response = match request(&ctx, REJECT_HOST).await {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if response.status() != reqwest::StatusCode::OK {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("expected committed 200 response head, got {}", response.status()),
                        );
                    }

                    let mut delivered = Vec::new();
                    loop {
                        match response.chunk().await {
                            Ok(Some(chunk)) => delivered.extend_from_slice(&chunk),
                            Ok(None) | Err(_) => break,
                        }
                    }
                    if held_sse_leaked(&delivered) {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "policy Reject leaked held SSE bytes: {}",
                                String::from_utf8_lossy(&delivered)
                            ),
                        )
                    } else {
                        if let Err(error) = assert_one_canonical_call(&ctx, "reject").await {
                            return TestResult::failed(start.elapsed(), error);
                        }
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "policy Reject discarded the held window before termination".to_string(),
                        )
                    }
                })
            },
        )
    }

    fn one_call_per_canonical_window() -> TestCase {
        TestCase::new(
            "one_call_per_canonical_window",
            "Two canonical semantic windows produce exactly two serial decision calls",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(error) = reset_observations(&ctx).await {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    let response = match request_at_path(&ctx, PASS_HOST, TWO_WINDOWS_PATH, None).await {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let body = response.bytes().await.unwrap_or_default();
                    if !body.windows(b"first".len()).any(|window| window == b"first")
                        || !body.windows(b"second".len()).any(|window| window == b"second")
                    {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "two-window Pass lost provider bytes: {}",
                                String::from_utf8_lossy(&body)
                            ),
                        );
                    }
                    if let Err(error) = assert_canonical_calls(&ctx, "pass", 2).await {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "two canonical windows produced two bounded calls".to_string(),
                    )
                })
            },
        )
    }

    fn timeout_fail_close_discards_window() -> TestCase {
        TestCase::new(
            "timeout_fail_close_discards_window",
            "A decision exceeding the static deadline fails closed without releasing its window",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(error) = reset_observations(&ctx).await {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    let mut response = match request(&ctx, TIMEOUT_HOST).await {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if response.status() != reqwest::StatusCode::OK {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("expected committed 200 response head, got {}", response.status()),
                        );
                    }
                    let mut delivered = Vec::new();
                    loop {
                        match response.chunk().await {
                            Ok(Some(chunk)) => delivered.extend_from_slice(&chunk),
                            Ok(None) | Err(_) => break,
                        }
                    }
                    if held_sse_leaked(&delivered) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "timeout fail-close leaked SSE bytes: {}",
                                String::from_utf8_lossy(&delivered)
                            ),
                        );
                    }
                    if let Err(error) = assert_one_canonical_call(&ctx, "timeout").await {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "deadline failure discarded the held window".to_string(),
                    )
                })
            },
        )
    }

    fn ext_proc_observes_guardrail_output() -> TestCase {
        TestCase::new(
            "ext_proc_observes_guardrail_output",
            "Guardrail and ExtProc remain independent ordered response processors",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(error) = reset_observations(&ctx).await {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    let response = match request(&ctx, EXT_PROC_HOST).await {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let header_ok = response
                        .headers()
                        .get("x-ext-proc-resp")
                        .and_then(|value| value.to_str().ok())
                        == Some("seen");
                    let body = response.bytes().await.unwrap_or_default();
                    let body_ok = body
                        .windows(b"response-by-attached-ext-proc".len())
                        .any(|window| window == b"response-by-attached-ext-proc");
                    if let Err(error) = assert_one_canonical_call(&ctx, "pass").await {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    if header_ok && body_ok {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "Guardrail called once before ExtProc mutated the response".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "ExtProc evidence missing: header_ok={header_ok} body={}",
                                String::from_utf8_lossy(&body)
                            ),
                        )
                    }
                })
            },
        )
    }

    fn aggregate_log_is_emitted_once() -> TestCase {
        TestCase::new(
            "aggregate_log_is_emitted_once",
            "One request-level Guardrail summary is logged for one canonical window",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("guardrail-log-{}", uuid::Uuid::new_v4());
                    let response = match request_with_trace(&ctx, PASS_HOST, Some(&trace_id)).await {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let _ = response.bytes().await;
                    let entry = match ctx
                        .access_log_client()
                        .get_access_log_with_retry(&trace_id, 15, 200)
                        .await
                    {
                        Ok(entry) => entry,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("access log not found: {error}"))
                        }
                    };
                    let mut lines = Vec::new();
                    if let Some(stages) = entry.data["stage_logs"].as_array() {
                        for stage in stages {
                            if let Some(resources) = stage["edgion_plugins"].as_array() {
                                for resource in resources {
                                    if let Some(logs) = resource["logs"].as_array() {
                                        for log in logs {
                                            if let Some(items) = log["log"].as_array() {
                                                lines.extend(items.iter().filter_map(|item| item.as_str()));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    let summaries = lines
                        .iter()
                        .filter(|line| line.starts_with("guardrail "))
                        .copied()
                        .collect::<Vec<_>>();
                    if summaries.len() == 1 && summaries[0].contains("windows=1") && summaries[0].contains("passed=1") {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "exactly one aggregate Guardrail summary was logged".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("expected one aggregate Guardrail summary, got {summaries:?}"),
                        )
                    }
                })
            },
        )
    }
}

impl TestSuite for GuardrailTestSuite {
    fn name(&self) -> &str {
        "Guardrail"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::pass_is_byte_identical(),
            Self::replace_rewrites_only_semantic_token(),
            Self::policy_reject_is_not_failed_open(),
            Self::malformed_fail_open_releases_original(),
            Self::one_call_per_canonical_window(),
            Self::timeout_fail_close_discards_window(),
            Self::ext_proc_observes_guardrail_output(),
            Self::aggregate_log_is_emitted_once(),
        ]
    }
}
