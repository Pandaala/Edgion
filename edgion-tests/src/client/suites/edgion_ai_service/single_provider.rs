//! Static end-to-end coverage for one OpenAI-compatible AI service.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::metrics_helper::MetricsClient;
use async_trait::async_trait;
use edgion_gateway::plugins::common::redis_rate_limiter::build_cumulative_quota_keys;
use futures::StreamExt;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct SingleProviderTestSuite;
pub struct AiServiceSecretRotationInitialTestSuite;
pub struct AiServiceSecretRotationUpdatedTestSuite;

const DEFAULT_HOST: &str = "ai-default.example.com";
const PASSTHROUGH_HOST: &str = "ai-passthrough.example.com";
const OPTOUT_HOST: &str = "ai-optout.example.com";
const DISABLED_HOST: &str = "ai-disabled.example.com";
const QUOTA_HOST: &str = "ai-quota.example.com";
const RETRY_HOST: &str = "ai-retry.example.com";
const THROTTLE_HOST: &str = "ai-quota-throttle.example.com";
const DOWNGRADE_HOST: &str = "ai-quota-downgrade.example.com";
const PATH: &str = "/v1/chat/completions";
const QUOTA_WINDOW_MS: u128 = 60 * 60 * 1_000;

fn client(ctx: &TestContext, host: &str, h2: bool, raw_gzip: bool) -> Result<reqwest::Client, String> {
    let target_ip = ctx
        .target_host
        .parse::<IpAddr>()
        .ok()
        .or_else(|| {
            (ctx.target_host.as_str(), 0)
                .to_socket_addrs()
                .ok()?
                .next()
                .map(|addr| addr.ip())
        })
        .ok_or_else(|| format!("failed to resolve test target {}", ctx.target_host))?;
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .no_proxy()
        .resolve(host, SocketAddr::new(target_ip, ctx.http_port));
    if raw_gzip {
        builder = builder.no_gzip();
    }
    let builder = if h2 {
        builder.http2_prior_knowledge()
    } else {
        builder.http1_only()
    };
    builder
        .build()
        .map_err(|error| format!("failed to build HTTP client: {error}"))
}

fn request_url(ctx: &TestContext, host: &str, path: &str, query: &str) -> String {
    format!("http://{}:{}{}{}", host, ctx.http_port, path, query)
}

async fn post_inner(
    ctx: &TestContext,
    host: &str,
    body: Value,
    query: &str,
    h2: bool,
    trace_id: Option<&str>,
    send_client_authorization: bool,
    raw_gzip: bool,
) -> Result<reqwest::Response, String> {
    let mut request = client(ctx, host, h2, raw_gzip)?
        .post(request_url(ctx, host, PATH, query))
        .json(&body);
    if raw_gzip {
        request = request.header("accept-encoding", "gzip");
    }
    if send_client_authorization {
        request = request.header("authorization", "Bearer client-key-must-be-stripped");
    }
    if let Some(trace_id) = trace_id {
        request = request
            .header("x-trace-id", trace_id)
            .header("access_log", "test_store");
    }
    request
        .send()
        .await
        .map_err(|error| format!("AI request failed: {error}"))
}

async fn post(
    ctx: &TestContext,
    host: &str,
    body: Value,
    query: &str,
    h2: bool,
    trace_id: Option<&str>,
) -> Result<reqwest::Response, String> {
    post_inner(ctx, host, body, query, h2, trace_id, false, false).await
}

async fn post_with_client_authorization(
    ctx: &TestContext,
    host: &str,
    body: Value,
    query: &str,
    h2: bool,
    trace_id: Option<&str>,
) -> Result<reqwest::Response, String> {
    post_inner(ctx, host, body, query, h2, trace_id, true, false).await
}

async fn post_with_raw_gzip(
    ctx: &TestContext,
    host: &str,
    body: Value,
    query: &str,
    h2: bool,
) -> Result<reqwest::Response, String> {
    post_inner(ctx, host, body, query, h2, None, false, true).await
}

async fn post_with_quota_identity(
    ctx: &TestContext,
    identity: &str,
    body: Value,
    trace_id: Option<&str>,
) -> Result<reqwest::Response, String> {
    post_with_quota_identity_on_host(ctx, QUOTA_HOST, identity, body, trace_id).await
}

async fn post_with_quota_identity_on_host(
    ctx: &TestContext,
    host: &str,
    identity: &str,
    body: Value,
    trace_id: Option<&str>,
) -> Result<reqwest::Response, String> {
    let mut request = client(ctx, host, false, false)?
        .post(request_url(ctx, host, PATH, ""))
        .header("x-quota-user", identity)
        .json(&body);
    if let Some(trace_id) = trace_id {
        request = request
            .header("x-trace-id", trace_id)
            .header("access_log", "test_store");
    }
    request
        .send()
        .await
        .map_err(|error| format!("quota AI request failed: {error}"))
}

fn append_quota_identity_part(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(&(value.len() as u64).to_be_bytes());
    output.extend_from_slice(value);
}

fn quota_bucket_key(identity: &str, window_id: u64) -> Result<String, String> {
    let mut quota_identity = Vec::new();
    append_quota_identity_part(&mut quota_identity, b"edgion-test/ai-proxy-quota/#0");
    append_quota_identity_part(&mut quota_identity, identity.as_bytes());
    append_quota_identity_part(&mut quota_identity, &QUOTA_WINDOW_MS.to_be_bytes());
    append_quota_identity_part(&mut quota_identity, b"identity");
    build_cumulative_quota_keys("edgion:aiq:", &quota_identity, window_id, b"integration-inspection")
        .map(|keys| keys.bucket)
        .map_err(|error| format!("failed to derive Redis quota bucket key: {error}"))
}

async fn quota_bucket_used(ctx: &TestContext, key: &str) -> Result<Option<u64>, String> {
    let encoded_key = utf8_percent_encode(key, NON_ALPHANUMERIC);
    let url = format!(
        "http://{}:12001/api/v1/testing/link-sys/redis/default_redis-test/hget/{}/used",
        ctx.target_host, encoded_key
    );
    let mut request = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .no_proxy()
        .build()
        .map_err(|error| format!("failed to build Gateway admin client: {error}"))?
        .get(url);
    if let Ok(token) = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN") {
        if !token.is_empty() {
            request = request.bearer_auth(token);
        }
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("Redis quota inspection failed: {error}"))?;
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .map_err(|error| format!("Redis quota inspection returned invalid JSON: {error}"))?;
    if !status.is_success() || body["success"] != true {
        return Err(format!("Redis quota inspection returned {status}: {body}"));
    }
    Ok(body["data"]["value"]
        .as_str()
        .and_then(|value| value.parse::<u64>().ok()))
}

async fn quota_bucket_used_between(
    ctx: &TestContext,
    identity: &str,
    before_ms: u128,
    after_ms: u128,
) -> Result<(String, u64), String> {
    let mut window_ids = vec![(before_ms / QUOTA_WINDOW_MS) as u64];
    let after_window = (after_ms / QUOTA_WINDOW_MS) as u64;
    if window_ids[0] != after_window {
        window_ids.push(after_window);
    }
    for window_id in window_ids {
        let key = quota_bucket_key(identity, window_id)?;
        if let Some(used) = quota_bucket_used(ctx, &key).await? {
            return Ok((key, used));
        }
    }
    Err("the expected Redis quota bucket was not found".to_string())
}

async fn long_stream_observation(ctx: &TestContext, trace_id: &str) -> Result<Value, String> {
    ctx.http_client
        .get(format!(
            "http://{}:30001/ai-long-stream/observation/{trace_id}",
            ctx.target_host
        ))
        .send()
        .await
        .map_err(|error| format!("long-stream observation request failed: {error}"))?
        .json()
        .await
        .map_err(|error| format!("invalid long-stream observation response: {error}"))
}

fn mock(value: &Value) -> Result<&serde_json::Map<String, Value>, String> {
    value
        .get("mock")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("provider response has no mock observation: {value}"))
}

fn expect_bool(observation: &serde_json::Map<String, Value>, key: &str, expected: bool) -> Result<(), String> {
    let actual = observation.get(key).and_then(Value::as_bool);
    if actual == Some(expected) {
        Ok(())
    } else {
        Err(format!("expected mock.{key}={expected}, got {actual:?}"))
    }
}

async fn assert_credential_generation(ctx: TestContext, expected: &str, check_admin: bool) -> TestResult {
    let start = Instant::now();
    let response = match post(
        &ctx,
        DEFAULT_HOST,
        json!({"model":"fixture-model","messages":[]}),
        "",
        false,
        None,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => return TestResult::failed(start.elapsed(), error),
    };
    if response.status() != reqwest::StatusCode::OK {
        return TestResult::failed(
            start.elapsed(),
            format!("fresh AI request returned {}", response.status()),
        );
    }
    let value: Value = match response.json().await {
        Ok(value) => value,
        Err(error) => return TestResult::failed(start.elapsed(), format!("invalid provider JSON: {error}")),
    };
    let observation = match mock(&value) {
        Ok(value) => value,
        Err(error) => return TestResult::failed(start.elapsed(), error),
    };
    let actual = observation.get("credential_generation").and_then(Value::as_str);
    if actual != Some(expected) {
        return TestResult::failed(
            start.elapsed(),
            format!("expected provider credential generation {expected:?}, got {actual:?}"),
        );
    }

    if check_admin {
        let admin_base = std::env::var("EDGION_GATEWAY_ADMIN_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| format!("http://{}:12001", ctx.target_host));
        let mut request = ctx.http_client.get(format!(
            "{}/configclient/edgionaiservice/list",
            admin_base.trim_end_matches('/')
        ));
        if let Ok(token) = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN") {
            if !token.trim().is_empty() {
                request = request.bearer_auth(token);
            }
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                return TestResult::failed(start.elapsed(), format!("gateway configclient request failed: {error}"));
            }
        };
        let status = response.status();
        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => {
                return TestResult::failed(start.elapsed(), format!("configclient body read failed: {error}"))
            }
        };
        if !status.is_success() {
            return TestResult::failed(start.elapsed(), format!("configclient returned {status}: {body}"));
        }
        let listed: Value = match serde_json::from_str(&body) {
            Ok(value) => value,
            Err(error) => return TestResult::failed(start.elapsed(), format!("invalid configclient JSON: {error}")),
        };
        let service = listed["data"].as_array().and_then(|items| {
            items.iter().find(|item| {
                item.pointer("/metadata/namespace").and_then(Value::as_str) == Some("edgion-test")
                    && item.pointer("/metadata/name").and_then(Value::as_str) == Some("fixture-provider")
            })
        });
        let Some(service) = service else {
            return TestResult::failed(
                start.elapsed(),
                "fixture-provider missing from configclient list".to_string(),
            );
        };
        let credentials = service
            .pointer("/spec/credentialPool/credentials")
            .and_then(Value::as_array);
        if credentials.is_none_or(|items| {
            items.is_empty()
                || items
                    .iter()
                    .any(|credential| credential.get("secret").and_then(Value::as_str) != Some("[redacted]"))
        }) {
            return TestResult::failed(
                start.elapsed(),
                "expected every credentialPool secret to be redacted".to_string(),
            );
        }
        let rendered = service.to_string();
        for forbidden in ["ai-provider-test-key", "ai-provider-rotated-test-key"] {
            if rendered.contains(forbidden) {
                return TestResult::failed(
                    start.elapsed(),
                    "configclient leaked provider credential material".to_string(),
                );
            }
        }
    }

    TestResult::passed_with_message(
        start.elapsed(),
        format!("fresh request used credential generation {expected}; admin credential is redacted={check_admin}"),
    )
}

fn credential_generation_a(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(assert_credential_generation(ctx, "a", false))
}

fn credential_generation_b_redacted(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(assert_credential_generation(ctx, "b", true))
}

async fn collect_sse(response: reqwest::Response) -> Result<(Vec<Vec<u8>>, Vec<Instant>), String> {
    let mut stream = response.bytes_stream();
    let mut pending = Vec::new();
    let mut frames = Vec::new();
    let mut arrivals = Vec::new();
    while let Some(chunk) = stream.next().await {
        pending.extend_from_slice(&chunk.map_err(|error| format!("SSE body failed: {error}"))?);
        while let Some(boundary) = pending.windows(2).position(|window| window == b"\n\n") {
            let mut frame = pending.drain(..boundary + 2).collect::<Vec<_>>();
            frame.truncate(boundary);
            frames.push(frame);
            arrivals.push(Instant::now());
        }
    }
    if !pending.is_empty() {
        return Err(format!("SSE response ended with {} unframed bytes", pending.len()));
    }
    Ok((frames, arrivals))
}

fn sse_json(frame: &[u8]) -> Result<Option<Value>, String> {
    let text = std::str::from_utf8(frame).map_err(|error| format!("SSE frame was not UTF-8: {error}"))?;
    let data = text
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .ok_or_else(|| format!("SSE frame has no data field: {text:?}"))?;
    if data == "[DONE]" {
        Ok(None)
    } else {
        serde_json::from_str(data)
            .map(Some)
            .map_err(|error| format!("invalid SSE JSON: {error}"))
    }
}

async fn assert_paced_protocol(ctx: TestContext, h2: bool) -> TestResult {
    let start = Instant::now();
    let response = match post_with_raw_gzip(
        &ctx,
        DEFAULT_HOST,
        json!({"model":"fixture-model", "stream":true, "messages":[{"role":"user","content":"hello"}]}),
        "?fixture_query=preserved",
        h2,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => return TestResult::failed(start.elapsed(), error),
    };
    let expected_version = if h2 {
        reqwest::Version::HTTP_2
    } else {
        reqwest::Version::HTTP_11
    };
    if response.version() != expected_version {
        return TestResult::failed(
            start.elapsed(),
            format!("expected {expected_version:?}, got {:?}", response.version()),
        );
    }
    if response.status() != reqwest::StatusCode::OK {
        return TestResult::failed(start.elapsed(), format!("expected 200, got {}", response.status()));
    }
    if response.headers().contains_key(reqwest::header::CONTENT_ENCODING) {
        return TestResult::failed(start.elapsed(), "streaming response must not be compressed".to_string());
    }
    let body_started = Instant::now();
    let (frames, arrivals) = match collect_sse(response).await {
        Ok(result) => result,
        Err(error) => return TestResult::failed(start.elapsed(), error),
    };
    if frames.len() != 4 || arrivals.len() != 4 {
        return TestResult::failed(
            start.elapsed(),
            format!("expected four SSE frames, got {}", frames.len()),
        );
    }
    let first_wait = arrivals[0].saturating_duration_since(body_started);
    if first_wait >= Duration::from_millis(900) {
        return TestResult::failed(
            start.elapsed(),
            "first SSE event was not delivered incrementally".to_string(),
        );
    }
    if arrivals[2].saturating_duration_since(arrivals[0]) < Duration::from_millis(400) {
        return TestResult::failed(start.elapsed(), "SSE events arrived as a buffered burst".to_string());
    }
    for pair in arrivals.windows(2) {
        let gap = pair[1].saturating_duration_since(pair[0]);
        if !(Duration::from_millis(100)..Duration::from_millis(1_000)).contains(&gap) {
            return TestResult::failed(
                start.elapsed(),
                format!("SSE event gap {gap:?} was outside pacing bounds"),
            );
        }
    }
    let first = match sse_json(&frames[0]) {
        Ok(Some(value)) => value,
        Ok(None) => return TestResult::failed(start.elapsed(), "first SSE frame was [DONE]".to_string()),
        Err(error) => return TestResult::failed(start.elapsed(), error),
    };
    let observation = match mock(&first) {
        Ok(value) => value,
        Err(error) => return TestResult::failed(start.elapsed(), error),
    };
    for (key, expected) in [
        ("auth_is_provider_key", true),
        ("auth_is_client_key", false),
        ("path_has_prefix", true),
        ("query_is_preserved", true),
        ("include_usage", true),
    ] {
        if let Err(error) = expect_bool(observation, key, expected) {
            return TestResult::failed(start.elapsed(), error);
        }
    }
    if sse_json(&frames[3]).ok().flatten().is_some() {
        return TestResult::failed(start.elapsed(), "stream did not end with [DONE]".to_string());
    }
    TestResult::passed_with_message(
        start.elapsed(),
        format!(
            "paced SSE remained incremental over {expected_version:?}; first={first_wait:?}, span={:?}",
            arrivals[3].saturating_duration_since(arrivals[0])
        ),
    )
}

fn h1_paced(ctx: TestContext) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(assert_paced_protocol(ctx, false))
}

fn h2_paced(ctx: TestContext) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(assert_paced_protocol(ctx, true))
}

fn canonical_alias_auth_prefix(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        for model in ["fixture-model", "fixture-alias"] {
            let response = match post_with_client_authorization(
                &ctx,
                DEFAULT_HOST,
                json!({"model":model,"messages":[]}),
                "?fixture_query=preserved",
                false,
                None,
            )
            .await
            {
                Ok(response) => response,
                Err(error) => return TestResult::failed(start.elapsed(), error),
            };
            let value: Value = match response.json().await {
                Ok(value) => value,
                Err(error) => return TestResult::failed(start.elapsed(), format!("invalid JSON response: {error}")),
            };
            let observation = match mock(&value) {
                Ok(value) => value,
                Err(error) => return TestResult::failed(start.elapsed(), error),
            };
            for (key, expected) in [
                ("model_is_canonical", true),
                ("auth_is_provider_key", true),
                ("auth_is_client_key", false),
                ("host_is_expected", true),
                ("path_has_prefix", true),
                ("query_is_preserved", true),
            ] {
                if let Err(error) = expect_bool(observation, key, expected) {
                    return TestResult::failed(start.elapsed(), format!("model {model}: {error}"));
                }
            }
        }

        TestResult::passed_with_message(
            start.elapsed(),
            "canonical and alias models preserve credential replacement, prefix, and query behavior".to_string(),
        )
    })
}

fn rejection_modes(ctx: TestContext) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let body = json!({"model":"unknown-fixture-model","messages":[]});
        let rejected = match post(&ctx, DEFAULT_HOST, body.clone(), "", false, None).await {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if rejected.status() != reqwest::StatusCode::NOT_FOUND {
            return TestResult::failed(
                start.elapsed(),
                format!("unknown model expected 404, got {}", rejected.status()),
            );
        }
        let rejected_body = match rejected.text().await {
            Ok(body) => body,
            Err(error) => return TestResult::failed(start.elapsed(), format!("failed to read rejection: {error}")),
        };
        if rejected_body.contains("mock") || !rejected_body.contains("model_not_found") {
            return TestResult::failed(start.elapsed(), format!("unexpected rejection body: {rejected_body}"));
        }

        let passed = match post(&ctx, PASSTHROUGH_HOST, body, "", false, None).await {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let value: Value = match passed.json().await {
            Ok(value) => value,
            Err(error) => return TestResult::failed(start.elapsed(), format!("invalid passthrough response: {error}")),
        };
        if let Err(error) = expect_bool(
            match mock(&value) {
                Ok(v) => v,
                Err(e) => return TestResult::failed(start.elapsed(), e),
            },
            "model_is_unknown",
            true,
        ) {
            return TestResult::failed(start.elapsed(), error);
        }

        let disabled = match post(
            &ctx,
            DISABLED_HOST,
            json!({"model":"fixture-model","stream":true,"messages":[]}),
            "",
            false,
            None,
        )
        .await
        {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if disabled.status() != reqwest::StatusCode::BAD_REQUEST {
            return TestResult::failed(
                start.elapsed(),
                format!("streaming-disabled expected 400, got {}", disabled.status()),
            );
        }
        let body = disabled.text().await.unwrap_or_default();
        if !body.contains("streaming_not_supported") {
            return TestResult::failed(start.elapsed(), format!("unexpected streaming-disabled body: {body}"));
        }
        TestResult::passed_with_message(
            start.elapsed(),
            "unknown-model and streaming policy branches passed".to_string(),
        )
    })
}

fn usage_injection_optout_and_split(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let optout_trace = format!("ai-optout-estimated-{}", uuid::Uuid::new_v4());
        let optout = match post(
            &ctx,
            OPTOUT_HOST,
            json!({"model":"fixture-model","stream":true,"messages":[]}),
            "?mock_mode=missing_usage",
            false,
            Some(&optout_trace),
        )
        .await
        {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if optout.status() != reqwest::StatusCode::OK {
            return TestResult::failed(
                start.elapsed(),
                format!("opt-out stream expected 200, got {}", optout.status()),
            );
        }
        let (frames, _) = match collect_sse(optout).await {
            Ok(value) => value,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let Some(first_frame) = frames.first() else {
            return TestResult::failed(start.elapsed(), "opt-out stream returned no SSE frames".to_string());
        };
        let first = match sse_json(first_frame) {
            Ok(Some(value)) => value,
            Ok(None) => return TestResult::failed(start.elapsed(), "optout stream started with [DONE]".to_string()),
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if let Err(error) = expect_bool(
            match mock(&first) {
                Ok(v) => v,
                Err(e) => return TestResult::failed(start.elapsed(), e),
            },
            "include_usage",
            false,
        ) {
            return TestResult::failed(start.elapsed(), error);
        }
        let optout_entry = match ctx
            .access_log_client()
            .get_access_log_with_retry(&optout_trace, 15, 200)
            .await
        {
            Ok(entry) => entry,
            Err(error) => return TestResult::failed(start.elapsed(), format!("opt-out access log missing: {error}")),
        };
        let estimated = &optout_entry.data["genai"];
        if estimated["usage_estimated"] != true
            || estimated["input_tokens"].as_u64().unwrap_or(0) == 0
            || estimated["output_tokens"].as_u64().unwrap_or(0) == 0
        {
            return TestResult::failed(start.elapsed(), format!("missing usage was not estimated: {estimated}"));
        }

        let metrics = MetricsClient::from_host_port(&ctx.target_host, 12090);
        let before = match metrics.fetch_raw_metrics().await {
            Ok(raw) => raw,
            Err(error) => return TestResult::failed(start.elapsed(), format!("split metric baseline failed: {error}")),
        };
        let trace = format!("ai-split-usage-{}", uuid::Uuid::new_v4());
        let split = match post(
            &ctx,
            DEFAULT_HOST,
            json!({"model":"fixture-model","stream":true,"messages":[]}),
            "?mock_mode=split",
            false,
            Some(&trace),
        )
        .await
        {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if split.status() != reqwest::StatusCode::OK {
            return TestResult::failed(
                start.elapsed(),
                format!("split stream expected 200, got {}", split.status()),
            );
        }
        if let Err(error) = collect_sse(split).await {
            return TestResult::failed(start.elapsed(), error);
        }
        let entry = match ctx.access_log_client().get_access_log_with_retry(&trace, 15, 200).await {
            Ok(entry) => entry,
            Err(error) => return TestResult::failed(start.elapsed(), format!("split access log missing: {error}")),
        };
        let genai = &entry.data["genai"];
        if genai["input_tokens"] != 11 || genai["output_tokens"] != 7 || genai["usage_estimated"] != false {
            return TestResult::failed(
                start.elapsed(),
                format!("split usage was not extracted exactly: {genai}"),
            );
        }
        let after = match metrics.fetch_raw_metrics().await {
            Ok(raw) => raw,
            Err(error) => return TestResult::failed(start.elapsed(), format!("split metric read failed: {error}")),
        };
        let input_delta = metric_value(&after, "input", "reported") - metric_value(&before, "input", "reported");
        let output_delta = metric_value(&after, "output", "reported") - metric_value(&before, "output", "reported");
        if input_delta != 11.0 || output_delta != 7.0 {
            return TestResult::failed(
                start.elapsed(),
                format!("split metric deltas expected 11/7, got {input_delta}/{output_delta}"),
            );
        }
        TestResult::passed_with_message(
            start.elapsed(),
            "usage injection opt-out and split-field extraction passed".to_string(),
        )
    })
}

fn metric_value(raw: &str, direction: &str, source: &str) -> f64 {
    raw.lines()
        .filter(|line| line.starts_with("edgion_gateway_genai_tokens_total{"))
        .filter(|line| line.contains(&format!("direction=\"{direction}\"")))
        .filter(|line| line.contains("provider=\"openai_compatible\""))
        .filter(|line| line.contains(&format!("usage_source=\"{source}\"")))
        .filter_map(|line| line.split_whitespace().last()?.parse::<f64>().ok())
        .sum()
}

fn exact_access_log_and_metric(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let metrics = MetricsClient::from_host_port(&ctx.target_host, 12090);
        let before = match metrics.fetch_raw_metrics().await {
            Ok(raw) => raw,
            Err(error) => return TestResult::failed(start.elapsed(), format!("metrics baseline failed: {error}")),
        };
        let trace = format!("ai-exact-nonstream-{}", uuid::Uuid::new_v4());
        let response = match post(
            &ctx,
            DEFAULT_HOST,
            json!({"model":"fixture-alias","messages":[{"role":"user","content":"do not log this prompt"}]}),
            "",
            false,
            Some(&trace),
        )
        .await
        {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if response.status() != reqwest::StatusCode::OK {
            return TestResult::failed(
                start.elapsed(),
                format!("nonstream usage request got {}", response.status()),
            );
        }
        let response_text = match response.text().await {
            Ok(body) => body,
            Err(error) => return TestResult::failed(start.elapsed(), format!("response read failed: {error}")),
        };
        let entry = match ctx.access_log_client().get_access_log_with_retry(&trace, 15, 200).await {
            Ok(entry) => entry,
            Err(error) => return TestResult::failed(start.elapsed(), format!("access log missing: {error}")),
        };
        let genai = &entry.data["genai"];
        let expected = json!({
            "provider":"openai_compatible",
            "model":"fixture-alias",
            "input_tokens":11,
            "output_tokens":7,
            "streaming":false,
            "usage_estimated":false
        });
        for (key, value) in expected.as_object().unwrap() {
            if &genai[key] != value {
                return TestResult::failed(
                    start.elapsed(),
                    format!("genai.{key} expected {value}, got {}", genai[key]),
                );
            }
        }
        if genai["estimated_cost_usd"] != "0.00025" {
            return TestResult::failed(
                start.elapsed(),
                format!(
                    "configured pricing expected estimated_cost_usd=0.00025, got {}",
                    genai["estimated_cost_usd"]
                ),
            );
        }
        if !genai["quota_status"].is_null() {
            return TestResult::failed(
                start.elapsed(),
                format!("the no-quota route unexpectedly recorded quota_status: {genai}"),
            );
        }
        let rendered = entry.data.to_string();
        for forbidden in [
            "do not log this prompt",
            "fixed completion",
            "client-key-must-be-stripped",
            "ai-provider-test-key",
            "ai-provider-rotated-test-key",
        ] {
            if rendered.contains(forbidden) {
                return TestResult::failed(
                    start.elapsed(),
                    format!("access log leaked forbidden fixture {forbidden:?}"),
                );
            }
        }
        if !response_text.contains("fixed completion") {
            return TestResult::failed(
                start.elapsed(),
                "provider fixture response was not received".to_string(),
            );
        }
        let after = match metrics.fetch_raw_metrics().await {
            Ok(raw) => raw,
            Err(error) => return TestResult::failed(start.elapsed(), format!("metrics read failed: {error}")),
        };
        let input_delta = metric_value(&after, "input", "reported") - metric_value(&before, "input", "reported");
        let output_delta = metric_value(&after, "output", "reported") - metric_value(&before, "output", "reported");
        if input_delta != 11.0 || output_delta != 7.0 {
            return TestResult::failed(
                start.elapsed(),
                format!("expected metric deltas input=11/output=7, got {input_delta}/{output_delta}"),
            );
        }
        TestResult::passed_with_message(
            start.elapsed(),
            "exact access-log usage and bounded metric deltas passed".to_string(),
        )
    })
}

fn token_quota_reconciliation_and_admission(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let identity = format!("quota-{}", uuid::Uuid::new_v4());
        let trace = format!("ai-quota-stream-{}", uuid::Uuid::new_v4());
        let before_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let response = match post_with_quota_identity(
            &ctx,
            &identity,
            json!({"model":"fixture-model","stream":true,"messages":[]}),
            Some(&trace),
        )
        .await
        {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if response.status() != reqwest::StatusCode::OK {
            return TestResult::failed(
                start.elapsed(),
                format!("quota stream expected 200, got {}", response.status()),
            );
        }
        if let Err(error) = collect_sse(response).await {
            return TestResult::failed(start.elapsed(), error);
        }
        let after_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();

        let entry = match ctx.access_log_client().get_access_log_with_retry(&trace, 15, 200).await {
            Ok(entry) => entry,
            Err(error) => return TestResult::failed(start.elapsed(), format!("quota access log missing: {error}")),
        };
        let genai = &entry.data["genai"];
        if genai["input_tokens"] != 11
            || genai["output_tokens"] != 7
            || genai["quota_status"] != "charged"
            || genai["estimated_cost_usd"] != "0.00025"
        {
            return TestResult::failed(
                start.elapsed(),
                format!("quota stream was not reconciled with exact usage and cost: {genai}"),
            );
        }

        match quota_bucket_used_between(&ctx, &identity, before_ms, after_ms).await {
            Ok((_, 18)) => {}
            Ok((key, used)) => {
                return TestResult::failed(
                    start.elapsed(),
                    format!("quota bucket {key} expected exact final usage 18, got {used}"),
                );
            }
            Err(error) => return TestResult::failed(start.elapsed(), error),
        }

        let denied =
            match post_with_quota_identity(&ctx, &identity, json!({"model":"fixture-model","messages":[]}), None).await
            {
                Ok(response) => response,
                Err(error) => return TestResult::failed(start.elapsed(), error),
            };
        if denied.status() != reqwest::StatusCode::TOO_MANY_REQUESTS {
            return TestResult::failed(
                start.elapsed(),
                format!("exhausted quota expected 429, got {}", denied.status()),
            );
        }
        let retry_after = denied
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        if retry_after.is_none_or(|seconds| seconds == 0) {
            return TestResult::failed(
                start.elapsed(),
                format!("quota 429 returned invalid Retry-After: {retry_after:?}"),
            );
        }
        let body: Value = match denied.json().await {
            Ok(body) => body,
            Err(error) => {
                return TestResult::failed(start.elapsed(), format!("quota 429 returned invalid JSON: {error}"));
            }
        };
        if body["error"]["type"] != "rate_limit_error" || body["error"]["code"] != "token_quota_exceeded" {
            return TestResult::failed(
                start.elapsed(),
                format!("quota 429 returned the wrong OpenAI error envelope: {body}"),
            );
        }

        TestResult::passed_with_message(
            start.elapsed(),
            "streaming quota reconciled to 18 tokens in Redis and the next request was rejected".to_string(),
        )
    })
}

fn streaming_client_disconnect_finalizes_once(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let identity = format!("disconnect-{}", uuid::Uuid::new_v4());
        let trace = format!("ai-stream-disconnect-{}", uuid::Uuid::new_v4());
        let before_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let response = match client(&ctx, QUOTA_HOST, true, false) {
            Ok(client) => {
                client
                    .post(request_url(&ctx, QUOTA_HOST, PATH, "?mock_mode=long"))
                    .header("x-quota-user", &identity)
                    .header("x-trace-id", &trace)
                    .header("access_log", "test_store")
                    .json(&json!({"model":"fixture-model","stream":true,"messages":[]}))
                    .send()
                    .await
            }
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let response = match response {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), format!("disconnect request failed: {error}")),
        };
        if response.status() != reqwest::StatusCode::OK {
            return TestResult::failed(
                start.elapsed(),
                format!("disconnect stream expected 200, got {}", response.status()),
            );
        }

        let mut body = response.bytes_stream();
        let mut partial = Vec::new();
        while partial.windows(2).filter(|window| *window == b"\n\n").count() < 3 {
            let next = match tokio::time::timeout(Duration::from_secs(2), body.next()).await {
                Ok(Some(Ok(bytes))) => bytes,
                Ok(Some(Err(error))) => {
                    return TestResult::failed(start.elapsed(), format!("disconnect stream read failed: {error}"));
                }
                Ok(None) => {
                    return TestResult::failed(start.elapsed(), "long stream ended before three frames".to_string());
                }
                Err(_) => {
                    return TestResult::failed(start.elapsed(), "long stream did not yield three frames".to_string());
                }
            };
            partial.extend_from_slice(&next);
            if partial.len() > 64 * 1024 {
                return TestResult::failed(start.elapsed(), "long stream prefix exceeded 64 KiB".to_string());
            }
        }
        if partial.windows(7).any(|window| window == b"\"usage\"")
            || partial.windows(6).any(|window| window == b"[DONE]")
        {
            return TestResult::failed(
                start.elapsed(),
                "long stream reached terminal usage before the client disconnect".to_string(),
            );
        }
        drop(body);

        let mut provider_state = Value::Null;
        for _ in 0..40 {
            provider_state = match long_stream_observation(&ctx, &trace).await {
                Ok(state) => state,
                Err(error) => return TestResult::failed(start.elapsed(), error),
            };
            if provider_state["observation"]["upstream_cancelled"] == true
                || provider_state["observation"]["provider_completed"] == true
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if provider_state["found"] != true
            || provider_state["observation"]["upstream_cancelled"] != true
            || provider_state["observation"]["provider_completed"] != false
            || provider_state["observation"]["chunks_queued"]
                .as_u64()
                .is_none_or(|chunks| !(3..103).contains(&chunks))
        {
            return TestResult::failed(
                start.elapsed(),
                format!("gateway did not cancel the long provider stream: {provider_state}"),
            );
        }

        let entry = match ctx.access_log_client().get_access_log_with_retry(&trace, 20, 200).await {
            Ok(entry) => entry,
            Err(error) => {
                return TestResult::failed(start.elapsed(), format!("disconnect access log missing: {error}"));
            }
        };
        let genai = &entry.data["genai"];
        let Some(attempts) = genai["attempts"].as_array() else {
            return TestResult::failed(start.elapsed(), format!("disconnect attempts missing: {genai}"));
        };
        if genai["usage_estimated"] != true
            || genai["input_tokens"].as_u64().unwrap_or_default() == 0
            || genai["output_tokens"].as_u64().unwrap_or_default() == 0
            || genai["attempts_truncated"] == true
            || attempts.len() != 1
            || attempts[0]["number"] != 1
            || attempts[0]["status"] != 200
            || attempts[0]["failure"] != "incomplete_response"
            || attempts[0]["usage_source"] != "estimated"
            || attempts[0]["input_tokens"] != genai["input_tokens"]
            || attempts[0]["output_tokens"] != genai["output_tokens"]
        {
            return TestResult::failed(
                start.elapsed(),
                format!("disconnect did not produce one estimated incomplete attempt: {genai}"),
            );
        }

        let listed = match ctx.access_log_client().list_access_logs().await {
            Ok(listed) => listed,
            Err(error) => return TestResult::failed(start.elapsed(), format!("access-log list failed: {error}")),
        };
        let matching = listed
            .data
            .iter()
            .filter(|item| item.trace_id == trace)
            .collect::<Vec<_>>();
        if matching.len() != 1 || matching[0].store_count != 1 {
            return TestResult::failed(
                start.elapsed(),
                format!("disconnect logging boundary ran more than once: {matching:?}"),
            );
        }

        let after_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let (bucket_key, used) = match quota_bucket_used_between(&ctx, &identity, before_ms, after_ms).await {
            Ok(used) => used,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let attempt_total = attempts[0]["input_tokens"]
            .as_u64()
            .unwrap_or_default()
            .saturating_add(attempts[0]["output_tokens"].as_u64().unwrap_or_default());
        let expected_quota_status = if used > 18 { "over_limit" } else { "charged" };
        if used != attempt_total || genai["quota_status"] != expected_quota_status {
            return TestResult::failed(
                start.elapsed(),
                format!(
                    "disconnect quota reconciliation mismatch for {bucket_key}: used={used}, attempt={attempt_total}, genai={genai}"
                ),
            );
        }

        TestResult::passed_with_message(
            start.elapsed(),
            format!(
                "client disconnected after three SSE frames; provider cancelled, one incomplete attempt logged, and {used} estimated tokens reconciled"
            ),
        )
    })
}

fn priority_retry_attempt_ledger(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let trace = format!("ai-priority-retry-{}", uuid::Uuid::new_v4());
        let response = match post(
            &ctx,
            RETRY_HOST,
            json!({"model":"fixture-model","messages":[{"role":"user","content":"retry fixture"}]}),
            "",
            false,
            Some(&trace),
        )
        .await
        {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if response.status() != reqwest::StatusCode::OK {
            return TestResult::failed(
                start.elapsed(),
                format!("priority retry expected 200, got {}", response.status()),
            );
        }
        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => return TestResult::failed(start.elapsed(), format!("retry body read failed: {error}")),
        };
        if !body.contains("fixed completion") {
            return TestResult::failed(start.elapsed(), "retry did not reach the healthy provider".to_string());
        }

        let entry = match ctx.access_log_client().get_access_log_with_retry(&trace, 15, 200).await {
            Ok(entry) => entry,
            Err(error) => return TestResult::failed(start.elapsed(), format!("retry access log missing: {error}")),
        };
        let genai = &entry.data["genai"];
        let Some(attempts) = genai["attempts"].as_array() else {
            return TestResult::failed(start.elapsed(), format!("genai.attempts is not an array: {genai}"));
        };
        if attempts.len() != 2 || genai["attempts_truncated"] == true {
            return TestResult::failed(
                start.elapsed(),
                format!("expected two bounded, untruncated attempts: {genai}"),
            );
        }

        let allowed_fields = [
            "number",
            "service",
            "provider",
            "credential",
            "status",
            "failure",
            "retry_after_seconds",
            "ttft_ms",
            "usage_source",
            "input_tokens",
            "output_tokens",
            "estimated_cost_usd",
        ];
        for attempt in attempts {
            let Some(object) = attempt.as_object() else {
                return TestResult::failed(start.elapsed(), format!("attempt is not an object: {attempt}"));
            };
            if let Some(field) = object.keys().find(|field| !allowed_fields.contains(&field.as_str())) {
                return TestResult::failed(
                    start.elapsed(),
                    format!("attempt ledger exposed unexpected field {field:?}: {attempt}"),
                );
            }
        }

        let first = &attempts[0];
        let second = &attempts[1];
        if first["number"] != 1
            || first["service"] != "edgion-test/fixture-provider-retryable"
            || first["provider"] != "openai_compatible"
            || first["credential"] != "retry-primary"
            || first["status"] != 503
            || first["failure"] != "retryable_status"
            || first["usage_source"] != "estimated"
            || first["input_tokens"].as_u64().unwrap_or_default() == 0
            || first["output_tokens"] != 0
        {
            return TestResult::failed(start.elapsed(), format!("unexpected first attempt ledger: {first}"));
        }
        if second["number"] != 2
            || second["service"] != "edgion-test/fixture-provider"
            || second["provider"] != "openai_compatible"
            || second["credential"] != "primary"
            || second["status"] != 200
            || !second["failure"].is_null()
            || second["usage_source"] != "reported"
            || second["input_tokens"] != 11
            || second["output_tokens"] != 7
        {
            return TestResult::failed(start.elapsed(), format!("unexpected second attempt ledger: {second}"));
        }
        let summed_input = attempts
            .iter()
            .filter_map(|attempt| attempt["input_tokens"].as_u64())
            .sum::<u64>();
        let summed_output = attempts
            .iter()
            .filter_map(|attempt| attempt["output_tokens"].as_u64())
            .sum::<u64>();
        if summed_input <= 11 || summed_output != 7 {
            return TestResult::failed(
                start.elapsed(),
                format!("attempt usage was not summed across retry and success: {attempts:?}"),
            );
        }
        let rendered = entry.data.to_string();
        for forbidden in [
            "fixed retryable provider failure",
            "retry fixture",
            "ai-provider-test-key",
            "ai-provider-rotated-test-key",
            "127.0.0.1:30001",
        ] {
            if rendered.contains(forbidden) {
                return TestResult::failed(
                    start.elapsed(),
                    format!("attempt access log leaked forbidden fixture {forbidden:?}"),
                );
            }
        }
        TestResult::passed_with_message(
            start.elapsed(),
            format!("503 failover produced two safe attempts with summed usage {summed_input}/{summed_output}"),
        )
    })
}

fn token_quota_throttle_one_shot(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let identity = format!("quota-throttle-{}", uuid::Uuid::new_v4());
        let first_trace = format!("ai-quota-throttle-first-{}", uuid::Uuid::new_v4());
        let first = match post_with_quota_identity_on_host(
            &ctx,
            THROTTLE_HOST,
            &identity,
            json!({"model":"fixture-model","messages":[]}),
            Some(&first_trace),
        )
        .await
        {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if first.status() != reqwest::StatusCode::OK {
            return TestResult::failed(
                start.elapsed(),
                format!("throttle quota seed expected 200, got {}", first.status()),
            );
        }
        if let Err(error) = first.bytes().await {
            return TestResult::failed(start.elapsed(), format!("throttle seed body read failed: {error}"));
        }
        let first_entry = match ctx
            .access_log_client()
            .get_access_log_with_retry(&first_trace, 15, 200)
            .await
        {
            Ok(entry) => entry,
            Err(error) => {
                return TestResult::failed(start.elapsed(), format!("throttle seed access log missing: {error}"));
            }
        };
        if first_entry.data["genai"]["quota_status"] != "charged" {
            return TestResult::failed(
                start.elapsed(),
                format!("throttle seed quota did not reconcile: {}", first_entry.data["genai"]),
            );
        }

        let denied = match post_with_quota_identity_on_host(
            &ctx,
            THROTTLE_HOST,
            &identity,
            json!({"model":"fixture-model","messages":[]}),
            None,
        )
        .await
        {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if denied.status() != reqwest::StatusCode::TOO_MANY_REQUESTS {
            return TestResult::failed(
                start.elapsed(),
                format!("one-shot throttle retry expected 429, got {}", denied.status()),
            );
        }
        let retry_after = denied
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        if retry_after.is_none_or(|seconds| seconds == 0) {
            return TestResult::failed(
                start.elapsed(),
                format!("throttle 429 returned invalid Retry-After: {retry_after:?}"),
            );
        }
        let body: Value = match denied.json().await {
            Ok(body) => body,
            Err(error) => return TestResult::failed(start.elapsed(), format!("throttle 429 JSON failed: {error}")),
        };
        if body["error"]["code"] != "token_quota_exceeded" {
            return TestResult::failed(start.elapsed(), format!("unexpected throttle error envelope: {body}"));
        }
        TestResult::passed_with_message(
            start.elapsed(),
            "Throttle performed its bounded one-shot re-admission and returned the stable quota 429".to_string(),
        )
    })
}

fn token_quota_downgrade(ctx: TestContext) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let identity = format!("quota-downgrade-{}", uuid::Uuid::new_v4());
        let seed_trace = format!("ai-quota-premium-seed-{}", uuid::Uuid::new_v4());
        let seed = match post_with_quota_identity_on_host(
            &ctx,
            DOWNGRADE_HOST,
            &identity,
            json!({"model":"premium-model","messages":[]}),
            Some(&seed_trace),
        )
        .await
        {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if seed.status() != reqwest::StatusCode::OK {
            return TestResult::failed(
                start.elapsed(),
                format!("premium quota seed expected 200, got {}", seed.status()),
            );
        }
        if let Err(error) = seed.bytes().await {
            return TestResult::failed(start.elapsed(), format!("premium seed body read failed: {error}"));
        }
        let seed_entry = match ctx
            .access_log_client()
            .get_access_log_with_retry(&seed_trace, 15, 200)
            .await
        {
            Ok(entry) => entry,
            Err(error) => {
                return TestResult::failed(start.elapsed(), format!("premium seed access log missing: {error}"));
            }
        };
        if seed_entry.data["genai"]["quota_status"] != "charged" {
            return TestResult::failed(
                start.elapsed(),
                format!("premium seed quota did not reconcile: {}", seed_entry.data["genai"]),
            );
        }

        let trace = format!("ai-quota-downgrade-{}", uuid::Uuid::new_v4());
        let response = match post_with_quota_identity_on_host(
            &ctx,
            DOWNGRADE_HOST,
            &identity,
            json!({"model":"premium-model","messages":[]}),
            Some(&trace),
        )
        .await
        {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if response.status() != reqwest::StatusCode::OK {
            return TestResult::failed(
                start.elapsed(),
                format!("downgraded request expected 200, got {}", response.status()),
            );
        }
        let disclosed = response
            .headers()
            .get("x-edgion-ai-model-downgraded")
            .and_then(|value| value.to_str().ok());
        if disclosed != Some("economy-model") {
            return TestResult::failed(
                start.elapsed(),
                format!("downgrade disclosure header expected economy-model, got {disclosed:?}"),
            );
        }
        let body: Value = match response.json().await {
            Ok(body) => body,
            Err(error) => return TestResult::failed(start.elapsed(), format!("downgrade JSON failed: {error}")),
        };
        let observation = match mock(&body) {
            Ok(observation) => observation,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if let Err(error) = expect_bool(observation, "model_is_economy", true) {
            return TestResult::failed(start.elapsed(), error);
        }

        let entry = match ctx.access_log_client().get_access_log_with_retry(&trace, 15, 200).await {
            Ok(entry) => entry,
            Err(error) => {
                return TestResult::failed(start.elapsed(), format!("downgrade access log missing: {error}"));
            }
        };
        let genai = &entry.data["genai"];
        if genai["model"] != "economy-model" || genai["quota_status"] != "charged" {
            return TestResult::failed(
                start.elapsed(),
                format!("downgrade target or quota status missing from access log: {genai}"),
            );
        }
        TestResult::passed_with_message(
            start.elapsed(),
            "Premium quota exhaustion re-admitted economy-model and disclosed the bounded target".to_string(),
        )
    })
}

fn oversized_passthrough(ctx: TestContext) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let response = match post(
            &ctx,
            DEFAULT_HOST,
            json!({"model":"fixture-model","stream":true,"messages":[]}),
            "?mock_mode=oversized",
            false,
            None,
        )
        .await
        {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let body = match response.bytes().await {
            Ok(body) => body,
            Err(error) => return TestResult::failed(start.elapsed(), format!("oversized stream failed: {error}")),
        };
        let expected = format!(
            "data: {{\"id\":\"oversized-fixture\",\"choices\":[{{\"delta\":{{\"content\":\"{}\"}}}}]}}\n\ndata: {{\"id\":\"stream-fixture\",\"choices\":[],\"usage\":{{\"prompt_tokens\":11,\"completion_tokens\":7,\"total_tokens\":18}}}}\n\ndata: [DONE]\n\n",
            "x".repeat(70 * 1024)
        );
        if body.as_ref() != expected.as_bytes() {
            return TestResult::failed(
                start.elapsed(),
                format!(
                    "oversized SSE was not byte-preserved: expected {} bytes, got {}",
                    expected.len(),
                    body.len()
                ),
            );
        }
        TestResult::passed_with_message(
            start.elapsed(),
            format!("{} streaming bytes passed through", body.len()),
        )
    })
}

#[async_trait]
impl TestSuite for SingleProviderTestSuite {
    fn name(&self) -> &str {
        "EdgionAIService/SingleProvider"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionAIService/SingleProvider")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            TestCase::new(
                "h1_paced_compression_streaming",
                "H1 SSE remains paced and uncompressed",
                h1_paced,
            ),
            TestCase::new("h2_paced_streaming", "H2 SSE remains paced", h2_paced),
            TestCase::new(
                "canonical_alias_auth_prefix",
                "Canonical and alias models preserve credential replacement, prefix, and query behavior",
                canonical_alias_auth_prefix,
            ),
            TestCase::new(
                "unknown_and_streaming_policy",
                "Unknown-model reject/passthrough and streaming-disabled policy",
                rejection_modes,
            ),
            TestCase::new(
                "usage_injection_optout_and_split",
                "Usage option injection/opt-out and split SSE extraction",
                usage_injection_optout_and_split,
            ),
            TestCase::new(
                "exact_access_log_and_metric",
                "Exact durable access-log usage, configured cost, no-quota baseline, and optional metric deltas",
                exact_access_log_and_metric,
            ),
            TestCase::new(
                "token_quota_reconciliation_and_admission",
                "Streaming usage is reconciled exactly in Redis and exhausted admission returns an OpenAI 429",
                token_quota_reconciliation_and_admission,
            ),
            TestCase::new(
                "streaming_client_disconnect_finalizes_once",
                "A downstream disconnect cancels the provider stream and finalizes estimated usage exactly once",
                streaming_client_disconnect_finalizes_once,
            ),
            TestCase::new(
                "priority_retry_attempt_ledger",
                "A retryable AI 503 fails over to the next service with a bounded, credential-safe attempt ledger",
                priority_retry_attempt_ledger,
            ),
            TestCase::new(
                "token_quota_throttle_one_shot",
                "Throttle performs one bounded quota re-admission before returning the stable 429",
                token_quota_throttle_one_shot,
            ),
            TestCase::new(
                "token_quota_downgrade",
                "Premium exhaustion re-admits an explicit economy model and records the disclosed target",
                token_quota_downgrade,
            ),
            TestCase::new(
                "oversized_sse_passthrough",
                "An incomplete SSE event larger than 64 KiB remains byte-preserved",
                oversized_passthrough,
            ),
        ]
    }
}

#[async_trait]
impl TestSuite for AiServiceSecretRotationInitialTestSuite {
    fn name(&self) -> &str {
        "EdgionAIService/SingleProvider/SecretRotation/Initial"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionAIService/SingleProvider")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "credential_generation_a",
            "A fresh request uses the initial provider credential",
            credential_generation_a,
        )]
    }
}

#[async_trait]
impl TestSuite for AiServiceSecretRotationUpdatedTestSuite {
    fn name(&self) -> &str {
        "EdgionAIService/SingleProvider/SecretRotation/Updated"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionAIService/SingleProvider")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "credential_generation_b_and_admin_redaction",
            "A fresh request uses only the rotated credential and Admin output stays redacted",
            credential_generation_b_redacted,
        )]
    }
}
