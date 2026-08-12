//! Static end-to-end coverage for one OpenAI-compatible AI backend.

use super::gateway_admin::{admin_base, authed, backend_uid, CREDENTIAL_RESERVATION_KEY_PREFIX};
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::metrics_helper::MetricsClient;
use crate::transport::{RawH1Conn, RawH2Conn};
use async_trait::async_trait;
use bytes::Bytes;
use edgion_gateway::plugins::common::redis_rate_limiter::{
    build_credential_reservation_keys, build_cumulative_quota_keys,
};
use futures::StreamExt;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct SingleProviderTestSuite;
pub struct AiBackendSecretRotationInitialTestSuite;
pub struct AiBackendSecretRotationUpdatedTestSuite;

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
/// Routes to `fixture-provider-credential-tpm`, the only fixture service that
/// declares a per-credential `tokensPerMinute` limit and therefore the only
/// one whose attempts take out — and must settle — a credential reservation.
const CREDENTIAL_TPM_HOST: &str = "ai-credential-tpm.example.com";
const PATH: &str = "/v1/chat/completions";
const QUOTA_WINDOW_MS: u128 = 60 * 60 * 1_000;

/// The SSE event a terminated stream must carry immediately before the
/// gateway ends the response. Mirrors
/// `edgion_gateway::ai::usage::QUOTA_EXCEEDED_SSE_EVENT`, which is
/// `pub(crate)` inside the gateway crate and therefore not importable here.
/// Reuses the same message/type/code the 429 admission-time rejection
/// renders (`edgion_gateway::ai::error::AiProxyError::QuotaExceeded`).
///
/// Leads with a blank line, like its `QUOTA_UNENFORCEABLE_SSE_EVENT` sibling
/// below and for the same reason: the injection is appended after whatever
/// byte the upstream's chunk boundary fell on, so without its own boundary it
/// would continue a half-delivered event instead of starting a new one.
const QUOTA_EXCEEDED_SSE_EVENT: &[u8] = b"\n\nevent: error\ndata: {\"error\":{\"message\":\"The token quota has been exceeded.\",\"type\":\"rate_limit_error\",\"code\":\"token_quota_exceeded\"}}\n\n";

/// The SSE event carried by a stream cut short because quota could no longer
/// be evaluated (SSE decoding degraded or repeated malformed events). Mirrors
/// `edgion_gateway::ai::usage::QUOTA_UNENFORCEABLE_SSE_EVENT`, which is
/// `pub(crate)` and therefore not importable here. Reuses the same
/// message/type/code the admission path renders when enforcement cannot be
/// performed (`edgion_gateway::ai::error::AiProxyError::QuotaUnavailable`).
const QUOTA_UNENFORCEABLE_SSE_EVENT: &[u8] = b"\n\nevent: error\ndata: {\"error\":{\"message\":\"Token quota enforcement is temporarily unavailable.\",\"type\":\"api_error\",\"code\":\"token_quota_unavailable\"}}\n\n";

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

async fn post_with_raw_gzip_and_client_authorization(
    ctx: &TestContext,
    host: &str,
    body: Value,
    query: &str,
    h2: bool,
) -> Result<reqwest::Response, String> {
    post_inner(ctx, host, body, query, h2, None, true, true).await
}

async fn post_with_quota_identity(
    ctx: &TestContext,
    identity: &str,
    body: Value,
    trace_id: Option<&str>,
) -> Result<reqwest::Response, String> {
    post_with_quota_identity_on_host(ctx, QUOTA_HOST, identity, body, "", trace_id).await
}

async fn post_with_quota_identity_on_host(
    ctx: &TestContext,
    host: &str,
    identity: &str,
    body: Value,
    query: &str,
    trace_id: Option<&str>,
) -> Result<reqwest::Response, String> {
    let mut request = client(ctx, host, false, false)?
        .post(request_url(ctx, host, PATH, query))
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

fn quota_bucket_key_for_plugin(plugin: &[u8], identity: &str, window_id: u64) -> Result<String, String> {
    let mut quota_identity = Vec::new();
    append_quota_identity_part(&mut quota_identity, plugin);
    append_quota_identity_part(&mut quota_identity, identity.as_bytes());
    append_quota_identity_part(&mut quota_identity, &QUOTA_WINDOW_MS.to_be_bytes());
    append_quota_identity_part(&mut quota_identity, b"identity");
    build_cumulative_quota_keys("edgion:aiq:", &quota_identity, window_id, b"integration-inspection")
        .map(|keys| keys.bucket)
        .map_err(|error| format!("failed to derive Redis quota bucket key: {error}"))
}

fn quota_bucket_key(identity: &str, window_id: u64) -> Result<String, String> {
    quota_bucket_key_for_plugin(b"edgion-test/ai-proxy-quota/#0", identity, window_id)
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

async fn terminate_quota_bucket_used_between(
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
        let key = quota_bucket_key_for_plugin(b"edgion-test/ai-proxy-quota-terminate/#0", identity, window_id)?;
        if let Some(used) = quota_bucket_used(ctx, &key).await? {
            return Ok((key, used));
        }
    }
    Err("the expected terminate-route Redis quota bucket was not found".to_string())
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
            "{}/configclient/edgionbackend/list",
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
            .pointer("/spec/ai/credentialPool/credentials")
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
    let response = match post_with_raw_gzip_and_client_authorization(
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
            "backend",
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
            || first["backend"] != "edgion-test/fixture-provider-retryable"
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
            || second["backend"] != "edgion-test/fixture-provider"
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
            "",
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
            "",
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
            "",
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
            "",
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

        let exhausted = match post_with_quota_identity_on_host(
            &ctx,
            DOWNGRADE_HOST,
            &identity,
            json!({"model":"premium-model","messages":[]}),
            "",
            None,
        )
        .await
        {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if exhausted.status() != reqwest::StatusCode::TOO_MANY_REQUESTS {
            return TestResult::failed(
                start.elapsed(),
                format!("exhausted downgrade target expected 429, got {}", exhausted.status()),
            );
        }
        if exhausted.headers().contains_key("x-edgion-ai-model-downgraded") {
            return TestResult::failed(
                start.elapsed(),
                "an exhausted downgrade target must not trigger another downgrade".to_string(),
            );
        }
        let exhausted_body: Value = match exhausted.json().await {
            Ok(body) => body,
            Err(error) => {
                return TestResult::failed(start.elapsed(), format!("exhausted downgrade JSON failed: {error}"));
            }
        };
        if exhausted_body["error"]["code"] != "token_quota_exceeded" {
            return TestResult::failed(
                start.elapsed(),
                format!("unexpected exhausted downgrade envelope: {exhausted_body}"),
            );
        }
        TestResult::passed_with_message(
            start.elapsed(),
            "Downgrade re-admitted economy-model once, then returned 429 after both model buckets were exhausted"
                .to_string(),
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
            // Must mirror `test_server::oversized_event_payload_bytes`, which
            // is bound to the gateway's incomplete-event cap.
            "x".repeat(edgion_gateway::sse::DEFAULT_MAX_PENDING_EVENT_BYTES + 4096)
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

/// Asserts the injected error survives as a *dispatchable* event in the
/// client's own view of the stream, not merely as a byte sequence present
/// somewhere in the body.
///
/// Byte containment is not the property that matters. The gateway appends the
/// injection after whatever byte the upstream's chunk boundary fell on, so if
/// that boundary lands inside an SSE event the injected bytes continue the
/// half-delivered event instead of starting a new one: still present verbatim,
/// still framed legally, but parsed as one event whose data is truncated JSON
/// glued to the error JSON — which every conforming client drops. The stream
/// then reads exactly like an upstream hang-up, and the injected event is the
/// client's only way to tell the two apart. So this decodes the received bytes
/// with the same WHATWG-conformant decoder a client would use and asserts the
/// error arrives as its own event with parseable JSON.
fn check_injected_event_is_dispatchable(body: &[u8]) -> Option<String> {
    let mut decoder = edgion_gateway::sse::SseDecoder::with_default_limit();
    let mut events = Vec::new();
    decoder.feed(body, &mut events);

    let Some(last) = events.last() else {
        return Some("the terminated body dispatched no SSE events at all".to_string());
    };
    if last.event.as_deref() != Some("error") {
        return Some(format!(
            "the injected quota event was swallowed by a partial upstream event: the client's last dispatched \
             event is {:?}, not `error` ({} events total)",
            last.event,
            events.len()
        ));
    }
    match serde_json::from_str::<Value>(&last.data) {
        Ok(value) if value["error"]["code"] == "token_quota_exceeded" => None,
        Ok(value) => Some(format!(
            "the injected error event carried an unexpected payload: {value}"
        )),
        Err(error) => Some(format!(
            "the injected error event did not parse as JSON client-side ({error}) — its data was concatenated \
             onto a half-delivered upstream event: {:?}",
            last.data
        )),
    }
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
    // Parsed view first: both checks fail on a swallowed injection, but only
    // this one names the reason. The byte-level `ends_with` below would
    // otherwise report a generic "wrong tail" for the exact defect the parsed
    // check exists to describe.
    if let Some(error) = check_injected_event_is_dispatchable(body) {
        return Some(error);
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

fn check_malformed_quota_body(body: &[u8]) -> Option<String> {
    if !body
        .windows(b"data: \xff\n\n".len())
        .any(|window| window == b"data: \xff\n\n")
    {
        return Some("the first malformed SSE event did not reach the client".to_string());
    }
    if !body
        .windows(b"data: \xfe\n\n".len())
        .any(|window| window == b"data: \xfe\n\n")
    {
        return Some("the terminating callback did not forward the second malformed event".to_string());
    }
    if body
        .windows(b"quota-malformed-must-not-reach-client".len())
        .any(|window| window == b"quota-malformed-must-not-reach-client")
    {
        return Some("the fixture tail reached the client after malformed-event termination".to_string());
    }
    if body.windows(b"[DONE]".len()).any(|window| window == b"[DONE]") {
        return Some("the malformed stream reached its natural [DONE] boundary".to_string());
    }

    let mut decoder = edgion_gateway::sse::SseDecoder::with_default_limit();
    let mut events = Vec::new();
    decoder.feed(body, &mut events);
    if decoder.malformed_events() != 2 {
        return Some(format!(
            "the client observed {} completed malformed events instead of 2",
            decoder.malformed_events()
        ));
    }
    let valid_middle_seen = events.iter().any(|event| {
        serde_json::from_str::<Value>(&event.data)
            .is_ok_and(|value| value["choices"][0]["delta"]["content"] == "valid-middle")
    });
    if !valid_middle_seen {
        return Some(
            "the valid middle event was not dispatched, so the first malformed event may have terminated early"
                .to_string(),
        );
    }
    if !events
        .iter()
        .any(|event| event.event.is_none() && event.data == "quota-malformed-boundary-probe")
    {
        return Some("the injected event did not independently close the upstream partial SSE fragment".to_string());
    }
    let error_events = events
        .iter()
        .filter(|event| event.event.as_deref() == Some("error"))
        .filter_map(|event| serde_json::from_str::<Value>(&event.data).ok())
        .filter(|value| value["error"]["code"] == "token_quota_unavailable")
        .count();
    if error_events != 1 {
        return Some(format!(
            "expected exactly one dispatchable token_quota_unavailable event, got {error_events}"
        ));
    }
    if !body.ends_with(QUOTA_UNENFORCEABLE_SSE_EVENT) {
        return Some("the client-visible error was not the final SSE event".to_string());
    }
    None
}

#[cfg(test)]
mod malformed_quota_body_tests {
    use super::{check_malformed_quota_body, QUOTA_UNENFORCEABLE_SSE_EVENT};

    const UPSTREAM_PREFIX: &[u8] = b"data: \xff\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"valid-middle\"}}]}\n\ndata: \xfe\n\ndata: quota-malformed-boundary-probe";

    #[test]
    fn injected_error_requires_its_own_sse_boundary() {
        let mut correctly_bounded = UPSTREAM_PREFIX.to_vec();
        correctly_bounded.extend_from_slice(QUOTA_UNENFORCEABLE_SSE_EVENT);
        assert_eq!(check_malformed_quota_body(&correctly_bounded), None);

        let mut missing_boundary = UPSTREAM_PREFIX.to_vec();
        missing_boundary.extend_from_slice(&QUOTA_UNENFORCEABLE_SSE_EVENT[2..]);
        assert!(
            check_malformed_quota_body(&missing_boundary).is_some_and(|error| error.contains("independently close"))
        );
    }
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

/// D3 regression: the injected quota error event must survive an upstream
/// that ends its chunks mid-event.
///
/// Same setup as `quota_termination_h1_framing`, except the provider writes
/// one byte per write (`mock_mode=quota_terminate_byte_at_a_time`). That is
/// simply the worst case of a freedom every provider has — where a chunk ends
/// is not something the gateway controls — and ordinary TCP segmentation
/// reaches it intermittently. Because the quota flush is asynchronous, several
/// more bytes have already arrived by the time `over_limit()` is observed, so
/// the cut lands inside an SSE event rather than on a boundary.
///
/// The injection is appended after the current chunk's bytes, so without its
/// own leading boundary it would continue that half-delivered event: present
/// on the wire, framed legally, and dropped by every conforming parser. The
/// assertion is therefore on the client's parsed view
/// (`check_injected_event_is_dispatchable`, reached through
/// `check_quota_terminate_body`), not on raw byte containment.
///
/// Byte-at-a-time delivery also maximizes filter invocations and per-batch
/// boundaries, so this exercises the response-body sink's per-batch lifecycle
/// far harder than any whole-event fixture. If the boundary fix holds but this
/// case still fails intermittently, that is a different and more serious bug —
/// report it rather than relaxing the assertion.
fn byte_at_a_time_sse_delivery_still_terminates(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let identity = format!("quota-terminate-bytewise-{}", uuid::Uuid::new_v4());
        let request_body = json!({"model":"fixture-model","stream":true,"messages":[]}).to_string();
        let content_length = request_body.len().to_string();

        let mut conn = match RawH1Conn::connect(&ctx.target_host, ctx.http_port).await {
            Ok(conn) => conn,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let head_result = conn
            .write_head(
                "POST",
                "/v1/chat/completions?mock_mode=quota_terminate_byte_at_a_time",
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

        // Generous relative to the ~600ms this fixture needs to reach the cut
        // at 2ms per byte, while still failing loudly if the stream runs to
        // its natural end instead of being terminated.
        let body = match conn.read_body(&head, Duration::from_secs(10)).await {
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
                "byte-at-a-time upstream delivery still produced a dispatchable injected quota event \
                 ({} body bytes)",
                body.len()
            ),
        )
    })
}

/// D1 regression: one oversized SSE event must not silently switch quota
/// enforcement off for the rest of the stream.
///
/// `quota.add_output_units` is only reachable from inside `feed_each`'s
/// visitor, and a degraded decoder returns from `feed_each` immediately for
/// every later chunk. So once the decoder gives up, nothing grows the
/// accumulator, no flush runs, `over_limit()` can never flip, and the
/// over-limit termination path is dead for this request — the upstream would
/// bill freely to the request timeout. Enforcement therefore fails closed
/// here instead.
///
/// Reuses `mock_mode=oversized`, whose first chunk is one unterminated event
/// past the decoder's cap (sized from the gateway constant) followed by a
/// usage event and `[DONE]`. Those two trailing chunks are what must never
/// reach the client. Driven over `RawH1Conn` so the cut is asserted on the
/// wire rather than through a client library's parsed view.
fn oversized_sse_event_must_not_disable_mid_stream_quota_enforcement(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let identity = format!("quota-degraded-h1-{}", uuid::Uuid::new_v4());
        let request_body = json!({"model":"fixture-model","stream":true,"messages":[]}).to_string();
        let content_length = request_body.len().to_string();

        let mut conn = match RawH1Conn::connect(&ctx.target_host, ctx.http_port).await {
            Ok(conn) => conn,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let head_result = conn
            .write_head(
                "POST",
                "/v1/chat/completions?mock_mode=oversized",
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

        // Bounded well under the fixture's full run so "the stream completed
        // normally" fails loudly instead of hanging.
        let body = match conn.read_body(&head, Duration::from_secs(5)).await {
            Ok(body) => body,
            Err(error) => {
                return TestResult::failed(start.elapsed(), format!("chunked body did not end cleanly: {error}"));
            }
        };

        if !body.ends_with(QUOTA_UNENFORCEABLE_SSE_EVENT) {
            return TestResult::failed(
                start.elapsed(),
                format!(
                    "expected the response to end with the injected token_quota_unavailable SSE event; got tail {:?}",
                    String::from_utf8_lossy(&body[body.len().saturating_sub(200)..])
                ),
            );
        }
        if body.windows(7).any(|window| window == b"[DONE]\n") {
            return TestResult::failed(
                start.elapsed(),
                "a stream whose quota can no longer be evaluated must never reach the natural [DONE] end of \
                 stream — enforcement was silently switched off by the oversized event"
                    .to_string(),
            );
        }
        if body.windows(9).any(|window| window == b"\"usage\":{") {
            return TestResult::failed(
                start.elapsed(),
                "the fixture's trailing usage event leaked through instead of being cut off".to_string(),
            );
        }
        TestResult::passed_with_message(
            start.elapsed(),
            format!(
                "an oversized SSE event failed quota enforcement closed instead of disabling it ({} body bytes)",
                body.len()
            ),
        )
    })
}

/// D1b regression: malformed-event tolerance is cumulative for one response.
///
/// The real provider fixture emits malformed, valid, malformed, an incomplete
/// boundary probe in the same transport chunk, then a delayed unreachable
/// tail. The valid middle event proves the first malformed event was tolerated
/// and also prevents a consecutive-only implementation from passing. The
/// incomplete probe makes the client-side parser reject an injected error that
/// lacks its own leading SSE boundary. The request uses the existing
/// Redis-backed Reject quota route on the credential-limited provider. The
/// bucket inspections below prove both that the production token-quota path
/// was installed and that gateway termination reconciled the credential TPM
/// reservation to the small amount actually consumed.
fn repeated_malformed_sse_events_fail_quota_enforcement_closed(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();

        // Freeze one credential-reservation window around the request. A
        // rollover would make the before/after Redis delta meaningless. The
        // fixture completes in about two seconds, so an eight-second margin
        // keeps the whole exchange and the access-log settlement in one
        // window.
        let (_, elapsed) = credential_window_now();
        if elapsed > CREDENTIAL_RESERVATION_WINDOW_SECS - 8 {
            tokio::time::sleep(Duration::from_secs(CREDENTIAL_RESERVATION_WINDOW_SECS - elapsed + 1)).await;
        }
        let (credential_window, _) = credential_window_now();
        let provider_uid = match backend_uid(&ctx, "edgion-test", "fixture-provider-credential-tpm").await {
            Ok(uid) => uid,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let credential_tpm_before = match credential_tpm_used(&ctx, &provider_uid, "primary", credential_window).await {
            Ok(value) => value.unwrap_or(0),
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };

        let response_start = Instant::now();
        let identity = format!("quota-malformed-h1-{}", uuid::Uuid::new_v4());
        let trace = format!("ai-quota-malformed-h1-{}", uuid::Uuid::new_v4());
        let request_body = json!({"model":"fixture-model","stream":true,"messages":[]}).to_string();
        let content_length = request_body.len().to_string();
        let before_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();

        let mut conn = match RawH1Conn::connect(&ctx.target_host, ctx.http_port).await {
            Ok(conn) => conn,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if let Err(error) = conn
            .write_head(
                "POST",
                "/v1/chat/completions?mock_mode=quota_malformed",
                CREDENTIAL_TPM_HOST,
                &[
                    ("content-type", "application/json"),
                    ("content-length", content_length.as_str()),
                    ("x-quota-user", identity.as_str()),
                    ("x-trace-id", trace.as_str()),
                    ("access_log", "test_store"),
                ],
            )
            .await
        {
            return TestResult::failed(start.elapsed(), error);
        }
        if let Err(error) = conn.write_raw(request_body.as_bytes()).await {
            return TestResult::failed(start.elapsed(), error);
        }

        let head = match conn.read_response_head(Duration::from_secs(5)).await {
            Ok(head) => head,
            Err(error) => return TestResult::failed(start.elapsed(), format!("no response head: {error}")),
        };
        if head.status != 200 || !head.is_chunked() || head.header("content-length").is_some() {
            return TestResult::failed(
                start.elapsed(),
                format!("malformed quota termination expected chunked 200 without Content-Length; got {head}"),
            );
        }

        let body = match conn.read_body(&head, Duration::from_secs(5)).await {
            Ok(body) => body,
            Err(error) => {
                return TestResult::failed(start.elapsed(), format!("chunked body did not end cleanly: {error}"));
            }
        };
        let body_finished = response_start.elapsed();
        if body_finished >= Duration::from_millis(3500) {
            return TestResult::failed(
                body_finished,
                "the response did not finish before the fixture's four-second natural completion deadline".to_string(),
            );
        }
        if let Some(error) = check_malformed_quota_body(&body) {
            return TestResult::failed(start.elapsed(), error);
        }

        let after_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        if let Err(error) = terminate_quota_bucket_used_between(&ctx, &identity, before_ms, after_ms).await {
            return TestResult::failed(
                start.elapsed(),
                format!("the real Redis-backed quota path was not exercised: {error}"),
            );
        }

        let entry = match ctx.access_log_client().get_access_log_with_retry(&trace, 15, 200).await {
            Ok(entry) => entry,
            Err(error) => {
                return TestResult::failed(start.elapsed(), format!("malformed quota access log missing: {error}"));
            }
        };
        let errors = entry.data["errors"].as_array().cloned().unwrap_or_default();
        let has = |status: &str| errors.iter().any(|value| value == status);
        if !has("AiSseDecodeMalformed") || !has("AiSseMalformedQuotaTerminated") {
            return TestResult::failed(
                start.elapsed(),
                format!("malformed quota statuses were incomplete: {errors:?}"),
            );
        }
        if has("AiQuotaOverLimitTerminated") || has("AiSseDegradedQuotaTerminated") {
            return TestResult::failed(
                start.elapsed(),
                format!("malformed termination was assigned the wrong cause: {errors:?}"),
            );
        }
        let attempts = entry.data["genai"]["attempts"].as_array().cloned().unwrap_or_default();
        if attempts.is_empty()
            || attempts
                .iter()
                .any(|attempt| attempt["failure"] == "incomplete_response")
        {
            return TestResult::failed(
                start.elapsed(),
                format!(
                    "gateway termination must use the settled reservation path, not upstream truncation: {attempts:?}"
                ),
            );
        }

        if credential_window_now().0 != credential_window {
            return TestResult::failed(
                start.elapsed(),
                "the credential reservation window rolled over before settlement could be inspected — rerun the case"
                    .to_string(),
            );
        }
        let credential_tpm_after = match credential_tpm_used(&ctx, &provider_uid, "primary", credential_window).await {
            Ok(Some(used)) => used,
            Ok(None) => {
                return TestResult::failed(
                    start.elapsed(),
                    "no credential TPM bucket exists after the malformed stream — the credential reservation path was not exercised"
                        .to_string(),
                );
            }
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let Some(settled_delta) = credential_tpm_after.checked_sub(credential_tpm_before) else {
            return TestResult::failed(
                start.elapsed(),
                format!(
                    "the credential TPM counter moved backwards from {credential_tpm_before} to {credential_tpm_after}; the settlement assertion is not reliable"
                ),
            );
        };
        if settled_delta == 0 {
            return TestResult::failed(
                start.elapsed(),
                "the credential TPM bucket did not accrue any usage for the malformed stream — a pre-existing bucket alone does not prove that this attempt reserved and settled credential capacity"
                    .to_string(),
            );
        }
        if settled_delta >= CREDENTIAL_TPM_MAX_OUTPUT_TOKENS {
            return TestResult::failed(
                start.elapsed(),
                format!(
                    "the malformed-stream attempt added {settled_delta} tokens to the credential TPM bucket: it retained the full {CREDENTIAL_TPM_MAX_OUTPUT_TOKENS}-token output reservation instead of reconciling gateway termination"
                ),
            );
        }

        TestResult::passed_with_message(
            start.elapsed(),
            format!(
                "the second cumulative malformed event failed Redis-backed Reject enforcement closed in {:?} and settled its credential TPM reservation to {settled_delta} tokens",
                body_finished,
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

/// One credential-reservation window, in seconds. Mirrors the gateway's
/// private `attempt_runtime::WINDOW`.
const CREDENTIAL_RESERVATION_WINDOW_SECS: u64 = 60;
/// `defaults.maxOutputTokens` of `fixture-provider-credential-tpm`, i.e. the
/// output half of every reservation its attempts take out.
const CREDENTIAL_TPM_MAX_OUTPUT_TOKENS: u64 = 4096;

fn credential_window_now() -> (u64, u64) {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    (
        seconds / CREDENTIAL_RESERVATION_WINDOW_SECS,
        seconds % CREDENTIAL_RESERVATION_WINDOW_SECS,
    )
}

/// Reads the credential's `tpm` counter for `window`, the window frozen before
/// the request that took the reservation out. `None` means the bucket does not
/// exist — which is itself a finding here, so the caller must not treat it as
/// "nothing reserved yet": a reconciled bucket keeps its `tpm` field at a small
/// value, it is never deleted.
async fn credential_tpm_used(ctx: &TestContext, uid: &str, alias: &str, window: u64) -> Result<Option<u64>, String> {
    let keys = build_credential_reservation_keys(
        CREDENTIAL_RESERVATION_KEY_PREFIX,
        b"edgion-test",
        uid.as_bytes(),
        alias.as_bytes(),
        window,
        uuid::Uuid::new_v4().to_string().as_bytes(),
    )
    .map_err(|error| format!("failed to derive the credential reservation bucket: {error}"))?;
    let encoded = utf8_percent_encode(&keys.bucket, NON_ALPHANUMERIC);
    let response = authed(ctx.http_client.get(format!(
        "{}/api/v1/testing/link-sys/redis/default_redis-test/hgetall/{encoded}",
        admin_base(ctx)
    )))
    .send()
    .await
    .map_err(|error| format!("credential bucket inspection failed: {error}"))?;
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .map_err(|error| format!("invalid credential bucket response: {error}"))?;
    if !status.is_success() || body["success"] != true {
        return Err(format!("credential bucket inspection returned {status}: {body}"));
    }
    Ok(body["data"]["fields"]["tpm"]
        .as_str()
        .and_then(|value| value.parse::<u64>().ok()))
}

/// D2/D4 regression: a stream the gateway itself terminated must settle its
/// credential reservation and must not be booked as an upstream truncation.
///
/// Terminating stops the response immediately, so the `end_of_stream = true`
/// task never reaches the response-body hook — by design. Settlement used to
/// read that missing eos as "the upstream truncated", which has two visible
/// consequences, both asserted here:
///
///  - the attempt kept its whole `estimated input + maxOutputTokens`
///    reservation charged against the credential for the rest of the
///    60-second window, even though the gateway had just dropped that
///    upstream connection itself. Asserted twice, deliberately: directly, by
///    reading the credential's `tpm` counter back out of Redis (it must be a
///    reconciled handful of tokens, not the 4096-token estimate), and then
///    behaviorally, because `fixture-provider-credential-tpm` declares
///    `tokensPerMinute: 6000` against `maxOutputTokens: 4096` so exactly one
///    outstanding reservation fits — the second request below, a different
///    quota identity and therefore an unrelated tenant sharing the
///    credential, is admitted only if the first reservation was settled. The
///    direct read is what keeps this honest: without it the whole scenario
///    would still pass green if credential limits silently stopped reaching
///    the gateway, since then no bucket would be charged at all.
///  - the access log stamped `genai.attempts[].failure = "incomplete_response"`
///    on a perfectly healthy provider, contradicting the
///    `AiQuotaOverLimitTerminated` status carried by the very same log line.
fn quota_terminated_stream_records_its_status_and_settles_its_bucket(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();

        // Everything below must land in one reservation window. A rollover
        // would hand the neighbour request a fresh credential bucket and let
        // this case pass no matter how the first request settled, and would
        // point the direct read at a bucket that never existed. The scenario
        // runs in about two seconds, so waiting out the tail of a window
        // removes almost all of that risk; the frozen `window` below turns the
        // residue into an explicit skip-style failure rather than a false pass.
        let (_, elapsed) = credential_window_now();
        if elapsed > CREDENTIAL_RESERVATION_WINDOW_SECS - 8 {
            tokio::time::sleep(Duration::from_secs(CREDENTIAL_RESERVATION_WINDOW_SECS - elapsed + 1)).await;
        }
        let (window, _) = credential_window_now();

        let uid = match backend_uid(&ctx, "edgion-test", "fixture-provider-credential-tpm").await {
            Ok(uid) => uid,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };

        let identity = format!("quota-terminate-tpm-{}", uuid::Uuid::new_v4());
        let trace = format!("ai-quota-terminate-tpm-{}", uuid::Uuid::new_v4());
        let response = match post_with_quota_identity_on_host(
            &ctx,
            CREDENTIAL_TPM_HOST,
            &identity,
            json!({"model":"fixture-model","stream":true,"messages":[]}),
            "?mock_mode=quota_terminate",
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
                format!("terminated stream expected 200, got {}", response.status()),
            );
        }
        let frames = match collect_sse(response).await {
            Ok((frames, _)) => frames,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let contains = |frame: &[u8], needle: &[u8]| frame.windows(needle.len()).any(|window| window == needle);
        if !frames.iter().any(|frame| contains(frame, b"token_quota_exceeded"))
            || frames
                .iter()
                .any(|frame| contains(frame, b"quota-terminate-must-not-reach-client"))
        {
            return TestResult::failed(
                start.elapsed(),
                format!(
                    "the stream was not terminated over quota: {} frames, expected the injected quota_exceeded \
                     event and none of the fixture's post-termination tail",
                    frames.len()
                ),
            );
        }

        let entry = match ctx.access_log_client().get_access_log_with_retry(&trace, 15, 200).await {
            Ok(entry) => entry,
            Err(error) => {
                return TestResult::failed(
                    start.elapsed(),
                    format!("terminated stream access log missing: {error}"),
                )
            }
        };
        let errors = entry.data["errors"].as_array().cloned().unwrap_or_default();
        let has = |status: &str| errors.iter().any(|value| value == status);
        if !has("AiQuotaOverLimitTerminated") {
            return TestResult::failed(
                start.elapsed(),
                format!("the deliberate cut must be recorded as AiQuotaOverLimitTerminated; got errors {errors:?}"),
            );
        }
        if has("AiQuotaErrorEventDropped") {
            return TestResult::failed(
                start.elapsed(),
                format!("the injected diagnostic event must have fit the emit budget; got errors {errors:?}"),
            );
        }
        let attempts = entry.data["genai"]["attempts"].as_array().cloned().unwrap_or_default();
        if attempts.is_empty() {
            return TestResult::failed(
                start.elapsed(),
                format!("the terminated attempt must still be logged: {}", entry.data["genai"]),
            );
        }
        if attempts
            .iter()
            .any(|attempt| attempt["failure"] == "incomplete_response")
        {
            return TestResult::failed(
                start.elapsed(),
                format!(
                    "a gateway-initiated cut must not be booked as an upstream truncation — that failure class is \
                     what an operator alerts on for a genuinely broken provider: {attempts:?}"
                ),
            );
        }

        // The assertion that pins the defect itself. Settlement runs inline in
        // the logging phase (`pg_logging.rs` finalizes the attempt runtime
        // before the access-log entry is assembled), so by the time the entry
        // above was readable this counter is already final.
        match credential_tpm_used(&ctx, &uid, "primary", window).await {
            Ok(Some(used)) if used < CREDENTIAL_TPM_MAX_OUTPUT_TOKENS => {}
            Ok(Some(used)) => {
                return TestResult::failed(
                    start.elapsed(),
                    format!(
                        "the credential's per-minute token counter still holds {used} tokens: the terminated \
                         attempt retained its full {CREDENTIAL_TPM_MAX_OUTPUT_TOKENS}-token output reservation \
                         instead of reconciling it to what the stream actually consumed"
                    ),
                );
            }
            Ok(None) if credential_window_now().0 != window => {
                return TestResult::failed(
                    start.elapsed(),
                    "the reservation window rolled over mid-scenario, so neither the direct read nor the neighbour \
                     admission below would mean anything — rerun the case"
                        .to_string(),
                );
            }
            Ok(None) => {
                return TestResult::failed(
                    start.elapsed(),
                    "no credential reservation bucket exists for edgion-test/fixture-provider-credential-tpm — the \
                     per-credential TPM limit never reached the data plane, so this scenario proves nothing"
                        .to_string(),
                );
            }
            Err(error) => return TestResult::failed(start.elapsed(), error),
        }

        // The unrelated tenant. Same credential, same window, fresh quota
        // identity so the route's own 2-token user limit cannot reject it at
        // admission before the credential reservation is even consulted.
        let other_identity = format!("quota-terminate-tpm-other-{}", uuid::Uuid::new_v4());
        let neighbour = match post_with_quota_identity_on_host(
            &ctx,
            CREDENTIAL_TPM_HOST,
            &other_identity,
            json!({"model":"fixture-model","stream":true,"messages":[]}),
            "?mock_mode=quota_terminate",
            None,
        )
        .await
        {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if neighbour.status() != reqwest::StatusCode::OK {
            return TestResult::failed(
                start.elapsed(),
                format!(
                    "a second tenant sharing the credential was refused with {} — the terminated stream retained its \
                     full {}-token reservation instead of reconciling it, leaking the credential's per-minute budget \
                     for the rest of the window",
                    neighbour.status(),
                    CREDENTIAL_TPM_MAX_OUTPUT_TOKENS
                ),
            );
        }
        if let Err(error) = collect_sse(neighbour).await {
            return TestResult::failed(start.elapsed(), error);
        }

        TestResult::passed_with_message(
            start.elapsed(),
            "a quota-terminated stream recorded AiQuotaOverLimitTerminated without an incomplete_response failure, \
             and reconciled its credential reservation so an unrelated tenant kept its admission"
                .to_string(),
        )
    })
}

#[async_trait]
impl TestSuite for SingleProviderTestSuite {
    fn name(&self) -> &str {
        "EdgionBackend/SingleProvider"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionBackend/SingleProvider")
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
                "A retryable AI 503 fails over to the next backend with a bounded, credential-safe attempt ledger",
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
                "An incomplete SSE event past the decoder's cap remains byte-preserved",
                oversized_passthrough,
            ),
            TestCase::new(
                "oversized_sse_event_must_not_disable_mid_stream_quota_enforcement",
                "An oversized SSE event fails quota enforcement closed instead of silently disabling it",
                oversized_sse_event_must_not_disable_mid_stream_quota_enforcement,
            ),
            TestCase::new(
                "repeated_malformed_sse_events_fail_quota_enforcement_closed",
                "The second cumulative malformed SSE event fails Redis-backed Reject enforcement closed",
                repeated_malformed_sse_events_fail_quota_enforcement_closed,
            ),
            TestCase::new(
                "quota_termination_h1_framing",
                "H1: quota over-limit termination stays chunked with no Content-Length and ends cleanly",
                quota_termination_h1_framing,
            ),
            TestCase::new(
                "byte_at_a_time_sse_delivery_still_terminates",
                "An upstream ending its chunks mid-event still yields a dispatchable injected quota error event",
                byte_at_a_time_sse_delivery_still_terminates,
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
            TestCase::new(
                "quota_terminated_stream_records_its_status_and_settles_its_bucket",
                "A quota-terminated stream settles its credential reservation and is not logged as an upstream truncation",
                quota_terminated_stream_records_its_status_and_settles_its_bucket,
            ),
        ]
    }
}

#[async_trait]
impl TestSuite for AiBackendSecretRotationInitialTestSuite {
    fn name(&self) -> &str {
        "EdgionBackend/SingleProvider/SecretRotation/Initial"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionBackend/SingleProvider")
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
impl TestSuite for AiBackendSecretRotationUpdatedTestSuite {
    fn name(&self) -> &str {
        "EdgionBackend/SingleProvider/SecretRotation/Updated"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionBackend/SingleProvider")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "credential_generation_b_and_admin_redaction",
            "A fresh request uses only the rotated credential and Admin output stays redacted",
            credential_generation_b_redacted,
        )]
    }
}
