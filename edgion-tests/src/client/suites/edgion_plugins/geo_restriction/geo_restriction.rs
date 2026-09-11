// EdgionPlugins GeoRestriction suite — HTTP-layer coverage for the
// `GeoIpLocation` plugin (`GeoRuleSource::DirectPeerIp`/`RemoteIp` over
// `GeoRuleKey::{cidr,country,continent,city,subdivision}`).
//
// Against edgion-tests/integration/conf/EdgionPlugins/GeoRestriction/: a
// dedicated Gateway (31950) whose HTTPRoute points at a single GeoIpLocation
// resource ("geo-rr") that test cases hot-swap via ctx.apply_yaml(...) before
// each request (the same "one flexible resource, re-applied per case" pattern
// TCPRoute/GlobalConnectionIpRestriction uses for its Selector), plus a
// second static HTTPRoute/EdgionPlugins pair ("geo-chain") that chains a
// plain `IpRestriction` plugin BEFORE `GeoIpLocation` -- this is the intended
// post-refactor shape: GeoIpLocation only owns geo, any other restriction the
// request needs stays in its own plugin on the same chain.
//
// The fixture's authored address map (edgion-tests/geoip/README.md):
//   127.0.0.1 CN Beijing BJ | 127.0.0.5 RU Moscow MOW | 127.0.0.6 US (no city)
//   127.0.0.7 HK Kowloon KOW (with coordinates) | 127.0.0.2-4 unmapped
//   203.0.113.7 GB London ENG (with coordinates) | 198.51.100.4 HK (no city)
//   8.8.8.8 unmapped
//
// ONLY the 127.0.0.x half is usable as a SOURCE-BOUND address: `client_from`
// calls bind(2), which needs the address to exist locally, and the harness
// configures loopback aliases 127.0.0.1-.7 only. 203.0.113.7 / 198.51.100.4 /
// 8.8.8.8 are in the database so they can be spoofed as X-Forwarded-For VALUES
// (the `RemoteIp` source); source-binding to one fails at connect time and
// surfaces as a bogus "no successful response" rather than a policy verdict.
//
// Every source address below is the client's OWN bound source address
// (client-side SOURCE BINDING via `client_from`, exactly as
// TCPRoute/TLSRoute's suites do) -- the exemplary, safe-by-construction
// technique for exercising `DirectPeerIp`. `RemoteIp` needs a real
// XFF-resolved client, so the one test that exercises it (`remote_ip_source_
// resolves_via_xff`) forges `X-Forwarded-For` on top of a source-bound
// connection -- an ATTACK PATTERN, not a recommendation, safe to demonstrate
// here ONLY because this harness is closed (no real client can reach this
// Gateway from the outside claiming to be a trusted proxy); the shared base
// EdgionGatewayConfig already trusts 127.0.0.0/8 as a proxy (`realIp.
// trustedIps`).
//
// `ctx.apply_yaml` only waits for Controller admission, not for conf_sync
// propagation to the Gateway, so every assertion below polls via
// `expect_status` rather than firing one request right after an apply — the
// same reason the L4 GCIR suites retry after a Selector hot-swap.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use reqwest::Client;
use std::net::IpAddr;
use std::time::{Duration, Instant};

pub struct GeoRestrictionTestSuite;

const RR_HOST: &str = "geo-restriction.example.com";
const CHAIN_HOST: &str = "geo-chain.example.com";
const PARSEONLY_HOST: &str = "geo-parseonly.example.com";
const RR_NAME: &str = "geo-rr";
const RR_NS: &str = "edgion-default";

fn k8s_mode() -> bool {
    std::env::var("EDGION_TEST_K8S_MODE")
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// A reqwest client whose outbound TCP connection is bound to `src` — the
/// exemplary, source-bound technique (see module doc). `src` must be one of
/// the loopback aliases the harness provisions.
fn client_from(src: &str) -> Client {
    if k8s_mode() {
        return Client::builder()
            .timeout(Duration::from_secs(5))
            .no_proxy()
            .build()
            .expect("failed to build Kubernetes test client");
    }
    let ip: IpAddr = src.parse().expect("valid loopback alias");
    Client::builder()
        .timeout(Duration::from_secs(5))
        .local_address(ip)
        .no_proxy()
        .build()
        .expect("failed to build source-bound client")
}

/// Send a GET to `ctx.http_url()` with the given `Host`, optionally forging
/// `X-Forwarded-For`. `client` controls the actual TCP source address.
async fn get(
    client: &Client,
    ctx: &TestContext,
    host: &str,
    xff: Option<&str>,
    trace: Option<&str>,
) -> Result<reqwest::Response, String> {
    // test_server's catch-all route is `/{*path}`, which axum does not match
    // against the bare root "/" — use a non-root path so an ALLOWED request
    // actually reaches the backend instead of 404ing there.
    let url = format!("{}/probe", ctx.http_url());
    let mut req = client.get(&url).header("host", host);
    if let Some(xff) = xff {
        req = req.header("X-Forwarded-For", xff);
    }
    if let Some(trace) = trace {
        req = req
            .header("traceparent", crate::access_log_client::traceparent_for_marker(trace))
            .header("access_log", "test_store");
    }
    req.send().await.map_err(|e| format!("request failed: {e}"))
}

/// Poll `host` (with optional forged XFF) until it reports `want`, or fail
/// after a bounded wait.
async fn expect_status(
    client: &Client,
    ctx: &TestContext,
    host: &str,
    xff: Option<&str>,
    want: u16,
) -> Result<(), String> {
    // The controller's file-system config store uses a PollWatcher whose poll
    // interval scales with total conf-directory file count (up to 5s in a full
    // run_integration.sh tree — see file_watcher.rs's `poll_interval_for`), so
    // a hot-swap's propagation latency can approach that bound; retry for
    // comfortably longer than the worst case rather than the network-latency
    // scale most other suites use.
    let mut last = 0u16;
    let mut last_err = String::new();
    for _ in 0..60 {
        match get(client, ctx, host, xff, None).await {
            Ok(resp) => {
                last = resp.status().as_u16();
                if last == want {
                    return Ok(());
                }
            }
            Err(e) => last_err = e,
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    if last == 0 {
        Err(format!("no successful response after retries; last error: {last_err}"))
    } else {
        Err(format!("expected status {want}, last observed {last} after retries"))
    }
}

fn geo_rr_yaml(config_yaml: &str) -> String {
    format!(
        r#"apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: {RR_NAME}
  namespace: {RR_NS}
spec:
  requestPlugins:
    - enable: true
      type: GeoIpLocation
      config:
{config_yaml}
"#
    )
}

async fn apply_geo_rr(ctx: &TestContext, config_yaml: &str) -> Result<(), String> {
    ctx.apply_yaml(&geo_rr_yaml(config_yaml)).await
}

/// Poll `/configserver/EdgionPlugins` until the Accepted condition matches
/// `want_status` ("True" / "False"). Mirrors
/// suites/edgion_plugins/plugin_validation_status.rs::assert_accepted_false.
async fn wait_for_accepted(ctx: &TestContext, name: &str, want_status: &str) -> Result<(), String> {
    let mut last = String::new();
    for _ in 0..40 {
        let mut req = ctx.http_client.get(format!(
            "{}/configserver/EdgionPlugins?namespace={}&name={}",
            ctx.admin_api_url(),
            RR_NS,
            name
        ));
        if let Some(token) = &ctx.admin_token {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        match req.send().await {
            Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
                Ok(body) if body["success"].as_bool().unwrap_or(false) => {
                    if let Some(conditions) = body["data"]["status"]["conditions"].as_array() {
                        if let Some(accepted) = conditions.iter().find(|c| c["type"].as_str() == Some("Accepted")) {
                            if accepted["status"].as_str() == Some(want_status) {
                                return Ok(());
                            }
                            last = format!("Accepted={:?}", accepted);
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
    Err(format!(
        "Accepted never reached '{want_status}' for '{name}'; last: {last}"
    ))
}

impl GeoRestrictionTestSuite {
    /// A `DirectPeerIp/country` deny group: source-bound 127.0.0.5 (RU)
    /// matches -> rejected.
    fn deny_country_direct_peer() -> TestCase {
        TestCase::new(
            "deny_country_direct_peer",
            "a DirectPeerIp/country deny group rejects a source-bound RU peer",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let country = if k8s_mode() { "US" } else { "RU" };
                    let config = format!(
                        "          defaultAction: allow\n          failOpen: false\n          denyRuleGroups:\n            - name: block-country\n              rules:\n                - source: DirectPeerIp\n                  key: country\n                  values: [\"{country}\"]\n"
                    );
                    if let Err(e) = apply_geo_rr(&ctx, &config).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let client = client_from("127.0.0.5");
                    match expect_status(&client, &ctx, RR_HOST, None, 403).await {
                        Ok(()) => {
                            TestResult::passed_with_message(start.elapsed(), "RU peer correctly rejected".to_string())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Same deny group, a non-matching source (127.0.0.1, CN) passes.
    fn allow_pass_non_matching_country() -> TestCase {
        TestCase::new(
            "allow_pass_non_matching_country",
            "a source not covered by the deny group is allowed",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let config = "          defaultAction: allow\n          failOpen: false\n          denyRuleGroups:\n            - name: block-ru\n              rules:\n                - source: DirectPeerIp\n                  key: country\n                  values: [\"RU\"]\n";
                    if let Err(e) = apply_geo_rr(&ctx, config).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let client = client_from("127.0.0.1");
                    match expect_status(&client, &ctx, RR_HOST, None, 200).await {
                        Ok(()) => {
                            TestResult::passed_with_message(start.elapsed(), "CN peer correctly allowed".to_string())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// `country`/`continent`/`city`/`subdivision` each independently match
    /// against the same source-bound peer (127.0.0.7 -> HK/AS/Kowloon/KOW).
    fn other_dimensions_match() -> TestCase {
        TestCase::new(
            "other_dimensions_match",
            "continent, city, and subdivision each independently match against the same peer",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = client_from("127.0.0.7");
                    let dimensions = if k8s_mode() {
                        [("continent", "NA"), ("city", "Cambridge"), ("subdivision", "MA")]
                    } else {
                        [("continent", "AS"), ("city", "Kowloon"), ("subdivision", "KOW")]
                    };
                    for (key, value) in dimensions {
                        let config = format!(
                            "          defaultAction: allow\n          failOpen: false\n          denyRuleGroups:\n            - name: dim-{key}\n              rules:\n                - source: DirectPeerIp\n                  key: {key}\n                  values: [\"{value}\"]\n"
                        );
                        if let Err(e) = apply_geo_rr(&ctx, &config).await {
                            return TestResult::failed(start.elapsed(), format!("apply ({key}): {e}"));
                        }
                        if let Err(e) = expect_status(&client, &ctx, RR_HOST, None, 403).await {
                            return TestResult::failed(start.elapsed(), format!("dimension '{key}': {e}"));
                        }
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "continent/city/subdivision each matched independently".to_string(),
                    )
                })
            },
        )
    }

    /// `country`+`city` AND rule (both in one group): both match
    /// (HK/Kowloon, 127.0.0.7) -> rejected; only country matches (US,
    /// 127.0.0.6, a country-only record with no city) -> allowed (the AND
    /// must not degrade to an OR when one predicate is absent).
    fn and_group_must_not_degrade() -> TestCase {
        TestCase::new(
            "and_group_must_not_degrade",
            "an AND group requires every rule; a missing dimension must not let a partial match through",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let config = if k8s_mode() {
                        "          defaultAction: allow\n          failOpen: false\n          denyRuleGroups:\n            - name: us-cambridge-and\n              rules:\n                - source: DirectPeerIp\n                  key: country\n                  values: [\"US\"]\n                - source: DirectPeerIp\n                  key: city\n                  values: [\"Cambridge\"]\n"
                    } else {
                        "          defaultAction: allow\n          failOpen: false\n          denyRuleGroups:\n            - name: hk-kowloon-and\n              rules:\n                - source: DirectPeerIp\n                  key: country\n                  values: [\"HK\", \"US\"]\n                - source: DirectPeerIp\n                  key: city\n                  values: [\"Kowloon\"]\n"
                    };
                    if let Err(e) = apply_geo_rr(&ctx, config).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let hk_client = client_from("127.0.0.7");
                    if let Err(e) = expect_status(&hk_client, &ctx, RR_HOST, None, 403).await {
                        return TestResult::failed(start.elapsed(), format!("HK/Kowloon: {e}"));
                    }
                    if k8s_mode() {
                        let partial = "          defaultAction: allow\n          failOpen: false\n          denyRuleGroups:\n            - name: partial-match\n              rules:\n                - source: DirectPeerIp\n                  key: country\n                  values: [\"US\"]\n                - source: DirectPeerIp\n                  key: city\n                  values: [\"Kowloon\"]\n";
                        if let Err(e) = apply_geo_rr(&ctx, partial).await {
                            return TestResult::failed(start.elapsed(), format!("apply partial-match: {e}"));
                        }
                    }
                    let us_client = client_from("127.0.0.6");
                    match expect_status(&us_client, &ctx, RR_HOST, None, 200).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "both-match rejected, country-only (missing city) correctly allowed".to_string(),
                        ),
                        Err(e) => {
                            TestResult::failed(start.elapsed(), format!("US/no-city (AND must not degrade): {e}"))
                        }
                    }
                })
            },
        )
    }

    /// `RemoteIp` resolves the XFF-forwarded address independently of
    /// `DirectPeerIp`: an AND group across both sources (direct=CN AND
    /// remote=RU) only matches when the XFF actually forges a different
    /// address than the bound source.
    fn remote_ip_source_resolves_via_xff() -> TestCase {
        TestCase::new(
            "remote_ip_source_resolves_via_xff",
            "RemoteIp independently resolves the XFF-forwarded address, distinct from DirectPeerIp",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let direct_country = if k8s_mode() { "US" } else { "CN" };
                    let config = format!(
                        "          defaultAction: allow\n          failOpen: false\n          denyRuleGroups:\n            - name: direct-and-remote\n              rules:\n                - source: DirectPeerIp\n                  key: country\n                  values: [\"{direct_country}\"]\n                - source: RemoteIp\n                  key: country\n                  values: [\"RU\"]\n"
                    );
                    if let Err(e) = apply_geo_rr(&ctx, &config).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let client = client_from("127.0.0.1");
                    // XFF forges the RU proxy client address (attack pattern, contained —
                    // see module doc): direct=CN (bound source), remote=RU (forged XFF).
                    if let Err(e) = expect_status(&client, &ctx, RR_HOST, Some("127.0.0.5"), 403).await {
                        return TestResult::failed(start.elapsed(), format!("direct=CN + xff-remote=RU: {e}"));
                    }
                    // No XFF: RemoteIp falls back to the same bound address (CN), so the
                    // remote=RU half of the AND no longer holds -> allowed.
                    match expect_status(&client, &ctx, RR_HOST, None, 200).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "each source independently resolved its own address".to_string(),
                        ),
                        Err(e) => {
                            TestResult::failed(start.elapsed(), format!("direct=CN, no xff (remote also CN): {e}"))
                        }
                    }
                })
            },
        )
    }

    /// A whitelist-only config (`allowRuleGroups` non-empty): a miss denies
    /// even under `defaultAction: allow`. 127.0.0.2 is a bindable loopback
    /// alias that the fixture deliberately leaves out of the database, so it
    /// resolves `NotFound`, never matches an allow group, and is denied.
    fn allow_whitelist_miss_denies() -> TestCase {
        TestCase::new(
            "allow_whitelist_miss_denies",
            "an allow-configured-and-missed source is denied even though defaultAction is Allow",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let allowed_country = if k8s_mode() { "US" } else { "CN" };
                    let config = format!(
                        "          defaultAction: allow\n          failOpen: false\n          allowRuleGroups:\n            - name: country-only\n              rules:\n                - source: DirectPeerIp\n                  key: country\n                  values: [\"{allowed_country}\"]\n"
                    );
                    if let Err(e) = apply_geo_rr(&ctx, &config).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let cn_client = client_from("127.0.0.1");
                    if let Err(e) = expect_status(&cn_client, &ctx, RR_HOST, None, 200).await {
                        return TestResult::failed(start.elapsed(), format!("CN allow hit: {e}"));
                    }
                    if k8s_mode() {
                        let miss = "          defaultAction: allow\n          failOpen: false\n          allowRuleGroups:\n            - name: cn-only\n              rules:\n                - source: DirectPeerIp\n                  key: country\n                  values: [\"CN\"]\n";
                        if let Err(e) = apply_geo_rr(&ctx, miss).await {
                            return TestResult::failed(start.elapsed(), format!("apply miss: {e}"));
                        }
                    }
                    let unmapped_client = client_from("127.0.0.2");
                    match expect_status(&unmapped_client, &ctx, RR_HOST, None, 403).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "unmapped source correctly denied (allow configured, missed)".to_string(),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// A CIDR rule matches by membership, exactly like the shared
    /// `IpRestriction` matcher — proving `key: cidr` works identically at the
    /// HTTP layer.
    fn cidr_rule_matches_by_membership() -> TestCase {
        TestCase::new(
            "cidr_rule_matches_by_membership",
            "a key: cidr rule matches by IP membership, independent of geo data",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let denied_cidr = if k8s_mode() { "192.168.0.0/16" } else { "127.0.0.5/32" };
                    let config = format!(
                        "          defaultAction: allow\n          failOpen: false\n          denyRuleGroups:\n            - name: block-cidr\n              rules:\n                - source: DirectPeerIp\n                  key: cidr\n                  values: [\"{denied_cidr}\"]\n"
                    );
                    if let Err(e) = apply_geo_rr(&ctx, &config).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let denied_client = client_from("127.0.0.5");
                    if let Err(e) = expect_status(&denied_client, &ctx, RR_HOST, None, 403).await {
                        return TestResult::failed(start.elapsed(), format!("in-CIDR: {e}"));
                    }
                    if k8s_mode() {
                        let out_of_cidr = "          defaultAction: allow\n          failOpen: false\n          denyRuleGroups:\n            - name: block-other-cidr\n              rules:\n                - source: DirectPeerIp\n                  key: cidr\n                  values: [\"10.0.0.0/8\"]\n";
                        if let Err(e) = apply_geo_rr(&ctx, out_of_cidr).await {
                            return TestResult::failed(start.elapsed(), format!("apply out-of-CIDR: {e}"));
                        }
                    }
                    let allowed_client = client_from("127.0.0.1");
                    match expect_status(&allowed_client, &ctx, RR_HOST, None, 200).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "CIDR membership correctly enforced".to_string(),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), format!("out-of-CIDR: {e}")),
                    }
                })
            },
        )
    }

    /// Ingest rejection: a plugin with a deny rule group but no plugin-level
    /// `failOpen` -> Accepted=False.
    fn ingest_rejection_missing_fail_open() -> TestCase {
        TestCase::new(
            "ingest_rejection_missing_fail_open",
            "a plugin with a rule group but no failOpen is rejected at ingest (Accepted=False)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match wait_for_accepted(&ctx, "geo-rr-invalid", "False").await {
                        Ok(()) => {
                            TestResult::passed_with_message(start.elapsed(), "Accepted=False as expected".to_string())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Invalid ISO value (`values: ["ZZ"]`): accepted (lenient admission),
    /// not rejected — `check_fields` only validates structure/key/failOpen,
    /// never ISO-code membership; an unknown code only WARNs at gateway
    /// compile time.
    fn invalid_iso_value_is_accepted() -> TestCase {
        TestCase::new(
            "invalid_iso_value_is_accepted",
            "an unrecognized ISO country code is accepted, not rejected (lenient admission)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let config = "          defaultAction: allow\n          failOpen: false\n          denyRuleGroups:\n            - name: bogus-iso\n              rules:\n                - source: DirectPeerIp\n                  key: country\n                  values: [\"ZZ\"]\n";
                    if let Err(e) = apply_geo_rr(&ctx, config).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    match wait_for_accepted(&ctx, RR_NAME, "True").await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "an unrecognized ISO code ('ZZ') was accepted, not rejected".to_string(),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Access log content: a rule-mode config consulting only `DirectPeerIp`
    /// populates `geo.direct_peer.*` and leaves `geo.remote` absent — the log
    /// reads whatever slot(s) the plugin actually resolved, never forcing an
    /// unconsulted lookup.
    fn access_log_geo_fields() -> TestCase {
        TestCase::new(
            "access_log_geo_fields",
            "a consulted slot reports its country; an unconsulted slot is absent",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let expected_country = if k8s_mode() { "US" } else { "HK" };
                    // Establish a discriminating precondition before switching
                    // to the allow case. A 200-only propagation wait can pass
                    // against an earlier allow configuration and then capture
                    // an access log before this generation reaches the runtime.
                    let blocking_config = format!(
                        "          defaultAction: allow\n          failOpen: false\n          denyRuleGroups:\n            - name: matches-peer\n              rules:\n                - source: DirectPeerIp\n                  key: country\n                  values: [\"{expected_country}\"]\n"
                    );
                    if let Err(e) = apply_geo_rr(&ctx, &blocking_config).await {
                        return TestResult::failed(start.elapsed(), format!("apply blocking precondition: {e}"));
                    }
                    let client = client_from("127.0.0.7");
                    if let Err(e) = expect_status(&client, &ctx, RR_HOST, None, 403).await {
                        return TestResult::failed(start.elapsed(), format!("blocking propagation wait: {e}"));
                    }
                    // Deny on an unrelated value so the request is ALLOWED (200) and
                    // reliably reaches the access-log write path with the direct_peer slot resolved.
                    let config = "          defaultAction: allow\n          failOpen: false\n          denyRuleGroups:\n            - name: never-matches\n              rules:\n                - source: DirectPeerIp\n                  key: country\n                  values: [\"ZZ\"]\n";
                    if let Err(e) = apply_geo_rr(&ctx, config).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    // Wait for conf_sync propagation (the transition from the
                    // 403 precondition to 200) before firing the
                    // traced request that actually gets asserted below.
                    if let Err(e) = expect_status(&client, &ctx, RR_HOST, None, 200).await {
                        return TestResult::failed(start.elapsed(), format!("baseline propagation wait: {e}"));
                    }
                    let trace = format!("geo-access-log-{}", uuid::Uuid::new_v4());
                    if let Err(e) = get(&client, &ctx, RR_HOST, None, Some(&trace)).await {
                        return TestResult::failed(start.elapsed(), e);
                    }
                    let entry = match ctx.access_log_client().get_access_log_with_retry(&trace, 15, 200).await {
                        Ok(entry) => entry,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("access log missing: {e}")),
                    };
                    let direct_peer_country = entry.get_nested_str("geo.direct_peer.country");
                    let remote_country = entry.get_nested_str("geo.remote.country");
                    if direct_peer_country != Some(expected_country) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("geo.direct_peer.country expected {expected_country}, got {direct_peer_country:?}"),
                        );
                    }
                    // The rule under test only consults DirectPeerIp, so nothing ever
                    // resolved the remote slot — its absence proves the access log
                    // reads slots lazily instead of forcing a lookup on every request.
                    if remote_country.is_some() {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("geo.remote must be absent when no rule consulted it, got {remote_country:?}"),
                        );
                    }
                    // 127.0.0.7 (HK/Kowloon) carries coordinates in the fixture, so
                    // `coord` IS emitted — presence follows the record, rendered through
                    // the shared `masked` serializer under the process-global sensitive mode.
                    if entry.data["geo"]["direct_peer"]["coord"].is_null() {
                        return TestResult::failed(
                            start.elapsed(),
                            "geo.direct_peer.coord expected present (the fixture record carries coordinates)"
                                .to_string(),
                        );
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!("geo.direct_peer.country={expected_country} present, geo.remote absent"),
                    )
                })
            },
        )
    }

    /// Parse-only mode (`config: {}`): every request passes through
    /// unconditionally, and BOTH `geo.direct_peer`/`geo.remote` are populated in
    /// the access log — parse-only resolves both slots (its whole purpose),
    /// unlike rule mode which resolves only what a rule actually consults.
    /// This is the core new capability this refactor exists to add.
    fn parse_only_mode_passes_and_populates_access_log() -> TestCase {
        TestCase::new(
            "parse_only_mode_passes_and_populates_access_log",
            "config: {} never denies, and populates both geo.direct_peer and geo.remote in the access log",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = client_from("127.0.0.1");
                    if let Err(e) = expect_status(&client, &ctx, PARSEONLY_HOST, None, 200).await {
                        return TestResult::failed(start.elapsed(), format!("baseline propagation wait: {e}"));
                    }
                    let trace = format!("geo-parseonly-{}", uuid::Uuid::new_v4());
                    if let Err(e) = get(&client, &ctx, PARSEONLY_HOST, None, Some(&trace)).await {
                        return TestResult::failed(start.elapsed(), e);
                    }
                    let entry = match ctx.access_log_client().get_access_log_with_retry(&trace, 15, 200).await {
                        Ok(entry) => entry,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("access log missing: {e}")),
                    };
                    let direct_peer_country = entry.get_nested_str("geo.direct_peer.country");
                    let remote_country = entry.get_nested_str("geo.remote.country");
                    let expected_country = if k8s_mode() { "US" } else { "CN" };
                    if direct_peer_country != Some(expected_country) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("geo.direct_peer.country expected {expected_country}, got {direct_peer_country:?}"),
                        );
                    }
                    if remote_country != Some(expected_country) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "geo.remote.country expected {expected_country} (parse-only resolves both slots), got {remote_country:?}"
                            ),
                        );
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "parse-only mode passed through and populated both geo slots".to_string(),
                    )
                })
            },
        )
    }

    /// Two-plugin chain: `IpRestriction` (non-geo, first in the list) denies
    /// a source unrelated to geo, before `GeoIpLocation` ever runs.
    fn chain_ip_deny_precedes_geo() -> TestCase {
        TestCase::new(
            "chain_ip_deny_precedes_geo",
            "IpRestriction denies a matching source before GeoIpLocation is ever reached",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = client_from("127.0.0.6");
                    let xff = k8s_mode().then_some("127.0.0.6");
                    match expect_status(&client, &ctx, CHAIN_HOST, xff, 403).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "IpRestriction correctly denied a non-geo-matching source".to_string(),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Same chain: a source outside the IP deny list but inside the geo deny
    /// list (RU) is still denied — by `GeoIpLocation`, independently.
    fn chain_geo_deny_independent_of_ip() -> TestCase {
        TestCase::new(
            "chain_geo_deny_independent_of_ip",
            "GeoIpLocation denies a source the IpRestriction stage never touched",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = client_from("127.0.0.5");
                    let xff = k8s_mode().then_some("127.0.0.5");
                    match expect_status(&client, &ctx, CHAIN_HOST, xff, 403).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "GeoIpLocation correctly denied an RU source".to_string(),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Same chain: a source matching neither stage passes both.
    fn chain_allows_both() -> TestCase {
        TestCase::new(
            "chain_allows_both",
            "a source matching neither the IP nor the geo stage is allowed",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = client_from("127.0.0.1");
                    let xff = k8s_mode().then_some("127.0.0.1");
                    match expect_status(&client, &ctx, CHAIN_HOST, xff, 200).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "a source matching neither stage was correctly allowed".to_string(),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }
}

impl TestSuite for GeoRestrictionTestSuite {
    fn name(&self) -> &str {
        "EdgionPlugins/GeoRestriction"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            GeoRestrictionTestSuite::deny_country_direct_peer(),
            GeoRestrictionTestSuite::allow_pass_non_matching_country(),
            GeoRestrictionTestSuite::other_dimensions_match(),
            GeoRestrictionTestSuite::and_group_must_not_degrade(),
            GeoRestrictionTestSuite::remote_ip_source_resolves_via_xff(),
            GeoRestrictionTestSuite::allow_whitelist_miss_denies(),
            GeoRestrictionTestSuite::cidr_rule_matches_by_membership(),
            GeoRestrictionTestSuite::ingest_rejection_missing_fail_open(),
            GeoRestrictionTestSuite::invalid_iso_value_is_accepted(),
            GeoRestrictionTestSuite::access_log_geo_fields(),
            GeoRestrictionTestSuite::parse_only_mode_passes_and_populates_access_log(),
            GeoRestrictionTestSuite::chain_ip_deny_precedes_geo(),
            GeoRestrictionTestSuite::chain_geo_deny_independent_of_ip(),
            GeoRestrictionTestSuite::chain_allows_both(),
        ]
    }
}
