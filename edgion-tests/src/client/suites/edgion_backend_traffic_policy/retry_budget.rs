// EdgionBackendTrafficPolicy/RetryBudget scenario.
//
// End-to-end coverage for `EdgionBackendTrafficPolicy.spec.retryConstraint`
// (see docs/superpowers/specs/2026-07-21-retry-budget-retryconstraint-design.md),
// against the fixtures in
// edgion-tests/integration/conf/EdgionBackendTrafficPolicy/RetryBudget/:
//
//   Gateway rb-gateway (port from ports.json, listener "http")
//     hostname "retry-budget.example.com"
//     /  -> rb-svc  (shared plain-HTTP echo backend, 127.0.0.1:30001)
//   HTTPRoute rb-route: retry { attempts: 3, codes: [503] }, request timeout 10s
//   EdgionBackendTrafficPolicy rb-policy on Service rb-svc:
//     retryConstraint { budget: {percent: 20, interval: 10s},
//                       minRetryRate: {count: 5, interval: 1s} }
//
// The backend is driven to a permanent-503 state by hitting test_server's
// `/status/503` endpoint, so EVERY original attempt is a retryable failure and
// the route retry policy is exercised on every request. The `retryConstraint`
// then governs how many of those retries are actually admitted.
//
// Coverage map to spec §8 "Integration":
//   1. minRetryRate floor lets a trickle of retries through at low volume
//      -> test_floor_permits_low_volume_retry
//   2. Under sustained load the percentage cap refuses the majority of retries,
//      each refusal returns a deterministic 503, and the denial counter
//      `edgion_gateway_retry_budget_denied_total` advances
//      -> test_budget_denies_under_load
//
// NOTE ON APPROXIMATION (spec §9 / D7): the budget is a SOFT cap — under
// concurrency it may over-admit by up to N-1 concurrent deciders, so this suite
// asserts ranges/bands (a low-volume retry happens; a substantial number of
// retries are denied under load) rather than exact retry counts.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::{Duration, Instant};

pub struct RetryBudgetTestSuite;

const HOST: &str = "retry-budget.example.com";

/// Fetch the current value of the bare (unlabeled) counter
/// `edgion_gateway_retry_budget_denied_total` from the Gateway's Prometheus
/// endpoint (fixed port 12090, matching the pattern used by the sibling
/// UpstreamAuthority / LB suites — NOT `ctx.admin_port`, which is the
/// Controller's Admin API port). Returns 0.0 if the counter has never been
/// incremented yet (Prometheus does not expose a series before its first
/// increment).
async fn denied_total(ctx: &TestContext) -> Result<f64, String> {
    let metrics_client = crate::metrics_helper::MetricsClient::from_host_port(&ctx.target_host, 12090);
    crate::metrics_helper::checked_counter_value(
        metrics_client.fetch_raw_metrics().await,
        "edgion_gateway_retry_budget_denied_total",
        &[],
    )
    .map_err(|error| error.to_string())
}

/// Count the upstream attempts recorded for a request in its access-log entry.
/// `backend_context.upstreams` holds one element per upstream attempt (the
/// original plus every admitted retry), so a length >= 2 proves at least one
/// retry was actually attempted for that request.
fn upstream_attempts(entry: &crate::access_log_client::AccessLogEntry) -> usize {
    entry.data["backend_context"]["upstreams"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0)
}

impl RetryBudgetTestSuite {
    /// (1) minRetryRate floor: at low request volume the budget still admits a
    /// retry, so the low-traffic backend is not starved of retries by a
    /// percentage cap that rounds to ~0. Sent slowly (sequential, spaced) and
    /// FIRST in the suite so the sliding windows are empty — the percentage
    /// ceiling and the floor both permit the early retries. Proven from the
    /// access log: at least one request records >= 2 upstream attempts.
    fn test_floor_permits_low_volume_retry() -> TestCase {
        TestCase::new(
            "floor_permits_low_volume_retry",
            "At low volume the minRetryRate floor still admits a retry to the degrading backend",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/status/503", ctx.http_url());

                    let mut max_attempts = 0usize;
                    let mut saw_503 = false;
                    for i in 0..3 {
                        let trace_id = format!("rb-floor-{}-{}", i, uuid::Uuid::new_v4());
                        let resp = match ctx
                            .http_client
                            .get(&url)
                            .header("Host", HOST)
                            .header(
                                "traceparent",
                                crate::access_log_client::traceparent_for_marker(&trace_id),
                            )
                            .header("access_log", "test_store")
                            .send()
                            .await
                        {
                            Ok(r) => r,
                            Err(e) => return TestResult::failed(start.elapsed(), format!("request {i} failed: {e}")),
                        };
                        if resp.status().as_u16() == 503 {
                            saw_503 = true;
                        }

                        let al_client = ctx.access_log_client();
                        if let Ok(entry) = al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                            max_attempts = max_attempts.max(upstream_attempts(&entry));
                        }
                        // Space requests so the floor window (1s) is not saturated.
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    }

                    if !saw_503 {
                        return TestResult::failed(
                            start.elapsed(),
                            "expected the permanent-503 backend to return 503 to the client".to_string(),
                        );
                    }
                    if max_attempts < 2 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "expected at least one low-volume request to be retried (>= 2 upstream attempts), \
                                 saw a max of {max_attempts} attempts — the minRetryRate floor did not admit a retry"
                            ),
                        );
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "low-volume request retried ({max_attempts} upstream attempts); floor admits a trickle"
                        ),
                    )
                })
            },
        )
    }

    /// (2) Percentage cap under load: a burst of concurrent requests at the
    /// permanent-503 backend drives total attempt volume up fast; once the 20%
    /// ceiling and the 5/1s floor are exhausted, further retries are refused
    /// with a deterministic 503 and `edgion_gateway_retry_budget_denied_total`
    /// advances. Soft cap (spec §9/D7): we assert a BAND — a substantial number
    /// of retries were denied — not an exact count, and that every response is a
    /// 503 (either the backend's own 503 for admitted-and-exhausted retries, or
    /// the synthetic budget-denial 503).
    fn test_budget_denies_under_load() -> TestCase {
        TestCase::new(
            "budget_denies_under_load",
            "Under load the percentage cap refuses the majority of retries with a 503 and advances the denial counter",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/status/503", ctx.http_url());

                    let denied_before = match denied_total(&ctx).await {
                        Ok(value) => value,
                        Err(error) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("baseline metrics observation failed: {error}"),
                            )
                        }
                    };

                    // Fire a concurrent burst. Each request's original attempt is
                    // a 503 the route wants to retry; the budget admits only a
                    // minority (~20% of total attempts + the 5/1s floor).
                    const BURST: usize = 60;
                    let mut handles = Vec::with_capacity(BURST);
                    for i in 0..BURST {
                        let client = ctx.http_client.clone();
                        let url = url.clone();
                        handles.push(tokio::spawn(async move {
                            client
                                .get(&url)
                                .header("Host", HOST)
                                .header(
                                    "traceparent",
                                    crate::access_log_client::traceparent_for_marker(format!("rb-load-{i}")),
                                )
                                .send()
                                .await
                                .map(|r| r.status().as_u16())
                        }));
                    }

                    let mut non_503 = 0usize;
                    let mut errors = 0usize;
                    for h in handles {
                        match h.await {
                            Ok(Ok(503)) => {}
                            Ok(Ok(other)) => {
                                non_503 += 1;
                                if non_503 == 1 {
                                    eprintln!("[RetryBudget] unexpected non-503 status under load: {other}");
                                }
                            }
                            Ok(Err(e)) => {
                                errors += 1;
                                if errors == 1 {
                                    eprintln!("[RetryBudget] request error under load: {e}");
                                }
                            }
                            Err(e) => {
                                errors += 1;
                                if errors == 1 {
                                    eprintln!("[RetryBudget] join error under load: {e}");
                                }
                            }
                        }
                    }

                    if errors > 0 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("{errors}/{BURST} requests failed to complete under load"),
                        );
                    }
                    if non_503 > 0 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "{non_503}/{BURST} requests did not return 503 — every request to the \
                                 permanent-503 backend must end in a 503 (backend 503 or budget-denial 503)"
                            ),
                        );
                    }

                    // Allow a brief moment for the counter to settle, then read.
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    let denied_after = match denied_total(&ctx).await {
                        Ok(value) => value,
                        Err(error) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("afterward metrics observation failed: {error}"),
                            )
                        }
                    };
                    let delta = match crate::metrics_helper::checked_counter_delta(denied_before, denied_after) {
                        Ok(delta) => delta,
                        Err(error) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("edgion_gateway_retry_budget_denied_total delta failed: {error}"),
                            )
                        }
                    };

                    // Soft band: with 60 always-503 requests and a 20% ceiling +
                    // 5/1s floor, most first-retry decisions are refused, so the
                    // counter must climb by well more than a trickle. A generous
                    // lower bound (8) keeps this robust against the soft cap's
                    // over-admission slack while still proving the budget engaged.
                    if delta < 8.0 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "expected the retry-budget denial counter to advance substantially under load, \
                                 but edgion_gateway_retry_budget_denied_total only moved by {delta} \
                                 (before={denied_before}, after={denied_after}) — the percentage cap did not engage"
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "all {BURST} requests returned 503; budget refused ~{delta} retries \
                             (denied_total {denied_before} -> {denied_after})"
                        ),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for RetryBudgetTestSuite {
    fn name(&self) -> &str {
        "EdgionBackendTrafficPolicy/RetryBudget"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionBackendTrafficPolicy/RetryBudget")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            // Floor test runs first, while the sliding windows are still empty.
            Self::test_floor_permits_low_volume_retry(),
            Self::test_budget_denies_under_load(),
        ]
    }
}
