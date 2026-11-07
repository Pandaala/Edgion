//! Endpoint-scoped failover, RPM exhaustion, and TPM reconciliation.

use super::gateway_admin::{admin_base, authed, backend_uid, CREDENTIAL_RESERVATION_KEY_PREFIX as KEY_PREFIX};
use crate::framework::{test_backend_hosts, TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use edgion_gateway::plugins::common::redis_rate_limiter::build_credential_reservation_keys;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct CredentialExhaustionTestSuite;

const HOST: &str = "ai-credential-exhaustion.example.com";
const PATH: &str = "/v1/chat/completions";
const LOCAL_REDIS_NAME: &str = "default_credential-exhaustion-redis";
const K8S_REDIS_NAME: &str = "edgion-test_credential-exhaustion-redis";
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

fn redis_name() -> &'static str {
    if std::env::var("EDGION_TEST_K8S_MODE").as_deref() == Ok("true") {
        K8S_REDIS_NAME
    } else {
        LOCAL_REDIS_NAME
    }
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

fn request_trace_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

async fn provider_observations(ctx: &TestContext, trace: &str) -> Result<Value, String> {
    let hosts = test_backend_hosts(ctx);
    let aggregate = hosts.len() > 1;
    let trace = crate::access_log_client::trace_id_for_marker(trace);
    let mut observations = Vec::new();
    for host in hosts {
        let response = ctx
            .http_client
            .get(format!(
                "http://{host}:30001/ai-credential-exhaustion/observations/{trace}"
            ))
            .send()
            .await
            .map_err(|error| format!("provider observation request failed for {host}: {error}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("provider observation request for {host} returned {status}"));
        }
        let value = response
            .json::<Value>()
            .await
            .map_err(|error| format!("invalid provider observation response from {host}: {error}"))?;
        let Some(items) = value["observations"].as_array() else {
            return Err(format!(
                "provider observation response from {host} has no observations array: {value}"
            ));
        };
        observations.extend(items.iter().cloned());
    }
    if aggregate {
        observations.sort_by(|left, right| {
            left["backend"].as_str().cmp(&right["backend"].as_str()).then_with(|| {
                left["credential_generation"]
                    .as_str()
                    .cmp(&right["credential_generation"].as_str())
            })
        });
    }
    Ok(json!({"observations": observations}))
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
                "{}/api/v1/testing/link-sys/redis/{}/del",
                admin_base(ctx),
                redis_name()
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
            "{}/api/v1/testing/link-sys/redis/{}/hgetall/{encoded}",
            admin_base(ctx),
            redis_name()
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
        .header("traceparent", crate::access_log_client::traceparent_for_marker(trace))
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

fn endpoint_failover_then_credential_exhaustion(
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
        let first_trace = request_trace_id();
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
        let second_trace = request_trace_id();
        let (second_status, second_body) = match post(&ctx, &second_trace).await {
            Ok(result) => result,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        let observations = match provider_observations(&ctx, &first_trace).await {
            Ok(value) => value,
            Err(error) => return TestResult::failed(start.elapsed(), error),
        };
        // Multi-replica observations are merged and sorted, so only the set is
        // checked here; the access-log attempt ledger below checks the order.
        let observed = observations["observations"].as_array();
        let observations_ok = observed.is_some_and(|items| {
            let mut seen: Vec<_> = items
                .iter()
                .map(|item| (item["backend"].as_str(), item["credential_generation"].as_str()))
                .collect();
            seen.sort();
            seen == [
                (Some("primary"), Some("a")),
                (Some("primary"), Some("b")),
                (Some("secondary"), Some("a")),
            ]
        });
        if !observations_ok {
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
        if attempts.len() != 3 {
            return TestResult::failed(start.elapsed(), format!("expected three attempts, got {attempts:?}"));
        }
        let primary_credentials = [attempts[0]["credential"].as_str(), attempts[2]["credential"].as_str()];
        if !primary_credentials.contains(&Some("primary-a")) || !primary_credentials.contains(&Some("primary-b")) {
            return TestResult::failed(
                start.elapsed(),
                format!("primary credentials were not each attempted once: {attempts:?}"),
            );
        }
        // A provider 503 is endpoint-scoped: the first retry fails over to the
        // secondary backend, and the next round returns to the primary backend
        // on its untried credential because the first one exhausted its RPM.
        let expected_backends = [
            "edgion-test/credential-exhaustion-primary",
            "edgion-test/credential-exhaustion-secondary",
            "edgion-test/credential-exhaustion-primary",
        ];
        for (index, backend) in expected_backends.iter().enumerate() {
            let failure_ok = if index < 2 {
                attempts[index]["failure"] == "retryable_status"
            } else {
                attempts[index]["failure"].is_null()
            };
            if attempts[index]["backend"] != *backend
                || (index == 1 && attempts[index]["credential"] != "secondary-a")
                || attempts[index]["provider"] != "openai_compatible"
                || attempts[index]["status"] != 503
                || !failure_ok
            {
                return TestResult::failed(start.elapsed(), format!("unexpected attempt ledger: {attempts:?}"));
            }
        }
        for bucket in &buckets {
            let (rpm, tpm) = match bucket_usage(&ctx, bucket).await {
                Ok(usage) => usage,
                Err(error) => return TestResult::failed(start.elapsed(), error),
            };
            let Some(attempt) = attempts
                .iter()
                .find(|attempt| attempt["backend"] == bucket.backend && attempt["credential"] == bucket.alias)
            else {
                return TestResult::failed(
                    start.elapsed(),
                    format!(
                        "no attempt ledger entry found for {} {}: {attempts:?}",
                        bucket.backend, bucket.alias
                    ),
                );
            };
            let charged = attempt["input_tokens"].as_u64().unwrap_or_default()
                + attempt["output_tokens"].as_u64().unwrap_or_default();
            // Every attempt was a provider 503: the request still counts against
            // RPM, while TPM reconciles to zero because nothing was consumed.
            if rpm != 1 || tpm != 0 || charged != 0 {
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
            "endpoint-scoped 503 failed over to the secondary backend before the next round used the untried primary credential; RPM was exhausted, TPM was reconciled, and the follow-up failed locally with a fixed 503"
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
            "endpoint_failover_then_credential_exhaustion",
            "An endpoint-scoped provider failure fails over to the next AI backend before a new round skips the RPM-exhausted credential",
            endpoint_failover_then_credential_exhaustion,
        )
        .with_timeout(Duration::from_secs(90))]
    }
}
