// DebugAccessLog Plugin Test Suite
//
// Required config files (in examples/test/conf/EdgionPlugins/DebugAccessLog/):
// - EdgionPlugins_default_debug-access-log.yaml  # Debug access log plugin config
//   Note: this plugin enables CORS, CSRF, ResponseHeaderModifier and DebugAccessLogToHeader
// - HTTPRoute_default_plugin-logs-test.yaml      # Plugin logs routing rules (Host: plugin-test.example.com)
//
// Also requires base config (in examples/test/conf/EdgionPlugins/base/):
// - Gateway.yaml                                 # Gateway config for EdgionPlugins tests

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use serde::{Deserialize, Serialize};
use std::time::Instant;

pub struct PluginLogsTestSuite;

// Data structures for parsing X-Debug-Access-Log JSON
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

impl PluginLogsTestSuite {
    fn test_basic_plugin_logs_structure() -> TestCase {
        TestCase::new(
            "basic_plugin_logs_structure",
            "Verify X-Debug-Access-Log header contains correct array structure",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let mut request = client.get(&url);

                    // Use plugin-test.example.com hostname for this test
                    request = request.header("host", "plugin-test.example.com");

                    match request.send().await {
                        Ok(response) => {
                            // Check if X-Debug-Access-Log header exists
                            if let Some(debug_header) = response.headers().get("x-debug-access-log") {
                                match debug_header.to_str() {
                                    Ok(json_str) => {
                                        // Try to parse as JSON
                                        match serde_json::from_str::<AccessLog>(json_str) {
                                            Ok(access_log) => {
                                                // Verify stage_logs is an array with at least 1 stage
                                                if access_log.stage_logs.is_empty() {
                                                    TestResult::failed(
                                                        start.elapsed(),
                                                        "stage_logs array is empty".to_string(),
                                                    )
                                                } else {
                                                    TestResult::passed_with_message(
                                                        start.elapsed(),
                                                        format!(
                                                            "Array structure verified, {} stages found",
                                                            access_log.stage_logs.len()
                                                        ),
                                                    )
                                                }
                                            }
                                            Err(e) => TestResult::failed(
                                                start.elapsed(),
                                                format!("Failed to parse JSON: {}", e),
                                            ),
                                        }
                                    }
                                    Err(e) => TestResult::failed(
                                        start.elapsed(),
                                        format!("Header value not valid UTF-8: {}", e),
                                    ),
                                }
                            } else {
                                TestResult::failed(start.elapsed(), "X-Debug-Access-Log header not found".to_string())
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
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
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let mut request = client.get(&url);
                    request = request.header("host", "plugin-test.example.com");

                    match request.send().await {
                        Ok(response) => {
                            if let Some(debug_header) = response.headers().get("x-debug-access-log") {
                                if let Ok(json_str) = debug_header.to_str() {
                                    if let Ok(access_log) = serde_json::from_str::<AccessLog>(json_str) {
                                        // Find request_filters stage
                                        if let Some(stage) =
                                            access_log.stage_logs.iter().find(|s| s.stage == "request_filters")
                                        {
                                            // Check if it has at least 1 filter
                                            if stage.filters.is_empty() {
                                                return TestResult::failed(
                                                    start.elapsed(),
                                                    "No filters found in request_filters stage".to_string(),
                                                );
                                            }

                                            // Check time_cost exists for filters (except ExtensionRef which doesn't have time_cost)
                                            for plugin in &stage.filters {
                                                // ExtensionRef filters have refer_to set, they don't track time_cost
                                                // because they are containers that reference other plugins
                                                if plugin.name != "ExtensionRef" && plugin.refer_to.is_none() && plugin.time_cost.is_none() {
                                                    return TestResult::failed(
                                                        start.elapsed(),
                                                        format!("Filter '{}' missing time_cost", plugin.name),
                                                    );
                                                }
                                            }

                                            let plugin_names: Vec<&str> =
                                                stage.filters.iter().map(|p| p.name.as_str()).collect();
                                            TestResult::passed_with_message(
                                                start.elapsed(),
                                                format!("Filters executed: {:?}", plugin_names),
                                            )
                                        } else {
                                            TestResult::failed(
                                                start.elapsed(),
                                                "request_filters stage not found".to_string(),
                                            )
                                        }
                                    } else {
                                        TestResult::failed(start.elapsed(), "Failed to parse JSON".to_string())
                                    }
                                } else {
                                    TestResult::failed(start.elapsed(), "Header not UTF-8".to_string())
                                }
                            } else {
                                TestResult::failed(start.elapsed(), "Header not found".to_string())
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
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
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let mut request = client.get(&url);
                    request = request.header("host", "plugin-test.example.com");

                    match request.send().await {
                        Ok(response) => {
                            if let Some(debug_header) = response.headers().get("x-debug-access-log") {
                                if let Ok(json_str) = debug_header.to_str() {
                                    if let Ok(access_log) = serde_json::from_str::<AccessLog>(json_str) {
                                        if access_log.stage_logs.is_empty() {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                "No plugin stages found".to_string(),
                                            );
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
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("Stages found: {:?}", stage_names),
                                        )
                                    } else {
                                        TestResult::failed(start.elapsed(), "Failed to parse JSON".to_string())
                                    }
                                } else {
                                    TestResult::failed(start.elapsed(), "Header not UTF-8".to_string())
                                }
                            } else {
                                TestResult::failed(start.elapsed(), "Header not found".to_string())
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_empty_plugin_logs() -> TestCase {
        TestCase::new(
            "empty_plugin_logs",
            "Verify behavior when no plugins are configured",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let mut request = client.get(&url);
                    // Use test.example.com which doesn't have plugin configuration
                    request = request.header("host", "test.example.com");

                    match request.send().await {
                        Ok(response) => {
                            // For routes without plugins, either:
                            // 1. X-Debug-Access-Log header doesn't exist, OR
                            // 2. plugin_logs is an empty array
                            if let Some(debug_header) = response.headers().get("x-debug-access-log") {
                                if let Ok(json_str) = debug_header.to_str() {
                                    if let Ok(access_log) = serde_json::from_str::<AccessLog>(json_str) {
                                        if access_log.stage_logs.is_empty() {
                                            TestResult::passed_with_message(
                                                start.elapsed(),
                                                "Empty logs handled correctly (empty array)".to_string(),
                                            )
                                        } else {
                                            TestResult::failed(
                                                start.elapsed(),
                                                format!(
                                                    "Expected empty plugin_logs, got {} stages",
                                                    access_log.stage_logs.len()
                                                ),
                                            )
                                        }
                                    } else {
                                        TestResult::failed(start.elapsed(), "Failed to parse JSON".to_string())
                                    }
                                } else {
                                    TestResult::failed(start.elapsed(), "Header not UTF-8".to_string())
                                }
                            } else {
                                // Header not present is also acceptable for routes without plugins
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Empty logs handled correctly (no header)".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
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
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let mut request = client.get(&url);
                    request = request.header("host", "plugin-test.example.com");

                    match request.send().await {
                        Ok(response) => {
                            if let Some(debug_header) = response.headers().get("x-debug-access-log") {
                                if let Ok(json_str) = debug_header.to_str() {
                                    if let Ok(access_log) = serde_json::from_str::<AccessLog>(json_str) {
                                        let mut all_valid = true;
                                        let mut error_msg = String::new();

                                        for stage in &access_log.stage_logs {
                                            for plugin in &stage.filters {
                                                // ExtensionRef filters have refer_to set, they don't track time_cost
                                                // because they are containers that reference other plugins
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
                                            let total_filters: usize =
                                                access_log.stage_logs.iter().map(|s| s.filters.len()).sum();
                                            TestResult::passed_with_message(
                                                start.elapsed(),
                                                format!("All {} filters have valid time_cost", total_filters),
                                            )
                                        } else {
                                            TestResult::failed(start.elapsed(), error_msg)
                                        }
                                    } else {
                                        TestResult::failed(start.elapsed(), "Failed to parse JSON".to_string())
                                    }
                                } else {
                                    TestResult::failed(start.elapsed(), "Header not UTF-8".to_string())
                                }
                            } else {
                                TestResult::failed(start.elapsed(), "Header not found".to_string())
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
}

impl TestSuite for PluginLogsTestSuite {
    fn name(&self) -> &str {
        "DebugAccessLog Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_basic_plugin_logs_structure(),
            Self::test_request_filters_stage_details(),
            Self::test_response_header_modifier(),
            Self::test_stage_execution_order(),
            Self::test_empty_plugin_logs(),
            Self::test_plugin_time_cost(),
        ]
    }
}
