// Weighted Backend 测试套件
//
// 依赖的配置文件（位于 examples/conf/）：
// - HTTPRoute_default_weighted-backend.yaml    # Weighted backend测试路由（50:30:20）
// - Service_edge_backend-a.yaml                # Backend A服务
// - Service_edge_backend-b.yaml                # Backend B服务
// - Service_edge_backend-c.yaml                # Backend C服务
// - EndpointSlice_edge_backend-a.yaml          # Backend A endpoints
// - EndpointSlice_edge_backend-b.yaml          # Backend B endpoints
// - EndpointSlice_edge_backend-c.yaml          # Backend C endpoints
// - EdgionPlugins_default_timeout-debug.yaml   # Debug插件配置
// - Gateway_edge_example-gateway.yaml          # Gateway 配置
// - GatewayClass__public-gateway.yaml          # GatewayClass 配置

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::collections::HashMap;
use std::time::Instant;

pub struct WeightedBackendTestSuite;

impl WeightedBackendTestSuite {
    /// 测试weighted backend流量分布（50:30:20）
    fn test_weighted_distribution() -> TestCase {
        TestCase::new(
            "weighted_distribution",
            "测试weighted backend流量分布（50:30:20）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let total_requests = 100;
                    let mut backend_counts = HashMap::new();

                    // 发送100次请求
                    for _ in 0..total_requests {
                        let request = ctx
                            .http_client
                            .get(format!("{}/echo", ctx.http_url()))
                            .header("Host", "weighted-backend.example.com");

                        match request.send().await {
                            Ok(response) => {
                                // 解析X-Debug-Access-Log header
                                if let Some(debug_header) = response.headers().get("X-Debug-Access-Log") {
                                    if let Ok(debug_str) = debug_header.to_str() {
                                        if let Ok(debug_json) = serde_json::from_str::<serde_json::Value>(debug_str) {
                                            // 提取backend name
                                            if let Some(backend_name) = debug_json["backend_context"]["name"].as_str() {
                                                *backend_counts.entry(backend_name.to_string()).or_insert(0) += 1;
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("Request failed: {}", e));
                            }
                        }
                    }

                    // 计算实际分布
                    let backend_a_count = backend_counts.get("backend-a").unwrap_or(&0);
                    let backend_b_count = backend_counts.get("backend-b").unwrap_or(&0);
                    let backend_c_count = backend_counts.get("backend-c").unwrap_or(&0);

                    let backend_a_pct = (*backend_a_count as f64 / total_requests as f64) * 100.0;
                    let backend_b_pct = (*backend_b_count as f64 / total_requests as f64) * 100.0;
                    let backend_c_pct = (*backend_c_count as f64 / total_requests as f64) * 100.0;

                    // 验证分布（允许±10%误差）
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

    /// 测试相等权重时的均匀分布
    fn test_equal_weights() -> TestCase {
        TestCase::new(
            "equal_weights",
            "测试相等权重时的均匀分布（验证所有backend都有流量）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let total_requests = 30;
                    let mut backend_counts = HashMap::new();

                    // 发送30次请求
                    for _ in 0..total_requests {
                        let request = ctx
                            .http_client
                            .get(format!("{}/echo", ctx.http_url()))
                            .header("Host", "weighted-backend.example.com");

                        match request.send().await {
                            Ok(response) => {
                                if let Some(debug_header) = response.headers().get("X-Debug-Access-Log") {
                                    if let Ok(debug_str) = debug_header.to_str() {
                                        if let Ok(debug_json) = serde_json::from_str::<serde_json::Value>(debug_str) {
                                            if let Some(backend_name) = debug_json["backend_context"]["name"].as_str() {
                                                *backend_counts.entry(backend_name.to_string()).or_insert(0) += 1;
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("Request failed: {}", e));
                            }
                        }
                    }

                    // 验证所有backend都接收到流量
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

    /// 测试backend响应一致性（确保所有backend返回200）
    fn test_backend_consistency() -> TestCase {
        TestCase::new(
            "backend_consistency",
            "测试所有backend响应一致性（200 OK）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let total_requests = 20;
                    let mut all_ok = true;
                    let mut status_codes = HashMap::new();

                    // 发送20次请求
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

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_backend_consistency(),
            Self::test_equal_weights(),
            Self::test_weighted_distribution(),
        ]
    }
}
