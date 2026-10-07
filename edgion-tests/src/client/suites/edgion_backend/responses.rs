//! Native Responses transport, reported-only accounting and continuation fences.
use crate::framework::{test_backend_hosts, TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

pub struct ResponsesTestSuite;
pub struct ResponsesContinuationAllowedSuite;
pub struct ResponsesContinuationRejectedSuite;

type TestFuture = std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>>;

async fn post(
    ctx: &TestContext,
    host: &str,
    body: Value,
    mode: &str,
    h2: bool,
    marker: &str,
    path: &str,
) -> Result<reqwest::Response, String> {
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
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .resolve(host, SocketAddr::new(target_ip, ctx.http_port));
    builder = if h2 {
        builder.http2_prior_knowledge()
    } else {
        builder.http1_only()
    };
    builder
        .build()
        .map_err(|e| e.to_string())?
        .post(format!(
            "http://{}:{}{path}?fixture_query=preserved&mock_mode={mode}",
            host, ctx.http_port
        ))
        .header("authorization", "Bearer client-credential-must-be-stripped")
        .header("x-api-key", "client-api-key-must-be-stripped")
        .header("OpenAI-Organization", "org-client-selector")
        .header("OpenAI-Project", "proj-client-selector")
        .header("traceparent", crate::access_log_client::traceparent_for_marker(marker))
        .header("access_log", "test_store")
        .header("x-quota-user", marker)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())
}

fn body(stream: bool) -> Value {
    json!({"model":"fixture-model","stream":stream,"input":[{"role":"user","content":"hello"}],"tools":[{"type":"web_search"}],"native_extension":{"keep":true}})
}

async fn ledger(ctx: &TestContext, marker: &str, source: &str, failure: Option<&str>) -> Result<(), String> {
    let client = ctx.access_log_client();
    let entry = client
        .get_access_log_with_retry(marker, 15, 200)
        .await
        .map_err(|e| e.to_string())?;
    ctx.access_log_client()
        .require_single_write(marker)
        .await
        .map_err(|e| e.to_string())?;
    let genai = &entry.data["genai"];
    if genai["usage_source"] != source {
        return Err(format!("{marker} unexpected usage: {genai}"));
    }
    let attempts = genai["attempts"].as_array().ok_or("missing attempts")?;
    if attempts.len() != 1 || attempts[0]["usage_source"] != source {
        return Err(format!("wrong attempt ledger: {genai}"));
    }
    if source == "reported" {
        if genai["input_tokens"] != 11 || genai["output_tokens"] != 7 {
            return Err(format!("wrong provider totals: {genai}"));
        }
    } else if genai.get("input_tokens").is_some()
        || genai.get("output_tokens").is_some()
        || genai.get("estimated_cost_usd").is_some()
    {
        return Err(format!("unobserved Responses acquired numeric billing: {genai}"));
    }
    if genai["attempts_truncated"] == true || attempts[0]["status"] != 200 {
        return Err(format!("invalid completed attempt: {genai}"));
    }
    for key in ["input_tokens", "output_tokens", "estimated_cost_usd"] {
        if attempts[0].get(key) != genai.get(key) {
            return Err(format!("attempt/summary mismatch for {key}: {genai}"));
        }
    }
    if failure.is_none() && !attempts[0]["failure"].is_null() {
        return Err(format!("unexpected provider failure: {genai}"));
    }
    if let Some(failure) = failure {
        if attempts[0]["failure"] != failure {
            return Err(format!("wrong provider outcome: {genai}"));
        }
    }
    Ok(())
}

fn malformed_utf8(ctx: TestContext) -> TestFuture {
    Box::pin(async move {
        let start = Instant::now();
        let result = async {
            for observation in ["off", "provider"] {
                for model in ["fixture-model", "fixture-alias"] {
                    for nested in [false, true] {
                        let marker = format!("responses-utf8-{observation}-{model}-{nested}-{}", uuid::Uuid::new_v4());
                        let mut body = format!("{{\"model\":\"{model}\",").into_bytes();
                        body.extend_from_slice(if nested {
                            b"\"extension\":{\"items\":[\"\xff\"]}}"
                        } else {
                            b"\"input\":\"\xff\"}"
                        });
                        let response = ctx
                            .http_client
                            .post(format!("http://{}:{}/v1/responses", ctx.target_host, ctx.http_port))
                            .header("host", format!("responses-{observation}.example.com"))
                            .header("content-type", "application/json")
                            .header("traceparent", crate::access_log_client::traceparent_for_marker(&marker))
                            .header("access_log", "test_store")
                            .body(body)
                            .send()
                            .await
                            .map_err(|e| e.to_string())?;
                        let status = response.status().as_u16();
                        let body = response.text().await.map_err(|e| e.to_string())?;
                        if status != 400 || !body.contains("invalid_request_body") {
                            return Err(format!("invalid UTF-8 outcome: {status} {body}"));
                        }
                        let entry = ctx
                            .access_log_client()
                            .get_access_log_with_retry(&marker, 15, 200)
                            .await
                            .map_err(|e| e.to_string())?;
                        if entry.upstream_count() != 0 {
                            return Err(format!("invalid body reached origin: {}", entry.data));
                        }
                    }
                }
            }
            Ok(())
        }
        .await;
        match result {
            Ok(()) => TestResult::passed(start.elapsed()),
            Err(e) => TestResult::failed(start.elapsed(), e),
        }
    })
}

fn nullable_controls(ctx: TestContext) -> TestFuture {
    Box::pin(async move {
        let start = Instant::now();
        let request =
            json!({"model":"fixture-model","input":"native", "background":null,"stream":null,"max_output_tokens":null});
        let result = async {
            let response = post(
                &ctx,
                "responses-off.example.com",
                request,
                "split",
                false,
                &format!("responses-null-controls-{}", uuid::Uuid::new_v4()),
                "/v1/responses",
            )
            .await?;
            if !response.status().is_success() {
                return Err(format!("null controls returned {}", response.status()));
            }
            let value: Value = response.json().await.map_err(|e| e.to_string())?;
            let echoed = value
                .pointer("/native_extension/request")
                .ok_or("missing request echo")?;
            for key in ["background", "stream", "max_output_tokens"] {
                if echoed.get(key) != Some(&Value::Null) {
                    return Err(format!("native null {key} changed"));
                }
            }
            Ok::<(), String>(())
        }
        .await;
        match result {
            Ok(()) => TestResult::passed(start.elapsed()),
            Err(e) => TestResult::failed(start.elapsed(), e),
        }
    })
}

async fn native(ctx: &TestContext, h2: bool) -> Result<(), String> {
    for (host, source) in [
        ("responses-off.example.com", "notObserved"),
        ("responses-provider.example.com", "reported"),
    ] {
        for stream in [false, true] {
            let marker = format!("responses-native-{h2}-{stream}-{source}-{}", uuid::Uuid::new_v4());
            let r = post(ctx, host, body(stream), "split", h2, &marker, "/v1/responses").await?;
            if !r.status().is_success() {
                return Err(format!("native request returned {}", r.status()));
            }
            let text = r.text().await.map_err(|e| e.to_string())?;
            let response: Value = if stream {
                let terminal = text
                    .lines()
                    .filter_map(|line| line.strip_prefix("data: "))
                    .filter_map(|data| serde_json::from_str::<Value>(data).ok())
                    .find(|value| value["type"] == "response.completed")
                    .ok_or("missing native terminal")?;
                if text.contains("[DONE]") || !text.contains("response.output_text.delta") {
                    return Err("native stream changed dialect".into());
                }
                terminal["response"].clone()
            } else {
                serde_json::from_str(&text).map_err(|e| e.to_string())?
            };
            let observation: Value = serde_json::from_str(
                response
                    .pointer("/output/0/content/0/text")
                    .and_then(Value::as_str)
                    .ok_or("missing native output")?,
            )
            .map_err(|e| e.to_string())?;
            for key in [
                "auth_is_provider_key",
                "principal_selectors_absent",
                "host_is_expected",
                "path_has_prefix",
                "query_is_preserved",
                "model_is_canonical",
            ] {
                if observation[key] != true {
                    return Err(format!("failed provider isolation {key}: {observation}"));
                }
            }
            if observation["auth_is_client_key"] != false
                || observation["include_usage"] != false
                || response.pointer("/native_extension/request/native_extension/keep") != Some(&json!(true))
            {
                return Err(format!("native request changed: {response}"));
            }
            ledger(ctx, &marker, source, None).await?;
        }
    }
    Ok(())
}
fn h1(ctx: TestContext) -> TestFuture {
    Box::pin(async move {
        let start = Instant::now();
        match native(&ctx, false).await {
            Ok(()) => TestResult::passed(start.elapsed()),
            Err(e) => TestResult::failed(start.elapsed(), e),
        }
    })
}
fn h2(ctx: TestContext) -> TestFuture {
    Box::pin(async move {
        let start = Instant::now();
        match native(&ctx, true).await {
            Ok(()) => TestResult::passed(start.elapsed()),
            Err(e) => TestResult::failed(start.elapsed(), e),
        }
    })
}

fn outcomes(ctx: TestContext) -> TestFuture {
    Box::pin(async move {
        let start = Instant::now();
        let result = async {
            for (mode, source, failure) in [
                ("incomplete", "reported", Some("provider_incomplete")),
                ("failed", "reported", Some("provider_failed")),
                ("cancelled", "reported", Some("provider_cancelled")),
                ("missing_usage", "unknown", None),
                ("malformed", "unknown", None),
                ("large", "reported", None),
                ("oversized", "unknown", None),
            ] {
                for stream in if mode == "missing_usage" {
                    vec![false, true]
                } else {
                    vec![mode == "oversized"]
                } {
                    let marker = format!("responses-outcome-{mode}-{}", uuid::Uuid::new_v4());
                    let r = post(
                        &ctx,
                        "responses-provider.example.com",
                        body(stream),
                        mode,
                        false,
                        &marker,
                        "/v1/responses",
                    )
                    .await?;
                    if !r.status().is_success() {
                        return Err(format!("passive {mode} returned {}", r.status()));
                    }
                    let text = r.text().await.map_err(|e| e.to_string())?;
                    if mode == "large" && text.len() < 256 * 1024 {
                        return Err("large native output was truncated".into());
                    }
                    if mode == "oversized" && text.len() < 1024 * 1024 {
                        return Err("passive oversized native event was truncated".into());
                    }
                    ledger(&ctx, &marker, source, failure).await?;
                }
            }
            for stream in [false, true] {
                let marker = format!("responses-missing-off-{stream}-{}", uuid::Uuid::new_v4());
                let response = post(
                    &ctx,
                    "responses-off.example.com",
                    body(stream),
                    "missing_usage",
                    false,
                    &marker,
                    "/v1/responses",
                )
                .await?;
                if response.status() != 200 {
                    return Err("missing usage changed native admission".into());
                }
                response.bytes().await.map_err(|e| e.to_string())?;
                ledger(&ctx, &marker, "notObserved", None).await?;
            }
            Ok::<(), String>(())
        }
        .await;
        match result {
            Ok(()) => TestResult::passed(start.elapsed()),
            Err(e) => TestResult::failed(start.elapsed(), e),
        }
    })
}

async fn continuation(ctx: &TestContext, allowed: bool) -> Result<(), String> {
    for field in ["previous_response_id", "conversation"] {
        let mut request = body(false);
        request[field] = json!("opaque-provider-id");
        let r = post(
            ctx,
            "responses-provider.example.com",
            request,
            "completed",
            false,
            &format!("responses-continuation-{field}-{allowed}-{}", uuid::Uuid::new_v4()),
            "/v1/responses",
        )
        .await?;
        let status = r.status();
        let text = r.text().await.map_err(|e| e.to_string())?;
        if allowed && !status.is_success() || !allowed && status != 400 {
            return Err(format!(
                "continuation {field} expected allowed={allowed}, got {status}: {text}"
            ));
        }
        if allowed {
            let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            if value.pointer(&format!("/native_extension/request/{field}")) != Some(&json!("opaque-provider-id")) {
                return Err("continuation identifier mutated".into());
            }
        }
    }
    let r = post(
        ctx,
        "responses-provider.example.com",
        body(false),
        "completed",
        false,
        &format!("responses-stateless-after-update-{}", uuid::Uuid::new_v4()),
        "/v1/responses",
    )
    .await?;
    if !r.status().is_success() {
        return Err(format!("stateless became unavailable: {}", r.status()));
    }
    r.bytes().await.map_err(|e| e.to_string())?;
    Ok(())
}
fn continuation_allowed(ctx: TestContext) -> TestFuture {
    Box::pin(async move {
        let start = Instant::now();
        match continuation(&ctx, true).await {
            Ok(()) => TestResult::passed(start.elapsed()),
            Err(e) => TestResult::failed(start.elapsed(), e),
        }
    })
}
fn continuation_rejected(ctx: TestContext) -> TestFuture {
    Box::pin(async move {
        let start = Instant::now();
        match continuation(&ctx, false).await {
            Ok(()) => TestResult::passed(start.elapsed()),
            Err(e) => TestResult::failed(start.elapsed(), e),
        }
    })
}

fn rejects(ctx: TestContext) -> TestFuture {
    Box::pin(async move {
        let start = Instant::now();
        let result = async {
            let mut cases = Vec::new();
            let mut request = body(false);
            request["background"] = json!(true);
            cases.push(("responses-provider.example.com", "/v1/responses", request));
            let mut request = body(false);
            request["previous_response_id"] = json!("opaque");
            request["model"] = json!("fixture-alias");
            cases.push(("responses-provider.example.com", "/v1/responses", request));
            let mut request = body(false);
            request["previous_response_id"] = json!("opaque");
            cases.push(("responses-multi.example.com", "/v1/responses", request));
            cases.push(("responses-provider.example.com", "/v1/chat/completions", body(false)));
            for host in [
                "responses-guard-before.example.com",
                "responses-guard-after.example.com",
            ] {
                cases.push((host, "/v1/responses", body(false)));
            }
            cases.push(("responses-guard-rewrite.example.com", "/custom/responses", body(false)));
            for (index, (host, path, request)) in cases.into_iter().enumerate() {
                let r = post(
                    &ctx,
                    host,
                    request,
                    "completed",
                    false,
                    &format!("responses-reject-{index}-{}", uuid::Uuid::new_v4()),
                    path,
                )
                .await?;
                if r.status() != if path == "/v1/chat/completions" { 404 } else { 400 } {
                    return Err(format!("{host}{path} returned {}", r.status()));
                }
                let value: Value = r.json().await.map_err(|e| e.to_string())?;
                if value.pointer("/error/code").is_none() {
                    return Err("local reject missing fixed error".into());
                }
            }
            Ok::<(), String>(())
        }
        .await;
        match result {
            Ok(()) => TestResult::passed(start.elapsed()),
            Err(e) => TestResult::failed(start.elapsed(), e),
        }
    })
}

fn termination(ctx: TestContext) -> TestFuture {
    Box::pin(async move {
        let start = Instant::now();
        let result = async {
            for observation in ["off", "provider"] {
                let host = format!("responses-{observation}.example.com");
                let source = if observation == "off" { "notObserved" } else { "unknown" };
                for h2 in [false, true] {
                    let marker = format!("responses-cancel-{observation}-{h2}-{}", uuid::Uuid::new_v4());
                    let response = post(&ctx, &host, body(true), "long", h2, &marker, "/v1/responses").await?;
                    if response.status() != 200 {
                        return Err("long stream not committed as 200".into());
                    }
                    let mut stream = response.bytes_stream();
                    let mut prefix = Vec::new();
                    loop {
                        let chunk = stream
                            .next()
                            .await
                            .ok_or("no nonterminal content delta")?
                            .map_err(|e| e.to_string())?;
                        if prefix.len() + chunk.len() > 16 * 1024 {
                            return Err("cancel prefix cap exceeded".into());
                        }
                        prefix.extend_from_slice(&chunk);
                        let text = String::from_utf8_lossy(&prefix);
                        if text.contains("event: response.completed") {
                            return Err("terminal preceded cancellation".into());
                        }
                        if text.contains("event: response.output_text.delta") && text.ends_with("\n\n") {
                            break;
                        }
                    }
                    drop(stream);
                    let trace = crate::access_log_client::trace_id_for_marker(&marker);
                    let mut cancelled = false;
                    for _ in 0..30 {
                        for backend in crate::framework::test_backend_hosts(&ctx) {
                            let authority = backend
                                .parse::<IpAddr>()
                                .map(|ip| SocketAddr::new(ip, 30001).to_string())
                                .unwrap_or_else(|_| format!("{backend}:30001"));
                            let value: Value = ctx
                                .http_client
                                .get(format!("http://{authority}/ai-long-stream/observation/{trace}"))
                                .send()
                                .await
                                .map_err(|e| e.to_string())?
                                .json()
                                .await
                                .map_err(|e| e.to_string())?;
                            let state = &value["observation"];
                            if state["upstream_cancelled"] == true {
                                let count = state["chunks_queued"].as_u64().ok_or("missing producer count")?;
                                if value["found"] != true
                                    || state["provider_completed"] != false
                                    || !(2..1000).contains(&count)
                                {
                                    return Err(format!("invalid cancelled producer: {value}"));
                                }
                                cancelled = true;
                                break;
                            }
                        }
                        if cancelled {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    if !cancelled {
                        return Err("downstream cancellation did not stop upstream producer".into());
                    }
                    ledger(&ctx, &marker, source, Some("incomplete_response")).await?;
                    let marker = format!("responses-abort-{observation}-{h2}-{}", uuid::Uuid::new_v4());
                    let response = post(&ctx, &host, body(true), "abort", h2, &marker, "/v1/responses").await?;
                    if response.status() != 200 {
                        return Err("abort stream not committed as 200".into());
                    }
                    if let Ok(bytes) = response.bytes().await {
                        if bytes.is_empty() || String::from_utf8_lossy(&bytes).contains("event: response.completed") {
                            return Err("aborted body did not expose an incomplete stream".into());
                        }
                    }
                    ledger(&ctx, &marker, source, Some("incomplete_response")).await?;
                }
            }
            Ok::<(), String>(())
        }
        .await;
        match result {
            Ok(()) => TestResult::passed(start.elapsed()),
            Err(e) => TestResult::failed(start.elapsed(), e),
        }
    })
}

fn quota(ctx: TestContext) -> TestFuture {
    Box::pin(async move {
        let start = Instant::now();
        let result = async {
            let identity = format!("responses-quota-{}", uuid::Uuid::new_v4());
            let r = post(
                &ctx,
                "responses-quota.example.com",
                body(false),
                "completed",
                false,
                &identity,
                "/v1/responses",
            )
            .await?;
            if !r.status().is_success() {
                return Err(format!("quota initial admission {}", r.status()));
            }
            r.bytes().await.map_err(|e| e.to_string())?;
            ledger(&ctx, &identity, "reported", None).await?;
            let followup = uuid::Uuid::new_v4().to_string();
            let authority = ctx
                .target_host
                .parse::<IpAddr>()
                .map(|ip| SocketAddr::new(ip, ctx.http_port).to_string())
                .unwrap_or_else(|_| format!("{}:{}", ctx.target_host, ctx.http_port));
            let r = ctx
                .http_client
                .post(format!("http://{authority}/v1/responses"))
                .header("host", "responses-quota.example.com")
                .header("x-quota-user", &identity)
                .header(
                    "traceparent",
                    crate::access_log_client::traceparent_for_marker(&followup),
                )
                .header("access_log", "test_store")
                .json(&body(false))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if r.status() != 429 {
                return Err(format!("terminal usage did not settle Redis quota: {}", r.status()));
            }
            r.bytes().await.map_err(|e| e.to_string())?;
            let marker = format!("responses-quota-cut-{}", uuid::Uuid::new_v4());
            let r = post(
                &ctx,
                "responses-quota.example.com",
                body(true),
                "oversized",
                false,
                &marker,
                "/v1/responses",
            )
            .await?;
            if !r.status().is_success() {
                return Err(format!("quota stream admission {}", r.status()));
            }
            let text = r.text().await.map_err(|e| e.to_string())?;
            let error = text
                .lines()
                .filter_map(|line| line.strip_prefix("data: "))
                .filter_map(|data| serde_json::from_str::<Value>(data).ok())
                .find(|value| value["type"] == "error")
                .ok_or("missing typed Responses quota error")?;
            if error.get("sequence_number").and_then(Value::as_u64).is_none()
                || error.get("param") != Some(&Value::Null)
                || error.get("code").is_none()
                || text.contains("[DONE]")
            {
                return Err(format!("wrong local error dialect: {error}"));
            }
            ledger(&ctx, &marker, "unknown", None).await?;
            Ok::<(), String>(())
        }
        .await;
        match result {
            Ok(()) => TestResult::passed(start.elapsed()),
            Err(e) => TestResult::failed(start.elapsed(), e),
        }
    })
}

#[async_trait]
impl TestSuite for ResponsesTestSuite {
    fn name(&self) -> &str {
        "EdgionBackend/Responses"
    }
    fn port_key(&self) -> Option<&str> {
        Some("EdgionBackend/Responses")
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            TestCase::new(
                "malformed_utf8",
                "Invalid ignored strings fail before provider dispatch in every observation/model mode",
                malformed_utf8,
            ),
            TestCase::new(
                "quota",
                "Reported terminal settlement and typed active framing cut",
                quota,
            ),
            TestCase::new(
                "nullable_controls",
                "Nullable native controls retain their exact meaning",
                nullable_controls,
            ),
            TestCase::new("h1_native", "H1 native JSON/SSE and provider isolation", h1),
            TestCase::new("h2_native", "H2 native JSON/SSE", h2),
            TestCase::new(
                "outcomes",
                "Terminal outcomes, unknown usage, huge native output",
                outcomes,
            ),
            TestCase::new(
                "continuation",
                "Opaque continuation on one principal",
                continuation_allowed,
            ),
            TestCase::new("rejects", "Capability and ambiguous continuation fences", rejects),
            TestCase::new("termination", "Downstream cancellation and upstream abort", termination),
        ]
    }
}
#[async_trait]
impl TestSuite for ResponsesContinuationAllowedSuite {
    fn name(&self) -> &str {
        "ResponsesContinuationAllowed"
    }
    fn port_key(&self) -> Option<&str> {
        Some("EdgionBackend/Responses")
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "continuation_allowed",
            "One declared credential permits continuation",
            continuation_allowed,
        )]
    }
}
#[async_trait]
impl TestSuite for ResponsesContinuationRejectedSuite {
    fn name(&self) -> &str {
        "ResponsesContinuationRejected"
    }
    fn port_key(&self) -> Option<&str> {
        Some("EdgionBackend/Responses")
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "continuation_rejected",
            "Unresolved second binding rejects continuation while stateless remains usable",
            continuation_rejected,
        )]
    }
}
