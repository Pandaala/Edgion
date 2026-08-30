//! Credential-first scheduling, RPM exhaustion, and TPM reconciliation.

use super::gateway_admin::{admin_base, authed, backend_uid, CREDENTIAL_RESERVATION_KEY_PREFIX as KEY_PREFIX};
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
const REDIS_NAME: &str = "default_credential-exhaustion-redis";
const MIN_QUOTA_WINDOW_REMAINING: Duration = Duration::from_secs(25);
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

struct CredentialBucket {
    backend: &'static str,
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

async fn prepare_buckets(ctx: &TestContext) -> Result<Vec<CredentialBucket>, String> {
    let primary_uid = backend_uid(ctx, "edgion-test", "credential-exhaustion-primary").await?;
    let secondary_uid = backend_uid(ctx, "edgion-test", "credential-exhaustion-secondary").await?;
    let window = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock is before the Unix epoch: {error}"))?
        .as_secs()
        / 60;
    let random_inspection_id = uuid::Uuid::new_v4().to_string();
    let mut buckets = Vec::new();
    for (backend, uid, alias) in [
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
        buckets.push(CredentialBucket { backend, alias, keys });
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

async fn wait_for_stable_quota_window() -> Result<(), String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock is before the Unix epoch: {error}"))?;
    let elapsed_in_window = Duration::new(now.as_secs() % 60, now.subsec_nanos());
    let remaining = Duration::from_secs(60).saturating_sub(elapsed_in_window);
    if remaining < MIN_QUOTA_WINDOW_REMAINING {
        tokio::time::sleep(remaining.saturating_add(Duration::from_millis(100))).await;
    }
    Ok(())
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
        bucket.backend, bucket.alias
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
        if let Err(error) = wait_for_stable_quota_window().await {
            return TestResult::failed(start.elapsed(), error);
        }
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
        let second_trace = even_trace_id();
        let (second_status, second_body) = match post(&ctx, &second_trace).await {
            Ok(result) => result,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let observations = match provider_observations(&ctx, &first_trace).await {
            Ok(value) => value,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let expected_observations = json!([
            {"backend":"primary","credential_generation":"a"},
            {"backend":"primary","credential_generation":"b"},
            {"backend":"secondary","credential_generation":"a"}
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
        for (index, (backend, alias)) in expected_targets.iter().enumerate() {
            let failure_ok = if index < 2 {
                attempts[index]["failure"] == "retryable_status"
            } else {
                attempts[index]["failure"].is_null()
            };
            if attempts[index]["backend"] != *backend
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
                        bucket.backend, bucket.alias
                    ),
                );
            }
        }

        if second_status != reqwest::StatusCode::SERVICE_UNAVAILABLE
            || second_body["error"]["type"] != "api_error"
            || second_body["error"]["code"] != "service_unavailable"
            || second_body["error"]["message"] != "The AI backend is temporarily unavailable."
        {
            return TestResult::failed(
                start.elapsed(),
                format!("expected fixed local backend-unavailable response, got {second_status}: {second_body}"),
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
            "credential-first dispatch used primary-a then primary-b before the secondary backend; RPM was exhausted, TPM was reconciled, and the follow-up failed locally with a fixed 503"
                .to_string(),
        )
    })
}

#[async_trait]
impl TestSuite for CredentialExhaustionTestSuite {
    fn name(&self) -> &str {
        "EdgionBackend/CredentialExhaustion"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionBackend/CredentialExhaustion")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "credential_first_then_service_next",
            "Credential exhaustion rotates within one AI backend before advancing to the next backend",
            credential_first_then_service_next,
        )
        .with_timeout(Duration::from_secs(90))]
    }
}
