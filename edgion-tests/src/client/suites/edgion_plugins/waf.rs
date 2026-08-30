//! Native WAF hot-reload integration phase suites.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const HOST: &str = "waf-dynamic.example.com";

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
            let url = format!("http://{}:{}/test/waf-dynamic", ctx.target_host, ctx.http_port);
            let mut request = if body.is_some() {
                ctx.http_client.post(url).header("content-type", "text/plain")
            } else {
                ctx.http_client.get(url)
            };
            request = request.header("host", HOST).header("x-waf-version", version);
            if let Some(body) = body {
                request = request.body(body);
            }
            match request.send().await {
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

phase_suite!(
    WafDynamicInitialTestSuite,
    "Native WAF Dynamic - Initial",
    [
        ("waf_initial_v1_allowed", "initial WAF allows v1", "v1", None, 200),
        ("waf_initial_v2_denied", "initial WAF denies v2", "v2", None, 403),
        (
            "waf_initial_body_denied",
            "initial WAF denies matching request body",
            "v1",
            Some("blocked"),
            406
        ),
    ]
);

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
