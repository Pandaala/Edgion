//! Cross-process DSL explicit-termination contract tests.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct DslTerminateTestSuite;

async fn assert_no_terminate_fallback(
    ctx: &TestContext,
    marker: &str,
    expected_upstreams: usize,
) -> Result<(), String> {
    let entry = ctx
        .access_log_client()
        .get_access_log_with_retry(marker, 10, 200)
        .await
        .map_err(|error| format!("terminate access log was not available: {error}"))?;
    if entry.has_diagnostic("error", "PluginTerminateWithoutResponse") {
        return Err(format!(
            "explicit DSL termination incorrectly used the naked-terminate fallback: {}",
            entry.data
        ));
    }
    if entry.upstream_count() != expected_upstreams {
        return Err(format!(
            "expected {expected_upstreams} upstream attempt(s), observed {}: {}",
            entry.upstream_count(),
            entry.data
        ));
    }
    Ok(())
}

impl DslTerminateTestSuite {
    fn request_source_contract() -> TestCase {
        Self::request_case("dsl_terminate_request_source_contract", "source", false)
    }

    fn request_bytecode_head_contract() -> TestCase {
        Self::request_case("dsl_terminate_request_bytecode_head_contract", "bytecode", true)
    }

    fn request_case(name: &'static str, form: &'static str, head: bool) -> TestCase {
        TestCase::new(
            name,
            "DSL terminate owns an empty HTTP 502 and stops the request chain",
            move |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let marker = name;
                    let url = format!(
                        "http://{}:{}/test/dsl-terminate-contract/request/{form}",
                        ctx.target_host, ctx.http_port
                    );
                    let request = if head {
                        ctx.http_client.head(url)
                    } else {
                        ctx.http_client.get(url)
                    };
                    let response = request
                        .header("host", "dsl-terminate-http-request.example.com")
                        .header("x-dsl-form", form)
                        .header("traceparent", crate::access_log_client::traceparent_for_marker(marker))
                        .header("access_log", "test_store")
                        .send()
                        .await;
                    let response = match response {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    };
                    let status = response.status().as_u16();
                    let queued = response
                        .headers()
                        .get("x-dsl-queued")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let body = match response.bytes().await {
                        Ok(body) => body,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("Read response body: {error}"));
                        }
                    };
                    if status != 502 || !body.is_empty() || queued != form {
                        return TestResult::failed(
                        start.elapsed(),
                        format!("expected empty 502 with queued {form} header, got status={status} queued={queued:?} body={body:?}"),
                    );
                    }
                    match assert_no_terminate_fallback(&ctx, marker, 0).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    fn response_source_contract() -> TestCase {
        Self::response_case(
            "dsl_terminate_response_source_contract",
            "source",
            "/request-header-test",
        )
    }

    fn response_bytecode_bodyless_contract() -> TestCase {
        Self::response_case(
            "dsl_terminate_response_bytecode_bodyless_contract",
            "bytecode",
            "/status-empty/204",
        )
    }

    fn response_case(name: &'static str, form: &'static str, path: &'static str) -> TestCase {
        TestCase::new(
            name,
            "DSL response-stage terminate replaces the upstream response with an empty HTTP/1 502",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let response = ctx
                        .http_client
                        .get(format!("http://{}:{}{path}", ctx.target_host, ctx.http_port))
                        .header("host", "dsl-terminate-http-response.example.com")
                        .header("x-dsl-form", form)
                        .header("traceparent", crate::access_log_client::traceparent_for_marker(name))
                        .header("access_log", "test_store")
                        .send()
                        .await;
                    let response = match response {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    };
                    let status = response.status().as_u16();
                    let queued = response
                        .headers()
                        .get("x-dsl-queued")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let connection = response
                        .headers()
                        .get("connection")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    let body = match response.bytes().await {
                        Ok(body) => body,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("Read response body: {error}"));
                        }
                    };
                    if status != 502 || !body.is_empty() || queued != "response-stage" || connection != "close" {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "expected empty HTTP/1 502 with queued header and connection close, got status={status} queued={queued:?} connection={connection:?} body={body:?}"
                            ),
                        );
                    }
                    match assert_no_terminate_fallback(&ctx, name, 1).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }
}

impl TestSuite for DslTerminateTestSuite {
    fn name(&self) -> &str {
        "DSL Explicit Termination"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::request_source_contract(),
            Self::request_bytecode_head_contract(),
            Self::response_source_contract(),
            Self::response_bytecode_bodyless_contract(),
        ]
    }
}
