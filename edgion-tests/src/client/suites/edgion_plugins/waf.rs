//! Native WAF hot-reload integration phase suites.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::{Duration, Instant};

const HOST: &str = "waf-dynamic.example.com";
const LISTENER_ATTEMPTS: usize = 20;
const LISTENER_RETRY_DELAY: Duration = Duration::from_millis(100);

async fn send_waf_request(
    ctx: &TestContext,
    version: &str,
    body: Option<&str>,
    invalid_response_status: bool,
    trace_id: Option<&str>,
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
        if invalid_response_status {
            request = request.header("x-waf-invalid-status", "trigger");
        }
        if let Some(trace_id) = trace_id {
            request = request
                .header("x-trace-id", trace_id)
                .header("access_log", "test_store");
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
            match send_waf_request(&ctx, version, body, false, None).await {
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

async fn check_invalid_response_status(ctx: &TestContext, trace: &str) -> Result<(), String> {
    let response = send_waf_request(ctx, "v1", None, true, Some(trace)).await;
    let mut failures = Vec::new();
    match response {
        Ok(response) => {
            let status = response.status().as_u16();
            match response.text().await {
                Ok(body) => {
                    if status != 500 {
                        failures.push(format!("expected wire status 500, got {status}; body={body:?}"));
                    }
                    for upstream_marker in ["Server: 0.0.0.0:30001", "Path: /test/waf-dynamic"] {
                        if body.contains(upstream_marker) {
                            failures.push(format!(
                                "invalid WAF status leaked the upstream response marker {upstream_marker:?}; body={body:?}"
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
            if access_log.status_code() != Some(500)
                || upstreams
                    .is_none_or(|upstreams| upstreams.len() != 1 || upstreams[0]["status"].as_u64() != Some(200))
                || access_log.data["waf"]["outcome"].as_str() != Some("blocked")
                || access_log.data["waf"]["intervention"]["phase"].as_str() != Some("response_headers")
                || access_log.data["waf"]["intervention"]["rule_id"].as_u64() != Some(1003)
                || access_log.data["waf"]["intervention"]["status"].as_u64() != Some(100)
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

async fn cleanup_invalid_status_access_log(ctx: &TestContext, trace: &str) -> Result<(), String> {
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
        Some(error) => Err(format!("WAF invalid-status access-log cleanup failed: {error}")),
        None => Ok(()),
    }
}

fn invalid_response_status_case() -> TestCase {
    TestCase::new(
        "waf_response_invalid_status_fails_closed",
        "phase-3 WAF status 100 fails closed without forwarding the upstream response",
        |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let trace = format!("waf-invalid-response-status-{}", uuid::Uuid::new_v4());
                let result = tokio::time::timeout(Duration::from_secs(15), check_invalid_response_status(&ctx, &trace))
                    .await
                    .unwrap_or_else(|_| Err("WAF invalid-status assertions exceeded their deadline".to_string()));
                let cleanup =
                    tokio::time::timeout(Duration::from_secs(5), cleanup_invalid_status_access_log(&ctx, &trace))
                        .await
                        .map_err(|_| "WAF invalid-status access-log cleanup exceeded its deadline".to_string())
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
            invalid_response_status_case(),
        ]
    }
}

phase_suite!(
    WafDynamicUpdatedTestSuite,
    "Native WAF Dynamic - Updated",
    [
        ("waf_updated_v2_allowed", "updated WAF allows v2", "v2", None, 200),
        ("waf_updated_v1_denied", "updated WAF denies v1", "v1", None, 451),
    ]
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
