// TCPRoute GeoRestriction (L4 geo) integration suite — design §6.3 rows.
//
// Coverage against edgion-tests/integration/conf/TCPRoute/GeoRestriction/:
//   - the "tcp" (Stage-2, post route-match) layer via tcp-geo-gw:31951 /
//     tcp-geo-route (route-level edgion-stream-plugins annotation);
//   - the "stage1" (pre-accept ConnectionFilter) layer via
//     tcp-geo-bare-gw:31952 (gateway-level annotation, no TCPRoute at all).
//
// Every source address is the client's OWN bound source address (client-side
// source binding, per task-15-brief.md), never a spoofed header — this is
// what makes L4 geo (`DirectGeo`-equivalent; L4 has no HTTP layer at all)
// testable from a closed harness with no production backdoor. The fixture's
// authored address map (edgion-tests/geoip/README.md):
//   127.0.0.1 CN, 127.0.0.5 RU, 127.0.0.6 US, 127.0.0.7 HK.
//
// Per design §6.6, an L4 geo denial is never logged and carries no client
// address — the metric `edgion_geoip_denied_total{layer,reason}` is the ONLY
// observable. Every denial row below asserts it (via the shared
// `metrics_helper::counter_value` scrape helper) in addition to the
// connection-level verdict, and also asserts the negative for a plain IP-deny
// row (no geo counter bump — the geo stage was never reached, row 1/2 are
// terminal before geo per plugin.rs's `on_connection`).
//
// Verdict discipline: `probe_tcp_echo` below classifies a failed `connect()`
// as `Inconclusive`, NOT `Closed` — mirroring
// suites/tcp_route/global_connection_ip_restriction/global_connection_ip_restriction.rs,
// NOT the looser suites/tls_route/stream_plugins.rs pattern. A GCIR/geo denial
// is observed as a post-accept EOF; a `connect()` failure (e.g. because the
// test failed to bind its own claimed source address) must be loud, not
// silently counted as "successfully denied".

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpSocket;

pub struct TcpGeoRestrictionTestSuite;

/// The "bare" listener's port (edgion-tests/integration/conf/ports.json,
/// suite key "TCPRoute/GeoRestriction" -> "tcp_bare"). No TestContext field
/// exists for it (only "tcp" is a first-class port); hardcoded here the same
/// way other suites hardcode the fixed metrics port 12090.
const BARE_PORT: u16 = 31952;

enum TcpEchoProbe {
    Echoed(usize),
    Closed(String),
    Inconclusive(String),
}

/// Connect FROM `src` (a loopback alias mapped to a synthetic country by the
/// GeoIP test fixture) TO `addr`. The gateway's peer address — and therefore
/// its geo lookup — is exactly the address this test bound.
async fn probe_from(src: &str, addr: &str, test_data: &[u8]) -> TcpEchoProbe {
    let sock = match TcpSocket::new_v4() {
        Ok(s) => s,
        Err(e) => return TcpEchoProbe::Inconclusive(format!("socket() failed: {e}")),
    };
    let src_addr = match format!("{src}:0").parse() {
        Ok(a) => a,
        Err(e) => return TcpEchoProbe::Inconclusive(format!("unparseable source addr {src}: {e}")),
    };
    // A bind failure must be LOUD (Inconclusive), never silently read as a
    // successful denial — see the module doc's verdict-discipline note.
    if let Err(e) = sock.bind(src_addr) {
        return TcpEchoProbe::Inconclusive(format!("bind({src}) failed: {e} — is the loopback alias missing?"));
    }
    let dst_addr = match addr.parse() {
        Ok(a) => a,
        Err(e) => return TcpEchoProbe::Inconclusive(format!("unparseable dest addr {addr}: {e}")),
    };
    let mut stream = match tokio::time::timeout(Duration::from_secs(1), sock.connect(dst_addr)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => return TcpEchoProbe::Inconclusive(format!("connect failed: {e}")),
        Err(_) => return TcpEchoProbe::Inconclusive("connect timed out".to_string()),
    };

    if let Err(e) = stream.write_all(test_data).await {
        return TcpEchoProbe::Closed(format!("write failed: {e}"));
    }

    let mut buf = vec![0u8; 1024];
    match tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buf)).await {
        Ok(Ok(n)) if n > 0 => TcpEchoProbe::Echoed(n),
        Ok(Ok(_)) => TcpEchoProbe::Closed("connection closed with EOF".to_string()),
        Ok(Err(e)) => TcpEchoProbe::Closed(format!("read failed: {e}")),
        Err(_) => TcpEchoProbe::Inconclusive("read timed out".to_string()),
    }
}

async fn wait_for_echo(src: &str, addr: &str, test_data: &[u8]) -> Result<usize, String> {
    let mut last = "no probe completed".to_string();
    let wait = async {
        loop {
            match probe_from(src, addr, test_data).await {
                TcpEchoProbe::Echoed(n) => return n,
                TcpEchoProbe::Closed(r) | TcpEchoProbe::Inconclusive(r) => last = r,
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(8), wait)
        .await
        .map_err(|_| format!("TCP echo from {src} did not become available within 8s; last: {last}"))
}

/// Waits for a connection denial that a specific `edgion_geoip_denied_total{layer,reason}`
/// bump confirms actually reached the geo stage for the intended reason, not
/// just "the connection was refused".
///
/// A single connect-then-scrape is not safe here: `switch_profile` returns as
/// soon as the API call is acknowledged, before the Selector change has
/// propagated to the gateway via conf_sync (every other row's retry loop
/// tolerates this lag transparently). If the *previous* row happened to leave
/// an active profile whose own IP-deny/allow-miss rule also matches this
/// row's source address, the very first probe can be denied for that stale,
/// unrelated reason — satisfying a bare "was it closed?" check on the first
/// try, before propagation completes, with no retry left to reach the
/// intended geo-driven denial. The client cannot distinguish "denied by geo"
/// from "denied for any other reason" (§6.6: an L4 geo denial carries no
/// signal beyond a reset), so the metric bump is folded into the SAME retry
/// loop as the connection probe: only a `Closed` result whose immediately
/// following scrape shows the target `layer`/`reason` delta counts as
/// success. `baseline` is captured once, before the loop, so a
/// cross-test-cumulative counter can't produce a false positive.
async fn wait_for_geo_denial(
    ctx: &TestContext,
    src: &str,
    addr: &str,
    test_data: &[u8],
    layer: &str,
    reason: &str,
) -> Result<(String, f64), String> {
    let baseline = geoip_denied_total(ctx, layer, reason).await;
    let mut last = "no probe completed".to_string();
    let wait = async {
        loop {
            match probe_from(src, addr, test_data).await {
                TcpEchoProbe::Closed(close_reason) => {
                    let current = geoip_denied_total(ctx, layer, reason).await;
                    if current - baseline >= 1.0 {
                        return (close_reason, current);
                    }
                    last = format!("{close_reason} (edgion_geoip_denied_total{{layer=\"{layer}\",reason=\"{reason}\"}} not yet bumped: {current}, baseline {baseline})");
                }
                TcpEchoProbe::Echoed(n) => last = format!("received {n} echo bytes"),
                TcpEchoProbe::Inconclusive(reason) => last = reason,
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(8), wait).await.map_err(|_| {
        format!(
            "connection from {src} was not denied with edgion_geoip_denied_total{{layer=\"{layer}\",reason=\"{reason}\"}} \
             bumped within 8s; last: {last}"
        )
    })
}

/// Snapshot of every `edgion_geoip_denied_total{layer,reason}` combo this
/// suite's rows can produce, taken as one unit so a before/after comparison
/// is self-consistent.
struct GeoDeniedSnapshot {
    stage1_deny: f64,
    tcp_deny: f64,
    stage1_miss: f64,
    tcp_miss: f64,
}

impl GeoDeniedSnapshot {
    async fn capture(ctx: &TestContext) -> Self {
        Self {
            stage1_deny: geoip_denied_total(ctx, "stage1", "deny_country").await,
            tcp_deny: geoip_denied_total(ctx, "tcp", "deny_country").await,
            stage1_miss: geoip_denied_total(ctx, "stage1", "allow_miss").await,
            tcp_miss: geoip_denied_total(ctx, "tcp", "allow_miss").await,
        }
    }

    fn any_increased_since(&self, before: &GeoDeniedSnapshot) -> bool {
        self.stage1_deny > before.stage1_deny
            || self.tcp_deny > before.tcp_deny
            || self.stage1_miss > before.stage1_miss
            || self.tcp_miss > before.tcp_miss
    }
}

/// For rows that are denied *without* ever reaching the geo stage (a
/// terminal IP-deny/allow-miss hit, or a "geo had no opinion" fall-through):
/// asserts the connection-level plugin's own `edgion_conn_ip_restriction_events_total{result="denied"}`
/// counter bumped, proving the deny path actually fired rather than the test
/// having silently stopped exercising it — the symmetric counterpart to
/// `wait_for_geo_denial` above, and also proves the geo-reason counters did
/// NOT bump (the geo stage was never reached).
///
/// The geo-counter comparison is scoped to a single retry iteration
/// (snapshot immediately before that one probe, snapshot immediately after),
/// NOT to the whole retry loop: while `switch_profile` is still propagating,
/// earlier iterations can land on the previous row's still-active profile
/// and produce their own, unrelated geo denials (observable as this suite's
/// own prior rows re-triggering) — those must not be blamed on the
/// eventually-confirmed connection.
async fn wait_for_non_geo_denial(ctx: &TestContext, src: &str, addr: &str, test_data: &[u8]) -> Result<String, String> {
    let mut last = "no probe completed".to_string();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "connection from {src} was not denied with edgion_conn_ip_restriction_events_total{{result=\"denied\"}} \
                 bumped within 8s; last: {last}"
            ));
        }
        let ip_before = conn_ip_restriction_denied_total(ctx).await;
        let geo_before = GeoDeniedSnapshot::capture(ctx).await;
        match probe_from(src, addr, test_data).await {
            TcpEchoProbe::Closed(close_reason) => {
                let ip_after = conn_ip_restriction_denied_total(ctx).await;
                if ip_after - ip_before >= 1.0 {
                    let geo_after = GeoDeniedSnapshot::capture(ctx).await;
                    if geo_after.any_increased_since(&geo_before) {
                        return Err(format!(
                            "denied ({close_reason}) via the IP stage as expected, but a geo-reason counter also \
                             bumped during this exact connection (stage1/deny_country {}->{}, tcp/deny_country {}->{}, \
                             stage1/allow_miss {}->{}, tcp/allow_miss {}->{}) — the geo stage should never have been \
                             reached for this row",
                            geo_before.stage1_deny,
                            geo_after.stage1_deny,
                            geo_before.tcp_deny,
                            geo_after.tcp_deny,
                            geo_before.stage1_miss,
                            geo_after.stage1_miss,
                            geo_before.tcp_miss,
                            geo_after.tcp_miss,
                        ));
                    }
                    return Ok(format!("{close_reason}, ip-restriction metric={ip_after}"));
                }
                last = format!(
                    "{close_reason} (edgion_conn_ip_restriction_events_total{{result=\"denied\"}} not yet bumped: \
                     {ip_after}, baseline {ip_before} — likely still denied by a stale prior profile)"
                );
            }
            TcpEchoProbe::Echoed(n) => last = format!("received {n} echo bytes"),
            TcpEchoProbe::Inconclusive(reason) => last = reason,
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn selector_yaml_with(active_profile: &str) -> String {
    format!(
        r#"apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: tcp-geo-selector
  namespace: edgion-test
spec:
  data:
    type: Selector
    config:
      active: {active_profile}
"#,
    )
}

async fn switch_profile(ctx: &TestContext, profile: &str) -> Result<(), String> {
    ctx.apply_yaml(&selector_yaml_with(profile)).await
}

async fn geoip_denied_total(ctx: &TestContext, layer: &str, reason: &str) -> f64 {
    let metrics = crate::metrics_helper::MetricsClient::from_host_port(&ctx.target_host, 12090);
    let Ok(raw) = metrics.fetch_raw_metrics().await else {
        return 0.0;
    };
    crate::metrics_helper::counter_value(
        &raw,
        "edgion_geoip_denied_total",
        &[("layer", layer), ("reason", reason)],
    )
}

/// `edgion_conn_ip_restriction_events_total{result="denied"}` — bumped by the
/// SAME `GlobalConnectionIpRestrictionPlugin::on_connection` for every deny
/// verdict that never reaches (or never triggers) the geo stage: a terminal
/// IP-deny hit (row 1), and the "geo has no opinion" fall-through to the
/// original IP-stage decision (rows 4 / 4'). Not layer-scoped (the plugin
/// records it identically regardless of which listener stage invoked it).
async fn conn_ip_restriction_denied_total(ctx: &TestContext) -> f64 {
    let metrics = crate::metrics_helper::MetricsClient::from_host_port(&ctx.target_host, 12090);
    let Ok(raw) = metrics.fetch_raw_metrics().await else {
        return 0.0;
    };
    crate::metrics_helper::counter_value(&raw, "edgion_conn_ip_restriction_events_total", &[("result", "denied")])
}

impl TcpGeoRestrictionTestSuite {
    /// Row 1: `deny: 127.0.0.5/32`, `geo.allowCountries: [RU]`, source
    /// 127.0.0.5 (RU) — IP deny is terminal; the geo stage never runs (no
    /// `geo:*` denial reason, no geo counter bump).
    fn row1_ip_deny_terminal() -> TestCase {
        TestCase::new(
            "row1_ip_deny_terminal",
            "row 1: an IP deny hit wins over geo and is terminal",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(e) = switch_profile(&ctx, "row1_ip_deny_terminal").await {
                        return TestResult::failed(start.elapsed(), format!("switch profile: {e}"));
                    }
                    let addr = ctx.tcp_addr();
                    match wait_for_non_geo_denial(&ctx, "127.0.0.5", &addr, b"row1").await {
                        Ok(msg) => TestResult::passed_with_message(start.elapsed(), format!("denied ({msg})")),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Row 2: `allow: 127.0.0.5/32`, `geo.denyCountries: [RU]`, source
    /// 127.0.0.5 (RU) — the IP allow hit EXEMPTS a geo deny; this is the
    /// expressive value of merging geo into this plugin.
    fn row2_ip_allow_exempts() -> TestCase {
        TestCase::new(
            "row2_ip_allow_exempts",
            "row 2: an IP allow hit exempts a geo deny",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(e) = switch_profile(&ctx, "row2_ip_allow_exempts").await {
                        return TestResult::failed(start.elapsed(), format!("switch profile: {e}"));
                    }
                    let addr = ctx.tcp_addr();
                    match wait_for_echo("127.0.0.5", &addr, b"row2").await {
                        Ok(n) => TestResult::passed_with_message(start.elapsed(), format!("allowed, echoed {n} bytes")),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Row 3b: `geo.denyCountries: [RU]` only, source 127.0.0.5 (RU) — denied.
    fn row3b_deny_hit() -> TestCase {
        TestCase::new(
            "row3b_deny_hit",
            "row 3b: a bare geo denyCountries hit denies",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(e) = switch_profile(&ctx, "row3b_and_3d").await {
                        return TestResult::failed(start.elapsed(), format!("switch profile: {e}"));
                    }
                    let addr = ctx.tcp_addr();
                    match wait_for_geo_denial(&ctx, "127.0.0.5", &addr, b"row3b", "tcp", "deny_country").await {
                        Ok((reason, bumped)) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("denied ({reason}), metric={bumped}"),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Row 3d: same `row3b_and_3d` profile, source 127.0.0.1 (CN, not in
    /// denyCountries) — geo has no opinion, falls through to the profile's
    /// `defaultAction: allow`.
    fn row3d_falls_through() -> TestCase {
        TestCase::new(
            "row3d_falls_through",
            "row 3d: geo has no opinion on a non-listed country, falls through to defaultAction",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(e) = switch_profile(&ctx, "row3b_and_3d").await {
                        return TestResult::failed(start.elapsed(), format!("switch profile: {e}"));
                    }
                    let addr = ctx.tcp_addr();
                    match wait_for_echo("127.0.0.1", &addr, b"row3d").await {
                        Ok(n) => TestResult::passed_with_message(start.elapsed(), format!("allowed, echoed {n} bytes")),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Row 3c hit: `defaultAction: deny`, `geo.allowCountries: [HK]`, source
    /// 127.0.0.7 (HK) — allow-mode geo is terminal: allowed even though
    /// `defaultAction` is deny.
    fn row3c_hit() -> TestCase {
        TestCase::new(
            "row3c_hit",
            "row 3c hit: an allowCountries match is terminal, overriding defaultAction: deny",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(e) = switch_profile(&ctx, "row3c_terminal").await {
                        return TestResult::failed(start.elapsed(), format!("switch profile: {e}"));
                    }
                    let addr = ctx.tcp_addr();
                    match wait_for_echo("127.0.0.7", &addr, b"row3c-hit").await {
                        Ok(n) => TestResult::passed_with_message(start.elapsed(), format!("allowed, echoed {n} bytes")),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Row 3c miss: same profile, source 127.0.0.6 (US, not in
    /// allowCountries) — an allow-mode miss denies.
    fn row3c_miss() -> TestCase {
        TestCase::new(
            "row3c_miss",
            "row 3c miss: allow-mode geo denies a non-listed country",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(e) = switch_profile(&ctx, "row3c_terminal").await {
                        return TestResult::failed(start.elapsed(), format!("switch profile: {e}"));
                    }
                    let addr = ctx.tcp_addr();
                    match wait_for_geo_denial(&ctx, "127.0.0.6", &addr, b"row3c-miss", "tcp", "allow_miss").await {
                        Ok((reason, bumped)) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("denied ({reason}), metric={bumped}"),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Row 4 — the regression this whole design exists to prevent:
    /// `allow: 127.0.0.9/32` (unrelated -> whitelist mode), `defaultAction:
    /// allow` (the INERT default under whitelist mode), `geo.denyCountries:
    /// [RU]`, `failOpen: true`, source 127.0.0.6 (US) — not in the allowlist,
    /// not geo-denied (RU only), yet must still be DENIED:
    /// `DenyByWhitelistMiss` must survive the geo stage untouched, never
    /// resurrecting `defaultAction: allow`.
    fn row4_regression() -> TestCase {
        TestCase::new(
            "row4_regression",
            "row 4: an allowlist miss stays denied even though geo has no opinion and defaultAction is allow",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(e) = switch_profile(&ctx, "row4_regression").await {
                        return TestResult::failed(start.elapsed(), format!("switch profile: {e}"));
                    }
                    let addr = ctx.tcp_addr();
                    match wait_for_non_geo_denial(&ctx, "127.0.0.6", &addr, b"row4").await {
                        Ok(msg) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("denied ({msg}) — defaultAction: allow was correctly NOT resurrected"),
                        ),
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!("{e} — THIS IS THE REGRESSION: an inert defaultAction: allow leaked through"),
                        ),
                    }
                })
            },
        )
    }

    /// Row 4': no `allow` (blacklist/default mode), `defaultAction: deny`,
    /// deny-only geo with no hit, source 127.0.0.1 (CN, not RU) — falls
    /// through to the plain `defaultAction: deny`.
    fn row4prime_default_deny() -> TestCase {
        TestCase::new(
            "row4prime_default_deny",
            "row 4': no geo hit falls through to a plain defaultAction: deny",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(e) = switch_profile(&ctx, "row4prime_default_deny").await {
                        return TestResult::failed(start.elapsed(), format!("switch profile: {e}"));
                    }
                    let addr = ctx.tcp_addr();
                    match wait_for_non_geo_denial(&ctx, "127.0.0.1", &addr, b"row4prime").await {
                        Ok(msg) => TestResult::passed_with_message(start.elapsed(), format!("denied ({msg})")),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// No geo block at all: byte-for-byte legacy GlobalConnectionIpRestriction
    /// behavior (source 127.0.0.1, in the loopback allowlist -> allowed).
    fn no_geo_block_unchanged() -> TestCase {
        TestCase::new(
            "no_geo_block_unchanged",
            "a profile with no geo block behaves exactly as before this feature",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(e) = switch_profile(&ctx, "no_geo_baseline").await {
                        return TestResult::failed(start.elapsed(), format!("switch profile: {e}"));
                    }
                    let addr = ctx.tcp_addr();
                    let result = match wait_for_echo("127.0.0.1", &addr, b"no-geo").await {
                        Ok(n) => TestResult::passed_with_message(start.elapsed(), format!("allowed, echoed {n} bytes")),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    };
                    // Restore a known-good baseline for whatever runs next.
                    let _ = switch_profile(&ctx, "row1_ip_deny_terminal").await;
                    result
                })
            },
        )
    }

    /// Bare listener (no TCPRoute at all): the connection is evaluated purely
    /// by the Stage-1 pre-accept ConnectionFilter, so a geo denial here must
    /// bump `edgion_geoip_denied_total{layer="stage1",...}` — proving the
    /// "stage1" layer label is wired, not just "tcp".
    fn bare_listener_is_stage1() -> TestCase {
        TestCase::new(
            "bare_listener_is_stage1",
            "a bare listener (no TCPRoute) evaluates geo at the stage1 (pre-accept) layer",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(e) = switch_profile(&ctx, "row3b_and_3d").await {
                        return TestResult::failed(start.elapsed(), format!("switch profile: {e}"));
                    }
                    let addr = format!("{}:{}", ctx.target_host, BARE_PORT);
                    // The bare listener has no backend to echo from; a geo deny at
                    // accept time is observed as connect-then-immediate-EOF (no
                    // route ever gets a chance to proxy), same shape as a Stage-2
                    // denial.
                    match wait_for_geo_denial(&ctx, "127.0.0.5", &addr, b"bare", "stage1", "deny_country").await {
                        Ok((reason, bumped)) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("denied ({reason}), metric={bumped}"),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }
}

impl TestSuite for TcpGeoRestrictionTestSuite {
    fn name(&self) -> &str {
        "TCPRoute/GeoRestriction"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            TcpGeoRestrictionTestSuite::row1_ip_deny_terminal(),
            TcpGeoRestrictionTestSuite::row2_ip_allow_exempts(),
            TcpGeoRestrictionTestSuite::row3b_deny_hit(),
            TcpGeoRestrictionTestSuite::row3d_falls_through(),
            TcpGeoRestrictionTestSuite::row3c_hit(),
            TcpGeoRestrictionTestSuite::row3c_miss(),
            TcpGeoRestrictionTestSuite::row4_regression(),
            TcpGeoRestrictionTestSuite::row4prime_default_deny(),
            TcpGeoRestrictionTestSuite::no_geo_block_unchanged(),
            TcpGeoRestrictionTestSuite::bare_listener_is_stage1(),
        ]
    }
}
