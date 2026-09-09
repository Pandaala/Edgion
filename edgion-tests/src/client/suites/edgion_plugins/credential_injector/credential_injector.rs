use std::time::Instant;

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use serde_json::Value;
use tokio::time::{sleep, Duration};

const BEARER_HOST: &str = "credential-injector-bearer.example.com";
const CUSTOM_HOST: &str = "credential-injector-custom.example.com";
const LATER_REWRITE_HOST: &str = "credential-injector-later-rewrite.example.com";
const LATER_ROUTE_MODIFIER_HOST: &str = "credential-injector-later-route-modifier.example.com";
const DOUBLE_INJECTOR_HOST: &str = "credential-injector-double.example.com";
const MISSING_HOST: &str = "credential-injector-missing.example.com";
const MIRROR_HOST: &str = "credential-injector-mirror.example.com";
const DYNAMIC_HOST: &str = "credential-injector-dynamic.example.com";
const CONNECTION_CONFLICT_HOST: &str = "credential-connection-conflict.example.com";
const CONNECTION_CONTROL_HOST: &str = "credential-connection-control.example.com";

async fn connection_request(
    ctx: &TestContext,
    host: &str,
    path: &str,
    trace_id: &str,
) -> Result<(u16, String), &'static str> {
    let mut request = ctx
        .http_client
        .get(format!("{}{path}", ctx.http_url()))
        .version(reqwest::Version::HTTP_11)
        .header("host", host)
        .header("x-trace-id", trace_id)
        .timeout(Duration::from_secs(2));
    if host == CONNECTION_CONFLICT_HOST {
        request = request.header("access_log", "test_store");
    }
    let response = request
        .send()
        .await
        .map_err(|_| "Connection nomination request failed")?;
    if response.version() != reqwest::Version::HTTP_11 {
        return Err("Connection nomination case did not use HTTP/1.1");
    }
    let status = response.status().as_u16();
    let body = response
        .text()
        .await
        .map_err(|_| "Connection nomination response read failed")?;
    Ok((status, body))
}

async fn connection_captures(ctx: &TestContext, trace_id: &str) -> Result<Vec<Value>, &'static str> {
    let mut captures = Vec::new();
    for host in mirror_backend_hosts(ctx) {
        let response = ctx
            .http_client
            .get(format!("http://{host}:30001/mirror/query/{trace_id}"))
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .map_err(|_| "Backend capture query failed")?
            .error_for_status()
            .map_err(|_| "Backend capture query returned an error status")?;
        let data: Value = response
            .json()
            .await
            .map_err(|_| "Backend capture response was not JSON")?;
        let entries = data["captures"]
            .as_array()
            .ok_or("Backend capture response omitted captures")?;
        if data["count"].as_u64() != Some(entries.len() as u64) || data["found"].as_bool() != Some(!entries.is_empty())
        {
            return Err("Backend capture response had inconsistent counts");
        }
        captures.extend(entries.iter().cloned());
    }
    Ok(captures)
}

async fn check_connection_nomination(ctx: &TestContext, traces: &[String; 5]) -> Result<(), &'static str> {
    wait_connection_endpoint(ctx, &traces[4]).await?;
    // Prove the exact Secret generation and the observation path work first.
    let (status, _) = connection_request(ctx, CONNECTION_CONTROL_HOST, "/mirror/capture", &traces[0]).await?;
    if status != 200 {
        return Err("Credential connection control request was not successful");
    }
    let captures = connection_captures(ctx, &traces[0]).await?;
    if captures.len() != 1 || captures[0]["headers"]["x-provider-key"].as_str() != Some("Token static-provider-token") {
        return Err("Credential connection control did not capture the expected generation");
    }

    // Finish rejection and availability checks before the five-second ejection
    // window can expire. A stalled test must fail, not hide a transient ejection.
    tokio::time::timeout(Duration::from_secs(3), async {
        for trace in &traces[1..=3] {
            let (status, body) = connection_request(ctx, CONNECTION_CONFLICT_HOST, "/mirror/capture", trace).await?;
            if status != 500 {
                return Err("Late Connection nomination did not fail closed with 500");
            }
            let lower = body.to_ascii_lowercase();
            if [
                "static-provider-token",
                "x-provider-key",
                "credentialinjector",
                "credential_injector",
            ]
            .iter()
            .any(|secret_detail| lower.contains(secret_detail))
            {
                return Err("Connection rejection response exposed credential detail");
            }
        }
        let (status, _) = connection_request(ctx, CONNECTION_CONTROL_HOST, "/health", &traces[4]).await?;
        if status != 200 {
            return Err("Local credential rejection penalized the selected endpoint");
        }
        Ok(())
    })
    .await
    .map_err(|_| "Connection rejection checks exceeded the ejection observation deadline")??;

    let logs = ctx.access_log_client();
    for trace in &traces[1..=3] {
        let log = logs
            .get_access_log_with_retry(trace, 10, 100)
            .await
            .map_err(|_| "Connection rejection access log was unavailable")?;
        if log.status_code() != Some(500)
            || !log.has_diagnostic("error", "CredentialConnectionConflict")
            || log.backend_name() != Some("credential-connection")
            || log.upstream_count() != 1
        {
            return Err("Connection rejection did not have the expected local diagnostic and single upstream attempt");
        }
    }
    // Access-log completion precedes this bounded negative observation. Query
    // failures are infrastructure errors, never evidence of zero backend receipt.
    for observation in 0..3 {
        for trace in &traces[1..=3] {
            if !connection_captures(ctx, trace).await?.is_empty() {
                return Err("Backend received a request rejected for credential Connection nomination");
            }
        }
        if observation < 2 {
            sleep(Duration::from_millis(100)).await;
        }
    }

    // A real backend 502 must eject this same one-endpoint Service. Without
    // this positive control, a missing policy could make the earlier check pass.
    tokio::time::timeout(Duration::from_secs(3), async {
        let (status, _) = connection_request(ctx, CONNECTION_CONTROL_HOST, "/status/502", &traces[4]).await?;
        if status != 502 {
            return Err("Outlier positive control did not receive the backend 502");
        }
        let (status, _) = connection_request(ctx, CONNECTION_CONTROL_HOST, "/health", &traces[4]).await?;
        if !matches!(status, 502 | 503) {
            return Err("Outlier policy did not eject the endpoint after a real backend failure");
        }
        Ok(())
    })
    .await
    .map_err(|_| "Outlier positive control exceeded the ejection observation deadline")?
}

async fn wait_connection_endpoint(ctx: &TestContext, trace: &str) -> Result<(), &'static str> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if matches!(
                connection_request(ctx, CONNECTION_CONTROL_HOST, "/health", trace).await,
                Ok((200, _))
            ) {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| "Credential connection endpoint did not become available")
}

async fn cleanup_connection_nomination(ctx: &TestContext, traces: &[String; 5]) -> Result<(), &'static str> {
    // Always restore endpoint availability, including after an assertion fails.
    // Continue clearing observations even if recovery fails.
    let mut result = wait_connection_endpoint(ctx, &traces[4]).await;

    for host in mirror_backend_hosts(ctx) {
        let reset = ctx
            .http_client
            .get(format!("http://{host}:30001/mirror/reset"))
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .map_err(|_| "Credential connection capture cleanup failed")
            .and_then(|response| {
                response
                    .error_for_status()
                    .map_err(|_| "Credential connection capture cleanup returned an error status")
            })
            .map(|_| ());
        result = result.and(reset);
    }
    let logs = ctx.access_log_client();
    for trace in &traces[1..=3] {
        let deleted = logs
            .delete_access_log(trace)
            .await
            .map_err(|_| "Credential connection access-log cleanup failed")
            .and_then(|deleted| {
                if deleted {
                    Ok(())
                } else {
                    Err("Credential connection access-log cleanup was unsuccessful")
                }
            });
        result = result.and(deleted);
    }
    result
}

async fn assert_connection_nomination(ctx: TestContext) -> TestResult {
    let start = Instant::now();
    let traces = std::array::from_fn(|_| format!("credential-connection-{}", uuid::Uuid::new_v4()));
    let result = tokio::time::timeout(Duration::from_secs(30), check_connection_nomination(&ctx, &traces))
        .await
        .unwrap_or(Err("Credential connection scenario exceeded its deadline"));
    // Keep cleanup outside the scenario timeout so cancellation still restores
    // captured data and the dedicated endpoint before the next suite runs.
    let cleanup = tokio::time::timeout(Duration::from_secs(20), cleanup_connection_nomination(&ctx, &traces))
        .await
        .unwrap_or(Err("Credential connection cleanup exceeded its deadline"));
    match (result, cleanup) {
        (Ok(()), Ok(())) => TestResult::passed(start.elapsed()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => TestResult::failed(start.elapsed(), error.to_string()),
        (Err(error), Err(cleanup)) => TestResult::failed(start.elapsed(), format!("{error}; {cleanup}")),
    }
}

fn echoed_header(body: &str, expected_name: &str) -> Option<String> {
    let response: Value = serde_json::from_str(body).ok()?;
    let expected_name = expected_name.to_ascii_lowercase();
    response
        .get("headers")?
        .get(&expected_name)?
        .as_str()
        .map(ToOwned::to_owned)
}

async fn assert_upstream_header(
    ctx: TestContext,
    host: &str,
    header_name: &str,
    expected_value: &str,
    spoofed_value: Option<&str>,
) -> TestResult {
    let start = Instant::now();
    let mut request = ctx
        .http_client
        .get(format!("{}/headers", ctx.http_url()))
        .header("host", host);
    if let Some(value) = spoofed_value {
        request = request.header(header_name, value);
    }

    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => return TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
    };
    if response.status().as_u16() != 200 {
        return TestResult::failed(
            start.elapsed(),
            format!("Expected 200, got {}", response.status().as_u16()),
        );
    }

    let body = match response.text().await {
        Ok(body) => body,
        Err(error) => return TestResult::failed(start.elapsed(), format!("Read response failed: {error}")),
    };
    match echoed_header(&body, header_name) {
        Some(actual) if actual == expected_value => TestResult::passed(start.elapsed()),
        Some(_) => TestResult::failed(
            start.elapsed(),
            format!("Upstream {header_name} did not contain the expected generation"),
        ),
        None => TestResult::failed(start.elapsed(), format!("Upstream {header_name} was absent")),
    }
}

async fn assert_fail_closed(ctx: TestContext, host: &str) -> TestResult {
    let start = Instant::now();
    match ctx
        .http_client
        .get(format!("{}/headers", ctx.http_url()))
        .header("host", host)
        .header("authorization", "Bearer client-spoof")
        .send()
        .await
    {
        Ok(response) if response.status().as_u16() == 500 => {
            let body = response.text().await.unwrap_or_default();
            let lower = body.to_ascii_lowercase();
            if [
                "client-spoof",
                "bearertoken",
                "credentialinjector",
                "credential_injector",
            ]
            .iter()
            .any(|detail| lower.contains(detail))
            {
                TestResult::failed(
                    start.elapsed(),
                    "Fail-closed response exposed credential detail".to_string(),
                )
            } else {
                TestResult::passed(start.elapsed())
            }
        }
        Ok(response) => TestResult::failed(
            start.elapsed(),
            format!("Expected fail-closed 500, got {}", response.status().as_u16()),
        ),
        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
    }
}

fn mirror_backend_hosts(ctx: &TestContext) -> Vec<String> {
    let k8s_mode = std::env::var("EDGION_TEST_K8S_MODE")
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);
    if !k8s_mode {
        return vec![ctx.target_host.clone()];
    }
    if let Ok(ips) = std::env::var("EDGION_TEST_BACKEND_ENDPOINT_IPS") {
        let hosts: Vec<_> = ips
            .split(',')
            .map(str::trim)
            .filter(|host| !host.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        if !hosts.is_empty() {
            return hosts;
        }
    }
    vec![std::env::var("EDGION_TEST_MIRROR_BACKEND_HOST")
        .unwrap_or_else(|_| "edgion-test-server.edgion-test.svc.cluster.local".to_string())]
}

async fn reset_mirror_store(ctx: &TestContext) {
    for host in mirror_backend_hosts(ctx) {
        let _ = ctx
            .http_client
            .get(format!("http://{host}:30001/mirror/reset"))
            .send()
            .await;
    }
}

async fn poll_main_and_mirror_captures(ctx: &TestContext, trace_id: &str) -> Result<Vec<Value>, String> {
    let hosts = mirror_backend_hosts(ctx);
    for attempt in 0..=20 {
        let mut all_captures = Vec::new();
        for host in &hosts {
            let response = ctx
                .http_client
                .get(format!("http://{host}:30001/mirror/query/{trace_id}"))
                .send()
                .await;
            if let Ok(response) = response {
                if let Ok(body) = response.json::<Value>().await {
                    all_captures.extend(body["captures"].as_array().cloned().unwrap_or_default());
                }
            }
        }
        let has_main = all_captures
            .iter()
            .any(|capture| capture["headers"]["x-mirror"].as_str() != Some("true"));
        let has_mirror = all_captures
            .iter()
            .any(|capture| capture["headers"]["x-mirror"].as_str() == Some("true"));
        if has_main && has_mirror {
            return Ok(all_captures);
        }
        if attempt < 20 {
            sleep(Duration::from_millis(100)).await;
        }
    }
    Err("main and mirror captures were not both observed".to_string())
}

async fn assert_mirror_isolation(ctx: TestContext) -> TestResult {
    let start = Instant::now();
    reset_mirror_store(&ctx).await;
    let trace_id = format!("credential-injector-mirror-{}", uuid::Uuid::new_v4());
    let response = match ctx
        .http_client
        .get(format!("{}/mirror/capture", ctx.http_url()))
        .header("host", MIRROR_HOST)
        .header("x-trace-id", &trace_id)
        .header("authorization", "Bearer client-spoof")
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => return TestResult::failed(start.elapsed(), format!("Main request failed: {error}")),
    };
    if response.status().as_u16() != 200 {
        return TestResult::failed(
            start.elapsed(),
            format!("Expected main request 200, got {}", response.status().as_u16()),
        );
    }
    let _ = response.text().await;

    let captures = match poll_main_and_mirror_captures(&ctx, &trace_id).await {
        Ok(captures) => captures,
        Err(error) => return TestResult::failed(start.elapsed(), error),
    };
    let main = captures
        .iter()
        .find(|capture| capture["headers"]["x-mirror"].as_str() != Some("true"));
    let mirror = captures
        .iter()
        .find(|capture| capture["headers"]["x-mirror"].as_str() == Some("true"));
    let main_has_resolved =
        main.and_then(|capture| capture["headers"]["authorization"].as_str()) == Some("Bearer static-provider-token");
    let mirror_has_no_target = mirror.is_some_and(|capture| capture["headers"]["authorization"].is_null());

    if main_has_resolved && mirror_has_no_target {
        TestResult::passed(start.elapsed())
    } else {
        TestResult::failed(
            start.elapsed(),
            format!(
                "Credential isolation mismatch: main_resolved={main_has_resolved}, mirror_target_absent={mirror_has_no_target}"
            ),
        )
    }
}

pub struct CredentialInjectorTestSuite;

impl TestSuite for CredentialInjectorTestSuite {
    fn name(&self) -> &str {
        "CredentialInjector"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            TestCase::new(
                "default_bearer_replaces_client_spoof",
                "Default target replaces a client Authorization header with the Secret-backed bearer value",
                |ctx| {
                    Box::pin(async move {
                        assert_upstream_header(
                            ctx,
                            BEARER_HOST,
                            "authorization",
                            "Bearer static-provider-token",
                            Some("Bearer client-spoof"),
                        )
                        .await
                    })
                },
            ),
            TestCase::new(
                "custom_header_prefix_replaces_client_spoof",
                "Custom Header target applies valuePrefix and strips the client value",
                |ctx| {
                    Box::pin(async move {
                        assert_upstream_header(
                            ctx,
                            CUSTOM_HOST,
                            "x-provider-key",
                            "Token static-provider-token",
                            Some("client-spoof"),
                        )
                        .await
                    })
                },
            ),
            TestCase::new(
                "later_plugin_mutation_wins",
                "A later request plugin cancels the deferred credential mutation for the same header",
                |ctx| {
                    Box::pin(async move {
                        assert_upstream_header(
                            ctx,
                            LATER_REWRITE_HOST,
                            "x-provider-key",
                            "later-plugin-value",
                            Some("client-spoof"),
                        )
                        .await
                    })
                },
            ),
            TestCase::new(
                "later_gateway_api_header_modifier_wins",
                "A later Gateway API RequestHeaderModifier cancels the deferred credential mutation",
                |ctx| {
                    Box::pin(async move {
                        assert_upstream_header(
                            ctx,
                            LATER_ROUTE_MODIFIER_HOST,
                            "x-provider-key",
                            "later-route-value",
                            Some("client-spoof"),
                        )
                        .await
                    })
                },
            ),
            TestCase::new(
                "later_credential_injector_wins",
                "A later CredentialInjector replaces an earlier deferred credential for the same target",
                |ctx| {
                    Box::pin(async move {
                        assert_upstream_header(
                            ctx,
                            DOUBLE_INJECTOR_HOST,
                            "x-provider-key",
                            "Second static-provider-token",
                            Some("client-spoof"),
                        )
                        .await
                    })
                },
            ),
            TestCase::new(
                "missing_secret_fails_closed",
                "An unresolved credential Secret returns a generic 500",
                |ctx| Box::pin(async move { assert_fail_closed(ctx, MISSING_HOST).await }),
            ),
            TestCase::new(
                "mirror_after_injector_cannot_observe_credential",
                "A later RequestMirror sees neither the client spoof nor the deferred Secret while the main upstream receives it",
                |ctx| Box::pin(async move { assert_mirror_isolation(ctx).await }),
            ),
            TestCase::new(
                "late_connection_nomination_fails_before_backend_receipt",
                "Real H1 sanitization rejects a late credential nomination without penalizing the selected endpoint",
                |ctx| Box::pin(async move { assert_connection_nomination(ctx).await }),
            )
            .with_timeout(Duration::from_secs(60)),
        ]
    }
}

pub struct CredentialInjectorDynamicInitialTestSuite;

impl TestSuite for CredentialInjectorDynamicInitialTestSuite {
    fn name(&self) -> &str {
        "CredentialInjector Dynamic - Initial"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "credential_injector_dynamic_initial",
            "The initial Secret generation is injected",
            |ctx| {
                Box::pin(async move {
                    assert_upstream_header(ctx, DYNAMIC_HOST, "authorization", "Bearer dynamic-token-initial", None)
                        .await
                })
            },
        )]
    }
}

pub struct CredentialInjectorDynamicUpdatedTestSuite;

impl TestSuite for CredentialInjectorDynamicUpdatedTestSuite {
    fn name(&self) -> &str {
        "CredentialInjector Dynamic - Updated"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "credential_injector_dynamic_updated",
            "A Secret update replaces the injected generation without a restart",
            |ctx| {
                Box::pin(async move {
                    assert_upstream_header(ctx, DYNAMIC_HOST, "authorization", "Bearer dynamic-token-rotated", None)
                        .await
                })
            },
        )]
    }
}

pub struct CredentialInjectorDynamicDeletedTestSuite;

impl TestSuite for CredentialInjectorDynamicDeletedTestSuite {
    fn name(&self) -> &str {
        "CredentialInjector Dynamic - Secret Deleted"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "credential_injector_dynamic_deleted",
            "Deleting the referenced Secret fails closed",
            |ctx| Box::pin(async move { assert_fail_closed(ctx, DYNAMIC_HOST).await }),
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::echoed_header;

    #[test]
    fn parses_echoed_headers_case_insensitively() {
        let body = r#"{"headers":{"authorization":"Bearer value","x-other":"ok"}}"#;
        assert_eq!(echoed_header(body, "authorization").as_deref(), Some("Bearer value"));
        assert_eq!(echoed_header(body, "missing"), None);
    }
}
