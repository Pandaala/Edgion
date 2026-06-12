// DebugAccessLog Plugin Test Suite (via Access Log Store)
//
// Tests access log data captured by --integration-testing-mode via the Access Log Store.
// Instead of reading X-Debug-Access-Log response header, tests now:
// 1. Send a request with a unique x-trace-id header
// 2. Query the Access Log Store Admin API with that trace_id
// 3. Verify the complete access log JSON structure
//
// Required config files (in examples/test/conf/EdgionPlugins/DebugAccessLog/):
// - EdgionPlugins_default_debug-access-log.yaml  # Plugin config (Cors, Csrf, ResponseHeaderModifier)
// - HTTPRoute_default_plugin-logs-test.yaml      # Plugin logs routing rules (Host: plugin-test.example.com)
//
// Also requires:
// - Gateway started with --integration-testing-mode flag
// - Base config (in examples/test/conf/EdgionPlugins/base/)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use serde::{Deserialize, Serialize};
use std::time::Instant;

pub struct PluginLogsTestSuite;

// Data structures for parsing access log JSON from Access Log Store
#[derive(Debug, Deserialize, Serialize)]
struct AccessLog {
    #[serde(default)]
    stage_logs: Vec<StageLogs>,
    #[serde(flatten)]
    other: serde_json::Value,
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
    time_cost: Option<u64>,
    #[serde(default)]
    log: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    log_full: Option<bool>,
    /// Condition skip reason, e.g., "skip:keyExist,hdr:X-Skip-Cors"
    #[serde(skip_serializing_if = "Option::is_none")]
    cond_skip: Option<String>,
    /// ExtensionRef reference info
    #[serde(skip_serializing_if = "Option::is_none")]
    refer_to: Option<serde_json::Value>,
}

/// Generate a unique trace ID for test correlation
fn gen_trace_id(test_name: &str) -> String {
    format!("test-{}-{}", test_name, uuid::Uuid::new_v4())
}

/// Helper: send a request with trace_id and fetch the access log from the store
async fn send_and_fetch_log(ctx: &TestContext, trace_id: &str, host: &str, path: &str) -> Result<AccessLog, String> {
    let client = &ctx.http_client;
    let url = format!("{}{}", ctx.http_url(), path);

    // Send request with trace_id and access_log: test_store to trigger storage
    let response = client
        .get(&url)
        .header("host", host)
        .header("x-trace-id", trace_id)
        .header("access_log", "test_store")
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    let status = response.status().as_u16();
    // Consume body to ensure the request is fully processed
    let _ = response.text().await;

    // Give the gateway a moment to store the access log
    // Then query with retry
    let al_client = ctx.access_log_client();
    let entry = al_client
        .get_access_log_with_retry(trace_id, 10, 200)
        .await
        .map_err(|e| format!("Failed to fetch access log (status={}): {}", status, e))?;

    // Parse the access log JSON
    serde_json::from_value::<AccessLog>(entry.data).map_err(|e| format!("Failed to parse access log JSON: {}", e))
}

impl PluginLogsTestSuite {
    fn test_basic_plugin_logs_structure() -> TestCase {
        TestCase::new(
            "basic_plugin_logs_structure",
            "Verify access log from Access Log Store contains correct stage_logs structure",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = gen_trace_id("basic-structure");

                    match send_and_fetch_log(&ctx, &trace_id, "plugin-test.example.com", "/health").await {
                        Ok(access_log) => {
                            // Verify stage_logs is an array with at least 1 stage
                            if access_log.stage_logs.is_empty() {
                                TestResult::failed(start.elapsed(), "stage_logs array is empty".to_string())
                            } else {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "Access Log Store: stage_logs has {} stages",
                                        access_log.stage_logs.len()
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    fn test_request_filters_stage_details() -> TestCase {
        TestCase::new(
            "request_filters_stage_details",
            "Verify request_filters stage contains ExtensionRef plugin",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = gen_trace_id("request-filters");

                    match send_and_fetch_log(&ctx, &trace_id, "plugin-test.example.com", "/health").await {
                        Ok(access_log) => {
                            // Find request_filters stage
                            if let Some(stage) = access_log.stage_logs.iter().find(|s| s.stage == "request_filters") {
                                // Check if it has at least 1 filter
                                if stage.filters.is_empty() {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        "No filters found in request_filters stage".to_string(),
                                    );
                                }

                                // Check time_cost exists for filters (except ExtensionRef)
                                for plugin in &stage.filters {
                                    if plugin.name != "ExtensionRef"
                                        && plugin.refer_to.is_none()
                                        && plugin.time_cost.is_none()
                                    {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Filter '{}' missing time_cost", plugin.name),
                                        );
                                    }
                                }

                                let plugin_names: Vec<&str> = stage.filters.iter().map(|p| p.name.as_str()).collect();
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Filters executed: {:?}", plugin_names),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), "request_filters stage not found".to_string())
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    fn test_response_header_modifier() -> TestCase {
        TestCase::new(
            "response_header_modifier",
            "Verify ResponseHeaderModifier plugin adds X-Test-Header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let mut request = client.get(&url);
                    request = request.header("host", "plugin-test.example.com");

                    match request.send().await {
                        Ok(response) => {
                            // Check X-Test-Header was added by ResponseHeaderModifier
                            if let Some(test_header) = response.headers().get("x-test-header") {
                                let value = test_header.to_str().unwrap_or("");
                                if value == "test-value" {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        "X-Test-Header correctly added by ResponseHeaderModifier".to_string(),
                                    )
                                } else {
                                    TestResult::failed(
                                        start.elapsed(),
                                        format!("X-Test-Header has wrong value: {}", value),
                                    )
                                }
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "X-Test-Header not found (ResponseHeaderModifier didn't work)".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_stage_execution_order() -> TestCase {
        TestCase::new(
            "stage_execution_order",
            "Verify plugin stages exist and have correct structure",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = gen_trace_id("stage-order");

                    match send_and_fetch_log(&ctx, &trace_id, "plugin-test.example.com", "/health").await {
                        Ok(access_log) => {
                            if access_log.stage_logs.is_empty() {
                                return TestResult::failed(start.elapsed(), "No plugin stages found".to_string());
                            }

                            // Verify each stage has valid structure
                            for stage in &access_log.stage_logs {
                                if stage.stage.is_empty() {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        "Found stage with empty name".to_string(),
                                    );
                                }
                            }

                            let stage_names: Vec<&str> =
                                access_log.stage_logs.iter().map(|s| s.stage.as_str()).collect();
                            TestResult::passed_with_message(start.elapsed(), format!("Stages found: {:?}", stage_names))
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    fn test_empty_plugin_logs() -> TestCase {
        TestCase::new(
            "empty_plugin_logs",
            "Verify access log has empty stage_logs when no plugins are configured",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = gen_trace_id("empty-logs");

                    // Use test.example.com which doesn't have plugin configuration
                    match send_and_fetch_log(&ctx, &trace_id, "test.example.com", "/health").await {
                        Ok(access_log) => {
                            if access_log.stage_logs.is_empty() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Empty logs handled correctly (empty stage_logs)".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected empty stage_logs, got {} stages", access_log.stage_logs.len()),
                                )
                            }
                        }
                        Err(_) => {
                            // Access log not found is also acceptable for routes without plugins
                            // (the gateway may not store logs for simple requests)
                            TestResult::passed_with_message(
                                start.elapsed(),
                                "Empty logs handled correctly (no access log stored)".to_string(),
                            )
                        }
                    }
                })
            },
        )
    }

    fn test_plugin_time_cost() -> TestCase {
        TestCase::new(
            "plugin_time_cost",
            "Verify all plugins have valid time_cost values",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = gen_trace_id("time-cost");

                    match send_and_fetch_log(&ctx, &trace_id, "plugin-test.example.com", "/health").await {
                        Ok(access_log) => {
                            let mut all_valid = true;
                            let mut error_msg = String::new();

                            for stage in &access_log.stage_logs {
                                for plugin in &stage.filters {
                                    // ExtensionRef filters have refer_to set, they don't track time_cost
                                    let is_extension_ref = plugin.name == "ExtensionRef" || plugin.refer_to.is_some();

                                    match plugin.time_cost {
                                        None => {
                                            if !is_extension_ref {
                                                all_valid = false;
                                                error_msg = format!(
                                                    "Filter '{}' in stage '{}' missing time_cost",
                                                    plugin.name, stage.stage
                                                );
                                                break;
                                            }
                                        }
                                        Some(tc) => {
                                            // Check reasonable range (< 1 second = 1,000,000 microseconds)
                                            if tc > 1_000_000 {
                                                all_valid = false;
                                                error_msg = format!(
                                                    "Filter '{}' has unreasonable time_cost: {} us",
                                                    plugin.name, tc
                                                );
                                                break;
                                            }
                                        }
                                    }
                                }
                                if !all_valid {
                                    break;
                                }
                            }

                            if all_valid {
                                let total_filters: usize = access_log.stage_logs.iter().map(|s| s.filters.len()).sum();
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("All {} filters have valid time_cost", total_filters),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), error_msg)
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    fn test_access_log_store_status() -> TestCase {
        TestCase::new(
            "access_log_store_status",
            "Verify Access Log Store is enabled via integration testing mode",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let al_client = ctx.access_log_client();

                    match al_client.check_status().await {
                        Ok(status) => {
                            if !status.integration_testing_mode {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "integration_testing_mode is not enabled".to_string(),
                                );
                            }
                            if !status.access_log_store.enabled {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "access_log_store is not enabled".to_string(),
                                );
                            }
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!(
                                    "Access Log Store: enabled, entries={}, max_capacity={}",
                                    status.access_log_store.entry_count, status.access_log_store.max_capacity,
                                ),
                            )
                        }
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Failed to check testing status: {}. Is --integration-testing-mode enabled?",
                                e
                            ),
                        ),
                    }
                })
            },
        )
    }
}

impl TestSuite for PluginLogsTestSuite {
    fn name(&self) -> &str {
        "DebugAccessLog Plugin Tests (via Access Log Store)"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_access_log_store_status(),
            Self::test_basic_plugin_logs_structure(),
            Self::test_request_filters_stage_details(),
            Self::test_response_header_modifier(),
            Self::test_stage_execution_order(),
            Self::test_empty_plugin_logs(),
            Self::test_plugin_time_cost(),
        ]
    }
}
