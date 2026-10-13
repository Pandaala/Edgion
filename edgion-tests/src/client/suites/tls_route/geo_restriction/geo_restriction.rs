// TLS listener GeoRestriction coverage through the Gateway Stage-1 policy.
// GlobalConnectionIpRestriction runs before GeoIpLocation. Local sources use
// fixture GeoIP mappings; Kubernetes uses the real pod peer and policy updates.
// Bind/connect failures and timeouts must never count as policy denial.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::tls_route::basic::basic::make_tls_connector;
use rustls::pki_types::ServerName;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpSocket, TcpStream};

pub struct TlsGeoRestrictionTestSuite;

const HOST: &str = "probe.geo.example.com";

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

enum Verdict {
    Echoed(usize),
    Denied(String),
    /// Something other than a policy decision went wrong (bind failure,
    /// timeout, unparseable address) — must never be read as a denial.
    Inconclusive(String),
}

async fn probe(src: &str, ctx: &TestContext) -> Verdict {
    let dst = format!("{}:31953", ctx.target_host);
    let tcp_stream = if k8s_mode() {
        match tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(&dst)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(e)) => return Verdict::Inconclusive(format!("connect failed: {e}")),
            Err(_) => return Verdict::Inconclusive("connect timed out".to_string()),
        }
    } else {
        let sock = match TcpSocket::new_v4() {
            Ok(s) => s,
            Err(e) => return Verdict::Inconclusive(format!("socket() failed: {e}")),
        };
        let src_addr = match format!("{src}:0").parse() {
            Ok(a) => a,
            Err(e) => return Verdict::Inconclusive(format!("unparseable source addr {src}: {e}")),
        };
        if let Err(e) = sock.bind(src_addr) {
            return Verdict::Inconclusive(format!("bind({src}) failed: {e} — is the loopback alias missing?"));
        }
        let dst_addr = match dst.parse() {
            Ok(a) => a,
            Err(e) => return Verdict::Inconclusive(format!("unparseable dest addr {dst}: {e}")),
        };
        match tokio::time::timeout(Duration::from_secs(2), sock.connect(dst_addr)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(e)) => return Verdict::Inconclusive(format!("connect failed: {e}")),
            Err(_) => return Verdict::Inconclusive("connect timed out".to_string()),
        }
    };

    let connector = make_tls_connector();
    let sni = match ServerName::try_from(HOST) {
        Ok(s) => s,
        Err(e) => return Verdict::Inconclusive(format!("bad SNI: {e}")),
    };
    let mut tls_stream = match tokio::time::timeout(Duration::from_secs(2), connector.connect(sni, tcp_stream)).await {
        Ok(Ok(s)) => s,
        // Gateway admission can close the connection before the TLS handshake.
        Ok(Err(e)) => return Verdict::Denied(format!("TLS handshake rejected: {e}")),
        Err(_) => return Verdict::Inconclusive("TLS handshake timed out".into()),
    };

    let test_data = b"geo-tls-probe";
    if let Err(e) = tls_stream.write_all(test_data).await {
        return Verdict::Denied(format!("write failed post-handshake: {e}"));
    }
    let mut buf = vec![0u8; 1024];
    match tokio::time::timeout(Duration::from_secs(2), tls_stream.read(&mut buf)).await {
        Ok(Ok(0)) => Verdict::Denied("connection closed (EOF) after TLS handshake".to_string()),
        Ok(Ok(n)) => Verdict::Echoed(n),
        Ok(Err(e)) => Verdict::Denied(format!("read failed: {e}")),
        Err(_) => Verdict::Inconclusive("read timed out".to_string()),
    }
}

async fn geoip_denied_total(ctx: &TestContext, reason: &str) -> f64 {
    let metrics = crate::metrics_helper::MetricsClient::from_host_port(&ctx.target_host, 12090);
    let Ok(raw) = metrics.fetch_raw_metrics().await else {
        return 0.0;
    };
    crate::metrics_helper::counter_value(
        &raw,
        "edgion_geoip_denied_total",
        &[("layer", "stage1"), ("reason", reason)],
    )
}

async fn apply_geo_chain(ctx: &TestContext, denied_country: &str) -> Result<(), String> {
    let yaml = format!(
        r#"apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: tls-geo-deny
  namespace: edgion-test
spec:
  plugins:
    - type: GlobalConnectionIpRestriction
      config:
        enable: true
        activeProfile: default
        profiles:
          default:
            defaultAction: allow
            deny:
              - name: deny-unrelated-source
                cidrs:
                  - "127.0.0.9/32"
    - type: GeoIpLocation
      config:
        defaultAction: allow
        failOpen: false
        denyRuleGroups:
          - name: block-country
            rules:
              - source: DirectPeerIp
                key: country
                values: ["{denied_country}"]
"#
    );
    ctx.apply_yaml(&yaml).await
}

impl TlsGeoRestrictionTestSuite {
    fn geo_deny_ru() -> TestCase {
        TestCase::new(
            "geo_deny_ru",
            "a TLS connection sourced from an RU-mapped loopback alias is denied at layer=stage1",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let denied_country = if k8s_mode() { "US" } else { "RU" };
                    if let Err(e) = apply_geo_chain(&ctx, denied_country).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    // Delta-based, not absolute: a bare `>= 1.0` on the first
                    // `Denied` result would be vulnerable to the same race
                    // documented in the TCPRoute/GeoRestriction suite (a
                    // denial observed before conf_sync has actually finished
                    // propagating this ESP could be misattributed to geo).
                    // This suite's ESP never switches profile at runtime, so
                    // that race is only theoretical here — kept for
                    // consistency with the TCP suite's discipline, not
                    // because it was observed to fire for TLS.
                    let baseline = geoip_denied_total(&ctx, "deny_country").await;
                    let mut last = String::new();
                    for _ in 0..20 {
                        match probe(test_source("127.0.0.5"), &ctx).await {
                            Verdict::Denied(reason) => {
                                let current = geoip_denied_total(&ctx, "deny_country").await;
                                if current - baseline >= 1.0 {
                                    return TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!("denied ({reason}), metric={current}"),
                                    );
                                }
                                last = format!(
                                    "{reason} (edgion_geoip_denied_total{{layer=\"stage1\",reason=\"deny_country\"}} not yet bumped: {current}, baseline {baseline})"
                                );
                            }
                            Verdict::Echoed(n) => last = format!("unexpectedly allowed, echoed {n} bytes"),
                            Verdict::Inconclusive(reason) => last = reason,
                        }
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                    TestResult::failed(
                        start.elapsed(),
                        format!(
                            "RU source was never denied with edgion_geoip_denied_total{{layer=\"stage1\",reason=\"deny_country\"}} bumped; last: {last}"
                        ),
                    )
                })
            },
        )
    }

    fn geo_allow_cn_unaffected() -> TestCase {
        TestCase::new(
            "geo_allow_cn_unaffected",
            "a TLS connection sourced from a CN-mapped loopback alias is unaffected by the RU denyCountries rule",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(e) = apply_geo_chain(&ctx, "RU").await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let mut last = String::new();
                    for _ in 0..20 {
                        match probe(test_source("127.0.0.1"), &ctx).await {
                            Verdict::Echoed(n) => {
                                return TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("allowed, echoed {n} bytes"),
                                )
                            }
                            Verdict::Denied(reason) => last = format!("unexpectedly denied: {reason}"),
                            Verdict::Inconclusive(reason) => last = reason,
                        }
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                    TestResult::failed(start.elapsed(), format!("CN source was never allowed; last: {last}"))
                })
            },
        )
    }
}

impl TestSuite for TlsGeoRestrictionTestSuite {
    fn name(&self) -> &str {
        "TLSRoute/GeoRestriction"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            TlsGeoRestrictionTestSuite::geo_deny_ru(),
            TlsGeoRestrictionTestSuite::geo_allow_cn_unaffected(),
            TlsGeoRestrictionTestSuite::cidr_list_stage1_live_and_missing_recovery(),
        ]
    }
}

async fn apply_geo_cidrs(ctx: &TestContext, cidr: &str) -> Result<(), String> {
    let yaml = format!(
        r#"apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: geo-tls-cidrs
  namespace: edgion-test
spec:
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

async fn wait_geo_verdict(ctx: &TestContext, source: &str, denial: Option<&str>) -> Result<(), String> {
    let baseline = if let Some(reason) = denial {
        geoip_denied_total(ctx, reason).await
    } else {
        0.0
    };
    let mut last = "no completed TLS probe".to_string();
    for _ in 0..25 {
        match probe(source, ctx).await {
            Verdict::Echoed(n) if denial.is_none() => {
                return if n > 0 {
                    Ok(())
                } else {
                    Err("empty echo is not forwarding".into())
                }
            }
            Verdict::Echoed(n) => last = format!("unexpected echo of {n} bytes"),
            Verdict::Denied(message) => {
                if let Some(reason) = denial {
                    if geoip_denied_total(ctx, reason).await - baseline >= 1.0 {
                        return Ok(());
                    }
                }
                last = message;
            }
            Verdict::Inconclusive(message) => return Err(message),
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err(format!("TLS verdict did not converge: {last}"))
}

async fn restore_geo_fixture(ctx: &TestContext) -> Result<(), String> {
    use serde::Deserialize;
    let source = if k8s_mode() {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/k8s/conf/TLSRoute/GeoRestriction/resources.yaml"
        ))
    } else {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/conf/TLSRoute/GeoRestriction/01_EdgionStreamPlugins_geo_deny.yaml"
        ))
    };
    let yaml = {
        let mut selected = None;
        for document in serde_yaml::Deserializer::from_str(source) {
            let value = serde_yaml::Value::deserialize(document).map_err(|error| error.to_string())?;
            if value["kind"].as_str() == Some("EdgionStreamPlugins")
                && value["metadata"]["name"].as_str() == Some("tls-geo-deny")
            {
                selected = Some(serde_yaml::to_string(&value).map_err(|error| error.to_string())?);
                break;
            }
        }
        selected.ok_or_else(|| "original TLS Geo fixture is missing".to_string())?
    };
    ctx.apply_yaml(&yaml).await
}

impl TlsGeoRestrictionTestSuite {
    fn cidr_list_stage1_live_and_missing_recovery() -> TestCase {
        TestCase::new(
            "cidr_list_stage1_live_and_missing_recovery",
            "TLS Stage-1 CIDR references use the physical peer and recover on list-only changes",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let hit = if k8s_mode() { "192.168.0.0/16" } else { "127.0.0.5/32" };
                    let miss = if k8s_mode() { "10.0.0.0/8" } else { "127.0.0.1/32" };
                    // Isolate Geo: prior GCIR cannot supply a false denial/metric proof.
                    let policy = "apiVersion: edgion.io/v1\nkind: EdgionStreamPlugins\nmetadata:\n  name: tls-geo-deny\n  namespace: edgion-test\nspec:\n  plugins:\n    - type: GeoIpLocation\n      config:\n        failOpen: true\n        denyRuleGroups:\n          - name: peer-list\n            rules:\n              - source: DirectPeerIp\n                key: cidr\n                ipRefs: [{name: geo-tls-cidrs}]\n";
                    let source = test_source("127.0.0.5");
                    let check = tokio::time::timeout(Duration::from_secs(85), async {
                        apply_geo_cidrs(&ctx, miss).await?;
                        ctx.apply_yaml(policy).await?;
                        wait_geo_verdict(&ctx, source, None).await?;
                        apply_geo_cidrs(&ctx, hit).await?;
                        wait_geo_verdict(&ctx, source, Some("deny_country")).await?;
                        if !k8s_mode() {
                            wait_geo_verdict(&ctx, "127.0.0.1", None).await?;
                        }
                        ctx.delete_resource("EdgionConfigData", "edgion-test", "geo-tls-cidrs").await?;
                        wait_geo_verdict(&ctx, source, Some("ref_unavailable")).await?;
                        apply_geo_cidrs(&ctx, miss).await?;
                        wait_geo_verdict(&ctx, source, None).await
                    })
                        .await
                        .unwrap_or_else(|_| Err("Geo verification timed out after 85s; cleanup remains reserved".to_string()));
                    let cleanup = cleanup_geo_resources(&ctx, hit).await;
                    match (check, cleanup) {
                        (Ok(()), Ok(())) => TestResult::passed_with_message(
                            start.elapsed(),
                            "actual Stage-1 reference metrics, TLS forwarding/denial and list-only recovery; cleanup complete".into(),
                        ),
                        (result, cleanup) => TestResult::failed(start.elapsed(), format!("verification={result:?}; cleanup={cleanup:?}")),
                    }
                })
            },
        ).with_timeout(Duration::from_secs(120))
    }
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
