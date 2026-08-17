// EdgionPlugins GeoRestriction suite — HTTP-layer (RequestRestriction /
// GlobalAccessControl) coverage for `ClientGeo` / `DirectGeo` predicates.
//
// Against edgion-tests/integration/conf/EdgionPlugins/GeoRestriction/: a
// dedicated Gateway (31950) whose HTTPRoute points at a single RequestRestriction
// resource ("geo-rr") that test cases hot-swap via ctx.apply_yaml(...) before
// each request (the same "one flexible resource, re-applied per case" pattern
// TCPRoute/GlobalConnectionIpRestriction uses for its Selector), plus a
// second static HTTPRoute/EdgionPlugins pair exercising the shared
// RestrictionRule geo predicate through GlobalAccessControl too.
//
// The fixture's authored address map (edgion-tests/geoip/README.md):
//   127.0.0.1 CN Beijing BJ | 127.0.0.5 RU Moscow MOW | 127.0.0.6 US (no city)
//   127.0.0.7 HK Kowloon KOW | 203.0.113.7 GB London ENG | 198.51.100.4 HK (no city)
//   8.8.8.8 unmapped
//
// ClientGeo needs a real client-address source. Two techniques are used,
// deliberately labeled:
//   - Client-side SOURCE BINDING (`client_from`): the exemplary, safe-by-
//     construction technique — the TEST bind()s its own local address before
//     connecting, so the gateway's peer address (and DirectGeo) is exactly
//     that bound address; combined with real_ip trusting 127.0.0.0/8, ClientGeo
//     resolves the same way.
//   - X-Forwarded-For SPOOFING: an ATTACK PATTERN, not a recommendation.
//     Trusting 127.0.0.1 as a proxy and forging XFF is exactly the bypass
//     design §5.2 describes — safe to demonstrate here ONLY because this
//     harness is closed (no real client can reach this Gateway from the
//     outside claiming to be a trusted proxy). It exists so ClientGeo (which
//     reads the XFF-resolved address) and DirectGeo (which never does) can be
//     shown disagreeing in the same request.
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
const GAC_HOST: &str = "geo-gac.example.com";
const RR_NAME: &str = "geo-rr";
const RR_NS: &str = "edgion-default";

/// A reqwest client whose outbound TCP connection is bound to `src` — the
/// exemplary, source-bound technique (see module doc). `src` must be one of
/// the loopback aliases the harness provisions (127.0.0.1/.5/.6/.7).
fn client_from(src: &str) -> Client {
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
        req = req.header("x-trace-id", trace).header("access_log", "test_store");
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

fn rr_yaml(mode: &str, status: u16, rule_groups_yaml: &str) -> String {
    format!(
        r#"apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: {RR_NAME}
  namespace: {RR_NS}
spec:
  requestPlugins:
    - enable: true
      type: RequestRestriction
      config:
        mode: {mode}
        ruleGroups:
{rule_groups_yaml}
        status: {status}
        message: "Access denied by geo policy"
        failOpen: false
"#
    )
}

async fn apply_rr(ctx: &TestContext, mode: &str, status: u16, rule_groups_yaml: &str) -> Result<(), String> {
    ctx.apply_yaml(&rr_yaml(mode, status, rule_groups_yaml)).await
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
    /// `ClientGeo/country` blacklist: XFF 203.0.113.7 (GB) -> rejected.
    fn client_geo_country_blacklist() -> TestCase {
        TestCase::new(
            "client_geo_country_blacklist",
            "ClientGeo country blacklist rejects an XFF-forged GB source",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if let Err(e) = apply_rr(
                        &ctx,
                        "Blacklist",
                        403,
                        "          - name: client-country-gb\n            rules:\n              - name: client-country-gb\n                source: ClientGeo\n                key: country\n                values: [\"GB\"]\n",
                    )
                    .await
                    {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let client = client_from("127.0.0.1");
                    match expect_status(&client, &ctx, RR_HOST, Some("203.0.113.7"), 403).await {
                        Ok(()) => {
                            TestResult::passed_with_message(start.elapsed(), "GB source correctly rejected".to_string())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// `ClientGeo/continent`, `/city`, `/subdivision` each independently
    /// match against the same XFF (203.0.113.7 -> GB/EU/London/ENG).
    fn client_geo_other_dimensions_match() -> TestCase {
        TestCase::new(
            "client_geo_other_dimensions_match",
            "continent, city, and subdivision each independently match against the same source",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = client_from("127.0.0.1");
                    for (key, value) in [("continent", "EU"), ("city", "London"), ("subdivision", "ENG")] {
                        let groups = format!(
                            "          - name: dim-{key}\n            rules:\n              - name: dim-{key}\n                source: ClientGeo\n                key: {key}\n                values: [\"{value}\"]\n"
                        );
                        if let Err(e) = apply_rr(&ctx, "Blacklist", 403, &groups).await {
                            return TestResult::failed(start.elapsed(), format!("apply ({key}): {e}"));
                        }
                        if let Err(e) = expect_status(&client, &ctx, RR_HOST, Some("203.0.113.7"), 403).await {
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

    /// `country`+`city` AND group: both match (GB/London) -> rejected; only
    /// country matches (HK, no city on that record) -> allowed (the AND must
    /// not degrade to an OR when one predicate is absent).
    fn and_group_must_not_degrade() -> TestCase {
        TestCase::new(
            "and_group_must_not_degrade",
            "an AND group requires every predicate; a missing dimension must not let a partial match through",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let groups = "          - name: gb-london-and\n            rules:\n              - name: country-gb-or-hk\n                source: ClientGeo\n                key: country\n                values: [\"GB\", \"HK\"]\n              - name: city-london\n                source: ClientGeo\n                key: city\n                values: [\"London\"]\n";
                    if let Err(e) = apply_rr(&ctx, "Blacklist", 403, groups).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let client = client_from("127.0.0.1");
                    if let Err(e) = expect_status(&client, &ctx, RR_HOST, Some("203.0.113.7"), 403).await {
                        return TestResult::failed(start.elapsed(), format!("GB/London: {e}"));
                    }
                    match expect_status(&client, &ctx, RR_HOST, Some("198.51.100.4"), 200).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "both-match rejected, country-only (missing city) correctly allowed".to_string(),
                        ),
                        Err(e) => {
                            TestResult::failed(start.elapsed(), format!("HK/no-city (AND must not degrade): {e}"))
                        }
                    }
                })
            },
        )
    }

    /// `DirectGeo` resolves: source-bound 127.0.0.1 (CN) matches — the row
    /// the fixture unlocks (no address spoofing involved at all).
    fn direct_geo_resolves() -> TestCase {
        TestCase::new(
            "direct_geo_resolves",
            "DirectGeo resolves the real, source-bound peer address (no XFF at all)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let groups = "          - name: direct-country-cn\n            rules:\n              - name: direct-country-cn\n                source: DirectGeo\n                key: country\n                values: [\"CN\"]\n";
                    if let Err(e) = apply_rr(&ctx, "Blacklist", 403, groups).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let client = client_from("127.0.0.1");
                    match expect_status(&client, &ctx, RR_HOST, None, 403).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "DirectGeo correctly resolved CN and rejected".to_string(),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// `DirectGeo` and `ClientGeo` disagree: source-bound 127.0.0.1 (CN) +
    /// XFF 127.0.0.5 (RU) — each predicate sees its own address. Labeled as
    /// the deliberately-unsafe-but-contained XFF attack pattern (see module
    /// doc): trusting 127.0.0.1 as a proxy here is safe ONLY because this
    /// harness is closed.
    fn direct_and_client_geo_disagree() -> TestCase {
        TestCase::new(
            "direct_and_client_geo_disagree",
            "DirectGeo and ClientGeo independently resolve their own address, even when they disagree",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let groups = "          - name: direct-cn-and-client-ru\n            rules:\n              - name: direct-country-cn\n                source: DirectGeo\n                key: country\n                values: [\"CN\"]\n              - name: client-country-ru\n                source: ClientGeo\n                key: country\n                values: [\"RU\"]\n";
                    if let Err(e) = apply_rr(&ctx, "Blacklist", 403, groups).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let client = client_from("127.0.0.1");
                    // XFF forges the RU proxy client address (attack pattern, contained —
                    // see module doc): direct=CN (bound source), client=RU (forged XFF).
                    if let Err(e) = expect_status(&client, &ctx, RR_HOST, Some("127.0.0.5"), 403).await {
                        return TestResult::failed(start.elapsed(), format!("direct=CN + xff-client=RU: {e}"));
                    }
                    // No XFF: ClientGeo falls back to the same bound address (CN), so the
                    // ClientGeo=RU half of the AND no longer holds -> allowed. This is the
                    // exemplary source-bound-only variant (no XFF forged at all).
                    match expect_status(&client, &ctx, RR_HOST, None, 200).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "each predicate independently resolved its own address".to_string(),
                        ),
                        Err(e) => {
                            TestResult::failed(start.elapsed(), format!("direct=CN, no xff (client also CN): {e}"))
                        }
                    }
                })
            },
        )
    }

    /// `onMissing: NoMatch` on an unknown address (XFF 8.8.8.8, unmapped in
    /// the fixture) -> predicate false, request allowed.
    fn on_missing_no_match_unknown_address() -> TestCase {
        TestCase::new(
            "on_missing_no_match_unknown_address",
            "an unmapped address with onMissing: NoMatch (default) allows the request",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let groups = "          - name: country-us-nomatch\n            rules:\n              - name: country-us-nomatch\n                source: ClientGeo\n                key: country\n                values: [\"US\"]\n                onMissing: NoMatch\n";
                    if let Err(e) = apply_rr(&ctx, "Blacklist", 403, groups).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let client = client_from("127.0.0.1");
                    match expect_status(&client, &ctx, RR_HOST, Some("8.8.8.8"), 200).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "unmapped address correctly allowed".to_string(),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Dimension absent on the record (XFF 127.0.0.6, US with no city in the
    /// fixture): `onMissing` applies, NOT `failOpen` — proven by using
    /// `onMissing: Match` (the opposite of the previous case's `NoMatch`) so
    /// the request is rejected only if `onMissing` (not a fail-closed path)
    /// governed the missing dimension.
    fn on_missing_governs_absent_dimension() -> TestCase {
        TestCase::new(
            "on_missing_governs_absent_dimension",
            "a found record with an absent dimension is governed by onMissing, not failOpen",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let groups = "          - name: city-absent-on-missing-match\n            rules:\n              - name: city-absent-on-missing-match\n                source: ClientGeo\n                key: city\n                values: [\"Anything\"]\n                onMissing: Match\n";
                    if let Err(e) = apply_rr(&ctx, "Blacklist", 403, groups).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let client = client_from("127.0.0.1");
                    match expect_status(&client, &ctx, RR_HOST, Some("127.0.0.6"), 403).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "absent city on a found US record correctly matched via onMissing: Match".to_string(),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// All-`Skip` whitelist -> `considered == 0` -> every request 403s. Per
    /// design §5.4, this hazard needs only NotFound + Skip, NOT an
    /// unavailable database.
    fn all_skip_whitelist_denies_everything() -> TestCase {
        TestCase::new(
            "all_skip_whitelist_denies_everything",
            "an all-Skip whitelist group is never considered, so every request 403s even with the database loaded",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    // `values` is required even for a Skip-only predicate: the
                    // shared structural check (`RestrictionRule::field_error`)
                    // requires at least one of values/regex/secretRef unless
                    // onMissing is Match, and that check is source-agnostic (it
                    // runs for geo rules too). The value never actually gets
                    // compared here — 8.8.8.8 has no country record at all, so
                    // onMissing: Skip always drops this predicate regardless of
                    // what candidate values are configured.
                    let groups = "          - name: all-skip\n            rules:\n              - name: country-skip\n                source: ClientGeo\n                key: country\n                values: [\"ZZ\"]\n                onMissing: Skip\n";
                    if let Err(e) = apply_rr(&ctx, "Whitelist", 403, groups).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let client = client_from("127.0.0.1");
                    match expect_status(&client, &ctx, RR_HOST, Some("8.8.8.8"), 403).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "all-Skip whitelist correctly denies (considered == 0, no group ever matches)".to_string(),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Access log content: `geo.client.country` / `geo.direct.country` are
    /// present and correct, and `geo.client.coord` is absent (null) by
    /// default (`logCoordinates: false` in the harness's geoip block).
    fn access_log_geo_fields() -> TestCase {
        TestCase::new(
            "access_log_geo_fields",
            "a consulted slot reports its country; an unconsulted slot is absent (the log never forces a lookup)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    // Blacklist on an unrelated value so the request is ALLOWED (200) and
                    // reliably reaches the access-log write path with both slots resolved.
                    let groups = "          - name: never-matches\n            rules:\n              - name: never-matches\n                source: ClientGeo\n                key: country\n                values: [\"ZZ\"]\n";
                    if let Err(e) = apply_rr(&ctx, "Blacklist", 403, groups).await {
                        return TestResult::failed(start.elapsed(), format!("apply: {e}"));
                    }
                    let client = client_from("127.0.0.1");
                    // Wait for conf_sync propagation (a 200 baseline) before firing the
                    // traced request that actually gets asserted below.
                    if let Err(e) = expect_status(&client, &ctx, RR_HOST, Some("203.0.113.7"), 200).await {
                        return TestResult::failed(start.elapsed(), format!("baseline propagation wait: {e}"));
                    }
                    let trace = format!("geo-access-log-{}", uuid::Uuid::new_v4());
                    if let Err(e) = get(&client, &ctx, RR_HOST, Some("203.0.113.7"), Some(&trace)).await {
                        return TestResult::failed(start.elapsed(), e);
                    }
                    let entry = match ctx.access_log_client().get_access_log_with_retry(&trace, 15, 200).await {
                        Ok(entry) => entry,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("access log missing: {e}")),
                    };
                    let client_country = entry.get_nested_str("geo.client.country");
                    let direct_country = entry.get_nested_str("geo.direct.country");
                    if client_country != Some("GB") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("geo.client.country expected GB, got {client_country:?}"),
                        );
                    }
                    // The rule under test only consults ClientGeo, so nothing ever
                    // resolved the direct slot — and building the log entry must NOT
                    // resolve it either. Its absence is the observable proof that the
                    // access log reads the slots lazily instead of forcing a lookup on
                    // every request.
                    if direct_country.is_some() {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "geo.direct must be absent when no rule consulted it, got {direct_country:?}"
                            ),
                        );
                    }
                    // 203.0.113.7 (GB/London) carries coordinates in the fixture, so
                    // `coord` IS emitted: there is no switch for it any more — presence
                    // follows the record, and the value renders through the shared
                    // `masked` serializer under the process-global sensitive mode.
                    if entry.data["geo"]["client"]["coord"].is_null() {
                        return TestResult::failed(
                            start.elapsed(),
                            "geo.client.coord expected present (the fixture record carries coordinates)"
                                .to_string(),
                        );
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "geo.client.country=GB, geo.direct.country=CN, coord absent".to_string(),
                    )
                })
            },
        )
    }

    /// Ingest rejection: a plugin with a geo rule but no plugin-level
    /// `failOpen` -> Accepted=False.
    fn ingest_rejection_missing_fail_open() -> TestCase {
        TestCase::new(
            "ingest_rejection_missing_fail_open",
            "a plugin with a geo rule but no failOpen is rejected at ingest (Accepted=False)",
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
    /// not rejected — `geo_field_error` only validates structure/key/
    /// failOpen, never ISO-code membership; an unknown code only WARNs at
    /// gateway compile time.
    fn invalid_iso_value_is_accepted() -> TestCase {
        TestCase::new(
            "invalid_iso_value_is_accepted",
            "an unrecognized ISO country code is accepted, not rejected (lenient admission)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let groups = "          - name: bogus-iso\n            rules:\n              - name: bogus-iso\n                source: ClientGeo\n                key: country\n                values: [\"ZZ\"]\n";
                    if let Err(e) = apply_rr(&ctx, "Blacklist", 403, groups).await {
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

    /// GlobalAccessControl also honors the shared geo predicate: CN is
    /// allowed (source-bound 127.0.0.1), GB is denied by default_action.
    fn global_access_control_geo_predicate() -> TestCase {
        TestCase::new(
            "global_access_control_geo_predicate",
            "GlobalAccessControl's shared RestrictionRule engine also evaluates ClientGeo",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = client_from("127.0.0.1");
                    if let Err(e) = expect_status(&client, &ctx, GAC_HOST, None, 200).await {
                        return TestResult::failed(start.elapsed(), format!("CN via DirectGeo: {e}"));
                    }
                    match expect_status(&client, &ctx, GAC_HOST, Some("203.0.113.7"), 403).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "GlobalAccessControl correctly allowed CN and denied GB via ClientGeo".to_string(),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), format!("GB: {e}")),
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
            GeoRestrictionTestSuite::client_geo_country_blacklist(),
            GeoRestrictionTestSuite::client_geo_other_dimensions_match(),
            GeoRestrictionTestSuite::and_group_must_not_degrade(),
            GeoRestrictionTestSuite::direct_geo_resolves(),
            GeoRestrictionTestSuite::direct_and_client_geo_disagree(),
            GeoRestrictionTestSuite::on_missing_no_match_unknown_address(),
            GeoRestrictionTestSuite::on_missing_governs_absent_dimension(),
            GeoRestrictionTestSuite::all_skip_whitelist_denies_everything(),
            GeoRestrictionTestSuite::access_log_geo_fields(),
            GeoRestrictionTestSuite::ingest_rejection_missing_fail_open(),
            GeoRestrictionTestSuite::invalid_iso_value_is_accepted(),
            GeoRestrictionTestSuite::global_access_control_geo_predicate(),
        ]
    }
}
