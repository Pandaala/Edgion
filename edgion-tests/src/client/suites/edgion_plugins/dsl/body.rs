// DSL request-body buffering integration tests.
//
// Configuration lives in the dedicated EdgionPlugins/DslBody suite so a
// Gateway without the body-buffering capability skips it before loading YAML.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use sha2::{Digest, Sha256};
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const BODY_SIZE: usize = 256 * 1024;
const BODY_SHA256: &str = "d509bff642a353f88582e8a846ecae041c333b79c57a7a24ff310fbdb7e914e9";

pub struct DslBodyTestSuite;

impl DslBodyTestSuite {
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
                    let unbuffered_status = match unbuffered {
                        Ok(response) => response.status().as_u16(),
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("Unbuffered request failed: {error}"));
                        }
                    };

                    if buffered_status == 200 && buffered_marker == "matched" && unbuffered_status == 500 {
                        TestResult::passed(start.elapsed())
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected buffered=200/matched and unbuffered=500, got buffered={buffered_status}/{buffered_marker:?}, unbuffered={unbuffered_status}"
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
                    let gateway = match format!("{}:31180", ctx.target_host).parse() {
                        Ok(address) => address,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("Gateway address invalid: {error}"));
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
}

impl TestSuite for DslBodyTestSuite {
    fn name(&self) -> &str {
        "DSL Request Body Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_late_body_read_requires_explicit_prebuffer(),
            Self::test_expect_continue_single_interim_then_success(),
            Self::test_expect_continue_single_interim_then_reject(),
            Self::test_chunked_unknown_length_rejected_at_limit(),
            Self::test_request_body_spill_and_replay(),
            Self::test_h2_request_body_spill_and_replay(),
            Self::test_disk_body_replays_after_upstream_retry(),
        ]
    }
}
