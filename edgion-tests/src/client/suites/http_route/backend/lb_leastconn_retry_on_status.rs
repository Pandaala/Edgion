// ============================================================================
// LB LeastConn + retry-on-status Test Suite
// ============================================================================
//
// Setup: retry attempts=3, codes=[503], LeastConn LB, 2 reachable endpoints.
// All endpoints point at the same test_server (bound to 0.0.0.0:30001) but use
// 127.0.0.1 / 127.0.0.2 so the LeastConn picker sees two independent backends.
//
// The storm requires four recorded LeastConn attempts per request, with
// RetryOnStatus on the first three. Direct lb::attempt unit tests own ledger
// accounting; the existing following healthy-traffic check owns distribution.

use crate::access_log_client::{trace_id_for_marker, AccessLogEntry};
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::metrics_helper::MetricsClient;
use async_trait::async_trait;
use std::time::{Duration, Instant};

pub struct LBLeastConnRetryOnStatusTestSuite;

impl LBLeastConnRetryOnStatusTestSuite {
    /// 8 concurrent requests to /status/503 trigger retry exhaust (status 503);
    /// then 8 /delay/1 happy-path requests verify LB still distributes properly.
    fn test_retry_storm_then_healthy_distribution() -> TestCase {
        TestCase::new(
            "leastconn_retry_storm_then_healthy",
            "LeastConn + retry-on-503 storm followed by healthy traffic still distributes across endpoints",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let port = 31124;

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(15))
                        .no_proxy()
                        .build()
                        .expect("Failed to create HTTP client");

                    // ── Phase 1: retry storm ────────────────────────────────────
                    // /status/503 returns 503 on every attempt; after retry exhausts
                    // the client sees 503. This path repeatedly exercises
                    // begin_upstream_attempt's LeastConn cleanup.
                    let storm_count = 8u32;
                    let invocation = uuid::Uuid::new_v4();
                    let markers: Vec<_> = (0..storm_count)
                        .map(|i| format!("lc-rs-storm-{invocation}-{i:04}"))
                        .collect();
                    let mut tasks = Vec::with_capacity(storm_count as usize);
                    for trace_id in &markers {
                        let client = client.clone();
                        let url = format!("http://{}:{}/status/503", ctx.target_host, port);
                        let trace_id = trace_id.clone();
                        tasks.push(tokio::spawn(async move {
                            client
                                .get(&url)
                                .header("host", "lb-lc-rs.example.com")
                                .header(
                                    "traceparent",
                                    crate::access_log_client::traceparent_for_marker(&trace_id),
                                )
                                .header("access_log", "test_store")
                                .send()
                                .await
                                .map(|r| r.status().as_u16())
                        }));
                    }

                    let mut storm_503 = 0u32;
                    let mut storm_failures = Vec::new();
                    // Drain all tasks before inspecting or deleting their logs.
                    for task in tasks {
                        match task.await {
                            Ok(Ok(503)) => storm_503 += 1,
                            Ok(Ok(status)) => storm_failures.push(format!("unexpected storm status {status}")),
                            Ok(Err(_)) => storm_failures.push("storm request failed".to_string()),
                            Err(_) => storm_failures.push("storm task failed".to_string()),
                        }
                    }
                    let logs = ctx.access_log_client();
                    let logs = &logs;
                    let observed = tokio::time::timeout(Duration::from_secs(5), async {
                        let results = futures::future::join_all(markers.iter().map(|marker| async move {
                            let entry = logs
                                .get_access_log_with_retry(marker, 10, 200)
                                .await
                                .map_err(|_| format!("missing storm access log for {marker}"))?;
                            validate_storm_attempts(&entry, marker)
                        }))
                        .await;
                        results.into_iter().filter_map(Result::err).collect::<Vec<_>>()
                    })
                    .await;
                    match observed {
                        Ok(failures) => storm_failures.extend(failures),
                        Err(_) => storm_failures.push("storm access-log observation timed out".to_string()),
                    }
                    // Delete only this invocation's evidence, including after
                    // failed requests or observations. An absent entry is fine.
                    let cleanup = tokio::time::timeout(Duration::from_secs(5), async {
                        futures::future::join_all(markers.iter().map(|marker| logs.delete_access_log(marker))).await
                    })
                    .await;
                    match cleanup {
                        Ok(results) if results.iter().all(Result::is_ok) => {}
                        Ok(_) => storm_failures.push("storm access-log cleanup failed".to_string()),
                        Err(_) => storm_failures.push("storm access-log cleanup timed out".to_string()),
                    }
                    if !storm_failures.is_empty() {
                        return TestResult::failed(start.elapsed(), storm_failures.join("; "));
                    }

                    // ── Phase 2: wait for logging flush, then send happy-path ───
                    tokio::time::sleep(tokio::time::Duration::from_millis(800)).await;

                    let happy_count = 8u32;
                    let mut h_tasks = Vec::with_capacity(happy_count as usize);
                    for i in 0..happy_count {
                        let client = client.clone();
                        let url = format!("http://{}:{}/delay/1", ctx.target_host, port);
                        let trace_id = format!("lc-rs-happy-{:04}", i);
                        h_tasks.push(tokio::spawn(async move {
                            client
                                .get(&url)
                                .header("host", "lb-lc-rs.example.com")
                                .header(
                                    "traceparent",
                                    crate::access_log_client::traceparent_for_marker(&trace_id),
                                )
                                .send()
                                .await
                                .map(|r| r.status().as_u16())
                        }));
                    }

                    let mut happy_ok = 0u32;
                    for t in h_tasks {
                        match t.await {
                            Ok(Ok(s)) if (200..300).contains(&s) => happy_ok += 1,
                            Ok(Ok(s)) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Phase2: unexpected non-2xx status: {}", s),
                                );
                            }
                            Ok(Err(e)) => {
                                return TestResult::failed(start.elapsed(), format!("happy req error: {}", e));
                            }
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("happy join error: {}", e));
                            }
                        }
                    }

                    if happy_ok != happy_count {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Phase2: expected {} 2xx, got {}", happy_count, happy_ok),
                        );
                    }

                    // ── Metrics check: happy-path phase must hit at least 2 endpoints ──
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    let metrics = MetricsClient::from_host_port(&ctx.target_host, 12090);
                    let analysis = match metrics.analyze_lb_distribution("lb-lc-rs").await {
                        Ok(a) => a,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("metrics fetch failed: {}", e)),
                    };

                    let endpoint_count = analysis.by_endpoint.len();
                    let dist: Vec<String> = analysis
                        .by_endpoint
                        .iter()
                        .map(|(ep, c)| format!("{}={}", ep, c))
                        .collect();

                    if endpoint_count < 2 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "LB collapsed to single endpoint after retry storm — \
                                 likely g1-007 ledger drift. endpoints=[{}], total={}",
                                dist.join(", "),
                                analysis.total_requests
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "storm: {} × 503 (retry exhausted) | happy: {} × 2xx | dist=[{}]",
                            storm_503,
                            happy_ok,
                            dist.join(", ")
                        ),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for LBLeastConnRetryOnStatusTestSuite {
    fn name(&self) -> &str {
        "LB LeastConn + RetryOnStatus Tests"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/LBLeastConnRetryOnStatus")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_retry_storm_then_healthy_distribution()]
    }
}

// Pure verdict over one canonical terminal entry; no log-body diagnostics.
fn validate_storm_attempts(entry: &AccessLogEntry, marker: &str) -> Result<(), String> {
    if entry.data["request_info"]["trace_id"].as_str() != Some(trace_id_for_marker(marker).as_str()) {
        return Err(format!("storm access-log identity mismatch for {marker}"));
    }
    let attempts = entry.data["backend_context"]["upstreams"]
        .as_array()
        .ok_or_else(|| format!("missing storm upstream attempts for {marker}"))?;
    if attempts.len() != 4 {
        return Err(format!(
            "expected four storm attempts for {marker}, got {}",
            attempts.len()
        ));
    }
    for (index, attempt) in attempts.iter().enumerate() {
        if attempt["status"].as_u64() != Some(503) || attempt["lb_policy"].as_str() != Some("LeastConn") {
            return Err(format!(
                "unexpected storm status or policy for {marker} attempt {index}"
            ));
        }
        let retried = match attempt.get("err") {
            None => false,
            Some(serde_json::Value::Array(errors)) if errors.iter().all(serde_json::Value::is_string) => {
                errors.iter().any(|error| error.as_str() == Some("RetryOnStatus"))
            }
            Some(_) => return Err(format!("malformed storm retry reasons for {marker} attempt {index}")),
        };
        if retried != (index < 3) {
            return Err(format!("unexpected storm retry marker for {marker} attempt {index}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(marker: &str) -> AccessLogEntry {
        AccessLogEntry {
            data: json!({
                "request_info": {"trace_id": trace_id_for_marker(marker)},
                "backend_context": {"upstreams": [
                    {"status": 503, "lb_policy": "LeastConn", "err": ["RetryOnStatus"]},
                    {"status": 503, "lb_policy": "LeastConn", "err": ["RetryOnStatus"]},
                    {"status": 503, "lb_policy": "LeastConn", "err": ["RetryOnStatus"]},
                    {"status": 503, "lb_policy": "LeastConn"}
                ]}
            }),
        }
    }

    #[test]
    fn exact_chain_accepts_omitted_or_empty_final_reasons() {
        let mut log = entry("current");
        validate_storm_attempts(&log, "current").unwrap();
        log.data["backend_context"]["upstreams"][3]["err"] = json!([]);
        validate_storm_attempts(&log, "current").unwrap();
    }

    #[test]
    fn rejects_missing_stale_truncated_or_extra_attempt_evidence() {
        let valid = entry("current");
        assert!(validate_storm_attempts(&valid, "previous").is_err());
        for bad in [json!(null), json!({}), json!([])] {
            let mut log = valid.clone();
            log.data["backend_context"]["upstreams"] = bad;
            assert!(validate_storm_attempts(&log, "current").is_err());
        }
        let mut log = valid.clone();
        log.data["request_info"] = json!({});
        assert!(validate_storm_attempts(&log, "current").is_err());
        for length in [1, 3, 5] {
            let mut log = valid.clone();
            let attempts = log.data["backend_context"]["upstreams"].as_array_mut().unwrap();
            attempts.resize(length, json!({"status": 503, "lb_policy": "LeastConn"}));
            assert!(validate_storm_attempts(&log, "current").is_err());
        }
    }

    #[test]
    fn rejects_wrong_status_policy_retry_markers_and_malformed_reasons() {
        for index in 0..4 {
            for (field, value) in [
                ("status", json!(200)),
                ("status", json!("503")),
                ("lb_policy", json!("Ewma")),
                ("err", json!(null)),
                ("err", json!("RetryOnStatus")),
                ("err", json!(["RetryOnStatus", 42])),
                ("err", if index < 3 { json!([]) } else { json!(["RetryOnStatus"]) }),
            ] {
                let mut log = entry("current");
                log.data["backend_context"]["upstreams"][index][field] = value;
                assert!(validate_storm_attempts(&log, "current").is_err(), "{index} {field}");
            }
        }
    }
}
