// gRPC rule-level RateLimit — shared-CMS regression suite.
//
// Pins the highest-risk invariant of the gRPC types/compiled split
// (tasks/review-decouple/grpc-01-missing-shared-cms-regression-test.md):
//
//   A GRPCRoute rule with >= 2 match-items builds >= 2 GrpcRouteRuleUnits, but
//   the rule-level Arc<PluginRuntime> (and the RateLimit Count-Min-Sketch state
//   inside it) MUST be built ONCE and shared across all of the rule's units.
//   If a regression rebuilds the runtime per-unit, each match-item gets its own
//   CMS and the effective limit becomes N x (N = match-item count).
//
// Scenario (conf/GRPCRoute/RateLimit): one rule, two match-items distinguished
// by `x-variant: a` / `x-variant: b`, both routing SayHello, one rule-level
// RateLimit (rate=10, fixed `x-rate-key` => one shared bucket). The gateway maps
// the plugin's HTTP 429 to gRPC status 8 RESOURCE_EXHAUSTED (http_to_grpc_status).
//
// A sibling case pins the orthogonal HOSTNAME (H) dimension: one rule, two
// hostnames, one match-item, asserting the budget stays shared across hostnames.
// gRPC has no per-hostname unit fan-out (flat engine), so that pin is a weaker
// contract guard than the HTTP analog; see GRPCRoute_rate-limit-shared-hostnames.yaml.
//
// Required config files (conf/GRPCRoute/RateLimit/):
// - Gateway.yaml                              # grpc listener :31063
// - GRPCRoute_rate-limit-shared.yaml          # 1 rule, 2 header match-items, rule-level RateLimit (K dim)
// - GRPCRoute_rate-limit-shared-hostnames.yaml # 1 rule, 2 hostnames, 1 match-item, rule-level RateLimit (H dim)
// - EdgionPlugins_rate-limit-grpc.yaml        # RateLimit rate=10, key=header x-rate-key
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
// 60s RateLimit window of one case from leaking into another (or into a re-run).
static KEY_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct GrpcRateLimitTestSuite;

impl GrpcRateLimitTestSuite {
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

    /// THE pin: a rule with two match-items shares one RateLimit window.
    ///
    /// Send RATE (=10) requests split across BOTH variants (5x a + 5x b) — all must
    /// succeed (global budget not yet exhausted). The (RATE+1)th must be rejected
    /// with RESOURCE_EXHAUSTED because the budget is shared by the rule, not the
    /// match-item.
    ///
    /// Regression behavior this catches: if the runtime were rebuilt per match-item,
    /// variant `a` and `b` would each get an independent budget of 10. After 5a+5b
    /// each bucket holds only 5, so the 11th request (variant a => 6th in its bucket)
    /// would still SUCCEED — this test would then fail on the "expected rejection"
    /// assertion, flagging the N x limit inflation.
    fn test_shared_limit_across_match_items() -> TestCase {
        TestCase::new(
            "grpc_ratelimit_shared_across_match_items",
            "gRPC rule-level RateLimit is shared across a rule's match-items (global budget)",
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
                                 The rule-level RateLimit budget is NOT shared across match-items — \
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
                                 RateLimit rejectStatus 429 maps to RESOURCE_EXHAUSTED via http_to_grpc_status.",
                                RATE + 1,
                                code
                            ),
                        ),
                    }
                })
            },
        )
    }

    /// THE pin (H dimension): a rule with two HOSTNAMES shares one RateLimit window.
    ///
    /// gRPC uses a flat match engine — `effective_hostnames` is a Vec on the shared
    /// route_info and a rule's plugin_runtime is built once per rule regardless of
    /// hostname count (grpc/conf_handler_impl.rs has no per-hostname unit fan-out),
    /// so today both hostnames structurally resolve to the same unit. This case pins
    /// that observable contract (one global budget across hostnames) and future-proofs
    /// it against a refactor that would fan gRPC units out per (rule, hostname) — the
    /// HTTP topology — without re-sharing the runtime.
    ///
    /// Send RATE (=10) requests split across BOTH hostnames (5x a + 5x b) over two
    /// channels with distinct `:authority`, all carrying the same fixed `x-rate-key` —
    /// all must succeed (global budget not yet exhausted). The (RATE+1)th must be
    /// rejected with RESOURCE_EXHAUSTED because the budget is shared by the rule, not
    /// split per hostname.
    ///
    /// Regression behavior this catches: if a future per-hostname runtime split gave
    /// each hostname its own budget of 10, after 5a+5b each bucket holds only 5, so the
    /// 11th request (hostname a => 6th in its bucket) would still SUCCEED — this test
    /// would then fail on the "expected rejection" assertion, flagging the H x inflation.
    fn test_shared_limit_across_hostnames() -> TestCase {
        TestCase::new(
            "grpc_ratelimit_shared_across_hostnames",
            "gRPC rule-level RateLimit is shared across a rule's hostnames (global budget)",
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
                                 The rule-level RateLimit budget is NOT shared across hostnames — \
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
                                 RateLimit rejectStatus 429 maps to RESOURCE_EXHAUSTED via http_to_grpc_status.",
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
    /// Guards against the RateLimit plugin not running at all on the gRPC rule path.
    fn test_in_budget_requests_succeed() -> TestCase {
        TestCase::new(
            "grpc_ratelimit_in_budget_succeeds",
            "gRPC rule-level RateLimit allows requests within the budget across match-items",
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
impl TestSuite for GrpcRateLimitTestSuite {
    fn name(&self) -> &str {
        "gRPC RateLimit (shared CMS)"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_in_budget_requests_succeed(),
            Self::test_shared_limit_across_match_items(),
            Self::test_shared_limit_across_hostnames(),
        ]
    }
}
