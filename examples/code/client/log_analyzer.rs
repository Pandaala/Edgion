//! Access Log Analyzer for Backend Resolver Testing
//!
//! Parses and analyzes JSON access logs to verify LB policy usage

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
struct LogEntry {
    #[serde(rename = "request_info")]
    request_info: RequestInfo,
    #[serde(rename = "backend_context")]
    backend_context: Option<BackendContext>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RequestInfo {
    #[serde(rename = "x-trace-id")]
    trace_id: Option<String>,
    host: String,
    path: String,
    status: u16,
}

#[derive(Debug, Serialize, Deserialize)]
struct BackendContext {
    upstreams: Vec<UpstreamInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
struct UpstreamInfo {
    ip: String,
    port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lb_policy: Option<serde_json::Value>, // Can be string or object
}

#[derive(Debug)]
pub struct LBAnalysisResult {
    #[allow(dead_code)]
    pub test_type: String,
    pub total_requests: usize,
    pub lb_policy_counts: HashMap<String, usize>,
    pub backend_counts: HashMap<String, usize>,
    pub errors: Vec<String>,
}

pub struct AccessLogAnalyzer {
    log_path: PathBuf,
}

impl AccessLogAnalyzer {
    pub fn new(log_path: impl Into<PathBuf>) -> Self {
        Self {
            log_path: log_path.into(),
        }
    }

    fn parse_log(&self) -> Result<Vec<LogEntry>> {
        let file = File::open(&self.log_path)?;
        let reader = BufReader::new(file);
        let mut entries = Vec::new();

        for (line_num, line) in reader.lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<LogEntry>(&line) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    tracing::warn!("Failed to parse line {}: {} - {}", line_num + 1, line, e);
                    // Continue parsing other lines
                }
            }
        }
        Ok(entries)
    }

    pub fn analyze_by_prefix(&self, prefix: &str) -> Result<LBAnalysisResult> {
        let entries = self.parse_log()?;
        let mut result = LBAnalysisResult {
            test_type: prefix.to_string(),
            total_requests: 0,
            lb_policy_counts: HashMap::new(),
            backend_counts: HashMap::new(),
            errors: Vec::new(),
        };

        for entry in entries {
            if let Some(trace_id) = &entry.request_info.trace_id {
                if trace_id.starts_with(prefix) {
                    result.total_requests += 1;
                    if let Some(backend_ctx) = entry.backend_context {
                        if let Some(upstream) = backend_ctx.upstreams.last() {
                            // Parse LB policy
                            let policy_str = match &upstream.lb_policy {
                                Some(val) => {
                                    if val.is_string() {
                                        val.as_str().unwrap_or("RoundRobin").to_string()
                                    } else if val.is_object() {
                                        // For ConsistentHash with object format
                                        if val.get("ConsistentHash").is_some() {
                                            "ConsistentHash".to_string()
                                        } else {
                                            format!("{:?}", val)
                                        }
                                    } else {
                                        "RoundRobin".to_string()
                                    }
                                }
                                None => "RoundRobin".to_string(),
                            };
                            *result.lb_policy_counts.entry(policy_str).or_insert(0) += 1;

                            let backend_addr = format!("{}:{}", upstream.ip, upstream.port);
                            *result.backend_counts.entry(backend_addr).or_insert(0) += 1;
                        } else {
                            result
                                .errors
                                .push(format!("No upstream info for trace_id: {}", trace_id));
                        }
                    } else {
                        result
                            .errors
                            .push(format!("No backend context for trace_id: {}", trace_id));
                    }
                }
            }
        }
        Ok(result)
    }

    #[allow(dead_code)]
    pub fn generate_report(&self, results: &[LBAnalysisResult]) -> String {
        let mut report = String::new();
        report.push_str("============================================================\n");
        report.push_str("Backend Resolver LB Policy Analysis Report\n");
        report.push_str("============================================================\n\n");

        for res in results {
            report.push_str(&format!("📊 {}  ({})\n", res.test_type.replace("-", ""), res.test_type));
            report.push_str(&format!("   Total requests: {}\n", res.total_requests));

            if !res.lb_policy_counts.is_empty() {
                report.push_str("   LB policy distribution:\n");
                for (policy, count) in &res.lb_policy_counts {
                    report.push_str(&format!(
                        "     - {}: {} ({:.1}%)\n",
                        policy,
                        count,
                        (*count as f64 / res.total_requests as f64) * 100.0
                    ));
                }
            } else {
                report.push_str("   LB policy distribution: not recorded\n");
            }

            if !res.backend_counts.is_empty() {
                report.push_str("   :\n");
                for (backend, count) in &res.backend_counts {
                    report.push_str(&format!(
                        "     - {}: {} ({:.1}%)\n",
                        backend,
                        count,
                        (*count as f64 / res.total_requests as f64) * 100.0
                    ));
                }
            } else {
                report.push_str("   : not recorded\n");
            }

            if res.errors.is_empty() {
                report.push_str("   ✅ : LB \n");
            } else {
                report.push_str(&format!("   ❌ : {:?}\n", res.errors));
            }
            report.push('\n');
        }

        report.push_str("============================================================\n");
        report.push_str(":\n");
        for res in results {
            if res.errors.is_empty() {
                report.push_str(&format!("  ✅ {} : Passed - LB \n", res.test_type.replace("-", "")));
            } else {
                report.push_str(&format!(
                    "  ❌ {} :  - {:?}\n",
                    res.test_type.replace("-", ""),
                    res.errors
                ));
            }
        }
        report.push_str("============================================================\n");
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_parse_access_log() {
        let mut temp_file = NamedTempFile::new().unwrap();
        let log_content = r#"{"request_info":{"x-trace-id":"rr-test-0001","host":"lb-roundrobin.test","path":"/health","status":200},"backend_context":{"upstreams":[{"ip":"127.0.0.1","port":30001,"lb_policy":"RoundRobin"}]}}
{"request_info":{"x-trace-id":"rr-test-0002","host":"lb-roundrobin.test","path":"/health","status":200},"backend_context":{"upstreams":[{"ip":"127.0.0.1","port":30002}]}}
"#;
        temp_file.write_all(log_content.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        let analyzer = AccessLogAnalyzer::new(temp_file.path());
        let result = analyzer.analyze_by_prefix("rr-test").unwrap();

        assert_eq!(result.total_requests, 2);
        assert_eq!(*result.lb_policy_counts.get("RoundRobin").unwrap(), 2);
        assert_eq!(result.backend_counts.len(), 2);
    }
}
