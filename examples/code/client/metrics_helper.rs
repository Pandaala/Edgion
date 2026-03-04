//! Metrics Helper for Test Client
//!
//! Provides utilities to fetch and parse Prometheus metrics from the gateway.
//! Used primarily for LB distribution verification and test data analysis.

use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Backend request metric data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendMetric {
    /// Gateway namespace
    pub gateway_ns: String,
    /// Gateway name
    pub gateway_name: String,
    /// Backend namespace
    pub backend_ns: String,
    /// Backend name
    pub backend_name: String,
    /// Protocol (http, grpc, websocket)
    pub protocol: String,
    /// Status group (2xx, 4xx, 5xx, etc.)
    pub status: String,
    /// Test key for filtering
    pub test_key: String,
    /// Parsed test data (from JSON)
    pub test_data: Option<TestData>,
    /// Request count
    pub count: u64,
}

/// Test data parsed from metrics
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TestData {
    /// Backend IP address
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    /// Backend port
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Consistent hash key
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash_key: Option<String>,
    /// Retry attempt count
    #[serde(skip_serializing_if = "Option::is_none")]
    pub try_count: Option<u32>,
    /// Error message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Latency in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

/// Metrics client for fetching and parsing Prometheus metrics
pub struct MetricsClient {
    client: Client,
    base_urls: Vec<String>,
}

impl MetricsClient {
    /// Create a new metrics client
    pub fn new(base_url: String) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            base_urls: vec![base_url],
        }
    }

    pub fn with_base_urls(base_urls: Vec<String>) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to create HTTP client");

        let filtered = base_urls
            .into_iter()
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty())
            .collect::<Vec<_>>();

        Self {
            client,
            base_urls: if filtered.is_empty() {
                vec!["http://127.0.0.1:5901".to_string()]
            } else {
                filtered
            },
        }
    }

    /// Create from host and port
    pub fn from_host_port(host: &str, port: u16) -> Self {
        if let Ok(raw) = std::env::var("EDGION_TEST_GATEWAY_METRICS_ENDPOINTS") {
            let endpoints = raw
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| {
                    if s.starts_with("http://") || s.starts_with("https://") {
                        s.trim_end_matches('/').to_string()
                    } else if s.contains(':') {
                        format!("http://{}", s)
                    } else {
                        format!("http://{}:{}", s, port)
                    }
                })
                .collect::<Vec<_>>();
            if !endpoints.is_empty() {
                return Self::with_base_urls(endpoints);
            }
        }
        Self::new(format!("http://{}:{}", host, port))
    }

    /// Fetch raw Prometheus metrics text
    pub async fn fetch_raw_metrics(&self) -> Result<String> {
        let base_url = self
            .base_urls
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("No metrics endpoint configured"))?;
        let url = format!("{}/metrics", base_url);
        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            return Err(anyhow!("Failed to fetch metrics: HTTP {}", response.status()));
        }

        Ok(response.text().await?)
    }

    /// Fetch and parse backend request metrics
    ///
    /// Returns all `edgion_backend_requests_total` metrics parsed into structured data.
    pub async fn fetch_backend_metrics(&self) -> Result<Vec<BackendMetric>> {
        if self.base_urls.len() <= 1 {
            let raw = self.fetch_raw_metrics().await?;
            return parse_backend_metrics(&raw);
        }

        let mut all_metrics = Vec::new();
        let mut errors = Vec::new();

        for base_url in &self.base_urls {
            let url = format!("{}/metrics", base_url);
            match self.client.get(&url).send().await {
                Ok(resp) => {
                    if !resp.status().is_success() {
                        errors.push(format!("{} -> HTTP {}", url, resp.status()));
                        continue;
                    }
                    match resp.text().await {
                        Ok(body) => match parse_backend_metrics(&body) {
                            Ok(mut metrics) => all_metrics.append(&mut metrics),
                            Err(e) => errors.push(format!("{} -> parse error: {}", url, e)),
                        },
                        Err(e) => errors.push(format!("{} -> read body error: {}", url, e)),
                    }
                }
                Err(e) => {
                    errors.push(format!("{} -> request error: {}", url, e));
                }
            }
        }

        if all_metrics.is_empty() {
            return Err(anyhow!(
                "Failed to fetch metrics from all endpoints: {}",
                errors.join("; ")
            ));
        }
        Ok(all_metrics)
    }

    /// Fetch backend metrics filtered by test_key
    ///
    /// Only returns metrics that match the specified test_key.
    pub async fn fetch_backend_metrics_by_key(&self, test_key: &str) -> Result<Vec<BackendMetric>> {
        let metrics = self.fetch_backend_metrics().await?;
        Ok(metrics.into_iter().filter(|m| m.test_key == test_key).collect())
    }

    /// Analyze LB distribution for a specific test_key
    ///
    /// Returns a map of backend (ip:port or name) -> request count
    pub async fn analyze_lb_distribution(&self, test_key: &str) -> Result<LbDistributionAnalysis> {
        let metrics = self.fetch_backend_metrics_by_key(test_key).await?;
        analyze_lb_distribution(&metrics)
    }

    /// Analyze consistent hash consistency for a specific test_key
    ///
    /// Verifies that the same hash_key always routes to the same backend.
    pub async fn analyze_chash_consistency(&self, test_key: &str) -> Result<ConsistentHashAnalysis> {
        let metrics = self.fetch_backend_metrics_by_key(test_key).await?;
        analyze_chash_consistency(&metrics)
    }
    /// Analyze latency for a specific test_key
    ///
    /// Returns latency statistics (min, max, avg) based on `latency_ms` in test_data.
    pub async fn analyze_latency(&self, test_key: &str) -> Result<LatencyAnalysis> {
        let metrics = self.fetch_backend_metrics_by_key(test_key).await?;
        analyze_latency(&metrics)
    }
}

/// LB distribution analysis result
#[derive(Debug, Clone)]
pub struct LbDistributionAnalysis {
    /// Total request count
    pub total_requests: u64,
    /// Distribution by backend IP:port
    pub by_endpoint: HashMap<String, u64>,
    /// Distribution by backend service name
    pub by_service: HashMap<String, u64>,
    /// Distribution ratio (0.0 - 1.0) by endpoint
    pub ratio_by_endpoint: HashMap<String, f64>,
    /// Whether distribution is balanced (within 10% variance)
    pub is_balanced: bool,
    /// Maximum variance from expected distribution
    pub max_variance: f64,
}

/// Consistent hash analysis result
#[derive(Debug, Clone)]
pub struct ConsistentHashAnalysis {
    /// Total request count
    pub total_requests: u64,
    /// Number of unique hash keys
    pub unique_keys: usize,
    /// Map of hash_key -> backend endpoint (ip:port)
    pub key_to_endpoint: HashMap<String, String>,
    /// Whether all requests with same hash_key went to same backend
    pub is_consistent: bool,
    /// Consistency rate (0.0 - 1.0)
    pub consistency_rate: f64,
    /// Inconsistent keys (if any)
    pub inconsistent_keys: Vec<String>,
}

/// Latency analysis result
#[derive(Debug, Clone, Default)]
pub struct LatencyAnalysis {
    /// Total requests with latency data
    pub total_requests: u64,
    /// Minimum latency in ms
    pub min_latency_ms: u64,
    /// Maximum latency in ms
    pub max_latency_ms: u64,
    /// Average latency in ms
    pub avg_latency_ms: f64,
    /// All latency values
    pub samples: Vec<u64>,
}

/// Parse backend metrics from Prometheus text format
fn parse_backend_metrics(raw: &str) -> Result<Vec<BackendMetric>> {
    let mut metrics = Vec::new();

    for line in raw.lines() {
        // Skip comments and empty lines
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }

        // Look for edgion_backend_requests_total metric
        if !line.starts_with("edgion_backend_requests_total{") {
            continue;
        }

        if let Some(metric) = parse_metric_line(line) {
            metrics.push(metric);
        }
    }

    Ok(metrics)
}

/// Parse a single metric line
///
/// Format: `edgion_backend_requests_total{label1="value1",label2="value2",...} count`
fn parse_metric_line(line: &str) -> Option<BackendMetric> {
    // Split into labels part and value part
    let brace_start = line.find('{')?;
    let brace_end = line.rfind('}')?;
    let labels_str = &line[brace_start + 1..brace_end];
    let value_str = line[brace_end + 1..].trim();

    // Parse count value
    let count: u64 = value_str.parse().ok()?;

    // Parse labels
    let labels = parse_labels(labels_str);

    // Extract test_data JSON and parse it
    let test_data = labels
        .get("test_data")
        .filter(|s| !s.is_empty())
        .and_then(|json| serde_json::from_str(json).ok());

    Some(BackendMetric {
        gateway_ns: labels.get("gateway_namespace").cloned().unwrap_or_default(),
        gateway_name: labels.get("gateway_name").cloned().unwrap_or_default(),
        backend_ns: labels.get("backend_namespace").cloned().unwrap_or_default(),
        backend_name: labels.get("backend_name").cloned().unwrap_or_default(),
        protocol: labels.get("protocol").cloned().unwrap_or_default(),
        status: labels.get("status").cloned().unwrap_or_default(),
        test_key: labels.get("test_key").cloned().unwrap_or_default(),
        test_data,
        count,
    })
}

/// Parse Prometheus label string into a HashMap
///
/// Handles escaped quotes and special characters in label values.
fn parse_labels(labels_str: &str) -> HashMap<String, String> {
    let mut labels = HashMap::new();
    let mut chars = labels_str.chars().peekable();

    while chars.peek().is_some() {
        // Skip whitespace and commas
        while chars.peek().is_some_and(|c| *c == ',' || c.is_whitespace()) {
            chars.next();
        }

        if chars.peek().is_none() {
            break;
        }

        // Parse label name
        let mut name = String::new();
        while chars.peek().is_some_and(|c| *c != '=') {
            name.push(chars.next().unwrap());
        }
        chars.next(); // skip '='

        // Parse label value (quoted string)
        if chars.next() != Some('"') {
            continue;
        }

        let mut value = String::new();
        let mut escaped = false;
        loop {
            match chars.next() {
                Some('\\') if !escaped => {
                    escaped = true;
                }
                Some('"') if !escaped => {
                    break;
                }
                Some(c) => {
                    if escaped {
                        // Handle escape sequences
                        match c {
                            'n' => value.push('\n'),
                            't' => value.push('\t'),
                            '\\' => value.push('\\'),
                            '"' => value.push('"'),
                            _ => {
                                value.push('\\');
                                value.push(c);
                            }
                        }
                        escaped = false;
                    } else {
                        value.push(c);
                    }
                }
                None => break,
            }
        }

        labels.insert(name, value);
    }

    labels
}

/// Analyze LB distribution from metrics
fn analyze_lb_distribution(metrics: &[BackendMetric]) -> Result<LbDistributionAnalysis> {
    let mut by_endpoint: HashMap<String, u64> = HashMap::new();
    let mut by_service: HashMap<String, u64> = HashMap::new();
    let mut total_requests: u64 = 0;

    for metric in metrics {
        total_requests += metric.count;

        // Group by service
        let service_key = format!("{}/{}", metric.backend_ns, metric.backend_name);
        *by_service.entry(service_key).or_insert(0) += metric.count;

        // Group by endpoint (ip:port) if available in test_data
        if let Some(ref test_data) = metric.test_data {
            if let (Some(ip), Some(port)) = (&test_data.ip, test_data.port) {
                let endpoint_key = format!("{}:{}", ip, port);
                *by_endpoint.entry(endpoint_key).or_insert(0) += metric.count;
            }
        }
    }

    // Calculate ratios and variance
    let endpoint_count = by_endpoint.len();
    let expected_ratio = if endpoint_count > 0 {
        1.0 / endpoint_count as f64
    } else {
        0.0
    };

    let mut ratio_by_endpoint: HashMap<String, f64> = HashMap::new();
    let mut max_variance: f64 = 0.0;

    for (endpoint, count) in &by_endpoint {
        let actual_ratio = if total_requests > 0 {
            *count as f64 / total_requests as f64
        } else {
            0.0
        };
        ratio_by_endpoint.insert(endpoint.clone(), actual_ratio);

        let variance = (actual_ratio - expected_ratio).abs();
        if variance > max_variance {
            max_variance = variance;
        }
    }

    // Consider balanced if max variance is within 10%
    let is_balanced = max_variance <= 0.1;

    Ok(LbDistributionAnalysis {
        total_requests,
        by_endpoint,
        by_service,
        ratio_by_endpoint,
        is_balanced,
        max_variance,
    })
}

/// Analyze consistent hash consistency from metrics
///
/// Verifies that the same hash_key always routes to the same backend.
fn analyze_chash_consistency(metrics: &[BackendMetric]) -> Result<ConsistentHashAnalysis> {
    let mut total_requests: u64 = 0;
    // Map: hash_key -> (endpoint, count)
    let mut key_mapping: HashMap<String, HashMap<String, u64>> = HashMap::new();

    for metric in metrics {
        total_requests += metric.count;

        if let Some(ref test_data) = metric.test_data {
            // Need both hash_key and endpoint (ip:port)
            if let (Some(hash_key), Some(ip), Some(port)) = (&test_data.hash_key, &test_data.ip, test_data.port) {
                if !hash_key.is_empty() {
                    let endpoint = format!("{}:{}", ip, port);
                    let endpoints = key_mapping.entry(hash_key.clone()).or_default();
                    *endpoints.entry(endpoint).or_insert(0) += metric.count;
                }
            }
        }
    }

    // Analyze consistency
    let unique_keys = key_mapping.len();
    let mut key_to_endpoint: HashMap<String, String> = HashMap::new();
    let mut inconsistent_keys: Vec<String> = Vec::new();
    let mut consistent_count: u64 = 0;
    let mut total_keyed_requests: u64 = 0;

    for (hash_key, endpoints) in &key_mapping {
        // Count total requests for this key
        let key_total: u64 = endpoints.values().sum();
        total_keyed_requests += key_total;

        if endpoints.len() == 1 {
            // All requests for this key went to same endpoint - consistent
            let endpoint = endpoints.keys().next().unwrap().clone();
            key_to_endpoint.insert(hash_key.clone(), endpoint);
            consistent_count += key_total;
        } else {
            // Multiple endpoints for same key - inconsistent
            // Use the most common endpoint as the "expected" one
            let (primary_endpoint, primary_count) = endpoints.iter().max_by_key(|(_, count)| *count).unwrap();
            key_to_endpoint.insert(hash_key.clone(), primary_endpoint.clone());
            inconsistent_keys.push(hash_key.clone());
            consistent_count += primary_count;
        }
    }

    let consistency_rate = if total_keyed_requests > 0 {
        consistent_count as f64 / total_keyed_requests as f64
    } else {
        1.0 // No keyed requests means technically consistent
    };

    let is_consistent = inconsistent_keys.is_empty() && unique_keys > 0;

    Ok(ConsistentHashAnalysis {
        total_requests,
        unique_keys,
        key_to_endpoint,
        is_consistent,
        consistency_rate,
        inconsistent_keys,
    })
}

/// Analyze latency statistics from metrics
fn analyze_latency(metrics: &[BackendMetric]) -> Result<LatencyAnalysis> {
    let mut samples: Vec<u64> = Vec::new();

    for metric in metrics {
        if let Some(ref test_data) = metric.test_data {
            if let Some(latency) = test_data.latency_ms {
                // If count > 1, we assume all requests in this bucket had similar latency?
                // Or simply add it `count` times?
                // Prometheus metrics are counters.
                // But `edgion_backend_requests_total` is a Counter. The value is total requests ever made.
                // Wait, if I fetch metrics, I get the current value of the counter.
                // If I made 5 requests, the counter is 5.
                // How does `test_data` work with counters?
                // If `test_data` varies per request (e.g. latency), then each request MUST produce a unique metric line?
                // Unless the labels are identical.
                // If `latency_ms` is in `test_data` label, then for every unique latency, a new time series is created!
                // This would be high cardinality.
                // But for a test with 1 request, it's fine.
                // For multiple requests, if latencies differ, we get multiple lines.
                // So we shout iterate and add `latency` to samples `metric.count` times?
                // No, metric.count is the value of the counter.
                // If the metric line is for latency=100ms and count=1, we have 1 request.
                // If latency=101ms and count=1, we have another.
                // So yes, we should add `latency` `metric.count` times.

                for _ in 0..metric.count {
                    samples.push(latency);
                }
            }
        }
    }

    if samples.is_empty() {
        return Ok(LatencyAnalysis::default());
    }

    samples.sort_unstable();
    let min_latency_ms = *samples.first().unwrap();
    let max_latency_ms = *samples.last().unwrap();
    let sum: u64 = samples.iter().sum();
    let avg_latency_ms = sum as f64 / samples.len() as f64;

    Ok(LatencyAnalysis {
        total_requests: samples.len() as u64,
        min_latency_ms,
        max_latency_ms,
        avg_latency_ms,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_labels() {
        let labels_str =
            r#"gateway_namespace="default",gateway_name="test-gw",test_data="{\"ip\":\"10.0.0.1\",\"port\":8080}""#;
        let labels = parse_labels(labels_str);

        assert_eq!(labels.get("gateway_namespace"), Some(&"default".to_string()));
        assert_eq!(labels.get("gateway_name"), Some(&"test-gw".to_string()));
        assert!(labels.get("test_data").is_some());
    }

    #[test]
    fn test_parse_metric_line() {
        let line = r#"edgion_backend_requests_total{gateway_namespace="default",gateway_name="gw",backend_namespace="ns",backend_name="svc",protocol="http",status="2xx",test_key="test-001",test_data=""} 100"#;

        let metric = parse_metric_line(line).unwrap();
        assert_eq!(metric.gateway_ns, "default");
        assert_eq!(metric.gateway_name, "gw");
        assert_eq!(metric.backend_ns, "ns");
        assert_eq!(metric.backend_name, "svc");
        assert_eq!(metric.protocol, "http");
        assert_eq!(metric.status, "2xx");
        assert_eq!(metric.test_key, "test-001");
        assert_eq!(metric.count, 100);
    }

    #[test]
    fn test_parse_metric_line_with_test_data() {
        let line = r#"edgion_backend_requests_total{gateway_namespace="default",gateway_name="gw",backend_namespace="ns",backend_name="svc",protocol="http",status="2xx",test_key="test-001",test_data="{\"ip\":\"10.0.0.1\",\"port\":8080}"} 50"#;

        let metric = parse_metric_line(line).unwrap();
        assert_eq!(metric.count, 50);

        let test_data = metric.test_data.unwrap();
        assert_eq!(test_data.ip, Some("10.0.0.1".to_string()));
        assert_eq!(test_data.port, Some(8080));
    }

    #[test]
    fn test_analyze_lb_distribution() {
        let metrics = vec![
            BackendMetric {
                gateway_ns: "default".to_string(),
                gateway_name: "gw".to_string(),
                backend_ns: "ns".to_string(),
                backend_name: "svc".to_string(),
                protocol: "http".to_string(),
                status: "2xx".to_string(),
                test_key: "test".to_string(),
                test_data: Some(TestData {
                    ip: Some("10.0.0.1".to_string()),
                    port: Some(8080),
                    ..Default::default()
                }),
                count: 50,
            },
            BackendMetric {
                gateway_ns: "default".to_string(),
                gateway_name: "gw".to_string(),
                backend_ns: "ns".to_string(),
                backend_name: "svc".to_string(),
                protocol: "http".to_string(),
                status: "2xx".to_string(),
                test_key: "test".to_string(),
                test_data: Some(TestData {
                    ip: Some("10.0.0.2".to_string()),
                    port: Some(8080),
                    ..Default::default()
                }),
                count: 50,
            },
        ];

        let analysis = analyze_lb_distribution(&metrics).unwrap();
        assert_eq!(analysis.total_requests, 100);
        assert_eq!(analysis.by_endpoint.len(), 2);
        assert!(analysis.is_balanced);
        assert!(analysis.max_variance <= 0.1);
    }
}
