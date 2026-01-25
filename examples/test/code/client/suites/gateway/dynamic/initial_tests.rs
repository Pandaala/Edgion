// Initial Phase Tests - Verify constraints are enforced before dynamic updates

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct InitialPhaseTestSuite;

impl TestSuite for InitialPhaseTestSuite {
    fn name(&self) -> &str {
        "Gateway Dynamic Tests - Initial Phase"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_hostname_restriction(), Self::test_get_method_only()]
    }
}

impl InitialPhaseTestSuite {
    /// Scenario 1 Initial: Hostname mismatch should be rejected (404)
    fn test_hostname_restriction() -> TestCase {
        TestCase::new(
            "scenario1_initial_hostname_restriction",
            "[INITIAL] Hostname mismatch should be rejected (expect 404)",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::builder().no_proxy().build().unwrap();

                    // 尝试访问 hostname 不匹配的路由
                    let resp = client
                        .get("http://127.0.0.1:31250/match")
                        .header("Host", "other.example.com") // 不匹配 api.example.com
                        .send()
                        .await;

                    match resp {
                        Ok(resp) => {
                            if resp.status() == 404 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "✓ Hostname restriction works (404 as expected)".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 404 for hostname mismatch, got {}", resp.status()),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Scenario 2 Initial: GET method should work
    fn test_get_method_only() -> TestCase {
        TestCase::new(
            "scenario2_initial_get_method_only",
            "[INITIAL] GET /api/v1 should work (expect 200 or backend response)",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::builder().no_proxy().build().unwrap();

                    // GET 请求应该成功（需要带正确的 Host header）
                    let resp = client
                        .get("http://127.0.0.1:31251/api/v1")
                        .header("Host", "method-test.example.com")
                        .send()
                        .await;

                    match resp {
                        Ok(resp) => {
                            let status = resp.status();
                            // 接受 200、502 或 503 (backend 未运行)
                            if status.is_success() || status == 502 || status == 503 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("✓ GET method works (status: {})", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected success or 502, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
}
