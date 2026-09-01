//! AI retry and committed-stream resilience integration coverage.

use crate::framework::{test_backend_hosts, TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

pub struct ResilienceTestSuite;

const RATE_LIMIT_HOST: &str = "ai-resilience-429.example.com";
const SSE_HOST: &str = "ai-resilience-sse.example.com";
const HALF_OPEN_HOST: &str = "ai-resilience-half-open.example.com";
const PATH: &str = "/v1/chat/completions";
const SAFE_ATTEMPT_FIELDS: [&str; 12] = [
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

fn client(ctx: &TestContext, host: &str) -> Result<reqwest::Client, String> {
    let target_ip = ctx
        .target_host
        .parse::<IpAddr>()
        .ok()
        .or_else(|| {
            (ctx.target_host.as_str(), 0)
                .to_socket_addrs()
                .ok()?
                .next()
                .map(|address| address.ip())
        })
        .ok_or_else(|| format!("failed to resolve test target {}", ctx.target_host))?;
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .no_proxy()
        .http1_only()
        .resolve(host, SocketAddr::new(target_ip, ctx.http_port))
        .build()
        .map_err(|error| format!("failed to build resilience client: {error}"))
}

async fn post(ctx: &TestContext, host: &str, trace: &str, streaming: bool) -> Result<reqwest::Response, String> {
    post_in_fixture(ctx, host, trace, trace, streaming).await
}

async fn post_in_fixture(
    ctx: &TestContext,
    host: &str,
    trace: &str,
    fixture_id: &str,
    streaming: bool,
) -> Result<reqwest::Response, String> {
    client(ctx, host)?
        .post(format!("http://{host}:{}{}", ctx.http_port, PATH))
        .header("x-trace-id", trace)
        .header("x-resilience-fixture-id", fixture_id)
        .header("x-resilience-case", "cross-provider")
        .header("access_log", "test_store")
        .json(&json!({
            "model": "fixture-model",
            "stream": streaming,
            "messages": [{"role": "user", "content": "resilience fixture"}]
        }))
        .send()
        .await
        .map_err(|error| format!("resilience AI request failed: {error}"))
}

async fn set_half_open_mode(ctx: &TestContext, fixture_id: &str, mode: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .no_proxy()
        .build()
        .map_err(|error| format!("failed to build fixture control client: {error}"))?;
    for host in test_backend_hosts(ctx) {
        let response = client
            .post(format!(
                "http://{host}:30001/ai-resilience/mode/{fixture_id}/half-open-primary/{mode}"
            ))
            .send()
            .await
            .map_err(|error| format!("half-open fixture control failed for {host}: {error}"))?;
        if response.status() != reqwest::StatusCode::NO_CONTENT {
            return Err(format!(
                "half-open fixture control for {host} returned {}",
                response.status()
            ));
        }
    }
    Ok(())
}

async fn half_open_counts(ctx: &TestContext, fixture_id: &str) -> Result<Value, String> {
    aggregate_counts(ctx, &format!("counts/{fixture_id}")).await
}

async fn aggregate_counts(ctx: &TestContext, path: &str) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .no_proxy()
        .build()
        .map_err(|error| format!("failed to build fixture count client: {error}"))?;
    let mut totals = [0_u64; 6];
    for host in test_backend_hosts(ctx) {
        let value: Value = client
            .get(format!("http://{host}:30001/ai-resilience/{path}"))
            .send()
            .await
            .map_err(|error| format!("resilience count request failed for {host}: {error}"))?
            .json()
            .await
            .map_err(|error| format!("invalid resilience count response from {host}: {error}"))?;
        for (index, field) in [
            "rate_limited",
            "healthy",
            "stream_primary",
            "stream_fallback",
            "half_open_primary",
            "half_open_fallback",
        ]
        .iter()
        .enumerate()
        {
            totals[index] = totals[index].saturating_add(value[*field].as_u64().unwrap_or(0));
        }
    }
    Ok(json!({
        "rate_limited": totals[0],
        "healthy": totals[1],
        "stream_primary": totals[2],
        "stream_fallback": totals[3],
        "half_open_primary": totals[4],
        "half_open_fallback": totals[5],
    }))
}

async fn release_half_open_probe(ctx: &TestContext, fixture_id: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .no_proxy()
        .build()
        .map_err(|error| format!("failed to build fixture release client: {error}"))?;
    for host in test_backend_hosts(ctx) {
        let response = client
            .post(format!(
                "http://{host}:30001/ai-resilience/release/{fixture_id}/half-open-primary"
            ))
            .send()
            .await
            .map_err(|error| format!("half-open fixture release failed for {host}: {error}"))?;
        if response.status() != reqwest::StatusCode::NO_CONTENT {
            return Err(format!(
                "half-open fixture release for {host} returned {}",
                response.status()
            ));
        }
    }
    Ok(())
}

async fn complete_request(ctx: &TestContext, trace: &str, fixture_id: &str) -> Result<(), String> {
    let response = post_in_fixture(ctx, HALF_OPEN_HOST, trace, fixture_id, false).await?;
    let status = response.status();
    response
        .bytes()
        .await
        .map_err(|error| format!("half-open response body failed: {error}"))?;
    if status != reqwest::StatusCode::OK {
        return Err(format!("half-open request expected final 200, got {status}"));
    }
    Ok(())
}

async fn counts(ctx: &TestContext, trace: &str) -> Result<Value, String> {
    aggregate_counts(ctx, &format!("count/{trace}")).await
}

fn safe_attempts(genai: &Value) -> Result<&Vec<Value>, String> {
    let attempts = genai["attempts"]
        .as_array()
        .ok_or_else(|| format!("genai.attempts is not an array: {genai}"))?;
    for attempt in attempts {
        let object = attempt
            .as_object()
            .ok_or_else(|| format!("attempt is not an object: {attempt}"))?;
        if let Some(field) = object
            .keys()
            .find(|field| !SAFE_ATTEMPT_FIELDS.contains(&field.as_str()))
        {
            return Err(format!("attempt ledger exposed unexpected field {field:?}: {attempt}"));
        }
    }
    let rendered = genai.to_string();
    for forbidden in [
        "private resilience fixture detail",
        "private half-open fixture detail",
        "resilience fixture",
        "ai-provider-test-key",
        "127.0.0.1:30001",
    ] {
        if rendered.contains(forbidden) {
            return Err(format!("attempt ledger leaked forbidden fixture {forbidden:?}"));
        }
    }
    Ok(attempts)
}

async fn logged_attempts(ctx: &TestContext, trace: &str) -> Result<Vec<Value>, String> {
    let entry = ctx
        .access_log_client()
        .get_access_log_with_retry(trace, 20, 200)
        .await
        .map_err(|error| format!("half-open access log missing for {trace}: {error}"))?;
    safe_attempts(&entry.data["genai"]).cloned()
}

fn provider_429_priority_fallback(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let trace = format!("ai-resilience-429-{}", uuid::Uuid::new_v4());
        let response = match post(&ctx, RATE_LIMIT_HOST, &trace, false).await {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if response.status() != reqwest::StatusCode::OK || response.headers().contains_key("retry-after") {
            return TestResult::failed(
                start.elapsed(),
                format!(
                    "429 fallback expected final 200 without leaked Retry-After, got {} {:?}",
                    response.status(),
                    response.headers().get("retry-after")
                ),
            );
        }
        let body: Value = match response.json().await {
            Ok(body) => body,
            Err(error) => return TestResult::failed(start.elapsed(), format!("fallback body read failed: {error}")),
        };
        let observation = body
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .and_then(|content| serde_json::from_str::<Value>(content).ok());
        let Some(observation) = observation else {
            return TestResult::failed(
                start.elapsed(),
                format!("fallback was not translated to OpenAI shape: {body}"),
            );
        };
        for key in [
            "credential_valid",
            "authorization_absent",
            "version_valid",
            "beta_absent",
            "accept_encoding_absent",
            "host_valid",
            "path_valid",
            "query_absent",
            "model_canonical",
            "max_tokens_valid",
            "messages_valid",
            "openai_fields_absent",
        ] {
            if observation[key] != true {
                return TestResult::failed(
                    start.elapsed(),
                    format!("cross-provider request translation failed {key}: {observation}"),
                );
            }
        }
        let observed = match counts(&ctx, &trace).await {
            Ok(observed) => observed,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if observed["rate_limited"] != 1 || observed["healthy"] != 1 {
            return TestResult::failed(start.elapsed(), format!("unexpected provider hit counts: {observed}"));
        }

        let entry = match ctx.access_log_client().get_access_log_with_retry(&trace, 20, 200).await {
            Ok(entry) => entry,
            Err(error) => return TestResult::failed(start.elapsed(), format!("fallback access log missing: {error}")),
        };
        let genai = &entry.data["genai"];
        let attempts = match safe_attempts(genai) {
            Ok(attempts) => attempts,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if attempts.len() != 2
            || attempts[0]["backend"] != "edgion-test/resilience-rate-limited"
            || attempts[0]["credential"] != "rate-primary"
            || attempts[0]["status"] != 429
            || attempts[0]["failure"] != "retryable_status"
            || attempts[0]["retry_after_seconds"] != 2
            || attempts[1]["backend"] != "edgion-test/resilience-healthy"
            || attempts[1]["provider"] != "anthropic"
            || attempts[1]["credential"] != "healthy-primary"
            || attempts[1]["status"] != 200
            || !attempts[1]["failure"].is_null()
        {
            return TestResult::failed(start.elapsed(), format!("unexpected 429 fallback ledger: {attempts:?}"));
        }
        TestResult::passed_with_message(
            start.elapsed(),
            "provider reset cooldown normalized to 2s and Priority fallback completed on the second AI backend"
                .to_string(),
        )
    })
}

fn committed_sse_disconnect_never_falls_back(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let trace = format!("ai-resilience-sse-{}", uuid::Uuid::new_v4());
        let response = match post(&ctx, SSE_HOST, &trace, true).await {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if response.status() != reqwest::StatusCode::OK {
            return TestResult::failed(
                start.elapsed(),
                format!("SSE fixture expected 200, got {}", response.status()),
            );
        }
        let mut stream = response.bytes_stream();
        let mut pending = Vec::new();
        let mut events = 0usize;
        let mut saw_done = false;
        while let Some(chunk) = stream.next().await {
            let Ok(chunk) = chunk else {
                break;
            };
            pending.extend_from_slice(&chunk);
            while let Some(end) = pending.windows(2).position(|window| window == b"\n\n") {
                let frame = pending.drain(..end + 2).collect::<Vec<_>>();
                if frame.starts_with(b"data:") {
                    events += 1;
                    saw_done |= frame.windows(6).any(|window| window == b"[DONE]");
                }
            }
        }
        if events != 3 || saw_done {
            return TestResult::failed(
                start.elapsed(),
                format!(
                    "expected exactly three SSE events followed by disconnect, got events={events}, done={saw_done}"
                ),
            );
        }
        let observed = match counts(&ctx, &trace).await {
            Ok(observed) => observed,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if observed["stream_primary"] != 1 || observed["stream_fallback"] != 0 {
            return TestResult::failed(
                start.elapsed(),
                format!("committed SSE unexpectedly fell back: {observed}"),
            );
        }

        let entry = match ctx.access_log_client().get_access_log_with_retry(&trace, 20, 200).await {
            Ok(entry) => entry,
            Err(error) => return TestResult::failed(start.elapsed(), format!("SSE access log missing: {error}")),
        };
        let genai = &entry.data["genai"];
        let attempts = match safe_attempts(genai) {
            Ok(attempts) => attempts,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if attempts.len() != 1
            || attempts[0]["backend"] != "edgion-test/resilience-stream-primary"
            || attempts[0]["credential"] != "stream-primary"
            || attempts[0]["status"] != 200
            || attempts[0]["failure"] != "incomplete_response"
        {
            return TestResult::failed(
                start.elapsed(),
                format!("unexpected committed SSE ledger: {attempts:?}"),
            );
        }
        TestResult::passed_with_message(
            start.elapsed(),
            "three committed SSE events were followed by an upstream disconnect with zero fallback hits".to_string(),
        )
    })
}

fn half_open_single_probe_recovers_primary(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let fixture_id = format!("ai-resilience-half-open-{}", uuid::Uuid::new_v4());
        if let Err(error) = set_half_open_mode(&ctx, &fixture_id, "fail").await {
            return TestResult::failed(start.elapsed(), error);
        }

        for sequence in 0..2 {
            let trace = format!("{fixture_id}-open-{sequence}");
            if let Err(error) = complete_request(&ctx, &trace, &fixture_id).await {
                return TestResult::failed(start.elapsed(), error);
            }
            let attempts = match logged_attempts(&ctx, &trace).await {
                Ok(attempts) => attempts,
                Err(error) => return TestResult::failed(start.elapsed(), error),
            };
            if attempts.len() != 2
                || attempts[0]["backend"] != "edgion-test/resilience-half-open-primary"
                || attempts[0]["credential"] != "half-open-primary"
                || attempts[0]["status"] != 503
                || attempts[0]["failure"] != "retryable_status"
                || attempts[1]["backend"] != "edgion-test/resilience-half-open-fallback"
                || attempts[1]["credential"] != "half-open-fallback"
                || attempts[1]["status"] != 200
                || !attempts[1]["failure"].is_null()
            {
                return TestResult::failed(
                    start.elapsed(),
                    format!("failure {sequence} did not produce one verdict per dispatch: {attempts:?}"),
                );
            }
        }

        let open_trace = format!("{fixture_id}-open-rejected");
        if let Err(error) = complete_request(&ctx, &open_trace, &fixture_id).await {
            return TestResult::failed(start.elapsed(), error);
        }
        let open_attempts = match logged_attempts(&ctx, &open_trace).await {
            Ok(attempts) => attempts,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if open_attempts.len() != 1
            || open_attempts[0]["backend"] != "edgion-test/resilience-half-open-fallback"
            || open_attempts[0]["status"] != 200
            || !open_attempts[0]["failure"].is_null()
        {
            return TestResult::failed(
                start.elapsed(),
                format!("open primary was not bypassed with one fallback verdict: {open_attempts:?}"),
            );
        }

        if let Err(error) = set_half_open_mode(&ctx, &fixture_id, "probe-success").await {
            return TestResult::failed(start.elapsed(), error);
        }
        tokio::time::sleep(Duration::from_millis(10_200)).await;

        let concurrent = (0..8)
            .map(|sequence| {
                let ctx = ctx.clone();
                let fixture_id = fixture_id.clone();
                tokio::spawn(async move {
                    let trace = format!("{fixture_id}-probe-{sequence}");
                    complete_request(&ctx, &trace, &fixture_id).await?;
                    let attempts = logged_attempts(&ctx, &trace).await?;
                    Ok::<_, String>((trace, attempts))
                })
            })
            .collect::<Vec<_>>();

        let mut admission_counts = Value::Null;
        for _ in 0..100 {
            admission_counts = match half_open_counts(&ctx, &fixture_id).await {
                Ok(counts) => counts,
                Err(error) => {
                    let _ = release_half_open_probe(&ctx, &fixture_id).await;
                    return TestResult::failed(start.elapsed(), error);
                }
            };
            if admission_counts["half_open_primary"] == 3 && admission_counts["half_open_fallback"] == 10 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if let Err(error) = release_half_open_probe(&ctx, &fixture_id).await {
            return TestResult::failed(start.elapsed(), error);
        }
        if admission_counts["half_open_primary"] != 3 || admission_counts["half_open_fallback"] != 10 {
            return TestResult::failed(
                start.elapsed(),
                format!("concurrent peers did not reach the fixture before probe release: {admission_counts}"),
            );
        }

        let mut primary_probes = 0usize;
        let mut fallback_requests = 0usize;
        for joined in futures::future::join_all(concurrent).await {
            let (trace, attempts) = match joined {
                Ok(Ok(value)) => value,
                Ok(Err(error)) => return TestResult::failed(start.elapsed(), error),
                Err(error) => {
                    return TestResult::failed(start.elapsed(), format!("half-open request task failed: {error}"));
                }
            };
            if attempts.len() != 1 || attempts[0]["status"] != 200 || !attempts[0]["failure"].is_null() {
                return TestResult::failed(
                    start.elapsed(),
                    format!("concurrent request {trace} did not emit exactly one successful verdict: {attempts:?}"),
                );
            }
            match attempts[0]["backend"].as_str() {
                Some("edgion-test/resilience-half-open-primary") => primary_probes += 1,
                Some("edgion-test/resilience-half-open-fallback") => fallback_requests += 1,
                _ => {
                    return TestResult::failed(
                        start.elapsed(),
                        format!("concurrent request {trace} selected an unexpected backend: {attempts:?}"),
                    );
                }
            }
        }
        if primary_probes != 1 || fallback_requests != 7 {
            return TestResult::failed(
                start.elapsed(),
                format!("half-open admission expected one probe and seven fallbacks, got primary={primary_probes} fallback={fallback_requests}"),
            );
        }

        if let Err(error) = set_half_open_mode(&ctx, &fixture_id, "healthy").await {
            return TestResult::failed(start.elapsed(), error);
        }
        let recovered_trace = format!("{fixture_id}-recovered");
        if let Err(error) = complete_request(&ctx, &recovered_trace, &fixture_id).await {
            return TestResult::failed(start.elapsed(), error);
        }
        let recovered_attempts = match logged_attempts(&ctx, &recovered_trace).await {
            Ok(attempts) => attempts,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if recovered_attempts.len() != 1
            || recovered_attempts[0]["backend"] != "edgion-test/resilience-half-open-primary"
            || recovered_attempts[0]["status"] != 200
            || !recovered_attempts[0]["failure"].is_null()
        {
            return TestResult::failed(
                start.elapsed(),
                format!("successful probe did not close primary state: {recovered_attempts:?}"),
            );
        }

        let observed = match counts(&ctx, &fixture_id).await {
            Ok(observed) => observed,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if observed["half_open_primary"] != 4 || observed["half_open_fallback"] != 10 {
            return TestResult::failed(
                start.elapsed(),
                format!("unexpected half-open backend counts: {observed}"),
            );
        }

        TestResult::passed_with_message(
            start.elapsed(),
            "two failures opened the primary; one exclusive half-open probe recovered it while seven peers fell back"
                .to_string(),
        )
    })
}

#[async_trait]
impl TestSuite for ResilienceTestSuite {
    fn name(&self) -> &str {
        "EdgionBackend/Resilience"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionBackend/Resilience")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            TestCase::new(
                "provider_429_priority_fallback",
                "A provider 429 cooldown rotates to the next Priority AI backend",
                provider_429_priority_fallback,
            ),
            TestCase::new(
                "committed_sse_disconnect_never_falls_back",
                "A committed SSE response never falls back after an upstream disconnect",
                committed_sse_disconnect_never_falls_back,
            ),
            TestCase::new(
                "half_open_single_probe_recovers_primary",
                "Only one concurrent request probes a half-open primary before recovery",
                half_open_single_probe_recovers_primary,
            )
            .with_timeout(Duration::from_secs(120)),
        ]
    }
}
