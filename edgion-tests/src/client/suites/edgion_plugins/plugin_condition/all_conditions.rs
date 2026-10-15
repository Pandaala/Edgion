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
// Required config files (in edgion-tests/integration/conf/EdgionPlugins/PluginCondition/):
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
    #[serde(default)]
    alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cond_skip: Option<String>,
    #[serde(default)]
    log: Vec<String>,
}

/// Helper to build URL with custom port
fn build_url(ctx: &TestContext, path: &str) -> String {
    format!("http://{}:{}{}", ctx.target_host, GATEWAY_PORT, path)
}

/// Observe exactly one fixture entry, including entries skipped before Mock runs.
fn target_outcome(access_log: &AccessLog, alias: &str, skip_cause: &str) -> Result<bool, String> {
    let mut targets = access_log
        .stage_logs
        .iter()
        .filter(|stage| stage.stage == "request_filters")
        .flat_map(|stage| &stage.edgion_plugins)
        .filter(|resource| resource.name == "condition-all-types")
        .flat_map(|resource| &resource.logs)
        .filter(|log| log.name == "Mock" && log.alias.as_deref() == Some(alias));
    let target = targets
        .next()
        .ok_or_else(|| format!("Missing target Mock alias {alias}"))?;
    if targets.next().is_some() {
        return Err(format!("Duplicate target Mock alias {alias}"));
    }
    match target.cond_skip.as_deref() {
        None => Ok(true),
        Some(cause) if cause == skip_cause => Ok(false),
        Some(cause) => Err(format!(
            "Target {alias} skipped with {cause:?}, expected {skip_cause:?}"
        )),
    }
}

fn assert_target(access_log: &AccessLog, alias: &str, expected_run: bool, skip_cause: &str) -> Result<(), String> {
    let ran = target_outcome(access_log, alias, skip_cause)?;
    if ran != expected_run {
        return Err(format!(
            "Target {alias}: expected {}, observed {}",
            if expected_run { "run" } else { "skip" },
            if ran { "run" } else { "skip" }
        ));
    }
    Ok(())
}

fn target_result(
    start: Instant,
    access_log: &AccessLog,
    alias: &str,
    expected_run: bool,
    skip_cause: &str,
) -> TestResult {
    match assert_target(access_log, alias, expected_run, skip_cause) {
        Ok(()) => TestResult::passed_with_message(
            start.elapsed(),
            format!(
                "Target {alias} correctly {}",
                if expected_run { "ran" } else { "skipped" }
            ),
        ),
        Err(error) => TestResult::failed(start.elapsed(), error),
    }
}

fn consistent_samples(results: &[bool]) -> bool {
    results.len() == 5 && results.iter().all(|result| *result == results[0])
}

/// Helper function to send request and parse access log
async fn send_request_and_get_log(
    ctx: &TestContext,
    url: &str,
    host: &str,
    extra_headers: Vec<(&str, &str)>,
    method: &str,
) -> Result<AccessLog, String> {
    let client = &ctx.http_client;
    let trace_id = format!("test-all-cond-{}", uuid::Uuid::new_v4());

    let mut request_builder = match method {
        "POST" => client.post(url),
        "PUT" => client.put(url),
        "DELETE" => client.delete(url),
        "OPTIONS" => client.request(reqwest::Method::OPTIONS, url),
        _ => client.get(url),
    };

    request_builder = request_builder
        .header("host", host)
        .header(
            "traceparent",
            crate::access_log_client::traceparent_for_marker(&trace_id),
        )
        .header("access_log", "test_store");

    for (key, value) in extra_headers {
        request_builder = request_builder.header(key, value);
    }

    match request_builder.send().await {
        Ok(_) => {
            // Fetch access log from store
            let al_client = ctx.access_log_client();
            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                Ok(entry) => serde_json::from_value::<AccessLog>(entry.data).map_err(|e| format!("Parse error: {}", e)),
                Err(e) => Err(format!("Failed to fetch access log: {}", e)),
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
                        &ctx,
                        &url,
                        TEST_HOST,
                        vec![("X-Skip-KeyExist-Header", "true")],
                        "GET",
                    )
                    .await
                    {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "key-exist-header",
                            false,
                            "skip:skip.anyOf[0]:keyExist:hdr:X-Skip-KeyExist-Header",
                        ),
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
                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "key-exist-header",
                            true,
                            "skip:skip.anyOf[0]:keyExist:hdr:X-Skip-KeyExist-Header",
                        ),
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

                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "key-exist-query",
                            false,
                            "skip:skip.anyOf[0]:keyExist:query:skip_query",
                        ),
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

                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![("Cookie", "session_id=abc123")], "GET")
                        .await
                    {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "key-exist-cookie",
                            true,
                            "!run:run.allOf[0]:keyExist:cookie:session_id",
                        ),
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
                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "key-exist-cookie",
                            false,
                            "!run:run.allOf[0]:keyExist:cookie:session_id",
                        ),
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

                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![("X-Env", "production")], "GET").await {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "key-match-exact",
                            true,
                            "!run:run.allOf[0]:keyMatch:hdr:X-Env",
                        ),
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
                        &ctx,
                        &url,
                        TEST_HOST,
                        vec![("X-Env", "staging")], // Not 'production'
                        "GET",
                    )
                    .await
                    {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "key-match-exact",
                            false,
                            "!run:run.allOf[0]:keyMatch:hdr:X-Env",
                        ),
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

                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![("User-Agent", "TestBot/1.0")], "GET")
                        .await
                    {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "key-match-regex",
                            true,
                            "!run:run.allOf[0]:keyMatch:hdr:User-Agent",
                        ),
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
                        &ctx,
                        &url,
                        TEST_HOST,
                        vec![("User-Agent", "Mozilla/5.0")], // Doesn't match ^TestBot.*
                        "GET",
                    )
                    .await
                    {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "key-match-regex",
                            false,
                            "!run:run.allOf[0]:keyMatch:hdr:User-Agent",
                        ),
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

                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "key-match-query",
                            true,
                            "!run:run.allOf[0]:keyMatch:query:version",
                        ),
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
                        &ctx,
                        &url,
                        TEST_HOST,
                        vec![("User-Agent", "Mozilla/5.0 Firefox")],
                        "GET",
                    )
                    .await
                    {
                        Ok(access_log) => {
                            if let Err(error) = assert_target(
                                &access_log,
                                "key-match-multi-regex",
                                true,
                                "!run:run.allOf[0]:keyMatch:hdr:User-Agent",
                            ) {
                                return TestResult::failed(start.elapsed(), error);
                            }
                        }
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    }

                    // Test Chrome
                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![("User-Agent", "Chrome/100.0")], "GET")
                        .await
                    {
                        Ok(access_log) => {
                            if let Err(error) = assert_target(
                                &access_log,
                                "key-match-multi-regex",
                                true,
                                "!run:run.allOf[0]:keyMatch:hdr:User-Agent",
                            ) {
                                return TestResult::failed(start.elapsed(), error);
                            }
                        }
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    }

                    // Test Safari (case-insensitive - (?i:^safari.*))
                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![("User-Agent", "SAFARI/605.1")], "GET")
                        .await
                    {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "key-match-multi-regex",
                            true,
                            "!run:run.allOf[0]:keyMatch:hdr:User-Agent",
                        ),
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
                        &ctx,
                        &url,
                        TEST_HOST,
                        vec![("User-Agent", "curl/7.64.1")], // Doesn't match any pattern
                        "GET",
                    )
                    .await
                    {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "key-match-multi-regex",
                            false,
                            "!run:run.allOf[0]:keyMatch:hdr:User-Agent",
                        ),
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
                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![("X-Region", "us-east")], "GET").await {
                        Ok(access_log) => {
                            if let Err(error) = assert_target(
                                &access_log,
                                "key-match-multi-values",
                                true,
                                "!run:run.allOf[0]:keyMatch:hdr:X-Region",
                            ) {
                                return TestResult::failed(start.elapsed(), error);
                            }
                        }
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    }

                    // Test eu-west
                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![("X-Region", "eu-west")], "GET").await {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "key-match-multi-values",
                            true,
                            "!run:run.allOf[0]:keyMatch:hdr:X-Region",
                        ),
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

                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![("X-Region", "unknown-region")], "GET")
                        .await
                    {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "key-match-multi-values",
                            false,
                            "!run:run.allOf[0]:keyMatch:hdr:X-Region",
                        ),
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

                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "time-range-valid",
                            true,
                            "!run:run.allOf[0]:timeRange",
                        ),
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

                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "time-range-expired",
                            false,
                            "!run:run.allOf[0]:timeRange",
                        ),
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
                    for _ in 0..3 {
                        match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![], "GET").await {
                            Ok(access_log) => {
                                if let Err(error) =
                                    assert_target(&access_log, "probability-100", true, "!run:run.allOf[0]:probability")
                                {
                                    return TestResult::failed(start.elapsed(), error);
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

                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "probability-0",
                            false,
                            "!run:run.allOf[0]:probability",
                        ),
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
                        match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![("X-User-ID", user_id)], "GET").await
                        {
                            Ok(access_log) => {
                                match target_outcome(
                                    &access_log,
                                    "probability-deterministic",
                                    "!run:run.allOf[0]:probability",
                                ) {
                                    Ok(ran) => results.push(ran),
                                    Err(error) => return TestResult::failed(start.elapsed(), error),
                                }
                            }
                            Err(e) => return TestResult::failed(start.elapsed(), e),
                        }
                    }

                    // All results should be the same (deterministic)
                    let first = results[0];
                    let all_same = consistent_samples(&results);

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

                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "include-path",
                            true,
                            "!run:run.allOf[0]:keyMatch:path",
                        ),
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

                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "include-path",
                            false,
                            "!run:run.allOf[0]:keyMatch:path",
                        ),
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

                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![], "POST").await {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "include-method",
                            true,
                            "!run:run.allOf[0]:keyMatch:method",
                        ),
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

                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "include-method",
                            false,
                            "!run:run.allOf[0]:keyMatch:method",
                        ),
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

                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "include-regex",
                            true,
                            "!run:run.allOf[0]:keyMatch:path",
                        ),
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

                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "include-regex",
                            true,
                            "!run:run.allOf[0]:keyMatch:path",
                        ),
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

                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "exclude-path",
                            true,
                            "skip:skip.anyOf[0]:keyMatch:path",
                        ),
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

                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "exclude-path",
                            false,
                            "skip:skip.anyOf[0]:keyMatch:path",
                        ),
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

                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![], "OPTIONS").await {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "exclude-method",
                            false,
                            "skip:skip.anyOf[0]:keyMatch:method",
                        ),
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

                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "exclude-regex",
                            false,
                            "skip:skip.anyOf[0]:keyMatch:path",
                        ),
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

                    match send_request_and_get_log(&ctx, &url, TEST_HOST, vec![], "GET").await {
                        Ok(access_log) => target_result(
                            start,
                            &access_log,
                            "exclude-regex",
                            true,
                            "skip:skip.anyOf[0]:keyMatch:path",
                        ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ALIAS: &str = "probability-deterministic";
    const CAUSE: &str = "!run:run.allOf[0]:probability";

    fn observation(cause: Option<&str>) -> AccessLog {
        serde_json::from_value(json!({"stage_logs": [{
            "stage": "request_filters",
            "edgion_plugins": [{"name": "condition-all-types", "logs": [
                {"name": "Mock", "alias": "probability-100"},
                {"name": "Mock", "alias": ALIAS, "cond_skip": cause}
            ]}]
        }]}))
        .unwrap()
    }

    #[test]
    fn target_identity_is_required_despite_running_siblings() {
        let empty = AccessLog { stage_logs: vec![] };
        assert!(assert_target(&empty, ALIAS, true, CAUSE).is_err());
        assert!(assert_target(&empty, ALIAS, false, CAUSE).is_err());
        let mut missing = observation(None);
        missing.stage_logs[0].edgion_plugins[0].logs.pop();
        assert!(assert_target(&missing, ALIAS, true, CAUSE).is_err());
        for field in ["alias", "stage", "resource", "plugin"] {
            let mut log = observation(None);
            match field {
                "alias" => log.stage_logs[0].edgion_plugins[0].logs[1].alias = None,
                "stage" => log.stage_logs[0].stage = "upstream_response_filters".into(),
                "resource" => log.stage_logs[0].edgion_plugins[0].name = "other-resource".into(),
                "plugin" => log.stage_logs[0].edgion_plugins[0].logs[1].name = "Other".into(),
                _ => unreachable!(),
            }
            assert!(target_outcome(&log, ALIAS, CAUSE).is_err(), "{field}");
        }
    }

    #[test]
    fn duplicates_across_matching_containers_fail() {
        for scope in ["entry", "resource", "stage"] {
            let mut log = observation(None);
            match scope {
                "entry" => {
                    let duplicate = log.stage_logs[0].edgion_plugins[0].logs[1].clone();
                    log.stage_logs[0].edgion_plugins[0].logs.push(duplicate);
                }
                "resource" => {
                    let duplicate = log.stage_logs[0].edgion_plugins[0].clone();
                    log.stage_logs[0].edgion_plugins.push(duplicate);
                }
                "stage" => log.stage_logs.push(log.stage_logs[0].clone()),
                _ => unreachable!(),
            }
            assert!(target_outcome(&log, ALIAS, CAUSE).is_err(), "{scope}");
        }
    }

    #[test]
    fn exact_target_outcome_and_current_cause_are_required() {
        let run = observation(None);
        let skip = observation(Some(CAUSE));
        assert!(assert_target(&run, ALIAS, true, CAUSE).is_ok());
        assert!(assert_target(&skip, ALIAS, false, CAUSE).is_ok());
        assert!(assert_target(&run, ALIAS, false, CAUSE).is_err());
        assert!(assert_target(&skip, ALIAS, true, CAUSE).is_err());
        for wrong in [
            "!run:probability",
            "!run:0:probability",
            "!run:run.allOf[1]:probability",
            "skip:run.allOf[0]:probability",
            "!run:run.allOf[0]:timeRange",
        ] {
            assert!(
                target_outcome(&observation(Some(wrong)), ALIAS, CAUSE).is_err(),
                "{wrong}"
            );
        }
        // A target elsewhere does not make the valid scoped observation ambiguous.
        let mut log = observation(None);
        let mut other = log.stage_logs[0].clone();
        other.stage = "upstream_response_filters".into();
        log.stage_logs.push(other);
        assert!(assert_target(&log, ALIAS, true, CAUSE).is_ok());
    }

    #[test]
    fn deterministic_samples_observe_the_target_on_every_request() {
        let outcomes = [None, None, Some(CAUSE), None, None].map(observation);
        let results: Result<Vec<_>, _> = outcomes.iter().map(|log| target_outcome(log, ALIAS, CAUSE)).collect();
        assert!(!consistent_samples(&results.unwrap()));
        assert!(consistent_samples(&[true; 5]));
        assert!(consistent_samples(&[false; 5]));
        assert!(!consistent_samples(&[]));
        assert!(!consistent_samples(&[true; 4]));
        let mut outcomes = [None; 5].map(observation);
        outcomes[4].stage_logs[0].edgion_plugins[0].logs.pop();
        let results: Result<Vec<_>, _> = outcomes.iter().map(|log| target_outcome(log, ALIAS, CAUSE)).collect();
        assert!(results.is_err());
    }

    #[test]
    fn fixture_aliases_and_exclusion_polarity_match_in_both_trees() {
        let local: serde_yaml::Value = serde_yaml::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/conf/EdgionPlugins/PluginCondition/EdgionPlugins_default_condition-all-types.yaml"
        )))
        .unwrap();
        let kubernetes: serde_yaml::Value = serde_yaml::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/k8s/conf/EdgionPlugins/PluginCondition/EdgionPlugins_default_condition-all-types.yaml"
        )))
        .unwrap();
        assert_eq!(local, kubernetes);
        let entries = local["spec"]["requestPlugins"].as_sequence().unwrap();
        let aliases: Vec<_> = entries.iter().map(|entry| entry["alias"].as_str().unwrap()).collect();
        assert_eq!(
            aliases,
            [
                "key-exist-header",
                "key-exist-query",
                "key-exist-cookie",
                "key-match-exact",
                "key-match-regex",
                "key-match-query",
                "key-match-multi-regex",
                "key-match-multi-values",
                "time-range-valid",
                "time-range-expired",
                "probability-100",
                "probability-0",
                "probability-deterministic",
                "include-path",
                "include-method",
                "include-regex",
                "exclude-path",
                "exclude-method",
                "exclude-regex"
            ]
        );
        for entry in entries
            .iter()
            .filter(|entry| entry["alias"].as_str().unwrap().starts_with("exclude-"))
        {
            assert!(entry["conditions"]["skip"]["anyOf"][0]["negate"].is_null());
        }
    }
}
