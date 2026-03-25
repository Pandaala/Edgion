// FilterConflict Test suite
//
// Tests that URLRewrite + RequestRedirect filter conflict is detected by Controller:
// 1. rule[0]: URLRewrite only → should proxy successfully (200)
// 2. rule[1]: URLRewrite + RequestRedirect conflict → rule skipped by Gateway (404)
// 3. rule[2]: RequestRedirect only → should redirect (302)

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

    fn test_conflict_rule_skipped() -> TestCase {
        TestCase::new(
            "conflict_rule_skipped",
            "Rule[1] with URLRewrite+RequestRedirect conflict should be skipped (404)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let request = ctx
                        .http_client
                        .get(format!("{}/conflict", ctx.http_url()))
                        .header("Host", "filter-conflict.example.com");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 404 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Conflict rule skipped: 404 as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected 404 for conflicting rule, got {} (rule was not skipped)",
                                        status
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
            Self::test_conflict_rule_skipped(),
            Self::test_redirect_only_works(),
        ]
    }
}
