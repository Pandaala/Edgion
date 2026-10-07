//! Native WAF hot-reload integration phase suites.

#[path = "waf_json_wire.rs"]
pub(crate) mod json_wire;
#[path = "waf_original_wire.rs"]
pub(crate) mod original_wire;
#[path = "waf_wire.rs"]
mod wire;
#[path = "waf_xml_wire.rs"]
pub(crate) mod xml_wire;

#[path = "waf_supported_wire.rs"]
mod supported_wire;

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::{Duration, Instant};

const HOST: &str = "waf-dynamic.example.com";
const LISTENER_ATTEMPTS: usize = 20;
const LISTENER_RETRY_DELAY: Duration = Duration::from_millis(100);

async fn send_waf_request(
    ctx: &TestContext,
    version: &str,
    body: Option<&str>,
    response_deny: bool,
    trace_id: Option<&str>,
    sql: Option<&str>,
    xss: Option<&str>,
) -> Result<reqwest::Response, String> {
    let url = format!("http://{}:{}/test/waf-dynamic", ctx.target_host, ctx.http_port);
    let mut last_error = None;
    for attempt in 1..=LISTENER_ATTEMPTS {
        let mut request = if body.is_some() {
            ctx.http_client.post(&url).header("content-type", "text/plain")
        } else {
            ctx.http_client.get(&url)
        };
        request = request.header("host", HOST).header("x-waf-version", version);
        if response_deny {
            request = request.header("x-waf-response-deny", "trigger");
        }
        if let Some(trace_id) = trace_id {
            request = request
                .header(
                    "traceparent",
                    crate::access_log_client::traceparent_for_marker(trace_id),
                )
                .header("access_log", "test_store");
        }
        if let Some(sql) = sql {
            request = request.header("x-waf-sql", sql);
        }
        if let Some(xss) = xss {
            request = request.header("x-waf-xss", xss);
        }
        if let Some(body) = body {
            request = request.body(body.to_string());
        }
        match request.send().await {
            Ok(response) => return Ok(response),
            Err(error) => last_error = Some(error),
        }
        if attempt < LISTENER_ATTEMPTS {
            tokio::time::sleep(LISTENER_RETRY_DELAY).await;
        }
    }
    Err(format!(
        "request failed after {LISTENER_ATTEMPTS} attempts: {}",
        last_error.map_or_else(|| "unknown request error".to_string(), |error| error.to_string())
    ))
}

fn request_case(
    name: &'static str,
    description: &'static str,
    version: &'static str,
    body: Option<&'static str>,
    expected_status: u16,
) -> TestCase {
    TestCase::new(name, description, move |ctx: TestContext| {
        Box::pin(async move {
            let start = Instant::now();
            match send_waf_request(&ctx, version, body, false, None, None, None).await {
                Ok(response) if response.status().as_u16() == expected_status => TestResult::passed(start.elapsed()),
                Ok(response) => TestResult::failed(
                    start.elapsed(),
                    format!("expected status {expected_status}, got {}", response.status()),
                ),
                Err(error) => TestResult::failed(start.elapsed(), format!("request failed: {error}")),
            }
        })
    })
}

async fn check_response_stage_deny(ctx: &TestContext, trace: &str) -> Result<(), String> {
    let response = send_waf_request(ctx, "v1", None, true, Some(trace), None, None).await;
    let mut failures = Vec::new();
    match response {
        Ok(response) => {
            let status = response.status().as_u16();
            match response.text().await {
                Ok(body) => {
                    if status != 403 {
                        failures.push(format!("expected wire status 403, got {status}; body={body:?}"));
                    }
                    for upstream_marker in ["Server: 0.0.0.0:30001", "Path: /test/waf-dynamic"] {
                        if body.contains(upstream_marker) {
                            failures.push(format!(
                                "WAF response denial leaked the upstream response marker {upstream_marker:?}; body={body:?}"
                            ));
                        }
                    }
                }
                Err(error) => failures.push(format!("response body failed: {error}")),
            }
        }
        Err(error) => failures.push(format!("request failed: {error}")),
    }

    match ctx.access_log_client().get_access_log_with_retry(trace, 15, 200).await {
        Ok(access_log) => {
            let upstreams = access_log.data["backend_context"]["upstreams"].as_array();
            if access_log.status_code() != Some(403)
                || upstreams
                    .is_none_or(|upstreams| upstreams.len() != 1 || upstreams[0]["status"].as_u64() != Some(200))
                || access_log.data["waf"]["outcome"].as_str() != Some("blocked")
                || access_log.data["waf"]["intervention"]["phase"].as_str() != Some("response_headers")
                || access_log.data["waf"]["intervention"]["rule_id"].as_u64() != Some(1003)
                || access_log.data["waf"]["intervention"]["status"].as_u64() != Some(403)
            {
                failures.push(format!("unexpected access log: {}", access_log.data));
            }
        }
        Err(error) => failures.push(format!("access log missing: {error}")),
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

async fn cleanup_response_deny_access_log(ctx: &TestContext, trace: &str) -> Result<(), String> {
    let client = ctx.access_log_client();
    let mut last_error = None;
    for attempt in 0..15 {
        match client.delete_access_log(trace).await {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) => last_error = Some(error.to_string()),
        }
        if attempt < 14 {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    match last_error {
        Some(error) => Err(format!("WAF response-denial access-log cleanup failed: {error}")),
        None => Ok(()),
    }
}

fn response_stage_deny_case() -> TestCase {
    TestCase::new(
        "waf_response_stage_denial_replaces_upstream",
        "phase-3 WAF denial replaces the upstream response with 403",
        |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let trace = format!("waf-response-deny-{}", uuid::Uuid::new_v4());
                let result = tokio::time::timeout(Duration::from_secs(15), check_response_stage_deny(&ctx, &trace))
                    .await
                    .unwrap_or_else(|_| Err("WAF response-denial assertions exceeded their deadline".to_string()));
                let cleanup =
                    tokio::time::timeout(Duration::from_secs(5), cleanup_response_deny_access_log(&ctx, &trace))
                        .await
                        .map_err(|_| "WAF response-denial access-log cleanup exceeded its deadline".to_string())
                        .and_then(|result| result);
                match (result, cleanup) {
                    (Ok(()), Ok(())) => TestResult::passed(start.elapsed()),
                    (Err(error), Ok(())) | (Ok(()), Err(error)) => TestResult::failed(start.elapsed(), error),
                    (Err(error), Err(cleanup)) => TestResult::failed(start.elapsed(), format!("{error}; {cleanup}")),
                }
            })
        },
    )
}

macro_rules! phase_suite {
    ($type_name:ident, $suite_name:literal, [$(($name:literal, $description:literal, $version:literal, $body:expr, $status:expr)),+ $(,)?]) => {
        pub struct $type_name;

        impl TestSuite for $type_name {
            fn name(&self) -> &str {
                $suite_name
            }

            fn test_cases(&self) -> Vec<TestCase> {
                vec![$(request_case($name, $description, $version, $body, $status)),+]
            }
        }
    };
}

pub struct WafDynamicInitialTestSuite;

impl TestSuite for WafDynamicInitialTestSuite {
    fn name(&self) -> &str {
        "Native WAF Dynamic - Initial"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            request_case("waf_initial_v1_allowed", "initial WAF allows v1", "v1", None, 200),
            request_case("waf_initial_v2_denied", "initial WAF denies v2", "v2", None, 403),
            request_case(
                "waf_initial_body_denied",
                "initial WAF denies matching request body",
                "v1",
                Some("blocked"),
                406,
            ),
            response_stage_deny_case(),
            wire::case(false, 65_536),
        ]
    }
}

pub struct WafDynamicUpdatedTestSuite;

pub struct WafDynamicConfiguredPrefixTestSuite;

impl TestSuite for WafDynamicConfiguredPrefixTestSuite {
    fn name(&self) -> &str {
        "Native WAF Dynamic - Configured Prefix"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![wire::case(false, 1024 * 1024)]
    }
}

pub struct WafDynamicDetectionTestSuite;

pub struct WafDynamicInjectionTestSuite;

pub struct WafDynamicControlTestSuite;

impl TestSuite for WafDynamicControlTestSuite {
    fn name(&self) -> &str {
        "Native WAF Dynamic - Request Control"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            control_case(
                "waf_control_exempt",
                "tag removal affects the matching request",
                "exempt",
                None,
                Some("blocked"),
                200,
            ),
            control_case(
                "waf_control_deny",
                "tagged rule still denies the next request",
                "ordinary",
                None,
                Some("blocked"),
                403,
            ),
            control_case(
                "waf_control_processor",
                "phase-1 processor override feeds JSON arguments",
                "processor",
                Some("{\"name\":\"blocked\"}"),
                None,
                407,
            ),
            control_case(
                "waf_control_processor_raw_hidden",
                "JSON processor does not expose raw body by default",
                "processor",
                Some("{\"name\":\"clean\",\"note\":\"raw-only\"}"),
                None,
                200,
            ),
            control_case(
                "waf_control_force_on",
                "force On exposes the raw JSON body",
                "force-on",
                Some("{\"name\":\"clean\",\"note\":\"raw-only\"}"),
                None,
                408,
            ),
            control_case(
                "waf_control_raw_default",
                "raw body rule denies without force Off",
                "ordinary",
                Some("blocked"),
                None,
                406,
            ),
            control_case(
                "waf_control_force_off",
                "force Off hides raw body but still forwards it",
                "force-off",
                Some("blocked"),
                None,
                200,
            ),
            control_case(
                "waf_control_response_deny",
                "response-stage chain denies on upstream 200",
                "response",
                None,
                None,
                409,
            ),
            control_case(
                "waf_control_response_exempt",
                "response target exclusion leaves other requests unchanged",
                "response",
                None,
                Some("response-exempt"),
                200,
            ),
            control_case(
                "waf_control_successful_chain",
                "capture and setvar macros allow chain ctl to commit",
                "chain-allow",
                None,
                Some("allow"),
                200,
            ),
            control_case(
                "waf_control_failed_chain",
                "failed capture chain does not commit ctl",
                "chain-deny",
                None,
                Some("allow"),
                403,
            ),
        ]
    }
}

fn control_case(
    name: &'static str,
    description: &'static str,
    version: &'static str,
    body: Option<&'static str>,
    sql: Option<&'static str>,
    status: u16,
) -> TestCase {
    TestCase::new(name, description, move |ctx: TestContext| {
        Box::pin(async move {
            let started = Instant::now();
            match send_waf_request(&ctx, version, body, false, None, sql, None).await {
                Ok(response) if response.status().as_u16() == status => {
                    if body.is_some() && status == 200 {
                        match response.text().await {
                            Ok(text) if text.contains("Method: POST") && text.contains("Path: /test/waf-dynamic") => {
                                TestResult::passed(started.elapsed())
                            }
                            Ok(text) => TestResult::failed(
                                started.elapsed(),
                                format!("allowed body did not reach the backend: {text}"),
                            ),
                            Err(error) => {
                                TestResult::failed(started.elapsed(), format!("backend response failed: {error}"))
                            }
                        }
                    } else {
                        TestResult::passed(started.elapsed())
                    }
                }
                Ok(response) => TestResult::failed(
                    started.elapsed(),
                    format!("expected status {status}, got {}", response.status()),
                ),
                Err(error) => TestResult::failed(started.elapsed(), error),
            }
        })
    })
}

impl TestSuite for WafDynamicInjectionTestSuite {
    fn name(&self) -> &str {
        "Native WAF Dynamic - Injection"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            injection_case("waf_injection_sqli", Some("1%20OR%201%3D1"), None, 403),
            injection_case(
                "waf_injection_xss",
                None,
                Some("%3Cscript%3Ealert(1)%3C/script%3E"),
                406,
            ),
            injection_case("waf_injection_safe", Some("ordinary"), Some("hello"), 200),
        ]
    }
}

fn injection_case(name: &'static str, sql: Option<&'static str>, xss: Option<&'static str>, status: u16) -> TestCase {
    TestCase::new(
        name,
        "native libinjection verdict after hot reload",
        move |ctx: TestContext| {
            Box::pin(async move {
                let started = Instant::now();
                match send_waf_request(&ctx, "v3", None, false, None, sql, xss).await {
                    Ok(response) => {
                        let actual = response.status().as_u16();
                        let body = response.text().await.unwrap_or_default();
                        if actual != status {
                            return TestResult::failed(started.elapsed(), format!("expected {status}, got {actual}"));
                        }
                        if status != 200 && body.contains("Server: 0.0.0.0:30001") {
                            return TestResult::failed(
                                started.elapsed(),
                                "denied request reached the backend".to_string(),
                            );
                        }
                        TestResult::passed(started.elapsed())
                    }
                    Err(error) => TestResult::failed(started.elapsed(), error),
                }
            })
        },
    )
}

impl TestSuite for WafDynamicDetectionTestSuite {
    fn name(&self) -> &str {
        "Native WAF Dynamic - Detection"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![wire::detection_case()]
    }
}

impl TestSuite for WafDynamicUpdatedTestSuite {
    fn name(&self) -> &str {
        "Native WAF Dynamic - Updated"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            request_case("waf_updated_v2_allowed", "updated WAF allows v2", "v2", None, 200),
            request_case("waf_updated_v1_denied", "updated WAF denies v1", "v1", None, 451),
            wire::case(true, 65_536),
        ]
    }
}

pub struct WafDynamicJsonV1TestSuite;
pub struct WafDynamicJsonV2TestSuite;
pub struct WafDynamicJsonDetectionTestSuite;
pub struct WafDynamicJsonZeroSourceTestSuite;

macro_rules! json_phase_suite {
    ($type:ident, $title:literal, $version:literal, $detection:literal, $zero:literal) => {
        impl TestSuite for $type {
            fn name(&self) -> &str {
                $title
            }
            fn test_cases(&self) -> Vec<TestCase> {
                vec![
                    request_case(
                        "waf_json_generation_ready",
                        "ordinary Phase 1 confirms JSON generation readiness",
                        $version,
                        None,
                        200,
                    ),
                    json_wire::matrix($version, $detection, $zero),
                ]
            }
        }
    };
}

json_phase_suite!(
    WafDynamicJsonV1TestSuite,
    "Native WAF Dynamic - JSON V1",
    "json-v1",
    false,
    false
);
json_phase_suite!(
    WafDynamicJsonV2TestSuite,
    "Native WAF Dynamic - JSON V2",
    "json-v2",
    false,
    false
);
json_phase_suite!(
    WafDynamicJsonDetectionTestSuite,
    "Native WAF Dynamic - JSON Detection",
    "json-v1",
    true,
    false
);
json_phase_suite!(
    WafDynamicJsonZeroSourceTestSuite,
    "Native WAF Dynamic - JSON Zero Source",
    "json-v1",
    false,
    true
);

phase_suite!(
    WafDynamicRecoveredTestSuite,
    "Native WAF Dynamic - Recovered",
    [
        ("waf_recovered_v3_allowed", "recovered WAF allows v3", "v3", None, 200),
        ("waf_recovered_v2_denied", "recovered WAF denies v2", "v2", None, 429),
    ]
);

phase_suite!(
    WafDynamicDeletedTestSuite,
    "Native WAF Dynamic - Deleted",
    [(
        "waf_deleted_reference_fails_closed",
        "deleted WAF reference fails closed",
        "v3",
        None,
        500
    )]
);

phase_suite!(
    WafDynamicBundleV1TestSuite,
    "Native WAF Dynamic - Bundle V1",
    [
        ("waf_bundle_v1_denied", "bundle phrase asset denies v1", "v1", None, 451),
        (
            "waf_bundle_v2_allowed",
            "bundle phrase asset allows v2",
            "v2",
            None,
            200
        ),
    ]
);

phase_suite!(
    WafDynamicBundleV2TestSuite,
    "Native WAF Dynamic - Bundle V2",
    [
        ("waf_bundle_v1_allowed", "updated bundle allows v1", "v1", None, 200),
        ("waf_bundle_v2_denied", "updated bundle denies v2", "v2", None, 451),
    ]
);

phase_suite!(
    WafDynamicSelectorHighTestSuite,
    "Native WAF Dynamic - Selector High",
    [
        (
            "waf_selector_high_v1_allowed",
            "selected high profile allows v1",
            "v1",
            None,
            200
        ),
        (
            "waf_selector_high_v2_denied",
            "selected high profile denies v2",
            "v2",
            None,
            451
        ),
        (
            "waf_selector_high_body_denied",
            "second bundle in high profile denies request body",
            "v1",
            Some("blocked"),
            406
        ),
    ]
);

pub struct WafDynamicXmlV1TestSuite;
pub struct WafDynamicXmlV2TestSuite;
pub struct WafDynamicXmlDetectionTestSuite;
macro_rules! xml_phase_suite {
    ($type:ident, $title:literal, $version:literal, $detection:literal) => {
        impl TestSuite for $type {
            fn name(&self) -> &str {
                $title
            }
            fn test_cases(&self) -> Vec<TestCase> {
                vec![
                    request_case(
                        "waf_xml_generation_ready",
                        "Phase 1 confirms XML generation readiness",
                        $version,
                        None,
                        200,
                    ),
                    xml_wire::matrix($version, $detection),
                ]
            }
        }
    };
}
xml_phase_suite!(WafDynamicXmlV1TestSuite, "Waf XML V1", "xml-v1", false);
xml_phase_suite!(WafDynamicXmlV2TestSuite, "Waf XML V2", "xml-v2", false);
xml_phase_suite!(WafDynamicXmlDetectionTestSuite, "Waf XML DetectionOnly", "xml-v1", true);

pub struct WafDynamicOriginalOnTestSuite;
pub struct WafDynamicOriginalDetectionTestSuite;
macro_rules! original_phase_suite {
    ($type:ident, $title:literal, $detection:literal) => {
        impl TestSuite for $type {
            fn name(&self) -> &str {
                $title
            }
            fn test_cases(&self) -> Vec<TestCase> {
                vec![original_wire::matrix($detection)]
            }
        }
    };
}
original_phase_suite!(WafDynamicOriginalOnTestSuite, "Waf Original CRS On", false);
original_phase_suite!(
    WafDynamicOriginalDetectionTestSuite,
    "Waf Original CRS DetectionOnly",
    true
);

macro_rules! supported_phase_suite {
    ($type:ident, $title:literal, $strict:literal, $detection:literal, $measure:literal) => {
        pub struct $type;
        impl TestSuite for $type {
            fn name(&self) -> &str {
                $title
            }
            fn test_cases(&self) -> Vec<TestCase> {
                vec![supported_wire::matrix($strict, $detection, $measure)]
            }
        }
    };
}
supported_phase_suite!(
    WafDynamicSupportedOnTestSuite,
    "Waf Supported Strict On",
    true,
    false,
    true
);
supported_phase_suite!(
    WafDynamicSupportedDetectionTestSuite,
    "Waf Supported Strict DetectionOnly",
    true,
    true,
    true
);
supported_phase_suite!(
    WafDynamicSupportedStandardTestSuite,
    "Waf Supported Standard On",
    false,
    false,
    true
);
supported_phase_suite!(
    WafDynamicSupportedRetainedTestSuite,
    "Waf Supported Strict Retained/Recovered",
    true,
    false,
    false
);
