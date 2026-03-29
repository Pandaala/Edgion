// PercentMirror Test Suite
//
// Tests Gateway API native requestMirror.percent and requestMirror.fraction fields
// in HTTPRoute (same code path as conformance test HTTPRouteRequestPercentageMirror).
//
// Each test uses a different hostname (different HTTPRoute) with PathPrefix: /
// so all requests go through /mirror/capture (the test_server capture endpoint).
//
// Routes:
//   mirror-100.example.com     — percent: 100 (all mirrored)
//   mirror-0.example.com       — percent: 0 (none mirrored)
//   mirror-frac50.example.com  — fraction: {numerator: 1, denominator: 2} (50%)
//   mirror-20.example.com      — percent: 20

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use serde_json::Value;
use std::time::Instant;
use tokio::time::{sleep, Duration};

const HOST_100: &str = "mirror-100.example.com";
const HOST_0: &str = "mirror-0.example.com";
const HOST_FRAC50: &str = "mirror-frac50.example.com";
const HOST_20: &str = "mirror-20.example.com";

pub struct PercentMirrorTestSuite;

fn gen_trace_id(label: &str) -> String {
    format!("pct-mirror-{}-{}", label, uuid::Uuid::new_v4())
}

async fn poll_mirror_received(
    ctx: &TestContext,
    trace_id: &str,
    retries: u32,
    interval_ms: u64,
) -> Result<Vec<Value>, String> {
    let direct_url = format!("http://{}:30001/mirror/query/{}", ctx.target_host, trace_id);
    let client = &ctx.http_client;

    for attempt in 0..=retries {
        match client.get(&direct_url).send().await {
            Ok(resp) => {
                if let Ok(body) = resp.json::<Value>().await {
                    if body["found"].as_bool().unwrap_or(false) {
                        let captures = body["captures"].as_array().cloned().unwrap_or_default();
                        if find_mirror_capture(&captures).is_some() {
                            return Ok(captures);
                        }
                    }
                }
            }
            Err(e) => {
                if attempt == retries {
                    return Err(format!("query request failed: {}", e));
                }
            }
        }
        if attempt < retries {
            sleep(Duration::from_millis(interval_ms)).await;
        }
    }
    Err(format!(
        "mirror capture (x-mirror: true) not received after {} retries",
        retries
    ))
}

async fn reset_mirror_store(ctx: &TestContext) {
    let url = format!("http://{}:30001/mirror/reset", ctx.target_host);
    let _ = ctx.http_client.get(&url).send().await;
}

fn find_mirror_capture(captures: &[Value]) -> Option<&Value> {
    captures
        .iter()
        .find(|c| c["headers"]["x-mirror"].as_str() == Some("true"))
}

impl PercentMirrorTestSuite {
    // ==================== TC-01: percent=100 — all mirrored ====================
    fn test_percent_100_all_mirrored() -> TestCase {
        TestCase::new(
            "percent_100_all_mirrored",
            "PercentMirror: percent=100 mirrors every request (native HTTPRoute)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    reset_mirror_store(&ctx).await;

                    let trace_id = gen_trace_id("pct100");
                    let url = format!("{}/mirror/capture", ctx.http_url());

                    let resp = match ctx
                        .http_client
                        .get(&url)
                        .header("host", HOST_100)
                        .header("x-trace-id", &trace_id)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("main request failed: {}", e)),
                    };

                    if resp.status().as_u16() != 200 {
                        return TestResult::failed(start.elapsed(), format!("main request returned {}", resp.status()));
                    }
                    let _ = resp.text().await;

                    match poll_mirror_received(&ctx, &trace_id, 20, 100).await {
                        Ok(captures) => {
                            let cap = match find_mirror_capture(&captures) {
                                Some(c) => c,
                                None => {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "No mirror capture with x-mirror: true; {} total capture(s)",
                                            captures.len()
                                        ),
                                    );
                                }
                            };
                            let has_mirror_hdr = cap["headers"]["x-mirror"]
                                .as_str()
                                .map(|v| v == "true")
                                .unwrap_or(false);
                            if !has_mirror_hdr {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("x-mirror header missing; headers: {}", cap["headers"]),
                                );
                            }
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("percent=100: mirror received ({} capture(s))", captures.len()),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Mirror not received: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== TC-02: percent=0 — none mirrored ====================
    fn test_percent_0_none_mirrored() -> TestCase {
        TestCase::new(
            "percent_0_none_mirrored",
            "PercentMirror: percent=0 mirrors no requests (native HTTPRoute)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    reset_mirror_store(&ctx).await;

                    let trace_id = gen_trace_id("pct0");
                    let url = format!("{}/mirror/capture", ctx.http_url());

                    let resp = match ctx
                        .http_client
                        .get(&url)
                        .header("host", HOST_0)
                        .header("x-trace-id", &trace_id)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("main request failed: {}", e)),
                    };

                    if resp.status().as_u16() != 200 {
                        return TestResult::failed(start.elapsed(), format!("main request returned {}", resp.status()));
                    }
                    let _ = resp.text().await;

                    sleep(Duration::from_millis(800)).await;

                    let direct_url = format!("http://{}:30001/mirror/query/{}", ctx.target_host, trace_id);
                    match ctx.http_client.get(&direct_url).send().await {
                        Ok(resp) => match resp.json::<Value>().await {
                            Ok(body) => {
                                if body["found"].as_bool().unwrap_or(false) {
                                    let captures = body["captures"].as_array().cloned().unwrap_or_default();
                                    if find_mirror_capture(&captures).is_some() {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            "percent=0 should NOT produce a mirror capture".to_string(),
                                        );
                                    }
                                }
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "percent=0: no mirror capture (as expected)".to_string(),
                                )
                            }
                            Err(e) => TestResult::failed(start.elapsed(), format!("Failed to parse response: {}", e)),
                        },
                        Err(e) => TestResult::failed(start.elapsed(), format!("query failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== TC-03: fraction 1/2 — ~50% mirrored ====================
    fn test_fraction_50_sampling() -> TestCase {
        TestCase::new(
            "fraction_50_sampling",
            "PercentMirror: fraction {1/2} mirrors roughly 50% (native HTTPRoute)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    reset_mirror_store(&ctx).await;

                    let total = 40u32;
                    let url = format!("{}/mirror/capture", ctx.http_url());

                    let mut trace_ids = Vec::with_capacity(total as usize);
                    for i in 0..total {
                        let tid = gen_trace_id(&format!("frac50-{}", i));
                        trace_ids.push(tid.clone());
                        let _ = ctx
                            .http_client
                            .get(&url)
                            .header("host", HOST_FRAC50)
                            .header("x-trace-id", &tid)
                            .send()
                            .await;
                    }

                    sleep(Duration::from_millis(1500)).await;

                    let mut received = 0u32;
                    for tid in &trace_ids {
                        let direct = format!("http://{}:30001/mirror/query/{}", ctx.target_host, tid);
                        if let Ok(resp) = ctx.http_client.get(&direct).send().await {
                            if let Ok(body) = resp.json::<Value>().await {
                                if body["found"].as_bool().unwrap_or(false) {
                                    if let Some(caps) = body["captures"].as_array() {
                                        if find_mirror_capture(caps).is_some() {
                                            received += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    let pct = (received as f64 / total as f64) * 100.0;
                    if (15.0..=85.0).contains(&pct) {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!("fraction 1/2: {}/{} mirrored ({:.0}%)", received, total, pct),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "fraction 1/2: {}/{} mirrored ({:.0}%) — expected [15%, 85%]",
                                received, total, pct
                            ),
                        )
                    }
                })
            },
        )
    }

    // ==================== TC-04: percent=20 — ~20% mirrored ====================
    fn test_percent_20_sampling() -> TestCase {
        TestCase::new(
            "percent_20_sampling",
            "PercentMirror: percent=20 mirrors roughly 20% (native HTTPRoute)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    reset_mirror_store(&ctx).await;

                    let total = 50u32;
                    let url = format!("{}/mirror/capture", ctx.http_url());

                    let mut trace_ids = Vec::with_capacity(total as usize);
                    for i in 0..total {
                        let tid = gen_trace_id(&format!("pct20-{}", i));
                        trace_ids.push(tid.clone());
                        let _ = ctx
                            .http_client
                            .get(&url)
                            .header("host", HOST_20)
                            .header("x-trace-id", &tid)
                            .send()
                            .await;
                    }

                    sleep(Duration::from_millis(1500)).await;

                    let mut received = 0u32;
                    for tid in &trace_ids {
                        let direct = format!("http://{}:30001/mirror/query/{}", ctx.target_host, tid);
                        if let Ok(resp) = ctx.http_client.get(&direct).send().await {
                            if let Ok(body) = resp.json::<Value>().await {
                                if body["found"].as_bool().unwrap_or(false) {
                                    if let Some(caps) = body["captures"].as_array() {
                                        if find_mirror_capture(caps).is_some() {
                                            received += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    let pct = (received as f64 / total as f64) * 100.0;
                    if (2.0..=45.0).contains(&pct) {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!("percent=20: {}/{} mirrored ({:.0}%)", received, total, pct),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "percent=20: {}/{} mirrored ({:.0}%) — expected [2%, 45%]",
                                received, total, pct
                            ),
                        )
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for PercentMirrorTestSuite {
    fn name(&self) -> &str {
        "PercentMirror"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Filters/PercentMirror")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_percent_100_all_mirrored(),
            Self::test_percent_0_none_mirrored(),
            Self::test_fraction_50_sampling(),
            Self::test_percent_20_sampling(),
        ]
    }
}
