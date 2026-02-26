// ============================================================================
// LB ConsistentHash Test Suite - using Prometheus metrics to verify consistent hashing
// ============================================================================
//
// This test suite verifies ConsistentHash Load Balancer policy through metrics analysis.
// Gateway must have test mode enabled (--test-mode) for metrics test features.
//
// Test scenarios:
// 1. Header hash + EndpointSlice - same header value always routes to same backend
// 2. Header hash + Endpoints - same as above with Endpoints resource
// 3. Cookie hash - consistent routing based on cookie value
// 4. Query arg hash - consistent routing based on query argument
// 5. Multi-slice - consistent hashing across multiple EndpointSlices
//
// Gateway annotations for test mode:
//   edgion.io/metrics-test-key: "lb-ch-test"
//   edgion.io/metrics-test-type: "lb"

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::metrics_helper::MetricsClient;
use async_trait::async_trait;
use std::collections::HashMap;
use std::time::Instant;

pub struct LBConsistentHashTestSuite;

impl LBConsistentHashTestSuite {
    /// Test ConsistentHash with header, using EndpointSlice
    fn test_chash_header_eps() -> TestCase {
        TestCase::new(
            "chash_header_eps",
            "Verify ConsistentHash (header) with EndpointSlice - same key routes to same backend",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(5))
                        .no_proxy()
                        .build()
                        .expect("Failed to create HTTP client");

                    // Use 3 different user IDs, send multiple requests for each
                    let user_ids = vec!["user-001", "user-002", "user-003"];
                    let requests_per_user = 4;

                    let mut tasks = Vec::new();
                    for user_id in &user_ids {
                        for i in 0..requests_per_user {
                            let client = client.clone();
                            let url = "http://127.0.0.1:31121/test".to_string();
                            let trace_id = format!("ch-header-{}-{}", user_id, i);
                            let user_id = user_id.to_string();

                            let task = tokio::spawn(async move {
                                client
                                    .get(&url)
                                    .header("host", "lb-ch-header-eps.example.com")
                                    .header("x-user-id", &user_id)
                                    .header("x-trace-id", &trace_id)
                                    .send()
                                    .await
                            });
                            tasks.push(task);
                        }
                    }

                    for task in tasks {
                        let _ = task.await;
                    }

                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                    let metrics_client = MetricsClient::from_host_port("127.0.0.1", 5901);
                    match metrics_client.analyze_chash_consistency("lb-ch-test").await {
                        Ok(analysis) => {
                            if analysis.total_requests == 0 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "No requests recorded in metrics".to_string(),
                                );
                            }

                            // Check consistency: each hash_key should always route to the same backend
                            let is_consistent = analysis.is_consistent;
                            let consistency_rate = analysis.consistency_rate;

                            let msg = format!(
                                "Header ConsistentHash (EPS): {} requests, {} unique keys, consistency={:.1}%, consistent={}",
                                analysis.total_requests,
                                analysis.unique_keys,
                                consistency_rate * 100.0,
                                is_consistent
                            );

                            if is_consistent {
                                TestResult::passed_with_message(start.elapsed(), msg)
                            } else {
                                TestResult::failed(start.elapsed(), format!("Inconsistent routing detected: {}", msg))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Metrics analysis failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test ConsistentHash with header, using Endpoints
    fn test_chash_header_ep() -> TestCase {
        TestCase::new(
            "chash_header_ep",
            "Verify ConsistentHash (header) with Endpoints (ServiceEndpoint kind)",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(5))
                        .no_proxy()
                        .build()
                        .expect("Failed to create HTTP client");

                    let user_ids = vec!["user-ep-001", "user-ep-002", "user-ep-003"];
                    let requests_per_user = 4;

                    let mut tasks = Vec::new();
                    for user_id in &user_ids {
                        for i in 0..requests_per_user {
                            let client = client.clone();
                            let url = "http://127.0.0.1:31121/test".to_string();
                            let trace_id = format!("ch-ep-{}-{}", user_id, i);
                            let user_id = user_id.to_string();

                            let task = tokio::spawn(async move {
                                client
                                    .get(&url)
                                    .header("host", "lb-ch-header-ep.example.com")
                                    .header("x-user-id", &user_id)
                                    .header("x-trace-id", &trace_id)
                                    .send()
                                    .await
                            });
                            tasks.push(task);
                        }
                    }

                    for task in tasks {
                        let _ = task.await;
                    }

                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                    let metrics_client = MetricsClient::from_host_port("127.0.0.1", 5901);
                    match metrics_client.analyze_chash_consistency("lb-ch-test").await {
                        Ok(analysis) => {
                            if analysis.total_requests == 0 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "No requests recorded in metrics".to_string(),
                                );
                            }

                            let is_consistent = analysis.is_consistent;
                            let consistency_rate = analysis.consistency_rate;

                            let msg = format!(
                                "Header ConsistentHash (EP): {} requests, {} unique keys, consistency={:.1}%, consistent={}",
                                analysis.total_requests,
                                analysis.unique_keys,
                                consistency_rate * 100.0,
                                is_consistent
                            );

                            if is_consistent {
                                TestResult::passed_with_message(start.elapsed(), msg)
                            } else {
                                TestResult::failed(start.elapsed(), format!("Inconsistent routing detected: {}", msg))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Metrics analysis failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test ConsistentHash with cookie
    fn test_chash_cookie() -> TestCase {
        TestCase::new(
            "chash_cookie",
            "Verify ConsistentHash (cookie) - same session routes to same backend",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(5))
                        .no_proxy()
                        .build()
                        .expect("Failed to create HTTP client");

                    let session_ids = vec!["sess-abc123", "sess-def456", "sess-ghi789"];
                    let requests_per_session = 4;

                    let mut tasks = Vec::new();
                    for session_id in &session_ids {
                        for i in 0..requests_per_session {
                            let client = client.clone();
                            let url = "http://127.0.0.1:31121/test".to_string();
                            let trace_id = format!("ch-cookie-{}-{}", session_id, i);
                            let cookie = format!("session-id={}", session_id);

                            let task = tokio::spawn(async move {
                                client
                                    .get(&url)
                                    .header("host", "lb-ch-cookie.example.com")
                                    .header("cookie", &cookie)
                                    .header("x-trace-id", &trace_id)
                                    .send()
                                    .await
                            });
                            tasks.push(task);
                        }
                    }

                    for task in tasks {
                        let _ = task.await;
                    }

                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                    let metrics_client = MetricsClient::from_host_port("127.0.0.1", 5901);
                    match metrics_client.analyze_chash_consistency("lb-ch-test").await {
                        Ok(analysis) => {
                            if analysis.total_requests == 0 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "No requests recorded in metrics".to_string(),
                                );
                            }

                            let is_consistent = analysis.is_consistent;
                            let consistency_rate = analysis.consistency_rate;

                            let msg = format!(
                                "Cookie ConsistentHash: {} requests, {} unique keys, consistency={:.1}%, consistent={}",
                                analysis.total_requests,
                                analysis.unique_keys,
                                consistency_rate * 100.0,
                                is_consistent
                            );

                            if is_consistent {
                                TestResult::passed_with_message(start.elapsed(), msg)
                            } else {
                                TestResult::failed(start.elapsed(), format!("Inconsistent routing detected: {}", msg))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Metrics analysis failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test ConsistentHash with query argument
    fn test_chash_arg() -> TestCase {
        TestCase::new(
            "chash_arg",
            "Verify ConsistentHash (query arg) - same user_id routes to same backend",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(5))
                        .no_proxy()
                        .build()
                        .expect("Failed to create HTTP client");

                    let user_ids = vec!["1001", "1002", "1003"];
                    let requests_per_user = 4;

                    let mut tasks = Vec::new();
                    for user_id in &user_ids {
                        for i in 0..requests_per_user {
                            let client = client.clone();
                            let url = format!("http://127.0.0.1:31121/test?user_id={}", user_id);
                            let trace_id = format!("ch-arg-{}-{}", user_id, i);

                            let task = tokio::spawn(async move {
                                client
                                    .get(&url)
                                    .header("host", "lb-ch-arg.example.com")
                                    .header("x-trace-id", &trace_id)
                                    .send()
                                    .await
                            });
                            tasks.push(task);
                        }
                    }

                    for task in tasks {
                        let _ = task.await;
                    }

                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                    let metrics_client = MetricsClient::from_host_port("127.0.0.1", 5901);
                    match metrics_client.analyze_chash_consistency("lb-ch-test").await {
                        Ok(analysis) => {
                            if analysis.total_requests == 0 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "No requests recorded in metrics".to_string(),
                                );
                            }

                            let is_consistent = analysis.is_consistent;
                            let consistency_rate = analysis.consistency_rate;

                            let msg = format!(
                                "Arg ConsistentHash: {} requests, {} unique keys, consistency={:.1}%, consistent={}",
                                analysis.total_requests,
                                analysis.unique_keys,
                                consistency_rate * 100.0,
                                is_consistent
                            );

                            if is_consistent {
                                TestResult::passed_with_message(start.elapsed(), msg)
                            } else {
                                TestResult::failed(start.elapsed(), format!("Inconsistent routing detected: {}", msg))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Metrics analysis failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test ConsistentHash with multiple EndpointSlices
    fn test_chash_multi_slice() -> TestCase {
        TestCase::new(
            "chash_multi_slice",
            "Verify ConsistentHash across multiple EndpointSlices (2 slices, 4 backends)",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(5))
                        .no_proxy()
                        .build()
                        .expect("Failed to create HTTP client");

                    // Use more user IDs to ensure distribution across all backends
                    let user_ids = vec!["multi-001", "multi-002", "multi-003", "multi-004", "multi-005"];
                    let requests_per_user = 3;

                    let mut tasks = Vec::new();
                    for user_id in &user_ids {
                        for i in 0..requests_per_user {
                            let client = client.clone();
                            let url = "http://127.0.0.1:31121/test".to_string();
                            let trace_id = format!("ch-multi-{}-{}", user_id, i);
                            let user_id = user_id.to_string();

                            let task = tokio::spawn(async move {
                                client
                                    .get(&url)
                                    .header("host", "lb-ch-multi.example.com")
                                    .header("x-user-id", &user_id)
                                    .header("x-trace-id", &trace_id)
                                    .send()
                                    .await
                            });
                            tasks.push(task);
                        }
                    }

                    for task in tasks {
                        let _ = task.await;
                    }

                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                    let metrics_client = MetricsClient::from_host_port("127.0.0.1", 5901);
                    match metrics_client.analyze_chash_consistency("lb-ch-test").await {
                        Ok(analysis) => {
                            if analysis.total_requests == 0 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "No requests recorded in metrics".to_string(),
                                );
                            }

                            let is_consistent = analysis.is_consistent;
                            let consistency_rate = analysis.consistency_rate;

                            let msg = format!(
                                "Multi-slice ConsistentHash: {} requests, {} unique keys, consistency={:.1}%, consistent={}",
                                analysis.total_requests,
                                analysis.unique_keys,
                                consistency_rate * 100.0,
                                is_consistent
                            );

                            if is_consistent {
                                TestResult::passed_with_message(start.elapsed(), msg)
                            } else {
                                TestResult::failed(start.elapsed(), format!("Inconsistent routing detected: {}", msg))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Metrics analysis failed: {}", e)),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for LBConsistentHashTestSuite {
    fn name(&self) -> &str {
        "LB ConsistentHash Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_chash_header_eps(),
            Self::test_chash_header_ep(),
            Self::test_chash_cookie(),
            Self::test_chash_arg(),
            Self::test_chash_multi_slice(),
        ]
    }
}
