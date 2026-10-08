// DSL request-body buffering integration tests.
//
// Configuration lives in the dedicated EdgionPlugins/DslBody suite so a
// Gateway without the body-buffering capability skips it before loading YAML.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use sha2::{Digest, Sha256};
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const BODY_SIZE: usize = 256 * 1024;
const DIGEST_CACHE_BODY_SIZE: usize = 1024 * 1024;
const BODY_SHA256: &str = "d509bff642a353f88582e8a846ecae041c333b79c57a7a24ff310fbdb7e914e9";

pub struct DslBodyTestSuite;

fn gateway_addr(ctx: &TestContext, port: u16) -> Result<SocketAddr, String> {
    (ctx.target_host.as_str(), port)
        .to_socket_addrs()
        .map_err(|error| format!("Gateway address resolution failed: {error}"))?
        .next()
        .ok_or_else(|| format!("Gateway host {} resolved to no addresses", ctx.target_host))
}

impl DslBodyTestSuite {
    async fn post_echo(ctx: &TestContext, mode: &str, body: Vec<u8>) -> Result<(u16, String), String> {
        let url = format!("http://{}:31180/echo", ctx.target_host);
        let response = ctx
            .http_client
            .post(url)
            .header("host", "dsl-req-body-read.example.com")
            .header("content-type", "application/octet-stream")
            .header("x-body-mode", mode)
            .body(body)
            .send()
            .await
            .map_err(|error| format!("Request failed: {error}"))?;
        let status = response.status().as_u16();
        let response_body = response
            .text()
            .await
            .map_err(|error| format!("Response body read failed: {error}"))?;
        Ok((status, response_body))
    }

    fn echoed_body(response_body: &str) -> Result<&str, String> {
        response_body.split_once("Echo: ").map(|(_, body)| body).ok_or_else(|| {
            format!(
                "Response did not contain the echo marker ({} bytes)",
                response_body.len()
            )
        })
    }

    async fn post_body_http_call(
        client: &reqwest::Client,
        url: &str,
        mode: &str,
        request_body: Vec<u8>,
        expected_body: &[u8],
    ) -> Result<(u16, String, String), String> {
        let expected_digest = hex::encode(Sha256::digest(expected_body));
        let response = client
            .post(url)
            .header("host", "dsl-body-http-call.example.com")
            .header("content-type", "application/octet-stream")
            .header("x-body-mode", mode)
            .header("x-expected-body-sha256", expected_digest)
            .header("x-expected-body-length", expected_body.len().to_string())
            .body(request_body)
            .send()
            .await
            .map_err(|error| format!("Body http.call request failed: {error}"))?;
        let status = response.status().as_u16();
        let marker = response
            .headers()
            .get("x-body-call")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let response_body = response
            .text()
            .await
            .map_err(|error| format!("Body http.call response read failed: {error}"))?;
        Ok((status, marker, response_body))
    }

    fn test_late_body_read_requires_explicit_prebuffer() -> TestCase {
        TestCase::new(
            "dsl_late_body_read_requires_explicit_prebuffer",
            "A response-stage DSL reads only a request body cached explicitly during the request stage",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/echo", ctx.target_host);
                    let buffered = ctx
                        .http_client
                        .post(&url)
                        .header("host", "dsl-late-body-buffered.example.com")
                        .header("X-Capture-Body", "yes")
                        .body("late-body")
                        .send()
                        .await;
                    let buffered = match buffered {
                        Ok(response) => response,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("Buffered request failed: {error}"));
                        }
                    };
                    let buffered_status = buffered.status().as_u16();
                    let buffered_marker = buffered
                        .headers()
                        .get("x-late-body")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();

                    let unbuffered = ctx
                        .http_client
                        .post(url)
                        .header("host", "dsl-late-body-unbuffered.example.com")
                        .body("late-body")
                        .send()
                        .await;
                    let (unbuffered_status, unbuffered_body) = match unbuffered {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            let body = response.text().await.unwrap_or_default();
                            (status, body)
                        }
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("Unbuffered request failed: {error}"));
                        }
                    };

                    if buffered_status == 200
                        && buffered_marker == "matched"
                        && unbuffered_status == 500
                        && unbuffered_body == "Request body processing failed"
                    {
                        TestResult::passed(start.elapsed())
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected buffered=200/matched and unbuffered=500 with the generic body, got buffered={buffered_status}/{buffered_marker:?}, unbuffered={unbuffered_status}/{unbuffered_body:?}"
                            ),
                        )
                    }
                })
            },
        )
    }

    fn test_late_body_read_respects_body_conditions() -> TestCase {
        TestCase::new(
            "dsl_late_body_read_respects_body_conditions",
            "A false RequestBodyBuffer body condition leaves no cache for a late reader",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match ctx
                        .http_client
                        .post(format!("http://{}:31180/echo", ctx.target_host))
                        .header("host", "dsl-late-body-buffered.example.com")
                        .body("late-body")
                        .send()
                        .await
                    {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            let body = response.text().await.unwrap_or_default();
                            if status == 500 && body == "Request body processing failed" {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected skipped capture to fail the late reader with generic 500, got {status}: {body:?}"
                                    ),
                                )
                            }
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    fn test_body_read_methods_preserve_replay() -> TestCase {
        TestCase::new(
            "dsl_body_read_methods_preserve_replay",
            "String-like predicates, ranges, full reads, equality, and hashing inspect without changing replay",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let request_body = "alpha-π-omega".as_bytes().to_vec();
                    let (status, response_body) = match Self::post_echo(&ctx, "inspect", request_body.clone()).await {
                        Ok(result) => result,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let echoed = match Self::echoed_body(&response_body) {
                        Ok(body) => body,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if status == 200 && echoed.as_bytes() == request_body {
                        TestResult::passed(start.elapsed())
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("Expected exact inspected replay, got status={status}, echoed={echoed:?}"),
                        )
                    }
                })
            },
        )
    }

    fn test_body_definite_json_selection() -> TestCase {
        TestCase::new(
            "dsl_body_definite_json_selection",
            "Definite JSON paths select typed values without changing upstream replay",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let request_body = br#"{"users":[{"name":"old"},{"name":"alice"}],"enabled":true}"#.to_vec();
                    let (status, response_body) = match Self::post_echo(&ctx, "json", request_body.clone()).await {
                        Ok(result) => result,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let echoed = match Self::echoed_body(&response_body) {
                        Ok(body) => body,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if status == 200 && echoed.as_bytes() == request_body {
                        TestResult::passed(start.elapsed())
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("Expected exact JSON replay, got status={status}, echoed={echoed:?}"),
                        )
                    }
                })
            },
        )
    }

    fn test_body_json_multi_select_uses_one_scan_for_memory_and_spill() -> TestCase {
        TestCase::new(
            "dsl_body_json_multi_select_one_scan",
            "Static multi-select reads typed values and aliases with one scan from memory or spill storage",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/echo", ctx.target_host);
                    let small = br#"{"user":{"id":7,"name":"alice"},"enabled":true,"nullValue":null}"#.to_vec();
                    let large = format!(
                        r#"{{"user":{{"id":7,"name":"alice"}},"enabled":true,"nullValue":null,"padding":"{}"}}"#,
                        "x".repeat(BODY_SIZE)
                    )
                    .into_bytes();

                    for request_body in [small, large] {
                        let response = match ctx
                            .http_client
                            .post(&url)
                            .header("host", "dsl-json-select.example.com")
                            .header("content-type", "application/json")
                            .body(request_body.clone())
                            .send()
                            .await
                        {
                            Ok(response) => response,
                            Err(error) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("JSON multi-select request failed: {error}"),
                                );
                            }
                        };
                        let status = response.status().as_u16();
                        let response_body = response.text().await.unwrap_or_default();
                        let echoed = match Self::echoed_body(&response_body) {
                            Ok(body) => body,
                            Err(error) => return TestResult::failed(start.elapsed(), error),
                        };
                        if status != 200 || echoed.as_bytes() != request_body {
                            return TestResult::failed(
                                start.elapsed(),
                                format!(
                                    "Expected exact JSON multi-select replay, got status={status}, echoed_bytes={}",
                                    echoed.len()
                                ),
                            );
                        }
                    }
                    TestResult::passed(start.elapsed())
                })
            },
        )
    }

    fn test_body_digest_cache_uses_one_scan_for_memory_and_spill() -> TestCase {
        TestCase::new(
            "dsl_body_digest_cache_one_scan",
            "Repeated SHA-256 calls share one immutable-snapshot scan in memory or spill storage",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/echo", ctx.target_host);
                    for request_body in [b"small digest body".to_vec(), vec![b'x'; DIGEST_CACHE_BODY_SIZE]] {
                        let digest = hex::encode(Sha256::digest(&request_body));
                        let response = match ctx
                            .http_client
                            .post(&url)
                            .header("host", "dsl-digest-cache.example.com")
                            .header("content-type", "application/octet-stream")
                            .header("x-expected-body-sha256", digest)
                            .body(request_body.clone())
                            .send()
                            .await
                        {
                            Ok(response) => response,
                            Err(error) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Digest-cache request failed: {error}"),
                                );
                            }
                        };
                        let status = response.status().as_u16();
                        let response_body = response.text().await.unwrap_or_default();
                        let echoed = match Self::echoed_body(&response_body) {
                            Ok(body) => body,
                            Err(error) => return TestResult::failed(start.elapsed(), error),
                        };
                        if status != 200 || echoed.as_bytes() != request_body {
                            return TestResult::failed(
                                start.elapsed(),
                                format!(
                                    "Expected exact digest-cache replay, got status={status}, echoed_bytes={}",
                                    echoed.len()
                                ),
                            );
                        }
                    }
                    TestResult::passed(start.elapsed())
                })
            },
        )
    }

    fn test_body_md5_and_hmac_for_memory_and_spill() -> TestCase {
        TestCase::new(
            "dsl_body_md5_and_hmac_memory_and_spill",
            "MD5 and HMAC-SHA-256 scan raw memory or spill snapshots and preserve exact replay",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/echo", ctx.target_host);
                    for request_body in [b"small MAC body".to_vec(), vec![b'm'; DIGEST_CACHE_BODY_SIZE]] {
                        let response = match ctx
                            .http_client
                            .post(&url)
                            .header("host", "dsl-body-mac.example.com")
                            .header("content-type", "application/octet-stream")
                            .body(request_body.clone())
                            .send()
                            .await
                        {
                            Ok(response) => response,
                            Err(error) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Body digest/MAC request failed: {error}"),
                                );
                            }
                        };
                        let status = response.status().as_u16();
                        let response_body = response.text().await.unwrap_or_default();
                        let echoed = match Self::echoed_body(&response_body) {
                            Ok(body) => body,
                            Err(error) => return TestResult::failed(start.elapsed(), error),
                        };
                        if status != 200 || echoed.as_bytes() != request_body {
                            return TestResult::failed(
                                start.elapsed(),
                                format!(
                                    "Expected exact digest/MAC replay, got status={status}, echoed_bytes={}",
                                    echoed.len()
                                ),
                            );
                        }
                    }
                    TestResult::passed(start.elapsed())
                })
            },
        )
    }

    fn test_body_mutations_replay_latest_snapshot() -> TestCase {
        TestCase::new(
            "dsl_body_mutations_replay_latest_snapshot",
            "Literal replacement, prepend, append, and byte-range removal compose transactionally",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let (status, response_body) = match Self::post_echo(&ctx, "mutate", b"cat dog dog".to_vec()).await {
                        Ok(result) => result,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let echoed = match Self::echoed_body(&response_body) {
                        Ok(body) => body,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if status == 200 && echoed == "[fox fox]" {
                        TestResult::passed(start.elapsed())
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("Expected mutated replay, got status={status}, echoed={echoed:?}"),
                        )
                    }
                })
            },
        )
    }

    fn test_bodyless_mutation_installs_replay_owner() -> TestCase {
        TestCase::new(
            "dsl_bodyless_mutation_installs_replay_owner",
            "Appending to an initially empty Body creates the upstream replay body",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let (status, response_body) = match Self::post_echo(&ctx, "bodyless-mutate", Vec::new()).await {
                        Ok(result) => result,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let echoed = match Self::echoed_body(&response_body) {
                        Ok(body) => body,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if status == 200 && echoed == "seed" {
                        TestResult::passed(start.elapsed())
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("Expected bodyless mutation replay, got status={status}, echoed={echoed:?}"),
                        )
                    }
                })
            },
        )
    }

    fn test_spilled_body_mutations_preserve_source_ranges() -> TestCase {
        TestCase::new(
            "dsl_spilled_body_mutations_preserve_source_ranges",
            "Range inspection and mutations replay the latest disk-backed snapshot exactly",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let request_body = vec![b'x'; BODY_SIZE];
                    let (status, response_body) = match Self::post_echo(&ctx, "spill-mutate", request_body).await {
                        Ok(result) => result,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let echoed = match Self::echoed_body(&response_body) {
                        Ok(body) => body,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let mut expected = Vec::with_capacity(BODY_SIZE + 1);
                    expected.push(b'[');
                    expected.extend(std::iter::repeat_n(b'x', BODY_SIZE - 1));
                    expected.push(b']');
                    if status == 200 && echoed.as_bytes() == expected {
                        TestResult::passed(start.elapsed())
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected {} mutated spill bytes, got status={status}, echoed_bytes={}",
                                expected.len(),
                                echoed.len()
                            ),
                        )
                    }
                })
            },
        )
    }

    fn test_spilled_body_predicates_cross_scan_boundary() -> TestCase {
        TestCase::new(
            "dsl_spilled_body_predicates_cross_scan_boundary",
            "Substring and streaming regex matches cross an internal disk-scan boundary",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let mut request_body = Vec::with_capacity(BODY_SIZE);
                    request_body.extend(std::iter::repeat_n(b'x', 64 * 1024 - 3));
                    request_body.extend_from_slice(b"needle");
                    request_body.resize(BODY_SIZE, b'x');
                    let (status, response_body) = match Self::post_echo(&ctx, "spill-scan", request_body.clone()).await
                    {
                        Ok(result) => result,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let echoed = match Self::echoed_body(&response_body) {
                        Ok(body) => body,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if status == 200 && echoed.as_bytes() == request_body {
                        TestResult::passed(start.elapsed())
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected exact spilled scan replay, got status={status}, echoed_bytes={}",
                                echoed.len()
                            ),
                        )
                    }
                })
            },
        )
    }

    fn test_spilled_body_definite_json_selection() -> TestCase {
        TestCase::new(
            "dsl_spilled_body_definite_json_selection",
            "Definite JSONPath selects a bounded object from a disk-backed body",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let padding = "x".repeat(BODY_SIZE);
                    let request_body = format!(r#"{{"padding":"{padding}","selected":{{"n":1.0}}}}"#).into_bytes();
                    let (status, response_body) = match Self::post_echo(&ctx, "spill-json", request_body.clone()).await
                    {
                        Ok(result) => result,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let echoed = match Self::echoed_body(&response_body) {
                        Ok(body) => body,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if status == 200 && echoed.as_bytes() == request_body {
                        TestResult::passed(start.elapsed())
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected exact spilled JSON replay, got status={status}, echoed_bytes={}",
                                echoed.len()
                            ),
                        )
                    }
                })
            },
        )
    }

    async fn raw_chunked_exchange(target_host: &str, host: &str, chunks: &[&[u8]]) -> Result<String, String> {
        let mut stream = TcpStream::connect(format!("{target_host}:31180"))
            .await
            .map_err(|error| format!("TCP connect failed: {error}"))?;
        let headers =
            format!("POST /echo HTTP/1.1\r\nHost: {host}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n");
        stream
            .write_all(headers.as_bytes())
            .await
            .map_err(|error| format!("Header write failed: {error}"))?;
        for chunk in chunks {
            stream
                .write_all(format!("{:X}\r\n", chunk.len()).as_bytes())
                .await
                .map_err(|error| format!("Chunk prefix write failed: {error}"))?;
            stream
                .write_all(chunk)
                .await
                .map_err(|error| format!("Chunk write failed: {error}"))?;
            stream
                .write_all(b"\r\n")
                .await
                .map_err(|error| format!("Chunk suffix write failed: {error}"))?;
        }
        stream
            .write_all(b"0\r\n\r\n")
            .await
            .map_err(|error| format!("Chunk terminator write failed: {error}"))?;
        let mut response = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(4), stream.read_to_end(&mut response))
            .await
            .map_err(|_| "Timed out waiting for chunked response".to_string())?
            .map_err(|error| format!("Response read failed: {error}"))?;
        Ok(String::from_utf8_lossy(&response).into_owned())
    }

    async fn raw_expect_exchange(target_host: &str, host: &str, body: &[u8]) -> Result<(String, String), String> {
        let address = format!("{target_host}:31180");
        let mut stream = TcpStream::connect(&address)
            .await
            .map_err(|error| format!("TCP connect failed: {error}"))?;
        let headers = format!(
            "POST /echo HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream
            .write_all(headers.as_bytes())
            .await
            .map_err(|error| format!("Header write failed: {error}"))?;

        let mut interim = Vec::new();
        let mut byte = [0u8; 1];
        while !interim.ends_with(b"\r\n\r\n") {
            match stream.read(&mut byte).await {
                Ok(0) => return Err("Connection closed before 100 Continue".to_string()),
                Ok(_) => interim.push(byte[0]),
                Err(error) => return Err(format!("Interim response read failed: {error}")),
            }
        }
        stream
            .write_all(body)
            .await
            .map_err(|error| format!("Body write failed: {error}"))?;
        let mut final_response = Vec::new();
        stream
            .read_to_end(&mut final_response)
            .await
            .map_err(|error| format!("Final response read failed: {error}"))?;
        Ok((
            String::from_utf8_lossy(&interim).into_owned(),
            String::from_utf8_lossy(&final_response).into_owned(),
        ))
    }

    fn test_expect_continue_single_interim_then_success() -> TestCase {
        TestCase::new(
            "body_expect_continue_single_interim_then_success",
            "A real HTTP/1.1 client receives one 100 Continue and an exact upstream replay",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let (interim, final_response) = match Self::raw_expect_exchange(
                        &ctx.target_host,
                        "expect-body-pass.example.com",
                        b"hello world",
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let all = format!("{interim}{final_response}");
                    if all.matches(" 100 ").count() == 1
                        && final_response.contains(" 200 ")
                        && final_response.contains("Echo: hello world")
                    {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "Exactly one 100 Continue preceded a successful exact replay".to_string(),
                        )
                    } else {
                        TestResult::failed(start.elapsed(), format!("Unexpected response: {all:?}"))
                    }
                })
            },
        )
    }

    fn test_expect_continue_single_interim_then_reject() -> TestCase {
        TestCase::new(
            "body_expect_continue_single_interim_then_reject",
            "A real HTTP/1.1 client receives one 100 Continue before a body-policy rejection",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let (interim, final_response) =
                        match Self::raw_expect_exchange(&ctx.target_host, "expect-body.example.com", b"hello world")
                            .await
                        {
                            Ok(result) => result,
                            Err(error) => return TestResult::failed(start.elapsed(), error),
                        };
                    if !interim.starts_with("HTTP/1.1 100") {
                        return TestResult::failed(start.elapsed(), format!("Expected 100 Continue, got {interim:?}"));
                    }
                    let all = format!("{interim}{final_response}");
                    let continue_count = all.matches(" 100 ").count();
                    if continue_count == 1 && all.contains(" 413 ") {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "Exactly one 100 Continue preceded the final 413 rejection".to_string(),
                        )
                    } else {
                        TestResult::failed(start.elapsed(), format!("Expected one 100 and final 413, got {all:?}"))
                    }
                })
            },
        )
    }

    fn test_chunked_unknown_length_rejected_at_limit() -> TestCase {
        TestCase::new(
            "body_chunked_unknown_length_rejected_at_limit",
            "A chunked request without Content-Length is rejected when it crosses the plugin body limit",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let response = match Self::raw_chunked_exchange(
                        &ctx.target_host,
                        "expect-body.example.com",
                        &[b"hel", b"lo ", b"world"],
                    )
                    .await
                    {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if response.contains(" 413 ") {
                        TestResult::passed(start.elapsed())
                    } else {
                        TestResult::failed(start.elapsed(), format!("Expected final 413, got {response:?}"))
                    }
                })
            },
        )
    }

    fn test_request_body_spill_and_replay() -> TestCase {
        TestCase::new(
            "dsl_request_body_spill_and_replay",
            "DSL inspects a 256 KiB body and upstream receives the exact replay",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let request_body = vec![b'x'; BODY_SIZE];
                    let url = format!("http://{}:31180/echo", ctx.target_host);
                    let response = ctx
                        .http_client
                        .post(url)
                        .header("host", "dsl-req-body-read.example.com")
                        .header("content-type", "application/octet-stream")
                        .body(request_body.clone())
                        .send()
                        .await;

                    let response = match response {
                        Ok(response) => response,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("Request failed: {error}"));
                        }
                    };
                    let status = response.status().as_u16();
                    let response_body = response.text().await.unwrap_or_default();
                    if status != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected 200, got {status} with {} response bytes", response_body.len()),
                        );
                    }

                    let Some((_, echoed_body)) = response_body.split_once("Echo: ") else {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Response did not contain the echo marker ({} bytes)",
                                response_body.len()
                            ),
                        );
                    };
                    if echoed_body.as_bytes() == request_body {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!("DSL inspected and upstream echoed all {BODY_SIZE} bytes"),
                        )
                    } else {
                        let expected_digest = hex::encode(Sha256::digest(&request_body));
                        let actual_digest = hex::encode(Sha256::digest(echoed_body.as_bytes()));
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Replay mismatch: expected {} bytes (sha256={expected_digest}), got {} bytes (sha256={actual_digest})",
                                request_body.len(),
                                echoed_body.len()
                            ),
                        )
                    }
                })
            },
        )
    }

    fn test_h2_request_body_spill_and_replay() -> TestCase {
        TestCase::new(
            "dsl_h2_request_body_spill_and_replay",
            "HTTP/2 body buffering inspects and replays a 256 KiB body",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let request_body = vec![b'x'; BODY_SIZE];
                    let gateway = match gateway_addr(&ctx, 31180) {
                        Ok(address) => address,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), error);
                        }
                    };
                    let client = match reqwest::Client::builder()
                        .http2_prior_knowledge()
                        .no_proxy()
                        .resolve("dsl-req-body-read.example.com", gateway)
                        .build()
                    {
                        Ok(client) => client,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("H2 client build failed: {error}"));
                        }
                    };
                    let response = client
                        .post("http://dsl-req-body-read.example.com:31180/echo")
                        .body(request_body.clone())
                        .send()
                        .await;
                    match response {
                        Ok(response) if response.status().as_u16() == 200 => {
                            let text = response.text().await.unwrap_or_default();
                            if text
                                .split_once("Echo: ")
                                .is_some_and(|(_, echoed)| echoed.as_bytes() == request_body)
                            {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), "HTTP/2 replay mismatch".to_string())
                            }
                        }
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Expected 200, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("HTTP/2 request failed: {error}")),
                    }
                })
            },
        )
    }

    fn test_disk_body_replays_after_upstream_retry() -> TestCase {
        TestCase::new(
            "dsl_disk_body_replays_after_upstream_retry",
            "A retrying upstream receives the identical 256 KiB disk-backed body twice",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let request_body = vec![b'x'; BODY_SIZE];
                    let test_id = format!("dsl-body-retry-{}", uuid::Uuid::new_v4());
                    let url = format!("http://{}:31180/retry-sequence/{test_id}/503,200", ctx.target_host);
                    let response = ctx
                        .http_client
                        .post(url)
                        .header("host", "dsl-req-body-read.example.com")
                        .header("content-type", "application/octet-stream")
                        .header("x-expected-body-sha256", BODY_SHA256)
                        .body(request_body)
                        .send()
                        .await;

                    let response = match response {
                        Ok(response) => response,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("Request failed: {error}"));
                        }
                    };
                    let status = response.status().as_u16();
                    let attempt = response
                        .headers()
                        .get("x-attempt-number")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    let response_body = response.text().await.unwrap_or_default();
                    if status == 200 && attempt == "2" {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!("Both upstream attempts received the exact {BODY_SIZE}-byte body"),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected retry attempt 2 with status 200, got attempt={attempt:?}, status={status}, response_bytes={}",
                                response_body.len()
                            ),
                        )
                    }
                })
            },
        )
    }

    fn test_body_http_call_memory_mutation_and_empty_framing() -> TestCase {
        TestCase::new(
            "dsl_body_http_call_memory_mutation_and_empty_framing",
            "Opaque Body calls preserve raw snapshots, mutations, and explicit zero-length framing",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/echo", ctx.target_host);
                    let scenarios = [
                        ("http-call", b"raw-body".to_vec(), b"raw-body".to_vec()),
                        ("http-call-mutate", b"middle".to_vec(), b"[middle]".to_vec()),
                        ("http-call-redirect", b"no-replay".to_vec(), b"no-replay".to_vec()),
                        ("http-call-timeout", b"timeout-body".to_vec(), b"timeout-body".to_vec()),
                        ("http-call", Vec::new(), Vec::new()),
                    ];

                    for (mode, request_body, expected_body) in scenarios {
                        let (status, marker, response_body) =
                            match Self::post_body_http_call(&ctx.http_client, &url, mode, request_body, &expected_body)
                                .await
                            {
                                Ok(result) => result,
                                Err(error) => return TestResult::failed(start.elapsed(), error),
                            };
                        let echoed = match Self::echoed_body(&response_body) {
                            Ok(body) => body,
                            Err(error) => return TestResult::failed(start.elapsed(), error),
                        };
                        if status != 200 || marker != "ok" || echoed.as_bytes() != expected_body {
                            return TestResult::failed(
                                start.elapsed(),
                                format!(
                                    "Body http.call mismatch for mode={mode}: status={status}, marker={marker:?}, expected_bytes={}, echoed_bytes={}",
                                    expected_body.len(),
                                    echoed.len()
                                ),
                            );
                        }
                    }

                    TestResult::passed(start.elapsed())
                })
            },
        )
    }

    fn test_h2_spilled_body_http_call_streaming() -> TestCase {
        TestCase::new(
            "dsl_h2_spilled_body_http_call_streaming",
            "An HTTP/2 downstream request streams a 256 KiB spilled snapshot through http.call",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let gateway = match gateway_addr(&ctx, 31180) {
                        Ok(address) => address,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), error);
                        }
                    };
                    let client = match reqwest::Client::builder()
                        .http2_prior_knowledge()
                        .no_proxy()
                        .resolve("dsl-body-http-call.example.com", gateway)
                        .build()
                    {
                        Ok(client) => client,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("H2 client build failed: {error}"));
                        }
                    };
                    let request_body = vec![b'x'; BODY_SIZE];
                    let result = Self::post_body_http_call(
                        &client,
                        "http://dsl-body-http-call.example.com/echo",
                        "http-call",
                        request_body.clone(),
                        &request_body,
                    )
                    .await;
                    let (status, marker, response_body) = match result {
                        Ok(result) => result,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let echoed = match Self::echoed_body(&response_body) {
                        Ok(body) => body,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if status == 200 && marker == "ok" && echoed.as_bytes() == request_body {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!("Streamed and replayed all {BODY_SIZE} bytes"),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected streamed spill success, got status={status}, marker={marker:?}, echoed_bytes={}",
                                echoed.len()
                            ),
                        )
                    }
                })
            },
        )
    }

    fn test_body_http_call_preserves_invalid_utf8_bytes() -> TestCase {
        TestCase::new(
            "dsl_body_http_call_preserves_invalid_utf8_bytes",
            "Opaque Body upload preserves non-UTF-8 bytes without decoding",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let request_body = vec![b'a', 0xff, 0x00, b'z'];
                    let url = format!("http://{}:31180/body-digest", ctx.target_host);
                    let result = Self::post_body_http_call(
                        &ctx.http_client,
                        &url,
                        "http-call",
                        request_body.clone(),
                        &request_body,
                    )
                    .await;
                    let (status, marker, response_body) = match result {
                        Ok(result) => result,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };

                    if status == 200 && marker == "ok" && response_body == "body-digest" {
                        TestResult::passed(start.elapsed())
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected byte-preserving call success, got status={status}, marker={marker:?}, body={response_body:?}"
                            ),
                        )
                    }
                })
            },
        )
    }
}

impl TestSuite for DslBodyTestSuite {
    fn name(&self) -> &str {
        "DSL Request Body Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let mut cases = vec![
            Self::test_late_body_read_requires_explicit_prebuffer(),
            Self::test_late_body_read_respects_body_conditions(),
            Self::test_body_read_methods_preserve_replay(),
            Self::test_body_definite_json_selection(),
            Self::test_body_json_multi_select_uses_one_scan_for_memory_and_spill(),
            Self::test_body_digest_cache_uses_one_scan_for_memory_and_spill(),
            Self::test_body_md5_and_hmac_for_memory_and_spill(),
            Self::test_body_mutations_replay_latest_snapshot(),
            Self::test_bodyless_mutation_installs_replay_owner(),
            Self::test_spilled_body_mutations_preserve_source_ranges(),
            Self::test_spilled_body_predicates_cross_scan_boundary(),
            Self::test_spilled_body_definite_json_selection(),
            Self::test_expect_continue_single_interim_then_success(),
            Self::test_expect_continue_single_interim_then_reject(),
            Self::test_chunked_unknown_length_rejected_at_limit(),
            Self::test_request_body_spill_and_replay(),
            Self::test_h2_request_body_spill_and_replay(),
            Self::test_disk_body_replays_after_upstream_retry(),
            Self::test_body_http_call_memory_mutation_and_empty_framing(),
            Self::test_h2_spilled_body_http_call_streaming(),
            Self::test_body_http_call_preserves_invalid_utf8_bytes(),
        ];
        cases.extend(super::xml::cases());
        cases
    }
}
