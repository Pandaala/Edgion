//! Static end-to-end coverage for one OpenAI-compatible AI service.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::metrics_helper::MetricsClient;
use crate::transport::{RawH1Conn, RawH2Conn};
use async_trait::async_trait;
use bytes::Bytes;
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
const QUOTA_TERMINATE_HOST: &str = "ai-quota-terminate.example.com";
const QUOTA_TERMINATE_GUARD_HOST: &str = "ai-quota-terminate-guard.example.com";
const PATH: &str = "/v1/chat/completions";
const QUOTA_WINDOW_MS: u128 = 60 * 60 * 1_000;

/// The SSE event a terminated stream must carry immediately before the
/// gateway ends the response. Mirrors
/// `edgion_gateway::ai::usage::QUOTA_EXCEEDED_SSE_EVENT`, which is
/// `pub(crate)` inside the gateway crate and therefore not importable here.
/// Reuses the same message/type/code the 429 admission-time rejection
/// renders (`edgion_gateway::ai::error::AiProxyError::QuotaExceeded`).
const QUOTA_EXCEEDED_SSE_EVENT: &[u8] = b"event: error\ndata: {\"error\":{\"message\":\"The token quota has been exceeded.\",\"type\":\"rate_limit_error\",\"code\":\"token_quota_exceeded\"}}\n\n";

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

        let mut window_ids = vec![(before_ms / QUOTA_WINDOW_MS) as u64];
        let after_window = (after_ms / QUOTA_WINDOW_MS) as u64;
        if window_ids[0] != after_window {
            window_ids.push(after_window);
        }
        let mut inspected = None;
        for window_id in window_ids {
            let key = match quota_bucket_key(&identity, window_id) {
                Ok(key) => key,
                Err(error) => return TestResult::failed(start.elapsed(), error),
            };
            match quota_bucket_used(&ctx, &key).await {
                Ok(Some(used)) => {
                    inspected = Some((key, used));
                    break;
                }
                Ok(None) => {}
                Err(error) => return TestResult::failed(start.elapsed(), error),
            }
        }
        match inspected {
            Some((_, 18)) => {}
            Some((key, used)) => {
                return TestResult::failed(
                    start.elapsed(),
                    format!("quota bucket {key} expected exact final usage 18, got {used}"),
                );
            }
            None => {
                return TestResult::failed(
                    start.elapsed(),
                    "the expected Redis quota bucket was not found".to_string(),
                );
            }
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

// ===========================================================================
// Response-body pipeline verification (task B4): quota over-limit termination
// ===========================================================================
//
// These three cases exercise the real, end-to-end quota-termination path
// against the real `default/redis-test` Redis instance the harness already
// starts for this item — no processor stub, no faked accumulator state. The
// `ai-proxy-quota-terminate` plugin (09_EdgionPlugins) uses a deliberately
// tiny limit (2 tokens) so the real `RedisQuotaSink` worker genuinely flips
// `over_limit()` mid-stream through the same Redis round trip production
// uses; see `edgion_gateway::ai::quota::runtime::RedisQuotaSink` and
// `edgion_gateway::ai::usage::UsageExtractionProcessor::on_chunk`.
//
// The `quota_terminate` fixture (`test_server.rs::ai_stream_plan`) sends 5
// one-quota-unit content deltas 150ms apart — generous headroom over the
// plugin's 10ms flush interval — followed by a usage event and `[DONE]`
// tagged with a distinct `unreachable_marker` so a test can assert their
// absence without pinning the exact chunk the gateway terminates on.
//
// Chunks a/b/c are overwhelmingly likely, but not structurally guaranteed,
// to never trip termination themselves. Within one `on_chunk` call there is
// no `.await` between `observe_chunk` and the `over_limit()` check
// (`ai/usage.rs`), so that check can only reflect a worker flush that
// completed *before* the call began. But the worker always flushes whatever
// `observed_output` currently holds, not a value pinned to the notify that
// woke it — so, in principle, a first flush delayed past two whole 150ms
// gaps could already report a later cumulative than the chunk that
// triggered it. Given the 10ms flush interval and a local Redis, that would
// take three orders of magnitude of scheduling lag beyond normal, and every
// run observed so far (including the mutation tests below) landed
// termination on exactly the fourth chunk. A violation of this margin would
// surface as `check_quota_terminate_body`'s prefix assertion failing
// loudly, not as a silent pass — which is why the assertions below check a
// near-certain prefix, injected-event presence, and the tail's absence,
// never an exact mid-stream cut point.

/// Mirrors `test_server.rs::ai_stream_plan("quota_terminate", ..)` byte for
/// byte. Kept in sync by construction (both sides are hand-written against
/// the same fixture contract), not by sharing code across the
/// gateway-crate/test-harness boundary.
fn quota_terminate_delta(label: &str) -> Vec<u8> {
    format!("data: {{\"id\":\"quota-terminate-fixture\",\"choices\":[{{\"delta\":{{\"content\":\"{label}\"}}}}]}}\n\n")
        .into_bytes()
}

/// The three leading chunks that are overwhelmingly likely — but not
/// structurally guaranteed — to never be terminated on; see the
/// module-level comment for the exact margin and why a violation would
/// fail loudly instead of passing silently.
fn quota_terminate_guaranteed_prefix() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&quota_terminate_delta("chunk-a"));
    out.extend_from_slice(&quota_terminate_delta("chunk-b"));
    out.extend_from_slice(&quota_terminate_delta("chunk-c"));
    out
}

/// The complete, untruncated fixture body — what a client must receive if
/// (and only if) termination never fires, i.e. exactly the shape a
/// non-streaming request behind the same quota must still get in full.
fn quota_terminate_full_body() -> Vec<u8> {
    let mut out = quota_terminate_guaranteed_prefix();
    out.extend_from_slice(&quota_terminate_delta("chunk-d"));
    out.extend_from_slice(&quota_terminate_delta("chunk-e"));
    out.extend_from_slice(
        b"data: {\"id\":\"quota-terminate-fixture\",\"choices\":[],\"usage\":{\"prompt_tokens\":11,\
          \"completion_tokens\":7,\"total_tokens\":18},\"unreachable_marker\":\"quota-terminate-must-not-reach-client\"}\n\n",
    );
    out.extend_from_slice(b"data: [DONE]\n\n");
    out
}

/// Shared assertion for a terminated stream's body, independent of exactly
/// which chunk index the gateway cut on.
fn check_quota_terminate_body(body: &[u8]) -> Option<String> {
    let prefix = quota_terminate_guaranteed_prefix();
    if !body.starts_with(prefix.as_slice()) {
        return Some(format!(
            "expected the body to start with the fixture's first 3 chunks ({} bytes — near-certain to survive \
             termination given the timing margin, see the module comment); got {} bytes starting {:?}",
            prefix.len(),
            body.len(),
            String::from_utf8_lossy(&body[..body.len().min(200)])
        ));
    }
    if !body.ends_with(QUOTA_EXCEEDED_SSE_EVENT) {
        return Some(format!(
            "expected the response to end with the injected quota_exceeded SSE event; got tail {:?}",
            String::from_utf8_lossy(&body[body.len().saturating_sub(200)..])
        ));
    }
    if body.windows(19).any(|window| window == b"unreachable_marker\"") {
        return Some(
            "terminated body must never contain the tail's unreachable_marker — the usage/[DONE] tail leaked \
             through instead of being cut off"
                .to_string(),
        );
    }
    if body.windows(7).any(|window| window == b"[DONE]\n") {
        return Some(
            "terminated body must never reach the natural [DONE] end of stream — the gateway did not actually \
             cut the response short"
                .to_string(),
        );
    }
    None
}

/// Item 1+2 (H1): the pre-termination content, the injected SSE error event,
/// and a clean end of response — asserted on the *raw* wire (a hand-driven
/// H1 connection, not a client library that would hide `Transfer-Encoding`).
fn quota_termination_h1_framing(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let identity = format!("quota-terminate-h1-{}", uuid::Uuid::new_v4());
        let request_body = json!({"model":"fixture-model","stream":true,"messages":[]}).to_string();
        let content_length = request_body.len().to_string();

        let mut conn = match RawH1Conn::connect(&ctx.target_host, ctx.http_port).await {
            Ok(conn) => conn,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let head_result = conn
            .write_head(
                "POST",
                "/v1/chat/completions?mock_mode=quota_terminate",
                QUOTA_TERMINATE_HOST,
                &[
                    ("content-type", "application/json"),
                    ("content-length", content_length.as_str()),
                    ("x-quota-user", identity.as_str()),
                ],
            )
            .await;
        if let Err(error) = head_result {
            return TestResult::failed(start.elapsed(), error);
        }
        if let Err(error) = conn.write_raw(request_body.as_bytes()).await {
            return TestResult::failed(start.elapsed(), error);
        }

        let head = match conn.read_response_head(Duration::from_secs(5)).await {
            Ok(head) => head,
            Err(error) => return TestResult::failed(start.elapsed(), format!("no response head: {error}")),
        };
        if head.status != 200 {
            return TestResult::failed(
                start.elapsed(),
                format!("expected 200, got {} {}", head.status, head.reason),
            );
        }
        if !head.is_chunked() {
            return TestResult::failed(
                start.elapsed(),
                format!("H1 terminated AI stream must be Transfer-Encoding: chunked; got {head}"),
            );
        }
        // `header("content-length")` on the raw header text, not
        // `content_length()` — the latter parses the value and reads a
        // malformed `Content-Length: abc` as absent, which would pass here
        // for the wrong reason.
        if head.header("content-length").is_some() {
            return TestResult::failed(
                start.elapsed(),
                format!("H1 terminated AI stream must not carry Content-Length; got {head}"),
            );
        }

        let body = match conn.read_body(&head, Duration::from_secs(5)).await {
            Ok(body) => body,
            Err(error) => {
                return TestResult::failed(start.elapsed(), format!("chunked body did not end cleanly: {error}"));
            }
        };
        if let Some(error) = check_quota_terminate_body(&body) {
            return TestResult::failed(start.elapsed(), error);
        }
        TestResult::passed_with_message(
            start.elapsed(),
            format!(
                "H1 quota termination stayed chunked with no Content-Length and ended cleanly ({} body bytes)",
                body.len()
            ),
        )
    })
}

/// Item 1+2 (H2): the same termination, over a hand-driven HTTP/2 connection
/// so `content-length` absence and a clean `END_STREAM` (not a mid-body RST)
/// are asserted on the real frames rather than a parsed client view.
fn quota_termination_h2_framing(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let identity = format!("quota-terminate-h2-{}", uuid::Uuid::new_v4());
        let request_body = json!({"model":"fixture-model","stream":true,"messages":[]}).to_string();
        let content_length = request_body.len().to_string();

        let mut conn = match RawH2Conn::connect(&ctx.target_host, ctx.http_port).await {
            Ok(conn) => conn,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let uri = format!(
            "http://{}:{}/v1/chat/completions?mock_mode=quota_terminate",
            QUOTA_TERMINATE_HOST, ctx.http_port
        );
        let headers = [
            ("content-type", "application/json"),
            ("content-length", content_length.as_str()),
            ("x-quota-user", identity.as_str()),
        ];
        let (response, mut send) = match conn.send_request("POST", &uri, &headers, false) {
            Ok(pair) => pair,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if let Err(error) = send.send_data(Bytes::from(request_body.into_bytes()), true) {
            return TestResult::failed(start.elapsed(), format!("H2 request body write failed: {error}"));
        }

        let response = match tokio::time::timeout(Duration::from_secs(5), response).await {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => return TestResult::failed(start.elapsed(), format!("H2 response failed: {error}")),
            Err(_) => return TestResult::failed(start.elapsed(), "no H2 response within 5s".to_string()),
        };
        if response.status().as_u16() != 200 {
            return TestResult::failed(start.elapsed(), format!("expected 200, got {}", response.status()));
        }
        if response.headers().get("content-length").is_some() {
            return TestResult::failed(
                start.elapsed(),
                format!(
                    "H2 terminated AI stream must not carry content-length; got {:?}",
                    response.headers()
                ),
            );
        }

        let mut body_stream = response.into_body();
        let mut body = Vec::new();
        loop {
            match tokio::time::timeout(Duration::from_secs(5), body_stream.data()).await {
                Ok(Some(Ok(chunk))) => {
                    let _ = body_stream.flow_control().release_capacity(chunk.len());
                    body.extend_from_slice(&chunk);
                }
                Ok(Some(Err(error))) => {
                    return TestResult::failed(start.elapsed(), format!("H2 body stream ended abnormally: {error}"));
                }
                Ok(None) => break,
                Err(_) => return TestResult::failed(start.elapsed(), "H2 body read timed out".to_string()),
            }
        }
        conn.shutdown();

        if let Some(error) = check_quota_terminate_body(&body) {
            return TestResult::failed(start.elapsed(), error);
        }
        TestResult::passed_with_message(
            start.elapsed(),
            format!(
                "H2 quota termination carried no content-length and ended with a clean END_STREAM ({} body bytes)",
                body.len()
            ),
        )
    })
}

/// Item 3: a client that declares `"stream": false` must never be truncated
/// by the quota-termination path, even when the upstream answers with an
/// SSE body that genuinely goes over the same tiny limit. This drives the
/// real `requested_streaming` gate on `UsageExtractionProcessor::on_chunk`
/// (`edgion-gateway/src/ai/usage.rs`) — `requested_streaming` comes from the
/// client's own request, and `stream_kind` (SSE vs. not) comes from the
/// actual upstream response, so the only way to exercise the gate for real
/// is a provider that answers with SSE despite the client's non-streaming
/// request. `quota_terminate_force_sse` is exactly that fixture behavior.
///
/// The full-body check alone is not sufficient evidence: it would also pass
/// if the quota plugin were silently disabled or misconfigured for this
/// route (e.g. `tokenQuota` failing to compile, the identity never
/// resolving, `default/redis-test` becoming unreachable) — cases where
/// there is nothing at all for the gate to hold back. A same-identity
/// follow-up request supplies that missing positive control — see the
/// comment at its call site below for exactly what it does and does not
/// prove.
fn quota_termination_requires_streaming_request(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let identity = format!("quota-terminate-guard-{}", uuid::Uuid::new_v4());
        let request = match client(&ctx, QUOTA_TERMINATE_GUARD_HOST, false, false) {
            Ok(client) => client,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        }
        .post(request_url(
            &ctx,
            QUOTA_TERMINATE_GUARD_HOST,
            PATH,
            "?mock_mode=quota_terminate_force_sse",
        ))
        .header("x-quota-user", identity.as_str())
        .json(&json!({"model":"fixture-model","stream":false,"messages":[]}));
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), format!("AI request failed: {error}")),
        };
        if response.status() != reqwest::StatusCode::OK {
            return TestResult::failed(
                start.elapsed(),
                format!("non-streaming guard request expected 200, got {}", response.status()),
            );
        }
        if response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            != Some("text/event-stream")
        {
            return TestResult::failed(
                start.elapsed(),
                format!(
                    "fixture must answer with an SSE content-type despite stream:false (test setup check); got {:?}",
                    response.headers().get("content-type")
                ),
            );
        }
        let body = match response.bytes().await {
            Ok(body) => body,
            Err(error) => return TestResult::failed(start.elapsed(), format!("body read failed: {error}")),
        };
        let expected = quota_terminate_full_body();
        if body.as_ref() != expected.as_slice() {
            let contains_injected_event = body
                .windows(QUOTA_EXCEEDED_SSE_EVENT.len())
                .any(|window| window == QUOTA_EXCEEDED_SSE_EVENT);
            return TestResult::failed(
                start.elapsed(),
                format!(
                    "a non-streaming ('stream':false) request must never be truncated by quota termination, even \
                     though the upstream answered with an over-limit SSE stream — expected the full {}-byte fixture \
                     body, got {} bytes (injected quota_exceeded event present: {contains_injected_event})",
                    expected.len(),
                    body.len()
                ),
            );
        }

        // Positive control. The check above (full body, no truncation) can
        // pass for two entirely different reasons: because the
        // `requested_streaming` gate correctly held back a real over-limit
        // termination, or because quota enforcement was never engaged for
        // this identity/route at all (plugin misconfiguration, a failed
        // `tokenQuota` compile, an unreachable `default/redis-test`, ...) —
        // in which case there was nothing for the gate to hold back. A
        // second request reusing the *same* `x-quota-user` identity on
        // `QUOTA_TERMINATE_HOST` proves which one happened: the identity's
        // Redis bucket is shared across both hostnames (same
        // `ai-proxy-quota-terminate` plugin, so the same `entry_scope`), and
        // `tokens: 2` means a guard request that was genuinely admitted and
        // billed leaves the bucket over limit for every subsequent request
        // in the same window. If quota enforcement were disabled or
        // misconfigured, this request would be freely admitted (200)
        // instead of rejected.
        //
        // This does not, by itself, prove the *mid-stream* estimate that
        // feeds `over_limit()` during a long-running stream
        // (`approximate_openai_output_units`) is what pushed the bucket
        // over: production also reconciles the bucket from the stream's
        // authoritative reported "usage" tokens once the response finishes
        // (`pg_logging.rs::finalize_ai_quota`), and a well-formed SSE
        // response will normally supply those regardless of whether the
        // per-chunk estimate is healthy. That narrower claim — that
        // `over_limit()` genuinely flips true *during* a stream and
        // termination fires because of it — is what `quota_termination_h1_
        // framing`/`_h2_framing` verify instead, including via mutation
        // testing (see the B4 report). This control's job is narrower and
        // still real: it proves quota enforcement is live for this
        // identity, not merely that a body happened to arrive intact.
        let control = match client(&ctx, QUOTA_TERMINATE_HOST, false, false) {
            Ok(client) => client,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        }
        .post(request_url(&ctx, QUOTA_TERMINATE_HOST, PATH, ""))
        .header("x-quota-user", identity.as_str())
        .json(&json!({"model":"fixture-model","messages":[]}))
        .send()
        .await;
        let control = match control {
            Ok(response) => response,
            Err(error) => {
                return TestResult::failed(start.elapsed(), format!("positive-control request failed: {error}"))
            }
        };
        if control.status() != reqwest::StatusCode::TOO_MANY_REQUESTS {
            return TestResult::failed(
                start.elapsed(),
                format!(
                    "positive control failed: expected the guard request to have driven the shared identity's \
                     bucket over the 2-token limit, so a same-identity follow-up should be rejected with 429 — got \
                     {} instead, meaning the guard request may never have accrued real quota usage",
                    control.status()
                ),
            );
        }
        let control_body: Value = match control.json().await {
            Ok(body) => body,
            Err(error) => {
                return TestResult::failed(
                    start.elapsed(),
                    format!("positive-control 429 body was not valid JSON: {error}"),
                )
            }
        };
        if control_body["error"]["code"] != "token_quota_exceeded" {
            return TestResult::failed(
                start.elapsed(),
                format!("positive-control 429 returned the wrong OpenAI error envelope: {control_body}"),
            );
        }

        TestResult::passed_with_message(
            start.elapsed(),
            format!(
                "non-streaming request received the full untruncated {}-byte body despite an over-limit SSE \
                 upstream (requested_streaming gate held), and a same-identity follow-up confirmed the bucket was \
                 genuinely driven over its 2-token limit (429 token_quota_exceeded)",
                body.len()
            ),
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
            TestCase::new(
                "quota_termination_h1_framing",
                "H1: quota over-limit termination stays chunked with no Content-Length and ends cleanly",
                quota_termination_h1_framing,
            ),
            TestCase::new(
                "quota_termination_h2_framing",
                "H2: quota over-limit termination carries no content-length and ends with a clean END_STREAM",
                quota_termination_h2_framing,
            ),
            TestCase::new(
                "quota_termination_requires_streaming_request",
                "A non-streaming request is never truncated by quota termination, even over an over-limit SSE upstream",
                quota_termination_requires_streaming_request,
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
