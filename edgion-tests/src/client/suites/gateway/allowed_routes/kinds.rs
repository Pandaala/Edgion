// AllowedRoutes Kinds test suite
//
// Required config files:
// - Gateway/AllowedRoutes/Kinds/Gateway.yaml         # Gateway with allowedRoutes.kinds: [HTTPRoute]
// - Gateway/AllowedRoutes/Kinds/HTTPRoute.yaml       # HTTPRoute (allowed)
// - Gateway/AllowedRoutes/Kinds/GRPCRoute.yaml       # GRPCRoute (denied)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::grpc_route::test::HelloRequest;
use crate::transport::RawH2Conn;
use bytes::Bytes;
use prost::Message;
use std::time::Instant;

#[cfg(test)]
#[path = "kinds_outcome_tests.rs"]
mod outcome_tests;

fn validate_kind_denial(
    status: http::StatusCode,
    headers: &http::HeaderMap,
    receive_end_stream: bool,
) -> Result<(), &'static str> {
    if status != http::StatusCode::OK {
        return Err("Expected HTTP 200 for native gRPC kind denial");
    }
    let mut content_types = headers.get_all(http::header::CONTENT_TYPE).iter();
    if content_types.next().map(|value| value.as_bytes()) != Some(b"application/grpc") || content_types.next().is_some()
    {
        return Err("Expected exactly one application/grpc content-type for kind denial");
    }
    // This fixture has no admitted native routes: its HTTP lookup miss maps to
    // NOT_FOUND, not the UNIMPLEMENTED produced by a populated native table.
    let mut grpc_statuses = headers.get_all("grpc-status").iter();
    if grpc_statuses.next().map(|value| value.as_bytes()) != Some(b"5") || grpc_statuses.next().is_some() {
        return Err("Expected exactly one grpc-status 5 for kind denial");
    }
    // h2 requires received EOS and an empty pending receive queue. No response
    // DATA or trailers may remain; a later send-half reset preserves this EOS.
    if !receive_end_stream {
        return Err("Expected a completed empty native gRPC kind-denial response");
    }
    Ok(())
}

async fn probe_kind_denial(ctx: &TestContext) -> Result<(), &'static str> {
    let mut connection = RawH2Conn::connect(&ctx.target_host, 31213)
        .await
        .map_err(|_| "Failed to connect to the kind-denial HTTP/2 listener")?;
    let (response, mut request_body) = connection
        .send_request(
            "POST",
            "http://kinds-grpc.example.com:31213/test.TestService/SayHello",
            &[("content-type", "application/grpc"), ("te", "trailers")],
            false,
        )
        .map_err(|_| "Failed to send kind-denial gRPC request headers")?;
    let message = HelloRequest {
        name: "KindsTest".to_string(),
    }
    .encode_to_vec();
    let message_length = u32::try_from(message.len()).map_err(|_| "Kind-denial gRPC request is too large")?;
    let mut frame = Vec::with_capacity(message.len() + 5);
    frame.push(0);
    frame.extend_from_slice(&message_length.to_be_bytes());
    frame.extend_from_slice(&message);
    request_body
        .send_data(Bytes::from(frame), true)
        .map_err(|_| "Failed to send kind-denial gRPC request message")?;
    let response = response
        .await
        .map_err(|_| "Failed to receive kind-denial gRPC response")?;
    let result = validate_kind_denial(response.status(), response.headers(), response.body().is_end_stream());
    // RawH2Conn owns the driver through the observation and aborts it on every
    // exit, including cancellation by the owning TestCase's 30-second timeout.
    drop(response);
    drop(connection);
    result
}

pub struct AllowedRoutesKindsTestSuite;

impl AllowedRoutesKindsTestSuite {
    /// Test HTTPRoute is allowed when kinds includes HTTPRoute
    fn test_http_route_allowed() -> TestCase {
        TestCase::new(
            "http_route_allowed",
            "Test HTTPRoute is allowed when specified in kinds",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    // Use no_proxy to bypass system proxy for local testing
                    let client = reqwest::Client::builder().no_proxy().build().unwrap();
                    let url = format!("http://{}:31213/health", ctx.target_host);

                    let response = client.get(&url).header("Host", "kinds-http.example.com").send().await;

                    match response {
                        Ok(resp) => {
                            if resp.status().is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "✓ HTTPRoute allowed when specified in kinds (status: {})",
                                        resp.status()
                                    ),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected 200 OK for HTTPRoute with kinds restriction, got {}",
                                        resp.status()
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test GRPCRoute is denied when kinds doesn't include GRPCRoute
    fn test_grpc_route_denied() -> TestCase {
        TestCase::new(
            "grpc_route_denied",
            "Test GRPCRoute is denied when not in kinds (negative test)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    match probe_kind_denial(&ctx).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "GRPCRoute returned the completed native NOT_FOUND kind-denial response".to_string(),
                        ),
                        Err(message) => TestResult::failed(start.elapsed(), message.to_string()),
                    }
                })
            },
        )
    }
}

impl TestSuite for AllowedRoutesKindsTestSuite {
    fn name(&self) -> &str {
        "Gateway AllowedRoutes Kinds Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_http_route_allowed(), Self::test_grpc_route_denied()]
    }
}
