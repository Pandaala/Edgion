//! Native Messages wire preservation, reported-only accounting and binding fences.
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};
pub struct MessagesTestSuite;
pub struct MessagesBindingAllowedSuite;
pub struct MessagesBindingRejectedSuite;
pub struct MessagesCredentialBSuite;
type TestFuture = std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>>;
fn body(stream: bool) -> Value {
    json!({"model":"fixture-model","max_tokens":16,"stream":stream,
        "messages":[{"role":"user","content":[{"type":"text","text":"hello","cache_control":{"type":"ephemeral"}}]}],
        "system":[{"type":"text","text":"fixture system"}],
        "tools":[{"name":"weather","description":"fixture tool","input_schema":{"type":"object","properties":{"city":{"type":"string"}}}}],
        "native_extension":{"keep":true}})
}
// Explicit wire axes make individual protocol/header assertions reviewable.
#[allow(clippy::too_many_arguments)]
async fn post(
    ctx: &TestContext,
    host: &str,
    body: &Value,
    mode: &str,
    h2: bool,
    marker: &str,
    path: &str,
    version: &str,
    profile: bool,
) -> Result<reqwest::Response, String> {
    let ip = ctx
        .target_host
        .parse::<IpAddr>()
        .ok()
        .or_else(|| {
            (ctx.target_host.as_str(), 0)
                .to_socket_addrs()
                .ok()?
                .next()
                .map(|a| a.ip())
        })
        .ok_or("test address unavailable")?;
    let builder = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .resolve(host, SocketAddr::new(ip, ctx.http_port));
    let builder = if h2 {
        builder.http2_prior_knowledge()
    } else {
        builder.http1_only()
    };
    let mut request = builder
        .build()
        .map_err(|e| e.to_string())?
        .post(format!(
            "http://{host}:{}{path}?fixture_query=preserved&mock_mode={mode}",
            ctx.http_port
        ))
        .header("authorization", "Bearer caller-key")
        .header("api-key", "caller-key")
        .header("x-api-key", "caller-key")
        .header("anthropic-workspace-id", "caller-workspace")
        .header("anthropic-version", version)
        .header("anthropic-beta", "first-beta")
        .header("anthropic-beta", "second-beta")
        .header("traceparent", crate::access_log_client::traceparent_for_marker(marker))
        .header("access_log", "test_store")
        .header(
            "x-quota-user",
            format!(
                "{marker}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ),
        );
    if profile {
        request = request.header("anthropic-user-profile-id", "caller-profile");
    }
    request.json(body).send().await.map_err(|e| e.to_string())
}
async fn ledger(ctx: &TestContext, marker: &str, source: &str, failure: Option<&str>) -> Result<(), String> {
    let entry = ctx
        .access_log_client()
        .get_access_log_with_retry(marker, 15, 200)
        .await
        .map_err(|e| e.to_string())?;
    ctx.access_log_client()
        .require_single_write(marker)
        .await
        .map_err(|e| e.to_string())?;
    let genai = &entry.data["genai"];
    let attempts = genai["attempts"].as_array().ok_or("missing attempts")?;
    if attempts.len() != 1 || attempts[0]["usage_source"] != source {
        return Err(format!("wrong attempt ledger: {genai}"));
    }
    if genai["usage_source"] != source {
        return Err(format!("{marker}: unexpected usage {genai}"));
    }
    if source == "reported" {
        if genai["input_tokens"] != 13 || genai["output_tokens"] != 7 {
            return Err(format!("incorrect late/cache totals: {genai}"));
        }
    } else if genai.get("input_tokens").is_some()
        || genai.get("output_tokens").is_some()
        || genai.get("estimated_cost_usd").is_some()
    {
        return Err(format!("unknown/off acquired numeric consumption: {genai}"));
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
        if genai["attempts"][0]["failure"] != failure {
            return Err(format!("wrong provider failure: {genai}"));
        }
    }
    Ok(())
}
async fn observation(response: reqwest::Response, stream: bool) -> Result<Value, String> {
    if response.status() != 200 {
        return Err(format!("native status {}", response.status()));
    }
    let text = response.text().await.map_err(|e| e.to_string())?;
    if stream {
        if text.contains("[DONE]") || !text.contains("event: message_stop") {
            return Err("native event framing changed".into());
        }
        for line in text.lines().filter_map(|l| l.strip_prefix("data: ")) {
            let event: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
            if event["type"] == "content_block_delta" && event["delta"]["type"] == "text_delta" {
                return serde_json::from_str(event["delta"]["text"].as_str().ok_or("missing text")?)
                    .map_err(|e| e.to_string());
            }
        }
        Err("native content delta missing".into())
    } else {
        let message: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        if message["type"] != "message" {
            return Err("native message shape changed".into());
        }
        serde_json::from_str(message["content"][0]["text"].as_str().ok_or("missing content")?)
            .map_err(|e| e.to_string())
    }
}
async fn native(ctx: &TestContext, h2: bool) -> Result<(), String> {
    for (host, source) in [
        ("messages-off.example.com", "notObserved"),
        ("messages-provider.example.com", "reported"),
    ] {
        for stream in [false, true] {
            let marker = format!("messages-native-{h2}-{stream}-{source}-{}", uuid::Uuid::new_v4());
            let request = body(stream);
            let observed = observation(
                post(
                    ctx,
                    host,
                    &request,
                    "split",
                    h2,
                    &marker,
                    "/v1/messages",
                    "2023-06-01",
                    false,
                )
                .await?,
                stream,
            )
            .await?;
            if observed["request"] != request
                || observed["path"] != "native/v1/messages"
                || observed["query"]["fixture_query"] != "preserved"
                || observed["credential_label"] != "A"
                || observed["auth_absent"] != true
                || observed["workspace_absent"] != true
                || observed["beta"] != json!(["first-beta", "second-beta"])
                || observed["version"] != "2023-06-01"
            {
                return Err(format!("native request/header isolation failed: {observed}"));
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
            for stream in [false, true] {
                for (mode, source) in [
                    ("normal", "reported"),
                    ("pause", "reported"),
                    ("iterations", "reported"),
                    ("composite", "unknown"),
                    ("missing_usage", "unknown"),
                    ("malformed", "unknown"),
                ] {
                    let marker = format!("messages-outcome-{mode}-{stream}-{}", uuid::Uuid::new_v4());
                    let response = post(
                        &ctx,
                        "messages-provider.example.com",
                        &body(stream),
                        mode,
                        false,
                        &marker,
                        "/v1/messages",
                        "2023-06-01",
                        false,
                    )
                    .await?;
                    if response.status() != 200 {
                        return Err(format!("passive native observation cut {mode}"));
                    }
                    let _ = response.text().await.map_err(|e| e.to_string())?;
                    ledger(&ctx, &marker, source, None).await?;
                }
            }
            let marker = format!("messages-sse-provider-error-{}", uuid::Uuid::new_v4());
            let text = post(
                &ctx,
                "messages-provider.example.com",
                &body(true),
                "error",
                false,
                &marker,
                "/v1/messages",
                "2023-06-01",
                false,
            )
            .await?
            .text()
            .await
            .map_err(|e| e.to_string())?;
            if !text.contains("overloaded_error") {
                return Err("provider native error changed".into());
            }
            ledger(&ctx, &marker, "unknown", Some("provider_failed")).await?;
            for (host, source) in [
                ("messages-off.example.com", "notObserved"),
                ("messages-provider.example.com", "unknown"),
            ] {
                let marker = format!("messages-unsupported-version-{source}-{}", uuid::Uuid::new_v4());
                let observed = observation(
                    post(
                        &ctx,
                        host,
                        &body(false),
                        "normal",
                        false,
                        &marker,
                        "/v1/messages",
                        "future-version",
                        false,
                    )
                    .await?,
                    false,
                )
                .await?;
                if observed["version"] != "future-version" {
                    return Err("native version rewritten".into());
                }
                ledger(&ctx, &marker, source, None).await?;
            }
            for stream in [false, true] {
                let marker = format!("messages-missing-off-{stream}-{}", uuid::Uuid::new_v4());
                let response = post(
                    &ctx,
                    "messages-off.example.com",
                    &body(stream),
                    "missing_usage",
                    false,
                    &marker,
                    "/v1/messages",
                    "2023-06-01",
                    false,
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
fn rejects(ctx: TestContext) -> TestFuture {
    Box::pin(async move {
        let start = Instant::now();
        let result = async {
            let mut cases = vec![
                (
                    "messages-provider.example.com",
                    "/v1/models",
                    body(false),
                    "2023-06-01",
                    false,
                ),
                (
                    "messages-provider.example.com",
                    "/v1/messages",
                    body(false),
                    "2023-06-01",
                    true,
                ),
                (
                    "messages-quota.example.com",
                    "/v1/messages",
                    body(false),
                    "future",
                    false,
                ),
            ];
            let mut invalid = body(false);
            invalid["max_tokens"] = json!("16");
            cases.push((
                "messages-provider.example.com",
                "/v1/messages",
                invalid,
                "2023-06-01",
                false,
            ));
            let mut composite = body(false);
            composite["fallbacks"] = json!("default");
            cases.push((
                "messages-quota.example.com",
                "/v1/messages",
                composite,
                "2023-06-01",
                false,
            ));
            for host in [
                "messages-guard-before.example.com",
                "messages-guard-after.example.com",
                "messages-guard-rewrite.example.com",
            ] {
                cases.push((
                    host,
                    if host.ends_with("rewrite.example.com") {
                        "/custom/messages"
                    } else {
                        "/v1/messages"
                    },
                    body(false),
                    "2023-06-01",
                    false,
                ));
            }
            let mut state = body(false);
            state["messages"] =
                json!([{"role":"assistant","content":[{"type":"thinking","thinking":"fixture","signature":"opaque"}]}]);
            cases.push((
                "messages-multi.example.com",
                "/v1/messages",
                state.clone(),
                "2023-06-01",
                false,
            ));
            state["model"] = json!("fixture-alias");
            cases.push((
                "messages-provider.example.com",
                "/v1/messages",
                state,
                "2023-06-01",
                false,
            ));
            for (i, (host, path, body, version, profile)) in cases.into_iter().enumerate() {
                let response = post(
                    &ctx,
                    host,
                    &body,
                    "normal",
                    false,
                    &format!("messages-reject-{i}-{}", uuid::Uuid::new_v4()),
                    path,
                    version,
                    profile,
                )
                .await?;
                if response.status() != 400 && response.status() != 404 {
                    return Err(format!("case {i}: unexpected status {}", response.status()));
                }
                let error: Value = response.json().await.map_err(|e| e.to_string())?;
                if error["type"] != "error" || !error["error"]["type"].is_string() {
                    return Err(format!("wrong client dialect: {error}"));
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
async fn binding(ctx: &TestContext, allowed: bool, label: &str) -> Result<(), String> {
    let mut request = body(false);
    request["container"] = json!("container-fixture");
    request["messages"] = json!([{"role":"assistant","content":[{"type":"thinking","thinking":"fixture","signature":"opaque-fixture-signature"}]},{"role":"user","content":"continue"}]);
    let response = post(
        ctx,
        "messages-provider.example.com",
        &request,
        "normal",
        false,
        &format!("messages-binding-{allowed}-{label}-{}", uuid::Uuid::new_v4()),
        "/v1/messages",
        "2023-06-01",
        false,
    )
    .await?;
    if allowed {
        let observed = observation(response, false).await?;
        if observed["request"] != request || observed["credential_label"] != label {
            return Err(format!("binding/principal snapshot changed: {observed}"));
        }
    } else {
        if response.status() != 400 {
            return Err(format!("ambiguous native state forwarded: {}", response.status()));
        }
        let ordinary = observation(
            post(
                ctx,
                "messages-provider.example.com",
                &body(false),
                "normal",
                false,
                &format!("messages-stateless-after-update-{}", uuid::Uuid::new_v4()),
                "/v1/messages",
                "2023-06-01",
                false,
            )
            .await?,
            false,
        )
        .await?;
        if ordinary["credential_label"] != "A" {
            return Err("unresolved declaration disabled ordinary access".into());
        }
    }
    Ok(())
}
fn binding_allowed(ctx: TestContext) -> TestFuture {
    Box::pin(async move {
        let start = Instant::now();
        match binding(&ctx, true, "A").await {
            Ok(()) => TestResult::passed(start.elapsed()),
            Err(e) => TestResult::failed(start.elapsed(), e),
        }
    })
}
fn binding_rejected(ctx: TestContext) -> TestFuture {
    Box::pin(async move {
        let start = Instant::now();
        match binding(&ctx, false, "A").await {
            Ok(()) => TestResult::passed(start.elapsed()),
            Err(e) => TestResult::failed(start.elapsed(), e),
        }
    })
}
fn credential_b(ctx: TestContext) -> TestFuture {
    Box::pin(async move {
        let start = Instant::now();
        match binding(&ctx, true, "B").await {
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
                let host = format!("messages-{observation}.example.com");
                let source = if observation == "off" { "notObserved" } else { "unknown" };
                for h2 in [false, true] {
                    let marker = format!("messages-cancel-{observation}-{h2}-{}", uuid::Uuid::new_v4());
                    let response = post(
                        &ctx,
                        &host,
                        &body(true),
                        "long",
                        h2,
                        &marker,
                        "/v1/messages",
                        "2023-06-01",
                        false,
                    )
                    .await?;
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
                        if text.contains("event: message_stop") {
                            return Err("terminal preceded cancellation".into());
                        }
                        if text.contains("\"type\":\"text_delta\"") && text.ends_with("\n\n") {
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
                                    || !(4..1000).contains(&count)
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
                    let marker = format!("messages-abort-{observation}-{h2}-{}", uuid::Uuid::new_v4());
                    let response = post(
                        &ctx,
                        &host,
                        &body(true),
                        "abort",
                        h2,
                        &marker,
                        "/v1/messages",
                        "2023-06-01",
                        false,
                    )
                    .await?;
                    if response.status() != 200 {
                        return Err("abort stream not committed as 200".into());
                    }
                    if let Ok(bytes) = response.bytes().await {
                        if bytes.is_empty() || String::from_utf8_lossy(&bytes).contains("event: message_stop") {
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

fn active_quota(ctx: TestContext) -> TestFuture {
    Box::pin(async move {
        let start = Instant::now();
        let result = async {
            for h2 in [false, true] {
                let marker = format!("messages-quota-wire-{h2}-{}", uuid::Uuid::new_v4());
                let mut request = body(true);
                request["max_tokens"] = json!(64);
                let response = post(
                    &ctx,
                    "messages-quota.example.com",
                    &request,
                    "quota",
                    h2,
                    &marker,
                    "/v1/messages",
                    "2023-06-01",
                    false,
                )
                .await?;
                if response.status() != 200 {
                    return Err(format!("quota terminated before stream: {}", response.status()));
                }
                let text = response.text().await.map_err(|e| e.to_string())?;
                if !text.contains("event: error\n") || text.contains("[DONE]") || text.contains("event: message_stop") {
                    return Err(format!("wrong native Reject framing: {text}"));
                }
                let tail = text
                    .lines()
                    .filter_map(|line| line.strip_prefix("data: "))
                    .next_back()
                    .ok_or("missing error payload")?;
                let error: Value = serde_json::from_str(tail).map_err(|e| e.to_string())?;
                if error["type"] != "error" || error["error"]["type"] != "rate_limit_error" {
                    return Err(format!("wrong quota native error: {error}"));
                }
                ledger(&ctx, &marker, "unknown", None).await?;
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
#[async_trait]
impl TestSuite for MessagesTestSuite {
    fn name(&self) -> &str {
        "EdgionBackend/Messages"
    }
    fn port_key(&self) -> Option<&str> {
        Some("EdgionBackend/Messages")
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            TestCase::new("active_quota", "H1/H2 native committed quota Reject", active_quota),
            TestCase::new(
                "h1_native",
                "H1 native JSON/SSE, byte preservation and provider isolation",
                h1,
            ),
            TestCase::new("h2_native", "H2 native JSON/SSE", h2),
            TestCase::new(
                "outcomes",
                "Late cache usage, composite and unsupported-version unknown",
                outcomes,
            ),
            TestCase::new(
                "binding",
                "State-sensitive history on one declared binding",
                binding_allowed,
            ),
            TestCase::new(
                "rejects",
                "Scoped native errors and capability/binding/accounting fences",
                rejects,
            ),
            TestCase::new(
                "termination",
                "Source abort and client cancellation retain unknown usage",
                termination,
            ),
        ]
    }
}
#[async_trait]
impl TestSuite for MessagesBindingAllowedSuite {
    fn name(&self) -> &str {
        "MessagesBindingAllowed"
    }
    fn port_key(&self) -> Option<&str> {
        Some("EdgionBackend/Messages")
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "binding",
            "native binding and credential A",
            binding_allowed,
        )]
    }
}
#[async_trait]
impl TestSuite for MessagesBindingRejectedSuite {
    fn name(&self) -> &str {
        "MessagesBindingRejected"
    }
    fn port_key(&self) -> Option<&str> {
        Some("EdgionBackend/Messages")
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "binding",
            "extra raw declaration rejects native state only",
            binding_rejected,
        )]
    }
}
#[async_trait]
impl TestSuite for MessagesCredentialBSuite {
    fn name(&self) -> &str {
        "MessagesCredentialB"
    }
    fn port_key(&self) -> Option<&str> {
        Some("EdgionBackend/Messages")
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "binding",
            "native binding and credential B",
            credential_b,
        )]
    }
}
