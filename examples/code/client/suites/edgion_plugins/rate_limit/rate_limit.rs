// RateLimit Plugin Test Suite
//
// ：
// -
// -  429
// -
// -  key
//
// ：rate=5, interval=10s, key=Header(X-Rate-Key)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

//  key
static KEY_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct RateLimitTestSuite;

impl RateLimitTestSuite {
    ///  key，
    fn generate_test_key() -> String {
        let count = KEY_COUNTER.fetch_add(1, Ordering::SeqCst);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("test-key-{}-{}", now, count)
    }

    // ==================== 1.  ====================
    fn test_allows_within_limit() -> TestCase {
        TestCase::new("rate_limit_allows_within_limit", ":  (200)", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let client = &ctx.http_client;
                let url = format!("http://{}:31180/test/rate-limit/api", ctx.target_host);
                let test_key = Self::generate_test_key();

                //  3 （ 5），
                for i in 0..3 {
                    let response = client
                        .get(&url)
                        .header("host", "rate-limit.example.com")
                        .header("X-Rate-Key", &test_key)
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Request {} expected 200, got {}", i + 1, status),
                                );
                            }
                        }
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Request {} failed: {}", i + 1, e));
                        }
                    }
                }

                TestResult::passed_with_message(
                    start.elapsed(),
                    format!("All 3 requests passed within limit (key: {})", test_key),
                )
            })
        })
    }

    // ==================== 2.  ====================
    fn test_blocks_over_limit() -> TestCase {
        TestCase::new("rate_limit_blocks_over_limit", ":  429", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let client = &ctx.http_client;
                let url = format!("http://{}:31180/test/rate-limit/api", ctx.target_host);
                let test_key = Self::generate_test_key();

                //  5
                for i in 0..5 {
                    let response = client
                        .get(&url)
                        .header("host", "rate-limit.example.com")
                        .header("X-Rate-Key", &test_key)
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let _ = resp.text().await; // drain body so the pooled conn is reusable
                            if status != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Pre-fill request {} expected 200, got {}", i + 1, status),
                                );
                            }
                        }
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Pre-fill request {} failed: {}", i + 1, e),
                            );
                        }
                    }
                }

                //  6
                let response = client
                    .get(&url)
                    .header("host", "rate-limit.example.com")
                    .header("X-Rate-Key", &test_key)
                    .send()
                    .await;

                match response {
                    Ok(resp) => {
                        let status = resp.status().as_u16();
                        if status == 429 {
                            //
                            let body = resp.text().await.unwrap_or_default();
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("Request 6 blocked with 429 (body: {})", body),
                            )
                        } else {
                            TestResult::failed(start.elapsed(), format!("Request 6 expected 429, got {}", status))
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), format!("Request 6 failed: {}", e)),
                }
            })
        })
    }

    // ==================== 2b. Cross-route shared counter ====================
    //
    // Shared-CMS regression pin for the types-decouple plugin side-table.
    //
    // Two HTTPRoutes (rate-limit.example.com and rate-limit-b.example.com) both
    // ExtensionRef the SAME EdgionPlugins `rate-limit-header-key` (rate=5/10s,
    // key=X-Rate-Key). Both resolve ONE `Arc<CompiledPlugins>` from
    // GLOBAL_PLUGIN_STORE (built once per ns/name by `compile_plugins`), so a
    // single X-Rate-Key shares ONE budget across both routes.
    //
    // Send 5 requests with the same key split across the two hostnames (3 + 2) —
    // all must be 200 (shared budget not yet spent). The 6th (either host) must
    // be 429. Regression caught: if the plugin runtime were compiled per
    // referencing route instead of shared per ns/name, each route would hold its
    // own CMS (3 and 2, both < 5), so the 6th request would still pass — this
    // test would then fail on the "expected 429" assertion, flagging the doubled
    // effective limit. This is the HTTP/side-table analog of
    // `grpc_ratelimit_shared_across_match_items`.
    fn test_shared_counter_across_routes() -> TestCase {
        TestCase::new(
            "rate_limit_shared_counter_across_routes",
            "in-memory RateLimit budget is shared across routes referencing the same EdgionPlugins",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/rate-limit/api", ctx.target_host);
                    let test_key = Self::generate_test_key();

                    const HOST_A: &str = "rate-limit.example.com";
                    const HOST_B: &str = "rate-limit-b.example.com";

                    // 5 requests (rate=5) split across the two routes, same key.
                    // Hosts: A, B, A, B, A => 3 via A + 2 via B; the shared budget
                    // must count all 5 against the one X-Rate-Key.
                    let hosts = [HOST_A, HOST_B, HOST_A, HOST_B, HOST_A];
                    for (i, host) in hosts.iter().enumerate() {
                        let response = client
                            .get(&url)
                            .header("host", *host)
                            .header("X-Rate-Key", &test_key)
                            .send()
                            .await;
                        match response {
                            Ok(resp) => {
                                let status = resp.status().as_u16();
                                let _ = resp.text().await; // drain body so the pooled conn is reusable
                                if status != 200 {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "Pre-fill request {} via {} expected 200, got {} \
                                             (shared budget should not be exhausted yet)",
                                            i + 1,
                                            host,
                                            status
                                        ),
                                    );
                                }
                            }
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Pre-fill request {} via {} failed: {}", i + 1, host, e),
                                );
                            }
                        }
                    }

                    // 6th request (via route B) must be rejected: the rule's single
                    // CMS budget for this key is shared across BOTH routes and now spent.
                    let response = client
                        .get(&url)
                        .header("host", HOST_B)
                        .header("X-Rate-Key", &test_key)
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            if status == 429 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "Shared budget enforced across both routes: 5 requests (3 via A + \
                                         2 via B) passed, the 6th (via B) returned 429 (body: {})",
                                        body
                                    ),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Request 6 (via {}) expected 429, got {}. The RateLimit budget is \
                                         NOT shared across routes referencing the same EdgionPlugins — likely \
                                         a per-route PluginRuntime build instead of one Arc<CompiledPlugins> \
                                         shared via the store. Body: {}",
                                        HOST_B, status, body
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request 6 failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 3.  ====================
    fn test_headers_present() -> TestCase {
        TestCase::new("rate_limit_headers_present", ":  X-RateLimit-* ", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let client = &ctx.http_client;
                let url = format!("http://{}:31180/test/rate-limit/api", ctx.target_host);
                let test_key = Self::generate_test_key();

                let response = client
                    .get(&url)
                    .header("host", "rate-limit.example.com")
                    .header("X-Rate-Key", &test_key)
                    .send()
                    .await;

                match response {
                    Ok(resp) => {
                        let status = resp.status().as_u16();
                        let headers = resp.headers();

                        //
                        let limit = headers.get("X-RateLimit-Limit");
                        let remaining = headers.get("X-RateLimit-Remaining");
                        let reset = headers.get("X-RateLimit-Reset");

                        let mut missing_headers = Vec::new();
                        if limit.is_none() {
                            missing_headers.push("X-RateLimit-Limit");
                        }
                        if remaining.is_none() {
                            missing_headers.push("X-RateLimit-Remaining");
                        }
                        if reset.is_none() {
                            missing_headers.push("X-RateLimit-Reset");
                        }

                        if !missing_headers.is_empty() {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Missing headers: {:?}", missing_headers),
                            );
                        }

                        let limit_val = limit.unwrap().to_str().unwrap_or("?");
                        let remaining_val = remaining.unwrap().to_str().unwrap_or("?");
                        let reset_val = reset.unwrap().to_str().unwrap_or("?");

                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!(
                                "Status {}, Limit={}, Remaining={}, Reset={}",
                                status, limit_val, remaining_val, reset_val
                            ),
                        )
                    }
                    Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                }
            })
        })
    }

    // ==================== 4.  key  ====================
    fn test_different_keys_independent() -> TestCase {
        TestCase::new(
            "rate_limit_different_keys_independent",
            ":  key ",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/rate-limit/api", ctx.target_host);

                    let key_a = Self::generate_test_key();
                    let key_b = Self::generate_test_key();

                    //  key_a  (5 )
                    for i in 0..5 {
                        let response = client
                            .get(&url)
                            .header("host", "rate-limit.example.com")
                            .header("X-Rate-Key", &key_a)
                            .send()
                            .await;

                        if let Err(e) = response {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Key A request {} failed: {}", i + 1, e),
                            );
                        }
                    }

                    //  key_a
                    let resp_a = client
                        .get(&url)
                        .header("host", "rate-limit.example.com")
                        .header("X-Rate-Key", &key_a)
                        .send()
                        .await;

                    match resp_a {
                        Ok(resp) => {
                            if resp.status().as_u16() != 429 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Key A should be rate limited, got {}", resp.status()),
                                );
                            }
                        }
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Key A verify failed: {}", e));
                        }
                    }

                    // key_b （）
                    let resp_b = client
                        .get(&url)
                        .header("host", "rate-limit.example.com")
                        .header("X-Rate-Key", &key_b)
                        .send()
                        .await;

                    match resp_b {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Key A (exhausted) blocked 429, Key B (fresh) allowed 200".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Key B expected 200 (independent), got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Key B request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 5.  key  ====================
    fn test_missing_key_allows() -> TestCase {
        TestCase::new(
            "rate_limit_missing_key_allows",
            ":  X-Rate-Key  (onMissingKey=Allow)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/rate-limit/api", ctx.target_host);

                    //  X-Rate-Key header
                    let response = client.get(&url).header("host", "rate-limit.example.com").send().await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Request without X-Rate-Key allowed (onMissingKey=Allow)".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 for missing key, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 6. 429  Retry-After  ====================
    fn test_retry_after_header() -> TestCase {
        TestCase::new(
            "rate_limit_retry_after_header",
            ": 429  Retry-After ",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/rate-limit/api", ctx.target_host);
                    let test_key = Self::generate_test_key();

                    //
                    for _ in 0..5 {
                        let _ = client
                            .get(&url)
                            .header("host", "rate-limit.example.com")
                            .header("X-Rate-Key", &test_key)
                            .send()
                            .await;
                    }

                    //  429
                    let response = client
                        .get(&url)
                        .header("host", "rate-limit.example.com")
                        .header("X-Rate-Key", &test_key)
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 429 {
                                return TestResult::failed(start.elapsed(), format!("Expected 429, got {}", status));
                            }

                            let retry_after = resp.headers().get("Retry-After");
                            if let Some(val) = retry_after {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("429 with Retry-After: {}", val.to_str().unwrap_or("?")),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "429 response missing Retry-After header".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Shared-CMS regression: a rule with two match-items shares one RateLimit window.
    ///
    /// Pins the highest-risk invariant of the HTTP types/compiled split (the
    /// pattern-establishing pilot, tasks/review-decouple/http-01-*): a rule with
    /// >= 2 match-items builds >= 2 HttpRouteRuleUnits, but the rule-level
    /// Arc<PluginRuntime> (and the RateLimit Count-Min-Sketch inside it) MUST be
    /// built ONCE and shared across all of the rule's units. If a regression
    /// rebuilds the runtime per-unit, each match-item gets its own CMS and the
    /// effective limit becomes N x (N = match-item count).
    ///
    /// Scenario (conf/EdgionPlugins/RateLimit/02_HTTPRoute_rate-limit-shared.yaml):
    /// one rule, two match-items distinguished by `x-variant: a` / `x-variant: b`,
    /// both PathPrefix `/shared`, one rule-level RateLimit (rate=10, fixed
    /// `X-Rate-Key` => one shared bucket).
    ///
    /// Send RATE_SHARED (=10) requests split across BOTH variants (5x a + 5x b) —
    /// all must succeed (the global budget is not yet exhausted). The (RATE+1)th
    /// must return 429 because the budget is shared by the rule, not the match-item.
    /// Under a per-unit-split regression each variant would get an independent
    /// budget of 10; after 5a+5b each bucket holds only 5, so the 11th request
    /// would wrongly succeed and this test would fail on the "expected 429" assertion.
    fn test_shared_limit_across_match_items() -> TestCase {
        TestCase::new(
            "rate_limit_shared_across_match_items",
            "Rule-level RateLimit is shared across a rule's match-items (global budget)",
            |ctx: TestContext| {
                Box::pin(async move {
                    const RATE_SHARED: usize = 10;
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/shared", ctx.target_host);
                    let test_key = Self::generate_test_key();

                    // RATE requests alternating a/b => 5x a + 5x b, all within the shared budget.
                    for i in 0..RATE_SHARED {
                        let variant = if i % 2 == 0 { "a" } else { "b" };
                        let response = client
                            .get(&url)
                            .header("host", "http-ratelimit-shared.example.com")
                            .header("X-Rate-Key", &test_key)
                            .header("x-variant", variant)
                            .send()
                            .await;

                        match response {
                            Ok(resp) => {
                                let status = resp.status().as_u16();
                                if status != 200 {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "Request {} (variant {}) expected 200 within the budget of {}, got {}",
                                            i + 1,
                                            variant,
                                            RATE_SHARED,
                                            status
                                        ),
                                    );
                                }
                            }
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Request {} (variant {}) failed: {}", i + 1, variant, e),
                                );
                            }
                        }
                    }

                    // The (RATE+1)th request must be rejected — the rule's single budget is now spent.
                    // Use variant `a` (it has only consumed 5 so far); under the shared invariant the
                    // rule budget is exhausted, under a per-unit-split regression it would still pass.
                    let response = client
                        .get(&url)
                        .header("host", "http-ratelimit-shared.example.com")
                        .header("X-Rate-Key", &test_key)
                        .header("x-variant", "a")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 429 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "Shared budget enforced: {} requests across both match-items succeeded, \
                                         the {}th returned 429",
                                        RATE_SHARED,
                                        RATE_SHARED + 1
                                    ),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Request {} (variant a) returned {} but should have been rate-limited (429). \
                                         The rule-level RateLimit budget is NOT shared across match-items — \
                                         likely a per-unit PluginRuntime split (effective limit inflated to 2x).",
                                        RATE_SHARED + 1,
                                        status
                                    ),
                                )
                            }
                        }
                        Err(e) => {
                            TestResult::failed(start.elapsed(), format!("Request {} failed: {}", RATE_SHARED + 1, e))
                        }
                    }
                })
            },
        )
    }

    fn test_stage_logs_contain_rate_limit() -> TestCase {
        TestCase::new(
            "stage_logs_contain_rate_limit",
            "Access log stage_logs contains RateLimit plugin entry on allowed request",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("test-rate-limit-{}", uuid::Uuid::new_v4());
                    let test_key = format!("al-probe-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/test/rate-limit/api", ctx.http_url());

                    let _ = ctx
                        .http_client
                        .get(&url)
                        .header("host", "rate-limit.example.com")
                        .header("X-Rate-Key", &test_key)
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await;

                    let entry = match ctx
                        .access_log_client()
                        .get_access_log_with_retry(&trace_id, 10, 200)
                        .await
                    {
                        Ok(e) => e,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Access log not found: {}", e)),
                    };

                    let found = entry.data["stage_logs"]
                        .as_array()
                        .map(|stages| {
                            stages.iter().any(|stage| {
                                stage["edgion_plugins"]
                                    .as_array()
                                    .map(|eps| {
                                        eps.iter().any(|ep| {
                                            ep["logs"]
                                                .as_array()
                                                .map(|logs| {
                                                    logs.iter().any(|l| l["name"].as_str() == Some("RateLimit"))
                                                })
                                                .unwrap_or(false)
                                        })
                                    })
                                    .unwrap_or(false)
                            })
                        })
                        .unwrap_or(false);

                    if found {
                        TestResult::passed_with_message(start.elapsed(), "RateLimit found in stage_logs".to_string())
                    } else {
                        TestResult::failed(start.elapsed(), "RateLimit not found in stage_logs".to_string())
                    }
                })
            },
        )
    }
}

impl TestSuite for RateLimitTestSuite {
    fn name(&self) -> &str {
        "RateLimit Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            //
            Self::test_allows_within_limit(),
            // Per-request reuse guard: 5x 200 then the 6th -> 429 on the SAME key.
            // Re-enabled (was disabled for an early-response flakiness concern that
            // the active rate_limit_redis 429 tests prove is not real). Each case
            // uses a unique key via generate_test_key(), so no ~30s window wait is
            // needed and cases never interfere. If the runtime were rebuilt per
            // request, every request would get a fresh CMS and never hit 429.
            Self::test_blocks_over_limit(),
            // Cross-route reuse guard: the same key shares one CMS budget across
            // two routes that ExtensionRef the same EdgionPlugins.
            Self::test_shared_counter_across_routes(),
            // Cross-match-item reuse guard (http-01): a rule with >= 2 match-items
            // builds >= 2 units that must share the rule's single Arc<PluginRuntime>
            // (and its CMS), so the budget stays global per rule, not per match-item.
            Self::test_shared_limit_across_match_items(),
            // NOTE: test_headers_present() stays disabled — separate response-header
            // surfacing issue, unrelated to the runtime-reuse invariant.
            // Self::test_headers_present(),
            Self::test_retry_after_header(),
            // key
            Self::test_different_keys_independent(),
            Self::test_missing_key_allows(),
            // access log regression
            Self::test_stage_logs_contain_rate_limit(),
        ]
    }
}
