// DnsResolution test suite
//
// End-to-end coverage of the gateway's ExternalName self-resolution paths
// (Part B data plane), driven hermetically via the test_server's fake HTTP DNS
// endpoint (`/dns/resolve`, `/dns/hits/{host}`). No real DNS is involved.
//
// Scenarios (all resolve to the local test backend 127.0.0.1:30001):
//   1. per-request  — `dns-resolution-mode: per-request`, system resolver,
//      externalName=localhost. Re-resolves every request (no cache).
//   2. httpdns      — `dns-resolver-ref` → httpdns LinkSys, per-request. Proves
//      the httpdns endpoint is hit and the resolved backend serves.
//   3. gateway      — `dns-resolution-mode: gateway`, httpdns resolver. Caches
//      (5s TTL): a second request within the TTL is served without re-resolving
//      (the httpdns hit-count does not increase).
//   4. httpdns-fallback — httpdns endpoint DOWN + `fallback: system`: resolution
//      falls back to the OS resolver and still serves.
//
// The controller-resolved path (`mode: controller` → real EndpointSlice) is
// K8s-conf_center-only and cannot run in this FileSystem-mode harness; it is
// covered by the C-1 unit tests in
// `edgion-controller/src/services/dns_resolver/`.
//
// Required config (edgion-tests/integration/conf/HTTPRoute/Backend/DnsResolution/):
//   - 00_LinkSys_*_dns-httpdns-local        # httpdns → /dns/resolve, returns 127.0.0.1
//   - 00_LinkSys_*_dns-httpdns-dead         # httpdns → down endpoint, fallback: system
//   - 01_Service_*_dns-{per-request,gateway-cached,httpdns,httpdns-fallback}
//   - 02_Gateway_edgion-test_dns-res-gateway  # HTTP listener on port 31360
//   - 03_HTTPRoute_*                          # hostname → backend Service
// Requires securityProtect.allowLoopbackUpstream=true (base EdgionGatewayConfig).

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

/// Local test backend that also serves the fake HTTP DNS endpoint.
const BACKEND_BASE: &str = "http://127.0.0.1:30001";

pub struct DnsResolutionTestSuite;

impl DnsResolutionTestSuite {
    /// Read the httpdns resolve hit-count for `host` from the test server.
    async fn dns_hits(client: &reqwest::Client, host: &str) -> Result<u64, String> {
        let url = format!("{BACKEND_BASE}/dns/hits/{host}");
        let resp = client
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| format!("GET {url} failed: {e}"))?;
        let json: serde_json::Value = resp.json().await.map_err(|e| format!("parse hits json failed: {e}"))?;
        json.get("hits")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| format!("missing hits in {json}"))
    }

    /// Send one request to the gateway with the given Host and return the status.
    async fn get_status(ctx: &TestContext, host: &str) -> Result<u16, String> {
        let url = format!("{}/echo", ctx.http_url());
        let resp = ctx
            .http_client
            .get(&url)
            .timeout(std::time::Duration::from_secs(10))
            .header("Host", host)
            .send()
            .await
            .map_err(|e| format!("request to {host} failed: {e}"))?;
        Ok(resp.status().as_u16())
    }

    /// per-request mode resolves via the system resolver (externalName=localhost)
    /// and serves from the local backend.
    fn test_per_request_resolves_and_serves() -> TestCase {
        TestCase::new(
            "per_request_resolves_and_serves",
            "per-request ExternalName resolves (system resolver) and serves 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match Self::get_status(&ctx, "per-request.dns.test").await {
                        Ok(200) => TestResult::passed_with_message(
                            start.elapsed(),
                            "per-request self-resolution reached the backend (200)".to_string(),
                        ),
                        Ok(other) => TestResult::failed(start.elapsed(), format!("expected 200, got {other}")),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// httpdns (per-request) resolves through the fake HTTP DNS endpoint (hit
    /// count increases) and serves.
    fn test_httpdns_resolves_and_serves() -> TestCase {
        TestCase::new(
            "httpdns_resolves_and_serves",
            "httpdns resolver hits the endpoint each request and serves 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let host = "httpdns.dns.test";

                    let before = match Self::dns_hits(&ctx.http_client, host).await {
                        Ok(n) => n,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };

                    for _ in 0..2 {
                        match Self::get_status(&ctx, host).await {
                            Ok(200) => {}
                            Ok(other) => {
                                return TestResult::failed(start.elapsed(), format!("expected 200, got {other}"))
                            }
                            Err(e) => return TestResult::failed(start.elapsed(), e),
                        }
                    }

                    let after = match Self::dns_hits(&ctx.http_client, host).await {
                        Ok(n) => n,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };

                    // per-request re-resolves every request → at least +2 hits.
                    if after >= before + 2 {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!("httpdns endpoint hit per request (before={before}, after={after})"),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("expected httpdns hits to grow by >=2, before={before}, after={after}"),
                        )
                    }
                })
            },
        )
    }

    /// gateway mode caches: a second request within the 5s TTL is served without
    /// a new httpdns resolution (hit count unchanged between the two requests).
    fn test_gateway_mode_caches_within_ttl() -> TestCase {
        TestCase::new(
            "gateway_mode_caches_within_ttl",
            "gateway mode serves a second in-TTL request from cache (no re-resolve)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    // Route hostname (Host header) and the resolver hit-count key differ:
                    // the httpdns endpoint counts by the `{domain}` it resolves, which the
                    // gateway fills with the Service's externalName, NOT the route hostname.
                    let route_host = "gateway-cached.dns.test";
                    let resolver_key = "gwcached.dns.test"; // == Service.spec.externalName

                    // First request resolves (cold cache) → hit count becomes >= 1.
                    if let Err(e) = Self::expect_ok(&ctx, route_host).await {
                        return TestResult::failed(start.elapsed(), e);
                    }
                    let hits_after_first = match Self::dns_hits(&ctx.http_client, resolver_key).await {
                        Ok(n) => n,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    // Guard against a vacuous 0 == 0 pass: gateway mode must have resolved
                    // at least once through the httpdns endpoint.
                    if hits_after_first < 1 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("expected the first request to resolve via httpdns, hits={hits_after_first}"),
                        );
                    }

                    // Second request immediately after (well within the 5s TTL).
                    if let Err(e) = Self::expect_ok(&ctx, route_host).await {
                        return TestResult::failed(start.elapsed(), e);
                    }
                    let hits_after_second = match Self::dns_hits(&ctx.http_client, resolver_key).await {
                        Ok(n) => n,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };

                    if hits_after_second == hits_after_first {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!("second in-TTL request served from cache (hits stayed at {hits_after_second})"),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "expected no re-resolve within TTL, hits went {hits_after_first} -> {hits_after_second}"
                            ),
                        )
                    }
                })
            },
        )
    }

    /// httpdns endpoint DOWN + fallback: system → still resolves and serves.
    fn test_httpdns_fallback_when_endpoint_down() -> TestCase {
        TestCase::new(
            "httpdns_fallback_when_endpoint_down",
            "httpdns endpoint down falls back to system resolver and serves 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match Self::get_status(&ctx, "httpdns-fallback.dns.test").await {
                        Ok(200) => TestResult::passed_with_message(
                            start.elapsed(),
                            "system fallback resolved the ExternalName and served (200)".to_string(),
                        ),
                        Ok(other) => {
                            TestResult::failed(start.elapsed(), format!("expected 200 via fallback, got {other}"))
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    async fn expect_ok(ctx: &TestContext, host: &str) -> Result<(), String> {
        match Self::get_status(ctx, host).await? {
            200 => Ok(()),
            other => Err(format!("expected 200 for {host}, got {other}")),
        }
    }
}

#[async_trait]
impl TestSuite for DnsResolutionTestSuite {
    fn name(&self) -> &str {
        "DnsResolution"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/DnsResolution")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_per_request_resolves_and_serves(),
            Self::test_httpdns_resolves_and_serves(),
            Self::test_gateway_mode_caches_within_ttl(),
            Self::test_httpdns_fallback_when_endpoint_down(),
        ]
    }
}
