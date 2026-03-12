// PluginCondition Test Suite
//
// Tests plugin conditional execution based on keyExist condition
//
// Required config files (in examples/test/conf/EdgionPlugins/PluginCondition/):
// - EdgionPlugins_default_condition-test.yaml  # CORS with skip condition on X-Skip-Cors header
// - HTTPRoute_default_condition-test.yaml      # Route for condition-test.example.com
//
// Also requires base config (in examples/test/conf/EdgionPlugins/base/):
// - Gateway.yaml                               # Gateway for EdgionPlugins tests

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use serde::{Deserialize, Serialize};
use std::time::Instant;

pub struct PluginConditionTestSuite;

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

impl PluginConditionTestSuite {
    /// Test 1: Without X-Skip-Cors header, CORS plugin should run
    fn test_cors_runs_without_skip_header() -> TestCase {
        TestCase::new(
            "cors_runs_without_skip_header",
            "Verify CORS plugin runs when X-Skip-Cors header is absent",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let trace_id = format!("test-cond-cors-run-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/health", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", "condition-test.example.com")
                        .header("origin", "http://example.com")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store");

                    match request.send().await {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Request failed with status: {}", response.status()),
                                );
                            }

                            // Fetch access log from store
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    match serde_json::from_value::<AccessLog>(entry.data) {
                                        Ok(access_log) => {
                                            // Find Cors plugin log in edgion_plugins
                                            let cors_log = access_log
                                                .stage_logs
                                                .iter()
                                                .flat_map(|stage| &stage.edgion_plugins)
                                                .flat_map(|ep| &ep.logs)
                                                .find(|log| log.name == "Cors");

                                            match cors_log {
                                                Some(log) => {
                                                    // Check if cond_skip is None (CORS ran)
                                                    if log.cond_skip.is_none() {
                                                        // Check if log contains CORS output
                                                        let has_cors_output =
                                                            log.log.iter().any(|l| l.contains("CORS"));
                                                        if has_cors_output {
                                                            TestResult::passed_with_message(
                                                                start.elapsed(),
                                                                "CORS plugin ran (no cond_skip)".to_string(),
                                                            )
                                                        } else {
                                                            TestResult::passed_with_message(
                                                                start.elapsed(),
                                                                "CORS plugin ran (cond_skip is None)".to_string(),
                                                            )
                                                        }
                                                    } else {
                                                        TestResult::failed(
                                                            start.elapsed(),
                                                            format!(
                                                                "CORS was skipped unexpectedly: {:?}",
                                                                log.cond_skip
                                                            ),
                                                        )
                                                    }
                                                }
                                                None => TestResult::failed(
                                                    start.elapsed(),
                                                    "Cors plugin log not found in edgion_plugins".to_string(),
                                                ),
                                            }
                                        }
                                        Err(e) => TestResult::failed(start.elapsed(), format!("Parse error: {}", e)),
                                    }
                                }
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

    /// Test 2: With X-Skip-Cors header, CORS plugin should be skipped
    fn test_cors_skipped_with_skip_header() -> TestCase {
        TestCase::new(
            "cors_skipped_with_skip_header",
            "Verify CORS plugin is skipped when X-Skip-Cors header is present",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let trace_id = format!("test-cond-cors-skip-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/health", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", "condition-test.example.com")
                        .header("X-Skip-Cors", "true") //  skip
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store");

                    match request.send().await {
                        Ok(response) => {
                            // Check status first to fail fast
                            if !response.status().is_success() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Request failed with status: {}", response.status()),
                                );
                            }

                            // Fetch access log from store
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    match serde_json::from_value::<AccessLog>(entry.data) {
                                        Ok(access_log) => {
                                            // Find Cors plugin log in edgion_plugins
                                            let cors_log = access_log
                                                .stage_logs
                                                .iter()
                                                .flat_map(|stage| &stage.edgion_plugins)
                                                .flat_map(|ep| &ep.logs)
                                                .find(|log| log.name == "Cors");

                                            match cors_log {
                                                Some(log) => {
                                                    // Check if cond_skip is set
                                                    if let Some(ref skip_reason) = log.cond_skip {
                                                        // Verify skip reason format: "skip:keyExist,hdr:X-Skip-Cors"
                                                        if skip_reason.contains("skip:keyExist")
                                                            && skip_reason.contains("hdr:X-Skip-Cors")
                                                        {
                                                            TestResult::passed_with_message(
                                                                start.elapsed(),
                                                                format!("CORS skipped correctly: {}", skip_reason),
                                                            )
                                                        } else {
                                                            TestResult::failed(
                                                                start.elapsed(),
                                                                format!("Unexpected skip reason: {}", skip_reason),
                                                            )
                                                        }
                                                    } else {
                                                        // No cond_skip found - CORS ran but should have been skipped
                                                        TestResult::failed(
                                                            start.elapsed(),
                                                            format!(
                                                                "CORS ran but should have been skipped. Log: {:?}",
                                                                log.log
                                                            ),
                                                        )
                                                    }
                                                }
                                                None => TestResult::failed(
                                                    start.elapsed(),
                                                    "Cors plugin log not found in edgion_plugins".to_string(),
                                                ),
                                            }
                                        }
                                        Err(e) => TestResult::failed(start.elapsed(), format!("Parse error: {}", e)),
                                    }
                                }
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

impl TestSuite for PluginConditionTestSuite {
    fn name(&self) -> &str {
        "PluginCondition Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_cors_runs_without_skip_header(),
            Self::test_cors_skipped_with_skip_header(),
        ]
    }
}
