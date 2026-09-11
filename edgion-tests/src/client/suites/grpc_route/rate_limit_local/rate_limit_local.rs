// gRPC rule-level RateLimitLocal — end-to-end test.
//
// Guards the gRPC match_unit.rs fan-out (mirrors HTTP
// `edgion_plugins::rate_limit::test_shared_limit_across_match_items`).
// `plugin_runtime` is a per-unit field that conf_handler must populate for EVERY
// match-item unit (`compile_grpc_rule` builds it once per rule and Arc-shares it
// into each unit). The K-dimension case pins exactly that: every match-item unit of
// one rule must route AND execute the rule-level ExtensionRef plugin chain, so both
// variants count against the one rule budget. A regression that dropped the rule-level
// filters from a non-first match-item unit would let `x-variant: b` traffic skip the
// limiter and the (RATE+1)th request would wrongly succeed — this test catches that.
//
// What this does NOT pin: the RateLimitLocal Count-Min-Sketch is NOT held in the rule
// runtime — it lives in the global PluginStore, one instance per EdgionPlugins ns/name,
// resolved by key at request time (plugins/http/plugin_store.rs:132,173,
// global_plugins_resolve.rs:49, gateway_api_filters/extension_ref.rs:131-146). So a
// hypothetical per-unit rebuild that still carried the same ExtensionRef would NOT split
// the CMS (both units resolve the same store entry); CMS instance uniqueness is
// unobservable and not pinnable via RateLimitLocal. Store-level cross-route CMS sharing is
// covered on the HTTP side by `edgion_plugins::rate_limit::test_shared_counter_across_routes`.
//
// Also verified end-to-end: gRPC rule-level ExtensionRef plugins execute on the gRPC
// path, and a RateLimitLocal rejection maps HTTP 429 -> gRPC status 8 RESOURCE_EXHAUSTED
// (runtime/server/error_response.rs).
//
// Scenario (conf/GRPCRoute/RateLimitLocal): one rule, two match-items distinguished by
// `x-variant: a` / `x-variant: b`, both routing SayHello, one rule-level RateLimitLocal
// (rate=10, fixed `x-rate-key` => one shared bucket). A sibling case covers the
// hostname dimension (one rule, two hostnames, one match-item) — weaker, see its doc.
//
// Required config files (conf/GRPCRoute/RateLimitLocal/):
// - Gateway.yaml                              # grpc listener :31063
// - GRPCRoute_rate-limit-shared.yaml          # 1 rule, 2 header match-items, rule-level RateLimitLocal (K dim)
// - GRPCRoute_rate-limit-shared-hostnames.yaml # 1 rule, 2 hostnames, 1 match-item, rule-level RateLimitLocal (H dim)
// - EdgionPlugins_rate-limit-grpc.yaml        # RateLimitLocal rate=10, key=header x-rate-key
// - Service_test-grpc.yaml                    # gRPC backend service
// - EndpointSlice_test-grpc.yaml              # gRPC backend endpoint (127.0.0.1:30021)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use super::super::test::test_service_client::TestServiceClient;
use super::super::test::HelloRequest;

const HOSTNAME: &str = "grpc-ratelimit.example.com";
// Two hostnames sharing ONE GRPCRoute rule (conf GRPCRoute_rate-limit-shared-hostnames.yaml),
// used by the H-dimension pin: the rule-level budget must stay shared across both.
const HOSTNAME_A: &str = "a.shared.example.com";
const HOSTNAME_B: &str = "b.shared.example.com";
const RATE: usize = 10;

// Per-process counter so each test case uses a distinct `x-rate-key`, keeping the
// 60s RateLimitLocal window of one case from leaking into another (or into a re-run).
static KEY_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct GrpcRateLimitLocalTestSuite;

impl GrpcRateLimitLocalTestSuite {
    /// Unique rate-limit key per invocation (nanos + counter).
    fn generate_test_key() -> String {
        let count = KEY_COUNTER.fetch_add(1, Ordering::SeqCst);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("grpc-rl-{}-{}", now, count)
    }

    /// Build a tonic endpoint whose :authority is the scenario hostname.
    fn make_endpoint(ctx: &TestContext, hostname: &str) -> Result<tonic::transport::Endpoint, String> {
        let grpc_url = format!("http://{}:{}", ctx.target_host, ctx.grpc_port);
        let endpoint =
            tonic::transport::Endpoint::from_shared(grpc_url).map_err(|e| format!("Invalid endpoint: {}", e))?;
        let origin_uri: http::Uri = format!("http://{}:{}", hostname, ctx.grpc_port)
            .parse()
            .map_err(|e| format!("Invalid origin URI: {}", e))?;
        Ok(endpoint.origin(origin_uri))
    }

    /// Send one SayHello carrying the shared rate-key and the given variant header.
    /// Returns Ok(()) on a successful RPC, or Err(grpc Code) when the gateway rejects.
    async fn say_hello(
        client: &mut TestServiceClient<tonic::transport::Channel>,
        rate_key: &str,
        variant: &str,
    ) -> Result<(), tonic::Code> {
        let mut request = tonic::Request::new(HelloRequest {
            name: format!("variant-{}", variant),
        });
        request.metadata_mut().insert("x-rate-key", rate_key.parse().unwrap());
        request.metadata_mut().insert("x-variant", variant.parse().unwrap());
        client.say_hello(request).await.map(|_| ()).map_err(|s| s.code())
    }

    /// Send one SayHello carrying only the shared rate-key (no variant header).
    /// Used by the cross-hostname pin, where the rule has a single match-item and
    /// requests are distinguished by the channel's `:authority` (hostname), not a header.
    async fn say_hello_plain(
        client: &mut TestServiceClient<tonic::transport::Channel>,
        rate_key: &str,
    ) -> Result<(), tonic::Code> {
        let mut request = tonic::Request::new(HelloRequest {
            name: "shared-hostname".to_string(),
        });
        request.metadata_mut().insert("x-rate-key", rate_key.parse().unwrap());
        client.say_hello(request).await.map(|_| ()).map_err(|s| s.code())
    }

    /// Pin: every match-item unit of one rule executes the rule-level plugin chain.
    ///
    /// Send RATE (=10) requests split across BOTH variants (5x a + 5x b) — all must
    /// succeed (global budget not yet exhausted). The (RATE+1)th must be rejected with
    /// RESOURCE_EXHAUSTED.
    ///
    /// WHAT IT PINS: conf_handler must Arc-share the rule's compiled runtime into EVERY
    /// match-item unit. If a non-first match-item unit is missing the rule-level filters,
    /// `x-variant: b` would skip the limiter, only 5 (variant a) requests would count,
    /// and the 11th (variant a) would wrongly return Ok instead of RESOURCE_EXHAUSTED —
    /// this assertion catches it.
    ///
    /// WHAT IT DOES NOT PIN: CMS instance uniqueness. The RateLimitLocal CMS lives in the
    /// global PluginStore (keyed by EdgionPlugins identity), so a per-unit rebuild that
    /// still carried the same ExtensionRef would resolve the SAME CMS and stay green.
    /// See the suite-level comment.
    fn test_shared_limit_across_match_items() -> TestCase {
        TestCase::new(
            "grpc_ratelimit_shared_across_match_items",
            "gRPC rule-level RateLimitLocal is shared across a rule's match-items (global budget)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let endpoint = match Self::make_endpoint(&ctx, HOSTNAME) {
                        Ok(ep) => ep,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Connect failed: {}", e)),
                    };

                    let rate_key = Self::generate_test_key();

                    // RATE requests alternating a/b => 5x a + 5x b, all within the shared budget.
                    for i in 0..RATE {
                        let variant = if i % 2 == 0 { "a" } else { "b" };
                        match Self::say_hello(&mut client, &rate_key, variant).await {
                            Ok(()) => {}
                            Err(code) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Request {} (variant {}) was rejected early with {:?}; \
                                         expected success within the budget of {}",
                                        i + 1,
                                        variant,
                                        code,
                                        RATE
                                    ),
                                );
                            }
                        }
                    }

                    // The (RATE+1)th request must be rejected — the rule's single budget is now spent.
                    // Use variant `a` (it has only consumed 5 so far); under the shared invariant the
                    // rule budget is exhausted, under a per-unit-split regression it would still pass.
                    match Self::say_hello(&mut client, &rate_key, "a").await {
                        Ok(()) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Request {} (variant a) SUCCEEDED but should have been rate-limited. \
                                 The rule-level RateLimitLocal budget is NOT shared across match-items — \
                                 likely a per-unit PluginRuntime split (effective limit inflated to {}x).",
                                RATE + 1,
                                2
                            ),
                        ),
                        Err(tonic::Code::ResourceExhausted) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!(
                                "Shared budget enforced: {} requests across both match-items succeeded, \
                                 the {}th returned RESOURCE_EXHAUSTED",
                                RATE,
                                RATE + 1
                            ),
                        ),
                        Err(code) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Request {} was rejected with {:?}, expected RESOURCE_EXHAUSTED (gRPC 8). \
                                 RateLimitLocal rejectStatus 429 maps to RESOURCE_EXHAUSTED via http_to_grpc_status.",
                                RATE + 1,
                                code
                            ),
                        ),
                    }
                })
            },
        )
    }

    /// Smoke (H dimension, WEAKER than the K case): one rule enforces one global
    /// RateLimitLocal budget across its hostnames.
    ///
    /// Send RATE (=10) requests split across BOTH hostnames (5x a + 5x b) over two
    /// channels with distinct `:authority`, all carrying the same fixed `x-rate-key` —
    /// all must succeed (global budget not yet exhausted). The (RATE+1)th must be
    /// rejected with RESOURCE_EXHAUSTED because the budget is shared.
    ///
    /// SCOPE: unlike the K (match-item) case, this does NOT pin per-unit plugin
    /// execution. gRPC uses a flat match engine with NO per-hostname unit fan-out
    /// (`effective_hostnames` is a Vec on the shared route_info), so both hostnames
    /// resolve to the same single unit — there is no per-hostname build site to guard.
    /// It verifies only the user-visible "one rule = one budget across hostnames"
    /// contract, which here is guaranteed by both the single shared unit and the global
    /// PluginStore. (The HTTP side, which DOES fan units out per hostname, is where a
    /// per-hostname execution pin would have teeth.)
    fn test_shared_limit_across_hostnames() -> TestCase {
        TestCase::new(
            "grpc_ratelimit_shared_across_hostnames",
            "gRPC rule-level RateLimitLocal is shared across a rule's hostnames (global budget)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    // One channel per hostname: tonic pins `:authority` at the channel
                    // (origin) level, so distinct hostnames need distinct channels.
                    let endpoint_a = match Self::make_endpoint(&ctx, HOSTNAME_A) {
                        Ok(ep) => ep,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let mut client_a = match TestServiceClient::connect(endpoint_a).await {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Connect (host a) failed: {}", e))
                        }
                    };
                    let endpoint_b = match Self::make_endpoint(&ctx, HOSTNAME_B) {
                        Ok(ep) => ep,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let mut client_b = match TestServiceClient::connect(endpoint_b).await {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Connect (host b) failed: {}", e))
                        }
                    };

                    let rate_key = Self::generate_test_key();

                    // RATE requests alternating hostname a/b => 5x a + 5x b, all within the shared budget.
                    for i in 0..RATE {
                        let (host_label, res) = if i % 2 == 0 {
                            ("a", Self::say_hello_plain(&mut client_a, &rate_key).await)
                        } else {
                            ("b", Self::say_hello_plain(&mut client_b, &rate_key).await)
                        };
                        if let Err(code) = res {
                            return TestResult::failed(
                                start.elapsed(),
                                format!(
                                    "Request {} (hostname {}) was rejected early with {:?}; \
                                     expected success within the budget of {}",
                                    i + 1,
                                    host_label,
                                    code,
                                    RATE
                                ),
                            );
                        }
                    }

                    // The (RATE+1)th request must be rejected — the rule's single budget is now spent.
                    // Use hostname a (it has only consumed 5 so far); under the shared invariant the
                    // rule budget is exhausted, under a per-hostname-split regression it would still pass.
                    match Self::say_hello_plain(&mut client_a, &rate_key).await {
                        Ok(()) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Request {} (hostname a) SUCCEEDED but should have been rate-limited. \
                                 The rule-level RateLimitLocal budget is NOT shared across hostnames — \
                                 likely a per-hostname PluginRuntime split (effective limit inflated to {}x).",
                                RATE + 1,
                                2
                            ),
                        ),
                        Err(tonic::Code::ResourceExhausted) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!(
                                "Shared budget enforced: {} requests across both hostnames succeeded, \
                                 the {}th returned RESOURCE_EXHAUSTED",
                                RATE,
                                RATE + 1
                            ),
                        ),
                        Err(code) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Request {} was rejected with {:?}, expected RESOURCE_EXHAUSTED (gRPC 8). \
                                 RateLimitLocal rejectStatus 429 maps to RESOURCE_EXHAUSTED via http_to_grpc_status.",
                                RATE + 1,
                                code
                            ),
                        ),
                    }
                })
            },
        )
    }

    /// Sanity: requests below the budget, spread across both match-items, all succeed.
    /// Guards against the RateLimitLocal plugin not running at all on the gRPC rule path.
    fn test_in_budget_requests_succeed() -> TestCase {
        TestCase::new(
            "grpc_ratelimit_in_budget_succeeds",
            "gRPC rule-level RateLimitLocal allows requests within the budget across match-items",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let endpoint = match Self::make_endpoint(&ctx, HOSTNAME) {
                        Ok(ep) => ep,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Connect failed: {}", e)),
                    };

                    let rate_key = Self::generate_test_key();

                    // A few requests on each variant, comfortably below RATE.
                    for (i, variant) in ["a", "b", "a", "b"].iter().enumerate() {
                        match Self::say_hello(&mut client, &rate_key, variant).await {
                            Ok(()) => {}
                            Err(code) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "In-budget request {} (variant {}) failed with {:?}",
                                        i + 1,
                                        variant,
                                        code
                                    ),
                                );
                            }
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "All in-budget requests across both match-items succeeded".to_string(),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for GrpcRateLimitLocalTestSuite {
    fn name(&self) -> &str {
        "gRPC RateLimitLocal (fan-out + e2e)"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_in_budget_requests_succeed(),
            Self::test_shared_limit_across_match_items(),
            Self::test_shared_limit_across_hostnames(),
        ]
    }
}
