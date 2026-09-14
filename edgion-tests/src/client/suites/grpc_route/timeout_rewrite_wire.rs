use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::transport::{TransportEvent, TransportProbe, TransportSnapshot, KIND_REQ_HEADERS, LISTENER_H2};
use async_trait::async_trait;
use bytes::{BufMut, BytesMut};
use prost::Message;
use std::time::{Duration, Instant};

use super::test::test_service_client::TestServiceClient;
use super::test::{HelloRequest, HelloResponse};

const GRPC_HOST: &str = "grpc-timeout-wire.example.com";
const GRPC_WEB_HOST: &str = "grpc-web-timeout-wire.example.com";
const CLIENT_TIMEOUT: &str = "5000000u";
const LOCAL_RECORDER_HOST: &str = "127.0.0.1";

pub struct GrpcTimeoutRewriteWireTestSuite;

fn recorder_host_from(value: Option<String>) -> String {
    value
        .filter(|host| !host.trim().is_empty())
        .unwrap_or_else(|| LOCAL_RECORDER_HOST.to_string())
}

fn recorder_host() -> String {
    recorder_host_from(std::env::var("EDGION_TEST_TRANSPORT_RECORDER_HOST").ok())
}

fn is_k8s_mode() -> bool {
    std::env::var("EDGION_TEST_K8S_MODE").as_deref() == Ok("true")
}

fn validate_retry_decay(values: &[u128], gaps: &[u64; 2], k8s_mode: bool) -> Result<(), String> {
    if !values.windows(2).all(|window| window[1] <= window[0]) {
        return Err(format!("timeout regained budget: {values:?}"));
    }
    if k8s_mode {
        if !values.windows(2).all(|window| window[1] < window[0]) {
            return Err(format!(
                "timeout did not strictly decay across transport retries: {values:?}"
            ));
        }
        return Ok(());
    }
    if gaps.iter().any(|gap| *gap < 75) {
        return Err(format!("configured 100ms backoff missing from recorder gaps {gaps:?}"));
    }
    if values[0].saturating_sub(values[2]) < 150_000_000 {
        return Err(format!("timeout decay too small: {values:?}"));
    }
    Ok(())
}

fn timeout_ns(event: &TransportEvent) -> Result<u128, String> {
    let value = event
        .grpc_timeout_value
        .ok_or_else(|| format!("missing parsed grpc-timeout: {event}"))?;
    let unit = event
        .grpc_timeout_unit
        .as_deref()
        .ok_or_else(|| format!("missing grpc-timeout unit: {event}"))?;
    if value.to_string().len() > 8 {
        return Err(format!("grpc-timeout has more than eight digits: {event}"));
    }
    let scale = match unit {
        "H" => 3_600_000_000_000u128,
        "M" => 60_000_000_000,
        "S" => 1_000_000_000,
        "m" => 1_000_000,
        "u" => 1_000,
        "n" => 1,
        _ => return Err(format!("unsupported grpc-timeout unit: {event}")),
    };
    Ok(u128::from(value) * scale)
}

async fn grpc_call(ctx: &TestContext, trace: &str, reset_first: Option<usize>) -> Result<HelloResponse, String> {
    let endpoint = tonic::transport::Endpoint::from_shared(format!("http://{}:{}", ctx.target_host, ctx.grpc_port))
        .map_err(|e| format!("endpoint: {e}"))?
        .origin(
            format!("http://{GRPC_HOST}:{}", ctx.grpc_port)
                .parse()
                .map_err(|e| format!("origin: {e}"))?,
        );
    let mut client = TestServiceClient::connect(endpoint)
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let mut request = tonic::Request::new(HelloRequest {
        name: "TimeoutWire".to_string(),
    });
    let metadata = request.metadata_mut();
    metadata.insert("grpc-timeout", CLIENT_TIMEOUT.parse().unwrap());
    metadata.insert(
        "traceparent",
        crate::access_log_client::traceparent_for_marker(trace)
            .parse()
            .map_err(|e| format!("trace metadata: {e}"))?,
    );
    metadata.insert("x-up-behavior", "grpc-say-hello".parse().unwrap());
    metadata.insert("x-up-reset-code", "REFUSED_STREAM".parse().unwrap());
    if let Some(count) = reset_first {
        metadata.insert("x-up-reset-first", count.to_string().parse().unwrap());
    }
    client
        .say_hello(request)
        .await
        .map(|r| r.into_inner())
        .map_err(|e| format!("SayHello: {:?}: {}", e.code(), e.message()))
}

fn req_headers(snapshot: &TransportSnapshot, trace: &str) -> Vec<TransportEvent> {
    snapshot
        .events()
        .iter()
        .filter(|e| e.is(KIND_REQ_HEADERS) && e.listener == LISTENER_H2 && e.tagged(trace))
        .cloned()
        .collect()
}

fn frame_request(message: &HelloRequest) -> Vec<u8> {
    let payload = message.encode_to_vec();
    let mut frame = BytesMut::with_capacity(5 + payload.len());
    frame.put_u8(0);
    frame.put_u32(payload.len() as u32);
    frame.put_slice(&payload);
    frame.to_vec()
}

fn parse_web_frames(body: &[u8]) -> Result<Vec<(u8, &[u8])>, String> {
    let mut frames = Vec::new();
    let mut offset = 0;
    while offset < body.len() {
        if offset + 5 > body.len() {
            return Err(format!("truncated frame header at {offset}"));
        }
        let flag = body[offset];
        let len = u32::from_be_bytes(body[offset + 1..offset + 5].try_into().unwrap()) as usize;
        let start = offset + 5;
        let end = start + len;
        if end > body.len() {
            return Err(format!("truncated frame body at {offset}"));
        }
        frames.push((flag, &body[start..end]));
        offset = end;
    }
    Ok(frames)
}

impl GrpcTimeoutRewriteWireTestSuite {
    fn native_elapsed() -> TestCase {
        TestCase::new(
            "grpc_timeout_rewrite_elapsed",
            "upstream grpc-timeout deducts the fixed request delay",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let probe = match TransportProbe::start(&recorder_host(), 30001, "grpc-timeout-elapsed").await {
                        Ok(v) => v,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let reply = match grpc_call(&ctx, probe.trace_id(), None).await {
                        Ok(v) => v,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("{e}\n{}", probe.dump().await)),
                    };
                    if reply.message != "Hello, TimeoutWire!" {
                        return TestResult::failed(start.elapsed(), format!("unexpected reply: {:?}", reply.message));
                    }
                    let snapshot = match probe.snapshot().await {
                        Ok(v) => v,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let heads = req_headers(&snapshot, probe.trace_id());
                    if heads.len() != 1 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("expected one ReqHeaders, got {}\n{snapshot}", heads.len()),
                        );
                    }
                    match timeout_ns(&heads[0]) {
                        Ok(ns) if ns < 4_900_000_000 && ns > 3_000_000_000 => TestResult::passed_with_message(
                            start.elapsed(),
                            format!(
                                "observed {}{} after 300ms delay",
                                heads[0].grpc_timeout_value.unwrap(),
                                heads[0].grpc_timeout_unit.as_deref().unwrap()
                            ),
                        ),
                        Ok(ns) => TestResult::failed(
                            start.elapsed(),
                            format!("timeout did not show bounded initial decay: {ns}ns\n{snapshot}"),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    fn retry_decay() -> TestCase {
        TestCase::new(
            "grpc_timeout_rewrite_retries",
            "REFUSED_STREAM attempts never regain timeout budget",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let probe = match TransportProbe::start(&recorder_host(), 30001, "grpc-timeout-retry").await {
                        Ok(v) => v,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let reply = match grpc_call(&ctx, probe.trace_id(), Some(2)).await {
                        Ok(v) => v,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("{e}\n{}", probe.dump().await)),
                    };
                    if reply.message != "Hello, TimeoutWire!" {
                        return TestResult::failed(start.elapsed(), format!("unexpected reply: {:?}", reply.message));
                    }
                    let snapshot = match probe.snapshot().await {
                        Ok(v) => v,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let heads = req_headers(&snapshot, probe.trace_id());
                    if heads.len() != 3 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "script resets twice then succeeds, expected exactly 3 ReqHeaders, got {}\n{snapshot}",
                                heads.len()
                            ),
                        );
                    }
                    let values: Vec<u128> = match heads.iter().map(timeout_ns).collect() {
                        Ok(v) => v,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let gaps = [
                        heads[1].at_ms.saturating_sub(heads[0].at_ms),
                        heads[2].at_ms.saturating_sub(heads[1].at_ms),
                    ];
                    if let Err(error) = validate_retry_decay(&values, &gaps, is_k8s_mode()) {
                        return TestResult::failed(start.elapsed(), format!("{error}\n{snapshot}"));
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!("three attempts, gaps={gaps:?}ms, timeouts={values:?}ns"),
                    )
                })
            },
        )
    }

    fn grpc_web_elapsed() -> TestCase {
        TestCase::new(
            "grpc_web_timeout_rewrite_elapsed",
            "gRPC-Web bridge preserves data and trailer framing after timeout rewrite",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let probe = match TransportProbe::start(&recorder_host(), 30001, "grpc-web-timeout").await {
                        Ok(v) => v,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let response = match reqwest::Client::builder()
                        .http1_only()
                        .no_proxy()
                        .timeout(Duration::from_secs(10))
                        .build()
                        .unwrap()
                        .post(format!(
                            "http://{}:{}/test.TestService/SayHello",
                            ctx.target_host, ctx.grpc_port
                        ))
                        .header("host", GRPC_WEB_HOST)
                        .header("content-type", "application/grpc-web+proto")
                        .header("grpc-timeout", CLIENT_TIMEOUT)
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(probe.trace_id()),
                        )
                        .header("x-up-behavior", "grpc-say-hello")
                        .body(frame_request(&HelloRequest {
                            name: "TimeoutWire".to_string(),
                        }))
                        .send()
                        .await
                    {
                        Ok(v) => v,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("request: {e}")),
                    };
                    if response.status() != 200 || response.headers().get("grpc-status").is_some() {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "invalid gRPC-Web response headers: {} {:?}",
                                response.status(),
                                response.headers()
                            ),
                        );
                    }
                    let body = match response.bytes().await {
                        Ok(v) => v,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("body: {e}")),
                    };
                    let frames = match parse_web_frames(&body) {
                        Ok(v) => v,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    if frames.len() != 2 || frames[0].0 != 0 || frames[1].0 != 0x80 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "expected data then trailer frame, flags={:?}",
                                frames.iter().map(|f| f.0).collect::<Vec<_>>()
                            ),
                        );
                    }
                    let reply = match HelloResponse::decode(frames[0].1) {
                        Ok(v) => v,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("decode reply: {e}")),
                    };
                    let trailers = String::from_utf8_lossy(frames[1].1);
                    if reply.message != "Hello, TimeoutWire!"
                        || !trailers.lines().any(|line| {
                            line.split_once(':')
                                .is_some_and(|(k, v)| k == "grpc-status" && v.trim() == "0")
                        })
                    {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("bad reply/trailers: {:?} {trailers:?}", reply.message),
                        );
                    }
                    let snapshot = match probe.snapshot().await {
                        Ok(v) => v,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let heads = req_headers(&snapshot, probe.trace_id());
                    if heads.len() != 1 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("expected one upstream ReqHeaders, got {}\n{snapshot}", heads.len()),
                        );
                    }
                    match timeout_ns(&heads[0]) {
                        Ok(ns) if ns < 4_900_000_000 => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("gRPC-Web success with upstream timeout {ns}ns"),
                        ),
                        Ok(ns) => {
                            TestResult::failed(start.elapsed(), format!("gRPC-Web timeout did not decay: {ns}ns"))
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for GrpcTimeoutRewriteWireTestSuite {
    fn name(&self) -> &str {
        "gRPC timeout rewrite wire"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::native_elapsed(), Self::retry_decay(), Self::grpc_web_elapsed()]
    }
}

#[cfg(test)]
mod tests {
    use super::{recorder_host_from, validate_retry_decay, LOCAL_RECORDER_HOST};

    #[test]
    fn recorder_host_uses_local_default_and_nonblank_override() {
        assert_eq!(recorder_host_from(None), LOCAL_RECORDER_HOST);
        assert_eq!(recorder_host_from(Some("".to_string())), LOCAL_RECORDER_HOST);
        assert_eq!(
            recorder_host_from(Some("transport-recorder.edgion-test.svc.cluster.local".to_string())),
            "transport-recorder.edgion-test.svc.cluster.local"
        );
    }

    #[test]
    fn retry_decay_uses_transport_semantics_in_k8s_and_backoff_semantics_locally() {
        let fast_gaps = [0, 1];
        let small_decay = [4_000_000_000, 3_999_999_000, 3_999_998_000];
        assert!(validate_retry_decay(&small_decay, &fast_gaps, true).is_ok());
        assert!(validate_retry_decay(&small_decay, &fast_gaps, false).is_err());

        let local_gaps = [100, 100];
        let local_decay = [4_000_000_000, 3_900_000_000, 3_800_000_000];
        assert!(validate_retry_decay(&local_decay, &local_gaps, false).is_ok());
    }

    #[test]
    fn k8s_retry_decay_rejects_equal_or_regained_timeout_budget() {
        assert!(validate_retry_decay(&[4_000, 4_000, 3_999], &[0, 1], true).is_err());
        assert!(validate_retry_decay(&[4_000, 3_999, 4_000], &[0, 1], true).is_err());
    }
}
