// FaultInjection Plugin Test Suite
//
// Verifies delay-only / abort-only / combined behaviour, entry-condition
// gating, abort empty body, and access-log substrings.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::{Duration, Instant};

const HOST: &str = "fault.example.com";

fn fault_url(ctx: &TestContext, segment: &str) -> String {
    format!("http://{}:31180/test/fault/{}", ctx.target_host, segment)
}

pub struct FaultInjectionTestSuite;

impl FaultInjectionTestSuite {
    // ==================== 1. Fixed delay ====================
    fn test_delay_fixed() -> TestCase {
        TestCase::new(
            "fault_delay_fixed",
            "fixed 300ms delay, upstream 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = fault_url(&ctx, "delay-fixed/api");
                    let t0 = Instant::now();
                    let resp = client.get(&url).header("host", HOST).send().await;
                    match resp {
                        Ok(r) => {
                            let elapsed = t0.elapsed();
                            let status = r.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("expected 200, got {}", status));
                            }
                            if elapsed < Duration::from_millis(290) {
                                return TestResult::failed(start.elapsed(), format!("delay too short: {:?}", elapsed));
                            }
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("delay {:?} >= 290ms, status 200", elapsed),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("request error: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 2. Range delay ====================
    fn test_delay_range() -> TestCase {
        TestCase::new(
            "fault_delay_range",
            "uniform delay in [100, 500]ms over 20 requests",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = fault_url(&ctx, "delay-range/api");
                    for i in 0..20usize {
                        let t0 = Instant::now();
                        let resp = client.get(&url).header("host", HOST).send().await;
                        let elapsed = t0.elapsed();
                        match resp {
                            Ok(r) => {
                                let status = r.status().as_u16();
                                let _ = r.text().await; // drain body
                                if status != 200 {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("req {}: status {}", i, status),
                                    );
                                }
                            }
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("req {} error: {}", i, e));
                            }
                        }
                        if elapsed < Duration::from_millis(90) || elapsed > Duration::from_millis(700) {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("req {} delay out of expected band: {:?}", i, elapsed),
                            );
                        }
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "all 20 delays within [90, 700]ms band".to_string(),
                    )
                })
            },
        )
    }

    // ==================== 3. Abort with body and header ====================
    fn test_abort() -> TestCase {
        TestCase::new(
            "fault_abort",
            "503 with body + custom header X-Fault-Injected: true",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = fault_url(&ctx, "abort/api");
                    let resp = client.get(&url).header("host", HOST).send().await;
                    match resp {
                        Ok(r) => {
                            let status = r.status().as_u16();
                            let injected = r
                                .headers()
                                .get("x-fault-injected")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("")
                                .to_string();
                            let body = r.text().await.unwrap_or_default();
                            if status != 503 {
                                return TestResult::failed(start.elapsed(), format!("expected 503, got {}", status));
                            }
                            if injected != "true" {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("X-Fault-Injected expected 'true', got '{}'", injected),
                                );
                            }
                            if !body.contains("injected by fault_injection") {
                                return TestResult::failed(start.elapsed(), format!("body unexpected: '{}'", body));
                            }
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("503 abort with correct header and body (body='{}')", body),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("request error: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 4. Delay then abort ====================
    fn test_delay_and_abort() -> TestCase {
        TestCase::new(
            "fault_delay_and_abort",
            "200ms delay then 504 abort with X-Fault-Injected: delay-then-abort",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = fault_url(&ctx, "delay-and-abort/api");
                    let t0 = Instant::now();
                    let resp = client.get(&url).header("host", HOST).send().await;
                    match resp {
                        Ok(r) => {
                            let elapsed = t0.elapsed();
                            let status = r.status().as_u16();
                            let injected = r
                                .headers()
                                .get("x-fault-injected")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("")
                                .to_string();
                            if status != 504 {
                                return TestResult::failed(start.elapsed(), format!("expected 504, got {}", status));
                            }
                            if elapsed < Duration::from_millis(190) {
                                return TestResult::failed(start.elapsed(), format!("delay too short: {:?}", elapsed));
                            }
                            if injected != "delay-then-abort" {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("X-Fault-Injected expected 'delay-then-abort', got '{}'", injected),
                                );
                            }
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("504 abort after {:?} delay, header correct", elapsed),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("request error: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 5. Ratio zero — no faults injected ====================
    fn test_ratio_zero() -> TestCase {
        TestCase::new(
            "fault_ratio_zero",
            "ratios=0 -> all upstream 200, no notable delay",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = fault_url(&ctx, "ratio-zero/api");
                    for i in 0..20usize {
                        let t0 = Instant::now();
                        let resp = client.get(&url).header("host", HOST).send().await;
                        let elapsed = t0.elapsed();
                        match resp {
                            Ok(r) => {
                                let status = r.status().as_u16();
                                let _ = r.text().await; // drain body
                                if status != 200 {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("req {}: expected 200, got {}", i, status),
                                    );
                                }
                            }
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("req {} error: {}", i, e));
                            }
                        }
                        if elapsed > Duration::from_millis(100) {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("req {}: unexpected delay {:?} (ratio=0 should be fast)", i, elapsed),
                            );
                        }
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "all 20 requests passed with 200 and no injected delay".to_string(),
                    )
                })
            },
        )
    }

    // ==================== 6. Entry conditions gate ====================
    fn test_entry_conditions_gate() -> TestCase {
        TestCase::new(
            "fault_entry_conditions_gate",
            "entry conditions.run on X-Fault header: only matching requests aborted",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = fault_url(&ctx, "entry-conditions/api");

                    // Request with X-Fault: 1 must be aborted (503)
                    let r_gated = client
                        .get(&url)
                        .header("host", HOST)
                        .header("X-Fault", "1")
                        .send()
                        .await;

                    // Request without X-Fault must reach upstream (200)
                    let r_pass = client.get(&url).header("host", HOST).send().await;

                    match (r_gated, r_pass) {
                        (Ok(g), Ok(p)) => {
                            let gated_status = g.status().as_u16();
                            let pass_status = p.status().as_u16();
                            if gated_status != 503 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("gated request: expected 503, got {}", gated_status),
                                );
                            }
                            if pass_status != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("ungated request: expected 200, got {}", pass_status),
                                );
                            }
                            TestResult::passed_with_message(
                                start.elapsed(),
                                "gated->503, ungated->200 as expected".to_string(),
                            )
                        }
                        (Err(e), _) => TestResult::failed(start.elapsed(), format!("gated request error: {}", e)),
                        (_, Err(e)) => TestResult::failed(start.elapsed(), format!("ungated request error: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 7. Abort empty body (204) ====================
    fn test_abort_empty_body() -> TestCase {
        TestCase::new(
            "fault_abort_empty_body",
            "abort status=204 with empty body",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = fault_url(&ctx, "abort-empty/api");
                    let resp = client.get(&url).header("host", HOST).send().await;
                    match resp {
                        Ok(r) => {
                            let status = r.status().as_u16();
                            let cl = r
                                .headers()
                                .get("content-length")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("")
                                .to_string();
                            let body = r.text().await.unwrap_or_default();
                            if status != 204 {
                                return TestResult::failed(start.elapsed(), format!("expected 204, got {}", status));
                            }
                            if cl != "0" && !cl.is_empty() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Content-Length unexpected: '{}'", cl),
                                );
                            }
                            if !body.is_empty() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("expected empty body, got '{}'", body),
                                );
                            }
                            TestResult::passed_with_message(
                                start.elapsed(),
                                "204 with empty body confirmed".to_string(),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("request error: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 8. Access log plugin-log substrings ====================
    fn test_access_log_substrings() -> TestCase {
        TestCase::new(
            "fault_access_log_substrings",
            "access log carries 'fault delay=' and 'fault abort=504' for combined delay+abort run",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("test-fault-al-{}", uuid::Uuid::new_v4());
                    let url = fault_url(&ctx, "delay-and-abort/api");

                    // Fire the combined delay+abort scenario (ratio=1 for both).
                    // The response is a 504, so send() still returns Ok.
                    let _ = ctx
                        .http_client
                        .get(&url)
                        .header("host", HOST)
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await;

                    let entry = match ctx
                        .access_log_client()
                        .get_access_log_with_retry(&trace_id, 15, 200)
                        .await
                    {
                        Ok(e) => e,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("access log not found: {}", e)),
                    };

                    // Collect all plugin log lines from
                    // stage_logs[].edgion_plugins[].logs[].log[]
                    let mut log_items: Vec<String> = Vec::new();
                    if let Some(stages) = entry.data["stage_logs"].as_array() {
                        for stage in stages {
                            if let Some(eps) = stage["edgion_plugins"].as_array() {
                                for ep in eps {
                                    if let Some(logs) = ep["logs"].as_array() {
                                        for l in logs {
                                            if let Some(log_arr) = l["log"].as_array() {
                                                for item in log_arr {
                                                    if let Some(s) = item.as_str() {
                                                        log_items.push(s.to_string());
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    let has_delay = log_items.iter().any(|s| s.contains("fault delay="));
                    let has_abort = log_items.iter().any(|s| s.contains("fault abort=504"));

                    if !has_delay {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("missing 'fault delay=' in access log plugin items: {:?}", log_items),
                        );
                    }
                    if !has_abort {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("missing 'fault abort=504' in access log plugin items: {:?}", log_items),
                        );
                    }
                    TestResult::passed_with_message(start.elapsed(), "log substrings ok".into())
                })
            },
        )
    }
}

impl TestSuite for FaultInjectionTestSuite {
    fn name(&self) -> &str {
        "FaultInjection Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_delay_fixed(),
            Self::test_delay_range(),
            Self::test_abort(),
            Self::test_delay_and_abort(),
            Self::test_ratio_zero(),
            Self::test_entry_conditions_gate(),
            Self::test_abort_empty_body(),
            Self::test_access_log_substrings(),
        ]
    }
}
