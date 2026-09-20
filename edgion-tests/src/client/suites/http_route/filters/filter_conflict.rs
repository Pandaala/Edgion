// FilterConflict Test suite
//
// Tests that URLRewrite + RequestRedirect conflicts are rejected by the Controller
// while valid URLRewrite-only and RequestRedirect-only routes remain usable.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct FilterConflictTestSuite;

impl FilterConflictTestSuite {
    fn test_rewrite_only_proxies() -> TestCase {
        TestCase::new(
            "rewrite_only_proxies",
            "Rule[0] with URLRewrite only should proxy successfully",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let request = ctx
                        .http_client
                        .get(format!("{}/rewrite-only", ctx.http_url()))
                        .header("Host", "filter-conflict.example.com");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "URLRewrite-only rule proxied successfully: 200 OK".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 for URLRewrite-only rule, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_conflict_resource_rejected() -> TestCase {
        TestCase::new(
            "conflict_resource_rejected",
            "A rule combining URLRewrite and RequestRedirect should be rejected at admission",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let resource = serde_json::json!({
                        "apiVersion": "gateway.networking.k8s.io/v1",
                        "kind": "HTTPRoute",
                        "metadata": {
                            "name": "filter-conflict-invalid",
                            "namespace": "edgion-default"
                        },
                        "spec": {
                            "parentRefs": [{
                                "name": "filter-conflict-gateway",
                                "namespace": "edgion-test"
                            }],
                            "hostnames": ["filter-conflict-invalid.example.com"],
                            "rules": [{
                                "matches": [{
                                    "path": {"type": "Exact", "value": "/conflict"}
                                }],
                                "filters": [
                                    {
                                        "type": "URLRewrite",
                                        "urlRewrite": {"hostname": "rewritten.example.com"}
                                    },
                                    {
                                        "type": "RequestRedirect",
                                        "requestRedirect": {"scheme": "https", "statusCode": 301}
                                    }
                                ]
                            }]
                        }
                    });
                    let mut request = ctx
                        .http_client
                        .post(format!(
                            "{}/api/v1/namespaced/HTTPRoute/edgion-default",
                            ctx.admin_api_url()
                        ))
                        .json(&resource);
                    if let Some(token) = &ctx.admin_token {
                        request = request.bearer_auth(token);
                    }

                    match request.send().await {
                        Ok(response) if response.status() == reqwest::StatusCode::BAD_REQUEST => {
                            TestResult::passed_with_message(
                                start.elapsed(),
                                "Conflicting request-transform resource rejected with 400".to_string(),
                            )
                        }
                        Ok(response) => TestResult::failed(
                            start.elapsed(),
                            format!("Expected 400 for conflicting filters, got {}", response.status()),
                        ),
                        Err(error) => TestResult::failed(start.elapsed(), format!("Admission request failed: {error}")),
                    }
                })
            },
        )
    }

    fn test_redirect_only_works() -> TestCase {
        TestCase::new(
            "redirect_only_works",
            "Rule[2] with RequestRedirect only should redirect (302)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .unwrap();

                    let request = client
                        .get(format!("{}/redirect-only", ctx.http_url()))
                        .header("Host", "filter-conflict.example.com");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 302 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "RequestRedirect-only rule redirected: 302 as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 302 for RequestRedirect-only rule, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_redirect_only_preserves_query() -> TestCase {
        TestCase::new(
            "redirect_only_preserves_query",
            "RequestRedirect must preserve the request query string in the Location header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .unwrap();

                    let request = client
                        .get(format!("{}/redirect-only?returnTo=/dashboard&page=1", ctx.http_url()))
                        .header("Host", "filter-conflict.example.com");

                    match request.send().await {
                        Ok(response) => {
                            if response.status().as_u16() != 302 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 302, got {}", response.status().as_u16()),
                                );
                            }
                            match response.headers().get("location").and_then(|l| l.to_str().ok()) {
                                Some(location)
                                    if location.contains("returnTo=/dashboard") && location.contains("page=1") =>
                                {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!("Query string preserved in Location: {}", location),
                                    )
                                }
                                Some(location) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Query string dropped from Location: {}", location),
                                ),
                                None => TestResult::failed(start.elapsed(), "Missing Location header".to_string()),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for FilterConflictTestSuite {
    fn name(&self) -> &str {
        "FilterConflict"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Filters/FilterConflict")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_rewrite_only_proxies(),
            Self::test_conflict_resource_rejected(),
            Self::test_redirect_only_works(),
            Self::test_redirect_only_preserves_query(),
        ]
    }
}
