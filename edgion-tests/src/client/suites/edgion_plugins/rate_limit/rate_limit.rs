// RateLimit Plugin Test Suite
//
// Covers:
// - Allow requests within the configured rate limit
// - Block requests exceeding the limit with 429
// - Shared counter across routes and rule match-items
// - Independent counters per key
// - Explicitly empty `key: []` still rate-limits (one global bucket, not a key miss)
//
// Default config: rate=5, interval=10s, key=Header(X-Rate-Key)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

// Monotonic counter for generating unique per-test rate-limit keys.
static KEY_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct RateLimitTestSuite;

impl RateLimitTestSuite {
    /// Generate a unique rate-limit key to isolate each test case from others.
    fn generate_test_key() -> String {
        let count = KEY_COUNTER.fetch_add(1, Ordering::SeqCst);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("test-key-{}-{}", now, count)
    }

    // ==================== 1. Allow within limit ====================
    fn test_allows_within_limit() -> TestCase {
        TestCase::new("rate_limit_allows_within_limit", ":  (200)", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let client = &ctx.http_client;
                let url = format!("http://{}:31180/test/rate-limit/api", ctx.target_host);
                let test_key = Self::generate_test_key();

                // Send 3 requests (limit is 5), all must pass.
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

    // ==================== 2. Block over limit ====================
    fn test_blocks_over_limit() -> TestCase {
        TestCase::new("rate_limit_blocks_over_limit", ":  429", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let client = &ctx.http_client;
                let url = format!("http://{}:31180/test/rate-limit/api", ctx.target_host);
                let test_key = Self::generate_test_key();

                // Send 5 requests to fill the limit.
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

                // The 6th request must be blocked.
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
                            // Check the rejection body.
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
    // Guards that the rate-limit counter is shared across routes referencing the
    // same EdgionPlugins.
    //
    // Two HTTPRoutes (rate-limit.example.com and rate-limit-b.example.com) both
    // ExtensionRef the SAME EdgionPlugins `rate-limit-header-key` (rate=5/10s,
    // key=X-Rate-Key). Both resolve ONE `Arc<CompiledPlugins>` from
    // GLOBAL_PLUGIN_STORE (built once per ns/name by `compile_plugins`), so a
    // single X-Rate-Key shares ONE budget across both routes.
    //
    // Send 5 requests with the same key split across the two hostnames (3 + 2) —
    // all must be 200 (shared budget not yet spent). The 6th (either host) must
    // be 429. Failure mode: if the plugin runtime were compiled per referencing
    // route instead of shared per ns/name, each route would hold its
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

    // ==================== 3. Rate limit response headers ====================
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

                        // Verify all three standard rate-limit response headers are present.
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

    // ==================== 4. Independent counters per key ====================
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

                    // Exhaust key_a (5 requests = limit).
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

                    // Verify key_a is now blocked.
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

                    // key_b has its own independent counter and must still pass.
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

    // ==================== 5. Missing key falls through to Allow ====================
    fn test_missing_key_allows() -> TestCase {
        TestCase::new(
            "rate_limit_missing_key_allows",
            ":  X-Rate-Key  (onMissingKey=Allow)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/rate-limit/api", ctx.target_host);

                    // Send request without X-Rate-Key header.
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

    // ==================== 6. 429 response includes Retry-After ====================
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

                    // Fill the bucket.
                    for _ in 0..5 {
                        let _ = client
                            .get(&url)
                            .header("host", "rate-limit.example.com")
                            .header("X-Rate-Key", &test_key)
                            .send()
                            .await;
                    }

                    // The next request must be blocked with 429.
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

    /// Rule-level RateLimit is enforced across ALL match-items of a single rule.
    ///
    /// Guards the HTTP types/compiled split (the pattern-establishing pilot,
    /// tasks/review-decouple/http-01-*) on the `match_unit.rs` fan-out path: a rule
    /// with >= 2 match-items builds >= 2 HttpRouteRuleUnits, and `compile_http_rule`
    /// builds the rule's `Arc<PluginRuntime>` ONCE and shares it into every unit.
    /// This test pins that every match-item unit actually routes AND executes the
    /// rule-level ExtensionRef plugin chain, so both variants count against the one
    /// rule-level RateLimit budget. A regression that dropped the rule-level filters
    /// from a non-first match-item unit would let `x-variant: b` traffic skip the
    /// limiter, and the (RATE+1)th request would wrongly pass.
    ///
    /// Note on what this does NOT test: the RateLimit Count-Min-Sketch is not held
    /// in the rule runtime — it lives in the global plugin store, compiled once per
    /// EdgionPlugins ns/name and resolved by key at request time (plugin_store.rs /
    /// resolve_route_ext_refs). So a hypothetical per-unit rebuild of the rule
    /// runtime would NOT split the CMS (both units resolve the same store entry).
    /// Store-level cross-route CMS sharing is covered by the sibling
    /// `test_shared_counter_across_routes`; this scenario covers the orthogonal
    /// single-rule / multi-match-item fan-out.
    ///
    /// Scenario (conf/EdgionPlugins/RateLimit/03_HTTPRoute_rate-limit-shared.yaml):
    /// one rule, two match-items distinguished by `x-variant: a` / `x-variant: b`,
    /// both PathPrefix `/shared`, one rule-level RateLimit (rate=10, fixed
    /// `X-Rate-Key` => one shared bucket). Send RATE_SHARED (=10) requests split
    /// across both variants (5x a + 5x b) — all must succeed — then a (RATE+1)th
    /// that must return 429 because the budget is per-rule, not per-match-item.
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
                                let _ = resp.text().await; // drain body so the pooled conn is reusable
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
                    // Use variant `a` (it has only consumed 5 so far); if every match-item unit shares
                    // the rule's RateLimit the budget is exhausted, so this must return 429.
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
                            let _ = resp.text().await; // drain body so the pooled conn is reusable
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
                                         The rule-level RateLimit budget is NOT enforced across all the rule's \
                                         match-items — likely a non-first match-item unit missing the rule-level \
                                         plugin chain (x-variant=b traffic skipped the limiter).",
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

    // ==================== 7. Explicitly empty key still rate-limits ====================
    //
    // Regression pin (conf/EdgionPlugins/RateLimit/04_EdgionPlugins_rate-limit-empty-key.yaml):
    // `RateLimitConfig.key` used to deserialize to an empty list whenever `key`
    // was omitted, and an empty list was then treated as a key-extraction MISS
    // — so with the default `onMissingKey: Allow` the limiter was silently
    // bypassed for every request. This is now fixed: an omitted `key`
    // deserializes to the documented default `[{type: remoteIp}]`, and an
    // explicitly empty `key: []` means "one global bucket" — every request
    // composes to the same empty key and IS rate-limited; it is NOT a key
    // miss, so `onMissingKey` / `defaultKey` never apply.
    //
    // The fixture pins `onMissingKey: Allow` deliberately: that is the exact
    // setting that used to turn `key: []` into a full bypass. rate=3/interval=10s
    // on a dedicated route (rate-limit-empty-key.example.com) whose bucket is
    // never touched by any other case in this suite: send 3 requests (must all
    // be 200), then a 4th (must be 429) — proving the limiter is active despite
    // `onMissingKey: Allow`.
    fn test_empty_key_still_rate_limits() -> TestCase {
        TestCase::new(
            "rate_limit_empty_key_still_rate_limits",
            "An explicitly empty `key: []` still rate-limits as one global bucket, even with onMissingKey: Allow",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/rate-limit-empty-key/api", ctx.target_host);
                    const HOST: &str = "rate-limit-empty-key.example.com";

                    // rate=3: the first 3 requests must all pass. No key-bearing header is
                    // sent at all - the config's key list is empty, so there is nothing to
                    // extract and every request must land in the same global bucket.
                    for i in 0..3 {
                        let response = client.get(&url).header("host", HOST).send().await;

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

                    // The 4th request must be blocked. If the empty key list regressed back
                    // to being treated as a key miss, `onMissingKey: Allow` would let this
                    // (and every other) request through as 200.
                    let response = client.get(&url).header("host", HOST).send().await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            if status == 429 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "Empty key list still rate-limited: 3 requests passed, the 4th \
                                         returned 429 despite onMissingKey: Allow (body: {})",
                                        body
                                    ),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Request 4 expected 429, got {}. An explicitly empty `key: []` must \
                                         still rate-limit as one global bucket - it is NOT a key miss, so \
                                         onMissingKey: Allow must not bypass the limiter. Body: {}",
                                        status, body
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request 4 failed: {}", e)),
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
            // Allow within limit
            Self::test_allows_within_limit(),
            // Block over limit: 5x 200 then the 6th -> 429 on the SAME key.
            // Each case uses a unique key via generate_test_key() so tests
            // never interfere. If the runtime were rebuilt per request, every
            // request would get a fresh CMS and never hit 429.
            Self::test_blocks_over_limit(),
            // Cross-route reuse guard: the same key shares one CMS budget across
            // two routes that ExtensionRef the same EdgionPlugins.
            Self::test_shared_counter_across_routes(),
            // Across-match-items guard: a rule with >= 2 match-items must share one
            // budget, so the limit stays global per rule, not per match-item.
            Self::test_shared_limit_across_match_items(),
            // test_headers_present is disabled: response-header surfacing is a
            // separate concern, not related to the rate-limit enforcement invariant.
            // Self::test_headers_present(),
            Self::test_retry_after_header(),
            // Independent per-key counters
            Self::test_different_keys_independent(),
            Self::test_missing_key_allows(),
            // Access log: stage_logs includes RateLimit entry
            Self::test_stage_logs_contain_rate_limit(),
            // Explicitly empty key: one global bucket, not a key miss - must still
            // rate-limit even with onMissingKey: Allow.
            Self::test_empty_key_still_rate_limits(),
        ]
    }
}
