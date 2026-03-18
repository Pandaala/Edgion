// Weighted Backend Test suite
//
// Required config files (in examples/conf/):
// - HTTPRoute_default_weighted-backend.yaml    # Weighted backend test route（50:30:20）
// - Service_edge_backend-a.yaml                # Backend A service
// - Service_edge_backend-b.yaml                # Backend B service
// - Service_edge_backend-c.yaml                # Backend C service
// - EndpointSlice_edge_backend-a.yaml          # Backend A endpoints
// - EndpointSlice_edge_backend-b.yaml          # Backend B endpoints
// - EndpointSlice_edge_backend-c.yaml          # Backend C endpoints
// - EdgionPlugins_default_timeout-debug.yaml   # Debugconfig
// - Gateway_edge_example-gateway.yaml          # Gateway config
// - GatewayClass__public-gateway.yaml          # GatewayClass config

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::collections::HashMap;
use std::time::Instant;

pub struct WeightedBackendTestSuite;

impl WeightedBackendTestSuite {
    /// Test weighted backend traffic distribution（50:30:20）
    fn test_weighted_distribution() -> TestCase {
        TestCase::new(
            "weighted_distribution",
            "Test weighted backend traffic distribution（50:30:20）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let total_requests = 100;
                    let mut backend_counts = HashMap::new();

                    for _ in 0..total_requests {
                        let trace_id = format!("weighted-dist-{}", uuid::Uuid::new_v4());
                        let request = ctx
                            .http_client
                            .get(format!("{}/echo", ctx.http_url()))
                            .header("Host", "weighted-backend.example.com")
                            .header("x-trace-id", &trace_id)
                            .header("access_log", "test_store");

                        match request.send().await {
                            Ok(_) => {
                                // Fetch access log
                                let al_client = ctx.access_log_client();
                                if let Ok(entry) = al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                    if let Some(backend_name) = entry
                                        .data
                                        .get("backend_context")
                                        .and_then(|bc| bc.get("name"))
                                        .and_then(|n| n.as_str())
                                    {
                                        *backend_counts.entry(backend_name.to_string()).or_insert(0) += 1;
                                    }
                                }
                            }
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("Request failed: {}", e));
                            }
                        }
                    }

                    // Calculate actual distribution
                    let backend_a_count = backend_counts.get("backend-a").unwrap_or(&0);
                    let backend_b_count = backend_counts.get("backend-b").unwrap_or(&0);
                    let backend_c_count = backend_counts.get("backend-c").unwrap_or(&0);

                    let backend_a_pct = (*backend_a_count as f64 / total_requests as f64) * 100.0;
                    let backend_b_pct = (*backend_b_count as f64 / total_requests as f64) * 100.0;
                    let backend_c_pct = (*backend_c_count as f64 / total_requests as f64) * 100.0;

                    // Verify distribution (allow ±10% variance)
                    let a_ok = (backend_a_pct - 50.0).abs() <= 10.0;
                    let b_ok = (backend_b_pct - 30.0).abs() <= 10.0;
                    let c_ok = (backend_c_pct - 20.0).abs() <= 10.0;

                    if a_ok && b_ok && c_ok {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!(
                                "Weight distribution OK: backend-a={:.1}% (expect 50%), backend-b={:.1}% (expect 30%), backend-c={:.1}% (expect 20%)",
                                backend_a_pct, backend_b_pct, backend_c_pct
                            ),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Weight distribution FAILED: backend-a={:.1}% (expect 50±10%), backend-b={:.1}% (expect 30±10%), backend-c={:.1}% (expect 20±10%)",
                                backend_a_pct, backend_b_pct, backend_c_pct
                            ),
                        )
                    }
                })
            },
        )
    }

    /// Test equal weight uniform distribution
    fn test_equal_weights() -> TestCase {
        TestCase::new(
            "equal_weights",
            "Test equal weight uniform distribution（verify all backends receive traffic）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let total_requests = 30;
                    let mut backend_counts = HashMap::new();

                    for _ in 0..total_requests {
                        let trace_id = format!("weighted-equal-{}", uuid::Uuid::new_v4());
                        let request = ctx
                            .http_client
                            .get(format!("{}/echo", ctx.http_url()))
                            .header("Host", "weighted-backend.example.com")
                            .header("x-trace-id", &trace_id)
                            .header("access_log", "test_store");

                        match request.send().await {
                            Ok(_) => {
                                // Fetch access log
                                let al_client = ctx.access_log_client();
                                if let Ok(entry) = al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                    if let Some(backend_name) = entry
                                        .data
                                        .get("backend_context")
                                        .and_then(|bc| bc.get("name"))
                                        .and_then(|n| n.as_str())
                                    {
                                        *backend_counts.entry(backend_name.to_string()).or_insert(0) += 1;
                                    }
                                }
                            }
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("Request failed: {}", e));
                            }
                        }
                    }

                    // Verify all backends receive traffic
                    let backend_a_count = backend_counts.get("backend-a").unwrap_or(&0);
                    let backend_b_count = backend_counts.get("backend-b").unwrap_or(&0);
                    let backend_c_count = backend_counts.get("backend-c").unwrap_or(&0);

                    if *backend_a_count > 0 && *backend_b_count > 0 && *backend_c_count > 0 {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!(
                                "All backends received traffic: backend-a={}, backend-b={}, backend-c={}",
                                backend_a_count, backend_b_count, backend_c_count
                            ),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Not all backends received traffic: backend-a={}, backend-b={}, backend-c={}",
                                backend_a_count, backend_b_count, backend_c_count
                            ),
                        )
                    }
                })
            },
        )
    }

    /// Test backend response consistency（ensure all backends return200）
    fn test_backend_consistency() -> TestCase {
        TestCase::new(
            "backend_consistency",
            "Test all backend response consistency（200 OK）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let total_requests = 20;
                    let mut all_ok = true;
                    let mut status_codes = HashMap::new();

                    // Send 20 requests
                    for _ in 0..total_requests {
                        let request = ctx
                            .http_client
                            .get(format!("{}/echo", ctx.http_url()))
                            .header("Host", "weighted-backend.example.com");

                        match request.send().await {
                            Ok(response) => {
                                let status = response.status();
                                *status_codes.entry(status.as_u16()).or_insert(0) += 1;

                                if !status.is_success() {
                                    all_ok = false;
                                }
                            }
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("Request failed: {}", e));
                            }
                        }
                    }

                    if all_ok && status_codes.len() == 1 && status_codes.contains_key(&200) {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!("All {} requests returned 200 OK", total_requests),
                        )
                    } else {
                        TestResult::failed(start.elapsed(), format!("Inconsistent responses: {:?}", status_codes))
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for WeightedBackendTestSuite {
    fn name(&self) -> &str {
        "Weighted Backend"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/WeightedBackend")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_backend_consistency(),
            Self::test_equal_weights(),
            Self::test_weighted_distribution(),
        ]
    }
}
