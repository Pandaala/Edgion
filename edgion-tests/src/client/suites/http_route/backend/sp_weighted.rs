//! SessionPersistenceWeighted scenario phase suites (GEP-1619 traffic-split
//! semantics: weights govern only sessions without a token; an established
//! session keeps routing to its bound Service/endpoint).
//!
//! Initial phase (weights 70/30, spw-a:30001 / spw-b:30002):
//!   - token-less requests approximately respect the 70/30 split;
//!   - a session established on the 30%-weight backend (spw-b) stays there
//!     for 50 requests — under the old ordering (weighted pick first, then
//!     Service-scoped token resolve) this fails ~70% of the time per request;
//!   - two rules using the DEFAULT session name with disjoint backends do not
//!     share sessions (rule-scope isolation).
//!
//! zero_weight phase: spw-b's weight drops to 0 — new sessions all go to
//! spw-a, the established spw-b session must NOT migrate.
//!
//! final phase: spw-b's endpoints are deleted — the stale session falls back
//! to spw-a and the response carries a refreshed token that then sticks.
//!
//! Backend identity is read client-side from the test_server `/echo` body
//! (`Server: 0.0.0.0:{port}` line) — no access-log lookups needed.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use parking_lot::Mutex;
use std::sync::OnceLock;
use std::time::Instant;

/// Cross-phase cookie store: initial phase writes the spw-b-bound cookie,
/// zero_weight and final phases read it.
static SPW_COOKIE_B: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn cookie_slot() -> &'static Mutex<Option<String>> {
    SPW_COOKIE_B.get_or_init(|| Mutex::new(None))
}

const HOST: &str = "spw.example.com";
const SCOPE_HOST: &str = "spw-scope.example.com";
const PORT: u16 = 31939;
/// Session name must match the YAML `sessionPersistence.sessionName`.
const SESSION_NAME: &str = "edgion-spw";
/// Default session name used when the YAML omits `sessionName` (scope route).
const DEFAULT_SESSION_NAME: &str = "edgion-session-affinity";
const BACKEND_A: u16 = 30001;
const BACKEND_B: u16 = 30002;
/// Split-tolerance bounds for 100 draws at p=0.7 (±~3σ).
const SPLIT_TOTAL: usize = 100;
const SPLIT_A_MIN: usize = 55;
const SPLIT_A_MAX: usize = 85;
const STICKY_REQUESTS: usize = 50;

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(2))
        .timeout(std::time::Duration::from_secs(5))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("Failed to create HTTP client")
}

/// One request against the scenario listener. Returns
/// `(status, backend_port, set_cookie)` where `backend_port` is parsed from the
/// test_server `/echo` body line `Server: 0.0.0.0:{port}`.
async fn send(
    client: &reqwest::Client,
    ctx: &TestContext,
    host: &str,
    path: &str,
    cookie: Option<&str>,
) -> Result<(u16, u16, Option<String>), String> {
    let url = format!("http://{}:{}{}", ctx.target_host, PORT, path);
    let mut req = client.get(&url).header("host", host);
    if let Some(c) = cookie {
        req = req.header("cookie", c);
    }
    let resp = req.send().await.map_err(|e| format!("request failed: {}", e))?;
    let status = resp.status().as_u16();
    let set_cookie = resp
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let body = resp.text().await.map_err(|e| format!("body read failed: {}", e))?;
    let backend_port =
        parse_backend_port(&body).ok_or_else(|| format!("no 'Server: 0.0.0.0:{{port}}' in body: {}", body))?;
    Ok((status, backend_port, set_cookie))
}

/// Extract the backend port from the `/echo` body (`Server: 0.0.0.0:30001`).
fn parse_backend_port(body: &str) -> Option<u16> {
    body.lines()
        .find_map(|l| l.strip_prefix("Server: "))
        .and_then(|addr| addr.rsplit(':').next())
        .and_then(|p| p.trim().parse::<u16>().ok())
}

/// Extract `name=value` from a Set-Cookie header (drop attributes).
fn cookie_pair(set_cookie: &str) -> String {
    set_cookie.split(';').next().unwrap_or(set_cookie).trim().to_string()
}

/// Warm up the listener: retry until the route serves 200 (config propagation).
async fn warmup(client: &reqwest::Client, ctx: &TestContext, host: &str, path: &str) -> Result<(), String> {
    for _ in 0..50 {
        if let Ok((200, _, _)) = send(client, ctx, host, path, None).await {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    Err("route did not become ready within 10s".to_string())
}

// ---------------------------------------------------------------------------
// Initial phase suite
// ---------------------------------------------------------------------------

pub struct SpWeightedInitialTestSuite;

impl SpWeightedInitialTestSuite {
    /// Token-less requests approximately respect the 70/30 weight split, and
    /// every response carries a Set-Cookie (InjectNew on each new session).
    fn test_weighted_split_without_token() -> TestCase {
        TestCase::new(
            "spw_split_without_token",
            "Token-less requests respect the 70/30 split within tolerance",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = http_client();
                    if let Err(e) = warmup(&client, &ctx, HOST, "/echo").await {
                        return TestResult::failed(start.elapsed(), e);
                    }

                    let (mut count_a, mut count_b) = (0usize, 0usize);
                    for i in 0..SPLIT_TOTAL {
                        match send(&client, &ctx, HOST, "/echo", None).await {
                            Ok((200, port, Some(sc))) if sc.starts_with(&format!("{}=", SESSION_NAME)) => match port {
                                BACKEND_A => count_a += 1,
                                BACKEND_B => count_b += 1,
                                other => {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("request {} served by unexpected backend port {}", i, other),
                                    );
                                }
                            },
                            Ok((200, _, Some(sc))) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("request {} Set-Cookie has wrong name: {}", i, sc),
                                );
                            }
                            Ok((200, _, None)) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("request {} (no token) missing Set-Cookie", i),
                                );
                            }
                            Ok((status, _, _)) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("request {} got status {}", i, status),
                                );
                            }
                            Err(e) => return TestResult::failed(start.elapsed(), format!("request {}: {}", i, e)),
                        }
                    }

                    if count_b == 0 {
                        return TestResult::failed(
                            start.elapsed(),
                            "weight-30 backend never selected — topology cannot exercise the split".to_string(),
                        );
                    }
                    if !(SPLIT_A_MIN..=SPLIT_A_MAX).contains(&count_a) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "split out of tolerance: spw-a={} spw-b={} (expected spw-a in [{}, {}])",
                                count_a, count_b, SPLIT_A_MIN, SPLIT_A_MAX
                            ),
                        );
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!("split OK: spw-a={} spw-b={}", count_a, count_b),
                    )
                })
            },
        )
    }

    /// Establish a session on the 30%-weight backend (spw-b) and verify 50
    /// requests stay there with no token refresh. This is the core GEP-1619
    /// regression: the old ordering re-ran weighted selection per request and
    /// bounced the session between Services.
    fn test_sticky_across_weighted_backends() -> TestCase {
        TestCase::new(
            "spw_sticky_on_minority_backend",
            "A session bound to the weight-30 backend stays there for 50 requests",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = http_client();

                    // Establish a session on spw-b (~30% per try; 60 tries ≈ certain).
                    let mut cookie_b: Option<String> = None;
                    for _ in 0..60 {
                        match send(&client, &ctx, HOST, "/echo", None).await {
                            Ok((200, BACKEND_B, Some(sc))) => {
                                cookie_b = Some(cookie_pair(&sc));
                                break;
                            }
                            Ok((200, _, _)) => continue,
                            Ok((status, _, _)) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("session-establish request got status {}", status),
                                );
                            }
                            Err(e) => return TestResult::failed(start.elapsed(), e),
                        }
                    }
                    let Some(cookie_b) = cookie_b else {
                        return TestResult::failed(
                            start.elapsed(),
                            "could not establish a session on spw-b in 60 tries".to_string(),
                        );
                    };

                    for i in 0..STICKY_REQUESTS {
                        match send(&client, &ctx, HOST, "/echo", Some(&cookie_b)).await {
                            Ok((200, BACKEND_B, None)) => {}
                            Ok((200, BACKEND_B, Some(sc))) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("sticky request {} unexpectedly refreshed the token: {}", i, sc),
                                );
                            }
                            Ok((200, port, _)) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("sticky request {} migrated to backend port {}", i, port),
                                );
                            }
                            Ok((status, _, _)) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("sticky request {} got status {}", i, status),
                                );
                            }
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("sticky request {}: {}", i, e));
                            }
                        }
                    }

                    // Store for the zero_weight / final phases.
                    *cookie_slot().lock() = Some(cookie_b.clone());

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!("{} requests pinned to spw-b ({})", STICKY_REQUESTS, cookie_b),
                    )
                })
            },
        )
    }

    /// Two rules with the DEFAULT session name and disjoint backends must not
    /// share sessions: a cookie minted on /r1 (spw-a) replayed on /r2 (spw-b)
    /// must miss, serve spw-b, and refresh the token.
    fn test_rule_scope_isolation() -> TestCase {
        TestCase::new(
            "spw_rule_scope_isolation",
            "Default-named sessions do not leak across route rules with disjoint backends",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = http_client();
                    if let Err(e) = warmup(&client, &ctx, SCOPE_HOST, "/r1/echo").await {
                        return TestResult::failed(start.elapsed(), e);
                    }

                    let (status, port, sc) = match send(&client, &ctx, SCOPE_HOST, "/r1/echo", None).await {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("/r1 request: {}", e)),
                    };
                    if status != 200 || port != BACKEND_A {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("/r1 expected 200 from spw-a, got status={} port={}", status, port),
                        );
                    }
                    let r1_cookie = match sc {
                        Some(ref s) if s.starts_with(&format!("{}=", DEFAULT_SESSION_NAME)) => cookie_pair(s),
                        other => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("/r1 expected default-named Set-Cookie, got {:?}", other),
                            );
                        }
                    };

                    let (status, port, sc) = match send(&client, &ctx, SCOPE_HOST, "/r2/echo", Some(&r1_cookie)).await {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("/r2 request: {}", e)),
                    };
                    if status != 200 || port != BACKEND_B {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "/r2 with /r1 cookie expected 200 from spw-b, got status={} port={}",
                                status, port
                            ),
                        );
                    }
                    match sc {
                        Some(ref s) if cookie_pair(s) != r1_cookie => {}
                        Some(ref s) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("/r2 reissued the SAME token (session leaked across rules): {}", s),
                            );
                        }
                        None => {
                            return TestResult::failed(
                                start.elapsed(),
                                "/r2 did not refresh the token (foreign token silently accepted)".to_string(),
                            );
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "rule-scope isolation OK: /r1 cookie missed on /r2, token refreshed".to_string(),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for SpWeightedInitialTestSuite {
    fn name(&self) -> &str {
        "SpWeightedInitial"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/SessionPersistenceWeighted")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_weighted_split_without_token(),
            Self::test_sticky_across_weighted_backends(),
            Self::test_rule_scope_isolation(),
        ]
    }
}

// ---------------------------------------------------------------------------
// Zero-weight phase suite (polled by wait_phase)
// ---------------------------------------------------------------------------

pub struct SpWeightedZeroWeightTestSuite;

impl SpWeightedZeroWeightTestSuite {
    /// New sessions all land on spw-a once spw-b's weight is 0. Also the
    /// convergence signal for wait_phase (fails until the update propagates).
    fn test_new_sessions_avoid_zero_weight() -> TestCase {
        TestCase::new(
            "spw_zero_weight_new_sessions_all_a",
            "With weights 100/0, token-less requests all go to spw-a",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = http_client();
                    for i in 0..15 {
                        match send(&client, &ctx, HOST, "/echo", None).await {
                            Ok((200, BACKEND_A, _)) => {}
                            Ok((200, port, _)) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "new-session request {} went to port {} (weight 0 not applied yet?)",
                                        i, port
                                    ),
                                );
                            }
                            Ok((status, _, _)) => {
                                return TestResult::failed(start.elapsed(), format!("request {} status {}", i, status));
                            }
                            Err(e) => return TestResult::failed(start.elapsed(), format!("request {}: {}", i, e)),
                        }
                    }
                    TestResult::passed_with_message(start.elapsed(), "15 new sessions all on spw-a".to_string())
                })
            },
        )
    }

    /// The session established on spw-b in the initial phase must survive the
    /// weight change to 0 while its endpoint is still serving (GEP-1619 MUST).
    fn test_sticky_survives_zero_weight() -> TestCase {
        TestCase::new(
            "spw_zero_weight_sticky_survives",
            "The established spw-b session does not migrate when spw-b's weight becomes 0",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let Some(cookie_b) = cookie_slot().lock().clone() else {
                        return TestResult::failed(
                            start.elapsed(),
                            "no spw-b cookie stored by the initial phase".to_string(),
                        );
                    };
                    let client = http_client();
                    for i in 0..15 {
                        match send(&client, &ctx, HOST, "/echo", Some(&cookie_b)).await {
                            Ok((200, BACKEND_B, None)) => {}
                            Ok((200, port, sc)) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "sticky request {} broke: port={} set_cookie={:?} (expected spw-b, no refresh)",
                                        i, port, sc
                                    ),
                                );
                            }
                            Ok((status, _, _)) => {
                                return TestResult::failed(start.elapsed(), format!("request {} status {}", i, status));
                            }
                            Err(e) => return TestResult::failed(start.elapsed(), format!("request {}: {}", i, e)),
                        }
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "15 sticky requests stayed on weight-0 spw-b".to_string(),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for SpWeightedZeroWeightTestSuite {
    fn name(&self) -> &str {
        "SpWeightedZeroWeight"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/SessionPersistenceWeighted")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_new_sessions_avoid_zero_weight(),
            Self::test_sticky_survives_zero_weight(),
        ]
    }
}

// ---------------------------------------------------------------------------
// Final phase suite: spw-b endpoints deleted (polled by wait_phase)
// ---------------------------------------------------------------------------

pub struct SpWeightedFinalTestSuite;

impl SpWeightedFinalTestSuite {
    /// After spw-b's endpoints are deleted, the stale session falls back to
    /// spw-a and the response carries a refreshed token that then sticks.
    fn test_fallback_and_refresh() -> TestCase {
        TestCase::new(
            "spw_final_fallback_and_refresh",
            "Deleting the bound endpoint triggers fallback to spw-a and a token refresh",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let Some(cookie_b) = cookie_slot().lock().clone() else {
                        return TestResult::failed(
                            start.elapsed(),
                            "no spw-b cookie stored by the initial phase".to_string(),
                        );
                    };
                    let client = http_client();

                    let (status, port, sc) = match send(&client, &ctx, HOST, "/echo", Some(&cookie_b)).await {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    if status != 200 || port != BACKEND_A {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "stale-session request expected 200 from spw-a, got status={} port={}",
                                status, port
                            ),
                        );
                    }
                    let refreshed = match sc {
                        Some(ref s) if cookie_pair(s) != cookie_b => cookie_pair(s),
                        other => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("expected a refreshed token after fallback, got {:?}", other),
                            );
                        }
                    };

                    // The refreshed token must itself stick (to spw-a).
                    for i in 0..5 {
                        match send(&client, &ctx, HOST, "/echo", Some(&refreshed)).await {
                            Ok((200, BACKEND_A, None)) => {}
                            Ok((st, port, sc)) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "refreshed-token request {} not sticky: status={} port={} set_cookie={:?}",
                                        i, st, port, sc
                                    ),
                                );
                            }
                            Err(e) => return TestResult::failed(start.elapsed(), format!("request {}: {}", i, e)),
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "fallback served by spw-a, refreshed token sticks".to_string(),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for SpWeightedFinalTestSuite {
    fn name(&self) -> &str {
        "SpWeightedFinal"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/SessionPersistenceWeighted")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_fallback_and_refresh()]
    }
}
