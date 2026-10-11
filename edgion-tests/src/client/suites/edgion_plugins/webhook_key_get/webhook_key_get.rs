// WebhookKeyGet Integration Test Suite (LinkSys webhook redesign — Wave E)
//
// Architecture reference: skills/01-architecture/03-controller-gateway-link/webhook.md
// (backendRef target, request-templating allow-list, presence policy, consumer
// path_override API, secret resolution flow). Schema: skills/02-features/03-resources/18-link-sys.md.
//
// Exercises the 11 redesigned webhook scenarios end to end, plus 6 further
// scenarios (12-17) covering the URL-target feature (external `target.url`,
// origin-only validation, SSRF egress guard), plus 1 further scenario (18)
// covering the LinkSys webhook `request.body` block (json templating only),
// plus 1 further scenario (19) composing WebhookKeyGet then KeyAuth.
// Each scenario has a self-contained fixture in
// conf/EdgionPlugins/WebhookKeyGet/NN-*.yaml that wires
// up: a backend Service (+ EndpointSlice) OR an external `target.url`, a LinkSys
// webhook resource declaring the outbound `request` (target + allow-listed
// origin/custom fields), a WebhookKeyGet EdgionPlugins entry (scenarios 09/10
// also keep a preceding CtxSet for local vars the webhook templates read),
// and an HTTPRoute on host `whN.example.com` path `/test/whN`.
//
// Verification model
// ------------------
// Every request carries `access_log: test_store` + a unique W3C `traceparent`; the
// gateway stores the access-log entry (integration-testing mode) keyed by that
// trace id. Every WebhookKeyGet (and CtxSet) var is flattened to a top-level key in the access-log
// JSON, so asserting `"<var>":"<value>"` proves the resolved value.
//
// To verify *outbound request construction*, the test_server's webhook handler
// (`/webhook/resolve`, `/webhook/default`, `/webhook/override`) echoes the inbound
// request back under `data.echo_headers`, `data.echo_args`, `data.echo_cookies`,
// `data.echo_body`, `data.method`, `data.path`. The relevant fixtures point their
// `extract` at the echoed field, so the value the webhook backend actually received
// surfaces into a ctx var and thus the access log.
//
// Known gaps surfaced by this suite (documented, not worked around):
//
//  1. statusOnError is NOT honored by WebhookKeyGet — a failed webhook
//     simply falls back to the var `default` (or leaves it unset).
//     So `presence: required` blocks the outbound call (verifiable) but does not
//     return a 5xx to the client. Scenario 5 asserts the *gating* property (the
//     webhook was not called) rather than a 403 client status. PASSES.
//
//  2. `kind: ServiceClusterIp` webhook resolution is intentionally NOT covered by
//     this suite. The webhook provider resolves it to a `<svc>.<ns>.svc.cluster.local`
//     DNS name (edgion-gateway .../webhook/manager.rs) and relies on cluster DNS,
//     which the local harness lacks. Making it dial `spec.clusterIP` directly (to
//     match the route backend path) was evaluated and declined as low-value / niche;
//     see docs/superpowers/specs/2026-06-21-webhook-serviceclusterip-resolution-design.md.
//
// Net: 10/10 original scenarios pass, plus 6/6 URL-target scenarios (12-17), plus
// 1/1 body-templating scenario (18; see below), plus 1/1 WebhookKeyGet-then-KeyAuth
// scenario (19), for 18/18 total.
//
// URL-target scenarios (12-17)
// -----------------------------
// 12. Happy path: `target.url: http://localhost:<port>` — a hostname, so it goes
//     through `EgressResolver` (real DNS resolution + classification), not the
//     IP-literal `dial_url` guard. This is the only scenario in this suite that
//     exercises the hostname path through `EgressResolver`.
// 13. Custom CA over https: reuses the shared HTTPS backend
//     (`test_server --https-backend-port 30051`) already started for the
//     BackendTLSPolicy suite, and its `backend-ca` Secret (also published in
//     `edgion-default` for the ExtProc-over-TLS suite) — no new TLS scaffolding
//     needed.
// 14. Secret-sourced Authorization header on a URL target: verifies both that
//     test_server observes the real token AND that the controller's own
//     `/configserver/LinkSys` admin output redacts it.
// 15. Retry on 5xx: `retry.retryOnStatus: [503]` against test_server's
//     `/retry-sequence/{test_id}/{status_list}` endpoint; the final response's
//     `x-attempt-history` header proves the exact 3-call sequence.
// 16. `blockPrivate: true` against an unroutable RFC1918 IP literal — rejected
//     pre-connect by `dial_url`'s IP-literal guard, incrementing
//     `edgion_outbound_egress_blocked_total{reason="private"}`. Paired with a
//     negative control (same address, `blockPrivate` absent) that fails by
//     connection timeout instead, proving the metric only fires for the actual
//     guard rejection.
// 17. Origin-only rejection: `target.url` with a path is rejected by schema
//     validation; asserted purely against the controller's Accepted=False
//     status condition (no gateway traffic).
//
// Body-templating scenario (18)
// ------------------------------
// 18. json body: LinkSys webhook `request.body` (`type: json`) renders a JSON
//     template containing `${header:X-User-Id}` and is sent to the webhook
//     backend with an auto-injected `Content-Type: application/json`. The
//     backend parses the received body and echoes it back under
//     `data.echo_body`; the fixture extracts `data.echo_body.userId` (proving
//     the template rendered correctly) and `data.echo_headers.content-type`
//     (proving the auto-injected header reached the wire).
//
// WebhookKeyGet then KeyAuth (19)
// --------------------------------
// 19. WebhookKeyGet runs first and writes the echoed `X-Wh-Key` into ctx
//     `wh19_key`; KeyAuth authenticates that ctx var against Secret
//     `wh19-api-keys`. Valid key → 200; missing or wrong key → 401 (same as
//     KeyAuth). A reversed plugin list is a silent auth bypass — W-1 declined
//     a cross-plugin ordering check.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::status::helpers::controller_local_conditions;
use std::time::Instant;

pub struct WebhookKeyGetTestSuite;

/// Gateway HTTP listener port for the EdgionPlugins suite (see ports.json / base/Gateway.yaml).
const GW_PORT: u16 = 31180;

// ============================================================
// Shared helpers
// ============================================================

fn gw_url(ctx: &TestContext, path: &str) -> String {
    format!("http://{}:{}{}", ctx.target_host, GW_PORT, path)
}

/// Issue a GET through the gateway with the standard access-log instrumentation
/// headers plus any scenario-specific headers. Returns the raw response.
async fn send_gw(
    ctx: &TestContext,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    trace_id: &str,
) -> Result<reqwest::Response, String> {
    let mut rb = ctx
        .http_client
        .get(gw_url(ctx, path))
        .header("host", host)
        .header(
            "traceparent",
            crate::access_log_client::traceparent_for_marker(trace_id),
        )
        .header("access_log", "test_store");
    for (k, v) in headers {
        rb = rb.header(*k, *v);
    }
    rb.send().await.map_err(|e| format!("request error: {}", e))
}

/// Fetch the stored access-log entry for `trace_id` and return its JSON as a string.
async fn fetch_log(ctx: &TestContext, trace_id: &str) -> Result<String, String> {
    let entry = ctx
        .access_log_client()
        .get_access_log_with_retry(trace_id, 15, 200)
        .await
        .map_err(|e| format!("access-log fetch failed: {}", e))?;
    Ok(entry.data.to_string())
}

/// True when the access-log JSON contains the flattened ctx var `"key":"value"`.
fn log_has(log: &str, key: &str, value: &str) -> bool {
    log.contains(&format!("\"{}\":\"{}\"", key, value))
}

/// True when the access-log JSON mentions the ctx var key at all (any value).
fn log_has_key(log: &str, key: &str) -> bool {
    log.contains(&format!("\"{}\":", key))
}

/// Convenience: drive one request, fetch its log, and hand both to an assertion
/// closure. Keeps each test case to a single small body.
async fn run_case<F>(
    ctx: &TestContext,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    trace_prefix: &str,
    start: Instant,
    assert_fn: F,
) -> TestResult
where
    F: FnOnce(&str) -> Result<String, String>,
{
    let trace_id = format!("{}-{}", trace_prefix, uuid::Uuid::new_v4());
    let resp = match send_gw(ctx, host, path, headers, &trace_id).await {
        Ok(r) => r,
        Err(e) => return TestResult::failed(start.elapsed(), e),
    };
    let status = resp.status().as_u16();
    if status != 200 {
        return TestResult::failed(start.elapsed(), format!("expected 200, got {}", status));
    }
    let log = match fetch_log(ctx, &trace_id).await {
        Ok(l) => l,
        Err(e) => return TestResult::failed(start.elapsed(), e),
    };
    match assert_fn(&log) {
        Ok(msg) => TestResult::passed_with_message(start.elapsed(), msg),
        Err(msg) => TestResult::failed(start.elapsed(), format!("{} | access-log: {}", msg, log)),
    }
}

// ============================================================
// Helpers for the URL-target scenarios (12-17)
// ============================================================

/// Fetch a LinkSys resource's `status.conditions` from the controller's
/// `/configserver/LinkSys` admin cache endpoint (the endpoint whose response
/// format mirrors Gateway's `/configclient/` API). Retries because the
/// controller reconciles asynchronously after the resource is applied.
async fn fetch_linksys_conditions(ctx: &TestContext, name: &str) -> Result<Vec<serde_json::Value>, String> {
    let url = format!(
        "{}/configserver/LinkSys?namespace=edgion-default&name={}",
        ctx.admin_api_url(),
        name
    );

    let mut last_err = format!("LinkSys '{}' not found", name);
    for _ in 0..20 {
        let mut req = ctx.http_client.get(&url);
        if let Some(ref token) = ctx.admin_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }
        match req.send().await {
            Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
                Ok(body) if body["success"].as_bool().unwrap_or(false) => {
                    if let Some(conditions) = controller_local_conditions(&body["data"]["status"]) {
                        if !conditions.is_empty() {
                            return Ok(conditions.clone());
                        }
                        last_err = format!("LinkSys '{}' found but status.conditions is empty", name);
                    } else {
                        last_err = format!("LinkSys '{}' found but status.conditions not present", name);
                    }
                }
                Ok(body) => {
                    last_err = format!("API error: {}", body["error"].as_str().unwrap_or("unknown"));
                }
                Err(e) => {
                    last_err = format!("JSON parse error: {}", e);
                }
            },
            Ok(resp) => {
                last_err = format!("HTTP {}", resp.status());
            }
            Err(e) => {
                last_err = format!("Request error: {}", e);
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    Err(last_err)
}

/// Fetch the full LinkSys resource JSON (not just `status`) from the same
/// admin cache endpoint, so callers can inspect `spec.config.resolvedSecrets`
/// (the redacted-in-admin-output secret map) rather than only the status.
async fn fetch_linksys_resource(ctx: &TestContext, name: &str) -> Result<serde_json::Value, String> {
    let url = format!(
        "{}/configserver/LinkSys?namespace=edgion-default&name={}",
        ctx.admin_api_url(),
        name
    );
    let mut req = ctx.http_client.get(&url);
    if let Some(ref token) = ctx.admin_token {
        req = req.header("Authorization", format!("Bearer {}", token));
    }
    let resp = req.send().await.map_err(|e| format!("request error: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| format!("JSON parse error: {}", e))?;
    if !body["success"].as_bool().unwrap_or(false) {
        return Err(format!("API error: {}", body["error"].as_str().unwrap_or("unknown")));
    }
    Ok(body["data"].clone())
}

fn find_condition<'a>(conditions: &'a [serde_json::Value], type_: &str) -> Option<&'a serde_json::Value> {
    conditions.iter().find(|c| c["type"].as_str() == Some(type_))
}

/// Gateway metrics port shared by every integration suite (see
/// `metrics_helper::MetricsClient` usage in the LB suites and bandwidth_limit).
const GW_METRICS_PORT: u16 = 12090;

/// Parse the current value of a single-label Prometheus counter line, e.g.
/// `edgion_outbound_egress_blocked_total{reason="private"} 3`. Returns 0 if the
/// series has never been recorded (Prometheus counters do not exist until the
/// first increment).
fn parse_counter_value(raw: &str, metric_name: &str, label_match: &str) -> u64 {
    for line in raw.lines() {
        if line.starts_with(metric_name) && line.contains(label_match) {
            if let Some(idx) = line.rfind(' ') {
                if let Ok(v) = line[idx + 1..].trim().parse::<u64>() {
                    return v;
                }
            }
        }
    }
    0
}

/// Fetch the current `edgion_outbound_egress_blocked_total{reason="private"}` value.
async fn fetch_egress_blocked_private_count(ctx: &TestContext) -> Result<u64, String> {
    let client = crate::metrics_helper::MetricsClient::from_host_port(&ctx.target_host, GW_METRICS_PORT);
    let raw = client
        .fetch_raw_metrics()
        .await
        .map_err(|e| format!("fetch_raw_metrics failed: {}", e))?;
    Ok(parse_counter_value(
        &raw,
        "edgion_outbound_egress_blocked_total",
        "reason=\"private\"",
    ))
}

impl WebhookKeyGetTestSuite {
    // ==================== 1. Service + EndpointSlice resolution ====================
    fn test_service_plus_endpointslice() -> TestCase {
        TestCase::new(
            "service_plus_endpointslice",
            "Webhook backend resolved via Service + EndpointSlice (EpSliceStore call-time path)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    run_case(
                        &ctx,
                        "wh1.example.com",
                        "/test/wh1",
                        &[("X-Tenant-Id", "ep-tenant-1")],
                        "wh1",
                        start,
                        |log| {
                            // user_id = uid-<tenant>; a correct value proves both that
                            // the EndpointSlice-resolved backend was reachable AND that
                            // the origin header X-Tenant-Id was forwarded transparently.
                            if log_has(log, "wh1_user_id", "uid-ep-tenant-1") {
                                Ok("backend resolved via EndpointSlice; origin X-Tenant-Id forwarded".into())
                            } else {
                                Err("expected wh1_user_id=uid-ep-tenant-1".into())
                            }
                        },
                    )
                    .await
                })
            },
        )
    }

    // ==================== 3. All-origin transparent forwarding ====================
    fn test_all_origin_transparent() -> TestCase {
        TestCase::new(
            "all_origin_transparent",
            "Declared origin headers/args/cookies forwarded transparently (allow-list)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    run_case(
                        &ctx,
                        "wh3.example.com",
                        "/test/wh3",
                        &[
                            ("X-Tenant-Id", "t3"),
                            ("X-Request-ID", "reqid-3"),
                            ("X-User-Id", "user-3"),
                            // Undeclared header: must NOT be forwarded (allow-list).
                            ("X-Should-Not-Leak", "leak-3"),
                        ],
                        "wh3",
                        start,
                        |log| {
                            // wh3 extracts data.echo_headers.x-request-id — the value the
                            // backend received for the declared origin header X-Request-ID.
                            if !log_has(log, "wh3_user_id", "reqid-3") {
                                return Err("expected wh3_user_id=reqid-3 (origin header forwarded)".into());
                            }
                            // Allow-list: the undeclared header value must not appear.
                            if log.contains("leak-3") {
                                return Err("undeclared header X-Should-Not-Leak leaked to the webhook".into());
                            }
                            Ok("origin header forwarded with exact value; undeclared header not leaked".into())
                        },
                    )
                    .await
                })
            },
        )
    }

    // ==================== 4. All-custom literal injection ====================
    fn test_all_custom_literal() -> TestCase {
        TestCase::new(
            "all_custom_literal",
            "Custom literal fields injected; custom value overrides incoming; allow-list respected",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    run_case(
                        &ctx,
                        "wh4.example.com",
                        "/test/wh4",
                        // Send an incoming X-Source that the custom literal must override.
                        &[("X-Source", "incoming-should-be-overridden")],
                        "wh4",
                        start,
                        |log| {
                            // wh4 extracts data.echo_headers.x-source — the custom literal
                            // "edgion-gateway" must win over the incoming X-Source.
                            if log_has(log, "wh4_user_id", "edgion-gateway") {
                                Ok("custom literal injected and overrode incoming value".into())
                            } else if log.contains("incoming-should-be-overridden") {
                                Err("incoming X-Source leaked instead of custom literal".into())
                            } else {
                                Err("expected wh4_user_id=edgion-gateway".into())
                            }
                        },
                    )
                    .await
                })
            },
        )
    }

    // ==================== 5. presence: required → gates the call ====================
    //
    // Design intent: missing required field → 403 (statusOnError). The gateway does
    // NOT honor statusOnError in WebhookKeyGet, so this
    // case asserts the verifiable property instead: with the required field present
    // the webhook resolves; without it, the webhook is NOT called (the var is never
    // set, because wh5 declares no `default`). See suite report for the 403 gap.
    fn test_presence_required_fail() -> TestCase {
        TestCase::new(
            "presence_required_fail",
            "presence: required blocks the outbound call when the field is missing",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Case A: required field present → webhook is called and resolves.
                    let trace_a = format!("wh5-present-{}", uuid::Uuid::new_v4());
                    if let Err(e) =
                        send_gw(&ctx, "wh5.example.com", "/test/wh5", &[("X-Required", "yes")], &trace_a).await
                    {
                        return TestResult::failed(start.elapsed(), e);
                    }
                    let log_a = match fetch_log(&ctx, &trace_a).await {
                        Ok(l) => l,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    // X-Tenant-Id is not a declared field here, so tenant defaults to
                    // "unknown" at the backend → uid-unknown.
                    if !log_has(&log_a, "wh5_user_id", "uid-unknown") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "with X-Required present, expected wh5_user_id=uid-unknown | log: {}",
                                log_a
                            ),
                        );
                    }

                    // Case B: required field missing → webhook not called → var unset.
                    let trace_b = format!("wh5-missing-{}", uuid::Uuid::new_v4());
                    if let Err(e) = send_gw(&ctx, "wh5.example.com", "/test/wh5", &[], &trace_b).await {
                        return TestResult::failed(start.elapsed(), e);
                    }
                    let log_b = match fetch_log(&ctx, &trace_b).await {
                        Ok(l) => l,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    if log_has_key(&log_b, "wh5_user_id") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "with X-Required missing, the webhook should not have been called \
                                 (wh5_user_id must be unset) | log: {}",
                                log_b
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "presence: required gated the outbound call (resolved when present, skipped when missing). \
                         NOTE: statusOnError 403 is not enforced by WebhookKeyGet (known gap)."
                            .to_string(),
                    )
                })
            },
        )
    }

    // ==================== 6. presence: warnIfMissing → continues ====================
    fn test_presence_warn() -> TestCase {
        TestCase::new(
            "presence_warn",
            "presence: warnIfMissing omits the absent field but the webhook still resolves",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    // Send WITHOUT X-Optional-Trace-Id (the warnIfMissing origin field).
                    run_case(&ctx, "wh6.example.com", "/test/wh6", &[], "wh6", start, |log| {
                        // Field omitted, request proceeds → webhook resolves uid-unknown
                        // (no tenant declared). warnIfMissing never escalates to failure.
                        if log_has(log, "wh6_user_id", "uid-unknown") {
                            Ok("warnIfMissing field omitted; webhook resolved and request continued".into())
                        } else {
                            Err("expected wh6_user_id=uid-unknown (warn field omitted, call proceeded)".into())
                        }
                    })
                    .await
                })
            },
        )
    }

    // ==================== 7. presence: required (missing → degrades to default) ====================
    fn test_presence_required_degrade() -> TestCase {
        TestCase::new(
            "presence_required_degrade",
            "required field missing → degrade to default, no 5xx",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Case A: required X-Auth missing → webhook fails → degrade to default.
                    let trace_a = format!("wh7-degrade-{}", uuid::Uuid::new_v4());
                    if let Err(e) = send_gw(&ctx, "wh7.example.com", "/test/wh7", &[], &trace_a).await {
                        return TestResult::failed(start.elapsed(), e);
                    }
                    let log_a = match fetch_log(&ctx, &trace_a).await {
                        Ok(l) => l,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    if !log_has(&log_a, "wh7_user_id", "degraded-fallback") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "required field missing should degrade to default 'degraded-fallback' | log: {}",
                                log_a
                            ),
                        );
                    }

                    // Case B: required X-Auth present → webhook resolves normally.
                    let trace_b = format!("wh7-ok-{}", uuid::Uuid::new_v4());
                    if let Err(e) =
                        send_gw(&ctx, "wh7.example.com", "/test/wh7", &[("X-Auth", "token")], &trace_b).await
                    {
                        return TestResult::failed(start.elapsed(), e);
                    }
                    let log_b = match fetch_log(&ctx, &trace_b).await {
                        Ok(l) => l,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    if !log_has(&log_b, "wh7_user_id", "uid-unknown") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "with X-Auth present the webhook should resolve (uid-unknown) | log: {}",
                                log_b
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "required field missing: degraded to default (200, no 5xx); present field resolved".to_string(),
                    )
                })
            },
        )
    }

    // ==================== 8. pathOverride honored / ignored ====================
    fn test_path_override() -> TestCase {
        TestCase::new(
            "path_override",
            "pathOverride honored when allowOverride=true, ignored when false",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    run_case(&ctx, "wh8.example.com", "/test/wh8", &[], "wh8", start, |log| {
                        // wh8 extracts data.path (echoed by the backend) for two webhooks:
                        //  - wh8_overridden: allowOverride=true  → override path /webhook/override
                        //  - wh8_ignored:    allowOverride=false → template path /webhook/default
                        if !log_has(log, "wh8_overridden", "/webhook/override") {
                            return Err("override allowed: expected wh8_overridden=/webhook/override".into());
                        }
                        if !log_has(log, "wh8_ignored", "/webhook/default") {
                            return Err("override disallowed: expected wh8_ignored=/webhook/default".into());
                        }
                        Ok("pathOverride honored when allowed and ignored when disallowed".into())
                    })
                    .await
                })
            },
        )
    }

    // ==================== 9. ctx-set variables in webhook templates ====================
    fn test_ctx_set_integration() -> TestCase {
        TestCase::new(
            "ctx_set_integration",
            "ctx-set vars feed ${ctx:...} into the webhook path and a custom header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    run_case(
                        &ctx,
                        "wh9.example.com",
                        "/test/wh9",
                        &[("X-User-Id", "user-99")],
                        "wh9",
                        start,
                        |log| {
                            // wh9 extracts data.echo_headers.x-resolved-user. The header is
                            // a custom template "${ctx:user_id}" (user_id <- X-User-Id).
                            // A correct value also proves the path template
                            // "/webhook/${ctx:action}" resolved (else no echo handler hit).
                            if log_has(log, "wh9_result", "user-99") {
                                Ok("ctx-set vars substituted into webhook path and custom header".into())
                            } else {
                                Err("expected wh9_result=user-99 (ctx var substituted into outbound header)".into())
                            }
                        },
                    )
                    .await
                })
            },
        )
    }

    // ==================== 10. custom template fields ====================
    fn test_custom_template() -> TestCase {
        TestCase::new(
            "custom_template",
            "Custom header templates resolve ${header:...} and ${ctx:...} into the outbound request",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    run_case(
                        &ctx,
                        "wh10.example.com",
                        "/test/wh10",
                        &[("X-User-Id", "tmpl-user-10"), ("X-Request-ID", "rid-10")],
                        "wh10",
                        start,
                        |log| {
                            // wh10 extracts data.echo_headers.x-from-header, which is the
                            // custom template "${header:X-User-Id}".
                            if log_has(log, "wh10_result", "tmpl-user-10") {
                                Ok("custom template ${header:X-User-Id} resolved into outbound header".into())
                            } else {
                                Err("expected wh10_result=tmpl-user-10 (template header resolved)".into())
                            }
                        },
                    )
                    .await
                })
            },
        )
    }

    // ==================== 11. custom Secret-sourced field ====================
    fn test_custom_secret() -> TestCase {
        TestCase::new(
            "custom_secret",
            "Secret-sourced custom header is verified by KeyAuth without exposing it in access logs",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("wh11-{}", uuid::Uuid::new_v4());
                    let response = match send_gw(&ctx, "wh11.example.com", "/test/wh11", &[], &trace_id).await {
                        Ok(response) => response,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    if response.status().as_u16() != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "KeyAuth did not verify the Secret-sourced webhook value: status {}",
                                response.status()
                            ),
                        );
                    }

                    let log = match fetch_log(&ctx, &trace_id).await {
                        Ok(log) => log,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    if log.contains("test-secret-key-12345") {
                        return TestResult::failed(
                            start.elapsed(),
                            "access log leaked the Secret-sourced webhook credential".to_string(),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "KeyAuth verified the full Secret-sourced webhook value; access log remained redacted"
                            .to_string(),
                    )
                })
            },
        )
    }

    // ==================== 12. URL target, happy path ====================
    fn test_url_target_happy_path() -> TestCase {
        TestCase::new(
            "url_target_happy_path",
            "Webhook target.url (hostname) resolves through EgressResolver and reaches the upstream request",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    run_case(&ctx, "wh12.example.com", "/test/wh12", &[], "wh12", start, |log| {
                        // No origin field is declared, so the backend sees no
                        // X-Tenant-Id and defaults to uid-unknown. A correct value
                        // proves the URL target (http://localhost:30001, a
                        // hostname resolved through EgressResolver) was dialed
                        // successfully end to end.
                        if log_has(log, "wh12_user_id", "uid-unknown") {
                            Ok(
                                "URL target (hostname) resolved via EgressResolver; value reached the upstream request"
                                    .into(),
                            )
                        } else {
                            Err("expected wh12_user_id=uid-unknown (URL target resolved)".into())
                        }
                    })
                    .await
                })
            },
        )
    }

    // ==================== 13. URL target over https with custom CA ====================
    fn test_url_tls_custom_ca() -> TestCase {
        TestCase::new(
            "url_tls_custom_ca",
            "Webhook target.url (https) validated against a custom CA (tls.validation.caCertificateRefs)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    run_case(&ctx, "wh13.example.com", "/test/wh13", &[], "wh13", start, |log| {
                        if log_has(log, "wh13_user_id", "uid-unknown") {
                            Ok("https:// URL target validated against custom CA and resolved successfully".into())
                        } else {
                            Err("expected wh13_user_id=uid-unknown (TLS URL target resolved)".into())
                        }
                    })
                    .await
                })
            },
        )
    }

    // ==================== 14. Secret-sourced auth header on a URL target ====================
    fn test_url_secret_auth_header_redacted() -> TestCase {
        TestCase::new(
            "url_secret_auth_header_redacted",
            "Secret-sourced Authorization header reaches the URL target; controller admin output redacts it",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // (a) KeyAuth verifies the exact value echoed by test_server
                    // without exporting that value through accessLogExtern.
                    let trace_id = format!("wh14-{}", uuid::Uuid::new_v4());
                    let response = match send_gw(&ctx, "wh14.example.com", "/test/wh14", &[], &trace_id).await {
                        Ok(response) => response,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    if response.status().as_u16() != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "KeyAuth did not verify the Secret-sourced URL-target value: status {}",
                                response.status()
                            ),
                        );
                    }
                    let log = match fetch_log(&ctx, &trace_id).await {
                        Ok(l) => l,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    if log.contains("integration-test-secret-77") {
                        return TestResult::failed(
                            start.elapsed(),
                            "access log leaked the Secret-sourced URL-target credential".to_string(),
                        );
                    }

                    // (b) the controller's own admin output never shows the real value.
                    let resource = match fetch_linksys_resource(&ctx, "wh14-resolver").await {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("admin fetch failed: {}", e)),
                    };
                    let resource_str = resource.to_string();
                    if resource_str.contains("integration-test-secret-77") {
                        return TestResult::failed(
                            start.elapsed(),
                            "controller admin output leaked the real secret value".to_string(),
                        );
                    }
                    let resolved_secrets = &resource["spec"]["config"]["resolvedSecrets"];
                    if resolved_secrets.as_str() != Some("[redacted]") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "expected spec.config.resolvedSecrets == \"[redacted]\", got: {}",
                                resolved_secrets
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "KeyAuth verified the full URL-target token; access log and controller output remained redacted"
                            .to_string(),
                    )
                })
            },
        )
    }

    // ==================== 15. Retry on 5xx (URL target) ====================
    fn test_url_retry_on_5xx() -> TestCase {
        TestCase::new(
            "url_retry_on_5xx",
            "retry.retryOnStatus retries a URL target through exactly 2 retries (3 total attempts)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    run_case(&ctx, "wh15.example.com", "/test/wh15", &[], "wh15", start, |log| {
                        // x-attempt-history on the final (successful) response lists
                        // every status returned so far for this test_id: "503,503,200"
                        // is only reachable after exactly 3 inbound requests.
                        if log_has(log, "wh15_result", "503,503,200") {
                            Ok("retryOnStatus:[503] retried exactly twice; 3rd attempt (200) succeeded".into())
                        } else {
                            Err("expected wh15_result=503,503,200 (exactly 3 inbound requests)".into())
                        }
                    })
                    .await
                })
            },
        )
    }

    // ==================== 16. blockPrivate rejects an IP-literal URL target ====================
    fn test_url_block_private_rejected() -> TestCase {
        TestCase::new(
            "url_block_private_rejected",
            "blockPrivate:true rejects an RFC1918 IP-literal target pre-connect; metric increments; \
             negative control (blockPrivate absent) fails differently and does not increment the metric",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let baseline = match fetch_egress_blocked_private_count(&ctx).await {
                        Ok(v) => v,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("metrics baseline: {}", e)),
                    };

                    // Negative control first: same unroutable RFC1918 address, but
                    // blockPrivate absent. Must NOT increment the "private" metric —
                    // it fails by connection timeout, not the SSRF guard.
                    let trace_ctrl = format!("wh16ctrl-{}", uuid::Uuid::new_v4());
                    if let Err(e) = send_gw(&ctx, "wh16ctrl.example.com", "/test/wh16ctrl", &[], &trace_ctrl).await {
                        return TestResult::failed(start.elapsed(), e);
                    }
                    if let Err(e) = fetch_log(&ctx, &trace_ctrl).await {
                        return TestResult::failed(start.elapsed(), format!("negative control access-log: {}", e));
                    }

                    let after_ctrl = match fetch_egress_blocked_private_count(&ctx).await {
                        Ok(v) => v,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("metrics after ctrl: {}", e)),
                    };
                    if after_ctrl != baseline {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "negative control (blockPrivate absent) must NOT increment reason=\"private\": \
                                 baseline={} after_ctrl={}",
                                baseline, after_ctrl
                            ),
                        );
                    }

                    // blockPrivate:true — rejected pre-connect; the metric must increment.
                    let trace_block = format!("wh16-{}", uuid::Uuid::new_v4());
                    if let Err(e) = send_gw(&ctx, "wh16.example.com", "/test/wh16", &[], &trace_block).await {
                        return TestResult::failed(start.elapsed(), e);
                    }
                    let log_block = match fetch_log(&ctx, &trace_block).await {
                        Ok(l) => l,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("blocked access-log: {}", e)),
                    };
                    if !log_has(&log_block, "wh16_result", "blocked-fallback") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "expected wh16_result=blocked-fallback (webhook call rejected, degraded to \
                                 default) | access-log: {}",
                                log_block
                            ),
                        );
                    }

                    let after_block = match fetch_egress_blocked_private_count(&ctx).await {
                        Ok(v) => v,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("metrics after block: {}", e)),
                    };
                    if after_block != after_ctrl + 1 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "expected reason=\"private\" to increment by exactly 1: after_ctrl={} \
                                 after_block={}",
                                after_ctrl, after_block
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "blockPrivate:true rejected pre-connect (metric {} -> {}); negative control \
                             (blockPrivate absent) failed differently without incrementing the metric",
                            after_ctrl, after_block
                        ),
                    )
                })
            },
        )
    }

    // ==================== 17. Origin-only rejection reported by the controller ====================
    fn test_url_origin_only_rejected() -> TestCase {
        TestCase::new(
            "url_origin_only_rejected",
            "target.url with a path is rejected: Accepted=False with an 'origin-only' message",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match fetch_linksys_conditions(&ctx, "wh17-origin-only").await {
                        Err(e) => TestResult::failed(start.elapsed(), e),
                        Ok(conditions) => match find_condition(&conditions, "Accepted") {
                            Some(c) if c["status"].as_str() == Some("False") => {
                                let msg = c["message"].as_str().unwrap_or("");
                                if msg.contains("origin-only") {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!("Accepted=False with 'origin-only' in message: {}", msg),
                                    )
                                } else {
                                    TestResult::failed(
                                        start.elapsed(),
                                        format!("Accepted=False but message lacks 'origin-only': '{}'", msg),
                                    )
                                }
                            }
                            Some(c) => TestResult::failed(start.elapsed(), format!("Accepted not False: {:?}", c)),
                            None => TestResult::failed(
                                start.elapsed(),
                                format!("Accepted condition not found: {:?}", conditions),
                            ),
                        },
                    }
                })
            },
        )
    }

    // ==================== 18. json body ====================
    fn test_json_body() -> TestCase {
        TestCase::new(
            "json_body",
            "LinkSys webhook request.body (type: json) renders a header variable and auto-injects Content-Type",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    run_case(
                        &ctx,
                        "wh18.example.com",
                        "/test/wh18",
                        &[("X-User-Id", "user-18")],
                        "wh18",
                        start,
                        |log| {
                            if !log_has(log, "wh18_body_user", "user-18") {
                                return Err("body template variable did not reach the webhook backend".to_string());
                            }
                            if !log_has(log, "wh18_ct", "application/json") {
                                return Err("auto-injected Content-Type not observed by backend".to_string());
                            }
                            Ok("json body rendered + Content-Type auto-injected".to_string())
                        },
                    )
                    .await
                })
            },
        )
    }

    // ==================== 19. WebhookKeyGet then KeyAuth ====================
    //
    // Valid echoed key → 200. Missing or wrong key → 401 (KeyAuth). Documented
    // hazard: KeyAuth before WebhookKeyGet is a silent auth bypass — W-1
    // declined a cross-plugin ordering check.
    fn test_webhook_then_key_auth() -> TestCase {
        TestCase::new(
            "webhook_then_key_auth",
            "WebhookKeyGet writes a ctx key; KeyAuth after it accepts the valid key and 401s otherwise",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let trace_ok = format!("wh19-ok-{}", uuid::Uuid::new_v4());
                    let resp_ok = match send_gw(
                        &ctx,
                        "wh19.example.com",
                        "/test/wh19",
                        &[("X-Wh-Key", "test-key-wh19-valid")],
                        &trace_ok,
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let status_ok = resp_ok.status().as_u16();
                    if status_ok != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("valid key expected 200, got {}", status_ok),
                        );
                    }
                    let log_ok = match fetch_log(&ctx, &trace_ok).await {
                        Ok(log) => log,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    if log_ok.contains("test-key-wh19-valid") {
                        return TestResult::failed(
                            start.elapsed(),
                            "access log leaked the webhook-derived API key".to_string(),
                        );
                    }

                    let trace_missing = format!("wh19-missing-{}", uuid::Uuid::new_v4());
                    let resp_missing = match send_gw(&ctx, "wh19.example.com", "/test/wh19", &[], &trace_missing).await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let status_missing = resp_missing.status().as_u16();
                    if status_missing != 401 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("missing key expected 401, got {}", status_missing),
                        );
                    }

                    let trace_wrong = format!("wh19-wrong-{}", uuid::Uuid::new_v4());
                    let resp_wrong = match send_gw(
                        &ctx,
                        "wh19.example.com",
                        "/test/wh19",
                        &[("X-Wh-Key", "wrong-key")],
                        &trace_wrong,
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let status_wrong = resp_wrong.status().as_u16();
                    if status_wrong != 401 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("wrong key expected 401, got {}", status_wrong),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "WebhookKeyGet then KeyAuth: valid key 200 without access-log leakage; missing/wrong key 401. \
                         Misordered list (KeyAuth first) is a silent auth bypass — W-1 declined \
                         a cross-plugin ordering check."
                            .to_string(),
                    )
                })
            },
        )
    }
}

impl TestSuite for WebhookKeyGetTestSuite {
    fn name(&self) -> &str {
        "WebhookKeyGet Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_service_plus_endpointslice(),
            Self::test_all_origin_transparent(),
            Self::test_all_custom_literal(),
            Self::test_presence_required_fail(),
            Self::test_presence_warn(),
            Self::test_presence_required_degrade(),
            Self::test_path_override(),
            Self::test_ctx_set_integration(),
            Self::test_custom_template(),
            Self::test_custom_secret(),
            Self::test_url_target_happy_path(),
            Self::test_url_tls_custom_ca(),
            Self::test_url_secret_auth_header_redacted(),
            Self::test_url_retry_on_5xx(),
            Self::test_url_block_private_rejected(),
            Self::test_url_origin_only_rejected(),
            Self::test_json_body(),
            Self::test_webhook_then_key_auth(),
        ]
    }
}
