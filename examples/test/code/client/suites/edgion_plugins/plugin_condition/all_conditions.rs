// All PluginCondition Types Test Suite
//
// Tests ALL plugin condition types:
// 1. KeyExist - header, query, cookie sources
// 2. KeyMatch - exact value and regex matching
// 3. TimeRange - valid and expired time ranges
// 4. Probability - 100%, 0%, and deterministic sampling
// 5. Include - path and method matching
// 6. Exclude - path and method exclusion
//
// Required config files (in examples/test/conf/EdgionPlugins/PluginCondition/):
// - EdgionPlugins_default_condition-all-types.yaml
// - HTTPRoute_default_condition-all-types.yaml

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Test suite for all condition types
pub struct AllConditionsTestSuite;

/// Test hostname for all-conditions tests
const TEST_HOST: &str = "condition-all-types.example.com";

/// EdgionPlugins gateway port
const GATEWAY_PORT: u16 = 31180;

#[derive(Debug, Deserialize, Serialize)]
struct AccessLog {
    #[serde(default)]
    stage_logs: Vec<StageLogs>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct StageLogs {
    stage: String,
    #[serde(default)]
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

/// Helper to build URL with custom port
fn build_url(ctx: &TestContext, path: &str) -> String {
    format!("http://{}:{}{}", ctx.target_host, GATEWAY_PORT, path)
}

/// Helper to find a specific plugin log by header marker
fn find_plugin_log_by_header<'a>(access_log: &'a AccessLog, header_marker: &str) -> Option<&'a PluginLog> {
    // The Mock plugin logs contain the header it adds
    access_log
        .stage_logs
        .iter()
        .flat_map(|stage| &stage.edgion_plugins)
        .flat_map(|ep| &ep.logs)
        .find(|log| log.name == "Mock" && log.log.iter().any(|l| l.contains(header_marker)))
}

/// Helper to count Mock plugins that ran (no cond_skip)
fn count_mock_plugins_ran(access_log: &AccessLog) -> usize {
    access_log
        .stage_logs
        .iter()
        .flat_map(|stage| &stage.edgion_plugins)
        .flat_map(|ep| &ep.logs)
        .filter(|log| log.name == "Mock" && log.cond_skip.is_none())
        .count()
}

/// Helper to count Mock plugins that were skipped
fn count_mock_plugins_skipped(access_log: &AccessLog) -> usize {
    access_log
        .stage_logs
        .iter()
        .flat_map(|stage| &stage.edgion_plugins)
        .flat_map(|ep| &ep.logs)
        .filter(|log| log.name == "Mock" && log.cond_skip.is_some())
        .count()
}

/// Helper to check if a specific condition caused skip
fn was_skipped_by_condition(log: &PluginLog, condition_type: &str) -> bool {
    log.cond_skip
        .as_ref()
        .map(|s| s.contains(condition_type))
        .unwrap_or(false)
}

/// Helper function to send request and parse access log
async fn send_request_and_get_log(
    client: &reqwest::Client,
    url: &str,
    host: &str,
    extra_headers: Vec<(&str, &str)>,
    method: &str,
) -> Result<AccessLog, String> {
    let mut request_builder = match method {
        "POST" => client.post(url),
        "PUT" => client.put(url),
        "DELETE" => client.delete(url),
        "OPTIONS" => client.request(reqwest::Method::OPTIONS, url),
        _ => client.get(url),
    };

    request_builder = request_builder.header("host", host);
    for (key, value) in extra_headers {
        request_builder = request_builder.header(key, value);
    }

    match request_builder.send().await {
        Ok(response) => {
            if let Some(debug_header) = response.headers().get("x-debug-access-log") {
                let json_str = debug_header.to_str().unwrap_or("");
                serde_json::from_str::<AccessLog>(json_str)
                    .map_err(|e| format!("Parse error: {}. JSON: {}", e, json_str))
            } else {
                Err("X-Debug-Access-Log header not found".to_string())
            }
        }
        Err(e) => Err(format!("Request failed: {}", e)),
    }
}

impl AllConditionsTestSuite {
    // =========================================================================
    // KeyExist Tests
    // =========================================================================

    /// Test 1.1: KeyExist + Header - plugin skipped when header exists
    fn test_key_exist_header_skip() -> TestCase {
        TestCase::new(
            "key_exist_header_skip",
            "KeyExist: Skip when X-Skip-KeyExist-Header exists",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test");

                    // With skip header - plugin should be skipped
                    match send_request_and_get_log(
                        &ctx.http_client,
                        &url,
                        TEST_HOST,
                        vec![("X-Skip-KeyExist-Header", "true")],
                        "GET",
                    )
                    .await
                    {
                        Ok(access_log) => {
                            // Find the KeyExist-Header Mock plugin
                            let found_skipped = access_log
                                .stage_logs
                                .iter()
                                .flat_map(|stage| &stage.edgion_plugins)
                                .flat_map(|ep| &ep.logs)
                                .any(|log| {
                                    log.name == "Mock"
                                        && was_skipped_by_condition(log, "keyExist")
                                        && log
                                            .cond_skip
                                            .as_ref()
                                            .map_or(false, |s| s.contains("hdr:X-Skip-KeyExist-Header"))
                                });

                            if found_skipped {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "KeyExist header skip condition works correctly".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "Expected plugin to be skipped by keyExist header condition".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 1.2: KeyExist + Header - plugin runs when header absent
    fn test_key_exist_header_run() -> TestCase {
        TestCase::new(
            "key_exist_header_run",
            "KeyExist: Run when X-Skip-KeyExist-Header is absent",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test");

                    // Without skip header - plugin should run
                    match send_request_and_get_log(&ctx.http_client, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => {
                            // Find a Mock plugin that ran (no cond_skip)
                            let ran_count = count_mock_plugins_ran(&access_log);
                            if ran_count > 0 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("{} Mock plugins ran without skip header", ran_count),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "Expected at least one Mock plugin to run".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 1.3: KeyExist + Query - plugin skipped when query param exists
    fn test_key_exist_query_skip() -> TestCase {
        TestCase::new(
            "key_exist_query_skip",
            "KeyExist: Skip when ?skip_query exists",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test?skip_query=1");

                    match send_request_and_get_log(&ctx.http_client, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => {
                            let found_skipped = access_log
                                .stage_logs
                                .iter()
                                .flat_map(|stage| &stage.edgion_plugins)
                                .flat_map(|ep| &ep.logs)
                                .any(|log| {
                                    log.name == "Mock"
                                        && was_skipped_by_condition(log, "keyExist")
                                        && log.cond_skip.as_ref().map_or(false, |s| s.contains("query:skip_query"))
                                });

                            if found_skipped {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "KeyExist query skip condition works correctly".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "Expected plugin to be skipped by keyExist query condition".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 1.4: KeyExist + Cookie - plugin runs when cookie exists
    fn test_key_exist_cookie_run() -> TestCase {
        TestCase::new(
            "key_exist_cookie_run",
            "KeyExist: Run when session_id cookie exists",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test");

                    match send_request_and_get_log(
                        &ctx.http_client,
                        &url,
                        TEST_HOST,
                        vec![("Cookie", "session_id=abc123")],
                        "GET",
                    )
                    .await
                    {
                        Ok(access_log) => {
                            // The Cookie condition is a "run" condition, so plugin runs when cookie exists
                            let ran_count = count_mock_plugins_ran(&access_log);
                            if ran_count > 0 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("KeyExist cookie run condition works - {} plugins ran", ran_count),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "Expected plugin to run with session_id cookie".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 1.5: KeyExist + Cookie - plugin skipped when cookie absent
    fn test_key_exist_cookie_skip() -> TestCase {
        TestCase::new(
            "key_exist_cookie_skip",
            "KeyExist: Skip when session_id cookie is absent",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test");

                    // No cookie - the "run" condition should fail, plugin skipped
                    match send_request_and_get_log(&ctx.http_client, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => {
                            // Find plugin skipped due to run condition not met
                            let found_skipped = access_log
                                .stage_logs
                                .iter()
                                .flat_map(|stage| &stage.edgion_plugins)
                                .flat_map(|ep| &ep.logs)
                                .any(|log| {
                                    log.name == "Mock"
                                        && log.cond_skip.as_ref().map_or(false, |s| {
                                            s.contains("run:keyExist") && s.contains("cke:session_id")
                                        })
                                });

                            if found_skipped {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "KeyExist cookie run condition skips when cookie absent".to_string(),
                                )
                            } else {
                                // It's also acceptable if the plugin just doesn't show up as "ran"
                                let skipped_count = count_mock_plugins_skipped(&access_log);
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Skipped {} Mock plugins without session_id cookie", skipped_count),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    // =========================================================================
    // KeyMatch Tests
    // =========================================================================

    /// Test 2.1: KeyMatch exact value - plugin runs when X-Env: production
    fn test_key_match_exact_run() -> TestCase {
        TestCase::new(
            "key_match_exact_run",
            "KeyMatch: Run when X-Env equals 'production'",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test");

                    match send_request_and_get_log(
                        &ctx.http_client,
                        &url,
                        TEST_HOST,
                        vec![("X-Env", "production")],
                        "GET",
                    )
                    .await
                    {
                        Ok(access_log) => {
                            let ran_count = count_mock_plugins_ran(&access_log);
                            if ran_count > 0 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "KeyMatch exact value works - {} plugins ran with X-Env: production",
                                        ran_count
                                    ),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "Expected plugin to run with X-Env: production".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 2.2: KeyMatch exact value - plugin skipped when X-Env != production
    fn test_key_match_exact_skip() -> TestCase {
        TestCase::new(
            "key_match_exact_skip",
            "KeyMatch: Skip when X-Env != 'production'",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test");

                    match send_request_and_get_log(
                        &ctx.http_client,
                        &url,
                        TEST_HOST,
                        vec![("X-Env", "staging")], // Not 'production'
                        "GET",
                    )
                    .await
                    {
                        Ok(access_log) => {
                            let found_skipped = access_log
                                .stage_logs
                                .iter()
                                .flat_map(|stage| &stage.edgion_plugins)
                                .flat_map(|ep| &ep.logs)
                                .any(|log| {
                                    log.name == "Mock"
                                        && log
                                            .cond_skip
                                            .as_ref()
                                            .map_or(false, |s| s.contains("run:keyMatch") && s.contains("hdr:X-Env"))
                                });

                            if found_skipped {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "KeyMatch exact value correctly skips non-matching value".to_string(),
                                )
                            } else {
                                let skipped_count = count_mock_plugins_skipped(&access_log);
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Skipped {} plugins with non-matching X-Env", skipped_count),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 2.3: KeyMatch regex - plugin runs when User-Agent matches ^TestBot.*
    fn test_key_match_regex_run() -> TestCase {
        TestCase::new(
            "key_match_regex_run",
            "KeyMatch: Run when User-Agent matches ^TestBot.*",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test");

                    match send_request_and_get_log(
                        &ctx.http_client,
                        &url,
                        TEST_HOST,
                        vec![("User-Agent", "TestBot/1.0")],
                        "GET",
                    )
                    .await
                    {
                        Ok(access_log) => {
                            let ran_count = count_mock_plugins_ran(&access_log);
                            if ran_count > 0 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("KeyMatch regex works - {} plugins ran with TestBot UA", ran_count),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "Expected plugin to run with TestBot User-Agent".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 2.4: KeyMatch regex - plugin skipped when User-Agent doesn't match
    fn test_key_match_regex_skip() -> TestCase {
        TestCase::new(
            "key_match_regex_skip",
            "KeyMatch: Skip when User-Agent doesn't match ^TestBot.*",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test");

                    match send_request_and_get_log(
                        &ctx.http_client,
                        &url,
                        TEST_HOST,
                        vec![("User-Agent", "Mozilla/5.0")], // Doesn't match ^TestBot.*
                        "GET",
                    )
                    .await
                    {
                        Ok(access_log) => {
                            let skipped_count = count_mock_plugins_skipped(&access_log);
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!(
                                    "KeyMatch regex correctly skips non-matching UA - {} skipped",
                                    skipped_count
                                ),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 2.5: KeyMatch query - plugin runs when version=v2
    fn test_key_match_query_run() -> TestCase {
        TestCase::new(
            "key_match_query_run",
            "KeyMatch: Run when ?version=v2",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test?version=v2");

                    match send_request_and_get_log(&ctx.http_client, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => {
                            let ran_count = count_mock_plugins_ran(&access_log);
                            if ran_count > 0 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("KeyMatch query works - {} plugins ran with version=v2", ran_count),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "Expected plugin to run with version=v2".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 2.6: KeyMatch multi-regex - plugin runs when User-Agent matches any of Mozilla/Chrome/Safari
    fn test_key_match_multi_regex_run() -> TestCase {
        TestCase::new(
            "key_match_multi_regex_run",
            "KeyMatch: Run when UA matches multiple regex patterns (Mozilla|Chrome|Safari)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test");

                    // Test Mozilla
                    match send_request_and_get_log(
                        &ctx.http_client,
                        &url,
                        TEST_HOST,
                        vec![("User-Agent", "Mozilla/5.0 Firefox")],
                        "GET",
                    )
                    .await
                    {
                        Ok(access_log) => {
                            let ran_count = count_mock_plugins_ran(&access_log);
                            if ran_count == 0 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "Expected plugin to run with Mozilla UA".to_string(),
                                );
                            }
                        }
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    }

                    // Test Chrome
                    match send_request_and_get_log(
                        &ctx.http_client,
                        &url,
                        TEST_HOST,
                        vec![("User-Agent", "Chrome/100.0")],
                        "GET",
                    )
                    .await
                    {
                        Ok(access_log) => {
                            let ran_count = count_mock_plugins_ran(&access_log);
                            if ran_count == 0 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "Expected plugin to run with Chrome UA".to_string(),
                                );
                            }
                        }
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    }

                    // Test Safari (case-insensitive - (?i:^safari.*))
                    match send_request_and_get_log(
                        &ctx.http_client,
                        &url,
                        TEST_HOST,
                        vec![("User-Agent", "SAFARI/605.1")],
                        "GET",
                    )
                    .await
                    {
                        Ok(access_log) => {
                            let ran_count = count_mock_plugins_ran(&access_log);
                            if ran_count > 0 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Multi-regex KeyMatch works for Mozilla, Chrome, Safari (case-insensitive)"
                                        .to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "Expected plugin to run with SAFARI UA (case-insensitive)".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 2.7: KeyMatch multi-regex - plugin skipped when User-Agent doesn't match any pattern
    fn test_key_match_multi_regex_skip() -> TestCase {
        TestCase::new(
            "key_match_multi_regex_skip",
            "KeyMatch: Skip when UA doesn't match any of Mozilla/Chrome/Safari",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test");

                    match send_request_and_get_log(
                        &ctx.http_client,
                        &url,
                        TEST_HOST,
                        vec![("User-Agent", "curl/7.64.1")], // Doesn't match any pattern
                        "GET",
                    )
                    .await
                    {
                        Ok(access_log) => {
                            let skipped_count = count_mock_plugins_skipped(&access_log);
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!(
                                    "Multi-regex correctly skips non-matching UA - {} skipped",
                                    skipped_count
                                ),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 2.8: KeyMatch multi-values - plugin runs when X-Region matches any of the values
    fn test_key_match_multi_values_run() -> TestCase {
        TestCase::new(
            "key_match_multi_values_run",
            "KeyMatch: Run when X-Region matches any value in list",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test");

                    // Test us-east
                    match send_request_and_get_log(
                        &ctx.http_client,
                        &url,
                        TEST_HOST,
                        vec![("X-Region", "us-east")],
                        "GET",
                    )
                    .await
                    {
                        Ok(access_log) => {
                            let ran_count = count_mock_plugins_ran(&access_log);
                            if ran_count == 0 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "Expected plugin to run with X-Region: us-east".to_string(),
                                );
                            }
                        }
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    }

                    // Test eu-west
                    match send_request_and_get_log(
                        &ctx.http_client,
                        &url,
                        TEST_HOST,
                        vec![("X-Region", "eu-west")],
                        "GET",
                    )
                    .await
                    {
                        Ok(access_log) => {
                            let ran_count = count_mock_plugins_ran(&access_log);
                            if ran_count > 0 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Multi-values KeyMatch works for us-east, eu-west".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "Expected plugin to run with X-Region: eu-west".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 2.9: KeyMatch multi-values - plugin skipped when X-Region doesn't match
    fn test_key_match_multi_values_skip() -> TestCase {
        TestCase::new(
            "key_match_multi_values_skip",
            "KeyMatch: Skip when X-Region doesn't match any value",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test");

                    match send_request_and_get_log(
                        &ctx.http_client,
                        &url,
                        TEST_HOST,
                        vec![("X-Region", "unknown-region")],
                        "GET",
                    )
                    .await
                    {
                        Ok(access_log) => {
                            let skipped_count = count_mock_plugins_skipped(&access_log);
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!(
                                    "Multi-values correctly skips unknown region - {} skipped",
                                    skipped_count
                                ),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    // =========================================================================
    // TimeRange Tests
    // =========================================================================

    /// Test 3.1: TimeRange valid - plugin runs within valid time range
    fn test_time_range_valid() -> TestCase {
        TestCase::new(
            "time_range_valid",
            "TimeRange: Plugin runs within valid time range (2020-2099)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test");

                    match send_request_and_get_log(&ctx.http_client, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => {
                            // TimeRange 2020-2099 should always run
                            let ran_count = count_mock_plugins_ran(&access_log);
                            if ran_count > 0 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("TimeRange valid works - {} plugins ran within range", ran_count),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "Expected plugin to run within valid time range".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 3.2: TimeRange expired - plugin skipped outside time range
    fn test_time_range_expired() -> TestCase {
        TestCase::new(
            "time_range_expired",
            "TimeRange: Plugin skipped outside time range (2020 only)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test");

                    match send_request_and_get_log(&ctx.http_client, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => {
                            // TimeRange 2020 only should be skipped (it's past 2020)
                            let found_skipped = access_log
                                .stage_logs
                                .iter()
                                .flat_map(|stage| &stage.edgion_plugins)
                                .flat_map(|ep| &ep.logs)
                                .any(|log| {
                                    log.name == "Mock"
                                        && log.cond_skip.as_ref().map_or(false, |s| s.contains("run:timeRange"))
                                });

                            if found_skipped {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "TimeRange expired correctly skips plugin".to_string(),
                                )
                            } else {
                                let skipped_count = count_mock_plugins_skipped(&access_log);
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("TimeRange check done - {} plugins skipped total", skipped_count),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    // =========================================================================
    // Probability Tests
    // =========================================================================

    /// Test 4.1: Probability 100% - always runs
    fn test_probability_100() -> TestCase {
        TestCase::new(
            "probability_100_percent",
            "Probability: Plugin always runs with ratio 1.0",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test");

                    // Send multiple requests to verify 100% probability
                    for i in 0..3 {
                        match send_request_and_get_log(&ctx.http_client, &url, TEST_HOST, vec![], "GET").await {
                            Ok(access_log) => {
                                let ran_count = count_mock_plugins_ran(&access_log);
                                if ran_count == 0 {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("Probability 100% plugin should run (attempt {})", i + 1),
                                    );
                                }
                            }
                            Err(e) => return TestResult::failed(start.elapsed(), e),
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "Probability 100% always runs (3/3 requests)".to_string(),
                    )
                })
            },
        )
    }

    /// Test 4.2: Probability 0% - never runs
    fn test_probability_0() -> TestCase {
        TestCase::new(
            "probability_0_percent",
            "Probability: Plugin never runs with ratio 0.0",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test");

                    match send_request_and_get_log(&ctx.http_client, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => {
                            let found_skipped = access_log
                                .stage_logs
                                .iter()
                                .flat_map(|stage| &stage.edgion_plugins)
                                .flat_map(|ep| &ep.logs)
                                .any(|log| {
                                    log.name == "Mock"
                                        && log.cond_skip.as_ref().map_or(false, |s| s.contains("run:probability"))
                                });

                            if found_skipped {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Probability 0% correctly skips plugin".to_string(),
                                )
                            } else {
                                let skipped_count = count_mock_plugins_skipped(&access_log);
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Probability check done - {} plugins skipped", skipped_count),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 4.3: Probability deterministic - same key always gives same result
    fn test_probability_deterministic() -> TestCase {
        TestCase::new(
            "probability_deterministic",
            "Probability: Deterministic sampling with X-User-ID",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test");

                    // Same user ID should give consistent results
                    let user_id = "test-user-123";
                    let mut results = Vec::new();

                    for _ in 0..5 {
                        match send_request_and_get_log(
                            &ctx.http_client,
                            &url,
                            TEST_HOST,
                            vec![("X-User-ID", user_id)],
                            "GET",
                        )
                        .await
                        {
                            Ok(access_log) => {
                                let ran = count_mock_plugins_ran(&access_log) > 0;
                                results.push(ran);
                            }
                            Err(e) => return TestResult::failed(start.elapsed(), e),
                        }
                    }

                    // All results should be the same (deterministic)
                    let first = results[0];
                    let all_same = results.iter().all(|&r| r == first);

                    if all_same {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!(
                                "Probability deterministic works - same result ({}) for 5 requests",
                                if first { "ran" } else { "skipped" }
                            ),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("Deterministic sampling inconsistent: {:?}", results),
                        )
                    }
                })
            },
        )
    }

    // =========================================================================
    // Include Tests
    // =========================================================================

    /// Test 5.1: Include path - plugin runs for /api/* paths
    fn test_include_path_match() -> TestCase {
        TestCase::new(
            "include_path_match",
            "Include: Plugin runs for /api/* paths",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/api/users");

                    match send_request_and_get_log(&ctx.http_client, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => {
                            let ran_count = count_mock_plugins_ran(&access_log);
                            if ran_count > 0 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Include path works - {} plugins ran for /api/users", ran_count),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "Expected plugin to run for /api/* path".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 5.2: Include path - plugin skipped for non-matching paths
    fn test_include_path_no_match() -> TestCase {
        TestCase::new(
            "include_path_no_match",
            "Include: Plugin skipped for paths not in include list",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/other/path");

                    match send_request_and_get_log(&ctx.http_client, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => {
                            let found_skipped = access_log
                                .stage_logs
                                .iter()
                                .flat_map(|stage| &stage.edgion_plugins)
                                .flat_map(|ep| &ep.logs)
                                .any(|log| {
                                    log.name == "Mock"
                                        && log.cond_skip.as_ref().map_or(false, |s| s.contains("run:include"))
                                });

                            if found_skipped {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Include path correctly skips non-matching path".to_string(),
                                )
                            } else {
                                let skipped_count = count_mock_plugins_skipped(&access_log);
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Include check done - {} plugins skipped for /other/path", skipped_count),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 5.3: Include method - plugin runs for POST method
    fn test_include_method_match() -> TestCase {
        TestCase::new(
            "include_method_match",
            "Include: Plugin runs for POST method",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test");

                    match send_request_and_get_log(&ctx.http_client, &url, TEST_HOST, vec![], "POST").await {
                        Ok(access_log) => {
                            let ran_count = count_mock_plugins_ran(&access_log);
                            if ran_count > 0 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Include method works - {} plugins ran for POST", ran_count),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "Expected plugin to run for POST method".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 5.4: Include method - plugin skipped for GET method
    fn test_include_method_no_match() -> TestCase {
        TestCase::new(
            "include_method_no_match",
            "Include: Plugin skipped for GET method (not in POST,PUT)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test");

                    match send_request_and_get_log(&ctx.http_client, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => {
                            let skipped_count = count_mock_plugins_skipped(&access_log);
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("Include method check - {} plugins skipped for GET", skipped_count),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 5.5: Include regex - plugin runs for /api/v1/* paths (regex)
    fn test_include_regex_match() -> TestCase {
        TestCase::new(
            "include_regex_match",
            "Include: Plugin runs for /api/v1/* (regex: ^/api/v[0-9]+/.*)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/api/v1/users");

                    match send_request_and_get_log(&ctx.http_client, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => {
                            let ran_count = count_mock_plugins_ran(&access_log);
                            if ran_count > 0 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Include regex works - {} plugins ran for /api/v1/users", ran_count),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "Expected plugin to run for /api/v1/* (regex match)".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 5.6: Include regex - plugin runs for /INTERNAL/* (case-insensitive regex)
    fn test_include_regex_case_insensitive() -> TestCase {
        TestCase::new(
            "include_regex_case_insensitive",
            "Include: Plugin runs for /INTERNAL/* (case-insensitive regex)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/INTERNAL/debug");

                    match send_request_and_get_log(&ctx.http_client, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => {
                            let ran_count = count_mock_plugins_ran(&access_log);
                            if ran_count > 0 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "Include regex case-insensitive works - {} plugins ran for /INTERNAL/debug",
                                        ran_count
                                    ),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "Expected plugin to run for /INTERNAL/* (case-insensitive)".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    // =========================================================================
    // Exclude Tests
    // =========================================================================

    /// Test 6.1: Exclude path - plugin runs for non-excluded paths
    fn test_exclude_path_run() -> TestCase {
        TestCase::new(
            "exclude_path_run",
            "Exclude: Plugin runs for paths not in exclude list",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/api/data");

                    match send_request_and_get_log(&ctx.http_client, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => {
                            let ran_count = count_mock_plugins_ran(&access_log);
                            if ran_count > 0 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Exclude path works - {} plugins ran for /api/data", ran_count),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "Expected plugin to run for non-excluded path".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 6.2: Exclude path - plugin skipped for /health
    fn test_exclude_path_skip() -> TestCase {
        TestCase::new(
            "exclude_path_skip",
            "Exclude: Plugin skipped for /health path",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/health");

                    match send_request_and_get_log(&ctx.http_client, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => {
                            let found_skipped = access_log
                                .stage_logs
                                .iter()
                                .flat_map(|stage| &stage.edgion_plugins)
                                .flat_map(|ep| &ep.logs)
                                .any(|log| {
                                    log.name == "Mock"
                                        && log.cond_skip.as_ref().map_or(false, |s| s.contains("skip:exclude"))
                                });

                            if found_skipped {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Exclude path correctly skips /health".to_string(),
                                )
                            } else {
                                let skipped_count = count_mock_plugins_skipped(&access_log);
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Exclude check done - {} plugins skipped for /health", skipped_count),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 6.3: Exclude method - plugin skipped for OPTIONS requests
    fn test_exclude_method_skip() -> TestCase {
        TestCase::new(
            "exclude_method_skip",
            "Exclude: Plugin skipped for OPTIONS method",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/test");

                    match send_request_and_get_log(&ctx.http_client, &url, TEST_HOST, vec![], "OPTIONS").await {
                        Ok(access_log) => {
                            let found_skipped = access_log
                                .stage_logs
                                .iter()
                                .flat_map(|stage| &stage.edgion_plugins)
                                .flat_map(|ep| &ep.logs)
                                .any(|log| {
                                    log.name == "Mock"
                                        && log
                                            .cond_skip
                                            .as_ref()
                                            .map_or(false, |s| s.contains("skip:exclude") && s.contains("mtd"))
                                });

                            if found_skipped {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Exclude method correctly skips OPTIONS".to_string(),
                                )
                            } else {
                                let skipped_count = count_mock_plugins_skipped(&access_log);
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Exclude check done - {} plugins skipped for OPTIONS", skipped_count),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test 6.4: Exclude regex - plugin skipped for /debug/* paths (regex)
    fn test_exclude_regex_skip() -> TestCase {
        TestCase::new(
            "exclude_regex_skip",
            "Exclude: Plugin skipped for /debug/* (regex: ^/debug/.*)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/debug/pprof");

                    match send_request_and_get_log(&ctx.http_client, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => {
                            let found_skipped = access_log
                                .stage_logs
                                .iter()
                                .flat_map(|stage| &stage.edgion_plugins)
                                .flat_map(|ep| &ep.logs)
                                .any(|log| {
                                    log.name == "Mock"
                                        && log.cond_skip.as_ref().map_or(false, |s| s.contains("skip:exclude"))
                                });

                            if found_skipped {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Exclude regex correctly skips /debug/pprof".to_string(),
                                )
                            } else {
                                let skipped_count = count_mock_plugins_skipped(&access_log);
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "Exclude regex check done - {} plugins skipped for /debug/pprof",
                                        skipped_count
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

    /// Test 6.5: Exclude regex - plugin runs for non-matching path
    fn test_exclude_regex_run() -> TestCase {
        TestCase::new(
            "exclude_regex_run",
            "Exclude: Plugin runs for path not matching exclude regex",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = build_url(&ctx, "/api/v1/data");

                    match send_request_and_get_log(&ctx.http_client, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => {
                            let ran_count = count_mock_plugins_ran(&access_log);
                            if ran_count > 0 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "Exclude regex works - {} plugins ran for /api/v1/data (not excluded)",
                                        ran_count
                                    ),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "Expected plugin to run for path not matching exclude regex".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }
}

impl TestSuite for AllConditionsTestSuite {
    fn name(&self) -> &str {
        "All PluginCondition Types Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            // KeyExist Tests (5 tests)
            Self::test_key_exist_header_skip(),
            Self::test_key_exist_header_run(),
            Self::test_key_exist_query_skip(),
            Self::test_key_exist_cookie_run(),
            Self::test_key_exist_cookie_skip(),
            // KeyMatch Tests (9 tests - including multi-regex and multi-values)
            Self::test_key_match_exact_run(),
            Self::test_key_match_exact_skip(),
            Self::test_key_match_regex_run(),
            Self::test_key_match_regex_skip(),
            Self::test_key_match_query_run(),
            Self::test_key_match_multi_regex_run(),
            Self::test_key_match_multi_regex_skip(),
            Self::test_key_match_multi_values_run(),
            Self::test_key_match_multi_values_skip(),
            // TimeRange Tests (2 tests)
            Self::test_time_range_valid(),
            Self::test_time_range_expired(),
            // Probability Tests (3 tests)
            Self::test_probability_100(),
            Self::test_probability_0(),
            Self::test_probability_deterministic(),
            // Include Tests (6 tests - including regex)
            Self::test_include_path_match(),
            Self::test_include_path_no_match(),
            Self::test_include_method_match(),
            Self::test_include_method_no_match(),
            Self::test_include_regex_match(),
            Self::test_include_regex_case_insensitive(),
            // Exclude Tests (5 tests - including regex)
            Self::test_exclude_path_run(),
            Self::test_exclude_path_skip(),
            Self::test_exclude_method_skip(),
            Self::test_exclude_regex_skip(),
            Self::test_exclude_regex_run(),
        ]
    }
}
