// GRPCRoute Weighted Session Persistence Test Suite (GEP-1619 traffic-split
// semantics on the native GRPCRoute path).
//
// Topology: two Services with weights 70/30 — grpc-spw-a (127.0.0.1:30021)
// and grpc-spw-b (127.0.0.1:30022), Header mode. Backend identity comes from
// the echoed `server_addr`.
//
// Scenarios:
// 1. Token-less RPCs approximately respect the 70/30 split.
// 2. A session established on the 30%-weight Service stays there for 50 RPCs
//    — under the old ordering (weighted pick first, then Service-scoped token
//    resolve) the session bounces to grpc-spw-a ~70% of the time.
// 3. A stale token falls back via weighted LB, gets refreshed, and the
//    refreshed token sticks.
//
// Required config files (conf/GRPCRoute/Backend/SessionPersistenceWeighted/):
// - Gateway.yaml                    # grpc listener :31938
// - GRPCRoute_spw.yaml              # Header mode, host grpc-spw.example.com, weights 70/30
// - Service_grpc-spw-{a,b}.yaml     # single port 8080, NO targetPort
// - EndpointSlice_grpc-spw-{a,b}.yaml  # 127.0.0.1:30021 / 127.0.0.1:30022

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

use super::super::test::test_service_client::TestServiceClient;
use super::super::test::HelloRequest;

const HOSTNAME: &str = "grpc-spw.example.com";
const HEADER_NAME: &str = "x-grpc-spw-session";
const BACKEND_A_SUFFIX: &str = ":30021";
const BACKEND_B_SUFFIX: &str = ":30022";
/// Split-tolerance bounds for 100 draws at p=0.7 (±~3σ).
const SPLIT_TOTAL: usize = 100;
const SPLIT_A_MIN: usize = 55;
const SPLIT_A_MAX: usize = 85;
const STICKY_RPCS: usize = 50;

/// Build a tonic endpoint whose :authority is the scenario hostname.
fn make_endpoint(ctx: &TestContext, hostname: &str) -> Result<tonic::transport::Endpoint, String> {
    let grpc_url = format!("http://{}:{}", ctx.target_host, ctx.grpc_port);
    let endpoint = tonic::transport::Endpoint::from_shared(grpc_url).map_err(|e| format!("Invalid endpoint: {}", e))?;
    let origin_uri: http::Uri = format!("http://{}:{}", hostname, ctx.grpc_port)
        .parse()
        .map_err(|e| format!("Invalid origin URI: {}", e))?;
    Ok(endpoint.origin(origin_uri))
}

async fn connect(ctx: &TestContext) -> Result<TestServiceClient<tonic::transport::Channel>, String> {
    let endpoint = make_endpoint(ctx, HOSTNAME)?;
    TestServiceClient::connect(endpoint)
        .await
        .map_err(|e| format!("Failed to connect: {}", e))
}

/// One SayHello RPC, optionally attaching the session header.
/// Returns `(server_addr, refreshed_token_if_any)`.
async fn say_hello(
    client: &mut TestServiceClient<tonic::transport::Channel>,
    token: Option<&str>,
) -> Result<(String, Option<String>), String> {
    let mut request = tonic::Request::new(HelloRequest {
        name: "Edgion".to_string(),
    });
    if let Some(value) = token {
        let val = value
            .parse::<tonic::metadata::AsciiMetadataValue>()
            .map_err(|e| format!("invalid metadata value: {}", e))?;
        request.metadata_mut().insert(HEADER_NAME, val);
    }
    let response = client
        .say_hello(request)
        .await
        .map_err(|e| format!("RPC failed: {:?}: {}", e.code(), e.message()))?;
    let issued = response
        .metadata()
        .get(HEADER_NAME)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    Ok((response.into_inner().server_addr, issued))
}

pub struct GrpcSpWeightedTestSuite;

impl GrpcSpWeightedTestSuite {
    /// Token-less RPCs approximately respect the 70/30 split.
    fn test_weighted_split_without_token() -> TestCase {
        TestCase::new(
            "grpc_spw_split_without_token",
            "Token-less RPCs respect the 70/30 split within tolerance",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let mut client = match connect(&ctx).await {
                        Ok(c) => c,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let (mut count_a, mut count_b) = (0usize, 0usize);
                    for i in 0..SPLIT_TOTAL {
                        match say_hello(&mut client, None).await {
                            Ok((addr, issued)) => {
                                if issued.is_none() {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("RPC {} (no token) got no session header", i),
                                    );
                                }
                                if addr.ends_with(BACKEND_A_SUFFIX) {
                                    count_a += 1;
                                } else if addr.ends_with(BACKEND_B_SUFFIX) {
                                    count_b += 1;
                                } else {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("RPC {} served by unexpected backend {}", i, addr),
                                    );
                                }
                            }
                            Err(e) => return TestResult::failed(start.elapsed(), format!("RPC {}: {}", i, e)),
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
                                "split out of tolerance: a={} b={} (expected a in [{}, {}])",
                                count_a, count_b, SPLIT_A_MIN, SPLIT_A_MAX
                            ),
                        );
                    }
                    TestResult::passed_with_message(start.elapsed(), format!("split OK: a={} b={}", count_a, count_b))
                })
            },
        )
    }

    /// A session established on the 30%-weight Service stays there for 50 RPCs
    /// and is never refreshed.
    fn test_sticky_across_weighted_backends() -> TestCase {
        TestCase::new(
            "grpc_spw_sticky_on_minority_backend",
            "A session bound to the weight-30 Service stays there for 50 RPCs",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let mut client = match connect(&ctx).await {
                        Ok(c) => c,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };

                    // Establish a session on grpc-spw-b (~30% per try).
                    let mut token_b: Option<String> = None;
                    for _ in 0..60 {
                        match say_hello(&mut client, None).await {
                            Ok((addr, Some(token))) if addr.ends_with(BACKEND_B_SUFFIX) => {
                                token_b = Some(token);
                                break;
                            }
                            Ok(_) => continue,
                            Err(e) => return TestResult::failed(start.elapsed(), e),
                        }
                    }
                    let Some(token_b) = token_b else {
                        return TestResult::failed(
                            start.elapsed(),
                            "could not establish a session on grpc-spw-b in 60 tries".to_string(),
                        );
                    };

                    for i in 0..STICKY_RPCS {
                        match say_hello(&mut client, Some(&token_b)).await {
                            Ok((addr, None)) if addr.ends_with(BACKEND_B_SUFFIX) => {}
                            Ok((addr, refreshed)) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "sticky RPC {} broke: addr={} refreshed={:?} (expected :30022, no refresh)",
                                        i, addr, refreshed
                                    ),
                                );
                            }
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("sticky RPC {}: {}", i, e));
                            }
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!("{} RPCs pinned to grpc-spw-b", STICKY_RPCS),
                    )
                })
            },
        )
    }

    /// Stale token: the RPC must succeed via weighted fallback and carry a
    /// refreshed token that then sticks.
    fn test_stale_token_refresh() -> TestCase {
        TestCase::new(
            "grpc_spw_stale_token_refresh",
            "A stale token falls back via weighted LB, gets refreshed, and sticks",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let mut client = match connect(&ctx).await {
                        Ok(c) => c,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };

                    // A valid 16-hex-char token shape that matches no real backend.
                    let stale = "f".repeat(16);
                    let (served_addr, refreshed) = match say_hello(&mut client, Some(&stale)).await {
                        Ok(r) => r,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("stale-token RPC failed (expected LB fallback): {}", e),
                            );
                        }
                    };
                    let refreshed = match refreshed {
                        Some(t) if !t.is_empty() && t != stale => t,
                        other => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("token not refreshed after stale-token fallback: {:?}", other),
                            );
                        }
                    };

                    for i in 0..5 {
                        match say_hello(&mut client, Some(&refreshed)).await {
                            Ok((addr, None)) if addr == served_addr => {}
                            Ok((addr, r)) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "refreshed-token RPC {} landed on {} refreshed={:?} (expected {})",
                                        i, addr, r, served_addr
                                    ),
                                );
                            }
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("refreshed-token RPC {}: {}", i, e),
                                );
                            }
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!("refreshed token pinned to {}", served_addr),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for GrpcSpWeightedTestSuite {
    fn name(&self) -> &str {
        "GRPCRoute Weighted Session Persistence"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_weighted_split_without_token(),
            Self::test_sticky_across_weighted_backends(),
            Self::test_stale_token_refresh(),
        ]
    }
}
