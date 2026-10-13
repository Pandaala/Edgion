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

    read_exact_echo(stream, test_data).await
}

async fn read_exact_echo<R: tokio::io::AsyncRead + Unpin>(reader: &mut R, expected: &[u8]) -> TcpEchoProbe {
    let read = async {
        let mut received = vec![0; expected.len()];
        let mut offset = 0;
        while offset < expected.len() {
            match reader.read(&mut received[offset..]).await {
                Ok(0) => return TcpEchoProbe::Closed("connection closed before complete echo".to_string()),
                Ok(n) => {
                    offset += n;
                    if received[..offset] != expected[..offset] {
                        return TcpEchoProbe::Inconclusive("received bytes differ from expected echo".to_string());
                    }
                }
                Err(e) => return TcpEchoProbe::Closed(format!("read failed: {e}")),
            }
        }
        TcpEchoProbe::Echoed(offset)
    };
    match tokio::time::timeout(Duration::from_secs(1), read).await {
        Ok(verdict) => verdict,
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

/// Load one explicit suite-owned collection; consumer application stays separate.
async fn apply_chain_list(ctx: &TestContext, relative_fixture: &str) -> Result<(), String> {
    let root = if k8s_mode() { "k8s" } else { "integration" };
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(root)
        .join("conf/TCPRoute/GeoRestriction")
        .join(relative_fixture);
    let yaml = tokio::fs::read_to_string(path)
        .await
        .map_err(|error| error.to_string())?;
    let yaml = if k8s_mode() {
        crate::suites::k8s_peer_identity::render_geo_template(&yaml)?
    } else {
        yaml
    };
    ctx.apply_yaml(&yaml).await
}

/// Restore both original inputs, even if one attempt fails or times out.
async fn cleanup_chain_resources(ctx: &TestContext, relative_fixture: &str) -> Result<(), String> {
    let list = tokio::time::timeout(Duration::from_secs(12), apply_chain_list(ctx, relative_fixture))
        .await
        .unwrap_or_else(|_| Err("chain list cleanup timed out after 12s".into()));
    let consumer = tokio::time::timeout(Duration::from_secs(12), restore_geo_fixture(ctx))
        .await
        .unwrap_or_else(|_| Err("chain consumer cleanup timed out after 12s".into()));
    match (list, consumer) {
        (Ok(()), Ok(())) => Ok(()),
        (list, consumer) => Err(format!("list cleanup={list:?}; consumer cleanup={consumer:?}")),
    }
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

const GEO_DENY_CIDR_RU: &str = "    - type: GeoIpLocation\n      config:\n        defaultAction: allow\n        failOpen: false\n        denyRuleGroups:\n          - name: block-ru-cidr\n            rules:\n              - source: DirectPeerIp\n                key: cidr\n                ipRefs: [{name: geo-stream-cidrs}]\n";

const GEO_DENY_K8S_CIDR: &str = "    - type: GeoIpLocation\n      config:\n        defaultAction: allow\n        failOpen: false\n        denyRuleGroups:\n          - name: block-k8s-cidr\n            rules:\n              - source: DirectPeerIp\n                key: cidr\n                ipRefs: [{name: geo-stream-cidrs}]\n";

const GEO_DENY_OTHER_CIDR: &str = "    - type: GeoIpLocation\n      config:\n        defaultAction: allow\n        failOpen: false\n        denyRuleGroups:\n          - name: block-other-cidr\n            rules:\n              - source: DirectPeerIp\n                key: cidr\n                ipRefs: [{name: geo-stream-cidrs}]\n";

const GEO_PARSE_ONLY: &str = "    - type: GeoIpLocation\n      config: {}\n";

const CHAIN_IP_DENY_THEN_GEO: &str = "    - type: GlobalConnectionIpRestriction\n      config:\n        enable: true\n        activeProfile: default\n        profiles:\n          default:\n            defaultAction: allow\n            denyRefs:\n              - name: geo-stream-chain-deny\n    - type: GeoIpLocation\n      config:\n        defaultAction: allow\n        failOpen: false\n        denyRuleGroups:\n          - name: block-ru\n            rules:\n              - source: DirectPeerIp\n                key: country\n                values: [\"RU\"]\n";

const CHAIN_IP_DENY_THEN_GEO_K8S: &str = "    - type: GlobalConnectionIpRestriction\n      config:\n        enable: true\n        activeProfile: default\n        profiles:\n          default:\n            defaultAction: allow\n            denyRefs:\n              - name: geo-stream-chain-deny\n    - type: GeoIpLocation\n      config:\n        defaultAction: allow\n        failOpen: false\n        denyRuleGroups:\n          - name: block-k8s-peer-country\n            rules:\n              - source: DirectPeerIp\n                key: country\n                values: [\"US\"]\n";

const CHAIN_ALLOW_K8S: &str = "    - type: GlobalConnectionIpRestriction\n      config:\n        enable: true\n        activeProfile: default\n        profiles:\n          default:\n            defaultAction: allow\n            denyRefs:\n              - name: geo-stream-chain-deny-other\n    - type: GeoIpLocation\n      config:\n        defaultAction: allow\n        failOpen: false\n        denyRuleGroups:\n          - name: block-ru\n            rules:\n              - source: DirectPeerIp\n                key: country\n                values: [\"RU\"]\n";

const GCIR_ONLY_LOOPBACK_ALLOW: &str = "    - type: GlobalConnectionIpRestriction\n      config:\n        enable: true\n        activeProfile: default\n        profiles:\n          default:\n            defaultAction: deny\n            allowRefs:\n              - name: geo-stream-loopback-allow\n";

const GCIR_ONLY_K8S_ALLOW: &str = "    - type: GlobalConnectionIpRestriction\n      config:\n        enable: true\n        activeProfile: default\n        profiles:\n          default:\n            defaultAction: deny\n            allowRefs:\n              - name: geo-stream-loopback-allow\n";

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
                    let addr = ctx.tcp_addr();
                    let hit = if k8s_mode() { "192.168.0.0/16" } else { "127.0.0.5/32" };
                    let config = if k8s_mode() {
                        GEO_DENY_K8S_CIDR
                    } else {
                        GEO_DENY_CIDR_RU
                    };
                    let check = tokio::time::timeout(Duration::from_secs(85), async {
                        apply_geo_cidrs(&ctx, hit).await?;
                        apply_esp(&ctx, config).await?;
                        wait_for_geo_denial(
                            &ctx,
                            test_source("127.0.0.5"),
                            &addr,
                            b"cidr-hit",
                            "tcp",
                            "deny_country",
                        )
                        .await?;
                        if k8s_mode() {
                            apply_geo_cidrs(&ctx, "10.0.0.0/8").await?;
                            apply_esp(&ctx, GEO_DENY_OTHER_CIDR).await?;
                        }
                        wait_for_echo(test_source("127.0.0.1"), &addr, b"cidr-miss").await
                    })
                    .await
                    .unwrap_or_else(|_| {
                        Err("Geo verification timed out after 85s; cleanup remains reserved".to_string())
                    });
                    let cleanup = cleanup_geo_resources(&ctx, hit).await;
                    match (check, cleanup) {
                        (Ok(n), Ok(())) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("out-of-CIDR source allowed, echoed {n} bytes; original resources restored"),
                        ),
                        (result, cleanup) => {
                            TestResult::failed(start.elapsed(), format!("verification={result:?}; cleanup={cleanup:?}"))
                        }
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(120))
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
                        apply_geo_cidrs(&ctx, if k8s_mode() { "10.0.0.0/8" } else { "127.0.0.5/32" }).await?;
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
                        apply_geo_cidrs(&ctx, if k8s_mode() { "10.0.0.0/8" } else { "127.0.0.5/32" }).await?;
                        apply_esp(&ctx, baseline).await?;
                        wait_for_esp_accepted(&ctx, "True").await?;
                        wait_for_echo(src, &addr, b"invalid-geo-recovered").await
                    };
                    let recovery = tokio::time::timeout(Duration::from_secs(30), restore)
                        .await
                        .unwrap_or_else(|_| Err("baseline restoration timed out after 30s".to_string()));
                    let original_cleanup = cleanup_geo_resources(&ctx, if k8s_mode() { "192.168.0.0/16" } else { "127.0.0.5/32" }).await;
                    if let Err(error) = original_cleanup {
                        return TestResult::failed(start.elapsed(), format!("verification={result:?}; recovery={recovery:?}; original cleanup: {error}"));
                    }
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
        .with_timeout(Duration::from_secs(105))
    }

    /// Two-plugin chain: `GlobalConnectionIpRestriction` (first) denies a
    /// matching source before `GeoIpLocation` (second) ever runs — no geo
    /// counter bump.
    fn chain_ip_deny_precedes_geo() -> TestCase {
        TestCase::new(
            "chain_ip_deny_precedes_geo",
            "first plugin (GlobalConnectionIpRestriction) denies before the GeoIpLocation entry",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let config = if k8s_mode() {
                        CHAIN_IP_DENY_THEN_GEO_K8S
                    } else {
                        CHAIN_IP_DENY_THEN_GEO
                    };
                    let fixture = "00_EdgionConfigData_geo-stream-chain-deny.yaml";
                    let check = tokio::time::timeout(Duration::from_secs(30), async {
                        apply_chain_list(&ctx, fixture).await?;
                        apply_esp(&ctx, config).await?;
                        wait_for_esp_accepted(&ctx, "True").await?;
                        let addr = ctx.tcp_addr();
                        wait_for_non_geo_denial(&ctx, test_source("127.0.0.5"), &addr, b"chain-ip-deny")
                            .await
                            .map(|message| format!("denied ({message})"))
                    })
                    .await
                    .unwrap_or_else(|_| Err("chain verification timed out after 30s".into()));
                    let cleanup = cleanup_chain_resources(&ctx, fixture).await;
                    match (check, cleanup) {
                        (Ok(message), Ok(())) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("{message}; original resources restored"),
                        ),
                        (check, cleanup) => {
                            TestResult::failed(start.elapsed(), format!("verification={check:?}; cleanup={cleanup:?}"))
                        }
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(65))
    }

    /// Same chain, a source matching neither stage passes both.
    fn chain_allows_both() -> TestCase {
        TestCase::new(
            "chain_allows_both",
            "source outside IP deny and geo deny is allowed by the chain",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let config = if k8s_mode() {
                        CHAIN_ALLOW_K8S
                    } else {
                        CHAIN_IP_DENY_THEN_GEO
                    };
                    let fixture = if k8s_mode() {
                        "00_EdgionConfigData_geo-stream-chain-deny-other.yaml"
                    } else {
                        "00_EdgionConfigData_geo-stream-chain-deny.yaml"
                    };
                    let check = tokio::time::timeout(Duration::from_secs(30), async {
                        apply_chain_list(&ctx, fixture).await?;
                        apply_esp(&ctx, config).await?;
                        wait_for_esp_accepted(&ctx, "True").await?;
                        let addr = ctx.tcp_addr();
                        wait_for_echo(test_source("127.0.0.1"), &addr, b"chain-allow")
                            .await
                            .map(|n| format!("allowed, echoed {n} bytes"))
                    })
                    .await
                    .unwrap_or_else(|_| Err("chain verification timed out after 30s".into()));
                    let cleanup = cleanup_chain_resources(&ctx, fixture).await;
                    match (check, cleanup) {
                        (Ok(message), Ok(())) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("{message}; original resources restored"),
                        ),
                        (check, cleanup) => {
                            TestResult::failed(start.elapsed(), format!("verification={check:?}; cleanup={cleanup:?}"))
                        }
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(65))
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
                    let fixture = "00_EdgionConfigData_geo-stream-loopback-allow.yaml";
                    let check = tokio::time::timeout(Duration::from_secs(30), async {
                        apply_chain_list(&ctx, fixture).await?;
                        apply_esp(&ctx, config).await?;
                        wait_for_esp_accepted(&ctx, "True").await?;
                        let addr = ctx.tcp_addr();
                        wait_for_echo(test_source("127.0.0.1"), &addr, b"no-geo")
                            .await
                            .map(|n| format!("allowed, echoed {n} bytes"))
                    })
                    .await
                    .unwrap_or_else(|_| Err("chain verification timed out after 30s".into()));
                    let cleanup = cleanup_chain_resources(&ctx, fixture).await;
                    match (check, cleanup) {
                        (Ok(message), Ok(())) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("{message}; original resources restored"),
                        ),
                        (check, cleanup) => {
                            TestResult::failed(start.elapsed(), format!("verification={check:?}; cleanup={cleanup:?}"))
                        }
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(65))
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
            TcpGeoRestrictionTestSuite::live_ip_list_and_invalid_generation_recovery(),
            TcpGeoRestrictionTestSuite::parse_only_is_rejected_at_l4(),
            TcpGeoRestrictionTestSuite::chain_ip_deny_precedes_geo(),
            TcpGeoRestrictionTestSuite::chain_allows_both(),
            TcpGeoRestrictionTestSuite::no_geo_plugin_unchanged(),
            TcpGeoRestrictionTestSuite::bare_listener_is_stage1(),
        ]
    }
}

/// Update the explicit list independently of the ref-only consuming plugin.
async fn apply_geo_cidrs(ctx: &TestContext, cidr: &str) -> Result<(), String> {
    apply_geo_cidrs_enabled(ctx, cidr, true).await
}
async fn apply_geo_cidrs_enabled(ctx: &TestContext, cidr: &str, enabled: bool) -> Result<(), String> {
    let yaml = format!(
        r#"apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: geo-stream-cidrs
  namespace: edgion-test
spec:
  enable: {enabled}
  data:
    type: IpList
    config:
      items:
        - name: test-peer
          cidrs: ["{cidr}"]
"#
    );
    let yaml = if k8s_mode() {
        crate::suites::k8s_peer_identity::render_geo_template(&yaml)?
    } else {
        yaml
    };
    ctx.apply_yaml(&yaml).await
}

impl TcpGeoRestrictionTestSuite {
    fn live_ip_list_and_invalid_generation_recovery() -> TestCase {
        TestCase::new(
            "live_ip_list_and_invalid_generation_recovery",
            "Stream Geo observes list-only changes and unavailable references cannot failOpen",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let hit = if k8s_mode() { "192.168.0.0/16" } else { "127.0.0.5/32" };
                    let miss = if k8s_mode() { "10.0.0.0/8" } else { "127.0.0.1/32" };
                    let config = if k8s_mode() {
                        GEO_DENY_K8S_CIDR
                    } else {
                        GEO_DENY_CIDR_RU
                    };
                    let config = config.replace("failOpen: false", "failOpen: true");
                    let src = test_source("127.0.0.5");
                    let addr = ctx.tcp_addr();
                    let check = tokio::time::timeout(Duration::from_secs(85), async {
                apply_geo_cidrs(&ctx,hit).await?; apply_esp(&ctx,&config).await?;
                wait_for_geo_denial(&ctx,src,&addr,b"geo-live-hit","tcp","deny_country").await?;
                apply_geo_cidrs(&ctx,miss).await?;
                wait_for_echo(src,&addr,b"geo-live-miss").await?;
                ctx.delete_resource("EdgionConfigData","edgion-test","geo-stream-cidrs").await?;
                wait_for_geo_denial(&ctx,src,&addr,b"geo-live-missing","tcp","ref_unavailable").await?;
                apply_geo_cidrs(&ctx,hit).await?;
                wait_for_geo_denial(&ctx,src,&addr,b"geo-live-restored","tcp","deny_country").await?;
                apply_geo_cidrs_enabled(&ctx,hit,false).await?;
                wait_for_geo_denial(&ctx,src,&addr,b"geo-live-disabled","tcp","ref_unavailable").await?;
                ctx.apply_yaml("apiVersion: edgion.io/v1\nkind: EdgionConfigData\nmetadata:\n  name: geo-stream-cidrs\n  namespace: edgion-test\nspec:\n  data:\n    type: Misc\n    config: {}\n").await?;
                wait_for_geo_denial(&ctx,src,&addr,b"geo-live-wrong-type","tcp","ref_unavailable").await?;
                apply_geo_cidrs(&ctx,hit).await?;
                wait_for_geo_denial(&ctx,src,&addr,b"geo-live-clean","tcp","deny_country").await?;
                apply_geo_cidrs(&ctx,miss).await?;
                wait_for_echo(src,&addr,b"geo-live-valid-control").await?;
                let invalid=format!("{config}                values: null\n");
                apply_esp(&ctx,&invalid).await?;
                wait_for_esp_accepted(&ctx,"False").await?;
                let denied=async {
                    loop {
                        match probe_from(src,&addr,b"geo-invalid-live").await {
                            TcpEchoProbe::Closed(_)=>return Ok(()),
                            TcpEchoProbe::Echoed(_)=>{},
                            TcpEchoProbe::Inconclusive(reason)=>return Err(reason),
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                };
                tokio::time::timeout(Duration::from_secs(8),denied).await.map_err(|_|"typed-invalid did not close connection".to_string())??;
                apply_esp(&ctx,&config).await?;
                wait_for_esp_accepted(&ctx,"True").await?;
                wait_for_echo(src,&addr,b"geo-live-clean-control").await.map(|_|())
            })
                        .await
                        .unwrap_or_else(|_| Err("Geo verification timed out after 85s; cleanup remains reserved".to_string()));
                    let cleanup = cleanup_geo_resources(&ctx, hit).await;
                    match (check, cleanup) {
                        (Ok(()), Ok(())) => TestResult::passed_with_message(
                            start.elapsed(),
                            "live reference denial/recovery and explicit cleanup".into(),
                        ),
                        (result, cleanup) => {
                            TestResult::failed(start.elapsed(), format!("verification={result:?}; cleanup={cleanup:?}"))
                        }
                    }
                })
            },
        ).with_timeout(Duration::from_secs(120))
    }
}

/// Restore the exact original native/K8s resource, including group names and adjacent ACLs.
async fn restore_geo_fixture(ctx: &TestContext) -> Result<(), String> {
    use serde::Deserialize;
    let source = if k8s_mode() {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/k8s/conf/TCPRoute/GeoRestriction/resources.yaml"
        ))
    } else {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/conf/TCPRoute/GeoRestriction/01_EdgionStreamPlugins_tcp_geo.yaml"
        ))
    };
    let yaml = {
        let mut selected = None;
        for document in serde_yaml::Deserializer::from_str(source) {
            let value = serde_yaml::Value::deserialize(document).map_err(|error| error.to_string())?;
            if value["kind"].as_str() == Some("EdgionStreamPlugins")
                && value["metadata"]["name"].as_str() == Some("tcp-geo-esp")
            {
                selected = Some(serde_yaml::to_string(&value).map_err(|error| error.to_string())?);
                break;
            }
        }
        selected.ok_or_else(|| "original Geo fixture resource is missing".to_string())?
    };
    ctx.apply_yaml(&yaml).await
}

/// Attempt both original resource restorations even if one fails or times out.
async fn cleanup_geo_resources(ctx: &TestContext, cidr: &str) -> Result<(), String> {
    let list = tokio::time::timeout(Duration::from_secs(12), apply_geo_cidrs(ctx, cidr))
        .await
        .unwrap_or_else(|_| Err("list cleanup timed out after 12s".into()));
    let consumer = tokio::time::timeout(Duration::from_secs(12), restore_geo_fixture(ctx))
        .await
        .unwrap_or_else(|_| Err("consumer cleanup timed out after 12s".into()));
    match (list, consumer) {
        (Ok(()), Ok(())) => Ok(()),
        (list, consumer) => Err(format!("list cleanup={list:?}; consumer cleanup={consumer:?}")),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn actual_kubernetes_geo_renderer_preserves_lists_and_proves_peer_hit_miss() {
        const CHILD: &str = "EDGION_POST_RUN_GEO_RENDER_CHILD";
        if std::env::var(CHILD).as_deref() != Ok("1") {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "suites::tcp_route::geo_restriction::geo_restriction::tests::actual_kubernetes_geo_renderer_preserves_lists_and_proves_peer_hit_miss", "--nocapture"])
                .env(CHILD, "1")
                .env("EDGION_TEST_K8S_PEER_UID", "prepared-peer-uid")
                .env("EDGION_TEST_K8S_PEER_IP", "192.168.99.42")
                .env("EDGION_TEST_K8S_PEER_HIT_CIDR", "192.168.99.42/32")
                .env("EDGION_TEST_K8S_PEER_MISS_CIDR", "198.51.100.0/24")
                .status().unwrap();
            assert!(status.success());
            return;
        }
        for (file, hit) in [
            ("geo-stream-chain-deny", true),
            ("geo-stream-chain-deny-other", false),
            ("geo-stream-loopback-allow", true),
        ] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
                "k8s/conf/TCPRoute/GeoRestriction/00_EdgionConfigData_{file}.yaml"
            ));
            let template = std::fs::read_to_string(path).unwrap();
            let rendered = crate::suites::k8s_peer_identity::render_geo_template(&template).unwrap();
            let _: edgion_resources::EdgionConfigData = serde_yaml::from_str(&rendered).unwrap();
            let mut expected: serde_yaml::Value = serde_yaml::from_str(&template).unwrap();
            expected["spec"]["data"]["config"]["items"][0]["cidrs"][0] =
                serde_yaml::Value::String(if hit { "192.168.99.42/32" } else { "198.51.100.0/24" }.into());
            let actual: serde_yaml::Value = serde_yaml::from_str(&rendered).unwrap();
            assert_eq!(actual, expected);
            let groups: Vec<edgion_resources::resources::edgion_config_data::IpGroup> =
                serde_yaml::from_value(actual["spec"]["data"]["config"]["items"].clone()).unwrap();
            let matcher = edgion_resources::matcher::ip_cidr_helpers::build_grouped_matcher(&groups).unwrap();
            assert_eq!(matcher.contains(&"192.168.99.42".parse().unwrap()), hit);
        }
    }

    #[tokio::test]
    async fn tcp_echo_requires_complete_exact_payload_across_partial_reads() {
        let (mut writer, mut reader) = tokio::io::duplex(3);
        let send = tokio::spawn(async move {
            writer.write_all(b"geo").await.unwrap();
            tokio::task::yield_now().await;
            writer.write_all(b"-probe").await.unwrap();
        });
        assert!(matches!(
            super::read_exact_echo(&mut reader, b"geo-probe").await,
            super::TcpEchoProbe::Echoed(9)
        ));
        send.await.unwrap();
    }

    #[tokio::test]
    async fn tcp_echo_rejects_wrong_payload_without_classifying_it_as_denial() {
        let mut wrong = &b"different"[..];
        assert!(matches!(
            super::read_exact_echo(&mut wrong, b"geo-probe").await,
            super::TcpEchoProbe::Inconclusive(_)
        ));
        let mut partial = &b"geo"[..];
        assert!(matches!(
            super::read_exact_echo(&mut partial, b"geo-probe").await,
            super::TcpEchoProbe::Closed(_)
        ));
    }

    use super::*;

    #[test]
    fn generated_gcir_chain_policies_have_exact_owning_lists_and_no_legacy_fields() {
        for (root, raw, fixture, reference, field, group, cidr) in [
            (
                "integration",
                CHAIN_IP_DENY_THEN_GEO,
                "00_EdgionConfigData_geo-stream-chain-deny.yaml",
                "geo-stream-chain-deny",
                "denyRefs",
                "deny-ru-source",
                "127.0.0.5/32",
            ),
            (
                "k8s",
                CHAIN_IP_DENY_THEN_GEO_K8S,
                "00_EdgionConfigData_geo-stream-chain-deny.yaml",
                "geo-stream-chain-deny",
                "denyRefs",
                "deny-k8s-source",
                "192.168.0.0/16",
            ),
            (
                "k8s",
                CHAIN_ALLOW_K8S,
                "00_EdgionConfigData_geo-stream-chain-deny-other.yaml",
                "geo-stream-chain-deny-other",
                "denyRefs",
                "deny-other-source",
                "10.0.0.0/8",
            ),
            (
                "integration",
                GCIR_ONLY_LOOPBACK_ALLOW,
                "00_EdgionConfigData_geo-stream-loopback-allow.yaml",
                "geo-stream-loopback-allow",
                "allowRefs",
                "loopback",
                "127.0.0.0/8",
            ),
            (
                "k8s",
                GCIR_ONLY_K8S_ALLOW,
                "00_EdgionConfigData_geo-stream-loopback-allow.yaml",
                "geo-stream-loopback-allow",
                "allowRefs",
                "k8s-pod-network",
                "192.168.0.0/16",
            ),
        ] {
            let yaml = format!("apiVersion: edgion.io/v1\nkind: EdgionStreamPlugins\nmetadata:\n  name: tcp-geo-esp\n  namespace: edgion-test\nspec:\n  plugins:\n{raw}");
            let consumer: edgion_resources::EdgionStreamPlugins = serde_yaml::from_str(&yaml).unwrap();
            use edgion_resources::resources::edgion_plugins::PluginValidation;
            use edgion_resources::resources::edgion_stream_plugins::EdgionStreamPlugin;
            assert!(consumer.unsupported_plugin_errors().is_empty());
            for entry in consumer.spec.plugins.iter().flatten() {
                match &entry.plugin {
                    EdgionStreamPlugin::GlobalConnectionIpRestriction(config) => {
                        assert!(config.validation_error().is_none())
                    }
                    EdgionStreamPlugin::GeoIpLocation(config) => assert!(config.validation_error().is_none()),
                    _ => panic!("unexpected plugin in declared chain fixture"),
                }
            }
            let value: serde_json::Value = serde_json::to_value(consumer).unwrap();
            let profile = &value["spec"]["plugins"][0]["config"]["profiles"]["default"];
            assert!(profile.get("allow").is_none() && profile.get("deny").is_none());
            assert_eq!(profile[field], serde_json::json!([{ "name": reference }]));
            assert_eq!(
                profile["defaultAction"],
                if field == "allowRefs" { "deny" } else { "allow" }
            );
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(root)
                .join("conf/TCPRoute/GeoRestriction")
                .join(fixture);
            let resource: edgion_resources::EdgionConfigData =
                serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            assert_eq!(resource.metadata.name.as_deref(), Some(reference));
            assert_eq!(resource.metadata.namespace.as_deref(), Some("edgion-test"));
            let value: serde_json::Value = serde_json::to_value(resource).unwrap();
            assert_eq!(value["spec"]["data"]["type"], "IpList");
            assert_eq!(
                value["spec"]["data"]["config"]["items"],
                serde_json::json!([{ "name":group, "cidrs":[cidr] }])
            );
        }
    }
}
