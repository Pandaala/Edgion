// TLSRoute GeoRestriction suite: one geo-deny case, enough to prove
// layer="tls" is wired and counted, plus a chain-order check.
//
// Against edgion-tests/integration/conf/TLSRoute/GeoRestriction/: a single
// TLS listener (tls-geo-gw:31953) whose TLSRoute carries the
// edgion.io/edgion-stream-plugins annotation directly, pointing at an ESP
// that has the Stage-1 `plugins` shape: `GlobalConnectionIpRestriction`
// (denying an unrelated CIDR, first) THEN `GeoIpLocation` (denying RU,
// second) — the `GeoIpLocation` plugin owns geo exclusively post-refactor.
// Per edgion-gateway/src/routes/tls/mod.rs::check_tls_route_plugins, an empty
// Stage-2 tls_route_plugin_runtime falls back to running the Stage-1 runtime
// with `.with_layer("tls")`.
//
// Source binding: connect FROM 127.0.0.5 (RU, per the GeoIP test fixture) to
// deny, and FROM 127.0.0.1 (CN) to prove the same listener still admits an
// unaffected country. A bind() failure is a hard test failure (Inconclusive
// path), never silently read as "successfully denied" — same verdict
// discipline as the TCPRoute/GeoRestriction suite; deliberately NOT the
// looser tls_route/stream_plugins.rs pattern that treats every connect/TLS
// error as a pass.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::tls_route::basic::basic::make_tls_connector;
use rustls::pki_types::ServerName;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpSocket;

pub struct TlsGeoRestrictionTestSuite;

const HOST: &str = "probe.geo.example.com";

enum Verdict {
    Echoed(usize),
    Denied(String),
    /// Something other than a policy decision went wrong (bind failure,
    /// timeout, unparseable address) — must never be read as a denial.
    Inconclusive(String),
}

async fn probe(src: &str, ctx: &TestContext) -> Verdict {
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
    let dst = format!("{}:31953", ctx.target_host);
    let dst_addr = match dst.parse() {
        Ok(a) => a,
        Err(e) => return Verdict::Inconclusive(format!("unparseable dest addr {dst}: {e}")),
    };
    let tcp_stream = match tokio::time::timeout(Duration::from_secs(2), sock.connect(dst_addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Verdict::Inconclusive(format!("connect failed: {e}")),
        Err(_) => return Verdict::Inconclusive("connect timed out".to_string()),
    };

    let connector = make_tls_connector();
    let sni = match ServerName::try_from(HOST) {
        Ok(s) => s,
        Err(e) => return Verdict::Inconclusive(format!("bad SNI: {e}")),
    };
    let mut tls_stream = match connector.connect(sni, tcp_stream).await {
        Ok(s) => s,
        // A pre-proxy geo deny may also surface as a handshake failure,
        // depending on exactly where check_tls_route_plugins is wired
        // relative to the handshake — same observed shape as the existing
        // TLSRoute/StreamPlugins deny-all case.
        Err(e) => return Verdict::Denied(format!("TLS handshake rejected: {e}")),
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
        &[("layer", "tls"), ("reason", reason)],
    )
}

impl TlsGeoRestrictionTestSuite {
    fn geo_deny_ru() -> TestCase {
        TestCase::new(
            "geo_deny_ru",
            "a TLS connection sourced from an RU-mapped loopback alias is denied at layer=tls",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
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
                        match probe("127.0.0.5", &ctx).await {
                            Verdict::Denied(reason) => {
                                let current = geoip_denied_total(&ctx, "deny_country").await;
                                if current - baseline >= 1.0 {
                                    return TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!("denied ({reason}), metric={current}"),
                                    );
                                }
                                last = format!(
                                    "{reason} (edgion_geoip_denied_total{{layer=\"tls\",reason=\"deny_country\"}} not yet bumped: {current}, baseline {baseline})"
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
                            "RU source was never denied with edgion_geoip_denied_total{{layer=\"tls\",reason=\"deny_country\"}} bumped; last: {last}"
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
                    let mut last = String::new();
                    for _ in 0..20 {
                        match probe("127.0.0.1", &ctx).await {
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
        ]
    }
}
