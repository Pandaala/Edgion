//! End-to-end coverage for native Anthropic Messages translation.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::metrics_helper::MetricsClient;
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

pub struct AnthropicProviderTestSuite;

const HOST: &str = "anthropic.example.com";
const PATH: &str = "/v1/chat/completions";

fn client(ctx: &TestContext, h2: bool, raw_gzip: bool) -> Result<reqwest::Client, String> {
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
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .no_proxy()
        .resolve(HOST, SocketAddr::new(target_ip, ctx.http_port));
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

fn base_body(stream: bool) -> Value {
    json!({
        "model":"claude-alias",
        "stream":stream,
        "messages":[
            {"role":"system","content":"fixed system instruction"},
            {"role":"user","content":"fixed user prompt"}
        ]
    })
}

async fn post(
    ctx: &TestContext,
    body: Value,
    mode: &str,
    h2: bool,
    trace_id: Option<&str>,
    raw_gzip: bool,
) -> Result<reqwest::Response, String> {
    let mut request = client(ctx, h2, raw_gzip)?
        .post(format!(
            "http://{}:{}{}?downstream_query=must_be_dropped",
            HOST, ctx.http_port, PATH
        ))
        .header("authorization", "Bearer client-credential-must-be-stripped")
        .header("x-api-key", "client-api-key-must-be-stripped")
        .header("anthropic-version", "1900-01-01")
        .header("anthropic-beta", "client-beta-must-be-stripped")
        .header("x-anthropic-fixture-mode", mode)
        .json(&body);
    if raw_gzip {
        request = request.header("accept-encoding", "gzip");
    }
    if let Some(trace_id) = trace_id {
        request = request
            .header("x-trace-id", trace_id)
            .header("access_log", "test_store");
    }
    request
        .send()
        .await
        .map_err(|error| format!("Anthropic request failed: {error}"))
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

fn sse_data(frame: &[u8]) -> Result<&str, String> {
    let text = std::str::from_utf8(frame).map_err(|error| format!("SSE frame was not UTF-8: {error}"))?;
    text.lines()
        .find_map(|line| line.strip_prefix("data: "))
        .ok_or_else(|| format!("SSE frame has no data field: {text:?}"))
}

fn sse_json(frame: &[u8]) -> Result<Option<Value>, String> {
    let data = sse_data(frame)?;
    if data == "[DONE]" {
        Ok(None)
    } else {
        serde_json::from_str(data)
            .map(Some)
            .map_err(|error| format!("invalid SSE JSON: {error}"))
    }
}

fn observation_from_completion(value: &Value) -> Result<Value, String> {
    let content = value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("completion has no observation content: {value}"))?;
    serde_json::from_str(content).map_err(|error| format!("invalid fixture observation: {error}"))
}

fn assert_observation(observation: &Value, require_system: bool) -> Result<(), String> {
    if observation["fixture_id"] != "anthropic-native-v1" {
        return Err(format!("unexpected fixture observation: {observation}"));
    }
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
            return Err(format!("expected {key}=true, got {}", observation[key]));
        }
    }
    if require_system && observation["system_valid"] != true {
        return Err(format!(
            "expected system_valid=true, got {}",
            observation["system_valid"]
        ));
    }
    Ok(())
}

async fn paced_protocol(ctx: TestContext, h2: bool) -> TestResult {
    let start = Instant::now();
    let response = match post(&ctx, base_body(true), "paced", h2, None, true).await {
        Ok(response) => response,
        Err(error) => return TestResult::failed(start.elapsed(), error),
    };
    let expected_version = if h2 {
        reqwest::Version::HTTP_2
    } else {
        reqwest::Version::HTTP_11
    };
    if response.version() != expected_version || response.status() != reqwest::StatusCode::OK {
        return TestResult::failed(
            start.elapsed(),
            format!(
                "expected 200 over {expected_version:?}, got {} over {:?}",
                response.status(),
                response.version()
            ),
        );
    }
    if response.headers().contains_key(reqwest::header::CONTENT_ENCODING)
        || response.headers().contains_key("request-id")
    {
        return TestResult::failed(start.elapsed(), "stream leaked provider/framing metadata".to_string());
    }
    let body_started = Instant::now();
    let (frames, arrivals) = match collect_sse(response).await {
        Ok(result) => result,
        Err(error) => return TestResult::failed(start.elapsed(), error),
    };
    if frames.len() != 5 || arrivals.len() != 5 {
        return TestResult::failed(
            start.elapsed(),
            format!("expected five OpenAI SSE frames, got {}", frames.len()),
        );
    }
    let first_wait = arrivals[0].saturating_duration_since(body_started);
    let visible_span = arrivals[2].saturating_duration_since(arrivals[0]);
    if first_wait >= Duration::from_millis(900) || visible_span < Duration::from_millis(450) {
        return TestResult::failed(
            start.elapsed(),
            format!("translated stream was buffered: first={first_wait:?}, visible_span={visible_span:?}"),
        );
    }
    let text_chunk = match sse_json(&frames[1]) {
        Ok(Some(value)) => value,
        other => return TestResult::failed(start.elapsed(), format!("missing text chunk: {other:?}")),
    };
    let observation: Value = match text_chunk
        .pointer("/choices/0/delta/content")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("text chunk missing content: {text_chunk}"))
        .and_then(|text| serde_json::from_str(text).map_err(|error| error.to_string()))
    {
        Ok(value) => value,
        Err(error) => return TestResult::failed(start.elapsed(), error),
    };
    if let Err(error) = assert_observation(&observation, true) {
        return TestResult::failed(start.elapsed(), error);
    }
    if sse_data(&frames[4]).ok() != Some("[DONE]") {
        return TestResult::failed(start.elapsed(), "stream did not end with [DONE]".to_string());
    }
    TestResult::passed_with_message(
        start.elapsed(),
        format!("Anthropic translation stayed paced over {expected_version:?}"),
    )
}

fn h1_paced(ctx: TestContext) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(paced_protocol(ctx, false))
}

fn h2_paced(ctx: TestContext) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(paced_protocol(ctx, true))
}

fn nonstream_canonical(ctx: TestContext) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let response = match post(&ctx, base_body(false), "default", false, None, false).await {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if response.status() != reqwest::StatusCode::OK
            || response.headers().contains_key(reqwest::header::CONTENT_LENGTH)
            || response.headers().contains_key("request-id")
        {
            return TestResult::failed(
                start.elapsed(),
                format!("unexpected translated headers/status: {response:?}"),
            );
        }
        let value: Value = match response.json().await {
            Ok(value) => value,
            Err(error) => return TestResult::failed(start.elapsed(), format!("invalid completion JSON: {error}")),
        };
        if value["id"] != "msg_fixture"
            || value["object"] != "chat.completion"
            || value["model"] != "claude-fixture"
            || value["choices"][0]["finish_reason"] != "stop"
            || value["usage"] != json!({"prompt_tokens":10,"completion_tokens":7,"total_tokens":17})
        {
            return TestResult::failed(start.elapsed(), format!("unexpected canonical completion: {value}"));
        }
        let observation = match observation_from_completion(&value) {
            Ok(value) => value,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if let Err(error) = assert_observation(&observation, true) {
            return TestResult::failed(start.elapsed(), error);
        }
        TestResult::passed_with_message(
            start.elapsed(),
            "canonical request, headers, response, and usage passed".to_string(),
        )
    })
}

fn tool_round_trip(ctx: TestContext) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let body = json!({
            "model":"claude-fixture",
            "messages":[{"role":"user","content":"use tools"}],
            "tools":[
                {"type":"function","function":{"name":"weather","description":"Weather","parameters":{"type":"object"}}},
                {"type":"function","function":{"name":"clock","parameters":{"type":"object"}}}
            ],
            "tool_choice":"required",
            "parallel_tool_calls":false
        });
        let response = match post(&ctx, body, "nonstream_tool", false, None, false).await {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if response.status() != reqwest::StatusCode::OK {
            return TestResult::failed(start.elapsed(), format!("tool request returned {}", response.status()));
        }
        let value: Value = match response.json().await {
            Ok(value) => value,
            Err(error) => return TestResult::failed(start.elapsed(), format!("invalid tool completion: {error}")),
        };
        let calls = match value.pointer("/choices/0/message/tool_calls").and_then(Value::as_array) {
            Some(calls) => calls,
            None => return TestResult::failed(start.elapsed(), format!("tool calls missing: {value}")),
        };
        if calls.len() != 2
            || calls[0]["index"] != 0
            || calls[1]["index"] != 1
            || calls[0]["function"]["arguments"] != "{\"city\":\"Paris\"}"
            || value["choices"][0]["finish_reason"] != "tool_calls"
        {
            return TestResult::failed(start.elapsed(), format!("unexpected tool conversion: {value}"));
        }
        TestResult::passed_with_message(start.elapsed(), "non-streaming tool conversion passed".to_string())
    })
}

fn streaming_tool(ctx: TestContext) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let mut body = base_body(true);
        body["tools"] = json!([{"type":"function","function":{"name":"weather","parameters":{"type":"object"}}}]);
        let response = match post(&ctx, body, "stream_tool", false, None, false).await {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let (frames, _) = match collect_sse(response).await {
            Ok(result) => result,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let values: Vec<Value> = match frames
            .iter()
            .filter_map(|frame| match sse_json(frame) {
                Ok(Some(value)) => Some(Ok(value)),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(values) => values,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let start_call = values
            .iter()
            .find_map(|value| value.pointer("/choices/0/delta/tool_calls/0"));
        let Some(start_call) = start_call else {
            return TestResult::failed(start.elapsed(), format!("tool start missing: {values:?}"));
        };
        if start_call["index"] != 0
            || start_call["id"] != "toolu_fixture"
            || start_call["function"]["name"] != "weather"
        {
            return TestResult::failed(start.elapsed(), format!("invalid tool start: {start_call}"));
        }
        let arguments = values
            .iter()
            .filter_map(|value| value.pointer("/choices/0/delta/tool_calls/0/function/arguments"))
            .filter_map(Value::as_str)
            .collect::<String>();
        if arguments != "{\"city\":\"Paris\"}"
            || !values
                .iter()
                .any(|value| value.pointer("/choices/0/finish_reason") == Some(&json!("tool_calls")))
        {
            return TestResult::failed(
                start.elapsed(),
                format!("invalid streamed tool arguments/finish: {values:?}"),
            );
        }
        TestResult::passed_with_message(
            start.elapsed(),
            "fragmented streaming tool conversion passed".to_string(),
        )
    })
}

fn metric_value(raw: &str, direction: &str, source: &str) -> f64 {
    raw.lines()
        .filter(|line| line.starts_with("edgion_gateway_genai_tokens_total{"))
        .filter(|line| line.contains(&format!("direction=\"{direction}\"")))
        .filter(|line| line.contains("provider=\"anthropic\""))
        .filter(|line| line.contains(&format!("usage_source=\"{source}\"")))
        .filter_map(|line| line.split_whitespace().last()?.parse::<f64>().ok())
        .sum()
}

fn usage_access_log_metric(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let metrics = MetricsClient::from_host_port(&ctx.target_host, 12090);
        let before = match metrics.fetch_raw_metrics().await {
            Ok(raw) => raw,
            Err(error) => return TestResult::failed(start.elapsed(), format!("metrics baseline failed: {error}")),
        };
        let trace = format!("anthropic-usage-{}", uuid::Uuid::new_v4());
        let response = match post(&ctx, base_body(true), "stream_tool", false, Some(&trace), false).await {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if let Err(error) = collect_sse(response).await {
            return TestResult::failed(start.elapsed(), error);
        }
        let entry = match ctx.access_log_client().get_access_log_with_retry(&trace, 15, 200).await {
            Ok(entry) => entry,
            Err(error) => return TestResult::failed(start.elapsed(), format!("access log missing: {error}")),
        };
        let genai = &entry.data["genai"];
        if genai["provider"] != "anthropic"
            || genai["model"] != "claude-alias"
            || genai["input_tokens"] != 10
            || genai["output_tokens"] != 7
            || genai["usage_estimated"] != false
        {
            return TestResult::failed(start.elapsed(), format!("unexpected Anthropic usage log: {genai}"));
        }
        let rendered = entry.data.to_string();
        for forbidden in [
            "fixed user prompt",
            "client-credential-must-be-stripped",
            "client-api-key-must-be-stripped",
            "anthropic-provider-test-key",
            "provider private detail",
        ] {
            if rendered.contains(forbidden) {
                return TestResult::failed(start.elapsed(), format!("access log leaked {forbidden:?}"));
            }
        }
        let after = match metrics.fetch_raw_metrics().await {
            Ok(raw) => raw,
            Err(error) => return TestResult::failed(start.elapsed(), format!("metrics read failed: {error}")),
        };
        let input_delta = metric_value(&after, "input", "reported") - metric_value(&before, "input", "reported");
        let output_delta = metric_value(&after, "output", "reported") - metric_value(&before, "output", "reported");
        let genai_metric_has_model_label = after
            .lines()
            .filter(|line| line.starts_with("edgion_gateway_genai_tokens_total{"))
            .any(|line| line.contains("model=\""));
        if input_delta != 10.0 || output_delta != 7.0 || genai_metric_has_model_label {
            return TestResult::failed(
                start.elapsed(),
                format!("metric delta/cardinality mismatch: {input_delta}/{output_delta}"),
            );
        }
        TestResult::passed_with_message(
            start.elapsed(),
            "exact cache-inclusive access-log and metric usage passed".to_string(),
        )
    })
}

async fn provider_count(ctx: &TestContext) -> Result<u64, String> {
    ctx.http_client
        .get(format!("http://{}:30001/anthropic-fixture/count", ctx.target_host))
        .send()
        .await
        .map_err(|error| format!("count request failed: {error}"))?
        .json::<Value>()
        .await
        .map_err(|error| format!("invalid count response: {error}"))?
        .get("count")
        .and_then(Value::as_u64)
        .ok_or_else(|| "count response omitted count".to_string())
}

fn fixed_errors_and_local_reject(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        for (mode, expected_status, expected_code) in [
            ("status_400", 400, "provider_invalid_request"),
            ("status_401", 503, "service_unavailable"),
            ("status_429", 429, "rate_limit_exceeded"),
        ] {
            let response = match post(&ctx, base_body(false), mode, false, None, false).await {
                Ok(response) => response,
                Err(error) => return TestResult::failed(start.elapsed(), error),
            };
            if response.status().as_u16() != expected_status
                || response.headers().contains_key("request-id")
                || response.headers().contains_key("location")
                || response.headers().contains_key("set-cookie")
            {
                return TestResult::failed(
                    start.elapsed(),
                    format!("provider error headers/status leaked for {mode}"),
                );
            }
            if mode == "status_429" && response.headers().get("retry-after").and_then(|v| v.to_str().ok()) != Some("3")
            {
                return TestResult::failed(
                    start.elapsed(),
                    "429 did not preserve validated Retry-After".to_string(),
                );
            }
            let body = response.text().await.unwrap_or_default();
            if !body.contains(expected_code)
                || body.contains("provider private detail")
                || body.contains("provider_private")
            {
                return TestResult::failed(
                    start.elapsed(),
                    format!("unexpected fixed error body for {mode}: {body}"),
                );
            }
        }

        let before = match provider_count(&ctx).await {
            Ok(count) => count,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let mut unsupported = base_body(false);
        unsupported["response_format"] = json!({"type":"json_object"});
        let response = match post(&ctx, unsupported, "default", false, None, false).await {
            Ok(response) => response,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let body = response.text().await.unwrap_or_default();
        let after = match provider_count(&ctx).await {
            Ok(count) => count,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if after != before || !body.contains("unsupported_parameter") {
            return TestResult::failed(
                start.elapsed(),
                format!("unsupported request reached provider or wrong error: count {before}->{after}, body={body}"),
            );
        }
        TestResult::passed_with_message(
            start.elapsed(),
            "fixed provider errors and must-not-reach rejection passed".to_string(),
        )
    })
}

fn malformed_and_oversized(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        for mode in ["malformed", "oversized"] {
            let response = match post(&ctx, base_body(true), mode, false, None, false).await {
                Ok(response) => response,
                Err(error) => return TestResult::failed(start.elapsed(), error),
            };
            let (frames, _) = match collect_sse(response).await {
                Ok(result) => result,
                Err(error) => return TestResult::failed(start.elapsed(), error),
            };
            if frames.len() != 1 {
                return TestResult::failed(start.elapsed(), format!("{mode} emitted {} frames", frames.len()));
            }
            let data = match sse_data(&frames[0]) {
                Ok(data) => data,
                Err(error) => return TestResult::failed(start.elapsed(), error),
            };
            if !data.contains("response_translation_failed")
                || data.contains("not-json")
                || data.contains("future_event")
                || data.contains("provider")
                || data == "[DONE]"
            {
                return TestResult::failed(start.elapsed(), format!("{mode} leaked raw provider data: {data}"));
            }
        }
        TestResult::passed_with_message(
            start.elapsed(),
            "malformed and oversized streams sealed without leakage".to_string(),
        )
    })
}

#[async_trait]
impl TestSuite for AnthropicProviderTestSuite {
    fn name(&self) -> &str {
        "EdgionAIService/AnthropicProvider"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionAIService/AnthropicProvider")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            TestCase::new(
                "h1_paced_translation",
                "H1 Anthropic SSE stays incremental and uncompressed",
                h1_paced,
            ),
            TestCase::new("h2_paced_translation", "H2 Anthropic SSE stays incremental", h2_paced),
            TestCase::new(
                "nonstream_canonical",
                "Request/header and canonical non-stream response translation",
                nonstream_canonical,
            ),
            TestCase::new(
                "nonstream_tool_round_trip",
                "Non-streaming tool call conversion",
                tool_round_trip,
            ),
            TestCase::new(
                "streaming_tool_fragments",
                "Streaming tool argument fragment conversion",
                streaming_tool,
            ),
            TestCase::new(
                "usage_access_log_metric",
                "Exact cache-inclusive usage in access log and optional metric",
                usage_access_log_metric,
            ),
            // Must run before fixed_errors_and_local_reject: that case applies
            // Retry-After cooldown on the suite's only credential.
            TestCase::new(
                "malformed_and_oversized",
                "Malformed and oversized provider streams seal safely",
                malformed_and_oversized,
            ),
            TestCase::new(
                "fixed_errors_and_local_reject",
                "Fixed provider errors and unsupported must-not-reach",
                fixed_errors_and_local_reject,
            ),
        ]
    }
}
