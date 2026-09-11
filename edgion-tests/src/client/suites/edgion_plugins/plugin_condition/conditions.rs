// PluginCondition Test Suite
//
// Tests plugin conditional execution based on keyExist condition
//
// Required config files (in edgion-tests/integration/conf/EdgionPlugins/PluginCondition/):
// - EdgionPlugins_default_condition-test.yaml  # CORS with skip condition on X-Skip-Cors header
// - HTTPRoute_default_condition-test.yaml      # Route for condition-test.example.com
//
// Also requires base config (in edgion-tests/integration/conf/EdgionPlugins/base/):
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
    #[serde(default)]
    alias: Option<String>,
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
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
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
                                                .find(|log| {
                                                    log.name == "Cors" && log.alias.as_deref() == Some("single-leaf")
                                                });

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
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
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
                                                .find(|log| {
                                                    log.name == "Cors" && log.alias.as_deref() == Some("single-leaf")
                                                });

                                            match cors_log {
                                                Some(log) => {
                                                    // Check if cond_skip is set
                                                    if let Some(ref skip_reason) = log.cond_skip {
                                                        let atomic_descriptor_emitted = log
                                                            .log
                                                            .iter()
                                                            .any(|entry| entry.starts_with("skip.allOf[0]:"));
                                                        if skip_reason.starts_with("skip:")
                                                            && !atomic_descriptor_emitted
                                                        {
                                                            TestResult::passed_with_message(
                                                                start.elapsed(),
                                                                format!(
                                                                    "single-leaf CORS skipped without atomic descriptor: {}",
                                                                    skip_reason
                                                                ),
                                                            )
                                                        } else {
                                                            TestResult::failed(
                                                                start.elapsed(),
                                                                format!(
                                                                    "single-leaf log threshold mismatch: cond_skip={skip_reason}, log={:?}",
                                                                    log.log
                                                                ),
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

    /// Test 3: A composite condition emits the matched atomic path.
    fn test_multi_leaf_match_descriptors_are_logged() -> TestCase {
        TestCase::new(
            "multi_leaf_match_descriptors_are_logged",
            "Verify composite conditions log their matched atomic path",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("test-cond-multi-leaf-{}", uuid::Uuid::new_v4());
                    let response = ctx
                        .http_client
                        .get(format!("{}/health", ctx.http_url()))
                        .header("host", "condition-test.example.com")
                        .header("X-Multi-A", "1")
                        .header("X-Multi-B", "1")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await;
                    let response = match response {
                        Ok(response) if response.status().is_success() => response,
                        Ok(response) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Request failed with status: {}", response.status()),
                            );
                        }
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("Request failed: {error}"));
                        }
                    };
                    drop(response);

                    let entry = match ctx
                        .access_log_client()
                        .get_access_log_with_retry(&trace_id, 10, 200)
                        .await
                    {
                        Ok(entry) => entry,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to fetch access log: {error}"));
                        }
                    };
                    let access_log = match serde_json::from_value::<AccessLog>(entry.data) {
                        Ok(access_log) => access_log,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("Parse error: {error}"));
                        }
                    };
                    let log = access_log
                        .stage_logs
                        .iter()
                        .flat_map(|stage| &stage.edgion_plugins)
                        .flat_map(|plugins| &plugins.logs)
                        .find(|log| log.name == "Cors" && log.alias.as_deref() == Some("multi-leaf"));
                    let Some(log) = log else {
                        return TestResult::failed(start.elapsed(), "multi-leaf Cors log not found".to_string());
                    };
                    let first = log
                        .log
                        .iter()
                        .any(|entry| entry.starts_with("skip.anyOf[0].allOf[0]:keyExist"));
                    let second = log
                        .log
                        .iter()
                        .any(|entry| entry.starts_with("skip.anyOf[0].allOf[1]:keyExist"));
                    if log.cond_skip.as_deref() == Some("skip:both-debug-headers") && first && second {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "composite condition emitted both matched atomic descriptors".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "multi-leaf log threshold mismatch: cond_skip={:?}, log={:?}",
                                log.cond_skip, log.log
                            ),
                        )
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
            Self::test_multi_leaf_match_descriptors_are_logged(),
        ]
    }
}
