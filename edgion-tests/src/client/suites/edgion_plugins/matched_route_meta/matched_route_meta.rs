// MatchedRouteMeta Integration Test Suite
//
// Proves the end-to-end path: an HTTPRoute with k8s labels and annotations +
// a CtxSet plugin using ${matchedHttpRoute:<selector>} templates renders the
// matched-route metadata into the request context at runtime.
//
// Assertion mechanism: the ctx variable `route_meta` is written during
// the request stage and surfaced through the access-log store (same approach
// as CtxSetTestSuite). The expected rendered value is:
//   meta-route|edgion-it|web|team-a|platform
//
// Required config:
//   conf/EdgionPlugins/MatchedRouteMeta/01_EdgionPlugins_matched-route-meta.yaml
//   conf/EdgionPlugins/MatchedRouteMeta/02_HTTPRoute_matched-route-meta.yaml
//   conf/EdgionPlugins/MatchedRouteMeta/ReferenceGrant_edgion-test_allow-edgion-it.yaml

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct MatchedRouteMetaTestSuite;

impl MatchedRouteMetaTestSuite {
    // ==================== 1. Renders all matched-route selectors ====================
    fn test_matched_route_meta_rendered() -> TestCase {
        TestCase::new(
            "matched_route_meta_rendered",
            "MatchedRouteMeta: ctx_set template renders name|namespace|label|annotation from matched HTTPRoute",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/matched-route-meta/ping", ctx.target_host);

                    let trace_id = format!("test-mrm-{}", uuid::Uuid::new_v4());

                    let response = client
                        .get(&url)
                        .header("host", "matched-route-meta.example.com")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            let al_client = ctx.access_log_client();
                            let entry = match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(e) => e,
                                Err(e) => {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("Failed to fetch access log: {}", e),
                                    )
                                }
                            };

                            let access_log = entry.data.to_string();

                            // The CtxSet plugin evaluates the composed template at request time.
                            // Expected ctx value (pipe-delimited):
                            //   meta-route  = ${matchedHttpRoute:name}
                            //   edgion-it   = ${matchedHttpRoute:namespace}
                            //   web         = ${matchedHttpRoute:label.app}
                            //   team-a      = ${matchedHttpRoute:label.edgion.io/team}
                            //   platform    = ${matchedHttpRoute:annotation.owner}
                            let expected = r#""route_meta":"meta-route|edgion-it|web|team-a|platform""#;

                            if access_log.contains(expected) {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ctx.route_meta correctly rendered from matched HTTPRoute metadata".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected route_meta=meta-route|edgion-it|web|team-a|platform not found. Access-Log: {}",
                                        access_log
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
}

impl TestSuite for MatchedRouteMetaTestSuite {
    fn name(&self) -> &str {
        "MatchedRouteMeta Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_matched_route_meta_rendered()]
    }
}
