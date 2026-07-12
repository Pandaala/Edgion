// GRPCRoute Delegation test suite
//
// Required config files (in edgion-tests/integration/conf/GRPCRoute/Delegation/):
// - Gateway.yaml                                     # grpc-delegation-gateway (port 31934)
// - GRPCRoute_edgion-test_delegation-parent.yaml     # root route: plain StreamNumbers rule + wildcard
//                                                     # delegation of test.TestService
// - GRPCRoute_edgion-grpc-deleg_child.yaml           # child: narrows to SayHello (valid) +
//                                                     # test.OtherService (out of scope, dropped);
//                                                     # reverse-locks to delegation-parent
// - Service/EndpointSlice for test-grpc (edgion-test) and child-grpc-svc (edgion-grpc-deleg),
//   both pointing at the same test_server gRPC backend (port 30021)
//
// Verifies the controller-side GRPCRoute delegation flatten end-to-end:
// - the delegated method (SayHello) routes through the child's backend and inherits the parent's
//   mandatory RequestHeaderModifier filter (proven via test_server's say_hello echoing `x-*`
//   request metadata into the response message — gRPC has no generic header-echo endpoint the way
//   the HTTP catch-all handler does)
// - a child rule matching a DIFFERENT service (out of the delegated scope) is dropped by the
//   flatten step -> the call gets UNIMPLEMENTED (12), matching unmatched_route.rs's assertion style
// - the parent's plain sibling rule (StreamNumbers, non-delegating) keeps working
// - status: parent gets PartiallyInvalid=True naming the dropped out-of-scope child rule while
//   ResolvedRefs stays True; child gets Accepted=True for its kind:GRPCRoute reverse-lock parentRef

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::status::helpers as status_helpers;
use async_trait::async_trait;
use std::time::Instant;

use super::test::test_service_client::TestServiceClient;
use super::test::{HelloRequest, HelloResponse, NumberRequest};

const HOST: &str = "grpc-delegation.example.com";
const PARENT_NS: &str = "edgion-test";
const PARENT_NAME: &str = "delegation-parent";
const CHILD_NS: &str = "edgion-grpc-deleg";
const CHILD_NAME: &str = "child";

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

    /// The child's out-of-scope rule (matching a different service) must be dropped by the
    /// flatten step, so a call to it gets UNIMPLEMENTED — not routed to the child backend.
    fn test_out_of_scope_child_rule_unimplemented() -> TestCase {
        TestCase::new(
            "grpc_out_of_scope_child_rule_unimplemented",
            "Child rule matching a different service is dropped by delegation flatten (UNIMPLEMENTED)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let endpoint = match make_endpoint(&ctx, HOST, ctx.grpc_port) {
                        Ok(ep) => ep,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let channel = match endpoint.connect().await {
                        Ok(c) => c,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Failed to connect: {e}")),
                    };
                    let mut grpc = tonic::client::Grpc::new(channel);
                    if let Err(e) = grpc.ready().await {
                        return TestResult::failed(start.elapsed(), format!("service was not ready: {e}"));
                    }
                    // No generated client exists for "test.OtherService" (it isn't a real
                    // service the test gRPC server implements — the point is that the
                    // *route* must never forward to it), so this builds the unary call
                    // directly against the raw path, reusing HelloRequest/HelloResponse
                    // as a stand-in codec shape; the call is expected to be rejected by
                    // the gateway's routing layer before any backend is contacted.
                    let codec = tonic::codec::ProstCodec::<HelloRequest, HelloResponse>::default();
                    let path = http::uri::PathAndQuery::from_static("/test.OtherService/Whatever");
                    let req = tonic::Request::new(HelloRequest {
                        name: "probe".to_string(),
                    });
                    match grpc.unary(req, path, codec).await {
                        Ok(_) => TestResult::failed(
                            start.elapsed(),
                            "expected UNIMPLEMENTED for an out-of-scope service, but the call succeeded".to_string(),
                        ),
                        Err(status) if status.code() == tonic::Code::Unimplemented => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("correctly UNIMPLEMENTED: {}", status.message()),
                        ),
                        Err(status) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "expected UNIMPLEMENTED(12), got {:?}: {}",
                                status.code(),
                                status.message()
                            ),
                        ),
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

    /// Parent status: PartiallyInvalid=True naming the dropped out-of-scope child rule;
    /// ResolvedRefs stays True (the drop is a rule-level finding, not a missing-child error).
    fn test_parent_status_partially_invalid_and_resolved_refs() -> TestCase {
        TestCase::new(
            "grpc_parent_status_partially_invalid",
            "Parent GRPCRoute gets PartiallyInvalid=True naming the dropped out-of-scope child rule, ResolvedRefs stays True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let mut last_status: Option<serde_json::Value> = None;
                    for _ in 0..20 {
                        if let Ok(route) =
                            status_helpers::fetch_resource_status(&ctx, "GRPCRoute", PARENT_NS, PARENT_NAME).await
                        {
                            let partially_invalid_ok = status_helpers::has_condition_with_message(
                                &route,
                                "PartiallyInvalid",
                                "True",
                                "rules[1]",
                            ) && status_helpers::has_condition_with_message(
                                &route,
                                "PartiallyInvalid",
                                "True",
                                "outside every delegated scope",
                            );
                            let resolved_refs_ok = status_helpers::has_condition(&route, "ResolvedRefs", "True");
                            if partially_invalid_ok && resolved_refs_ok {
                                return TestResult::passed_with_message(
                                    start.elapsed(),
                                    "PartiallyInvalid=True names the dropped rule; ResolvedRefs=True".to_string(),
                                );
                            }
                            last_status = Some(route);
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }
                    TestResult::failed(
                        start.elapsed(),
                        format!(
                            "parent status did not reach expected conditions; last: {}",
                            last_status
                                .map(|s| serde_json::to_string_pretty(&s).unwrap_or_default())
                                .unwrap_or_else(|| "none".to_string())
                        ),
                    )
                })
            },
        )
    }

    /// Child status: Accepted=True for its kind:GRPCRoute reverse-lock parentRef.
    fn test_child_reverse_lock_accepted() -> TestCase {
        TestCase::new(
            "grpc_child_reverse_lock_accepted",
            "Child GRPCRoute gets Accepted=True for its kind:GRPCRoute reverse-lock parentRef",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let mut last_status: Option<serde_json::Value> = None;
                    for _ in 0..20 {
                        if let Ok(route) =
                            status_helpers::fetch_resource_status(&ctx, "GRPCRoute", CHILD_NS, CHILD_NAME).await
                        {
                            if status_helpers::has_condition_with_message(
                                &route,
                                "Accepted",
                                "True",
                                "delegates to this route",
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
            Self::test_parent_status_partially_invalid_and_resolved_refs(),
            Self::test_child_reverse_lock_accepted(),
        ]
    }
}

// ============================================================
// Dynamic scenario phase suites (GrpcRouteDelegationDynamic)
// ============================================================

const DYN_HOST: &str = "grpc-delegation-dyn.example.com";
// Hard-code the DynamicTest listener port (matches http_route/delegation.rs's
// dyn_status): ctx.grpc_port under --dynamic-scenario points at the "default"
// port config (the test_server backend directly), which would bypass the
// gateway entirely.
const DYN_PORT: u16 = 31935;

async fn dyn_say_hello_status(ctx: &TestContext) -> Result<tonic::Code, String> {
    let endpoint = make_endpoint(ctx, DYN_HOST, DYN_PORT)?;
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
    let endpoint = make_endpoint(ctx, DYN_HOST, DYN_PORT)?;
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
