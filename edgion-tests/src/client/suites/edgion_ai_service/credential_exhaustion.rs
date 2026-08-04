//! Credential-first scheduling, RPM exhaustion, and TPM reconciliation.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use edgion_gateway::plugins::common::redis_rate_limiter::build_credential_reservation_keys;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct CredentialExhaustionTestSuite;

const HOST: &str = "ai-credential-exhaustion.example.com";
const PATH: &str = "/v1/chat/completions";
const KEY_PREFIX: &str = "edgion:aic:";
const REDIS_NAME: &str = "default_credential-exhaustion-redis";
const SAFE_ATTEMPT_FIELDS: [&str; 12] = [
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

struct CredentialBucket {
    service: &'static str,
    alias: &'static str,
    keys: Vec<String>,
}

fn gateway_client(ctx: &TestContext) -> Result<reqwest::Client, String> {
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
        .resolve(HOST, SocketAddr::new(target_ip, ctx.http_port))
        .build()
        .map_err(|error| format!("failed to build credential-exhaustion client: {error}"))
}

fn admin_base(ctx: &TestContext) -> String {
    std::env::var("EDGION_GATEWAY_ADMIN_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("http://{}:12001", ctx.target_host))
        .trim_end_matches('/')
        .to_string()
}

fn authed(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match std::env::var("EDGION_GATEWAY_ADMIN_TOKEN") {
        Ok(token) if !token.trim().is_empty() => request.bearer_auth(token),
        _ => request,
    }
}

fn even_trace_id() -> String {
    let mut bytes = *uuid::Uuid::new_v4().as_bytes();
    bytes[15] &= 0xfe;
    uuid::Uuid::from_bytes(bytes).to_string()
}

async fn provider_observations(ctx: &TestContext, trace: &str) -> Result<Value, String> {
    ctx.http_client
        .get(format!(
            "http://{}:30001/ai-credential-exhaustion/observations/{trace}",
            ctx.target_host
        ))
        .send()
        .await
        .map_err(|error| format!("provider observation request failed: {error}"))?
        .json()
        .await
        .map_err(|error| format!("invalid provider observation response: {error}"))
}

async fn service_uid(ctx: &TestContext, name: &str) -> Result<String, String> {
    let response = authed(
        ctx.http_client
            .get(format!("{}/configclient/edgionaiservice/list", admin_base(ctx))),
    )
    .send()
    .await
    .map_err(|error| format!("AI service configclient request failed: {error}"))?;
    let status = response.status();
    let listed: Value = response
        .json()
        .await
        .map_err(|error| format!("invalid AI service configclient response: {error}"))?;
    if !status.is_success() {
        return Err(format!("AI service configclient returned {status}: {listed}"));
    }
    let service = listed["data"]
        .as_array()
        .and_then(|items| {
            items.iter().find(|item| {
                item.pointer("/metadata/namespace").and_then(Value::as_str) == Some("edgion-test")
                    && item.pointer("/metadata/name").and_then(Value::as_str) == Some(name)
            })
        })
        .ok_or_else(|| format!("AI service edgion-test/{name} is not synced: {listed}"))?;
    Ok(service
        .pointer("/metadata/uid")
        .and_then(Value::as_str)
        .filter(|uid| !uid.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("edgion.io/EdgionAIService/edgion-test/{name}")))
}

async fn prepare_buckets(ctx: &TestContext) -> Result<Vec<CredentialBucket>, String> {
    let primary_uid = service_uid(ctx, "credential-exhaustion-primary").await?;
    let secondary_uid = service_uid(ctx, "credential-exhaustion-secondary").await?;
    let window = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock is before the Unix epoch: {error}"))?
        .as_secs()
        / 60;
    let random_inspection_id = uuid::Uuid::new_v4().to_string();
    let mut buckets = Vec::new();
    for (service, uid, alias) in [
        (
            "edgion-test/credential-exhaustion-primary",
            primary_uid.as_str(),
            "primary-a",
        ),
        (
            "edgion-test/credential-exhaustion-primary",
            primary_uid.as_str(),
            "primary-b",
        ),
        (
            "edgion-test/credential-exhaustion-secondary",
            secondary_uid.as_str(),
            "secondary-a",
        ),
    ] {
        let mut keys = Vec::new();
        for candidate_window in window.saturating_sub(1)..=window.saturating_add(1) {
            let derived = build_credential_reservation_keys(
                KEY_PREFIX,
                b"edgion-test",
                uid.as_bytes(),
                alias.as_bytes(),
                candidate_window,
                random_inspection_id.as_bytes(),
            )
            .map_err(|error| format!("failed to derive credential bucket: {error}"))?;
            keys.push(derived.bucket);
        }
        buckets.push(CredentialBucket { service, alias, keys });
    }
    let all_keys = buckets
        .iter()
        .flat_map(|bucket| bucket.keys.iter().cloned())
        .collect::<Vec<_>>();
    let response = authed(
        ctx.http_client
            .post(format!(
                "{}/api/v1/testing/link-sys/redis/{REDIS_NAME}/del",
                admin_base(ctx)
            ))
            .json(&json!({ "keys": all_keys })),
    )
    .send()
    .await
    .map_err(|error| format!("credential bucket cleanup failed: {error}"))?;
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .map_err(|error| format!("invalid credential bucket cleanup response: {error}"))?;
    if !status.is_success() || body["success"] != true {
        return Err(format!("credential bucket cleanup returned {status}: {body}"));
    }
    Ok(buckets)
}

async fn bucket_usage(ctx: &TestContext, bucket: &CredentialBucket) -> Result<(u64, u64), String> {
    for key in &bucket.keys {
        let encoded = utf8_percent_encode(key, NON_ALPHANUMERIC);
        let response = authed(ctx.http_client.get(format!(
            "{}/api/v1/testing/link-sys/redis/{REDIS_NAME}/hgetall/{encoded}",
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
        let fields = &body["data"]["fields"];
        let rpm = fields["rpm"].as_str().and_then(|value| value.parse::<u64>().ok());
        let tpm = fields["tpm"].as_str().and_then(|value| value.parse::<u64>().ok());
        if let (Some(rpm), Some(tpm)) = (rpm, tpm) {
            return Ok((rpm, tpm));
        }
    }
    Err(format!(
        "no active Redis bucket found for {} {}",
        bucket.service, bucket.alias
    ))
}

async fn post(ctx: &TestContext, trace: &str) -> Result<(reqwest::StatusCode, Value), String> {
    let response = gateway_client(ctx)?
        .post(format!("http://{HOST}:{}{PATH}", ctx.http_port))
        .header("x-trace-id", trace)
        .header("access_log", "test_store")
        .json(&json!({
            "model": "fixture-model",
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "credential exhaustion fixture"}]
        }))
        .send()
        .await
        .map_err(|error| format!("credential-exhaustion request failed: {error}"))?;
    let status = response.status();
    let body = response
        .json()
        .await
        .map_err(|error| format!("invalid credential-exhaustion response: {error}"))?;
    Ok((status, body))
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
        "ai-provider-test-key",
        "ai-provider-rotated-test-key",
        "credential exhaustion fixture",
        "127.0.0.1:30001",
    ] {
        if rendered.contains(forbidden) {
            return Err(format!("attempt ledger leaked forbidden fixture {forbidden:?}"));
        }
    }
    Ok(attempts)
}

fn credential_first_then_service_next(
    ctx: TestContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = TestResult> + Send>> {
    Box::pin(async move {
        let start = Instant::now();
        let buckets = match prepare_buckets(&ctx).await {
            Ok(buckets) => buckets,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let first_trace = even_trace_id();
        let (status, first_body) = match post(&ctx, &first_trace).await {
            Ok(result) => result,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if status != reqwest::StatusCode::SERVICE_UNAVAILABLE {
            return TestResult::failed(
                start.elapsed(),
                format!("expected final provider 503, got {status}: {first_body}"),
            );
        }
        let observations = match provider_observations(&ctx, &first_trace).await {
            Ok(value) => value,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let expected_observations = json!([
            {"service":"primary","credential_generation":"a"},
            {"service":"primary","credential_generation":"b"},
            {"service":"secondary","credential_generation":"a"}
        ]);
        if observations["observations"] != expected_observations {
            return TestResult::failed(
                start.elapsed(),
                format!("unexpected provider dispatch order: {observations}"),
            );
        }

        let entry = match ctx
            .access_log_client()
            .get_access_log_with_retry(&first_trace, 20, 200)
            .await
        {
            Ok(entry) => entry,
            Err(error) => return TestResult::failed(start.elapsed(), format!("first access log missing: {error}")),
        };
        let attempts = match safe_attempts(&entry.data["genai"]) {
            Ok(attempts) => attempts,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let expected_targets = [
            ("edgion-test/credential-exhaustion-primary", "primary-a"),
            ("edgion-test/credential-exhaustion-primary", "primary-b"),
            ("edgion-test/credential-exhaustion-secondary", "secondary-a"),
        ];
        if attempts.len() != expected_targets.len() {
            return TestResult::failed(start.elapsed(), format!("expected three attempts, got {attempts:?}"));
        }
        for (index, (service, alias)) in expected_targets.iter().enumerate() {
            let failure_ok = if index < 2 {
                attempts[index]["failure"] == "retryable_status"
            } else {
                attempts[index]["failure"].is_null()
            };
            if attempts[index]["service"] != *service
                || attempts[index]["credential"] != *alias
                || attempts[index]["provider"] != "openai_compatible"
                || attempts[index]["status"] != 503
                || !failure_ok
            {
                return TestResult::failed(start.elapsed(), format!("unexpected attempt ledger: {attempts:?}"));
            }
        }
        for (index, bucket) in buckets.iter().enumerate() {
            let (rpm, tpm) = match bucket_usage(&ctx, bucket).await {
                Ok(usage) => usage,
                Err(error) => return TestResult::failed(start.elapsed(), error),
            };
            let charged = attempts[index]["input_tokens"].as_u64().unwrap_or_default()
                + attempts[index]["output_tokens"].as_u64().unwrap_or_default();
            if rpm != 1 || tpm != charged || charged == 0 {
                return TestResult::failed(
                    start.elapsed(),
                    format!(
                        "unexpected RPM/TPM ledger for {} {}: rpm={rpm}, tpm={tpm}, charged={charged}",
                        bucket.service, bucket.alias
                    ),
                );
            }
        }

        let second_trace = even_trace_id();
        let (second_status, second_body) = match post(&ctx, &second_trace).await {
            Ok(result) => result,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if second_status != reqwest::StatusCode::SERVICE_UNAVAILABLE
            || second_body["error"]["type"] != "api_error"
            || second_body["error"]["code"] != "service_unavailable"
            || second_body["error"]["message"] != "The AI service is temporarily unavailable."
        {
            return TestResult::failed(
                start.elapsed(),
                format!("expected fixed local service-unavailable response, got {second_status}: {second_body}"),
            );
        }
        let second_observations = match provider_observations(&ctx, &second_trace).await {
            Ok(value) => value,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        if second_observations["observations"]
            .as_array()
            .is_none_or(|items| !items.is_empty())
        {
            return TestResult::failed(
                start.elapsed(),
                format!("exhausted credentials still reached a provider: {second_observations}"),
            );
        }
        TestResult::passed_with_message(
            start.elapsed(),
            "credential-first dispatch used primary-a then primary-b before the secondary service; RPM was exhausted, TPM was reconciled, and the follow-up failed locally with a fixed 503"
                .to_string(),
        )
    })
}

#[async_trait]
impl TestSuite for CredentialExhaustionTestSuite {
    fn name(&self) -> &str {
        "EdgionAIService/CredentialExhaustion"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionAIService/CredentialExhaustion")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "credential_first_then_service_next",
            "Credential exhaustion rotates within one AI service before advancing to the next service",
            credential_first_then_service_next,
        )]
    }
}
