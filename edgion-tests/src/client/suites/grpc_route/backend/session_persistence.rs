// GRPCRoute Session Persistence Test Suite
//
// Verifies Gateway API Session Persistence (Cookie and Header modes) on the
// native GRPCRoute path with BACKEND IDENTITY assertions: the gRPC test
// service echoes its listen address (`server_addr`), and every sticky RPC must
// land on the same backend that issued the token — a successful status alone
// is not enough.
//
// Topology: backend identity is only distinguishable by PORT (test_server
// reports `0.0.0.0:{port}`, one listener per port), so the scenario uses three
// EndpointSlices (127.0.0.1:30021/30022/30023) under one Service. The Service
// intentionally declares NO targetPort: resolve_target_port_key would rewrite
// every selected endpoint to a single-port Service's targetPort, collapsing
// all backends onto one port and letting the stickiness assertion pass with
// stickiness completely broken. The control test guards that trap at runtime.
//
// Required config files (conf/GRPCRoute/Backend/SessionPersistence/):
// - Gateway.yaml                            # grpc listener :31937
// - GRPCRoute_sp-cookie.yaml                # Cookie mode, host grpc-sp-cookie.example.com
// - GRPCRoute_sp-header.yaml                # Header mode, host grpc-sp-header.example.com
// - Service_grpc-sp.yaml                    # single port 8080, NO targetPort
// - EndpointSlice_grpc-sp-3002{1,2,3}.yaml  # one slice per backend port

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::collections::HashSet;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::super::test::test_service_client::TestServiceClient;
use super::super::test::HelloRequest;

const COOKIE_HOSTNAME: &str = "grpc-sp-cookie.example.com";
const HEADER_HOSTNAME: &str = "grpc-sp-header.example.com";
const COOKIE_NAME: &str = "edgion-grpc-sp";
const HEADER_NAME: &str = "x-grpc-session-id";
const STICKY_RPCS: usize = 50;
const CONTROL_RPCS: usize = 20;

/// Build a tonic endpoint whose :authority is the scenario hostname.
fn make_endpoint(ctx: &TestContext, hostname: &str) -> Result<tonic::transport::Endpoint, String> {
    let grpc_url = format!("http://{}:{}", ctx.target_host, ctx.grpc_port);
    let endpoint = tonic::transport::Endpoint::from_shared(grpc_url).map_err(|e| format!("Invalid endpoint: {}", e))?;
    let origin_uri: http::Uri = format!("http://{}:{}", hostname, ctx.grpc_port)
        .parse()
        .map_err(|e| format!("Invalid origin URI: {}", e))?;
    Ok(endpoint.origin(origin_uri))
}

async fn connect(ctx: &TestContext, hostname: &str) -> Result<TestServiceClient<tonic::transport::Channel>, String> {
    let endpoint = make_endpoint(ctx, hostname)?;
    TestServiceClient::connect(endpoint)
        .await
        .map_err(|e| format!("Failed to connect: {}", e))
}

/// One SayHello RPC, optionally attaching request metadata `(key, value)`.
/// Returns `(server_addr, response_metadata)`.
async fn say_hello(
    client: &mut TestServiceClient<tonic::transport::Channel>,
    metadata: Option<(&'static str, String)>,
) -> Result<(String, tonic::metadata::MetadataMap), String> {
    let mut request = tonic::Request::new(HelloRequest {
        name: "Edgion".to_string(),
    });
    if let Some((key, value)) = metadata {
        let val = value
            .parse::<tonic::metadata::AsciiMetadataValue>()
            .map_err(|e| format!("invalid metadata value: {}", e))?;
        request.metadata_mut().insert(key, val);
    }
    let response = client
        .say_hello(request)
        .await
        .map_err(|e| format!("RPC failed: {:?}: {}", e.code(), e.message()))?;
    let meta = response.metadata().clone();
    let reply = response.into_inner();
    Ok((reply.server_addr, meta))
}

pub struct GrpcSessionPersistenceTestSuite;

/// Send separate Cookie metadata fields over HTTP/2, preserving insertion order.
async fn cookie_wire_rpc(
    client: &mut TestServiceClient<tonic::transport::Channel>,
    cookies: &[&str],
) -> Result<(String, Option<String>), String> {
    let mut request = tonic::Request::new(HelloRequest { name: "Edgion".into() });
    for cookie in cookies {
        request.metadata_mut().append(
            "cookie",
            cookie.parse().map_err(|e| format!("building Cookie metadata: {e}"))?,
        );
    }
    let response = tokio::time::timeout(Duration::from_secs(5), client.say_hello(request))
        .await
        .map_err(|_| "cookie RPC timed out".to_string())?
        .map_err(|e| format!("cookie RPC failed: {:?}: {}", e.code(), e.message()))?;
    let mut tokens = response
        .metadata()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok()?.split(';').next()?.strip_prefix("edgion-grpc-sp="));
    let token = tokens.next().map(str::to_owned);
    if tokens.next().is_some() {
        return Err("multiple affinity Set-Cookie fields in one response".into());
    }
    let addr = response.into_inner().server_addr;
    if addr.is_empty() {
        return Err("cookie RPC returned an empty backend identity".into());
    }
    Ok((addr, token))
}

fn live_cookie_hash(token: &str) -> Result<&str, String> {
    let (hash, expiry) = token.split_once('.').ok_or("affinity token has no expiry")?;
    let expiry = expiry.parse::<u64>().map_err(|_| "affinity expiry is not a u64")?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("reading current epoch: {e}"))?
        .as_secs();
    if hash.len() != 16 || !hash.bytes().all(|c| c.is_ascii_hexdigit()) || expiry <= now {
        return Err("affinity token must contain a 16-hex hash and a future expiry".into());
    }
    Ok(hash)
}

async fn cookie_wire_scenarios(ctx: TestContext) -> Result<String, String> {
    let mut client = tokio::time::timeout(Duration::from_secs(5), connect(&ctx, COOKIE_HOSTNAME))
        .await
        .map_err(|_| "connecting cookie client timed out".to_string())??;
    let mut seen = HashSet::new();
    let mut issued = None;
    for _ in 0..CONTROL_RPCS {
        let (addr, token) = cookie_wire_rpc(&mut client, &[]).await?;
        let token = token.ok_or("control RPC did not issue an affinity cookie")?;
        live_cookie_hash(&token)?;
        seen.insert(addr.clone());
        issued = Some((addr, token));
    }
    if seen.len() < 2 {
        return Err(format!("control RPCs reached fewer than two backends: {seen:?}"));
    }
    let (issuing_addr, token) = issued.ok_or("no control response")?;
    let cookie = format!("{COOKIE_NAME}={token}");
    for _ in 0..10 {
        let (addr, refreshed) = cookie_wire_rpc(&mut client, &["unrelated=first", &cookie]).await?;
        if addr != issuing_addr || refreshed.is_some() {
            return Err(format!(
                "second Cookie field: backend={addr}, expected={issuing_addr}, refreshed={}",
                refreshed.is_some()
            ));
        }
    }
    let hash = live_cookie_hash(&token)?;
    for suffix in ["invalid", "", "18446744073709551616", "1.2"] {
        // Keep a known live hash so silently stripping the malformed suffix cannot pass.
        let malformed = format!("{COOKIE_NAME}={hash}.{suffix}");
        let (addr, refreshed) = cookie_wire_rpc(&mut client, &[&malformed]).await?;
        let refreshed = refreshed.ok_or_else(|| format!("malformed expiry {suffix:?} did not refresh"))?;
        live_cookie_hash(&refreshed)?;
        let refreshed_cookie = format!("{COOKIE_NAME}={refreshed}");
        // A refresh can legitimately issue identical bytes within the same second.
        for _ in 0..5 {
            let (replay_addr, reissued) = cookie_wire_rpc(&mut client, &[&refreshed_cookie]).await?;
            if replay_addr != addr || reissued.is_some() {
                return Err(format!(
                    "refreshed cookie for suffix {suffix:?} did not remain pinned without reissue"
                ));
            }
        }
    }
    Ok("Second Cookie field pins its issuing backend without refresh; four malformed expiry forms refresh and remain pinned".into())
}

impl GrpcSessionPersistenceTestSuite {
    fn test_cookie_fields_and_malformed_expiry() -> TestCase {
        TestCase::new(
            "grpc_sp_cookie_fields_and_malformed_expiry",
            "Repeated Cookie fields and malformed expiry on gRPC HTTP/2",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match cookie_wire_scenarios(ctx).await {
                        Ok(message) => TestResult::passed_with_message(start.elapsed(), message),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    /// Control: without a token, RPCs must spread across >=2 backends. Guards
    /// the vacuous-pass trap (targetPort rewrite or a single live backend
    /// would make the sticky assertions below meaningless).
    fn test_control_topology() -> TestCase {
        TestCase::new(
            "grpc_sp_control_topology",
            "Token-less RPCs reach >=2 distinct backends (vacuous-pass guard)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let mut client = match connect(&ctx, COOKIE_HOSTNAME).await {
                        Ok(c) => c,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let mut seen: HashSet<String> = HashSet::new();
                    for i in 0..CONTROL_RPCS {
                        match say_hello(&mut client, None).await {
                            Ok((addr, _)) => {
                                seen.insert(addr);
                            }
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("control RPC {} failed: {}", i, e));
                            }
                        }
                    }
                    if seen.len() >= 2 {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!("observed {} distinct backends: {:?}", seen.len(), seen),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "expected >=2 distinct backends without a token, got {:?} — the \
                                 topology cannot distinguish backends (check Service targetPort \
                                 and per-port EndpointSlices)",
                                seen
                            ),
                        )
                    }
                })
            },
        )
    }

    /// Cookie mode: first RPC returns Set-Cookie in initial metadata; >=50
    /// subsequent RPCs carrying that cookie all reach the token-issuing backend.
    fn test_cookie_persistence() -> TestCase {
        TestCase::new(
            "grpc_sp_cookie_persistence",
            "Cookie session persistence pins >=50 RPCs to the same backend",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let mut client = match connect(&ctx, COOKIE_HOSTNAME).await {
                        Ok(c) => c,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };

                    let (first_addr, meta) = match say_hello(&mut client, None).await {
                        Ok(r) => r,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("initial RPC failed: {}", e));
                        }
                    };

                    let set_cookie = match meta.get("set-cookie").and_then(|v| v.to_str().ok()) {
                        Some(sc) if sc.starts_with(&format!("{}=", COOKIE_NAME)) => sc.to_string(),
                        Some(sc) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Set-Cookie does not start with '{}=': {}", COOKIE_NAME, sc),
                            );
                        }
                        None => {
                            return TestResult::failed(
                                start.elapsed(),
                                "no set-cookie in initial response metadata".to_string(),
                            );
                        }
                    };

                    // Permanent lifetime + absoluteTimeout => Max-Age must be present.
                    for attr in ["Path=/", "Max-Age=", "HttpOnly", "SameSite=Lax"] {
                        if !set_cookie.contains(attr) {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Set-Cookie missing '{}': {}", attr, set_cookie),
                            );
                        }
                    }

                    let cookie_pair = set_cookie.split(';').next().unwrap_or(&set_cookie).to_string();

                    for i in 0..STICKY_RPCS {
                        match say_hello(&mut client, Some(("cookie", cookie_pair.clone()))).await {
                            Ok((addr, _)) if addr == first_addr => {}
                            Ok((addr, _)) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("sticky RPC {} landed on {} (expected {})", i, addr, first_addr),
                                );
                            }
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("sticky RPC {} failed: {}", i, e));
                            }
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!("{} RPCs pinned to {}", STICKY_RPCS, first_addr),
                    )
                })
            },
        )
    }

    /// Header mode: first RPC returns the session header in initial metadata;
    /// >=50 subsequent RPCs carrying it all reach the token-issuing backend.
    fn test_header_persistence() -> TestCase {
        TestCase::new(
            "grpc_sp_header_persistence",
            "Header session persistence pins >=50 RPCs to the same backend",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let mut client = match connect(&ctx, HEADER_HOSTNAME).await {
                        Ok(c) => c,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };

                    let (first_addr, meta) = match say_hello(&mut client, None).await {
                        Ok(r) => r,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("initial RPC failed: {}", e));
                        }
                    };

                    let token = match meta.get(HEADER_NAME).and_then(|v| v.to_str().ok()) {
                        Some(t) if !t.is_empty() => t.to_string(),
                        _ => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("no '{}' in initial response metadata", HEADER_NAME),
                            );
                        }
                    };

                    for i in 0..STICKY_RPCS {
                        match say_hello(&mut client, Some((HEADER_NAME, token.clone()))).await {
                            Ok((addr, _)) if addr == first_addr => {}
                            Ok((addr, _)) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("sticky RPC {} landed on {} (expected {})", i, addr, first_addr),
                                );
                            }
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("sticky RPC {} failed: {}", i, e));
                            }
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!("{} RPCs pinned to {}", STICKY_RPCS, first_addr),
                    )
                })
            },
        )
    }

    /// Stale token (strict unset): the RPC must succeed via LB fallback and the
    /// response must carry a refreshed token that then actually sticks.
    fn test_stale_token_refresh() -> TestCase {
        TestCase::new(
            "grpc_sp_stale_token_refresh",
            "A stale token falls back, gets refreshed, and the refreshed token sticks",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let mut client = match connect(&ctx, HEADER_HOSTNAME).await {
                        Ok(c) => c,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };

                    // A valid 16-hex-char token shape that matches no real backend.
                    let (served_addr, meta) = match say_hello(&mut client, Some((HEADER_NAME, "f".repeat(16)))).await {
                        Ok(r) => r,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("stale-token RPC failed (expected LB fallback): {}", e),
                            );
                        }
                    };

                    let refreshed = match meta.get(HEADER_NAME).and_then(|v| v.to_str().ok()) {
                        Some(t) if !t.is_empty() && t != "f".repeat(16) => t.to_string(),
                        Some(t) => {
                            return TestResult::failed(start.elapsed(), format!("token not refreshed (still '{}')", t));
                        }
                        None => {
                            return TestResult::failed(
                                start.elapsed(),
                                "stale-token response carried no refreshed token".to_string(),
                            );
                        }
                    };

                    for i in 0..5 {
                        match say_hello(&mut client, Some((HEADER_NAME, refreshed.clone()))).await {
                            Ok((addr, _)) if addr == served_addr => {}
                            Ok((addr, _)) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "refreshed-token RPC {} landed on {} (expected {})",
                                        i, addr, served_addr
                                    ),
                                );
                            }
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("refreshed-token RPC {} failed: {}", i, e),
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
impl TestSuite for GrpcSessionPersistenceTestSuite {
    fn name(&self) -> &str {
        "GRPCRoute Session Persistence"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_control_topology(),
            Self::test_cookie_persistence(),
            Self::test_header_persistence(),
            Self::test_stale_token_refresh(),
            Self::test_cookie_fields_and_malformed_expiry(),
        ]
    }
}
