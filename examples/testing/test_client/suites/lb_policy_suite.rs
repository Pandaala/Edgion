// LB Policy Test Suite - 使用 log_analyzer 验证负载均衡策略
// 这个测试套件演示如何通过 access log 分析来验证 LB 策略
//
// 依赖的配置文件（位于 examples/conf/）：
// - EndpointSlice_default_lb-rr-test.yaml     # 负载均衡测试后端（3个后端：127.0.0.1:9999, 127.0.0.2:9999, 127.0.0.3:9999）
// - Service_default_lb-rr-test.yaml           # 负载均衡测试服务定义
// - HTTPRoute_default_lb-rr-noretry.yaml      # RoundRobin LB 路由规则（Host: lb-rr-test.example.com）
//   注：该路由配置了 RoundRobin 负载均衡策略，并通过 annotation 禁用了重试
// - Gateway_edge_example-gateway.yaml         # Gateway 配置
// - GatewayClass__public-gateway.yaml         # GatewayClass 配置
// 
// 注：此测试使用不可达的后端地址来验证负载均衡分发策略

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
                
                // Create HTTP client with timeout > route timeout (1s backend + 2s total)
                let client = reqwest::Client::builder()
                    .connect_timeout(std::time::Duration::from_secs(2))
                    .timeout(std::time::Duration::from_secs(5))  // > route timeout (2s)
                    .build()
                    .expect("Failed to create HTTP client");
                
                let trace_prefix = "rr-noretry";
                let request_count = 9;
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
                
                // 2. Wait for all requests to complete
                for task in tasks {
                    let _ = task.await;
                }
                
                // 3. Wait for log flush
                // With route timeout of 1s, all requests should complete within 2s
                tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
                
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
                        
                        // Verify backend count (should have 3 backends)
                        if result.backend_counts.len() != 3 {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Expected 3 backends, found {}: {:?}", 
                                    result.backend_counts.len(), result.backend_counts)
                            );
                        }
                        
                        // Verify reasonable distribution (each backend should get at least 1 request)
                        // With 9 requests and 3 backends, expect roughly 3 each (±2 is acceptable)
                        for (backend, count) in &result.backend_counts {
                            if *count < 1 || *count > 5 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Backend {} used {} times, expected 1-5 (reasonable distribution)", backend, count)
                                );
                            }
                        }
                        
                        let distribution: Vec<String> = result.backend_counts.iter()
                            .map(|(backend, count)| format!("{}:{}", backend, count))
                            .collect();
                        
                        let msg = format!(
                            "RoundRobin verified: {} requests distributed across 3 backends [{}] (total: {:?})",
                            result.total_requests,
                            distribution.join(", "),
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


