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
