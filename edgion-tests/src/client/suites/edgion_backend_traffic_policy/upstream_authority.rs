// EdgionBackendTrafficPolicy/UpstreamAuthority scenario.
//
// End-to-end coverage for `EdgionBackendTrafficPolicy.spec.upstreamAuthority`
// (see docs/superpowers/specs/2026-07-14-dynamic-upstream-authority-design.md),
// against the fixtures in
// edgion-tests/integration/conf/EdgionBackendTrafficPolicy/UpstreamAuthority/:
//
//   Gateway ua-gateway (port from ports.json, listener "http")
//     hostname "upstream-authority.example.com"
//     /tls      -> ua-svc-tls    (HTTPS wildcard backend, port 30028, cert SAN "*.sandbox.internal")
//     /plain    -> ua-svc-plain  (cleartext echo backend, port 30001, shared test_server listener)
//     /outbound -> ua-svc-plain, fronted by ForwardAuth (same backend, pre-check call)
//
// Both Services carry `upstreamAuthority { pattern: "*.sandbox.internal",
// template: "${ctx:tenant}.sandbox.internal" }`; `ctx:tenant` is set per
// request by the CtxSet plugin from the `X-Tenant` header. ua-svc-plain also
// carries an active HTTP health check with no explicit `host` override, so a
// passing probe is only possible because of `healthCheckHost` (Task 8).
//
// Coverage map to the design doc's §9 "Integration" list / task-8-brief.md §9:
//   1. Standard path, HTTPS               -> test_https_two_tenant_dispatch
//   2. Standard path, fail closed         -> test_fail_closed_render_empty
//   3. Standard path, cleartext           -> test_cleartext_dispatch
//   4. Injection                          -> test_injection_rejected
//   5. Outbound path                      -> test_outbound_forwardauth_per_tenant_and_degradation
//   6. Pinning                            -> test_pinning_destination_is_fixed_endpoint
//   7. Health                             -> test_health_probe_keeps_endpoint_alive
//
// NOTE ON SCOPE: the Pingora active-health-check probe
// (edgion-gateway/src/backends/health/check/) has no TLS support at all today
// (it always dials plain `http://`), independent of Task 8. So case 7 is
// exercised against the cleartext Service (ua-svc-plain) rather than the TLS
// one — the fix under test (the `healthCheckHost` override reaching the
// probe's `Host` header) is fully exercised either way; only the "and TLS/SNI
// too" half of the brief's phrasing does not apply to this probe system as it
// exists in the codebase today. The *outbound* path's own active-health probe
// (ExternalHttpConnection.healthCheck.active, which does support TLS) gets
// the same override — that is covered by the unit tests in
// edgion-gateway/src/plugins/http/common/outbound/client.rs
// (`resolve_probe_base_uses_authority_health_check_host` and
// `resolve_probe_base_unchanged_without_authority`), not duplicated here.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::{Duration, Instant};

pub struct UpstreamAuthorityTestSuite;

const HOST: &str = "upstream-authority.example.com";

/// Fetch the current value of `edgion_gateway_upstream_authority_rejected_total{reason="<reason>"}`
/// from the Gateway's Prometheus endpoint (fixed port 12090, matching the
/// pattern already used by the LB suites — NOT `ctx.admin_port`, which is the
/// *Controller's* Admin API port). Returns 0.0 if the metric/reason has never
/// been recorded yet (metrics crate does not pre-register label combinations).
async fn rejected_total(ctx: &TestContext, reason: &str) -> f64 {
    let metrics_client = crate::metrics_helper::MetricsClient::from_host_port(&ctx.target_host, 12090);
    let Ok(raw) = metrics_client.fetch_raw_metrics().await else {
        return 0.0;
    };
    let needle = format!("reason=\"{}\"", reason);
    for line in raw.lines() {
        if line.starts_with("edgion_gateway_upstream_authority_rejected_total") && line.contains(&needle) {
            if let Some(value) = line.split_whitespace().last() {
                return value.parse().unwrap_or(0.0);
            }
        }
    }
    0.0
}

/// Extract a header value from the plain-text response body of
/// `test_server.rs`'s `catch_all_handler` (which every request in this suite
/// hits — none of `/tls/echo`, `/plain/echo`, `/outbound/echo` match a
/// literal route, so they fall through to the `/{*path}` catch-all). Header
/// names there are title-cased via `display_header_name` (e.g. "Host"), so
/// the match is case-insensitive on both the requested name and the line.
fn extract_echoed_header(body: &str, header: &str) -> Option<String> {
    let header_lower = header.to_ascii_lowercase();
    body.lines().find_map(|line| {
        let trimmed = line.trim_start();
        let (name, value) = trimmed.split_once(':')?;
        if name.trim().to_ascii_lowercase() == header_lower {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

impl UpstreamAuthorityTestSuite {
    /// (1) Standard path, HTTPS: two tenants reach their own vhost on the
    /// wildcard-cert backend. A successful TLS handshake at all already
    /// proves the SNI was inside "*.sandbox.internal" (VhostWildcardResolver
    /// rejects everything else, including the bare parent domain per RFC
    /// 6125); the echoed `Host` header proves the EXACT per-tenant value.
    fn test_https_two_tenant_dispatch() -> TestCase {
        TestCase::new(
            "https_two_tenant_dispatch",
            "Two tenants over HTTPS reach their own vhost on the wildcard-cert backend",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/tls/echo", ctx.http_url());

                    for tenant in ["acme", "globex"] {
                        let resp = match ctx
                            .http_client
                            .get(&url)
                            .header("Host", HOST)
                            .header("X-Tenant", tenant)
                            .send()
                            .await
                        {
                            Ok(r) => r,
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("tenant {tenant}: request failed: {e}"),
                                )
                            }
                        };
                        let status = resp.status();
                        let body = resp.text().await.unwrap_or_default();
                        if status != 200 {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("tenant {tenant}: expected 200, got {status}, body: {body}"),
                            );
                        }
                        let expected_host = format!("{tenant}.sandbox.internal");
                        let got_host = extract_echoed_header(&body, "host");
                        if got_host.as_deref() != Some(expected_host.as_str()) {
                            return TestResult::failed(
                                start.elapsed(),
                                format!(
                                    "tenant {tenant}: expected upstream Host {expected_host:?}, got {got_host:?}; body: {body}"
                                ),
                            );
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "acme and globex each reached their own vhost over HTTPS".to_string(),
                    )
                })
            },
        )
    }

    /// (2) Standard path, fail closed: no `X-Tenant` header means `ctx:tenant`
    /// was never set, so the label renders empty. Must be a hard 502, and the
    /// `render_empty` rejection metric must increment — never a silent
    /// fallback that would reach the backend with some other authority.
    fn test_fail_closed_render_empty() -> TestCase {
        TestCase::new(
            "fail_closed_render_empty",
            "Unset ctx:tenant yields 502 and increments the render_empty rejection metric",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let before = rejected_total(&ctx, "render_empty").await;

                    let url = format!("{}/tls/echo", ctx.http_url());
                    let resp = match ctx.http_client.get(&url).header("Host", HOST).send().await {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("request failed: {e}")),
                    };
                    let status = resp.status();
                    if status != 502 {
                        let body = resp.text().await.unwrap_or_default();
                        return TestResult::failed(
                            start.elapsed(),
                            format!("expected 502 for unset ctx:tenant, got {status}, body: {body}"),
                        );
                    }

                    let after = rejected_total(&ctx, "render_empty").await;
                    if after <= before {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "expected render_empty rejection metric to increment, before={before}, after={after}"
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!("502 + render_empty metric {before} -> {after}"),
                    )
                })
            },
        )
    }

    /// (3) Standard path, cleartext: same per-tenant Host dispatch, no TLS/SNI.
    fn test_cleartext_dispatch() -> TestCase {
        TestCase::new(
            "cleartext_dispatch",
            "A tenant reaches its own vhost by Host header alone over cleartext",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/plain/echo", ctx.http_url());

                    let resp = match ctx
                        .http_client
                        .get(&url)
                        .header("Host", HOST)
                        .header("X-Tenant", "acme")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("request failed: {e}")),
                    };
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    if status != 200 {
                        return TestResult::failed(start.elapsed(), format!("expected 200, got {status}: {body}"));
                    }
                    let got_host = extract_echoed_header(&body, "host");
                    if got_host.as_deref() != Some("acme.sandbox.internal") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("expected upstream Host acme.sandbox.internal, got {got_host:?}; body: {body}"),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "cleartext dispatch reached acme's vhost".to_string(),
                    )
                })
            },
        )
    }

    /// (4) Injection: a `ctx:tenant` containing a literal `.` or `@` cannot
    /// compose into a second label or escape the host — §5.4's label
    /// validation rejects it before any dial happens.
    fn test_injection_rejected() -> TestCase {
        TestCase::new(
            "injection_rejected",
            "ctx:tenant values with '.' or '@' are rejected (invalid_label), never dialed",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let before = rejected_total(&ctx, "invalid_label").await;

                    let url = format!("{}/tls/echo", ctx.http_url());
                    for payload in ["evil.com", "a@evil.com"] {
                        let resp = match ctx
                            .http_client
                            .get(&url)
                            .header("Host", HOST)
                            .header("X-Tenant", payload)
                            .send()
                            .await
                        {
                            Ok(r) => r,
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("payload {payload:?}: request failed: {e}"),
                                )
                            }
                        };
                        let status = resp.status();
                        if status != 502 {
                            let body = resp.text().await.unwrap_or_default();
                            return TestResult::failed(
                                start.elapsed(),
                                format!("payload {payload:?}: expected 502, got {status}: {body}"),
                            );
                        }
                    }

                    let after = rejected_total(&ctx, "invalid_label").await;
                    if after < before + 2.0 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "expected invalid_label metric to increment by >=2 (one per payload), before={before}, after={after}"
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!("both injection payloads rejected with 502; invalid_label {before} -> {after}"),
                    )
                })
            },
        )
    }

    /// (5) Outbound path: ForwardAuth anchored on ua-svc-plain dispatches its
    /// own pre-check call per tenant (same upstreamAuthority policy as the
    /// real request), and degrades per `statusOnError` (503, distinct from
    /// the standard path's 502) when the label is rejected.
    fn test_outbound_forwardauth_per_tenant_and_degradation() -> TestCase {
        TestCase::new(
            "outbound_forwardauth_per_tenant_and_degradation",
            "ForwardAuth's own outbound call dispatches per tenant, and returns statusOnError on a rejected label",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/outbound/echo", ctx.http_url());

                    // Happy path: the real backend call proves the vhost the
                    // MAIN request reached; a 200 also implies ForwardAuth's
                    // own pre-check call to the same Service succeeded (a
                    // failed/rejected pre-check would short-circuit to
                    // statusOnError before the real backend is ever called).
                    let resp = match ctx
                        .http_client
                        .get(&url)
                        .header("Host", HOST)
                        .header("X-Tenant", "acme")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("happy-path request failed: {e}"))
                        }
                    };
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    if status != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("happy path: expected 200, got {status}: {body}"),
                        );
                    }
                    let got_host = extract_echoed_header(&body, "host");
                    if got_host.as_deref() != Some("acme.sandbox.internal") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "happy path: expected real backend Host acme.sandbox.internal, got {got_host:?}; body: {body}"
                            ),
                        );
                    }

                    // Degradation path: an injected label rejects ForwardAuth's
                    // own outbound_call with AuthorityRejected; ForwardAuth is
                    // unconditionally fail-close, so the plugin returns
                    // statusOnError (503).
                    let resp = match ctx
                        .http_client
                        .get(&url)
                        .header("Host", HOST)
                        .header("X-Tenant", "a@evil.com")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("degradation-path request failed: {e}"))
                        }
                    };
                    let status = resp.status();
                    if status != 503 {
                        let body = resp.text().await.unwrap_or_default();
                        return TestResult::failed(
                            start.elapsed(),
                            format!("degradation path: expected statusOnError=503, got {status}: {body}"),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "outbound per-tenant dispatch ok; rejected label degraded to statusOnError=503".to_string(),
                    )
                })
            },
        )
    }

    /// (6) Pinning: the destination address is the Service endpoint
    /// regardless of the composed authority. None of the tenant labels used
    /// across this suite (`acme.sandbox.internal`, `globex.sandbox.internal`,
    /// `probe.sandbox.internal`, ...) are real, resolvable DNS names in the
    /// test environment — the ONLY way any of them can succeed at all is that
    /// the composed authority is never resolved and the connection stays
    /// pinned to ua-svc-tls's single EndpointSlice entry (127.0.0.1:30028).
    /// This case drives several distinct tenants back-to-back and asserts
    /// every one reaches the same (only) endpoint successfully.
    fn test_pinning_destination_is_fixed_endpoint() -> TestCase {
        TestCase::new(
            "pinning_destination_is_fixed_endpoint",
            "Distinct tenant authorities never get DNS-resolved; every one reaches the pinned endpoint",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/tls/echo", ctx.http_url());

                    for tenant in ["acme", "globex", "initech", "hooli"] {
                        let resp = match ctx
                            .http_client
                            .get(&url)
                            .header("Host", HOST)
                            .header("X-Tenant", tenant)
                            .timeout(Duration::from_secs(3))
                            .send()
                            .await
                        {
                            Ok(r) => r,
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "tenant {tenant}: request failed (a real DNS lookup of \
                                         {tenant}.sandbox.internal would fail/timeout here): {e}"
                                    ),
                                )
                            }
                        };
                        if resp.status() != 200 {
                            return TestResult::failed(
                                start.elapsed(),
                                format!(
                                    "tenant {tenant}: expected 200 from the pinned endpoint, got {}",
                                    resp.status()
                                ),
                            );
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "4 distinct tenant authorities all reached the pinned endpoint".to_string(),
                    )
                })
            },
        )
    }

    /// (7) Health: with `healthCheckHost` set, the Pingora active-health
    /// probe reaches `/vhost-health` with `Host: probe.sandbox.internal`
    /// (see test_server.rs `vhost_health_handler`), so the probe passes and
    /// the Service's single endpoint is never ejected. Proven indirectly:
    /// real traffic keeps succeeding across several health-check intervals.
    /// If `healthCheckHost` were not wired in (pre-Task-8 behavior), the
    /// probe would send the raw socket-address Host, `/vhost-health` would
    /// 421, and the sole endpoint would be ejected — this same request would
    /// then fail with "no ready backend" (502/503).
    fn test_health_probe_keeps_endpoint_alive() -> TestCase {
        TestCase::new(
            "health_probe_keeps_endpoint_alive",
            "healthCheckHost keeps the probe passing; the sole endpoint is never ejected",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/plain/echo", ctx.http_url());

                    // healthCheck.active.interval is 1s with healthy/unhealthyThreshold=1
                    // (see EdgionBackendTrafficPolicy_edgion-test_ua-authority-plain-policy.yaml),
                    // so a few seconds spans several probe cycles.
                    for _ in 0..6 {
                        let (status, body) = match ctx
                            .http_client
                            .get(&url)
                            .header("Host", HOST)
                            .header("X-Tenant", "acme")
                            .send()
                            .await
                        {
                            Ok(resp) => {
                                let status = resp.status().as_u16();
                                let body = resp.text().await.unwrap_or_default();
                                (status, body)
                            }
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("request failed while polling for endpoint liveness: {e}"),
                                )
                            }
                        };
                        if status != 200 {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("endpoint was ejected (probe must have failed): status={status}, body={body}"),
                            );
                        }
                        tokio::time::sleep(Duration::from_millis(700)).await;
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "endpoint stayed healthy across multiple probe cycles".to_string(),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for UpstreamAuthorityTestSuite {
    fn name(&self) -> &str {
        "EdgionBackendTrafficPolicy/UpstreamAuthority"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionBackendTrafficPolicy/UpstreamAuthority")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_https_two_tenant_dispatch(),
            Self::test_fail_closed_render_empty(),
            Self::test_cleartext_dispatch(),
            Self::test_injection_rejected(),
            Self::test_outbound_forwardauth_per_tenant_and_degradation(),
            Self::test_pinning_destination_is_fixed_endpoint(),
            Self::test_health_probe_keeps_endpoint_alive(),
        ]
    }
}
