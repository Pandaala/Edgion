use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use bytes::Bytes;
use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::future::poll_fn;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{sleep, Duration};

type HmacSha256 = Hmac<Sha256>;

pub struct BodyPluginsTestSuite;

fn echo_body(text: &str) -> Option<&str> {
    text.split_once("Echo: ").map(|(_, body)| body)
}

fn hmac_headers(host: &str, path: &str, body: &[u8]) -> (String, String, String) {
    let digest = format!("SHA-256={}", STANDARD.encode(Sha256::digest(body)));
    let date = Utc::now().to_rfc2822();
    let signing = format!("@request-target: post {path}\nhost: {host}\ndate: {date}\ndigest: {digest}");
    let mut mac = HmacSha256::new_from_slice(b"body-hmac-secret-key-must-be-at-least-32-bytes")
        .expect("static HMAC key is valid");
    mac.update(signing.as_bytes());
    let signature = STANDARD.encode(mac.finalize().into_bytes());
    let authorization = format!(
        "hmac username=\"body-user\", algorithm=\"hmac-sha256\", headers=\"@request-target host date digest\", signature=\"{signature}\""
    );
    (date, digest, authorization)
}

async fn wasm_request_body_counters(ctx: &TestContext) -> Result<(u64, u64, u64), String> {
    let response = ctx
        .http_client
        .get(format!("{}/headers", ctx.http_url()))
        .header("host", "wasm-body.example.com")
        .header("x-wasm-read-request-body", "1")
        .send()
        .await
        .map_err(|error| format!("Wasm counter request failed: {error}"))?;
    if response.status().as_u16() != 200 {
        return Err(format!("Wasm counter request returned {}", response.status()));
    }
    let value = response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| format!("Wasm counter response was not JSON: {error}"))?;
    let read = |name: &str| {
        value["headers"][name]
            .as_str()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| format!("Missing or invalid {name}: {}", value["headers"]))
    };
    Ok((
        read("x-wasm-requestbodybytes")?,
        read("x-wasm-requestbodycalls")?,
        read("x-wasm-requestbodyeos")?,
    ))
}

async fn raw_chunked_request(
    target_host: &str,
    host: &str,
    path: &str,
    content_type: &str,
    extra_headers: &[(&str, &str)],
    chunks: &[&[u8]],
) -> Result<String, String> {
    let mut stream = TcpStream::connect(format!("{target_host}:31180"))
        .await
        .map_err(|error| format!("TCP connect failed: {error}"))?;
    let mut headers = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: {content_type}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
    );
    let insertion = headers.len() - 2;
    let extra_headers = extra_headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect::<String>();
    headers.insert_str(insertion, &extra_headers);
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
        .map_err(|_| "Timed out waiting for chunked request response".to_string())?
        .map_err(|error| format!("Response read failed: {error}"))?;
    Ok(String::from_utf8_lossy(&response).into_owned())
}

async fn raw_unfinished_chunked_request(target_host: &str, host: &str, chunk: &[u8]) -> Result<String, String> {
    let mut stream = TcpStream::connect(format!("{target_host}:31180"))
        .await
        .map_err(|error| format!("TCP connect failed: {error}"))?;
    let headers = format!(
        "POST /echo HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n"
    );
    stream
        .write_all(headers.as_bytes())
        .await
        .map_err(|error| format!("Header write failed: {error}"))?;
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

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut response))
        .await
        .map_err(|_| "Gateway did not reject and close an unfinished over-limit chunked request".to_string())?
        .map_err(|error| format!("Response read failed: {error}"))?;
    Ok(String::from_utf8_lossy(&response).into_owned())
}

async fn h2_unfinished_over_limit_request(target_host: &str) -> Result<(), String> {
    let stream = TcpStream::connect(format!("{target_host}:31180"))
        .await
        .map_err(|error| format!("TCP connect failed: {error}"))?;
    let (mut sender, connection) = h2::client::handshake(stream)
        .await
        .map_err(|error| format!("HTTP/2 handshake failed: {error}"))?;
    let connection_task = tokio::spawn(async move {
        let _ = connection.await;
    });

    let request = http::Request::builder()
        .method(http::Method::POST)
        .uri("http://body-dynamic.example.com:31180/echo")
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(())
        .map_err(|error| format!("Build HTTP/2 request failed: {error}"))?;
    let (response, mut request_body) = sender
        .send_request(request, false)
        .map_err(|error| format!("Send HTTP/2 request headers failed: {error}"))?;
    request_body
        .send_data(Bytes::from_static(b"{\"payload\":\"larger-than-cap\"}"), false)
        .map_err(|error| format!("Send over-limit HTTP/2 DATA failed: {error}"))?;

    let response = tokio::time::timeout(Duration::from_secs(2), response)
        .await
        .map_err(|_| "Gateway did not reject an unfinished over-limit HTTP/2 stream".to_string())?
        .map_err(|error| format!("Read HTTP/2 rejection failed: {error}"))?;
    if response.status() != http::StatusCode::PAYLOAD_TOO_LARGE {
        connection_task.abort();
        return Err(format!("Expected HTTP/2 413, got {}", response.status()));
    }

    let reset_reason = tokio::time::timeout(Duration::from_secs(2), poll_fn(|cx| request_body.poll_reset(cx)))
        .await
        .map_err(|_| "Gateway did not reset the unfinished HTTP/2 request stream".to_string())?
        .map_err(|error| format!("Read HTTP/2 stream reset failed: {error}"))?;
    if reset_reason != h2::Reason::NO_ERROR {
        connection_task.abort();
        return Err(format!("Expected HTTP/2 NO_ERROR reset, got {reset_reason:?}"));
    }

    sender = sender
        .ready()
        .await
        .map_err(|error| format!("HTTP/2 connection was closed with the rejected stream: {error}"))?;
    let follow_up = http::Request::builder()
        .method(http::Method::GET)
        .uri("http://unmatched-after-body-limit.example.com:31180/")
        .body(())
        .map_err(|error| format!("Build follow-up HTTP/2 request failed: {error}"))?;
    let (follow_up, _) = sender
        .send_request(follow_up, true)
        .map_err(|error| format!("Send follow-up HTTP/2 request failed: {error}"))?;
    tokio::time::timeout(Duration::from_secs(2), follow_up)
        .await
        .map_err(|_| "Follow-up HTTP/2 stream did not receive a response".to_string())?
        .map_err(|error| format!("Follow-up HTTP/2 stream failed: {error}"))?;
    connection_task.abort();
    Ok(())
}

async fn poll_mirrored_body(ctx: &TestContext, trace_id: &str) -> Result<String, String> {
    let k8s_mode = std::env::var("EDGION_TEST_K8S_MODE")
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);
    let hosts = if k8s_mode {
        std::env::var("EDGION_TEST_BACKEND_ENDPOINT_IPS")
            .ok()
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|host| !host.is_empty())
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .filter(|hosts| !hosts.is_empty())
            .unwrap_or_else(|| {
                vec![std::env::var("EDGION_TEST_MIRROR_BACKEND_HOST")
                    .unwrap_or_else(|_| "edgion-test-server.edgion-test.svc.cluster.local".to_string())]
            })
    } else {
        vec![ctx.target_host.clone()]
    };

    for attempt in 0..=20 {
        for host in &hosts {
            let response = ctx
                .http_client
                .get(format!("http://{host}:30001/mirror/query/{trace_id}"))
                .send()
                .await;
            if let Ok(response) = response {
                if let Ok(value) = response.json::<serde_json::Value>().await {
                    if let Some(capture) = value["captures"].as_array().and_then(|captures| {
                        captures
                            .iter()
                            .find(|capture| capture["headers"]["x-mirror"].as_str() == Some("true"))
                    }) {
                        return capture["body"]
                            .as_str()
                            .map(ToOwned::to_owned)
                            .ok_or_else(|| format!("Mirror capture has no string body: {capture}"));
                    }
                }
            }
        }
        if attempt < 20 {
            sleep(Duration::from_millis(100)).await;
        }
    }
    Err("Mirror capture was not observed within 2 seconds".to_string())
}

impl BodyPluginsTestSuite {
    fn hmac_body() -> TestCase {
        TestCase::new(
            "hmac_disk_body_digest",
            "HMAC validates the Digest of a disk-backed request body",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let body = vec![b'h'; 256 * 1024];
                    let (date, digest, authorization) = hmac_headers("hmac-body.example.com", "/echo", &body);
                    let response = ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", "hmac-body.example.com")
                        .header("date", date)
                        .header("digest", digest)
                        .header("authorization", authorization)
                        .body(body)
                        .send()
                        .await;
                    match response {
                        Ok(response) if response.status().as_u16() == 200 => TestResult::passed(start.elapsed()),
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Expected 200, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    fn hmac_rejects_tampered_body() -> TestCase {
        TestCase::new(
            "hmac_rejects_tampered_body",
            "HMAC rejects bytes that do not match the signed Digest header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let (date, digest, authorization) =
                        hmac_headers("hmac-body.example.com", "/echo", b"original-body");
                    let response = ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", "hmac-body.example.com")
                        .header("date", date)
                        .header("digest", digest)
                        .header("authorization", authorization)
                        .body("tampered-body")
                        .send()
                        .await;
                    match response {
                        Ok(response) if response.status().as_u16() == 401 => TestResult::passed(start.elapsed()),
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Expected 401, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    fn hmac_accepts_empty_body_digest() -> TestCase {
        TestCase::new(
            "hmac_accepts_empty_body_digest",
            "HMAC accepts the standard SHA-256 digest of an empty request body",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let (date, digest, authorization) = hmac_headers("hmac-body.example.com", "/echo", b"");
                    let response = ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", "hmac-body.example.com")
                        .header("date", date)
                        .header("digest", digest)
                        .header("authorization", authorization)
                        .send()
                        .await;
                    match response {
                        Ok(response) if response.status().as_u16() == 200 => TestResult::passed(start.elapsed()),
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Expected 200, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    fn forward_auth_body() -> TestCase {
        TestCase::new(
            "forward_auth_forwards_original_body",
            "ForwardAuth sends the exact original request body to the auth service",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let body = b"forward-auth-body".to_vec();
                    let expected = hex::encode(Sha256::digest(&body));
                    let response = ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", "forward-auth-body.example.com")
                        .header("authorization", "Bearer valid-token")
                        .header("x-auth-expected-body-sha256", expected)
                        .body(body)
                        .send()
                        .await;
                    match response {
                        Ok(response) if response.status().as_u16() == 200 => TestResult::passed(start.elapsed()),
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Expected 200, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    fn proxy_rewrite_json_body() -> TestCase {
        TestCase::new(
            "proxy_rewrite_json_body",
            "ProxyRewrite applies ordered JSON Patch operations",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let response = ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", "proxy-rewrite-body.example.com")
                        .header("content-type", "application/json")
                        .body(r#"{"name":"old","secret":"drop"}"#)
                        .send()
                        .await;
                    match response {
                        Ok(response) if response.status().as_u16() == 200 => {
                            let text = response.text().await.unwrap_or_default();
                            match echo_body(&text).and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok())
                            {
                                Some(value) if value == serde_json::json!({"name":"gateway"}) => {
                                    TestResult::passed(start.elapsed())
                                }
                                _ => TestResult::failed(start.elapsed(), format!("Unexpected echo: {text}")),
                            }
                        }
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Expected 200, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    fn proxy_rewrite_chunked_body_reframes_upstream() -> TestCase {
        TestCase::new(
            "proxy_rewrite_chunked_body_reframes_upstream",
            "A length-changing mutation removes downstream chunked framing before upstream replay",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let response = raw_chunked_request(
                        &ctx.target_host,
                        "proxy-rewrite-body.example.com",
                        "/echo",
                        "application/json",
                        &[],
                        &[b"{\"name\":\"", b"old\",\"secret\":\"drop\"}"],
                    )
                    .await;
                    match response {
                        Ok(response)
                            if response.contains(" 200 ") && response.contains(r#"Echo: {"name":"gateway"}"#) =>
                        {
                            TestResult::passed(start.elapsed())
                        }
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Unexpected chunked response: {response:?}"))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    fn proxy_rewrite_advanced_json_patch() -> TestCase {
        TestCase::new(
            "proxy_rewrite_advanced_json_patch",
            "ProxyRewrite applies all RFC 6902 operations and a string template in order",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let response = ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", "json-patch-advanced.example.com")
                        .header("content-type", "application/json")
                        .header("x-tenant", "tenant-a")
                        .body(
                            r#"{"version":1,"name":"old","secret":"drop","items":["head"],"profile":{"tier":"gold"}}"#,
                        )
                        .send()
                        .await;
                    match response {
                        Ok(response) if response.status().as_u16() == 200 => {
                            let text = response.text().await.unwrap_or_default();
                            let actual =
                                echo_body(&text).and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok());
                            let expected = serde_json::json!({
                                "version": 1,
                                "displayName": "gateway",
                                "items": ["head", "tail"],
                                "profile": {"tier": "gold"},
                                "profileSnapshot": {"tier": "gold"},
                                "tenant": "tenant-a"
                            });
                            if actual == Some(expected) {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("Unexpected echo: {text}"))
                            }
                        }
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Expected 200, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    fn proxy_rewrite_patch_failure_is_atomic() -> TestCase {
        TestCase::new(
            "proxy_rewrite_patch_failure_is_atomic",
            "A failed JSON Patch test rejects without forwarding a partial mutation",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let response = ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", "json-patch-atomic-failure.example.com")
                        .header("content-type", "application/json")
                        .body(r#"{"version":1,"name":"original"}"#)
                        .send()
                        .await;
                    match response {
                        Ok(response) if response.status().as_u16() == 400 => TestResult::passed(start.elapsed()),
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Expected 400, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    fn proxy_rewrite_spilled_json_body() -> TestCase {
        TestCase::new(
            "proxy_rewrite_spilled_json_body",
            "ProxyRewrite materializes a forced-spill body once and publishes the complete patch",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let padding = "x".repeat(256 * 1024);
                    let request = serde_json::json!({
                        "version": "client",
                        "discard": "remove",
                        "meta": {"source": "disk"},
                        "padding": padding,
                    });
                    let response = ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", "json-patch-spill.example.com")
                        .header("content-type", "application/json")
                        .body(request.to_string())
                        .send()
                        .await;
                    match response {
                        Ok(response) if response.status().as_u16() == 200 => {
                            let text = response.text().await.unwrap_or_default();
                            let actual =
                                echo_body(&text).and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok());
                            match actual {
                                Some(value)
                                    if value["version"] == "spilled"
                                        && value.get("discard").is_none()
                                        && value["meta"] == serde_json::json!({"source": "disk"})
                                        && value["metaCopy"] == serde_json::json!({"source": "disk"})
                                        && value["padding"].as_str().is_some_and(|value| value.len() == 256 * 1024) =>
                                {
                                    TestResult::passed(start.elapsed())
                                }
                                _ => TestResult::failed(start.elapsed(), format!("Unexpected echo: {text}")),
                            }
                        }
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Expected 200, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    fn form_to_json_memory_and_strict_errors() -> TestCase {
        TestCase::new(
            "form_to_json_memory_and_strict_errors",
            "FormJsonTransform decodes a memory-backed form and rejects malformed escapes",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let response = ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", "form-to-json.example.com")
                        .header("content-type", "application/x-www-form-urlencoded; charset=utf-8")
                        .body("b=two&a=one&a=three&plus=%2B+%7e")
                        .send()
                        .await;
                    let valid = match response {
                        Ok(response) if response.status().as_u16() == 200 => {
                            let text = response.text().await.unwrap_or_default();
                            echo_body(&text).and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok())
                                == Some(serde_json::json!({
                                    "a": ["one", "three"],
                                    "b": "two",
                                    "plus": "+ ~"
                                }))
                        }
                        _ => false,
                    };
                    if !valid {
                        return TestResult::failed(start.elapsed(), "Valid form was not transformed".to_string());
                    }

                    match ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", "form-to-json.example.com")
                        .header("content-type", "application/x-www-form-urlencoded")
                        .body("bad=%GG")
                        .send()
                        .await
                    {
                        Ok(response) if response.status().as_u16() == 400 => TestResult::passed(start.elapsed()),
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Expected 400, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    fn json_to_form_and_ordered_chains() -> TestCase {
        TestCase::new(
            "json_to_form_and_ordered_chains",
            "Both directions and ordered transform/rewrite chains observe the committed body",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let response = ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", "json-to-form.example.com")
                        .header("content-type", "application/json")
                        .body(r#"{"z":"*~+ ","a":["one",true,2]}"#)
                        .send()
                        .await;
                    let form_ok = match response {
                        Ok(response) if response.status().as_u16() == 200 => {
                            let text = response.text().await.unwrap_or_default();
                            echo_body(&text) == Some("a=one&a=true&a=2&z=*%7E%2B+")
                        }
                        _ => false,
                    };
                    if !form_ok {
                        return TestResult::failed(start.elapsed(), "JSON-to-form output mismatch".to_string());
                    }

                    for (host, content_type, body, expected) in [
                        (
                            "form-to-json.example.com",
                            "application/x-www-form-urlencoded",
                            "a=1",
                            "application/json",
                        ),
                        (
                            "json-to-form.example.com",
                            "application/json",
                            r#"{"a":"1"}"#,
                            "application/x-www-form-urlencoded",
                        ),
                    ] {
                        let response = ctx
                            .http_client
                            .request(reqwest::Method::GET, format!("{}/headers", ctx.http_url()))
                            .header("host", host)
                            .header("content-type", content_type)
                            .body(body)
                            .send()
                            .await;
                        let observed = match response {
                            Ok(response) if response.status().as_u16() == 200 => response
                                .json::<serde_json::Value>()
                                .await
                                .ok()
                                .and_then(|value| value["headers"]["content-type"].as_str().map(ToOwned::to_owned)),
                            _ => None,
                        };
                        if observed.as_deref() != Some(expected) {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("{host}: expected upstream content-type {expected}, got {observed:?}"),
                            );
                        }
                    }

                    let response = ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", "form-json-rewrite-chain.example.com")
                        .header("content-type", "application/x-www-form-urlencoded")
                        .body("other=value&name=client")
                        .send()
                        .await;
                    let rewrite_ok = match response {
                        Ok(response) if response.status().as_u16() == 200 => {
                            let text = response.text().await.unwrap_or_default();
                            echo_body(&text).and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok())
                                == Some(serde_json::json!({"name": "gateway", "other": "value"}))
                        }
                        _ => false,
                    };
                    if !rewrite_ok {
                        return TestResult::failed(start.elapsed(), "Transform/rewrite chain mismatch".to_string());
                    }

                    let response = ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", "form-json-roundtrip.example.com")
                        .header("content-type", "application/x-www-form-urlencoded")
                        .body("b=2&a=1&a=3")
                        .send()
                        .await;
                    match response {
                        Ok(response) if response.status().as_u16() == 200 => {
                            let text = response.text().await.unwrap_or_default();
                            if echo_body(&text) == Some("a=1&a=3&b=2") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("Unexpected round trip: {text}"))
                            }
                        }
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Expected 200, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    fn form_json_chain_limits_and_media() -> TestCase {
        TestCase::new(
            "form_json_chain_limits_and_media",
            "A later transform applies its own limit and same-direction chaining rejects changed media",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    for (host, expected) in [
                        ("form-json-small-later.example.com", 413),
                        ("form-json-same-direction.example.com", 415),
                    ] {
                        match ctx
                            .http_client
                            .post(format!("{}/echo", ctx.http_url()))
                            .header("host", host)
                            .header("content-type", "application/x-www-form-urlencoded")
                            .body("a=123456789")
                            .send()
                            .await
                        {
                            Ok(response) if response.status().as_u16() == expected => {}
                            Ok(response) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("{host}: expected {expected}, got {}", response.status()),
                                );
                            }
                            Err(error) => {
                                return TestResult::failed(start.elapsed(), format!("Request failed: {error}"));
                            }
                        }
                    }
                    TestResult::passed(start.elapsed())
                })
            },
        )
    }

    fn form_json_chunked_body_reframes_upstream() -> TestCase {
        TestCase::new(
            "form_json_chunked_body_reframes_upstream",
            "Form conversion removes downstream chunked framing before upstream replay",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let response = raw_chunked_request(
                        &ctx.target_host,
                        "form-to-json.example.com",
                        "/echo",
                        "application/x-www-form-urlencoded",
                        &[],
                        &[b"b=2&", b"a=1"],
                    )
                    .await;
                    match response {
                        Ok(response)
                            if response.contains(" 200 ") && response.contains(r#"Echo: {"a":"1","b":"2"}"#) =>
                        {
                            TestResult::passed(start.elapsed())
                        }
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Unexpected chunked response: {response:?}"))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    fn form_json_spilled_body() -> TestCase {
        TestCase::new(
            "form_json_spilled_body",
            "FormJsonTransform materializes a disk-backed form and publishes complete JSON",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let value = "x".repeat(2048);
                    let body = (0..128)
                        .map(|index| format!("k{index:03}={value}"))
                        .collect::<Vec<_>>()
                        .join("&");
                    let response = ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", "form-json-spill.example.com")
                        .header("content-type", "application/x-www-form-urlencoded")
                        .body(body)
                        .send()
                        .await;
                    match response {
                        Ok(response) if response.status().as_u16() == 200 => {
                            let text = response.text().await.unwrap_or_default();
                            let actual =
                                echo_body(&text).and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok());
                            if !actual
                                .as_ref()
                                .and_then(|value| value.as_object())
                                .is_some_and(|object| {
                                    object.len() == 128
                                        && object
                                            .values()
                                            .all(|value| value.as_str().is_some_and(|value| value.len() == 2048))
                                })
                            {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "Spilled form-to-JSON transform mismatch".to_string(),
                                );
                            }
                        }
                        Ok(response) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Expected 200, got {}", response.status()),
                            );
                        }
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("Request failed: {error}"));
                        }
                    }

                    let json = serde_json::Value::Object(
                        (0..128)
                            .map(|index| (format!("k{index:03}"), serde_json::Value::String(value.clone())))
                            .collect(),
                    );
                    let response = ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", "json-form-spill.example.com")
                        .header("content-type", "application/json")
                        .body(json.to_string())
                        .send()
                        .await;
                    match response {
                        Ok(response) if response.status().as_u16() == 200 => {
                            let text = response.text().await.unwrap_or_default();
                            let pairs = echo_body(&text)
                                .map(|body| body.split('&').collect::<Vec<_>>())
                                .unwrap_or_default();
                            if pairs.len() == 128
                                && pairs.first().is_some_and(|pair| pair.starts_with("k000="))
                                && pairs.last().is_some_and(|pair| pair.starts_with("k127="))
                                && pairs.iter().all(|pair| pair.len() == 5 + 2048)
                            {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "Spilled JSON-to-form transform mismatch".to_string(),
                                )
                            }
                        }
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Expected 200, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    fn form_json_retry_replays_committed_body() -> TestCase {
        TestCase::new(
            "form_json_retry_replays_committed_body",
            "A retry replays the transformed JSON body with final framing on every attempt",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let expected = br#"{"a":"1","b":"2"}"#;
                    let test_id = format!("form-json-{}", uuid::Uuid::new_v4());
                    let response = ctx
                        .http_client
                        .post(format!("{}/retry-sequence/{test_id}/503,200", ctx.http_url()))
                        .header("host", "form-json-retry.example.com")
                        .header("content-type", "application/x-www-form-urlencoded")
                        .header("x-expected-body-sha256", hex::encode(Sha256::digest(expected)))
                        .body("b=2&a=1")
                        .send()
                        .await;
                    match response {
                        Ok(response)
                            if response.status().as_u16() == 200
                                && response
                                    .headers()
                                    .get("x-attempt-number")
                                    .and_then(|value| value.to_str().ok())
                                    == Some("2") =>
                        {
                            TestResult::passed(start.elapsed())
                        }
                        Ok(response) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected successful attempt 2, got status={} attempt={:?}",
                                response.status(),
                                response.headers().get("x-attempt-number")
                            ),
                        ),
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    fn ext_proc_buffered_body() -> TestCase {
        TestCase::new(
            "ext_proc_buffered_body",
            "ExtProc receives and replaces a BUFFERED request body",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let response = ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", "ext-proc-body.example.com")
                        .body("original")
                        .send()
                        .await;
                    match response {
                        Ok(response) if response.status().as_u16() == 200 => {
                            let text = response.text().await.unwrap_or_default();
                            if echo_body(&text) == Some("processed-by-ext-proc") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("Unexpected echo: {text}"))
                            }
                        }
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Expected 200, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    async fn run_ext_proc_contract(
        ctx: TestContext,
        mode: &'static str,
        body: Option<&'static str>,
        expected_status: u16,
        expected_body: Option<&'static str>,
    ) -> TestResult {
        let start = Instant::now();
        let mut request = ctx
            .http_client
            .post(format!("{}/echo", ctx.http_url()))
            .header("host", "ext-proc-body.example.com")
            .header("x-ext-proc-test-mode", mode);
        if let Some(body) = body {
            request = request.body(body);
        }
        match request.send().await {
            Ok(response) if response.status().as_u16() == expected_status => {
                if let Some(expected_body) = expected_body {
                    let text = response.text().await.unwrap_or_default();
                    if echo_body(&text) == Some(expected_body) {
                        TestResult::passed(start.elapsed())
                    } else {
                        TestResult::failed(start.elapsed(), format!("Unexpected upstream echo: {text}"))
                    }
                } else {
                    TestResult::passed(start.elapsed())
                }
            }
            Ok(response) => TestResult::failed(
                start.elapsed(),
                format!("Expected {expected_status}, got {}", response.status()),
            ),
            Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
        }
    }

    fn ext_proc_duplicate_header_fails_closed() -> TestCase {
        TestCase::new(
            "ext_proc_duplicate_header_fails_closed",
            "ExtProc rejects a duplicate header verdict in place of the body verdict",
            |ctx| {
                Box::pin(Self::run_ext_proc_contract(
                    ctx,
                    "duplicate-header",
                    Some("original"),
                    500,
                    None,
                ))
            },
        )
    }

    fn ext_proc_wrong_phase_fails_closed() -> TestCase {
        TestCase::new(
            "ext_proc_wrong_phase_fails_closed",
            "ExtProc rejects reordered body/header phase verdicts",
            |ctx| {
                Box::pin(Self::run_ext_proc_contract(
                    ctx,
                    "wrong-order",
                    Some("original"),
                    500,
                    None,
                ))
            },
        )
    }

    fn ext_proc_missing_body_verdict_fails_closed() -> TestCase {
        TestCase::new(
            "ext_proc_missing_body_verdict_fails_closed",
            "ExtProc rejects a stream closed after headers without the required body verdict",
            |ctx| {
                Box::pin(Self::run_ext_proc_contract(
                    ctx,
                    "missing-body",
                    Some("original"),
                    500,
                    None,
                ))
            },
        )
    }

    fn ext_proc_continue_and_replace_stops_body_phase() -> TestCase {
        TestCase::new(
            "ext_proc_continue_and_replace_stops_body_phase",
            "ExtProc applies a header-phase CONTINUE_AND_REPLACE without sending the original body",
            |ctx| {
                Box::pin(Self::run_ext_proc_contract(
                    ctx,
                    "continue-and-replace",
                    Some("original"),
                    200,
                    Some("replaced-at-headers"),
                ))
            },
        )
    }

    fn ext_proc_creates_body_from_empty_request() -> TestCase {
        TestCase::new(
            "ext_proc_creates_body_from_empty_request",
            "ExtProc can replace an empty request body with a non-empty current version",
            |ctx| {
                Box::pin(Self::run_ext_proc_contract(
                    ctx,
                    "bodyless-create",
                    None,
                    200,
                    Some("created-by-ext-proc"),
                ))
            },
        )
    }

    fn serial_owners_observe_committed_body_version() -> TestCase {
        TestCase::new(
            "serial_owners_observe_committed_body_version",
            "HMAC reads V0, ExtProc commits V1, ForwardAuth and upstream both receive V1",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let original = b"owner-chain-original";
                    let final_body = b"processed-by-ext-proc";
                    let (date, digest, authorization) = hmac_headers("body-owner-chain.example.com", "/echo", original);
                    let response = ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", "body-owner-chain.example.com")
                        .header("date", date)
                        .header("digest", digest)
                        .header("authorization", authorization)
                        .header("x-auth-expected-body-sha256", hex::encode(Sha256::digest(final_body)))
                        .body(original.to_vec())
                        .send()
                        .await;
                    match response {
                        Ok(response) if response.status().as_u16() == 200 => {
                            let text = response.text().await.unwrap_or_default();
                            if echo_body(&text).is_some_and(|body| body.as_bytes() == final_body) {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("Upstream did not receive V1: {text}"))
                            }
                        }
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Expected 200, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    fn mutation_retry_shorter_h1() -> TestCase {
        TestCase::new(
            "mutation_retry_shorter_h1",
            "HTTP/1.1 retry replays a shorter mutated body with final framing on every attempt",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let expected = br#"{"payload":"x"}"#;
                    let test_id = format!("body-shorter-{}", uuid::Uuid::new_v4());
                    let response = ctx
                        .http_client
                        .post(format!("{}/retry-sequence/{test_id}/503,200", ctx.http_url()))
                        .header("host", "body-mutation-retry-shorter.example.com")
                        .header("content-type", "application/json")
                        .header("x-expected-body-sha256", hex::encode(Sha256::digest(expected)))
                        .body(r#"{"payload":"this value is deliberately longer","discard":"remove-me"}"#)
                        .send()
                        .await;
                    match response {
                        Ok(response)
                            if response.status().as_u16() == 200
                                && response.headers().get("x-attempt-number").and_then(|v| v.to_str().ok())
                                    == Some("2") =>
                        {
                            TestResult::passed(start.elapsed())
                        }
                        Ok(response) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected successful attempt 2, got status={} attempt={:?}",
                                response.status(),
                                response.headers().get("x-attempt-number")
                            ),
                        ),
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    fn mutation_retry_longer_h2() -> TestCase {
        TestCase::new(
            "mutation_retry_longer_h2",
            "HTTP/2 retry replays a longer mutated body with final framing on every attempt",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let payload = "x".repeat(64);
                    let expected = serde_json::to_vec(&serde_json::json!({"payload": payload})).unwrap();
                    let gateway = match format!("{}:31180", ctx.target_host).parse() {
                        Ok(address) => address,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("Gateway address invalid: {error}"));
                        }
                    };
                    let client = match reqwest::Client::builder()
                        .http2_prior_knowledge()
                        .no_proxy()
                        .resolve("body-mutation-retry-longer.example.com", gateway)
                        .build()
                    {
                        Ok(client) => client,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("H2 client build failed: {error}"));
                        }
                    };
                    let test_id = format!("body-longer-{}", uuid::Uuid::new_v4());
                    let response = client
                        .post(format!(
                            "http://body-mutation-retry-longer.example.com:31180/retry-sequence/{test_id}/503,200"
                        ))
                        .header("content-type", "application/json")
                        .header("x-expected-body-sha256", hex::encode(Sha256::digest(&expected)))
                        .body(r#"{"payload":"x"}"#)
                        .send()
                        .await;
                    match response {
                        Ok(response)
                            if response.status().as_u16() == 200
                                && response.headers().get("x-attempt-number").and_then(|v| v.to_str().ok())
                                    == Some("2") =>
                        {
                            TestResult::passed(start.elapsed())
                        }
                        Ok(response) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected successful H2 attempt 2, got status={} attempt={:?}",
                                response.status(),
                                response.headers().get("x-attempt-number")
                            ),
                        ),
                        Err(error) => TestResult::failed(start.elapsed(), format!("H2 request failed: {error}")),
                    }
                })
            },
        )
    }

    async fn run_mutation_then_mirror(
        ctx: TestContext,
        host: &'static str,
        original: &'static str,
        expected: &'static str,
    ) -> TestResult {
        let start = Instant::now();
        let trace_id = format!("body-mirror-{}", uuid::Uuid::new_v4());
        let response = ctx
            .http_client
            .post(format!("{}/mirror/capture", ctx.http_url()))
            .header("host", host)
            .header("content-type", "application/json")
            .header("x-trace-id", &trace_id)
            .body(original)
            .send()
            .await;
        match response {
            Ok(response) if response.status().as_u16() == 200 => {
                let expected_digest = hex::encode(Sha256::digest(expected.as_bytes()));
                let actual_digest = response
                    .headers()
                    .get("x-captured-body-sha256")
                    .and_then(|value| value.to_str().ok());
                if actual_digest != Some(expected_digest.as_str()) {
                    return TestResult::failed(
                        start.elapsed(),
                        format!(
                            "Upstream captured wrong committed body digest: expected={expected_digest} actual={actual_digest:?}"
                        ),
                    );
                }
            }
            Ok(response) => {
                return TestResult::failed(
                    start.elapsed(),
                    format!("Expected main request 200, got {}", response.status()),
                );
            }
            Err(error) => {
                return TestResult::failed(start.elapsed(), format!("Main request failed: {error}"));
            }
        }

        match poll_mirrored_body(&ctx, &trace_id).await {
            Ok(body) if body.as_bytes() == expected.as_bytes() => TestResult::passed(start.elapsed()),
            Ok(body) => TestResult::failed(
                start.elapsed(),
                format!(
                    "Mirror body was not byte-identical: expected_sha256={} actual_sha256={}",
                    hex::encode(Sha256::digest(expected.as_bytes())),
                    hex::encode(Sha256::digest(body.as_bytes()))
                ),
            ),
            Err(error) => TestResult::failed(start.elapsed(), error),
        }
    }

    fn mutation_then_mirror_uses_committed_body() -> TestCase {
        TestCase::new(
            "mutation_then_mirror_uses_committed_body",
            "A later RequestMirror owner and upstream both receive a shorter committed body",
            |ctx| {
                Box::pin(Self::run_mutation_then_mirror(
                    ctx,
                    "body-mirror-chain.example.com",
                    r#"{"version":"original","discard":"remove-me"}"#,
                    r#"{"version":"committed"}"#,
                ))
            },
        )
    }

    fn longer_mutation_then_mirror_uses_committed_body() -> TestCase {
        TestCase::new(
            "longer_mutation_then_mirror_uses_committed_body",
            "A later RequestMirror owner and upstream both receive a longer committed body",
            |ctx| {
                Box::pin(Self::run_mutation_then_mirror(
                    ctx,
                    "body-mirror-chain-longer.example.com",
                    r#"{"version":"x"}"#,
                    r#"{"version":"committed-body-created-by-a-longer-proxy-rewrite-mutation"}"#,
                ))
            },
        )
    }

    fn wasm_request_body_hook() -> TestCase {
        TestCase::new(
            "wasm_request_body_hook",
            "Wasm proxy_on_request_body observes the shared buffered body",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let before = match wasm_request_body_counters(&ctx).await {
                        Ok(counters) => counters,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let first = ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", "wasm-body.example.com")
                        .body("hello world")
                        .send()
                        .await;
                    match first {
                        Ok(response) if response.status().as_u16() == 200 => {}
                        Ok(response) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Body-producing request expected 200, got {}", response.status()),
                            );
                        }
                        Err(error) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Body-producing request failed: {error}"),
                            );
                        }
                    }
                    let after = match wasm_request_body_counters(&ctx).await {
                        Ok(counters) => counters,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let Some(delta) = after
                        .0
                        .checked_sub(before.0)
                        .zip(after.1.checked_sub(before.1))
                        .zip(after.2.checked_sub(before.2))
                        .map(|((bytes, calls), eos)| (bytes, calls, eos))
                    else {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Wasm request-body counters decreased: before={before:?}, after={after:?}"),
                        );
                    };
                    if delta == (11, 1, 1) {
                        TestResult::passed(start.elapsed())
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected one terminal whole-body callback (11,1,1), got delta={delta:?}, before={before:?}, after={after:?}"
                            ),
                        )
                    }
                })
            },
        )
    }

    fn wasm_request_body_mutation() -> TestCase {
        TestCase::new(
            "wasm_request_body_mutation",
            "Proxy-Wasm range mutations rewrite memory-backed and bodyless requests",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let response = ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", "wasm-body.example.com")
                        .header("x-wasm-rewrite-body", "1")
                        .body("abcdef")
                        .send()
                        .await;
                    match response {
                        Ok(response) if response.status().as_u16() == 200 => {
                            let text = response.text().await.unwrap_or_default();
                            if echo_body(&text) != Some("a+def!") {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Wasm range rewrite mismatch: {text}"),
                                );
                            }
                        }
                        Ok(response) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Wasm range rewrite expected 200, got {}", response.status()),
                            );
                        }
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("Wasm range rewrite failed: {error}"));
                        }
                    }

                    let response = ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", "wasm-body.example.com")
                        .header("x-wasm-append-body", "1")
                        .body("")
                        .send()
                        .await;
                    match response {
                        Ok(response) if response.status().as_u16() == 200 => {
                            let text = response.text().await.unwrap_or_default();
                            if echo_body(&text) == Some("created") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("Wasm bodyless append mismatch: {text}"))
                            }
                        }
                        Ok(response) => TestResult::failed(
                            start.elapsed(),
                            format!("Wasm bodyless append expected 200, got {}", response.status()),
                        ),
                        Err(error) => {
                            TestResult::failed(start.elapsed(), format!("Wasm bodyless append failed: {error}"))
                        }
                    }
                })
            },
        )
    }

    fn wasm_request_body_mutation_spill_and_retry() -> TestCase {
        TestCase::new(
            "wasm_request_body_mutation_spill_and_retry",
            "Proxy-Wasm commits disk-backed mutations and retry replays the committed generation",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let original = format!("abcdef{}", "x".repeat(256 * 1024));
                    let expected = format!("a+def{}!", "x".repeat(256 * 1024));
                    let response = ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", "wasm-body.example.com")
                        .header("x-wasm-rewrite-body", "1")
                        .body(original)
                        .send()
                        .await;
                    match response {
                        Ok(response) if response.status().as_u16() == 200 => {
                            let text = response.text().await.unwrap_or_default();
                            if echo_body(&text) != Some(expected.as_str()) {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "Wasm spilled rewrite mismatch".to_string(),
                                );
                            }
                        }
                        Ok(response) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Wasm spilled rewrite expected 200, got {}", response.status()),
                            );
                        }
                        Err(error) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Wasm spilled rewrite failed: {error}"),
                            );
                        }
                    }

                    let test_id = format!("wasm-body-{}", uuid::Uuid::new_v4());
                    let expected = b"a+def!";
                    let response = ctx
                        .http_client
                        .post(format!("{}/retry-sequence/{test_id}/503,200", ctx.http_url()))
                        .header("host", "wasm-body-retry.example.com")
                        .header("x-wasm-rewrite-body", "1")
                        .header("x-expected-body-sha256", hex::encode(Sha256::digest(expected)))
                        .body("abcdef")
                        .send()
                        .await;
                    match response {
                        Ok(response)
                            if response.status().as_u16() == 200
                                && response
                                    .headers()
                                    .get("x-attempt-number")
                                    .and_then(|value| value.to_str().ok())
                                    == Some("2") =>
                        {
                            TestResult::passed(start.elapsed())
                        }
                        Ok(response) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Wasm retry expected successful attempt 2, got status={} attempt={:?}",
                                response.status(),
                                response.headers().get("x-attempt-number")
                            ),
                        ),
                        Err(error) => TestResult::failed(start.elapsed(), format!("Wasm retry failed: {error}")),
                    }
                })
            },
        )
    }

    fn wasm_request_body_mutation_reframes_chunked_upstream() -> TestCase {
        TestCase::new(
            "wasm_request_body_mutation_reframes_chunked_upstream",
            "A length-changing Wasm mutation replaces downstream chunked framing for upstream replay",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let response = raw_chunked_request(
                        &ctx.target_host,
                        "wasm-body.example.com",
                        "/echo",
                        "application/octet-stream",
                        &[("x-wasm-rewrite-body", "1")],
                        &[b"abc", b"def"],
                    )
                    .await;
                    match response {
                        Ok(response) if response.contains(" 200 ") && response.contains("Echo: a+def!") => {
                            TestResult::passed(start.elapsed())
                        }
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Unexpected chunked response: {response:?}"))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    fn wasm_request_body_mutation_mirror() -> TestCase {
        TestCase::new(
            "wasm_request_body_mutation_mirror",
            "A later RequestMirror and upstream receive the committed Wasm Body generation",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("wasm-body-mirror-{}", uuid::Uuid::new_v4());
                    let expected = "a+def!";
                    let response = ctx
                        .http_client
                        .post(format!("{}/mirror/capture", ctx.http_url()))
                        .header("host", "wasm-body-mirror.example.com")
                        .header("x-wasm-rewrite-body", "1")
                        .header("x-trace-id", &trace_id)
                        .body("abcdef")
                        .send()
                        .await;
                    match response {
                        Ok(response) if response.status().as_u16() == 200 => {
                            let expected_digest = hex::encode(Sha256::digest(expected.as_bytes()));
                            let actual_digest = response
                                .headers()
                                .get("x-captured-body-sha256")
                                .and_then(|value| value.to_str().ok());
                            if actual_digest != Some(expected_digest.as_str()) {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Upstream captured wrong Wasm body: expected={expected_digest} actual={actual_digest:?}"
                                    ),
                                );
                            }
                        }
                        Ok(response) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Wasm mirror request expected 200, got {}", response.status()),
                            );
                        }
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("Wasm mirror request failed: {error}"));
                        }
                    }

                    match poll_mirrored_body(&ctx, &trace_id).await {
                        Ok(body) if body == expected => TestResult::passed(start.elapsed()),
                        Ok(body) => {
                            TestResult::failed(start.elapsed(), format!("Mirror received wrong Wasm body: {body:?}"))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    fn wasm_response_body_hook() -> TestCase {
        TestCase::new(
            "wasm_response_body_hook",
            "Wasm response-body observation runs only in the body-capable suite",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/headers", ctx.http_url());
                    let _ = ctx
                        .http_client
                        .get(&url)
                        .header("host", "wasm-body.example.com")
                        .send()
                        .await;
                    let response = ctx
                        .http_client
                        .get(&url)
                        .header("host", "wasm-body.example.com")
                        .header("x-wasm-read-body", "1")
                        .send()
                        .await;
                    match response {
                        Ok(response) if response.status().as_u16() == 200 => {
                            let value = response.json::<serde_json::Value>().await.unwrap_or_default();
                            let count = value["headers"]["x-wasm-bodybytes"]
                                .as_str()
                                .and_then(|value| value.parse::<u64>().ok())
                                .unwrap_or(0);
                            if count > 0 {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "Response body hook observed zero bytes".to_string(),
                                )
                            }
                        }
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Expected 200, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }
}

impl TestSuite for BodyPluginsTestSuite {
    fn name(&self) -> &str {
        "Body-Aware Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::hmac_body(),
            Self::hmac_rejects_tampered_body(),
            Self::hmac_accepts_empty_body_digest(),
            Self::forward_auth_body(),
            Self::proxy_rewrite_json_body(),
            Self::proxy_rewrite_chunked_body_reframes_upstream(),
            Self::proxy_rewrite_advanced_json_patch(),
            Self::proxy_rewrite_patch_failure_is_atomic(),
            Self::proxy_rewrite_spilled_json_body(),
            Self::form_to_json_memory_and_strict_errors(),
            Self::json_to_form_and_ordered_chains(),
            Self::form_json_chain_limits_and_media(),
            Self::form_json_chunked_body_reframes_upstream(),
            Self::form_json_spilled_body(),
            Self::form_json_retry_replays_committed_body(),
            Self::ext_proc_buffered_body(),
            Self::ext_proc_duplicate_header_fails_closed(),
            Self::ext_proc_wrong_phase_fails_closed(),
            Self::ext_proc_missing_body_verdict_fails_closed(),
            Self::ext_proc_continue_and_replace_stops_body_phase(),
            Self::ext_proc_creates_body_from_empty_request(),
            Self::serial_owners_observe_committed_body_version(),
            Self::mutation_retry_shorter_h1(),
            Self::mutation_retry_longer_h2(),
            Self::mutation_then_mirror_uses_committed_body(),
            Self::longer_mutation_then_mirror_uses_committed_body(),
            Self::wasm_request_body_hook(),
            Self::wasm_request_body_mutation(),
            Self::wasm_request_body_mutation_spill_and_retry(),
            Self::wasm_request_body_mutation_reframes_chunked_upstream(),
            Self::wasm_request_body_mutation_mirror(),
            Self::wasm_response_body_hook(),
        ]
    }
}

async fn run_body_dynamic_case(ctx: TestContext, expected_version: &'static str) -> TestResult {
    let start = Instant::now();
    let response = ctx
        .http_client
        .post(format!("http://{}:31180/echo", ctx.target_host))
        .header("host", "body-dynamic.example.com")
        .header("content-type", "application/json")
        .body(r#"{"version":"client","discard":"remove-me"}"#)
        .send()
        .await;
    match response {
        Ok(response) if response.status().as_u16() == 200 => {
            let text = response.text().await.unwrap_or_default();
            let actual = echo_body(&text).and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok());
            let expected = serde_json::json!({"version": expected_version});
            if actual.as_ref() == Some(&expected) {
                TestResult::passed(start.elapsed())
            } else {
                TestResult::failed(
                    start.elapsed(),
                    format!("Expected committed body {expected}, got {text}"),
                )
            }
        }
        Ok(response) => TestResult::failed(start.elapsed(), format!("Expected 200, got {}", response.status())),
        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
    }
}

async fn fetch_body_dynamic_accepted(ctx: &TestContext) -> Result<(String, String), String> {
    let url = format!(
        "{}/configserver/EdgionPlugins?namespace=edgion-default&name=body-dynamic",
        ctx.admin_api_url()
    );
    let mut request = ctx.http_client.get(url);
    if let Some(token) = &ctx.admin_token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("Status request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("Status request returned {}", response.status()));
    }
    let body = response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| format!("Status response was not JSON: {error}"))?;
    let condition = body["data"]["status"]["conditions"]
        .as_array()
        .and_then(|conditions| {
            conditions
                .iter()
                .find(|condition| condition["type"].as_str() == Some("Accepted"))
        })
        .ok_or_else(|| format!("Accepted condition missing from response: {body}"))?;
    Ok((
        condition["status"].as_str().unwrap_or_default().to_string(),
        condition["reason"].as_str().unwrap_or_default().to_string(),
    ))
}

fn body_dynamic_initial_case() -> TestCase {
    TestCase::new(
        "body_dynamic_initial",
        "The active hot-reloaded plugin commits the expected complete request body",
        |ctx| Box::pin(run_body_dynamic_case(ctx, "initial")),
    )
}

fn body_dynamic_update_case() -> TestCase {
    TestCase::new(
        "body_dynamic_update",
        "The active hot-reloaded plugin commits the expected complete request body",
        |ctx| Box::pin(run_body_dynamic_case(ctx, "updated-body-version-is-longer")),
    )
}

fn body_dynamic_rejected_status_case() -> TestCase {
    TestCase::new(
        "body_dynamic_invalid_json_select_rejected",
        "A hot update with a dynamic json_select plan is rejected by configuration validation",
        |ctx| {
            Box::pin(async move {
                let start = Instant::now();
                match fetch_body_dynamic_accepted(&ctx).await {
                    Ok((status, reason)) if status == "False" => TestResult::passed_with_message(
                        start.elapsed(),
                        format!("Accepted=False as expected ({reason})"),
                    ),
                    Ok((status, reason)) => TestResult::failed(
                        start.elapsed(),
                        format!("Expected Accepted=False, got status={status} reason={reason}"),
                    ),
                    Err(error) => TestResult::failed(start.elapsed(), error),
                }
            })
        },
    )
}

fn body_dynamic_capped_case() -> TestCase {
    TestCase::new(
        "body_dynamic_capped",
        "A hot-reloaded gateway body cap rejects the next oversized request",
        |ctx| {
            Box::pin(async move {
                let start = Instant::now();
                match ctx
                    .http_client
                    .post(format!("http://{}:31180/echo", ctx.target_host))
                    .header("host", "body-dynamic.example.com")
                    .header("content-type", "application/json")
                    .body(r#"{"version":"client","discard":"remove-me"}"#)
                    .send()
                    .await
                {
                    Ok(response) if response.status().as_u16() == 413 => TestResult::passed(start.elapsed()),
                    Ok(response) => {
                        TestResult::failed(start.elapsed(), format!("Expected 413, got {}", response.status()))
                    }
                    Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                }
            })
        },
    )
}

fn body_dynamic_unfinished_h1_case() -> TestCase {
    TestCase::new(
        "body_dynamic_unfinished_h1",
        "An over-limit HTTP/1.1 chunked request receives 413 and closes without waiting for EOF",
        |ctx| {
            Box::pin(async move {
                let start = Instant::now();
                match raw_unfinished_chunked_request(
                    &ctx.target_host,
                    "body-dynamic.example.com",
                    b"{\"payload\":\"larger-than-cap\"}",
                )
                .await
                {
                    Ok(response) if response.contains(" 413 ") => TestResult::passed(start.elapsed()),
                    Ok(response) => {
                        TestResult::failed(start.elapsed(), format!("Expected HTTP/1.1 413, got {response:?}"))
                    }
                    Err(error) => TestResult::failed(start.elapsed(), error),
                }
            })
        },
    )
}

fn body_dynamic_unfinished_h2_case() -> TestCase {
    TestCase::new(
        "body_dynamic_unfinished_h2",
        "An over-limit HTTP/2 stream receives 413 and resets without closing the shared connection",
        |ctx| {
            Box::pin(async move {
                let start = Instant::now();
                match h2_unfinished_over_limit_request(&ctx.target_host).await {
                    Ok(()) => TestResult::passed(start.elapsed()),
                    Err(error) => TestResult::failed(start.elapsed(), error),
                }
            })
        },
    )
}

pub struct BodyDynamicInitialTestSuite;

impl TestSuite for BodyDynamicInitialTestSuite {
    fn name(&self) -> &str {
        "Request Body Dynamic - Initial Phase"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![body_dynamic_initial_case()]
    }
}

pub struct BodyDynamicCappedTestSuite;

impl TestSuite for BodyDynamicCappedTestSuite {
    fn name(&self) -> &str {
        "Request Body Dynamic - Capped Phase"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            body_dynamic_capped_case(),
            body_dynamic_unfinished_h1_case(),
            body_dynamic_unfinished_h2_case(),
        ]
    }
}

pub struct BodyDynamicUpdateTestSuite;

impl TestSuite for BodyDynamicUpdateTestSuite {
    fn name(&self) -> &str {
        "Request Body Dynamic - Update Phase"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![body_dynamic_update_case()]
    }
}

pub struct BodyDynamicRejectedTestSuite;

impl TestSuite for BodyDynamicRejectedTestSuite {
    fn name(&self) -> &str {
        "Request Body Dynamic - Rejected Phase"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![body_dynamic_rejected_status_case(), body_dynamic_update_case()]
    }
}
