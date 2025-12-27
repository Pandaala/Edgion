// LB Policy Test Suite - 使用 log_analyzer 验证负载均衡策略
// 这个测试套件演示如何通过 access log 分析来验证 LB 策略

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::log_analyzer::AccessLogAnalyzer;
use async_trait::async_trait;
use std::time::Instant;

pub struct LBPolicyTestSuite;

impl LBPolicyTestSuite {
    /// Test RoundRobin LB policy by analyzing access logs
    fn test_roundrobin_no_retry() -> TestCase {
        TestCase::new(
            "roundrobin_no_retry",
            "Verify RoundRobin with no retry through annotation",
            |ctx: TestContext| Box::pin(async move {
                let start = Instant::now();
                
                // Create HTTP client with 0.5s connect timeout (fast fail if retry misconfigured)
                let client = reqwest::Client::builder()
                    .connect_timeout(std::time::Duration::from_millis(500))
                    .timeout(std::time::Duration::from_secs(2))
                    .build()
                    .expect("Failed to create HTTP client");
                
                let trace_prefix = "rr-noretry";
                let request_count = 9;  // 9 requests = 3 backends × 3 rounds
                let mut tasks = Vec::new();
                
                // 1. Send concurrent requests
                for i in 0..request_count {
                    let client = client.clone();
                    let url = format!("{}/test", ctx.http_url());
                    let trace_id = format!("{}-{:04}", trace_prefix, i);
                    
                    let task = tokio::spawn(async move {
                        let mut request = client.get(&url);
                        request = request.header("host", "lb-rr-test.example.com");
                        request = request.header("x-trace-id", &trace_id);
                        request.send().await
                    });
                    tasks.push(task);
                }
                
                // 2. Wait for all requests to complete (including failed ones)
                // With 0.5s timeout, even if retry is misconfigured, max time ~1.5s
                for task in tasks {
                    let _ = task.await;  // Ignore results as we expect failures
                }
                
                // 3. Wait for log flush
                tokio::time::sleep(tokio::time::Duration::from_millis(800)).await;
                
                // 4. Analyze access logs
                let analyzer = AccessLogAnalyzer::new(&ctx.access_log_path);
                match analyzer.analyze_by_prefix(trace_prefix) {
                    Ok(result) => {
                        // Verify total request count
                        if result.total_requests != request_count {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Expected {} requests, found {}", request_count, result.total_requests)
                            );
                        }
                        
                        // Verify backend count
                        if result.backend_counts.len() != 3 {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Expected 3 backends, found {}: {:?}", 
                                    result.backend_counts.len(), result.backend_counts)
                            );
                        }
                        
                        // Verify RoundRobin distribution (each backend should be used 3 times)
                        for (backend, count) in &result.backend_counts {
                            if *count != 3 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Backend {} used {} times, expected 3 (perfect RR)", backend, count)
                                );
                            }
                        }
                        
                        let msg = format!(
                            "RoundRobin no-retry verified: 9 requests, 3 backends, 3 each (total: {:?})",
                            start.elapsed()
                        );
                        TestResult::passed_with_message(start.elapsed(), msg)
                    }
                    Err(e) => TestResult::failed(
                        start.elapsed(),
                        format!("Log analysis failed: {}", e)
                    ),
                }
            })
        )
    }
    
    /*
    /// Test EWMA LB policy by analyzing access logs
    fn test_ewma_lb_policy() -> TestCase {
        TestCase::new(
            "ewma_lb_policy",
            "Verify EWMA LB policy through access log analysis",
            |ctx: TestContext| Box::pin(async move {
                let start = Instant::now();
                let client = &ctx.http_client;
                
                // Send multiple requests with trace ID prefix
                let trace_prefix = "ewma-test";
                let request_count = 10;
                
                for i in 0..request_count {
                    let url = format!("{}/health", ctx.http_url());
                    let trace_id = format!("{}-{:04}", trace_prefix, i);
                    
                    let mut request = client.get(&url);
                    request = request.header("host", "ewma.test.example.com");
                    request = request.header("x-trace-id", &trace_id);
                    
                    match request.send().await {
                        Ok(_) => {},
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Request {} failed: {}", i, e)
                            );
                        }
                    }
                }
                
                // Give logger time to flush
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                
                // Analyze access logs
                let analyzer = AccessLogAnalyzer::new(&ctx.access_log_path);
                match analyzer.analyze_by_prefix(trace_prefix) {
                    Ok(result) => {
                        if result.total_requests != request_count {
                            return TestResult::failed(
                                start.elapsed(),
                                format!(
                                    "Expected {} requests in log, found {}",
                                    request_count, result.total_requests
                                )
                            );
                        }
                        
                        // Check if EWMA policy is recorded
                        let has_ewma = result.lb_policy_counts.contains_key("Ewma");
                        if !has_ewma {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Expected EWMA policy, found: {:?}", result.lb_policy_counts)
                            );
                        }
                        
                        let msg = format!(
                            "EWMA verified - {} requests with EWMA policy",
                            result.total_requests
                        );
                        
                        TestResult::passed_with_message(start.elapsed(), msg)
                    }
                    Err(e) => TestResult::failed(
                        start.elapsed(),
                        format!("Failed to analyze access log: {}", e)
                    ),
                }
            })
        )
    }
    */
    
    /*
    /// Test access log contains backend context
    fn test_access_log_backend_context() -> TestCase {
        TestCase::new(
            "access_log_backend_context",
            "Verify access log contains backend context information",
            |ctx: TestContext| Box::pin(async move {
                let start = Instant::now();
                let client = &ctx.http_client;
                
                let trace_id = "backend-ctx-test";
                let url = format!("{}/health", ctx.http_url());
                
                let mut request = client.get(&url);
                request = request.header("host", "test.example.com");
                request = request.header("x-trace-id", trace_id);
                
                match request.send().await {
                    Ok(_) => {},
                    Err(e) => {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Request failed: {}", e)
                        );
                    }
                }
                
                // Give logger time to flush
                tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                
                // Analyze access logs
                let analyzer = AccessLogAnalyzer::new(&ctx.access_log_path);
                match analyzer.analyze_by_prefix(trace_id) {
                    Ok(result) => {
                        if result.total_requests == 0 {
                            return TestResult::failed(
                                start.elapsed(),
                                "No requests found in access log".to_string()
                            );
                        }
                        
                        if result.backend_counts.is_empty() {
                            return TestResult::failed(
                                start.elapsed(),
                                "No backend context found in access log".to_string()
                            );
                        }
                        
                        // Get the backend address
                        let backend_addr = result.backend_counts.keys().next().unwrap();
                        let msg = format!(
                            "Backend context verified - routed to {}",
                            backend_addr
                        );
                        
                        TestResult::passed_with_message(start.elapsed(), msg)
                    }
                    Err(e) => TestResult::failed(
                        start.elapsed(),
                        format!("Failed to analyze access log: {}", e)
                    ),
                }
            })
        )
    }
    */
}

#[async_trait]
impl TestSuite for LBPolicyTestSuite {
    fn name(&self) -> &str {
        "LB Policy Tests (Log Analyzer)"
    }
    
    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_roundrobin_no_retry(),
            // Self::test_ewma_lb_policy(),  // Commented out
            // Self::test_access_log_backend_context(),  // Commented out
        ]
    }
}


