// GRPCRoute Delegation test suite (cascading match)
//
// Required config files (in edgion-tests/integration/conf/GRPCRoute/Delegation/):
// - Gateway.yaml                                     # grpc-delegation-gateway (port 31934)
// - GRPCRoute_edgion-test_delegation-parent.yaml     # root: plain StreamNumbers rule + wildcard
//                                                    # delegation of test.TestService
// - GRPCRoute_edgion-grpc-deleg_child.yaml           # child declaring the parent: SayHello +
//                                                    # test.OtherService (never matches)
// - GRPCRoute_edgion-test_grpc-multi-parent-{a,b}.yaml + GRPCRoute_edgion-grpc-deleg_multi-child.yaml
// - GRPCRoute_edgion-test_grpc-backtrack-parent.yaml + GRPCRoute_edgion-grpc-deleg_backtrack-child.yaml
// - GRPCRoute_edgion-test_grpc-matchless-parent.yaml + GRPCRoute_edgion-grpc-deleg_matchless-child.yaml
// - Service/EndpointSlice for test-grpc (edgion-test) and child-grpc-svc (edgion-grpc-deleg),
//   both pointing at the same test_server gRPC backend (port 30021)
//
// Verifies the Gateway-side GRPCRoute cascading match end-to-end (design §9):
// - the delegated method (SayHello) routes through the child's backend and carries the parent's
//   mandatory RequestHeaderModifier (test_server's say_hello echoes `x-*` request metadata)
// - a child rule matching a DIFFERENT service never matches through the parent; the call gets
//   UNIMPLEMENTED with Gateway RouteNotFound evidence
// - the parent's plain sibling rule (StreamNumbers, non-delegating) keeps working
// - status: child-side scope is never a parent error (no PartiallyInvalid on the parent); the
//   child gets Accepted=True / DelegationAccepted for its kind:GRPCRoute parentRef
// - one child shared by two parents, a child-subtree miss backtracking to the parent's own
//   catch-all rule, and a matchless child acting as a catch-all inside the delegated service
// - access-log `match_info` is an array ordered root -> leaf

use crate::access_log_client::{trace_id_for_marker, traceparent_for_marker, AccessLogEntry};
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::status::helpers as status_helpers;
use async_trait::async_trait;
use std::time::{Duration, Instant};

use super::test::test_service_client::TestServiceClient;
use super::test::{HelloRequest, HelloResponse, NumberRequest};

const HOST: &str = "grpc-delegation.example.com";
const PARENT_NS: &str = "edgion-test";
const PARENT_NAME: &str = "delegation-parent";
const CHILD_NS: &str = "edgion-grpc-deleg";
const CHILD_NAME: &str = "child";
const MULTI_HOST_A: &str = "grpc-multi-a.example.com";
const MULTI_HOST_B: &str = "grpc-multi-b.example.com";
const BACKTRACK_HOST: &str = "grpc-backtrack.example.com";
const MATCHLESS_HOST: &str = "grpc-matchless.example.com";

/// Build a tonic Endpoint targeting the delegation listener with an explicit
/// `:authority` (ctx.grpc_host is fixed to "grpc.example.com" for every
/// GRPCRoute suite in test_client.rs, so a suite that needs a different
/// hostname builds its own endpoint rather than relying on ctx.grpc_host —
/// mirrors GrpcMatchTestSuite::make_endpoint).
fn make_endpoint(ctx: &TestContext, hostname: &str, port: u16) -> Result<tonic::transport::Endpoint, String> {
    let grpc_url = format!("http://{}:{}", ctx.target_host, port);
    let endpoint = tonic::transport::Endpoint::from_shared(grpc_url).map_err(|e| format!("Invalid endpoint: {e}"))?;
    let origin_uri: http::Uri = format!("http://{}:{}", hostname, port)
        .parse()
        .map_err(|e| format!("Invalid origin URI: {e}"))?;
    Ok(endpoint.origin(origin_uri))
}

fn verify_delegation_rejection(code: tonic::Code, entry: &AccessLogEntry, trace_id: &str) -> Result<(), String> {
    if code != tonic::Code::Unimplemented {
        return Err("Expected UNIMPLEMENTED for the out-of-scope service".to_string());
    }
    if entry.get_nested_str("request_info.trace_id") != Some(trace_id)
        || !entry.has_diagnostic("error", "RouteNotFound")
    {
        return Err("Missing trace-correlated Gateway RouteNotFound evidence".to_string());
    }
    Ok(())
}

const STATUS_TIMEOUT: Duration = Duration::from_secs(10);

/// Attach the access-log test-store marker metadata to a request.
fn traced<T>(message: T, marker: &str) -> Result<tonic::Request<T>, String> {
    let mut request = tonic::Request::new(message);
    request.metadata_mut().insert(
        "traceparent",
        traceparent_for_marker(marker)
            .parse()
            .map_err(|_| "Invalid probe traceparent".to_string())?,
    );
    request
        .metadata_mut()
        .insert("access_log", "test_store".parse().expect("static metadata"));
    Ok(request)
}

async fn fetch_log(ctx: &TestContext, marker: &str) -> Result<AccessLogEntry, String> {
    let access_logs = ctx.access_log_client();
    let entry = access_logs
        .get_access_log_with_retry(marker, 30, 100)
        .await
        .map_err(|e| format!("access log not found: {e}"))?;
    let _ = access_logs.delete_access_log(marker).await;
    Ok(entry)
}

/// Traced SayHello through `host`; returns the reply message and access log.
async fn traced_say_hello(ctx: &TestContext, host: &str) -> Result<(String, AccessLogEntry), String> {
    let marker = format!("grpc-delegation-{}", uuid::Uuid::new_v4());
    let endpoint = make_endpoint(ctx, host, ctx.grpc_port)?;
    let mut client = TestServiceClient::connect(endpoint)
        .await
        .map_err(|e| format!("Failed to connect: {e}"))?;
    let request = traced(
        HelloRequest {
            name: "GrpcDelegation".to_string(),
        },
        &marker,
    )?;
    let reply = client
        .say_hello(request)
        .await
        .map_err(|e| format!("SayHello via {host} failed: {:?}: {}", e.code(), e.message()))?
        .into_inner();
    Ok((reply.message, fetch_log(ctx, &marker).await?))
}

/// Traced StreamNumbers(count=2) through `host`; returns the numbers and access log.
async fn traced_stream_numbers(ctx: &TestContext, host: &str) -> Result<(Vec<i32>, AccessLogEntry), String> {
    let marker = format!("grpc-delegation-{}", uuid::Uuid::new_v4());
    let endpoint = make_endpoint(ctx, host, ctx.grpc_port)?;
    let mut client = TestServiceClient::connect(endpoint)
        .await
        .map_err(|e| format!("Failed to connect: {e}"))?;
    let request = traced(NumberRequest { count: 2 }, &marker)?;
    let mut stream = client
        .stream_numbers(request)
        .await
        .map_err(|e| format!("StreamNumbers via {host} failed: {:?}: {}", e.code(), e.message()))?
        .into_inner();
    let mut got = Vec::new();
    loop {
        match stream.message().await {
            Ok(Some(msg)) => got.push(msg.number),
            Ok(None) => break,
            Err(e) => {
                return Err(format!(
                    "stream errored before clean close: {:?}: {}",
                    e.code(),
                    e.message()
                ))
            }
        }
    }
    Ok((got, fetch_log(ctx, &marker).await?))
}

/// `(ns, name, rule_id)` per `match_info` element, root -> leaf; every element
/// must be a GRPCRoute.
fn expect_chain(entry: &AccessLogEntry, expected: &[(&str, &str, u64)]) -> Result<(), String> {
    let actual: Vec<(String, String, String, u64)> = entry
        .route_chain()
        .iter()
        .map(|level| {
            let rsc = &level["rsc"];
            (
                rsc["kind"].as_str().unwrap_or_default().to_string(),
                rsc["ns"].as_str().unwrap_or_default().to_string(),
                rsc["name"].as_str().unwrap_or_default().to_string(),
                level["rule_id"].as_u64().unwrap_or(u64::MAX),
            )
        })
        .collect();
    let wanted: Vec<(String, String, String, u64)> = expected
        .iter()
        .map(|(ns, name, rule)| ("GRPCRoute".to_string(), ns.to_string(), name.to_string(), *rule))
        .collect();
    if actual == wanted {
        Ok(())
    } else {
        Err(format!(
            "match_info chain mismatch: expected {wanted:?}, got {actual:?}"
        ))
    }
}

/// Call `/test.OtherService/Whatever` through `host` and require UNIMPLEMENTED
/// plus trace-correlated Gateway RouteNotFound evidence (the backend itself
/// would also answer UNIMPLEMENTED, so the code alone proves nothing).
async fn other_service_probe(ctx: &TestContext, host: &str) -> Result<(), String> {
    let marker = format!("grpc-delegation-denial-{}", uuid::Uuid::new_v4());
    let trace_id = trace_id_for_marker(&marker);
    let access_logs = ctx.access_log_client();
    // Reserve time for terminal observation and cleanup within the case deadline.
    let operation = tokio::time::timeout(Duration::from_secs(15), async {
        let endpoint = make_endpoint(ctx, host, ctx.grpc_port)?;
        let channel = endpoint
            .connect()
            .await
            .map_err(|_| "Failed to connect delegation probe".to_string())?;
        let mut grpc = tonic::client::Grpc::new(channel);
        grpc.ready()
            .await
            .map_err(|_| "Delegation probe service was not ready".to_string())?;
        let codec = tonic_prost::ProstCodec::<HelloRequest, HelloResponse>::default();
        let path = http::uri::PathAndQuery::from_static("/test.OtherService/Whatever");
        let req = traced(
            HelloRequest {
                name: "probe".to_string(),
            },
            &marker,
        )?;
        Ok::<_, String>(match grpc.unary(req, path, codec).await {
            Ok(_) => tonic::Code::Ok,
            Err(status) => status.code(),
        })
    })
    .await;
    let outcome = match operation {
        Ok(Ok(code)) => match tokio::time::timeout(
            Duration::from_secs(5),
            access_logs.get_access_log_with_retry(&marker, 20, 100),
        )
        .await
        {
            Ok(Ok(entry)) => verify_delegation_rejection(code, &entry, &trace_id),
            _ => Err("Failed to observe delegation probe access log".to_string()),
        },
        Ok(Err(error)) => Err(error),
        Err(_) => Err("Delegation probe timed out".to_string()),
    };
    // Only delete this probe's entry; a late terminal entry after a timeout
    // or outer cancellation is outside this bounded cleanup attempt.
    let cleanup_ok = matches!(
        tokio::time::timeout(Duration::from_secs(5), access_logs.delete_access_log(&marker)).await,
        Ok(Ok(true))
    );
    match (outcome, cleanup_ok) {
        (Ok(()), true) => Ok(()),
        (Err(error), true) => Err(error),
        (Ok(()), false) => Err("Failed to clean up delegation probe access log".to_string()),
        (Err(error), false) => Err(format!("{error}; failed to clean up delegation probe access log")),
    }
}

fn done(start: Instant, result: Result<String, String>) -> TestResult {
    match result {
        Ok(message) => TestResult::passed_with_message(start.elapsed(), message),
        Err(error) => TestResult::failed(start.elapsed(), error),
    }
}

pub struct GrpcDelegationTestSuite;

impl GrpcDelegationTestSuite {
    /// Delegated SayHello reaches the child's backend and carries the parent's mandatory header.
    fn test_delegated_method_with_inherited_filter() -> TestCase {
        TestCase::new(
            "grpc_delegated_method_and_inherited_filter",
            "Delegated test.TestService/SayHello reaches the child backend and carries the parent's mandatory header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let endpoint = match make_endpoint(&ctx, HOST, ctx.grpc_port) {
                        Ok(ep) => ep,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Failed to connect: {e}")),
                    };
                    let request = tonic::Request::new(HelloRequest {
                        name: "GrpcDelegation".to_string(),
                    });
                    match client.say_hello(request).await {
                        Ok(response) => {
                            let reply = response.into_inner();
                            if !reply.message.contains("Hello, GrpcDelegation!") {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("unexpected response: {}", reply.message),
                                );
                            }
                            if reply.message.contains("x-parent-filter=on") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("delegated call OK with inherited x-parent-filter: {}", reply.message),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "parent filter header missing on delegated traffic; message: {}",
                                        reply.message
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!("RPC failed: {} (status: {:?})", e.message(), e.code()),
                        ),
                    }
                })
            },
        )
    }

    /// The child's rule matching a different service never matches through the
    /// parent, so a call to it gets UNIMPLEMENTED — not routed to the child backend.
    fn test_out_of_scope_child_rule_unimplemented() -> TestCase {
        TestCase::new(
            "grpc_out_of_scope_child_rule_unimplemented",
            "Child rule matching a different service never matches through the parent (UNIMPLEMENTED)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match other_service_probe(&ctx, HOST).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Gateway rejected the out-of-scope service with RouteNotFound and UNIMPLEMENTED"
                                .to_string(),
                        ),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    /// The parent's plain sibling rule (non-delegating) keeps working.
    fn test_plain_sibling_rule_intact() -> TestCase {
        TestCase::new(
            "grpc_plain_sibling_rule_intact",
            "Parent's plain StreamNumbers rule still routes normally",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let endpoint = match make_endpoint(&ctx, HOST, ctx.grpc_port) {
                        Ok(ep) => ep,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(c) => c,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Failed to connect: {e}")),
                    };
                    let request = tonic::Request::new(NumberRequest { count: 2 });
                    let mut stream = match client.stream_numbers(request).await {
                        Ok(resp) => resp.into_inner(),
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("stream_numbers RPC failed: {:?}: {}", e.code(), e.message()),
                            )
                        }
                    };
                    let mut got = Vec::new();
                    loop {
                        match stream.message().await {
                            Ok(Some(msg)) => got.push(msg.number),
                            Ok(None) => break,
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("stream errored before clean close: {:?}: {}", e.code(), e.message()),
                                )
                            }
                        }
                    }
                    if got == vec![1, 2] {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!("plain sibling rule routed StreamNumbers: {:?}", got),
                        )
                    } else {
                        TestResult::failed(start.elapsed(), format!("expected [1, 2], got {:?}", got))
                    }
                })
            },
        )
    }

    /// Parent status: child-side scope is never a parent error (design §7.1).
    /// The parent is Accepted and ResolvedRefs=True, with no PartiallyInvalid.
    fn test_parent_status_clean() -> TestCase {
        TestCase::new(
            "grpc_parent_status_clean",
            "Parent GRPCRoute is Accepted=True / ResolvedRefs=True and carries no child-caused PartiallyInvalid",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = status_helpers::wait_resource_status(
                        &ctx,
                        "GRPCRoute",
                        PARENT_NS,
                        PARENT_NAME,
                        STATUS_TIMEOUT,
                        |route| {
                            status_helpers::has_condition(route, "Accepted", "True")
                                && status_helpers::has_condition(route, "ResolvedRefs", "True")
                                && !status_helpers::has_condition(route, "PartiallyInvalid", "True")
                        },
                    )
                    .await;
                    match result {
                        Ok(_) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Accepted=True, ResolvedRefs=True, no PartiallyInvalid".to_string(),
                        ),
                        Err(error) => TestResult::failed(start.elapsed(), format!("parent status: {error}")),
                    }
                })
            },
        )
    }

    /// Child status: Accepted=True / DelegationAccepted for its kind:GRPCRoute parentRef.
    fn test_child_reverse_lock_accepted() -> TestCase {
        TestCase::new(
            "grpc_child_reverse_lock_accepted",
            "Child GRPCRoute gets Accepted=True / DelegationAccepted for its kind:GRPCRoute parentRef",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let mut last_status: Option<serde_json::Value> = None;
                    for _ in 0..20 {
                        if let Ok(route) =
                            status_helpers::fetch_resource_status(&ctx, "GRPCRoute", CHILD_NS, CHILD_NAME).await
                        {
                            if status_helpers::parent_entry_has(
                                &route,
                                PARENT_NS,
                                PARENT_NAME,
                                "Accepted",
                                "True",
                                "DelegationAccepted",
                            ) {
                                return TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Accepted=True for the reverse-lock parentRef".to_string(),
                                );
                            }
                            last_status = Some(route);
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }
                    TestResult::failed(
                        start.elapsed(),
                        format!(
                            "child status did not reach Accepted=True; last: {}",
                            last_status
                                .map(|s| serde_json::to_string_pretty(&s).unwrap_or_default())
                                .unwrap_or_else(|| "none".to_string())
                        ),
                    )
                })
            },
        )
    }

    /// existing scenario access log: SayHello logs the chain root -> leaf.
    fn test_delegated_match_info_chain() -> TestCase {
        TestCase::new(
            "grpc_delegated_match_info_chain",
            "Delegated SayHello logs a two-element match_info array ordered root -> leaf",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = async {
                        let (_, entry) = traced_say_hello(&ctx, HOST).await?;
                        expect_chain(&entry, &[(PARENT_NS, PARENT_NAME, 1), (CHILD_NS, CHILD_NAME, 0)])?;
                        Ok("match_info [parent r1, child r0]".to_string())
                    }
                    .await;
                    done(start, result)
                })
            },
        )
    }

    /// multi_parent_child (gRPC): one child declared under two parents.
    fn test_multi_parent_child() -> TestCase {
        TestCase::new(
            "grpc_multi_parent_child",
            "One GRPCRoute child is served through two parents, each applying only its own filter",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = async {
                        for (host, value, other, parent) in [
                            (MULTI_HOST_A, "a", "b", "grpc-multi-parent-a"),
                            (MULTI_HOST_B, "b", "a", "grpc-multi-parent-b"),
                        ] {
                            let (message, entry) = traced_say_hello(&ctx, host).await?;
                            if !message.contains(&format!("x-parent-filter={value}"))
                                || message.contains(&format!("x-parent-filter={other}"))
                            {
                                return Err(format!("{host}: expected only x-parent-filter={value}; got {message}"));
                            }
                            expect_chain(&entry, &[(PARENT_NS, parent, 0), (CHILD_NS, "multi-child", 0)])?;
                        }
                        status_helpers::wait_resource_status(
                            &ctx,
                            "GRPCRoute",
                            CHILD_NS,
                            "multi-child",
                            STATUS_TIMEOUT,
                            |route| {
                                ["grpc-multi-parent-a", "grpc-multi-parent-b"].iter().all(|parent| {
                                    status_helpers::parent_entry_has(
                                        route,
                                        PARENT_NS,
                                        parent,
                                        "Accepted",
                                        "True",
                                        "DelegationAccepted",
                                    )
                                })
                            },
                        )
                        .await
                        .map_err(|e| format!("multi-child status: {e}"))?;
                        Ok("both parents serve the child; two Accepted status entries".to_string())
                    }
                    .await;
                    done(start, result)
                })
            },
        )
    }

    /// backtracking (gRPC): StreamNumbers misses the child subtree and falls back
    /// to the parent's own matchless rule; SayHello is served by the child.
    fn test_backtrack_to_parent_rule() -> TestCase {
        TestCase::new(
            "grpc_backtrack_to_parent_rule",
            "Child-subtree miss backtracks to the parent's own catch-all rule",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = async {
                        let (numbers, entry) = traced_stream_numbers(&ctx, BACKTRACK_HOST).await?;
                        if numbers != vec![1, 2] {
                            return Err(format!("StreamNumbers: expected [1, 2], got {numbers:?}"));
                        }
                        expect_chain(&entry, &[(PARENT_NS, "grpc-backtrack-parent", 1)])?;
                        let (_, entry) = traced_say_hello(&ctx, BACKTRACK_HOST).await?;
                        expect_chain(
                            &entry,
                            &[
                                (PARENT_NS, "grpc-backtrack-parent", 0),
                                (CHILD_NS, "backtrack-child", 0),
                            ],
                        )?;
                        Ok("StreamNumbers -> parent rule 1; SayHello -> child".to_string())
                    }
                    .await;
                    done(start, result)
                })
            },
        )
    }

    /// matchless child (gRPC): a rule with no matches is a catch-all inside the
    /// delegated service; any other service is rejected at the root level.
    fn test_matchless_child() -> TestCase {
        TestCase::new(
            "grpc_matchless_child",
            "A matchless child serves every method of the delegated service and nothing else",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = async {
                        let (message, entry) = traced_say_hello(&ctx, MATCHLESS_HOST).await?;
                        if !message.contains("x-parent-filter=matchless") {
                            return Err(format!("SayHello lacks the parent's filter; got {message}"));
                        }
                        let chain = [
                            (PARENT_NS, "grpc-matchless-parent", 0),
                            (CHILD_NS, "matchless-child", 0),
                        ];
                        expect_chain(&entry, &chain)?;
                        let (numbers, entry) = traced_stream_numbers(&ctx, MATCHLESS_HOST).await?;
                        if numbers != vec![1, 2] {
                            return Err(format!("StreamNumbers: expected [1, 2], got {numbers:?}"));
                        }
                        expect_chain(&entry, &chain)?;
                        other_service_probe(&ctx, MATCHLESS_HOST).await?;
                        Ok("SayHello and StreamNumbers via the matchless child; OtherService rejected".to_string())
                    }
                    .await;
                    done(start, result)
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for GrpcDelegationTestSuite {
    fn name(&self) -> &str {
        "GRPCRoute Delegation"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_delegated_method_with_inherited_filter(),
            Self::test_out_of_scope_child_rule_unimplemented(),
            Self::test_plain_sibling_rule_intact(),
            Self::test_parent_status_clean(),
            Self::test_child_reverse_lock_accepted(),
            Self::test_delegated_match_info_chain(),
            Self::test_multi_parent_child(),
            Self::test_backtrack_to_parent_rule(),
            Self::test_matchless_child(),
        ]
    }
}

// ============================================================
// Dynamic scenario phase suites (GrpcRouteDelegationDynamic)
// ============================================================

const DYN_HOST: &str = "grpc-delegation-dyn.example.com";
async fn dyn_say_hello_status(ctx: &TestContext) -> Result<tonic::Code, String> {
    let endpoint = make_endpoint(ctx, DYN_HOST, ctx.grpc_port)?;
    let mut client = TestServiceClient::connect(endpoint)
        .await
        .map_err(|e| format!("Failed to connect: {e}"))?;
    let request = tonic::Request::new(HelloRequest {
        name: "dyn".to_string(),
    });
    Ok(match client.say_hello(request).await {
        Ok(_) => tonic::Code::Ok,
        Err(status) => status.code(),
    })
}

async fn dyn_stream_numbers_status(ctx: &TestContext) -> Result<tonic::Code, String> {
    let endpoint = make_endpoint(ctx, DYN_HOST, ctx.grpc_port)?;
    let mut client = TestServiceClient::connect(endpoint)
        .await
        .map_err(|e| format!("Failed to connect: {e}"))?;
    let request = tonic::Request::new(NumberRequest { count: 1 });
    Ok(match client.stream_numbers(request).await {
        Ok(resp) => {
            let mut stream = resp.into_inner();
            match stream.message().await {
                Ok(_) => tonic::Code::Ok,
                Err(status) => status.code(),
            }
        }
        Err(status) => status.code(),
    })
}

pub struct GrpcDelegationDynamicInitialTestSuite;

impl GrpcDelegationDynamicInitialTestSuite {
    fn say_hello_routes_stream_numbers_missing() -> TestCase {
        TestCase::new(
            "grpc_delegation_dyn_initial",
            "[INITIAL] SayHello -> OK, StreamNumbers -> UNIMPLEMENTED",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match (dyn_say_hello_status(&ctx).await, dyn_stream_numbers_status(&ctx).await) {
                        (Ok(tonic::Code::Ok), Ok(tonic::Code::Unimplemented)) => TestResult::passed(start.elapsed()),
                        (a, b) => TestResult::failed(
                            start.elapsed(),
                            format!("expected (Ok, Unimplemented), got ({a:?}, {b:?})"),
                        ),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for GrpcDelegationDynamicInitialTestSuite {
    fn name(&self) -> &str {
        "GRPCRoute Delegation Dynamic - Initial Phase"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::say_hello_routes_stream_numbers_missing()]
    }
}

pub struct GrpcDelegationDynamicUpdateTestSuite;

impl GrpcDelegationDynamicUpdateTestSuite {
    fn stream_numbers_routes_say_hello_missing() -> TestCase {
        TestCase::new(
            "grpc_delegation_dyn_update",
            "[UPDATE] child rule moved: StreamNumbers -> OK, SayHello -> UNIMPLEMENTED",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match (dyn_say_hello_status(&ctx).await, dyn_stream_numbers_status(&ctx).await) {
                        (Ok(tonic::Code::Unimplemented), Ok(tonic::Code::Ok)) => TestResult::passed(start.elapsed()),
                        (a, b) => TestResult::failed(
                            start.elapsed(),
                            format!("expected (Unimplemented, Ok), got ({a:?}, {b:?})"),
                        ),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for GrpcDelegationDynamicUpdateTestSuite {
    fn name(&self) -> &str {
        "GRPCRoute Delegation Dynamic - Update Phase"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::stream_numbers_routes_say_hello_missing()]
    }
}

#[cfg(test)]
mod rejection_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn delegation_rejection_requires_correlated_gateway_evidence() {
        let trace = trace_id_for_marker("delegation-verdict");
        let mut entry = AccessLogEntry {
            data: json!({"request_info": {"trace_id": trace}, "diagnostics": []}),
        };
        assert!(verify_delegation_rejection(tonic::Code::Unimplemented, &entry, &trace).is_err());
        entry.data["diagnostics"] = json!([{"kind": "error", "code": "RouteNotFound"}]);
        assert!(verify_delegation_rejection(tonic::Code::Unimplemented, &entry, &trace).is_ok());
        assert!(verify_delegation_rejection(tonic::Code::Unavailable, &entry, &trace).is_err());
        assert!(verify_delegation_rejection(tonic::Code::Unimplemented, &entry, "another-trace").is_err());
        entry.data["diagnostics"] = json!([{"kind": "info", "code": "RouteNotFound"}]);
        assert!(verify_delegation_rejection(tonic::Code::Unimplemented, &entry, &trace).is_err());
    }
}
