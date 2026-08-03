// ExtProc Plugin Test Suite
//
// Tests the ExtProc plugin, which hands request/response phases to an external
// processing service over a bidirectional gRPC stream (Envoy ext_proc
// compatible). The fake processor (test_server --ext-proc-port 30041):
//   - request phase: adds `x-ext-proc-req: seen`, or returns 403 if the request
//     carries `x-ext-proc-block`.
//   - response phase: adds `x-ext-proc-resp: seen`.
//
// Scenarios:
//   1. Basic: request header added (seen by backend) + response header added.
//   2. ImmediateResponse: `x-ext-proc-block` short-circuits with 403.
//   3. fail-close: a dead processor target returns 500 (failureModeAllow=false).
//   4. fail-open: a dead processor target is bypassed, request reaches backend.
//
// Requires the `--gateway` flag (exercises the real data path).

use std::time::Instant;

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};

const BASIC_HOST: &str = "ext-proc-basic.example.com";
const FAILCLOSE_HOST: &str = "ext-proc-failclose.example.com";
const FAILOPEN_HOST: &str = "ext-proc-failopen.example.com";
const COEXIST_HOST: &str = "ext-proc-coexist.example.com";
const TLS_HOST: &str = "ext-proc-tls.example.com";
const TLS_NOVERIFY_HOST: &str = "ext-proc-tls-noverify.example.com";
const STREAMED_HOST: &str = "ext-proc-streamed.example.com";
const STREAMED_FAILOPEN_HOST: &str = "ext-proc-streamed-failopen.example.com";

pub struct ExtProcTestSuite;

/// Shared assertion for the over-TLS scenarios: the processor adds a request
/// header (echoed by the backend `/headers`) and a response header (visible to
/// the client). Both phases dial the processor over TLS, so success proves the
/// rustls ext_proc connector completed the TLS+h2 handshake on request AND
/// upstream-response stages.
async fn assert_headers_applied_over_tls(ctx: TestContext, host: &str) -> TestResult {
    let start = Instant::now();
    let url = format!("{}/headers", ctx.http_url());
    let response = match ctx.http_client.get(&url).header("host", host).send().await {
        Ok(r) => r,
        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
    };

    let status = response.status().as_u16();
    if status != 200 {
        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
    }

    // Response header added by the response phase over TLS.
    let resp_header_ok = response
        .headers()
        .get("x-ext-proc-resp")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == "seen")
        .unwrap_or(false);

    // Request header added by the request phase over TLS, echoed by /headers.
    let body = response.text().await.unwrap_or_default();
    let body_lower = body.to_lowercase();
    let req_header_ok = body_lower.contains("x-ext-proc-req") && body_lower.contains("seen");

    if req_header_ok && resp_header_ok {
        TestResult::passed_with_message(
            start.elapsed(),
            "request and response headers both applied over TLS".to_string(),
        )
    } else {
        TestResult::failed(
            start.elapsed(),
            format!(
                "req_header_ok={} resp_header_ok={}; body: {}",
                req_header_ok,
                resp_header_ok,
                &body[..body.len().min(400)]
            ),
        )
    }
}

impl ExtProcTestSuite {
    /// Basic: the processor adds a request header (echoed by the backend) and a
    /// response header (visible to the client).
    fn test_basic_request_and_response_headers() -> TestCase {
        TestCase::new(
            "basic_request_and_response_headers",
            "ext_proc adds a request header (seen by backend) and a response header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/headers", ctx.http_url());
                    let response = match ctx.http_client.get(&url).header("host", BASIC_HOST).send().await {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    let status = response.status().as_u16();
                    if status != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                    }

                    // Response header added by the response phase.
                    let resp_header_ok = response
                        .headers()
                        .get("x-ext-proc-resp")
                        .and_then(|v| v.to_str().ok())
                        .map(|v| v == "seen")
                        .unwrap_or(false);

                    // Request header added by the request phase, echoed by /headers.
                    let body = response.text().await.unwrap_or_default();
                    let body_lower = body.to_lowercase();
                    let req_header_ok = body_lower.contains("x-ext-proc-req") && body_lower.contains("seen");

                    if req_header_ok && resp_header_ok {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "request and response headers both applied".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "req_header_ok={} resp_header_ok={}; body: {}",
                                req_header_ok,
                                resp_header_ok,
                                &body[..body.len().min(400)]
                            ),
                        )
                    }
                })
            },
        )
    }

    /// ExtProc over TLS (Scenario A): caSource SecretRef + verify. The processor
    /// is reached over an HTTPS/h2 stream (rustls connector trusting the private
    /// CA); both request and response header mutations must apply over TLS.
    fn test_tls_request_and_response_headers() -> TestCase {
        TestCase::new(
            "tls_request_and_response_headers",
            "ext_proc over TLS (caSource SecretRef + verify) applies request and response headers",
            |ctx: TestContext| Box::pin(async move { assert_headers_applied_over_tls(ctx, TLS_HOST).await }),
        )
    }

    /// ExtProc over TLS (Scenario B): verify=false against the self-signed
    /// processor cert. The stream still establishes (AcceptAll verifier, no env
    /// gate after M-A1) and mutates headers.
    fn test_tls_noverify_request_and_response_headers() -> TestCase {
        TestCase::new(
            "tls_noverify_request_and_response_headers",
            "ext_proc over TLS (verify=false) still establishes the stream and applies headers",
            |ctx: TestContext| Box::pin(async move { assert_headers_applied_over_tls(ctx, TLS_NOVERIFY_HOST).await }),
        )
    }

    /// ImmediateResponse: the `x-ext-proc-block` trigger short-circuits with 403.
    fn test_immediate_response_403() -> TestCase {
        TestCase::new(
            "immediate_response_403",
            "ext_proc returns an ImmediateResponse 403 when triggered",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/headers", ctx.http_url());
                    let response = match ctx
                        .http_client
                        .get(&url)
                        .header("host", BASIC_HOST)
                        .header("x-ext-proc-block", "1")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    let status = response.status().as_u16();
                    if status != 403 {
                        return TestResult::failed(start.elapsed(), format!("Expected 403, got {}", status));
                    }
                    // The ImmediateResponse also carries a custom header mutation;
                    // verify it reached the client (request-stage immediate replies
                    // honor processor header mutations).
                    let denied_header_ok = response
                        .headers()
                        .get("x-ext-proc-denied")
                        .and_then(|v| v.to_str().ok())
                        .map(|v| v == "1")
                        .unwrap_or(false);
                    let body = response.text().await.unwrap_or_default();
                    if body.contains("blocked by ext_proc") && denied_header_ok {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "short-circuited with 403 + custom header".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("403 unexpected: denied_header_ok={} body={}", denied_header_ok, body),
                        )
                    }
                })
            },
        )
    }

    /// fail-close: an unreachable processor returns 500.
    fn test_fail_close_returns_500() -> TestCase {
        TestCase::new(
            "fail_close_returns_500",
            "ext_proc with failureModeAllow=false returns 500 when the processor is unreachable",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/get", ctx.http_url());
                    let response = match ctx.http_client.get(&url).header("host", FAILCLOSE_HOST).send().await {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    let status = response.status().as_u16();
                    if status != 500 {
                        return TestResult::failed(start.elapsed(), format!("Expected 500, got {}", status));
                    }
                    // Assert the 500 came from the ext_proc fail-close path, not an
                    // unrelated upstream error.
                    let body = response.text().await.unwrap_or_default();
                    if body.contains("ext_proc unavailable") {
                        TestResult::passed_with_message(start.elapsed(), "fail-close returned 500".to_string())
                    } else {
                        TestResult::failed(start.elapsed(), format!("500 body not from ext_proc: {}", body))
                    }
                })
            },
        )
    }

    /// fail-open: an unreachable processor is bypassed; the request reaches the backend.
    fn test_fail_open_passes() -> TestCase {
        TestCase::new(
            "fail_open_passes",
            "ext_proc with failureModeAllow=true forwards to the backend when the processor is unreachable",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/get", ctx.http_url());
                    let response = match ctx.http_client.get(&url).header("host", FAILOPEN_HOST).send().await {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    let status = response.status().as_u16();
                    if status != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                    }
                    // The processor is dead, so it cannot have injected its header —
                    // assert the bypass is real (not "plugin silently did nothing
                    // but the route happened to work").
                    let body = response.text().await.unwrap_or_default();
                    if body.to_lowercase().contains("x-ext-proc-req") {
                        TestResult::failed(
                            start.elapsed(),
                            "fail-open but processor header present (unexpected)".to_string(),
                        )
                    } else {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "fail-open bypassed, reached backend".to_string(),
                        )
                    }
                })
            },
        )
    }

    /// A malformed/empty verdict from the processor must honor fail-close
    /// (the basic route is failureModeAllow=false) and 500, not silently pass.
    fn test_bad_verdict_fail_closes() -> TestCase {
        TestCase::new(
            "bad_verdict_fail_closes",
            "a malformed processor verdict triggers fail-close (500), not silent pass-through",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/headers", ctx.http_url());
                    let response = match ctx
                        .http_client
                        .get(&url)
                        .header("host", BASIC_HOST)
                        .header("x-ext-proc-badverdict", "1")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    let status = response.status().as_u16();
                    if status != 500 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected 500 (fail-close on bad verdict), got {}", status),
                        );
                    }
                    let body = response.text().await.unwrap_or_default();
                    if body.contains("ext_proc unavailable") {
                        TestResult::passed_with_message(start.elapsed(), "bad verdict fail-closed (500)".to_string())
                    } else {
                        TestResult::failed(start.elapsed(), format!("500 body not from ext_proc: {}", body))
                    }
                })
            },
        )
    }

    /// STREAMED body mode: each request-body chunk is exchanged with the
    /// processor per chunk; the fake processor replaces the chunk, so the
    /// backend echo must show the replacement. The header phase runs on the
    /// same stream (x-ext-proc-req applied) — proving one request-scoped
    /// stream carried both phases.
    fn test_streamed_body_mutation() -> TestCase {
        TestCase::new(
            "streamed_body_mutation",
            "requestBodyMode STREAMED mutates body chunks in flight (upstream sees the replacement)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.http_url());
                    let response = match ctx
                        .http_client
                        .post(&url)
                        .header("host", STREAMED_HOST)
                        .body("original-client-payload")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    let status = response.status().as_u16();
                    if status != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                    }
                    let body = response.text().await.unwrap_or_default();
                    if !body.contains("processed-by-ext-proc") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "backend did not receive the mutated body: {}",
                                &body[..body.len().min(400)]
                            ),
                        );
                    }
                    if body.contains("original-client-payload") {
                        return TestResult::failed(
                            start.elapsed(),
                            "original chunk bytes leaked through the streamed mutation".to_string(),
                        );
                    }
                    TestResult::passed_with_message(start.elapsed(), "streamed chunk mutated in flight".to_string())
                })
            },
        )
    }

    /// STREAMED mid-body rejection: the processor answers the first body chunk
    /// with an ImmediateResponse; the client must observe that status instead
    /// of a proxied response.
    fn test_streamed_immediate_response_mid_body() -> TestCase {
        TestCase::new(
            "streamed_immediate_response_mid_body",
            "requestBodyMode STREAMED terminates mid-body on an ImmediateResponse verdict",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.http_url());
                    let response = match ctx
                        .http_client
                        .post(&url)
                        .header("host", STREAMED_HOST)
                        .header("x-ext-proc-test-mode", "body-block")
                        .body("payload-to-block")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    let status = response.status().as_u16();
                    if status != 403 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected 403 from mid-body immediate response, got {}", status),
                        );
                    }
                    // The immediate response's header mutation must reach the
                    // client on the mid-body terminate path too.
                    let denied_header_ok = response
                        .headers()
                        .get("x-ext-proc-denied")
                        .and_then(|v| v.to_str().ok())
                        .map(|v| v == "1")
                        .unwrap_or(false);
                    let body = response.text().await.unwrap_or_default();
                    if body.contains("blocked by ext_proc") && denied_header_ok {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "mid-body 403 + custom header delivered".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("403 unexpected: denied_header_ok={} body={}", denied_header_ok, body),
                        )
                    }
                })
            },
        )
    }

    /// STREAMED multi-chunk upload: a channel-backed streaming body forces
    /// several transport chunks; the processor replaces each one, so the
    /// backend echo must contain only replacement markers.
    fn test_streamed_multi_chunk_mutation() -> TestCase {
        TestCase::new(
            "streamed_multi_chunk_mutation",
            "requestBodyMode STREAMED exchanges every chunk of a multi-chunk upload",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(1);
                    let sender = tokio::spawn(async move {
                        for index in 0u8..3 {
                            if tx.send(Ok(bytes::Bytes::from(vec![b'a' + index; 64]))).await.is_err() {
                                return;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        }
                    });
                    let body = reqwest::Body::wrap_stream(tokio_stream::wrappers::ReceiverStream::new(rx));
                    let url = format!("{}/echo", ctx.http_url());
                    let response = match ctx
                        .http_client
                        .post(&url)
                        .header("host", STREAMED_HOST)
                        .body(body)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    let _ = sender.await;
                    let status = response.status().as_u16();
                    if status != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                    }
                    let text = response.text().await.unwrap_or_default();
                    let replaced = text.matches("processed-by-ext-proc").count();
                    if replaced < 2 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "expected several replaced chunks, saw {}: {}",
                                replaced,
                                &text[..text.len().min(300)]
                            ),
                        );
                    }
                    if text.contains("aaaa") {
                        return TestResult::failed(start.elapsed(), "original chunk bytes leaked".to_string());
                    }
                    TestResult::passed_with_message(start.elapsed(), format!("{} chunks replaced in flight", replaced))
                })
            },
        )
    }

    /// STREAMED + bodyless request + a CONTINUE_AND_REPLACE headers verdict:
    /// the degenerate header-only path must reject it per the failure policy
    /// (fail-close 500), never silently ignore it.
    fn test_streamed_degenerate_continue_and_replace_fail_closes() -> TestCase {
        TestCase::new(
            "streamed_degenerate_continue_and_replace_fail_closes",
            "CONTINUE_AND_REPLACE on a bodyless streamed request fail-closes (500)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/headers", ctx.http_url());
                    let response = match ctx
                        .http_client
                        .get(&url)
                        .header("host", STREAMED_HOST)
                        .header("x-ext-proc-test-mode", "continue-and-replace")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    let status = response.status().as_u16();
                    if status != 500 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected 500 (CR rejected in streamed mode), got {}", status),
                        );
                    }
                    let body = response.text().await.unwrap_or_default();
                    if body.contains("ext_proc unavailable") {
                        TestResult::passed_with_message(start.elapsed(), "CR rejected fail-close".to_string())
                    } else {
                        TestResult::failed(start.elapsed(), format!("500 body not from ext_proc: {}", body))
                    }
                })
            },
        )
    }

    /// STREAMED + dead processor + failureModeAllow=true: the header phase
    /// fails open, no handler is installed, and the original body reaches the
    /// backend unmodified.
    fn test_streamed_fail_open_passes_original_body() -> TestCase {
        TestCase::new(
            "streamed_fail_open_passes_original_body",
            "streamed mode with a dead processor fails open and forwards the original body",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.http_url());
                    let response = match ctx
                        .http_client
                        .post(&url)
                        .header("host", STREAMED_FAILOPEN_HOST)
                        .body("untouched-payload")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    let status = response.status().as_u16();
                    if status != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                    }
                    let body = response.text().await.unwrap_or_default();
                    if body.contains("untouched-payload") {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "fail-open forwarded original body".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("original body missing at backend: {}", &body[..body.len().min(300)]),
                        )
                    }
                })
            },
        )
    }

    /// Coexistence: ForwardAuth + ExtProc on the same request chain both apply.
    fn test_coexist_with_forward_auth() -> TestCase {
        TestCase::new(
            "coexist_with_forward_auth",
            "ForwardAuth and ExtProc on one route both apply (no interference)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/headers", ctx.http_url());
                    let response = match ctx
                        .http_client
                        .get(&url)
                        .header("host", COEXIST_HOST)
                        .header("Authorization", "Bearer valid-token")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    let status = response.status().as_u16();
                    if status != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                    }
                    let body = response.text().await.unwrap_or_default();
                    let body_lower = body.to_lowercase();
                    // ForwardAuth copied X-User-ID; ExtProc added x-ext-proc-req.
                    let auth_ok = body_lower.contains("x-user-id") && body_lower.contains("user-123");
                    let ext_proc_ok = body_lower.contains("x-ext-proc-req") && body_lower.contains("seen");
                    if auth_ok && ext_proc_ok {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "ForwardAuth + ExtProc both applied".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "auth_ok={} ext_proc_ok={}; body: {}",
                                auth_ok,
                                ext_proc_ok,
                                &body[..body.len().min(400)]
                            ),
                        )
                    }
                })
            },
        )
    }
}

impl TestSuite for ExtProcTestSuite {
    fn name(&self) -> &str {
        "ExtProc"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_basic_request_and_response_headers(),
            Self::test_tls_request_and_response_headers(),
            Self::test_tls_noverify_request_and_response_headers(),
            Self::test_immediate_response_403(),
            Self::test_fail_close_returns_500(),
            Self::test_fail_open_passes(),
            Self::test_bad_verdict_fail_closes(),
            Self::test_streamed_body_mutation(),
            Self::test_streamed_immediate_response_mid_body(),
            Self::test_streamed_multi_chunk_mutation(),
            Self::test_streamed_degenerate_continue_and_replace_fail_closes(),
            Self::test_streamed_fail_open_passes_original_body(),
            Self::test_coexist_with_forward_auth(),
        ]
    }
}
