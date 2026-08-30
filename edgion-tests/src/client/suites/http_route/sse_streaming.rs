//! End-to-end response-streaming and compression coverage.
//!
//! The dedicated GatewayClass enables downstream compression. SSE must remain
//! uncompressed and arrive event-by-event, while an ordinary JSON response on
//! the same listener must be gzip-compressed.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct SseStreamingTestSuite;

const EVENT_COUNT: usize = 5;
const INTERVAL_MS: u64 = 300;
// A fully buffered five-event stream spans about 1,200ms, so this still catches
// the oldest event in a final burst while allowing substantial CI scheduler
// slack for correctly streamed events.
const MAX_EVENT_DELIVERY_LATENCY_MS: u64 = 1_000;
const MAX_FIRST_EVENT_AFTER_HEADERS_MS: u64 = 900;
// Primary no-final-burst invariant: correctly paced arrivals cover about
// 1,200ms, while a buffered response's events arrive together.
const MIN_EVENT_ARRIVAL_SPAN_MS: u64 = 900;

#[derive(Debug, Deserialize)]
struct EventPayload {
    seq: usize,
    ts_ms: u64,
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn raw_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .no_proxy()
        // Preserve Content-Encoding and encoded bytes if reqwest's optional
        // gzip feature is enabled in the future. The current workspace build
        // does not enable it, but this assertion should remain explicit.
        .no_gzip()
        .build()
        .map_err(|e| format!("failed to create raw HTTP client: {e}"))
}

fn parse_event(frame: &[u8]) -> Result<EventPayload, String> {
    let frame = std::str::from_utf8(frame).map_err(|e| format!("SSE event was not UTF-8: {e}"))?;
    let data = frame
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .ok_or_else(|| format!("SSE event had no data field: {frame:?}"))?;
    serde_json::from_str(data).map_err(|e| format!("SSE data was not the expected JSON payload: {e}"))
}

impl SseStreamingTestSuite {
    fn test_sse_is_uncompressed_and_paced() -> TestCase {
        TestCase::new(
            "sse_is_uncompressed_and_paced",
            "SSE bypasses gzip and every event arrives near its backend send time",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = match raw_http_client() {
                        Ok(client) => client,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let url = format!("{}/sse?events={EVENT_COUNT}&interval_ms={INTERVAL_MS}", ctx.http_url());
                    let mut request = client.get(url).header("Accept-Encoding", "gzip");
                    if let Some(host) = &ctx.http_host {
                        request = request.header("Host", host);
                    }

                    let response = match request.send().await {
                        Ok(response) => response,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("SSE request failed: {e}")),
                    };
                    if !response.status().is_success() {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("expected SSE status 200, got {}", response.status()),
                        );
                    }
                    let content_type = response
                        .headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("");
                    if !content_type.starts_with("text/event-stream") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("expected text/event-stream, got {content_type:?}"),
                        );
                    }
                    if let Some(encoding) = response.headers().get(reqwest::header::CONTENT_ENCODING) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("SSE must not be compressed, got Content-Encoding {encoding:?}"),
                        );
                    }

                    // Start the first-body timer only after response headers are
                    // available and validated. Connection setup and header
                    // processing are not part of the streaming-body invariant.
                    let body_wait_started = Instant::now();
                    let mut stream = response.bytes_stream();
                    let mut pending = Vec::new();
                    let mut arrivals = Vec::with_capacity(EVENT_COUNT);
                    while let Some(chunk) = stream.next().await {
                        let chunk = match chunk {
                            Ok(chunk) => chunk,
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("failed while reading SSE stream: {e}"),
                                );
                            }
                        };
                        pending.extend_from_slice(&chunk);
                        if pending.len() > 64 * 1024 {
                            return TestResult::failed(
                                start.elapsed(),
                                "SSE test receive buffer exceeded 64 KiB".to_string(),
                            );
                        }

                        while let Some(boundary) = pending.windows(2).position(|window| window == b"\n\n") {
                            let mut framed = pending.drain(..boundary + 2).collect::<Vec<_>>();
                            framed.truncate(boundary);
                            let received_at = Instant::now();
                            let received_epoch_ms = epoch_ms();
                            let payload = match parse_event(&framed) {
                                Ok(payload) => payload,
                                Err(e) => return TestResult::failed(start.elapsed(), e),
                            };
                            let delivery_latency_ms = received_epoch_ms.saturating_sub(payload.ts_ms);
                            if delivery_latency_ms >= MAX_EVENT_DELIVERY_LATENCY_MS {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "SSE event {} arrived {}ms after the backend timestamp; expected <{}ms",
                                        payload.seq, delivery_latency_ms, MAX_EVENT_DELIVERY_LATENCY_MS
                                    ),
                                );
                            }
                            arrivals.push((payload.seq, received_at));
                        }
                    }

                    if arrivals.len() != EVENT_COUNT {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("expected {EVENT_COUNT} SSE events, got {}", arrivals.len()),
                        );
                    }
                    for (expected, (actual, _)) in arrivals.iter().enumerate() {
                        if *actual != expected {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("expected SSE sequence {expected}, got {actual}"),
                            );
                        }
                    }

                    let first_body_wait = arrivals[0].1.duration_since(body_wait_started);
                    if first_body_wait >= Duration::from_millis(MAX_FIRST_EVENT_AFTER_HEADERS_MS) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "first SSE event took {first_body_wait:?} after response headers; expected <{}ms",
                                MAX_FIRST_EVENT_AFTER_HEADERS_MS
                            ),
                        );
                    }
                    let stream_span = arrivals[EVENT_COUNT - 1].1.duration_since(arrivals[0].1);
                    let minimum_span = Duration::from_millis(MIN_EVENT_ARRIVAL_SPAN_MS);
                    if stream_span < minimum_span {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "SSE events arrived in a burst over {stream_span:?}; expected at least {minimum_span:?}"
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "received {EVENT_COUNT} uncompressed SSE events over {stream_span:?}; first event arrived {first_body_wait:?} after headers"
                        ),
                    )
                })
            },
        )
    }

    fn test_json_is_gzip_compressed() -> TestCase {
        TestCase::new(
            "json_is_gzip_compressed",
            "The same compression-enabled listener gzip-compresses ordinary JSON",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = match raw_http_client() {
                        Ok(client) => client,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let mut request = client
                        .get(format!("{}/headers", ctx.http_url()))
                        .header("Accept-Encoding", "gzip");
                    if let Some(host) = &ctx.http_host {
                        request = request.header("Host", host);
                    }
                    let response = match request.send().await {
                        Ok(response) => response,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("JSON request failed: {e}")),
                    };
                    if !response.status().is_success() {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("expected JSON status 200, got {}", response.status()),
                        );
                    }
                    let content_type = response
                        .headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("");
                    if !content_type.starts_with("application/json") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("expected application/json, got {content_type:?}"),
                        );
                    }
                    let encoding = response
                        .headers()
                        .get(reqwest::header::CONTENT_ENCODING)
                        .and_then(|value| value.to_str().ok());
                    if encoding != Some("gzip") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("expected Content-Encoding gzip, got {encoding:?}"),
                        );
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "ordinary JSON response was gzip-compressed".to_string(),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for SseStreamingTestSuite {
    fn name(&self) -> &str {
        "HTTPRoute/SseStreaming"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_sse_is_uncompressed_and_paced(),
            Self::test_json_is_gzip_compressed(),
        ]
    }
}
