// RequestAccessPolicy Plugin Test Suite
//
// Covers:
// - Composite allow: client IP in 127.0.0.0/8 AND correct secret header => 200
// - Deny when secret header is missing => 403
// - Deny when secret header has a wrong value => 403
// - Deny when secret header matches a non-selected value from the same Secret => 403
// - Deny when client IP is outside the configured CIDR (10.0.0.0/8) even with correct header => 403
// - Access-log decision assertion: allow path records the IP source and rule
// - Access-log decision assertion: deny path records "deny:no-match"
//
// Required conf files (edgion-tests/integration/conf/EdgionPlugins/RequestAccessPolicy/):
//   01_Secret_edgion-test_gac-origin-secret.yaml
//   02_EdgionConfigData_edgion-test_gac-selector.yaml
//   03_EdgionPlugins_edgion-test_gac-lockdown.yaml
//   04_EdgionPlugins_edgion-test_gac-ip-outside.yaml
//   HTTPRoute_edgion-test_gac.yaml

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::edgion_plugins::hmac_test_utils::build_hmac_headers;
use crate::suites::status::helpers;
use serde::{Deserialize, Serialize};
use std::time::Instant;

pub struct RequestAccessPolicyTestSuite;

const HMAC_HOST: &str = "gac-hmac.example.com";
const URL_REF_HOST: &str = "gac-url.example.com";
const HMAC_USERNAME: &str = "policy-user";
const HMAC_SECRET: &[u8] = b"policy-hmac-secret-key-must-be-at-least-32-bytes";
const BAD_HMAC_SECRET: &[u8] = b"policy-hmac-wrong-secret-key-must-also-be-long";

async fn send_hmac_request(
    ctx: &TestContext,
    method: &str,
    path: &str,
    signing_body: Option<&[u8]>,
    request_body: Option<&[u8]>,
    secret: &[u8],
    fallback: bool,
) -> reqwest::Result<reqwest::Response> {
    let signed = build_hmac_headers(HMAC_USERNAME, secret, method, path, None, HMAC_HOST, signing_body);
    let url = format!("{}{path}", ctx.http_url());
    let method = reqwest::Method::from_bytes(method.as_bytes()).expect("static HTTP method");
    let mut request = ctx
        .http_client
        .request(method, url)
        .header("host", HMAC_HOST)
        .header("date", signed.date)
        .header("authorization", signed.authorization);
    if let Some(digest) = signed.digest {
        request = request.header("digest", digest);
    }
    if fallback {
        request = request.header("X-Hmac-Fallback", "allow");
    }
    if let Some(body) = request_body {
        request = request.body(body.to_vec());
    }
    request.send().await
}

async fn get_status(ctx: &TestContext, host: &str, path: &str, header: Option<(&str, &str)>) -> reqwest::Result<u16> {
    let mut request = ctx
        .http_client
        .get(format!("{}{path}", ctx.http_url()))
        .header("host", host);
    if let Some((name, value)) = header {
        request = request.header(name, value);
    }
    request.send().await.map(|response| response.status().as_u16())
}

async fn url_ref_hmac_status(ctx: &TestContext, secret: &[u8]) -> reqwest::Result<u16> {
    let path = "/test/gac/url/hmac";
    let signed = build_hmac_headers(HMAC_USERNAME, secret, "GET", path, None, URL_REF_HOST, None);
    ctx.http_client
        .get(format!("{}{path}", ctx.http_url()))
        .header("host", URL_REF_HOST)
        .header("date", signed.date)
        .header("authorization", signed.authorization)
        .send()
        .await
        .map(|response| response.status().as_u16())
}

// ---- Access-log deserialization types (mirrors plugin_condition/conditions.rs) ----

#[derive(Debug, Deserialize, Serialize)]
struct AccessLog {
    #[serde(default)]
    stage_logs: Vec<StageLogs>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct StageLogs {
    stage: String,
    filters: Vec<PluginLog>,
    #[serde(default)]
    edgion_plugins: Vec<EdgionPluginsLog>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct EdgionPluginsLog {
    name: String,
    logs: Vec<PluginLog>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct PluginLog {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cond_skip: Option<String>,
    #[serde(default)]
    log: Vec<String>,
}

impl RequestAccessPolicyTestSuite {
    // ==================== 1. Allow: in-CIDR AND correct secret ====================
    fn test_allow_in_cidr_and_secret() -> TestCase {
        TestCase::new(
            "allow_in_cidr_and_secret",
            "Allow: IP in 127.0.0.0/8 AND correct X-CF-Secret => 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/test/gac/lockdown/api", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", "gac.example.com")
                        .header("X-CF-Secret", "cf-origin-token-123");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Allowed with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 2. Deny: secret header missing ====================
    fn test_deny_secret_missing() -> TestCase {
        TestCase::new(
            "deny_secret_missing",
            "Deny: X-CF-Secret header absent => 403",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/test/gac/lockdown/api", ctx.http_url());

                    let request = client.get(&url).header("host", "gac.example.com");
                    // No X-CF-Secret header -- onMissing: NoMatch => group fails => deny.

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Missing secret denied with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 403, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 3. Deny: wrong secret value ====================
    fn test_deny_secret_wrong() -> TestCase {
        TestCase::new(
            "deny_secret_wrong",
            "Deny: X-CF-Secret has wrong value => 403",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/test/gac/lockdown/api", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", "gac.example.com")
                        .header("X-CF-Secret", "nope");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Wrong secret denied with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 403, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_deny_non_selected_secret_value() -> TestCase {
        TestCase::new(
            "deny_non_selected_secret_value",
            "Deny: a non-selected value from the referenced Secret must not match",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match get_status(
                        &ctx,
                        "gac.example.com",
                        "/test/gac/lockdown/api",
                        Some(("X-CF-Secret", "public-origin-user")),
                    )
                    .await
                    {
                        Ok(403) => TestResult::passed(start.elapsed()),
                        Ok(status) => TestResult::failed(start.elapsed(), format!("Expected status 403, got {status}")),
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    // ==================== 4. Deny: IP outside CIDR (10.0.0.0/8, test client is 127.0.0.1) ====================
    fn test_deny_ip_outside_cidr() -> TestCase {
        TestCase::new(
            "deny_ip_outside_cidr",
            "Deny: IP 127.0.0.1 outside 10.0.0.0/8 even with correct secret => 403",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/test/gac/ip-outside/api", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", "gac.example.com")
                        .header("X-CF-Secret", "cf-origin-token-123");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("IP outside CIDR denied with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 403, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 5. Access log: allow path records source and rule ====================
    fn test_access_log_allow_decision() -> TestCase {
        TestCase::new(
            "access_log_allow_decision",
            "Access log: allow path records allow:matched",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let trace_id = format!("test-gac-allow-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/test/gac/lockdown/api", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", "gac.example.com")
                        .header("X-CF-Secret", "cf-origin-token-123")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {} -- cannot assert access log", status),
                                );
                            }

                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => match serde_json::from_value::<AccessLog>(entry.data) {
                                    Ok(access_log) => {
                                        let policy_log = access_log
                                            .stage_logs
                                            .iter()
                                            .flat_map(|stage| &stage.edgion_plugins)
                                            .flat_map(|ep| &ep.logs)
                                            .find(|log| log.name == "RequestAccessPolicy");

                                        match policy_log {
                                            Some(log) => {
                                                if log.log.iter().any(|l| l.contains("allow:matched")) {
                                                    TestResult::passed_with_message(
                                                        start.elapsed(),
                                                        "Access log contains allow:matched".to_string(),
                                                    )
                                                } else {
                                                    TestResult::failed(
                                                        start.elapsed(),
                                                        format!("Expected allow:matched in log, got: {:?}", log.log),
                                                    )
                                                }
                                            }
                                            None => TestResult::failed(
                                                start.elapsed(),
                                                "RequestAccessPolicy plugin log not found in edgion_plugins"
                                                    .to_string(),
                                            ),
                                        }
                                    }
                                    Err(e) => TestResult::failed(start.elapsed(), format!("Parse error: {}", e)),
                                },
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("Failed to fetch access log: {}", e))
                                }
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 6. Access log: deny path records "deny:no-match" ====================
    fn test_access_log_deny_decision() -> TestCase {
        TestCase::new(
            "access_log_deny_decision",
            "Access log: deny path records deny:no-match",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let trace_id = format!("test-gac-deny-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/test/gac/lockdown/api", ctx.http_url());

                    // No X-CF-Secret => deny:no-match expected.
                    let request = client
                        .get(&url)
                        .header("host", "gac.example.com")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 403 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 403, got {} -- cannot assert access log", status),
                                );
                            }

                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => match serde_json::from_value::<AccessLog>(entry.data) {
                                    Ok(access_log) => {
                                        let policy_log = access_log
                                            .stage_logs
                                            .iter()
                                            .flat_map(|stage| &stage.edgion_plugins)
                                            .flat_map(|ep| &ep.logs)
                                            .find(|log| log.name == "RequestAccessPolicy");

                                        match policy_log {
                                            Some(log) => {
                                                if log.log.iter().any(|l| l.contains("deny:no-match")) {
                                                    TestResult::passed_with_message(
                                                        start.elapsed(),
                                                        "Access log contains deny:no-match".to_string(),
                                                    )
                                                } else {
                                                    TestResult::failed(
                                                        start.elapsed(),
                                                        format!("Expected deny:no-match in log, got: {:?}", log.log),
                                                    )
                                                }
                                            }
                                            None => TestResult::failed(
                                                start.elapsed(),
                                                "RequestAccessPolicy plugin log not found in edgion_plugins"
                                                    .to_string(),
                                            ),
                                        }
                                    }
                                    Err(e) => TestResult::failed(start.elapsed(), format!("Parse error: {}", e)),
                                },
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("Failed to fetch access log: {}", e))
                                }
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_hmac_valid_and_invalid_policy_outcomes() -> TestCase {
        TestCase::new(
            "hmac_valid_and_invalid_policy_outcomes",
            "Nested HMAC accepts a valid signature and maps invalid or missing auth to policy 403",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let valid = send_hmac_request(&ctx, "GET", "/health", None, None, HMAC_SECRET, false).await;
                    let invalid = send_hmac_request(&ctx, "GET", "/health", None, None, BAD_HMAC_SECRET, false).await;
                    let missing = ctx
                        .http_client
                        .get(format!("{}/health", ctx.http_url()))
                        .header("host", HMAC_HOST)
                        .send()
                        .await;
                    match (valid, invalid, missing) {
                        (Ok(valid), Ok(invalid), Ok(missing))
                            if valid.status().as_u16() == 200
                                && invalid.status().as_u16() == 403
                                && missing.status().as_u16() == 403 =>
                        {
                            TestResult::passed(start.elapsed())
                        }
                        (Ok(valid), Ok(invalid), Ok(missing)) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected valid/invalid/missing = 200/403/403, got {}/{}/{}",
                                valid.status(),
                                invalid.status(),
                                missing.status()
                            ),
                        ),
                        (valid, invalid, missing) => TestResult::failed(
                            start.elapsed(),
                            format!("Request failure: valid={valid:?}, invalid={invalid:?}, missing={missing:?}"),
                        ),
                    }
                })
            },
        )
    }

    fn test_hmac_invalid_signature_allows_explicit_sibling_fallback() -> TestCase {
        TestCase::new(
            "hmac_invalid_signature_allows_explicit_sibling_fallback",
            "Invalid HMAC is a non-match, so an explicit sibling rule can authorize",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match send_hmac_request(&ctx, "GET", "/fallback", None, None, BAD_HMAC_SECRET, true).await {
                        Ok(response) if response.status().as_u16() == 200 => TestResult::passed(start.elapsed()),
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Expected 200, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    fn test_hmac_policy_does_not_apply_standalone_header_mutations() -> TestCase {
        TestCase::new(
            "hmac_policy_does_not_apply_standalone_header_mutations",
            "Policy HMAC preserves Authorization and ignores credential metadata headers",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match send_hmac_request(&ctx, "GET", "/headers", None, None, HMAC_SECRET, false).await {
                        Ok(response) if response.status().as_u16() == 200 => {
                            let body = response.text().await.unwrap_or_default().to_ascii_lowercase();
                            let authorization_preserved = body.contains("authorization");
                            let metadata_absent =
                                !body.contains("x-policy-metadata") && !body.contains("should-not-forward");
                            if authorization_preserved && metadata_absent {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Unexpected upstream headers: authorization_preserved={authorization_preserved}, metadata_absent={metadata_absent}, body={body}"
                                    ),
                                )
                            }
                        }
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Expected 200, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    fn test_hmac_body_digest_and_forwardable_fallback() -> TestCase {
        TestCase::new(
            "hmac_body_digest_and_forwardable_fallback",
            "Policy HMAC validates Digest, denies a mismatch, and preserves the body for sibling fallback",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let original = b"signed-policy-body";
                    let tampered = b"tampered-policy-body";
                    let valid = send_hmac_request(
                        &ctx,
                        "POST",
                        "/echo",
                        Some(original),
                        Some(original),
                        HMAC_SECRET,
                        false,
                    )
                    .await;
                    let denied = send_hmac_request(
                        &ctx,
                        "POST",
                        "/echo",
                        Some(original),
                        Some(tampered),
                        HMAC_SECRET,
                        false,
                    )
                    .await;
                    let fallback =
                        send_hmac_request(&ctx, "POST", "/echo", Some(original), Some(tampered), HMAC_SECRET, true)
                            .await;
                    match (valid, denied, fallback) {
                        (Ok(valid), Ok(denied), Ok(fallback))
                            if valid.status().as_u16() == 200
                                && denied.status().as_u16() == 403
                                && fallback.status().as_u16() == 200 =>
                        {
                            let body = fallback.text().await.unwrap_or_default();
                            if body.contains("tampered-policy-body") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Fallback did not forward the captured body: {body}"),
                                )
                            }
                        }
                        (Ok(valid), Ok(denied), Ok(fallback)) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected valid/denied/fallback = 200/403/200, got {}/{}/{}",
                                valid.status(),
                                denied.status(),
                                fallback.status()
                            ),
                        ),
                        (valid, denied, fallback) => TestResult::failed(
                            start.elapsed(),
                            format!("Request failure: valid={valid:?}, denied={denied:?}, fallback={fallback:?}"),
                        ),
                    }
                })
            },
        )
    }

    fn test_imported_url_secret_and_ip_dependencies() -> TestCase {
        TestCase::new(
            "imported_url_secret_and_ip_dependencies",
            "Imported URL candidates bind nested Secret and IpList dependencies",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let secret_path = "/test/gac/url/secret/vendor-a";
                    let outcomes = [
                        get_status(
                            &ctx,
                            URL_REF_HOST,
                            secret_path,
                            Some(("X-CF-Secret", "cf-origin-token-123")),
                        )
                        .await,
                        get_status(&ctx, URL_REF_HOST, secret_path, None).await,
                        get_status(&ctx, URL_REF_HOST, secret_path, Some(("X-CF-Secret", "wrong"))).await,
                        get_status(&ctx, URL_REF_HOST, "/test/gac/url/ip-allow", None).await,
                        get_status(&ctx, URL_REF_HOST, "/test/gac/url/ip-deny", None).await,
                    ];
                    let actual = outcomes
                        .into_iter()
                        .map(|outcome| outcome.map_err(|error| error.to_string()))
                        .collect::<Result<Vec<_>, _>>();
                    match actual {
                        Ok(actual) if actual == [200, 403, 403, 200, 403] => TestResult::passed(start.elapsed()),
                        Ok(actual) => TestResult::failed(
                            start.elapsed(),
                            format!("Expected Secret/IP statuses [200, 403, 403, 200, 403], got {actual:?}"),
                        ),
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    fn test_imported_url_path_matchers() -> TestCase {
        TestCase::new(
            "imported_url_path_matchers",
            "One compiled URL index preserves exact, prefix, parameter, regex, and host ownership",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let cases = [
                        (URL_REF_HOST, "/test/gac/url/exact", 200),
                        (URL_REF_HOST, "/test/gac/url/exact/child", 403),
                        (URL_REF_HOST, "/test/gac/url/prefix/child", 200),
                        (URL_REF_HOST, "/test/gac/url/prefix-other", 403),
                        (URL_REF_HOST, "/test/gac/url/param/vendor-a", 200),
                        (URL_REF_HOST, "/test/gac/url/param", 403),
                        (URL_REF_HOST, "/test/gac/url/regex/42", 200),
                        (URL_REF_HOST, "/test/gac/url/regex/not-a-number", 403),
                        ("GAC-URL.EXAMPLE.COM:31180", "/test/gac/url/exact", 200),
                        ("wrong.example.com", "/test/gac/url/exact", 403),
                    ];
                    for (host, path, expected) in cases {
                        match get_status(&ctx, host, path, None).await {
                            Ok(actual) if actual == expected => {}
                            Ok(actual) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected {host}{path} to return {expected}, got {actual}"),
                                );
                            }
                            Err(error) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Request to {host}{path} failed: {error}"),
                                );
                            }
                        }
                    }
                    TestResult::passed(start.elapsed())
                })
            },
        )
    }

    fn test_imported_url_hmac_dependency() -> TestCase {
        TestCase::new(
            "imported_url_hmac_dependency",
            "Imported URL candidate resolves its nested HMAC Secret and denies invalid signatures",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match (
                        url_ref_hmac_status(&ctx, HMAC_SECRET).await,
                        url_ref_hmac_status(&ctx, BAD_HMAC_SECRET).await,
                    ) {
                        (Ok(200), Ok(403)) => TestResult::passed(start.elapsed()),
                        (Ok(valid), Ok(invalid)) => TestResult::failed(
                            start.elapsed(),
                            format!("Expected valid/invalid imported HMAC statuses 200/403, got {valid}/{invalid}"),
                        ),
                        (valid, invalid) => TestResult::failed(
                            start.elapsed(),
                            format!("Request failure: valid={valid:?}, invalid={invalid:?}"),
                        ),
                    }
                })
            },
        )
    }

    fn test_missing_url_ref_is_partial_not_unresolved() -> TestCase {
        TestCase::new(
            "missing_url_ref_is_partial_not_unresolved",
            "A missing optional URL list preserves valid siblings and reports PartiallyInvalid",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match helpers::fetch_resource_status(&ctx, "EdgionPlugins", "edgion-test", "gac-url-refs").await {
                        Ok(resource)
                            if helpers::has_condition(&resource, "Accepted", "True")
                                && helpers::has_condition(&resource, "ResolvedRefs", "True")
                                && helpers::has_condition(&resource, "PartiallyInvalid", "True") =>
                        {
                            TestResult::passed(start.elapsed())
                        }
                        Ok(resource) => TestResult::failed(
                            start.elapsed(),
                            format!("Unexpected RequestAccessPolicy status: {}", resource["status"]),
                        ),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }
}

impl TestSuite for RequestAccessPolicyTestSuite {
    fn name(&self) -> &str {
        "RequestAccessPolicy Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            // Static behavior cases
            Self::test_allow_in_cidr_and_secret(),
            Self::test_deny_secret_missing(),
            Self::test_deny_secret_wrong(),
            Self::test_deny_non_selected_secret_value(),
            Self::test_deny_ip_outside_cidr(),
            // Access-log decision assertions
            Self::test_access_log_allow_decision(),
            Self::test_access_log_deny_decision(),
            // Nested HMAC: Controller Secret binding -> conf_sync -> Gateway policy evaluation.
            Self::test_hmac_valid_and_invalid_policy_outcomes(),
            Self::test_hmac_invalid_signature_allows_explicit_sibling_fallback(),
            Self::test_hmac_policy_does_not_apply_standalone_header_mutations(),
            Self::test_hmac_body_digest_and_forwardable_fallback(),
            // Imported RequestAccessUrlAllowList, including nested second-level dependencies.
            Self::test_imported_url_secret_and_ip_dependencies(),
            Self::test_imported_url_path_matchers(),
            Self::test_imported_url_hmac_dependency(),
            Self::test_missing_url_ref_is_partial_not_unresolved(),
        ]
    }
}
