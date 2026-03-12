use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct PortOnlyEdgionTlsTestSuite;

impl PortOnlyEdgionTlsTestSuite {
    fn test_port_only_parent_ref_health() -> TestCase {
        TestCase::new(
            "port_only_parent_ref_health",
            "EdgionTls should resolve parentRef.port without sectionName",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let host = ctx.http_host.as_deref().unwrap_or(&ctx.target_host);
                    let url = format!("https://{}:{}/secure/health", host, ctx.https_port);

                    match ctx.http_client.get(&url).send().await {
                        Ok(response) => {
                            let status = response.status();
                            match response.text().await {
                                Ok(body) => {
                                    if status.is_success()
                                        && (body.contains("healthy") || body.contains("Path: /secure/health"))
                                    {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            "EdgionTls parentRef.port binding works".to_string(),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Unexpected response. Status: {}, Body: {}", status, body),
                                        )
                                    }
                                }
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("Failed to read response body: {}", e))
                                }
                            }
                        }
                        Err(e) => {
                            TestResult::failed(start.elapsed(), format!("HTTPS request failed: {}", e))
                        }
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for PortOnlyEdgionTlsTestSuite {
    fn name(&self) -> &str {
        "EdgionTls PortOnly"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_port_only_parent_ref_health()]
    }
}
