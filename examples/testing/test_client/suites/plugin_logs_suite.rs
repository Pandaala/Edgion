// Plugin Logs Test Suite
//
// 依赖的配置文件（位于 examples/conf/）：
// - EndpointSlice_edge_test-http.yaml         # HTTP 后端服务发现
// - Service_edge_test-http.yaml               # HTTP 服务定义
// - HTTPRoute_default_plugin-logs-test.yaml   # Plugin logs 路由规则（Host: plugin-test.example.com）
// - EdgionPlugins_default_debug-access-log.yaml  # Debug access log 插件配置
//   注：该插件启用了 CORS、CSRF、ResponseHeaderModifier 和 DebugAccessLogToHeader
// - Gateway_edge_example-gateway.yaml         # Gateway 配置
// - GatewayClass__public-gateway.yaml         # GatewayClass 配置

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use serde::{Deserialize, Serialize};
use std::time::Instant;

pub struct PluginLogsTestSuite;

// Data structures for parsing X-Debug-Access-Log JSON
#[derive(Debug, Deserialize, Serialize)]
struct AccessLog {
    #[serde(default)]
    plugin_logs: Vec<StagePluginLogs>,
    #[serde(flatten)]
    other: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct StagePluginLogs {
    stage: String,
    logs: Vec<PluginLog>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct PluginLog {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    time_cost: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    log: Option<String>,
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
                                                // Verify plugin_logs is an array
                                                if access_log.plugin_logs.is_empty() {
                                                    TestResult::failed(
                                                        start.elapsed(),
                                                        "plugin_logs array is empty".to_string(),
                                                    )
                                                } else if access_log.plugin_logs.len() == 2 {
                                                    TestResult::passed_with_message(
                                                        start.elapsed(),
                                                        format!(
                                                            "Array structure verified, {} stages found",
                                                            access_log.plugin_logs.len()
                                                        ),
                                                    )
                                                } else {
                                                    TestResult::failed(
                                                        start.elapsed(),
                                                        format!(
                                                            "Expected 2 stages, found {}",
                                                            access_log.plugin_logs.len()
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
            "Verify request_filters stage contains CORS and CSRF plugins",
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
                                            access_log.plugin_logs.iter().find(|s| s.stage == "request_filters")
                                        {
                                            // Check if it has 2 plugins
                                            if stage.logs.len() != 2 {
                                                return TestResult::failed(
                                                    start.elapsed(),
                                                    format!(
                                                        "Expected 2 plugins in request_filters, found {}",
                                                        stage.logs.len()
                                                    ),
                                                );
                                            }

                                            // Check plugin names
                                            let plugin_names: Vec<&str> =
                                                stage.logs.iter().map(|p| p.name.as_str()).collect();
                                            if !plugin_names.contains(&"cors") || !plugin_names.contains(&"csrf") {
                                                return TestResult::failed(
                                                    start.elapsed(),
                                                    format!(
                                                        "Expected 'cors' and 'csrf' plugins, found: {:?}",
                                                        plugin_names
                                                    ),
                                                );
                                            }

                                            // Check time_cost exists for all plugins
                                            for plugin in &stage.logs {
                                                if plugin.time_cost.is_none() {
                                                    return TestResult::failed(
                                                        start.elapsed(),
                                                        format!("Plugin '{}' missing time_cost", plugin.name),
                                                    );
                                                }
                                                if let Some(tc) = plugin.time_cost {
                                                    if tc == 0 {
                                                        return TestResult::failed(
                                                            start.elapsed(),
                                                            format!("Plugin '{}' has time_cost = 0", plugin.name),
                                                        );
                                                    }
                                                }
                                            }

                                            TestResult::passed_with_message(
                                                start.elapsed(),
                                                "CORS and CSRF plugins executed correctly".to_string(),
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

    fn test_upstream_response_filters_stage_details() -> TestCase {
        TestCase::new(
            "upstream_response_filters_stage_details",
            "Verify upstream_response_filters stage details",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let mut request = client.get(&url);
                    request = request.header("host", "plugin-test.example.com");

                    match request.send().await {
                        Ok(response) => {
                            // Check X-Test-Header was added
                            let has_test_header = response.headers().get("x-test-header").is_some();

                            if let Some(debug_header) = response.headers().get("x-debug-access-log") {
                                if let Ok(json_str) = debug_header.to_str() {
                                    if let Ok(access_log) = serde_json::from_str::<AccessLog>(json_str) {
                                        // Find upstream_response_filters stage
                                        if let Some(stage) = access_log
                                            .plugin_logs
                                            .iter()
                                            .find(|s| s.stage == "upstream_response_filters")
                                        {
                                            // Should have 2 plugins
                                            if stage.logs.len() != 2 {
                                                return TestResult::failed(
                                                    start.elapsed(),
                                                    format!("Expected 2 plugins, found {}", stage.logs.len()),
                                                );
                                            }

                                            // Check plugin order: ResponseHeaderModifier, then DebugAccessLogToHeader
                                            if stage.logs[0].name != "ResponseHeaderModifier" {
                                                return TestResult::failed(
                                                    start.elapsed(),
                                                    format!(
                                                        "Expected first plugin to be ResponseHeaderModifier, got {}",
                                                        stage.logs[0].name
                                                    ),
                                                );
                                            }

                                            if stage.logs[1].name != "DebugAccessLogToHeader" {
                                                return TestResult::failed(
                                                    start.elapsed(),
                                                    format!(
                                                        "Expected second plugin to be DebugAccessLogToHeader, got {}",
                                                        stage.logs[1].name
                                                    ),
                                                );
                                            }

                                            if !has_test_header {
                                                return TestResult::failed(
                                                    start.elapsed(),
                                                    "X-Test-Header not found (ResponseHeaderModifier didn't work)"
                                                        .to_string(),
                                                );
                                            }

                                            TestResult::passed_with_message(
                                                start.elapsed(),
                                                "ResponseHeaderModifier and DebugAccessLogToHeader present".to_string(),
                                            )
                                        } else {
                                            TestResult::failed(
                                                start.elapsed(),
                                                "upstream_response_filters stage not found".to_string(),
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

    fn test_stage_execution_order() -> TestCase {
        TestCase::new(
            "stage_execution_order",
            "Verify stages are in correct execution order",
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
                                        if access_log.plugin_logs.len() != 2 {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                format!(
                                                    "Expected exactly 2 stages, got {}",
                                                    access_log.plugin_logs.len()
                                                ),
                                            );
                                        }

                                        // First stage should be request_filters
                                        if access_log.plugin_logs[0].stage != "request_filters" {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                format!(
                                                    "First stage should be request_filters, got {}",
                                                    access_log.plugin_logs[0].stage
                                                ),
                                            );
                                        }

                                        // Second stage should be upstream_response_filters
                                        if access_log.plugin_logs[1].stage != "upstream_response_filters" {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                format!(
                                                    "Second stage should be upstream_response_filters, got {}",
                                                    access_log.plugin_logs[1].stage
                                                ),
                                            );
                                        }

                                        // Verify no upstream_responses stage
                                        if access_log.plugin_logs.iter().any(|s| s.stage == "upstream_responses") {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                "upstream_responses stage should not be present".to_string(),
                                            );
                                        }

                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            "Stages in correct order".to_string(),
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
                                        if access_log.plugin_logs.is_empty() {
                                            TestResult::passed_with_message(
                                                start.elapsed(),
                                                "Empty logs handled correctly (empty array)".to_string(),
                                            )
                                        } else {
                                            TestResult::failed(
                                                start.elapsed(),
                                                format!(
                                                    "Expected empty plugin_logs, got {} stages",
                                                    access_log.plugin_logs.len()
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

                                        for stage in &access_log.plugin_logs {
                                            for plugin in &stage.logs {
                                                match plugin.time_cost {
                                                    None => {
                                                        all_valid = false;
                                                        error_msg = format!(
                                                            "Plugin '{}' in stage '{}' missing time_cost",
                                                            plugin.name, stage.stage
                                                        );
                                                        break;
                                                    }
                                                    Some(tc) => {
                                                        if tc == 0 {
                                                            all_valid = false;
                                                            error_msg =
                                                                format!("Plugin '{}' has time_cost = 0", plugin.name);
                                                            break;
                                                        }
                                                        // Check reasonable range (< 1 second = 1,000,000 microseconds)
                                                        if tc > 1_000_000 {
                                                            all_valid = false;
                                                            error_msg = format!(
                                                                "Plugin '{}' has unreasonable time_cost: {} us",
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
                                            let total_plugins: usize =
                                                access_log.plugin_logs.iter().map(|s| s.logs.len()).sum();
                                            TestResult::passed_with_message(
                                                start.elapsed(),
                                                format!("All {} plugins have valid time_cost", total_plugins),
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
        "Plugin Logs Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_basic_plugin_logs_structure(),
            Self::test_request_filters_stage_details(),
            Self::test_upstream_response_filters_stage_details(),
            Self::test_stage_execution_order(),
            Self::test_empty_plugin_logs(),
            Self::test_plugin_time_cost(),
        ]
    }
}
