// TCPRoute GeoRestriction (L4) integration suite — coverage for the
// `GeoIpLocation` stream plugin (`EdgionStreamPlugin::GeoIpLocation`).
//
// Against edgion-tests/integration/conf/TCPRoute/GeoRestriction/:
//   - the "tcp" (Stage-2, post route-match) layer via tcp-geo-gw:31951 /
//     tcp-geo-route (route-level edgion-stream-plugins annotation);
//   - the "stage1" (pre-accept ConnectionFilter) layer via
//     tcp-geo-bare-gw:31952 (gateway-level annotation, no TCPRoute at all).
// Both listeners reference the SAME `tcp-geo-esp` EdgionStreamPlugins
// resource, hot-swapped via `ctx.apply_yaml` before each test case (its flat
// config has no per-profile Selector envelope to switch instead).
//
// Local-mode source addresses are the client's OWN bound loopback aliases,
// never spoofed headers — L4 has no XFF at all, so `GeoIpLocation` accepts
// only `DirectPeerIp`-sourced rules here (enforced at validation). In K8s the
// pod cannot bind synthetic loopback sources across the network, so each row
// uses its real pod peer and hot-swaps an equivalent rule around the K8s-only
// GeoIP mapping. The fixture's local authored address map
// (edgion-tests/geoip/README.md): 127.0.0.1 CN, 127.0.0.5 RU, 127.0.0.6 US.
//
// A geo denial at L4 is never logged and carries no client address — the
// metric `edgion_geoip_denied_total{layer,reason}` is the ONLY observable.
// Every geo-denial row below asserts it (via the shared `metrics_helper::
// counter_value` scrape helper) in addition to the connection-level verdict.
// A row denied by a DIFFERENT stage (the chained `GlobalConnectionIpRestriction`
// plugin, before GeoIpLocation ever runs) instead asserts the negative — no
// geo counter bump — via `edgion_conn_ip_restriction_events_total{result="denied"}`.
//
// Verdict discipline: `probe_from` below classifies a failed `connect()` as
// `Inconclusive`, NOT `Closed` — a GeoIpLocation/GCIR denial is observed as a
// post-accept EOF; a `connect()` failure (e.g. because the test failed to
// bind its own claimed source address) must be loud, not silently counted as
// "successfully denied".

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::status::helpers::controller_local_conditions;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpSocket, TcpStream};

pub struct TcpGeoRestrictionTestSuite;

/// The "bare" listener's port (edgion-tests/integration/conf/ports.json,
/// suite key "TCPRoute/GeoRestriction" -> "tcp_bare"). No TestContext field
/// exists for it (only "tcp" is a first-class port); hardcoded here the same
/// way other suites hardcode the fixed metrics port 12090.
const BARE_PORT: u16 = 31952;

fn k8s_mode() -> bool {
    std::env::var("EDGION_TEST_K8S_MODE")
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn test_source(local_source: &'static str) -> &'static str {
    if k8s_mode() {
        "k8s-pod-peer"
    } else {
        local_source
    }
}

enum TcpEchoProbe {
    Echoed(usize),
    Closed(String),
    Inconclusive(String),
}

/// Connect FROM `src` (a loopback alias mapped to a synthetic country by the
/// local GeoIP test fixture) TO `addr`. K8s mode deliberately skips source
/// binding and uses the pod's real peer address and the K8s-only GeoIP map.
async fn probe_from(src: &str, addr: &str, test_data: &[u8]) -> TcpEchoProbe {
    if k8s_mode() {
        let mut stream = match tokio::time::timeout(Duration::from_secs(1), TcpStream::connect(addr)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(e)) => return TcpEchoProbe::Inconclusive(format!("connect failed: {e}")),
            Err(_) => return TcpEchoProbe::Inconclusive("connect timed out".to_string()),
        };
        return probe_stream(&mut stream, test_data).await;
    }

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

    probe_stream(&mut stream, test_data).await
}

async fn probe_stream(stream: &mut TcpStream, test_data: &[u8]) -> TcpEchoProbe {
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
/// A single connect-then-scrape is not safe here: a hot-swap's conf_sync
/// propagation lags the apply acknowledgement, so the very first probe can be
/// denied for a stale, unrelated reason from the previous row's still-active
/// config — satisfying a bare "was it closed?" check before propagation
/// completes. The client cannot distinguish "denied by geo" from "denied for
/// any other reason", so the metric bump is folded into the SAME retry loop
/// as the connection probe: only a `Closed` result whose immediately
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

/// For rows that are denied *without* ever reaching the geo stage (a chained
/// `GlobalConnectionIpRestriction` deny that runs before `GeoIpLocation`):
/// asserts the connection-level plugin's own
/// `edgion_conn_ip_restriction_events_total{result="denied"}` counter bumped,
/// proving the deny path actually fired rather than the test having silently
/// stopped exercising it — and also proves the geo-reason counters did NOT
/// bump (the geo stage was never reached, since the runtime stops at the
/// first `Deny` — see `StreamPluginRuntime::run`).
///
/// The geo-counter comparison is scoped to a single retry iteration
/// (snapshot immediately before that one probe, snapshot immediately after),
/// NOT to the whole retry loop: while a hot-swap is still propagating,
/// earlier iterations can land on the previous row's still-active config and
/// produce their own, unrelated geo denials — those must not be blamed on the
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
                             bumped during this exact connection — the geo stage should never have been reached \
                             for this row"
                        ));
                    }
                    return Ok(format!("{close_reason}, ip-restriction metric={ip_after}"));
                }
                last = format!(
                    "{close_reason} (edgion_conn_ip_restriction_events_total{{result=\"denied\"}} not yet bumped: \
                     {ip_after}, baseline {ip_before} — likely still denied by a stale prior config)"
                );
            }
            TcpEchoProbe::Echoed(n) => last = format!("received {n} echo bytes"),
            TcpEchoProbe::Inconclusive(reason) => last = reason,
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Poll `/configserver/EdgionStreamPlugins` until `tcp-geo-esp`'s `Accepted`
/// condition reaches `want_status`.
///
/// The controller uses lenient admission: a `preparse` error does NOT fail the
/// apply, it stores the resource and reports the problem as
/// `Accepted: False`. So an invalid config is asserted here, on the status
/// condition, not on `apply_esp`'s return value. Mirrors
/// suites/edgion_plugins/geo_restriction's `wait_for_accepted`.
async fn wait_for_esp_accepted(ctx: &TestContext, want_status: &str) -> Result<String, String> {
    let mut last = String::new();
    for _ in 0..40 {
        let mut req = ctx.http_client.get(format!(
            "{}/configserver/EdgionStreamPlugins?namespace=edgion-test&name=tcp-geo-esp",
            ctx.admin_api_url()
        ));
        if let Some(token) = &ctx.admin_token {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        match req.send().await {
            Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
                Ok(body) if body["success"].as_bool().unwrap_or(false) => {
                    if let Some(conditions) = controller_local_conditions(&body["data"]["status"]) {
                        if let Some(accepted) = conditions.iter().find(|c| c["type"].as_str() == Some("Accepted")) {
                            if accepted["status"].as_str() == Some(want_status) {
                                return Ok(accepted["message"].as_str().unwrap_or("").to_string());
                            }
                            last = format!("Accepted={accepted:?}");
                        } else {
                            last = "status.conditions lacks Accepted".to_string();
                        }
                    } else {
                        last = "status.conditions not present".to_string();
                    }
                }
                Ok(body) => last = format!("API error: {}", body["error"].as_str().unwrap_or("unknown")),
                Err(e) => last = format!("JSON parse error: {e}"),
            },
            Ok(resp) => last = format!("HTTP {}", resp.status()),
            Err(e) => last = format!("request error: {e}"),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(format!("Accepted never reached '{want_status}'; last: {last}"))
}

async fn apply_esp(ctx: &TestContext, plugins_yaml: &str) -> Result<(), String> {
    let rendered;
    let plugins_yaml = if k8s_mode() {
        rendered = crate::suites::k8s_peer_identity::render_geo_template(plugins_yaml)?;
        rendered.as_str()
    } else {
        plugins_yaml
    };
    let yaml = format!(
        r#"apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: tcp-geo-esp
  namespace: edgion-test
spec:
  plugins:
{plugins_yaml}
"#
    );
    ctx.apply_yaml(&yaml).await
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

/// `edgion_conn_ip_restriction_events_total{result="denied"}` — bumped by
/// `GlobalConnectionIpRestrictionPlugin::on_connection` for a terminal IP-deny
/// hit. Not layer-scoped (the plugin records it identically regardless of
/// which listener stage invoked it).
async fn conn_ip_restriction_denied_total(ctx: &TestContext) -> f64 {
    let metrics = crate::metrics_helper::MetricsClient::from_host_port(&ctx.target_host, 12090);
    let Ok(raw) = metrics.fetch_raw_metrics().await else {
        return 0.0;
    };
    crate::metrics_helper::counter_value(&raw, "edgion_conn_ip_restriction_events_total", &[("result", "denied")])
}

const GEO_DENY_RU: &str = "    - type: GeoIpLocation\n      config:\n        defaultAction: allow\n        failOpen: false\n        denyRuleGroups:\n          - name: block-ru\n            rules:\n              - source: DirectPeerIp\n                key: country\n                values: [\"RU\"]\n";

const GEO_DENY_K8S_PEER: &str = "    - type: GeoIpLocation\n      config:\n        defaultAction: allow\n        failOpen: false\n        denyRuleGroups:\n          - name: block-k8s-peer-country\n            rules:\n              - source: DirectPeerIp\n                key: country\n                values: [\"US\"]\n";

const GEO_ALLOW_CN: &str = "    - type: GeoIpLocation\n      config:\n        defaultAction: allow\n        failOpen: false\n        allowRuleGroups:\n          - name: cn-only\n            rules:\n              - source: DirectPeerIp\n                key: country\n                values: [\"CN\"]\n";

const GEO_ALLOW_K8S_PEER: &str = "    - type: GeoIpLocation\n      config:\n        defaultAction: allow\n        failOpen: false\n        allowRuleGroups:\n          - name: k8s-peer-only\n            rules:\n              - source: DirectPeerIp\n                key: country\n                values: [\"US\"]\n";

const GEO_DENY_CIDR_RU: &str = "    - type: GeoIpLocation\n      config:\n        defaultAction: allow\n        failOpen: false\n        denyRuleGroups:\n          - name: block-ru-cidr\n            rules:\n              - source: DirectPeerIp\n                key: cidr\n                values: [\"127.0.0.5/32\"]\n";

const GEO_DENY_K8S_CIDR: &str = "    - type: GeoIpLocation\n      config:\n        defaultAction: allow\n        failOpen: false\n        denyRuleGroups:\n          - name: block-k8s-cidr\n            rules:\n              - source: DirectPeerIp\n                key: cidr\n                values: [\"192.168.0.0/16\"]\n";

const GEO_DENY_OTHER_CIDR: &str = "    - type: GeoIpLocation\n      config:\n        defaultAction: allow\n        failOpen: false\n        denyRuleGroups:\n          - name: block-other-cidr\n            rules:\n              - source: DirectPeerIp\n                key: cidr\n                values: [\"10.0.0.0/8\"]\n";

const GEO_PARSE_ONLY: &str = "    - type: GeoIpLocation\n      config: {}\n";

const CHAIN_IP_DENY_THEN_GEO: &str = "    - type: GlobalConnectionIpRestriction\n      config:\n        enable: true\n        activeProfile: default\n        profiles:\n          default:\n            defaultAction: allow\n            deny:\n              - name: deny-ru-source\n                cidrs:\n                  - \"127.0.0.5/32\"\n    - type: GeoIpLocation\n      config:\n        defaultAction: allow\n        failOpen: false\n        denyRuleGroups:\n          - name: block-ru\n            rules:\n              - source: DirectPeerIp\n                key: country\n                values: [\"RU\"]\n";

const CHAIN_IP_DENY_THEN_GEO_K8S: &str = "    - type: GlobalConnectionIpRestriction\n      config:\n        enable: true\n        activeProfile: default\n        profiles:\n          default:\n            defaultAction: allow\n            deny:\n              - name: deny-k8s-source\n                cidrs:\n                  - \"192.168.0.0/16\"\n    - type: GeoIpLocation\n      config:\n        defaultAction: allow\n        failOpen: false\n        denyRuleGroups:\n          - name: block-k8s-peer-country\n            rules:\n              - source: DirectPeerIp\n                key: country\n                values: [\"US\"]\n";

const CHAIN_ALLOW_K8S: &str = "    - type: GlobalConnectionIpRestriction\n      config:\n        enable: true\n        activeProfile: default\n        profiles:\n          default:\n            defaultAction: allow\n            deny:\n              - name: deny-other-source\n                cidrs:\n                  - \"10.0.0.0/8\"\n    - type: GeoIpLocation\n      config:\n        defaultAction: allow\n        failOpen: false\n        denyRuleGroups:\n          - name: block-ru\n            rules:\n              - source: DirectPeerIp\n                key: country\n                values: [\"RU\"]\n";

const GCIR_ONLY_LOOPBACK_ALLOW: &str = "    - type: GlobalConnectionIpRestriction\n      config:\n        enable: true\n        activeProfile: default\n        profiles:\n          default:\n            defaultAction: deny\n            allow:\n              - name: loopback\n                cidrs:\n                  - \"127.0.0.0/8\"\n";

const GCIR_ONLY_K8S_ALLOW: &str = "    - type: GlobalConnectionIpRestriction\n      config:\n        enable: true\n        activeProfile: default\n        profiles:\n          default:\n            defaultAction: deny\n            allow:\n              - name: k8s-pod-network\n                cidrs:\n                  - \"192.168.0.0/16\"\n";

impl TcpGeoRestrictionTestSuite {
    /// A bare `denyRuleGroups`/country hit denies at the "tcp" layer, and
    /// bumps `edgion_geoip_denied_total{layer="tcp",reason="deny_country"}`.
    fn geo_deny_country() -> TestCase {
        TestCase::new(
            "geo_deny_country",
            "a denyRuleGroups/country hit denies at the tcp layer",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let config = if k8s_mode() { GEO_DENY_K8S_PEER } else { GEO_DENY_RU };
                    if let Err(e) = apply_esp(&ctx, config).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let addr = ctx.tcp_addr();
                    match wait_for_geo_denial(&ctx, test_source("127.0.0.5"), &addr, b"deny", "tcp", "deny_country")
                        .await
                    {
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

    /// A whitelist (`allowRuleGroups`) hit passes.
    fn geo_allow_whitelist_hit_passes() -> TestCase {
        TestCase::new(
            "geo_allow_whitelist_hit_passes",
            "an allowRuleGroups hit passes",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let config = if k8s_mode() { GEO_ALLOW_K8S_PEER } else { GEO_ALLOW_CN };
                    if let Err(e) = apply_esp(&ctx, config).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let addr = ctx.tcp_addr();
                    match wait_for_echo(test_source("127.0.0.1"), &addr, b"allow-hit").await {
                        Ok(n) => TestResult::passed_with_message(start.elapsed(), format!("allowed, echoed {n} bytes")),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Same whitelist, a miss denies with reason `allow_miss` even under
    /// `defaultAction: allow`.
    fn geo_allow_whitelist_miss_denies() -> TestCase {
        TestCase::new(
            "geo_allow_whitelist_miss_denies",
            "an allowRuleGroups miss denies (reason=allow_miss) even under defaultAction: allow",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(e) = apply_esp(&ctx, GEO_ALLOW_CN).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let addr = ctx.tcp_addr();
                    match wait_for_geo_denial(
                        &ctx,
                        test_source("127.0.0.6"),
                        &addr,
                        b"allow-miss",
                        "tcp",
                        "allow_miss",
                    )
                    .await
                    {
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

    /// A `key: cidr` rule matches by IP membership, independent of geo data.
    fn geo_cidr_rule_matches() -> TestCase {
        TestCase::new(
            "geo_cidr_rule_matches",
            "a key: cidr denyRuleGroups rule matches by IP membership",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let config = if k8s_mode() {
                        GEO_DENY_K8S_CIDR
                    } else {
                        GEO_DENY_CIDR_RU
                    };
                    if let Err(e) = apply_esp(&ctx, config).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let addr = ctx.tcp_addr();
                    if let Err(e) = wait_for_geo_denial(
                        &ctx,
                        test_source("127.0.0.5"),
                        &addr,
                        b"cidr-hit",
                        "tcp",
                        "deny_country",
                    )
                    .await
                    {
                        return TestResult::failed(start.elapsed(), format!("in-CIDR: {e}"));
                    }
                    if k8s_mode() {
                        if let Err(e) = apply_esp(&ctx, GEO_DENY_OTHER_CIDR).await {
                            return TestResult::failed(start.elapsed(), format!("apply out-of-CIDR config: {e}"));
                        }
                    }
                    match wait_for_echo(test_source("127.0.0.1"), &addr, b"cidr-miss").await {
                        Ok(n) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("out-of-CIDR source allowed, echoed {n} bytes"),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), format!("out-of-CIDR: {e}")),
                    }
                })
            },
        )
    }

    /// Parse-only (`config: {}`) reports invalid status and installs a live
    /// fail-closed gate. A valid replacement must restore TCP forwarding.
    fn parse_only_is_rejected_at_l4() -> TestCase {
        TestCase::new(
            "parse_only_is_rejected_at_l4",
            "invalid L4 geo config denies TCP traffic and a valid replacement restores forwarding",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let src = test_source("127.0.0.1");
                    let addr = ctx.tcp_addr();
                    // CIDR-only rules allow this source without depending on MMDB.
                    let baseline = if k8s_mode() { GEO_DENY_OTHER_CIDR } else { GEO_DENY_CIDR_RU };
                    let check = async {
                        apply_esp(&ctx, baseline).await?;
                        wait_for_esp_accepted(&ctx, "True").await?;
                        wait_for_echo(src, &addr, b"invalid-geo-baseline").await?;

                        // Lenient admission reports invalidity through status; it
                        // does not prove that the Gateway has activated the gate.
                        apply_esp(&ctx, GEO_PARSE_ONLY).await?;
                        let msg = wait_for_esp_accepted(&ctx, "False").await?;
                        if !msg.contains("at least one") {
                            return Err(format!("Accepted=False, but not for the rule-less reason: {msg}"));
                        }

                        let mut last = "no TCP probe completed".to_string();
                        let denied = async {
                            loop {
                                match probe_from(src, &addr, b"invalid-geo-denial").await {
                                    TcpEchoProbe::Closed(reason) => return reason,
                                    // conf_sync may still be serving the proven
                                    // allow baseline; wait for its replacement.
                                    TcpEchoProbe::Echoed(n) => last = format!("received {n} echo bytes"),
                                    TcpEchoProbe::Inconclusive(reason) => last = reason,
                                }
                                tokio::time::sleep(Duration::from_millis(200)).await;
                            }
                        };
                        tokio::time::timeout(Duration::from_secs(8), denied)
                            .await
                            .map_err(|_| format!("invalid L4 geo config did not close TCP connections within 8s; last: {last}"))
                    };
                    let result = tokio::time::timeout(Duration::from_secs(40), check)
                        .await
                        .unwrap_or_else(|_| Err("invalid-generation verification timed out after 40s".to_string()));

                    // Always restore, even after an apply/status/probe failure.
                    // Recovery is also the positive control against a broken
                    // listener/backend being mistaken for a fail-closed gate.
                    let restore = async {
                        apply_esp(&ctx, baseline).await?;
                        wait_for_esp_accepted(&ctx, "True").await?;
                        wait_for_echo(src, &addr, b"invalid-geo-recovered").await
                    };
                    let recovery = tokio::time::timeout(Duration::from_secs(30), restore)
                        .await
                        .unwrap_or_else(|_| Err("baseline restoration timed out after 30s".to_string()));
                    match (result, recovery) {
                        (Ok(reason), Ok(n)) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("Accepted=False for rule-less config; denied ({reason}); valid replacement echoed {n} bytes"),
                        ),
                        (Err(e), Ok(_)) => TestResult::failed(start.elapsed(), e),
                        (Ok(_), Err(e)) => TestResult::failed(start.elapsed(), format!("recovery/cleanup: {e}")),
                        (Err(e), Err(cleanup)) => {
                            TestResult::failed(start.elapsed(), format!("{e}; recovery/cleanup: {cleanup}"))
                        }
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(75))
    }

    /// Two-plugin chain: `GlobalConnectionIpRestriction` (first) denies a
    /// matching source before `GeoIpLocation` (second) ever runs — no geo
    /// counter bump.
    fn chain_ip_deny_precedes_geo() -> TestCase {
        TestCase::new(
            "chain_ip_deny_precedes_geo",
            "a chained GlobalConnectionIpRestriction deny is terminal; GeoIpLocation never runs",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let config = if k8s_mode() {
                        CHAIN_IP_DENY_THEN_GEO_K8S
                    } else {
                        CHAIN_IP_DENY_THEN_GEO
                    };
                    if let Err(e) = apply_esp(&ctx, config).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let addr = ctx.tcp_addr();
                    match wait_for_non_geo_denial(&ctx, test_source("127.0.0.5"), &addr, b"chain-ip-deny").await {
                        Ok(msg) => TestResult::passed_with_message(start.elapsed(), format!("denied ({msg})")),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Same chain, a source matching neither stage passes both.
    fn chain_allows_both() -> TestCase {
        TestCase::new(
            "chain_allows_both",
            "a source matching neither the IP nor the geo stage is allowed",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let config = if k8s_mode() {
                        CHAIN_ALLOW_K8S
                    } else {
                        CHAIN_IP_DENY_THEN_GEO
                    };
                    if let Err(e) = apply_esp(&ctx, config).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let addr = ctx.tcp_addr();
                    let result = match wait_for_echo(test_source("127.0.0.1"), &addr, b"chain-allow").await {
                        Ok(n) => TestResult::passed_with_message(start.elapsed(), format!("allowed, echoed {n} bytes")),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    };
                    // Restore a known-good baseline for whatever runs next.
                    let baseline = if k8s_mode() { GEO_DENY_K8S_PEER } else { GEO_DENY_RU };
                    let _ = apply_esp(&ctx, baseline).await;
                    result
                })
            },
        )
    }

    /// No `GeoIpLocation` plugin at all: byte-for-byte plain
    /// GlobalConnectionIpRestriction behavior.
    fn no_geo_plugin_unchanged() -> TestCase {
        TestCase::new(
            "no_geo_plugin_unchanged",
            "an ESP with no GeoIpLocation entry behaves exactly as plain GlobalConnectionIpRestriction",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let config = if k8s_mode() {
                        GCIR_ONLY_K8S_ALLOW
                    } else {
                        GCIR_ONLY_LOOPBACK_ALLOW
                    };
                    if let Err(e) = apply_esp(&ctx, config).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let addr = ctx.tcp_addr();
                    let result = match wait_for_echo(test_source("127.0.0.1"), &addr, b"no-geo").await {
                        Ok(n) => TestResult::passed_with_message(start.elapsed(), format!("allowed, echoed {n} bytes")),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    };
                    // Restore a known-good baseline for whatever runs next.
                    let baseline = if k8s_mode() { GEO_DENY_K8S_PEER } else { GEO_DENY_RU };
                    let _ = apply_esp(&ctx, baseline).await;
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
                    let config = if k8s_mode() { GEO_DENY_K8S_PEER } else { GEO_DENY_RU };
                    if let Err(e) = apply_esp(&ctx, config).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let addr = format!("{}:{}", ctx.target_host, BARE_PORT);
                    // The bare listener has no backend to echo from; a geo deny at
                    // accept time is observed as connect-then-immediate-EOF (no
                    // route ever gets a chance to proxy), same shape as a Stage-2
                    // denial.
                    match wait_for_geo_denial(&ctx, test_source("127.0.0.5"), &addr, b"bare", "stage1", "deny_country")
                        .await
                    {
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
            TcpGeoRestrictionTestSuite::geo_deny_country(),
            TcpGeoRestrictionTestSuite::geo_allow_whitelist_hit_passes(),
            TcpGeoRestrictionTestSuite::geo_allow_whitelist_miss_denies(),
            TcpGeoRestrictionTestSuite::geo_cidr_rule_matches(),
            TcpGeoRestrictionTestSuite::parse_only_is_rejected_at_l4(),
            TcpGeoRestrictionTestSuite::chain_ip_deny_precedes_geo(),
            TcpGeoRestrictionTestSuite::chain_allows_both(),
            TcpGeoRestrictionTestSuite::no_geo_plugin_unchanged(),
            TcpGeoRestrictionTestSuite::bare_listener_is_stage1(),
        ]
    }
}
