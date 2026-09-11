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
    trace_id: RequestTraceId,
    host: String,
    path: String,
    status: u16,
    #[serde(default)]
    direct_peer_ip: Option<String>,
    #[serde(default)]
    direct_peer_port: Option<u16>,
    #[serde(default)]
    remote_ip: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RequestTraceId {
    #[serde(default)]
    correlation_ids: HashMap<String, String>,
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
            if let Some(trace_id) = entry.request_info.trace_id.correlation_ids.get("x-trace-id") {
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
            report.push_str(&format!("📊 {} ({})\n", res.test_type.replace("-", ""), res.test_type));
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
                report.push_str("   Backend distribution:\n");
                for (backend, count) in &res.backend_counts {
                    report.push_str(&format!(
                        "     - {}: {} ({:.1}%)\n",
                        backend,
                        count,
                        (*count as f64 / res.total_requests as f64) * 100.0
                    ));
                }
            } else {
                report.push_str("   Backend distribution: not recorded\n");
            }

            if res.errors.is_empty() {
                report.push_str("   ✅ Status: LB policy verified\n");
            } else {
                report.push_str(&format!("   ❌ Errors: {:?}\n", res.errors));
            }
            report.push('\n');
        }

        report.push_str("============================================================\n");
        report.push_str("Summary:\n");
        for res in results {
            if res.errors.is_empty() {
                report.push_str(&format!(
                    "  ✅ {}: Passed - LB policy verified\n",
                    res.test_type.replace("-", "")
                ));
            } else {
                report.push_str(&format!(
                    "  ❌ {}: Failed - {:?}\n",
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

    // Helper: write lines into a temp file and return (file, analyzer).
    // The NamedTempFile must outlive the analyze_by_prefix call, so it is
    // returned together with the analyzer rather than being dropped here.
    fn analyzer_for(lines: &str) -> (NamedTempFile, AccessLogAnalyzer) {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(lines.as_bytes()).unwrap();
        f.flush().unwrap();
        let a = AccessLogAnalyzer::new(f.path());
        (f, a)
    }

    #[test]
    fn test_parse_access_log() {
        let mut temp_file = NamedTempFile::new().unwrap();
        let log_content = r#"{"request_info":{"trace_id":{"correlation_ids":{"x-trace-id":"rr-test-0001"}},"host":"lb-roundrobin.test","path":"/health","status":200},"backend_context":{"upstreams":[{"ip":"127.0.0.1","port":30001,"lb_policy":"RoundRobin"}]}}
{"request_info":{"trace_id":{"correlation_ids":{"x-trace-id":"rr-test-0002"}},"host":"lb-roundrobin.test","path":"/health","status":200},"backend_context":{"upstreams":[{"ip":"127.0.0.1","port":30002}]}}
"#;
        temp_file.write_all(log_content.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        let analyzer = AccessLogAnalyzer::new(temp_file.path());
        let result = analyzer.analyze_by_prefix("rr-test").unwrap();

        assert_eq!(result.total_requests, 2);
        assert_eq!(*result.lb_policy_counts.get("RoundRobin").unwrap(), 2);
        assert_eq!(result.backend_counts.len(), 2);
    }

    #[test]
    fn test_parse_canonical_downstream_ip_fields() {
        let lines = r#"{"request_info":{"trace_id":{"correlation_ids":{"x-trace-id":"ip-1"}},"host":"h","path":"/","status":200,"direct_peer_ip":"192.0.2.10","direct_peer_port":443,"remote_ip":"203.0.113.7"},"backend_context":{"upstreams":[{"ip":"10.0.0.1","port":9000}]}}
"#;
        let (_f, a) = analyzer_for(lines);
        let entries = a.parse_log().unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].request_info.direct_peer_ip.as_deref(), Some("192.0.2.10"));
        assert_eq!(entries[0].request_info.direct_peer_port, Some(443));
        assert_eq!(entries[0].request_info.remote_ip.as_deref(), Some("203.0.113.7"));
    }

    // Test 1: prefix filtering — only entries whose trace-id starts with the
    // given prefix are counted; the other entry must not appear in results.
    #[test]
    fn test_prefix_filtering_keeps_only_matching_entries() {
        let lines = concat!(
            r#"{"request_info":{"trace_id":{"correlation_ids":{"x-trace-id":"aa-1"}},"host":"h","path":"/","status":200},"backend_context":{"upstreams":[{"ip":"10.0.0.1","port":8001,"lb_policy":"RoundRobin"}]}}"#,
            "\n",
            r#"{"request_info":{"trace_id":{"correlation_ids":{"x-trace-id":"bb-1"}},"host":"h","path":"/","status":200},"backend_context":{"upstreams":[{"ip":"10.0.0.2","port":8002,"lb_policy":"RoundRobin"}]}}"#,
            "\n",
        );
        let (_f, a) = analyzer_for(lines);
        let result = a.analyze_by_prefix("aa").unwrap();

        assert_eq!(result.total_requests, 1);
        assert!(result.backend_counts.contains_key("10.0.0.1:8001"));
        assert!(!result.backend_counts.contains_key("10.0.0.2:8002"));
    }

    // Test 2: lb_policy that is an object with a "ConsistentHash" key maps to
    // the string "ConsistentHash".
    #[test]
    fn test_lb_policy_consistent_hash_object() {
        let lines = concat!(
            r#"{"request_info":{"trace_id":{"correlation_ids":{"x-trace-id":"ch-1"}},"host":"h","path":"/","status":200},"backend_context":{"upstreams":[{"ip":"10.0.0.1","port":9000,"lb_policy":{"ConsistentHash":{"hash_on":"header"}}}]}}"#,
            "\n",
        );
        let (_f, a) = analyzer_for(lines);
        let result = a.analyze_by_prefix("ch").unwrap();

        assert_eq!(result.total_requests, 1);
        assert_eq!(*result.lb_policy_counts.get("ConsistentHash").unwrap(), 1);
    }

    // Test 3: lb_policy that is an object WITHOUT a "ConsistentHash" key falls
    // through to the Debug-format fallback — it must not be "RoundRobin" or
    // "ConsistentHash", but exactly one policy key must be recorded.
    #[test]
    fn test_lb_policy_other_object_uses_debug_fallback() {
        let lines = concat!(
            r#"{"request_info":{"trace_id":{"correlation_ids":{"x-trace-id":"wr-1"}},"host":"h","path":"/","status":200},"backend_context":{"upstreams":[{"ip":"10.0.0.1","port":9001,"lb_policy":{"WeightedRandom":{}}}]}}"#,
            "\n",
        );
        let (_f, a) = analyzer_for(lines);
        let result = a.analyze_by_prefix("wr").unwrap();

        assert!(!result.lb_policy_counts.contains_key("RoundRobin"));
        assert!(!result.lb_policy_counts.contains_key("ConsistentHash"));
        let total: usize = result.lb_policy_counts.values().sum();
        assert_eq!(total, 1);
    }

    // Test 4: lb_policy whose JSON value is a number (not string, not object)
    // must default to "RoundRobin".
    #[test]
    fn test_lb_policy_number_defaults_to_round_robin() {
        let lines = concat!(
            r#"{"request_info":{"trace_id":{"correlation_ids":{"x-trace-id":"num-1"}},"host":"h","path":"/","status":200},"backend_context":{"upstreams":[{"ip":"10.0.0.1","port":9002,"lb_policy":123}]}}"#,
            "\n",
        );
        let (_f, a) = analyzer_for(lines);
        let result = a.analyze_by_prefix("num").unwrap();

        assert_eq!(*result.lb_policy_counts.get("RoundRobin").unwrap(), 1);
    }

    // Test 5: lb_policy field absent entirely (upstream object has no lb_policy
    // key) must default to "RoundRobin".
    #[test]
    fn test_lb_policy_absent_defaults_to_round_robin() {
        let lines = concat!(
            r#"{"request_info":{"trace_id":{"correlation_ids":{"x-trace-id":"abs-1"}},"host":"h","path":"/","status":200},"backend_context":{"upstreams":[{"ip":"10.0.0.1","port":9003}]}}"#,
            "\n",
        );
        let (_f, a) = analyzer_for(lines);
        let result = a.analyze_by_prefix("abs").unwrap();

        assert_eq!(result.total_requests, 1);
        assert_eq!(*result.lb_policy_counts.get("RoundRobin").unwrap(), 1);
    }

    // Test 6: when multiple upstreams are present, analyze_by_prefix must use
    // the LAST upstream for both the backend address and the policy — not the
    // first one.
    #[test]
    fn test_uses_last_upstream_not_first() {
        let lines = concat!(
            r#"{"request_info":{"trace_id":{"correlation_ids":{"x-trace-id":"last-1"}},"host":"h","path":"/","status":200},"backend_context":{"upstreams":[{"ip":"10.0.0.1","port":1111,"lb_policy":"RoundRobin"},{"ip":"10.0.0.2","port":2222,"lb_policy":{"ConsistentHash":{}}}]}}"#,
            "\n",
        );
        let (_f, a) = analyzer_for(lines);
        let result = a.analyze_by_prefix("last").unwrap();

        assert!(result.backend_counts.contains_key("10.0.0.2:2222"));
        assert!(!result.backend_counts.contains_key("10.0.0.1:1111"));
        assert_eq!(*result.lb_policy_counts.get("ConsistentHash").unwrap(), 1);
        assert!(!result.lb_policy_counts.contains_key("RoundRobin"));
    }

    // Test 7: entry with a matching prefix but no backend_context must still
    // increment total_requests and must push an error containing both the
    // human-readable label and the trace id.
    #[test]
    fn test_missing_backend_context_produces_error() {
        let lines = concat!(
            r#"{"request_info":{"trace_id":{"correlation_ids":{"x-trace-id":"noctx-1"}},"host":"h","path":"/","status":200}}"#,
            "\n",
        );
        let (_f, a) = analyzer_for(lines);
        let result = a.analyze_by_prefix("noctx").unwrap();

        assert_eq!(result.total_requests, 1);
        assert!(result.lb_policy_counts.is_empty());
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].contains("No backend context"));
        assert!(result.errors[0].contains("noctx-1"));
    }

    // Test 8: entry with backend_context present but upstreams is an empty
    // array must push an error containing "No upstream info".
    #[test]
    fn test_empty_upstreams_produces_error() {
        let lines = concat!(
            r#"{"request_info":{"trace_id":{"correlation_ids":{"x-trace-id":"emup-1"}},"host":"h","path":"/","status":200},"backend_context":{"upstreams":[]}}"#,
            "\n",
        );
        let (_f, a) = analyzer_for(lines);
        let result = a.analyze_by_prefix("emup").unwrap();

        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].contains("No upstream info"));
        assert!(result.errors[0].contains("emup-1"));
    }

    // Test 9: a line of garbage (not valid JSON) must be silently skipped; the
    // surrounding valid entries must still be counted and no error must be
    // pushed into the result for the garbage line.
    #[test]
    fn test_malformed_json_line_is_skipped_silently() {
        let lines = concat!(
            r#"{"request_info":{"trace_id":{"correlation_ids":{"x-trace-id":"ok-1"}},"host":"h","path":"/","status":200},"backend_context":{"upstreams":[{"ip":"127.0.0.1","port":5001}]}}"#,
            "\n",
            "not json at all\n",
            r#"{"request_info":{"trace_id":{"correlation_ids":{"x-trace-id":"ok-2"}},"host":"h","path":"/","status":200},"backend_context":{"upstreams":[{"ip":"127.0.0.1","port":5002}]}}"#,
            "\n",
        );
        let (_f, a) = analyzer_for(lines);
        let result = a.analyze_by_prefix("ok").unwrap();

        assert_eq!(result.total_requests, 2);
        // No error must be recorded in result.errors for the garbage line.
        assert!(result.errors.is_empty());
    }

    // Test 10: blank (whitespace-only) lines between valid JSON entries must be
    // silently skipped; both surrounding entries must be counted.
    #[test]
    fn test_blank_lines_are_skipped() {
        let lines = concat!(
            r#"{"request_info":{"trace_id":{"correlation_ids":{"x-trace-id":"bl-1"}},"host":"h","path":"/","status":200},"backend_context":{"upstreams":[{"ip":"127.0.0.1","port":6001}]}}"#,
            "\n",
            "\n",
            r#"{"request_info":{"trace_id":{"correlation_ids":{"x-trace-id":"bl-2"}},"host":"h","path":"/","status":200},"backend_context":{"upstreams":[{"ip":"127.0.0.1","port":6002}]}}"#,
            "\n",
        );
        let (_f, a) = analyzer_for(lines);
        let result = a.analyze_by_prefix("bl").unwrap();

        assert_eq!(result.total_requests, 2);
    }

    // Test 11: multiple requests routed to the same backend address must
    // accumulate in backend_counts; lb_policy_counts must also accumulate
    // across all matching entries.
    #[test]
    fn test_backend_counts_aggregates_same_address() {
        let entry = r#"{"request_info":{"trace_id":{"correlation_ids":{"x-trace-id":"agg-x"}},"host":"h","path":"/","status":200},"backend_context":{"upstreams":[{"ip":"127.0.0.1","port":30001}]}}"#;
        let lines = format!("{}\n{}\n{}\n", entry, entry, entry);
        let (_f, a) = analyzer_for(&lines);
        let result = a.analyze_by_prefix("agg").unwrap();

        assert_eq!(*result.backend_counts.get("127.0.0.1:30001").unwrap(), 3);
        assert_eq!(*result.lb_policy_counts.get("RoundRobin").unwrap(), 3);
    }
}
