//! DSL `next()` -> `terminate()` -> `next()` hot-reload contract.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const HOST: &str = "dsl-terminate-dynamic.example.com";

fn generation_case(name: &'static str, expected_status: u16, expected_generation: &'static str) -> TestCase {
    TestCase::new(
        name,
        "DSL hot reload publishes the expected terminate generation",
        move |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let response = ctx
                    .http_client
                    .get(format!(
                        "http://{}:{}/test/dsl-terminate/api",
                        ctx.target_host, ctx.http_port
                    ))
                    .header("host", HOST)
                    .send()
                    .await;
                let response = match response {
                    Ok(response) => response,
                    Err(error) => return TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                };
                let status = response.status().as_u16();
                let generation = response
                    .headers()
                    .get("x-dsl-generation")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                let body = match response.bytes().await {
                    Ok(body) => body,
                    Err(error) => {
                        return TestResult::failed(start.elapsed(), format!("Read response body: {error}"));
                    }
                };
                if status != expected_status || generation != expected_generation {
                    return TestResult::failed(
                        start.elapsed(),
                        format!(
                            "expected status={expected_status} generation={expected_generation:?}, got status={status} generation={generation:?} body={body:?}"
                        ),
                    );
                }
                if expected_status == 502 && !body.is_empty() {
                    return TestResult::failed(
                        start.elapsed(),
                        format!("terminate generation returned {} body byte(s): {body:?}", body.len()),
                    );
                }
                TestResult::passed(start.elapsed())
            })
        },
    )
}

pub struct DslTerminateDynamicInitialTestSuite;

impl TestSuite for DslTerminateDynamicInitialTestSuite {
    fn name(&self) -> &str {
        "DSL Terminate Dynamic - Initial"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![generation_case("dsl_terminate_dynamic_initial", 200, "pass-v1")]
    }
}

pub struct DslTerminateDynamicUpdatedTestSuite;

impl TestSuite for DslTerminateDynamicUpdatedTestSuite {
    fn name(&self) -> &str {
        "DSL Terminate Dynamic - Updated"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![generation_case("dsl_terminate_dynamic_updated", 502, "terminate")]
    }
}

pub struct DslTerminateDynamicRestoredTestSuite;

impl TestSuite for DslTerminateDynamicRestoredTestSuite {
    fn name(&self) -> &str {
        "DSL Terminate Dynamic - Restored"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![generation_case("dsl_terminate_dynamic_restored", 200, "pass-v2")]
    }
}
