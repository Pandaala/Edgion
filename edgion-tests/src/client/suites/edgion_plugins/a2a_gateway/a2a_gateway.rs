use std::time::{Duration, Instant};

use futures::StreamExt;
use serde_json::{json, Value};

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};

const CARD_HOST: &str = "a2a-card.example.com";
const RPC_CLOSE_HOST: &str = "a2a-rpc-close.example.com";
const RPC_OPEN_HOST: &str = "a2a-rpc-open.example.com";
const DYNAMIC_HOST: &str = "a2a-card-dynamic.example.com";
const CARD_PATH: &str = "/.well-known/agent-card.json";
const EDGION_PLUGINS_PORT: u16 = 31180;
const RPC_REQUEST: &str = r#"{"jsonrpc":"2.0","method":"message/send","id":"request-id","params":{"message":{"role":"ROLE_USER","parts":[{"text":"fixture request"}]}}}"#;
const JSON_SUCCESS_BODY: &str = r#"{"jsonrpc":"2.0","id":"fixture-response-id","result":{"task":{"id":"private-task-id","contextId":"private-context-id","status":{"state":"TASK_STATE_COMPLETED"},"history":[{"role":"ROLE_AGENT","parts":[{"text":"private-result-payload"}]}]}}}"#;
const JSON_ERROR_BODY: &str = r#"{"jsonrpc":"2.0","id":"fixture-error-id","error":{"code":-32001,"message":"private backend error detail","data":{"credential":"private-error-token"}}}"#;
const SSE_EVENTS: [&str; 4] = [
    "data: {\"jsonrpc\":\"2.0\",\"id\":\"fixture-stream-id\",\"result\":{\"task\":{\"id\":\"private-stream-task\",\"contextId\":\"private-stream-context\",\"status\":{\"state\":\"TASK_STATE_SUBMITTED\"}}}}\n\n",
    "data: {\"jsonrpc\":\"2.0\",\"id\":\"fixture-stream-id\",\"result\":{\"message\":{\"role\":\"ROLE_AGENT\",\"parts\":[{\"text\":\"private-stream-message\"}]}}}\n\n",
    "data: {\"jsonrpc\":\"2.0\",\"id\":\"fixture-stream-id\",\"result\":{\"artifactUpdate\":{\"artifact\":{\"artifactId\":\"private-artifact-id\",\"parts\":[{\"text\":\"private-artifact-payload\"}]}}}}\n\n",
    "data: {\"jsonrpc\":\"2.0\",\"id\":\"fixture-stream-id\",\"result\":{\"statusUpdate\":{\"status\":{\"state\":\"TASK_STATE_COMPLETED\",\"message\":{\"role\":\"ROLE_AGENT\",\"parts\":[{\"text\":\"private-final-message\"}]}}}}}\n\n",
];
const PRIVATE_RESPONSE_VALUES: [&str; 14] = [
    "fixture-response-id",
    "private-task-id",
    "private-context-id",
    "private-result-payload",
    "fixture-error-id",
    "private backend error detail",
    "private-error-token",
    "fixture-stream-id",
    "private-stream-task",
    "private-stream-context",
    "private-stream-message",
    "private-artifact-id",
    "private-artifact-payload",
    "private-final-message",
];

async fn get_card(ctx: &TestContext, host: &str) -> Result<reqwest::Response, String> {
    let base = if host == DYNAMIC_HOST {
        format!("http://{}:{EDGION_PLUGINS_PORT}", ctx.target_host)
    } else {
        ctx.http_url()
    };
    ctx.http_client
        .get(format!("{base}{CARD_PATH}"))
        .header("host", host)
        .send()
        .await
        .map_err(|error| format!("Agent Card request failed: {error}"))
}

async fn rpc(ctx: &TestContext, host: &str, version: &str, body: &'static str) -> Result<reqwest::Response, String> {
    ctx.http_client
        .post(format!("{}/echo", ctx.http_url()))
        .header("host", host)
        .header("content-type", "application/json")
        .header("a2a-version", version)
        .body(body)
        .send()
        .await
        .map_err(|error| format!("A2A JSON-RPC request failed: {error}"))
}

async fn rpc_fixture(ctx: &TestContext, host: &str, path: &str, trace_id: &str) -> Result<reqwest::Response, String> {
    let base = if host == DYNAMIC_HOST {
        format!("http://{}:{EDGION_PLUGINS_PORT}", ctx.target_host)
    } else {
        ctx.http_url()
    };
    ctx.http_client
        .post(format!("{base}{path}"))
        .header("host", host)
        .header("content-type", "application/json")
        .header("a2a-version", "1.0")
        .header("x-trace-id", trace_id)
        .header("access_log", "test_store")
        .body(RPC_REQUEST)
        .send()
        .await
        .map_err(|error| format!("A2A response fixture request failed: {error}"))
}

fn response_summaries(entry: &crate::access_log_client::AccessLogEntry) -> Vec<&str> {
    let mut lines = Vec::new();
    if let Some(stages) = entry.data["stage_logs"].as_array() {
        for stage in stages {
            if let Some(resources) = stage["edgion_plugins"].as_array() {
                for resource in resources {
                    if let Some(logs) = resource["logs"].as_array() {
                        for log in logs {
                            if let Some(items) = log["log"].as_array() {
                                lines.extend(
                                    items
                                        .iter()
                                        .filter_map(Value::as_str)
                                        .filter(|line| line.starts_with("a2a_resp ")),
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

async fn assert_response_summary(ctx: &TestContext, trace_id: &str, required_terms: &[&str]) -> Result<(), String> {
    let entry = ctx
        .access_log_client()
        .get_access_log_with_retry(trace_id, 15, 200)
        .await
        .map_err(|error| format!("A2A access log not found: {error}"))?;
    let summaries = response_summaries(&entry);
    if summaries.len() != 1 {
        return Err(format!("expected one A2A response summary, got {summaries:?}"));
    }
    if required_terms.iter().any(|term| !summaries[0].contains(term)) {
        return Err(format!(
            "A2A response summary did not contain {required_terms:?}: {:?}",
            summaries[0]
        ));
    }
    let serialized = entry.data.to_string();
    if let Some(leaked) = PRIVATE_RESPONSE_VALUES
        .iter()
        .find(|value| serialized.contains(**value))
    {
        return Err(format!("A2A access log leaked a private response value: {leaked}"));
    }
    Ok(())
}

async fn assert_dynamic_card(ctx: &TestContext, version: &str, endpoint: &str) -> Result<(), String> {
    let response = get_card(ctx, DYNAMIC_HOST).await?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(format!("expected dynamic card 200, got {}", response.status()));
    }
    let card: Value = response
        .json()
        .await
        .map_err(|error| format!("dynamic Agent Card is invalid JSON: {error}"))?;
    if card["version"] != version || card["supportedInterfaces"][0]["url"] != endpoint {
        return Err(format!("unexpected dynamic Agent Card: {card}"));
    }
    Ok(())
}

async fn assert_dynamic_response_observer(ctx: &TestContext) -> Result<(), String> {
    let trace_id = format!("a2a-dynamic-{}", uuid::Uuid::new_v4());
    let response = rpc_fixture(ctx, DYNAMIC_HOST, "/a2a/json-success", &trace_id).await?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(format!("expected dynamic RPC 200, got {}", response.status()));
    }
    let body = response
        .bytes()
        .await
        .map_err(|error| format!("dynamic RPC response read failed: {error}"))?;
    if body.as_ref() != JSON_SUCCESS_BODY.as_bytes() {
        return Err(format!("dynamic observer changed the {}-byte RPC response", body.len()));
    }
    assert_response_summary(ctx, &trace_id, &["outcome=success", "kind=task", "state=completed"]).await
}

pub struct A2aGatewayTestSuite;

impl A2aGatewayTestSuite {
    fn local_card_is_filtered_and_rewritten() -> TestCase {
        TestCase::new(
            "a2a_local_card_is_filtered_and_rewritten",
            "The backendless discovery route returns a bounded card with only JSONRPC/1.0 interfaces",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let response = match get_card(&ctx, CARD_HOST).await {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if response.status() != reqwest::StatusCode::OK {
                        return TestResult::failed(start.elapsed(), format!("expected 200, got {}", response.status()));
                    }
                    if response
                        .headers()
                        .get("cache-control")
                        .and_then(|value| value.to_str().ok())
                        != Some("no-store")
                    {
                        return TestResult::failed(start.elapsed(), "missing Cache-Control: no-store".to_string());
                    }
                    let card: Value = match response.json().await {
                        Ok(card) => card,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("invalid Agent Card JSON: {error}"))
                        }
                    };
                    let expected_urls = json!([
                        "https://public.agent.example:8443/rpc?tenant=blue",
                        "https://public.agent.example:8443/rpc-secondary"
                    ]);
                    let actual_urls = card["supportedInterfaces"]
                        .as_array()
                        .map(|interfaces| {
                            Value::Array(interfaces.iter().map(|interface| interface["url"].clone()).collect())
                        })
                        .unwrap_or(Value::Null);
                    if actual_urls != expected_urls
                        || card["supportedInterfaces"][0]["extensionField"] != "preserved"
                        || card["extensionObject"]["retained"] != true
                    {
                        return TestResult::failed(start.elapsed(), format!("unexpected rewritten card: {card}"));
                    }
                    TestResult::passed(start.elapsed())
                })
            },
        )
    }

    fn valid_json_rpc_is_forwarded_byte_for_byte() -> TestCase {
        const BODY: &str = "{\n  \"jsonrpc\" : \"2.0\", \"method\" : \"message/send\", \"id\" : 7, \"params\" : {}\n}";
        TestCase::new(
            "a2a_valid_json_rpc_is_forwarded_byte_for_byte",
            "A valid A2A protocol request is observed through BodyStorage and replayed unchanged",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match rpc(&ctx, RPC_CLOSE_HOST, "1.0", BODY).await {
                        Ok(response) if response.status() == reqwest::StatusCode::OK => match response.text().await {
                            Ok(body) if body.split_once("Echo: ").map(|(_, value)| value) == Some(BODY) => {
                                TestResult::passed(start.elapsed())
                            }
                            Ok(body) => TestResult::failed(
                                start.elapsed(),
                                format!("backend did not receive exact body: {body:?}"),
                            ),
                            Err(error) => TestResult::failed(start.elapsed(), format!("response read failed: {error}")),
                        },
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("expected 200, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    fn wrong_version_fails_closed() -> TestCase {
        TestCase::new(
            "a2a_wrong_version_fails_closed",
            "A mismatched A2A-Version is rejected before the body is observed when failOpen is false",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match rpc(&ctx, RPC_CLOSE_HOST, "0.3", r#"{"method":"message/send"}"#).await {
                        Ok(response) if response.status() == reqwest::StatusCode::BAD_REQUEST => {
                            TestResult::passed(start.elapsed())
                        }
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("expected 400, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    fn wrong_version_can_fail_open_without_body_observation() -> TestCase {
        const BODY: &str = "not-json-and-must-not-be-observed";
        TestCase::new(
            "a2a_wrong_version_can_fail_open_without_body_observation",
            "failOpen forwards a protocol-header mismatch without parsing or changing its body",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match rpc(&ctx, RPC_OPEN_HOST, "0.3", BODY).await {
                        Ok(response) if response.status() == reqwest::StatusCode::OK => match response.text().await {
                            Ok(body) if body.split_once("Echo: ").map(|(_, value)| value) == Some(BODY) => {
                                TestResult::passed(start.elapsed())
                            }
                            Ok(body) => {
                                TestResult::failed(start.elapsed(), format!("fail-open changed the body: {body:?}"))
                            }
                            Err(error) => TestResult::failed(start.elapsed(), format!("response read failed: {error}")),
                        },
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("expected 200, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    fn json_rpc_success_is_observed_without_changing_bytes() -> TestCase {
        TestCase::new(
            "a2a_json_rpc_success_is_observed_without_changing_bytes",
            "A bounded task result is forwarded byte-identically and summarized once",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("a2a-success-{}", uuid::Uuid::new_v4());
                    let response = match rpc_fixture(&ctx, RPC_CLOSE_HOST, "/a2a/json-success", &trace_id).await {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if response.status() != reqwest::StatusCode::OK {
                        return TestResult::failed(start.elapsed(), format!("expected 200, got {}", response.status()));
                    }
                    match response.bytes().await {
                        Ok(body) if body.as_ref() == JSON_SUCCESS_BODY.as_bytes() => {}
                        Ok(body) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("A2A success response bytes changed: {} bytes", body.len()),
                            )
                        }
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("response read failed: {error}"))
                        }
                    }
                    match assert_response_summary(&ctx, &trace_id, &["outcome=success", "kind=task", "state=completed"])
                        .await
                    {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    fn json_rpc_error_is_observed_and_redacted() -> TestCase {
        TestCase::new(
            "a2a_json_rpc_error_is_observed_and_redacted",
            "An A2A error class is summarized without logging its message, data, or id",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("a2a-error-{}", uuid::Uuid::new_v4());
                    let response = match rpc_fixture(&ctx, RPC_CLOSE_HOST, "/a2a/json-error", &trace_id).await {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    match response.bytes().await {
                        Ok(body) if body.as_ref() == JSON_ERROR_BODY.as_bytes() => {}
                        Ok(body) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("A2A error response bytes changed: {} bytes", body.len()),
                            )
                        }
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("response read failed: {error}"))
                        }
                    }
                    match assert_response_summary(&ctx, &trace_id, &["outcome=error", "kind=none", "error=a2a"]).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    fn paced_sse_events_are_observed_without_changing_bytes() -> TestCase {
        TestCase::new(
            "a2a_paced_sse_events_are_observed_without_changing_bytes",
            "Split A2A task/message/artifact/status events remain paced and byte-identical",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("a2a-sse-{}", uuid::Uuid::new_v4());
                    let response = match rpc_fixture(&ctx, RPC_CLOSE_HOST, "/a2a/sse", &trace_id).await {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if response.status() != reqwest::StatusCode::OK
                        || response
                            .headers()
                            .get(reqwest::header::CONTENT_TYPE)
                            .and_then(|value| value.to_str().ok())
                            .is_none_or(|value| !value.starts_with("text/event-stream"))
                    {
                        return TestResult::failed(start.elapsed(), "fixture did not return A2A SSE".to_string());
                    }

                    let body_started = Instant::now();
                    let mut stream = response.bytes_stream();
                    let mut raw = Vec::new();
                    let mut completed_events = Vec::new();
                    let mut scanned = 0;
                    while let Some(chunk) = stream.next().await {
                        match chunk {
                            Ok(chunk) => raw.extend_from_slice(&chunk),
                            Err(error) => {
                                return TestResult::failed(start.elapsed(), format!("SSE read failed: {error}"))
                            }
                        }
                        while let Some(relative) = raw[scanned..].windows(2).position(|window| window == b"\n\n") {
                            scanned += relative + 2;
                            completed_events.push(Instant::now());
                        }
                    }

                    let expected = SSE_EVENTS.concat();
                    if raw != expected.as_bytes() {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("A2A SSE bytes changed: expected {}, got {}", expected.len(), raw.len()),
                        );
                    }
                    if completed_events.len() != SSE_EVENTS.len() {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "expected {} complete SSE events, got {}",
                                SSE_EVENTS.len(),
                                completed_events.len()
                            ),
                        );
                    }
                    if completed_events[0].saturating_duration_since(body_started) >= Duration::from_millis(900) {
                        return TestResult::failed(start.elapsed(), "first A2A SSE event was buffered".to_string());
                    }
                    if completed_events[3].saturating_duration_since(completed_events[0]) < Duration::from_millis(400) {
                        return TestResult::failed(
                            start.elapsed(),
                            "A2A SSE events arrived as a buffered burst".to_string(),
                        );
                    }
                    match assert_response_summary(
                        &ctx,
                        &trace_id,
                        &["outcome=success", "kind=status-update", "state=completed", "events=4"],
                    )
                    .await
                    {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }
}

impl TestSuite for A2aGatewayTestSuite {
    fn name(&self) -> &str {
        "A2aGateway"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::local_card_is_filtered_and_rewritten(),
            Self::valid_json_rpc_is_forwarded_byte_for_byte(),
            Self::wrong_version_fails_closed(),
            Self::wrong_version_can_fail_open_without_body_observation(),
            Self::json_rpc_success_is_observed_without_changing_bytes(),
            Self::json_rpc_error_is_observed_and_redacted(),
            Self::paced_sse_events_are_observed_without_changing_bytes(),
        ]
    }
}

pub struct A2aGatewayDynamicV1TestSuite;

impl TestSuite for A2aGatewayDynamicV1TestSuite {
    fn name(&self) -> &str {
        "A2aGateway Dynamic V1"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            TestCase::new("a2a_dynamic_card_v1", "The initial card snapshot serves v1", |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match assert_dynamic_card(&ctx, "1.0.0", "https://dynamic.agent.example/rpc-v1?source=card").await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            }),
            TestCase::new(
                "a2a_dynamic_response_observer_v1",
                "The initial snapshot installs response observation for a new RPC request",
                |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        match assert_dynamic_response_observer(&ctx).await {
                            Ok(()) => TestResult::passed(start.elapsed()),
                            Err(error) => TestResult::failed(start.elapsed(), error),
                        }
                    })
                },
            ),
        ]
    }
}

pub struct A2aGatewayDynamicV2TestSuite;

impl TestSuite for A2aGatewayDynamicV2TestSuite {
    fn name(&self) -> &str {
        "A2aGateway Dynamic V2"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            TestCase::new(
                "a2a_dynamic_card_v2",
                "The hot-reloaded card snapshot serves v2",
                |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        match assert_dynamic_card(&ctx, "2.0.0", "https://dynamic.agent.example/rpc-v2?source=card")
                            .await
                        {
                            Ok(()) => TestResult::passed(start.elapsed()),
                            Err(error) => TestResult::failed(start.elapsed(), error),
                        }
                    })
                },
            ),
            TestCase::new(
                "a2a_dynamic_response_observer_v2",
                "The hot-reloaded snapshot installs response observation for a new RPC request",
                |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        match assert_dynamic_response_observer(&ctx).await {
                            Ok(()) => TestResult::passed(start.elapsed()),
                            Err(error) => TestResult::failed(start.elapsed(), error),
                        }
                    })
                },
            ),
        ]
    }
}
