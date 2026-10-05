// gRPC-web bridge test suite.
//
// gRPC-web is routed via HTTPRoute (not GRPCRoute). The gateway's built-in GrpcWebBridge
// converts the grpc-web request to native gRPC (h2) toward the backend, and encodes the
// gRPC trailers back into a 0x80-prefixed body frame for the grpc-web client.
//
// Required config (edgion-tests/integration/conf/HTTPRoute/Protocol/GrpcWeb/):
// - Service_test-grpcweb.yaml (appProtocol: kubernetes.io/h2c), EndpointSlice_test-grpcweb.yaml
// - Gateway.yaml, HTTPRoute.yaml (host grpcweb.test.example.com)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use bytes::{BufMut, BytesMut};
use prost::Message;
use std::time::{Duration, Instant};

use super::test::{HelloRequest, HelloResponse};

const GRPC_WEB_HOST: &str = "grpcweb.test.example.com";

/// Frame a protobuf message as a gRPC-web data frame: [0x00][u32 BE len][bytes].
fn frame_grpc_web_request(msg_bytes: &[u8]) -> Vec<u8> {
    let mut buf = BytesMut::with_capacity(5 + msg_bytes.len());
    buf.put_u8(0x00); // data frame, uncompressed
    buf.put_u32(msg_bytes.len() as u32);
    buf.put_slice(msg_bytes);
    buf.to_vec()
}

/// A decoded gRPC-web frame.
struct GrpcWebFrame {
    flag: u8,
    payload: Vec<u8>,
}

/// Parse a gRPC-web response body into its frames. Returns Err on a truncated frame.
fn parse_grpc_web_frames(body: &[u8]) -> Result<Vec<GrpcWebFrame>, String> {
    let mut frames = Vec::new();
    let mut i = 0usize;
    while i < body.len() {
        if i + 5 > body.len() {
            return Err(format!("truncated frame header at offset {}", i));
        }
        let flag = body[i];
        let len = u32::from_be_bytes([body[i + 1], body[i + 2], body[i + 3], body[i + 4]]) as usize;
        let start = i + 5;
        let end = start + len;
        if end > body.len() {
            return Err(format!("truncated frame payload: need {} have {}", end, body.len()));
        }
        frames.push(GrpcWebFrame {
            flag,
            payload: body[start..end].to_vec(),
        });
        i = end;
    }
    Ok(frames)
}

/// Find the trailer frame (flag bit 0x80 set) and return its decoded text.
fn trailer_text(frames: &[GrpcWebFrame]) -> Option<String> {
    frames
        .iter()
        .find(|f| f.flag & 0x80 != 0)
        .map(|f| String::from_utf8_lossy(&f.payload).to_string())
}

#[derive(Default)]
pub struct GrpcWebTestSuite {
    http1_only: bool,
}

impl GrpcWebTestSuite {
    pub fn http1_only() -> Self {
        Self { http1_only: true }
    }

    fn client() -> Result<reqwest::Client, reqwest::Error> {
        reqwest::Client::builder()
            .http1_only()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
    }

    /// Success: a grpc-web SayHello returns a data frame + a 0x80 trailer frame carrying
    /// grpc-status:0.
    fn test_grpc_web_success() -> TestCase {
        TestCase::new(
            "grpc_web_success",
            "gRPC-web SayHello returns 0x80 trailer with grpc-status:0",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:{}/test.TestService/SayHello", ctx.target_host, ctx.http_port);

                    let req_msg = HelloRequest {
                        name: "Edgion".to_string(),
                    };
                    let body = frame_grpc_web_request(&req_msg.encode_to_vec());

                    let client = match Self::client() {
                        Ok(client) => client,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("build HTTP/1.1 client: {}", e)),
                    };
                    let resp = match client
                        .post(&url)
                        .header("host", GRPC_WEB_HOST)
                        .header("content-type", "application/grpc-web+proto")
                        .body(body)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("request failed: {}", e)),
                    };

                    if resp.status() != reqwest::StatusCode::OK {
                        return TestResult::failed(start.elapsed(), format!("HTTP status {}", resp.status()));
                    }
                    if resp.version() != reqwest::Version::HTTP_11 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("expected HTTP/1.1, got {:?}", resp.version()),
                        );
                    }
                    if resp.headers().get("grpc-status").is_some() {
                        return TestResult::failed(start.elapsed(), "grpc-status must be body-encoded".to_string());
                    }
                    let ct = resp
                        .headers()
                        .get("content-type")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    if !ct.starts_with("application/grpc-web") {
                        return TestResult::failed(start.elapsed(), format!("content-type not grpc-web: {}", ct));
                    }

                    let bytes = match resp.bytes().await {
                        Ok(b) => b,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("read body: {}", e)),
                    };
                    let frames = match parse_grpc_web_frames(&bytes) {
                        Ok(f) => f,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("frame parse: {}", e)),
                    };
                    if frames.len() != 2 || frames[0].flag != 0x00 || frames[1].flag != 0x80 {
                        return TestResult::failed(
                            start.elapsed(),
                            "expected one uncompressed protobuf frame followed by one trailer frame".to_string(),
                        );
                    }
                    let reply = match HelloResponse::decode(frames[0].payload.as_slice()) {
                        Ok(reply) => reply,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("decode SayHello response: {}", e))
                        }
                    };
                    // test_server may append echoed x-* metadata after the greeting.
                    if reply.message != "Hello, Edgion!" && !reply.message.starts_with("Hello, Edgion! metadata=(") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("unexpected greeting: {:?}", reply.message),
                        );
                    }
                    match trailer_text(&frames) {
                        Some(t)
                            if t.lines().any(|line| {
                                line.split_once(':')
                                    .is_some_and(|(key, value)| key == "grpc-status" && value.trim() == "0")
                            }) =>
                        {
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("trailer: {}", t.replace("\r\n", " ")),
                            )
                        }
                        Some(t) => TestResult::failed(
                            start.elapsed(),
                            format!("unexpected trailer: {}", t.replace("\r\n", " ")),
                        ),
                        None => TestResult::failed(start.elapsed(), "no 0x80 trailer frame in response".to_string()),
                    }
                })
            },
        )
    }

    /// Error: a grpc-web request to an unrouted path returns a 0x80 trailer carrying a
    /// non-zero grpc-status (NOT_FOUND/5), NOT grpc-status in the HEADERS.
    fn test_grpc_web_error() -> TestCase {
        TestCase::new(
            "grpc_web_error",
            "gRPC-web unrouted path returns 0x80 trailer with grpc-status:5",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    // Path does not match the /test.TestService prefix -> 404 -> grpc NOT_FOUND.
                    let url = format!("http://{}:{}/no.such.Service/Method", ctx.target_host, ctx.http_port);

                    let req_msg = HelloRequest {
                        name: "Edgion".to_string(),
                    };
                    let body = frame_grpc_web_request(&req_msg.encode_to_vec());

                    let client = match Self::client() {
                        Ok(client) => client,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("build HTTP/1.1 client: {}", e)),
                    };
                    let resp = match client
                        .post(&url)
                        .header("host", GRPC_WEB_HOST)
                        .header("content-type", "application/grpc-web+proto")
                        .body(body)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("request failed: {}", e)),
                    };

                    // grpc-status must NOT be a response header (that is the native single-frame
                    // shape and is wrong for grpc-web).
                    if resp.headers().get("grpc-status").is_some() {
                        return TestResult::failed(
                            start.elapsed(),
                            "grpc-status leaked into HEADERS; expected body-encoded trailer".to_string(),
                        );
                    }

                    let bytes = match resp.bytes().await {
                        Ok(b) => b,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("read body: {}", e)),
                    };
                    let frames = match parse_grpc_web_frames(&bytes) {
                        Ok(f) => f,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("frame parse: {}", e)),
                    };
                    match trailer_text(&frames) {
                        Some(t) if t.contains("grpc-status:5") => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("trailer: {}", t.replace("\r\n", " ")),
                        ),
                        Some(t) => TestResult::failed(
                            start.elapsed(),
                            format!("unexpected trailer: {}", t.replace("\r\n", " ")),
                        ),
                        None => {
                            TestResult::failed(start.elapsed(), "no 0x80 trailer frame in error response".to_string())
                        }
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for GrpcWebTestSuite {
    fn name(&self) -> &str {
        if self.http1_only {
            "gRPC-web Bridge (HTTP/1-only Gateway)"
        } else {
            "gRPC-web Bridge"
        }
    }

    fn port_key(&self) -> Option<&str> {
        Some(if self.http1_only {
            "HTTPRoute/Protocol/GrpcWebHttp1Only"
        } else {
            "HTTPRoute/Protocol/GrpcWeb"
        })
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_grpc_web_success(), Self::test_grpc_web_error()]
    }
}
