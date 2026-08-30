// DnsResolutionFlap dynamic phase suites.
//
// Backing suites for the `DnsResolutionFlap` dynamic scenario, which flips a
// Service's `edgion.io/dns-resolver-ref` annotation at runtime and asserts the
// gateway picks up the change without a restart:
//   - failing: resolver-ref → a DOWN httpdns LinkSys with `fallback: none`, so
//     per-request resolution fails and the backend returns 5xx.
//   - working: resolver-ref → a live httpdns LinkSys returning 127.0.0.1, so the
//     backend resolves and serves 200.
//
// Both phases target the flap scenario's own Gateway (port 31361); the request
// URL is hardcoded because dynamic phase suites run with the shared "default"
// TestContext port rather than a per-suite port_key.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::{Duration, Instant};

const FLAP_GATEWAY: &str = "http://127.0.0.1:31361";
const FLAP_HOST: &str = "flap.dns.test";
/// The ExternalName of the flap Service (the `{domain}` the httpdns endpoint
/// counts resolutions for).
const FLAP_EXTERNAL_NAME: &str = "flapbackend.test";
/// Local test backend serving the fake HTTP DNS endpoint (`/dns/hits`).
const BACKEND_BASE: &str = "http://127.0.0.1:30001";

/// GET the flap route and return the HTTP status.
async fn flap_status(ctx: &TestContext) -> Result<u16, String> {
    let resp = ctx
        .http_client
        .get(format!("{FLAP_GATEWAY}/echo"))
        .timeout(Duration::from_secs(10))
        .header("Host", FLAP_HOST)
        .send()
        .await
        .map_err(|e| format!("request to {FLAP_HOST} failed: {e}"))?;
    Ok(resp.status().as_u16())
}

/// Read how many times the live httpdns endpoint resolved `FLAP_EXTERNAL_NAME`.
async fn live_resolver_hits(ctx: &TestContext) -> Result<u64, String> {
    let resp = ctx
        .http_client
        .get(format!("{BACKEND_BASE}/dns/hits/{FLAP_EXTERNAL_NAME}"))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("GET /dns/hits failed: {e}"))?;
    let json: serde_json::Value = resp.json().await.map_err(|e| format!("parse hits json failed: {e}"))?;
    json.get("hits")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| format!("missing hits in {json}"))
}

/// Phase "failing": dead httpdns resolver + `fallback: none` → 5xx.
pub struct DnsFlapFailingTestSuite;

impl DnsFlapFailingTestSuite {
    fn test_resolution_fails() -> TestCase {
        TestCase::new(
            "resolution_fails",
            "dead httpdns resolver with fallback:none returns 5xx",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match flap_status(&ctx).await {
                        Ok(s) if s >= 500 => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("resolution failed as expected ({s})"),
                        ),
                        Ok(s) => TestResult::failed(start.elapsed(), format!("expected 5xx, got {s}")),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for DnsFlapFailingTestSuite {
    fn name(&self) -> &str {
        "DnsFlapFailing"
    }

    fn port_key(&self) -> Option<&str> {
        None
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_resolution_fails()]
    }
}

/// Phase "working": live httpdns resolver → 200.
pub struct DnsFlapWorkingTestSuite;

impl DnsFlapWorkingTestSuite {
    /// After the resolver-ref flips to the live httpdns LinkSys, a request must
    /// route through that endpoint — proven by its resolution hit-count growing.
    ///
    /// Asserts on the resolver hit-count rather than a 200 so the check is
    /// independent of the upstream loopback SSRF gate (dynamic scenarios run in a
    /// multi-suite context where `allowLoopbackUpstream` may be off; resolution
    /// still runs and the endpoint is still hit even if the upstream connect is
    /// then refused).
    fn test_resolver_switched_live() -> TestCase {
        TestCase::new(
            "resolver_switched_live",
            "after hot-reloading resolver-ref, requests route through the live httpdns endpoint",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let before = match live_resolver_hits(&ctx).await {
                        Ok(n) => n,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    if let Err(e) = flap_status(&ctx).await {
                        return TestResult::failed(start.elapsed(), e);
                    }
                    let after = match live_resolver_hits(&ctx).await {
                        Ok(n) => n,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    if after > before {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!("resolver-ref hot-reload routed to the live httpdns endpoint (hits {before} -> {after})"),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("live httpdns endpoint not hit after hot-reload (hits stayed at {before})"),
                        )
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for DnsFlapWorkingTestSuite {
    fn name(&self) -> &str {
        "DnsFlapWorking"
    }

    fn port_key(&self) -> Option<&str> {
        None
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_resolver_switched_live()]
    }
}
