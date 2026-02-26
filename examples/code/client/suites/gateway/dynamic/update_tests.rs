// Update Phase Tests - Verify dynamic changes took effect

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct UpdatePhaseTestSuite;

impl TestSuite for UpdatePhaseTestSuite {
    fn name(&self) -> &str {
        "Gateway Dynamic Tests - Update Phase"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_hostname_removed(), Self::test_post_method_works()]
    }
}

impl UpdatePhaseTestSuite {
    /// Scenario 1 After: Hostname restriction removed, should work now
    fn test_hostname_removed() -> TestCase {
        TestCase::new(
            "scenario1_after_hostname_removed",
            "[AFTER UPDATE] Previously rejected hostname should now work (expect 200 or 502)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::builder().no_proxy().build().unwrap();

                    // ，（ 502 backend ）
                    let resp = client
                        .get(format!("http://{}:31250/match", ctx.target_host))
                        .header("Host", "other.example.com")
                        .send()
                        .await;

                    match resp {
                        Ok(resp) => {
                            let status = resp.status();
                            //  200502  503， 404
                            if status.is_success() || status == 502 || status == 503 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("✓ Hostname restriction removed (status: {})", status),
                                )
                            } else if status == 404 {
                                TestResult::failed(
                                    start.elapsed(),
                                    "Got 404, hostname restriction still active".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected success or 502/503, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Scenario 2 After: POST method should work now, GET should fail
    fn test_post_method_works() -> TestCase {
        TestCase::new(
            "scenario2_after_post_method_works",
            "[AFTER UPDATE] POST should work, GET should fail (404)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::builder().no_proxy().build().unwrap();

                    // GET  (404)（ Host header）
                    let get_resp = client
                        .get(format!("http://{}:31251/api/v1", ctx.target_host))
                        .header("Host", "method-test.example.com")
                        .send()
                        .await;

                    let get_status = match get_resp {
                        Ok(resp) => resp.status(),
                        Err(e) => return TestResult::failed(start.elapsed(), format!("GET request failed: {}", e)),
                    };

                    if get_status != 404 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("GET should return 404 after update, got {}", get_status),
                        );
                    }

                    // POST  (200  502)（ Host header）
                    let post_resp = client
                        .post(format!("http://{}:31251/api/v1", ctx.target_host))
                        .header("Host", "method-test.example.com")
                        .send()
                        .await;

                    match post_resp {
                        Ok(resp) => {
                            let status = resp.status();
                            //  200502  503 (backend )
                            if status.is_success() || status == 502 || status == 503 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("✓ Method updated: GET→404, POST→{}", status),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("POST should succeed or 502, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("POST request failed: {}", e)),
                    }
                })
            },
        )
    }
}
