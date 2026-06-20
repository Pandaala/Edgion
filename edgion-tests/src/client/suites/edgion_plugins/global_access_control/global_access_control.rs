// GlobalAccessControl Plugin Test Suite
//
// Covers:
// - Composite allow: client IP in 127.0.0.0/8 AND correct secret header => 200
// - Deny when secret header is missing => 403
// - Deny when secret header has a wrong value => 403
// - Deny when client IP is outside the configured CIDR (10.0.0.0/8) even with correct header => 403
// - Access-log decision assertion: allow path records "allow:group=cloudflare"
// - Access-log decision assertion: deny path records "deny:no-match"
//
// Required conf files (edgion-tests/integration/conf/EdgionPlugins/GlobalAccessControl/):
//   01_Secret_edgion-test_gac-origin-secret.yaml
//   02_EdgionConfigData_edgion-test_gac-selector.yaml
//   03_EdgionPlugins_edgion-test_gac-lockdown.yaml
//   04_EdgionPlugins_edgion-test_gac-ip-outside.yaml
//   HTTPRoute_edgion-test_gac.yaml

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use serde::{Deserialize, Serialize};
use std::time::Instant;

pub struct GlobalAccessControlTestSuite;

// ---- Access-log deserialization types (mirrors plugin_condition/conditions.rs) ----

#[derive(Debug, Deserialize, Serialize)]
struct AccessLog {
    #[serde(default)]
    stage_logs: Vec<StageLogs>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct StageLogs {
    stage: String,
    filters: Vec<PluginLog>,
    #[serde(default)]
    edgion_plugins: Vec<EdgionPluginsLog>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct EdgionPluginsLog {
    name: String,
    logs: Vec<PluginLog>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct PluginLog {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cond_skip: Option<String>,
    #[serde(default)]
    log: Vec<String>,
}

impl GlobalAccessControlTestSuite {
    // ==================== 1. Allow: in-CIDR AND correct secret ====================
    fn test_allow_in_cidr_and_secret() -> TestCase {
        TestCase::new(
            "allow_in_cidr_and_secret",
            "Allow: IP in 127.0.0.0/8 AND correct X-CF-Secret => 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/test/gac/lockdown/api", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", "gac.example.com")
                        .header("X-CF-Secret", "cf-origin-token-123");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Allowed with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 2. Deny: secret header missing ====================
    fn test_deny_secret_missing() -> TestCase {
        TestCase::new(
            "deny_secret_missing",
            "Deny: X-CF-Secret header absent => 403",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/test/gac/lockdown/api", ctx.http_url());

                    let request = client.get(&url).header("host", "gac.example.com");
                    // No X-CF-Secret header -- onMissing: NoMatch => group fails => deny.

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Missing secret denied with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 403, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 3. Deny: wrong secret value ====================
    fn test_deny_secret_wrong() -> TestCase {
        TestCase::new(
            "deny_secret_wrong",
            "Deny: X-CF-Secret has wrong value => 403",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/test/gac/lockdown/api", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", "gac.example.com")
                        .header("X-CF-Secret", "nope");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Wrong secret denied with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 403, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 4. Deny: IP outside CIDR (10.0.0.0/8, test client is 127.0.0.1) ====================
    fn test_deny_ip_outside_cidr() -> TestCase {
        TestCase::new(
            "deny_ip_outside_cidr",
            "Deny: IP 127.0.0.1 outside 10.0.0.0/8 even with correct secret => 403",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/test/gac/ip-outside/api", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", "gac.example.com")
                        .header("X-CF-Secret", "cf-origin-token-123");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("IP outside CIDR denied with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 403, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 5. Access log: allow path records "allow:group=cloudflare" ====================
    fn test_access_log_allow_decision() -> TestCase {
        TestCase::new(
            "access_log_allow_decision",
            "Access log: allow path records allow:group=cloudflare",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let trace_id = format!("test-gac-allow-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/test/gac/lockdown/api", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", "gac.example.com")
                        .header("X-CF-Secret", "cf-origin-token-123")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {} -- cannot assert access log", status),
                                );
                            }

                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => match serde_json::from_value::<AccessLog>(entry.data) {
                                    Ok(access_log) => {
                                        let gac_log = access_log
                                            .stage_logs
                                            .iter()
                                            .flat_map(|stage| &stage.edgion_plugins)
                                            .flat_map(|ep| &ep.logs)
                                            .find(|log| log.name == "GlobalAccessControl");

                                        match gac_log {
                                            Some(log) => {
                                                if log.log.iter().any(|l| l.contains("allow:group=cloudflare")) {
                                                    TestResult::passed_with_message(
                                                        start.elapsed(),
                                                        "Access log contains allow:group=cloudflare".to_string(),
                                                    )
                                                } else {
                                                    TestResult::failed(
                                                        start.elapsed(),
                                                        format!(
                                                            "Expected allow:group=cloudflare in log, got: {:?}",
                                                            log.log
                                                        ),
                                                    )
                                                }
                                            }
                                            None => TestResult::failed(
                                                start.elapsed(),
                                                "GlobalAccessControl plugin log not found in edgion_plugins"
                                                    .to_string(),
                                            ),
                                        }
                                    }
                                    Err(e) => TestResult::failed(start.elapsed(), format!("Parse error: {}", e)),
                                },
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("Failed to fetch access log: {}", e))
                                }
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 6. Access log: deny path records "deny:no-match" ====================
    fn test_access_log_deny_decision() -> TestCase {
        TestCase::new(
            "access_log_deny_decision",
            "Access log: deny path records deny:no-match",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let trace_id = format!("test-gac-deny-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/test/gac/lockdown/api", ctx.http_url());

                    // No X-CF-Secret => deny:no-match expected.
                    let request = client
                        .get(&url)
                        .header("host", "gac.example.com")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 403 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 403, got {} -- cannot assert access log", status),
                                );
                            }

                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => match serde_json::from_value::<AccessLog>(entry.data) {
                                    Ok(access_log) => {
                                        let gac_log = access_log
                                            .stage_logs
                                            .iter()
                                            .flat_map(|stage| &stage.edgion_plugins)
                                            .flat_map(|ep| &ep.logs)
                                            .find(|log| log.name == "GlobalAccessControl");

                                        match gac_log {
                                            Some(log) => {
                                                if log.log.iter().any(|l| l.contains("deny:no-match")) {
                                                    TestResult::passed_with_message(
                                                        start.elapsed(),
                                                        "Access log contains deny:no-match".to_string(),
                                                    )
                                                } else {
                                                    TestResult::failed(
                                                        start.elapsed(),
                                                        format!("Expected deny:no-match in log, got: {:?}", log.log),
                                                    )
                                                }
                                            }
                                            None => TestResult::failed(
                                                start.elapsed(),
                                                "GlobalAccessControl plugin log not found in edgion_plugins"
                                                    .to_string(),
                                            ),
                                        }
                                    }
                                    Err(e) => TestResult::failed(start.elapsed(), format!("Parse error: {}", e)),
                                },
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("Failed to fetch access log: {}", e))
                                }
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
}

impl TestSuite for GlobalAccessControlTestSuite {
    fn name(&self) -> &str {
        "GlobalAccessControl Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            // Static behavior cases
            Self::test_allow_in_cidr_and_secret(),
            Self::test_deny_secret_missing(),
            Self::test_deny_secret_wrong(),
            Self::test_deny_ip_outside_cidr(),
            // Access-log decision assertions
            Self::test_access_log_allow_decision(),
            Self::test_access_log_deny_decision(),
        ]
    }
}
