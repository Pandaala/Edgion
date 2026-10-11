//! Wire assertions for the WAF verdict before forwarding contract.

use super::*;
use crate::framework::env_host_or;
use crate::transport::{RawH1Conn, TransportProbe, KIND_REQ_DATA, KIND_REQ_HEADERS, RECORDER_PORT};
use sha2::{Digest, Sha256};

const WINDOW: usize = 65_536;

async fn verify_wire(
    ctx: &TestContext,
    probe: &TransportProbe,
    upstream_h2: bool,
    downstream_h2: bool,
    body: Vec<u8>,
    blocked: bool,
    full: bool,
) -> Result<(), String> {
    let host = format!("waf-transport-h{}.example.com", if upstream_h2 { 2 } else { 1 });
    let address = tokio::net::lookup_host((ctx.target_host.as_str(), ctx.http_port))
        .await
        .map_err(|error| format!("failed to resolve test address: {error}"))?
        .next()
        .ok_or_else(|| "test address resolved to no endpoints".to_string())?;
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .resolve(&host, address)
        .timeout(Duration::from_secs(10));
    if downstream_h2 {
        builder = builder.http2_prior_knowledge();
    } else {
        builder = builder.http1_only();
    }
    let client = builder.build().map_err(|e| e.to_string())?;
    let response = client
        .post(format!("http://{host}:{}/waf-wire", ctx.http_port))
        .header(
            "traceparent",
            crate::access_log_client::traceparent_for_marker(probe.trace_id()),
        )
        .header("x-waf-version", if full { "v2" } else { "v1" })
        .header("content-type", "text/plain")
        .body(body.clone())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    if status != if blocked { 406 } else { 200 } {
        return Err(format!("unexpected status {status}; blocked={blocked}, full={full}"));
    }
    if blocked {
        assert_no_forward(probe).await?;
    } else {
        let headers = response.headers();
        let expected_len = body.len().to_string();
        let expected_digest = format!("{:x}", Sha256::digest(&body));
        for (name, expected) in [
            ("x-up-req-body-len", expected_len.as_str()),
            ("x-up-req-body-sha256", expected_digest.as_str()),
            ("x-up-http-version", if upstream_h2 { "2" } else { "1.1" }),
        ] {
            if headers.get(name).and_then(|v| v.to_str().ok()) != Some(expected) {
                return Err(format!("{name}: expected {expected}, got {:?}", headers.get(name)));
            }
        }
        response.bytes().await.map_err(|e| e.to_string())?;
        let snapshot = probe.snapshot().await?;
        if snapshot.dropped != 0
            || snapshot.count_matching(|e| e.is(KIND_REQ_HEADERS)) != 1
            || snapshot.req_body_total(probe.trace_id()) != body.len()
        {
            return Err(format!("expected one complete upstream request: {snapshot}"));
        }
    }
    Ok(())
}

async fn assert_no_forward(probe: &TransportProbe) -> Result<(), String> {
    let snapshot = probe.snapshot().await?;
    if snapshot.dropped != 0 || snapshot.count_matching(|e| e.is(KIND_REQ_HEADERS) || e.is(KIND_REQ_DATA)) != 0 {
        return Err(format!("WAF rejection exposed request bytes to backend: {snapshot}"));
    }
    probe
        .expect_none("upstream headers or body", Duration::from_millis(150), |e| {
            e.is(KIND_REQ_HEADERS) || e.is(KIND_REQ_DATA)
        })
        .await
}

async fn split_marker(ctx: &TestContext, probe: &TransportProbe) -> Result<(), String> {
    let mut conn = RawH1Conn::connect(&ctx.target_host, ctx.http_port).await?;
    let traceparent = crate::access_log_client::traceparent_for_marker(probe.trace_id());
    conn.write_head(
        "POST",
        "/waf-wire",
        "waf-transport-h1.example.com",
        &[
            ("transfer-encoding", "chunked"),
            ("expect", "100-continue"),
            ("content-type", "text/plain"),
            ("x-waf-version", "v1"),
            ("traceparent", &traceparent),
        ],
    )
    .await?;
    let interim = conn.read_interim(Duration::from_secs(3)).await?;
    if interim.as_ref().map(|head| head.status) != Some(100) {
        return Err(format!("expected local 100 Continue, got {interim:?}"));
    }
    conn.write_chunk(b"safe-prefix-bloc").await?;
    // Keep the upload open across a recorder observation window: the backend
    // must remain untouched while the verdict cannot yet be made.
    assert_no_forward(probe).await?;
    conn.write_chunk(b"ked-safe-suffix").await?;
    conn.finish_chunked().await?;
    let response = conn.read_response_head(Duration::from_secs(5)).await?;
    if response.status != 406 {
        return Err(format!("cross-chunk marker expected 406, got {response}"));
    }
    assert_no_forward(probe).await
}

pub(super) fn case(full: bool, window: usize) -> TestCase {
    TestCase::new(
        if full {
            "waf_full_wire_after_reload"
        } else {
            "waf_prefix_wire_matrix"
        },
        "WAF decides before backend headers; allowed prefix and tail arrive exactly once",
        move |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let run = async {
                    let recorder_host = env_host_or("EDGION_TEST_TRANSPORT_RECORDER_HOST", &ctx.target_host);
                    for downstream_h2 in [false, true] {
                        for upstream_h2 in [false, true] {
                            for size in [window - 1, window, window + 1] {
                                let probe = TransportProbe::start(&recorder_host, RECORDER_PORT, "waf-allowed").await?;
                                let body = (0..size).map(|i| b'a' + (i % 23) as u8).collect();
                                verify_wire(&ctx, &probe, upstream_h2, downstream_h2, body, false, full).await?;
                            }
                            let probe = TransportProbe::start(&recorder_host, RECORDER_PORT, "waf-denied").await?;
                            verify_wire(
                                &ctx,
                                &probe,
                                upstream_h2,
                                downstream_h2,
                                b"safe-blocked-suffix".to_vec(),
                                true,
                                full,
                            )
                            .await?;
                            let mut body = vec![b'x'; window];
                            body.extend_from_slice(b"blocked");
                            let probe = TransportProbe::start(&recorder_host, RECORDER_PORT, "waf-tail").await?;
                            verify_wire(&ctx, &probe, upstream_h2, downstream_h2, body, full, full).await?;
                            for offset in [window - b"blocked".len(), window - 3] {
                                let mut body = vec![b'x'; offset];
                                body.extend_from_slice(b"blocked");
                                let blocked = full || offset + b"blocked".len() <= window;
                                let probe =
                                    TransportProbe::start(&recorder_host, RECORDER_PORT, "waf-boundary").await?;
                                verify_wire(&ctx, &probe, upstream_h2, downstream_h2, body, blocked, full).await?;
                            }
                        }
                    }
                    if !full {
                        let probe = TransportProbe::start(&recorder_host, RECORDER_PORT, "waf-split").await?;
                        split_marker(&ctx, &probe).await?;
                    }
                    Ok::<(), String>(())
                };
                match tokio::time::timeout(Duration::from_secs(90), run).await {
                    Ok(Ok(())) => TestResult::passed(start.elapsed()),
                    Ok(Err(error)) => TestResult::failed(start.elapsed(), error),
                    Err(_) => TestResult::failed(start.elapsed(), "WAF wire matrix exceeded deadline".to_string()),
                }
            })
        },
    )
}

pub(super) fn detection_case() -> TestCase {
    TestCase::new(
        "waf_detection_wire_after_reload",
        "Detection-only inspection preserves a matching body and forwards exactly once",
        |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let run = async {
                    let recorder_host = env_host_or("EDGION_TEST_TRANSPORT_RECORDER_HOST", &ctx.target_host);
                    for downstream_h2 in [false, true] {
                        for upstream_h2 in [false, true] {
                            let probe = TransportProbe::start(&recorder_host, RECORDER_PORT, "waf-detection").await?;
                            let mut body = b"blocked".to_vec();
                            body.resize(WINDOW + 1, b'x');
                            verify_wire(&ctx, &probe, upstream_h2, downstream_h2, body, false, false).await?;
                        }
                    }
                    Ok::<(), String>(())
                };
                match tokio::time::timeout(Duration::from_secs(45), run).await {
                    Ok(Ok(())) => TestResult::passed(start.elapsed()),
                    Ok(Err(error)) => TestResult::failed(start.elapsed(), error),
                    Err(_) => TestResult::failed(start.elapsed(), "WAF detection matrix exceeded deadline".to_string()),
                }
            })
        },
    )
}

pub(super) fn format_limit_case(phase: &'static str) -> TestCase {
    TestCase::new(
        "waf_format_hard_limits",
        "JSON/XML hard bounds cover known and unknown lengths across H1/H2",
        move |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let run = async {
                    let recorder_host = env_host_or("EDGION_TEST_TRANSPORT_RECORDER_HOST", &ctx.target_host);
                    let address = tokio::net::lookup_host((ctx.target_host.as_str(), ctx.http_port))
                        .await
                        .map_err(|e| e.to_string())?
                        .next()
                        .ok_or("missing test address")?;
                    for downstream_h2 in [false, true] {
                        for upstream_h2 in [false, true] {
                            let host = format!("waf-transport-h{}.example.com", if upstream_h2 { 2 } else { 1 });
                            let builder = reqwest::Client::builder()
                                .no_proxy()
                                .resolve(&host, address)
                                .timeout(Duration::from_secs(10));
                            let client = if downstream_h2 {
                                builder.http2_prior_knowledge()
                            } else {
                                builder.http1_only()
                            }
                            .build()
                            .map_err(|e| e.to_string())?;
                            paused_format_overflow(&ctx, &client, &host, &recorder_host, phase, downstream_h2).await?;
                            for (content_type, limit) in
                                [("application/json", 128), ("application/xml", 64), ("text/plain", 256)]
                            {
                                for size in [limit - 1, limit, limit + 1] {
                                    for unknown_length in [false, true] {
                                        let probe =
                                            TransportProbe::start(&recorder_host, RECORDER_PORT, "waf-format-limit")
                                                .await?;
                                        // Prefix and header-only generations do not require a structured document.
                                        let body = vec![b'a'; size];
                                        let payload = if unknown_length {
                                            let chunks: Vec<_> = body
                                                .chunks(16)
                                                .map(|c| Ok::<_, std::io::Error>(bytes::Bytes::copy_from_slice(c)))
                                                .collect();
                                            reqwest::Body::wrap_stream(futures::stream::iter(chunks))
                                        } else {
                                            reqwest::Body::from(body.clone())
                                        };
                                        let response = client
                                            .post(format!("http://{host}:{}/waf-wire", ctx.http_port))
                                            .header("content-type", content_type)
                                            .header("x-waf-hard-phase", phase)
                                            .header(
                                                "traceparent",
                                                crate::access_log_client::traceparent_for_marker(probe.trace_id()),
                                            )
                                            .body(payload)
                                            .send()
                                            .await
                                            .map_err(|e| e.to_string())?;
                                        let rejected = content_type != "text/plain" && size > limit;
                                        let expected = if rejected { 413 } else { 200 };
                                        if response.status().as_u16() != expected {
                                            return Err(format!("{content_type} size={size} unknown={unknown_length} h2={downstream_h2}: expected {expected}, got {}", response.status()));
                                        }
                                        let expected_version = if downstream_h2 {
                                            reqwest::Version::HTTP_2
                                        } else {
                                            reqwest::Version::HTTP_11
                                        };
                                        if response.version() != expected_version {
                                            return Err(
                                                "format limit response used the wrong downstream protocol".into()
                                            );
                                        }
                                        if !rejected {
                                            let upstream_version = if upstream_h2 { "2" } else { "1.1" };
                                            if response
                                                .headers()
                                                .get("x-up-http-version")
                                                .and_then(|v| v.to_str().ok())
                                                != Some(upstream_version)
                                            {
                                                return Err(
                                                    "format limit request used the wrong upstream protocol".into()
                                                );
                                            }
                                            let digest = format!("{:x}", Sha256::digest(&body));
                                            if response
                                                .headers()
                                                .get("x-up-req-body-sha256")
                                                .and_then(|v| v.to_str().ok())
                                                != Some(digest.as_str())
                                            {
                                                return Err("allowed body digest changed across capture/replay".into());
                                            }
                                        } else if !unknown_length {
                                            assert_no_forward(&probe).await?;
                                        }
                                        response.bytes().await.map_err(|e| e.to_string())?;
                                    }
                                }
                            }
                        }
                    }
                    Ok::<(), String>(())
                };
                match tokio::time::timeout(Duration::from_secs(90), run).await {
                    Ok(Ok(())) => TestResult::passed(start.elapsed()),
                    Ok(Err(error)) => TestResult::failed(start.elapsed(), error),
                    Err(_) => TestResult::failed(start.elapsed(), "format hard-limit matrix exceeded deadline".into()),
                }
            })
        },
    )
}

async fn paused_format_overflow(
    ctx: &TestContext,
    client: &reqwest::Client,
    host: &str,
    recorder_host: &str,
    phase: &str,
    downstream_h2: bool,
) -> Result<(), String> {
    for (content_type, limit) in [
        ("application/json", 128usize),
        ("application/xml", if phase == "continue" { 128 } else { 64 }),
    ] {
        let probe = TransportProbe::start(recorder_host, RECORDER_PORT, "waf-format-paused").await?;
        let first = if phase == "head" {
            1
        } else if phase == "continue" {
            33
        } else {
            32
        };
        let initial = if phase == "continue" {
            let opening = if content_type == "application/json" {
                "{\"a\":\""
            } else {
                "<r>"
            };
            format!("{opening}{}", "a".repeat(first - opening.len())).into_bytes()
        } else {
            vec![b'a'; first]
        };
        let (sender, receiver) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(1);
        sender
            .send(Ok(bytes::Bytes::from(initial)))
            .await
            .map_err(|e| e.to_string())?;
        let stream = futures::stream::unfold(receiver, |mut receiver| async {
            receiver.recv().await.map(|value| (value, receiver))
        });
        let builder = client
            .post(format!("http://{host}:{}/waf-wire", ctx.http_port))
            .header("content-type", content_type)
            .header("x-waf-hard-phase", phase)
            .header("x-waf-inspection-phase", phase)
            .header(
                "traceparent",
                crate::access_log_client::traceparent_for_marker(probe.trace_id()),
            )
            .body(reqwest::Body::wrap_stream(stream));
        let mut response = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move { builder.send().await }));
        let result = async {
            // Prove dispatch while the tail is unavailable. One byte also
            // proves the new header-only generation no longer waits for the
            // old 32-byte prefix during hot-reload convergence.
            probe
                .wait_for(
                    "dispatch before releasing the oversized tail",
                    Duration::from_secs(3),
                    |event| event.is(KIND_REQ_HEADERS),
                )
                .await?;
            sender
                .send(Ok(bytes::Bytes::from(vec![b'a'; limit + 1 - first])))
                .await
                .map_err(|e| e.to_string())?;
            drop(sender);
            let reply = (&mut response)
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            if reply.status().as_u16() != 413 {
                return Err(format!(
                    "late {content_type} overflow returned {} instead of 413",
                    reply.status()
                ));
            }
            let expected_version = if downstream_h2 {
                reqwest::Version::HTTP_2
            } else {
                reqwest::Version::HTTP_11
            };
            if reply.version() != expected_version {
                return Err("late overflow response used the wrong downstream protocol".into());
            }
            reply.bytes().await.map_err(|e| e.to_string())?;
            Ok(())
        }
        .await;
        // A failed generation-convergence assertion must not leave an upload.
        response.abort();
        result?;
    }
    Ok(())
}

pub(super) fn inspection_limit_case(action: &'static str) -> TestCase {
    TestCase::new(
        "waf_inspection_limits",
        "Full inspection stops without changing forwarded bytes",
        move |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let run = async {
                    let address = tokio::net::lookup_host((ctx.target_host.as_str(), ctx.http_port))
                        .await
                        .map_err(|e| e.to_string())?
                        .next()
                        .ok_or("missing test address")?;
                    for downstream_h2 in [false, true] {
                        for upstream_h2 in [false, true] {
                            let host = format!("waf-transport-h{}.example.com", if upstream_h2 { 2 } else { 1 });
                            let builder = reqwest::Client::builder()
                                .no_proxy()
                                .resolve(&host, address)
                                .timeout(Duration::from_secs(10));
                            let client = if downstream_h2 {
                                builder.http2_prior_knowledge()
                            } else {
                                builder.http1_only()
                            }
                            .build()
                            .map_err(|e| e.to_string())?;
                            if action == "continue" {
                                let recorder_host =
                                    env_host_or("EDGION_TEST_TRANSPORT_RECORDER_HOST", &ctx.target_host);
                                paused_format_overflow(&ctx, &client, &host, &recorder_host, action, downstream_h2)
                                    .await?;
                            }
                            for content_type in ["application/json", "application/xml", "text/plain"] {
                                for size in [31usize, 32, 33, 129] {
                                    for unknown in [false, true] {
                                        if size == 129 && unknown {
                                            continue;
                                        }
                                        let body = match content_type {
                                            "application/json" => format!("{{\"a\":\"{}\"}}", "x".repeat(size - 8)),
                                            "application/xml" => format!("<r>{}</r>", "x".repeat(size - 7)),
                                            _ => "x".repeat(size),
                                        };
                                        let payload = if unknown {
                                            let chunks: Vec<_> = body
                                                .as_bytes()
                                                .chunks(3)
                                                .map(|c| Ok::<_, std::io::Error>(bytes::Bytes::copy_from_slice(c)))
                                                .collect();
                                            reqwest::Body::wrap_stream(futures::stream::iter(chunks))
                                        } else {
                                            reqwest::Body::from(body.clone())
                                        };
                                        let response = client
                                            .post(format!("http://{host}:{}/waf-wire", ctx.http_port))
                                            .header("content-type", content_type)
                                            .header("x-waf-inspection-phase", action)
                                            .body(payload)
                                            .send()
                                            .await
                                            .map_err(|e| e.to_string())?;
                                        let hard = content_type != "text/plain" && size > 128;
                                        let expected = if hard || (size > 32 && action == "reject") {
                                            413
                                        } else if size <= 32 {
                                            407
                                        } else {
                                            200
                                        };
                                        if response.status().as_u16() != expected {
                                            return Err(format!("inspection {action} {content_type} size={size} unknown={unknown} h2={downstream_h2}: expected {expected}, got {}", response.status()));
                                        }
                                        if expected == 200 {
                                            let digest = format!("{:x}", Sha256::digest(body.as_bytes()));
                                            if response
                                                .headers()
                                                .get("x-up-req-body-sha256")
                                                .and_then(|v| v.to_str().ok())
                                                != Some(digest.as_str())
                                            {
                                                return Err("inspection policy stop changed forwarded body".into());
                                            }
                                        }
                                        response.bytes().await.map_err(|e| e.to_string())?;
                                    }
                                }
                            }
                        }
                    }
                    Ok::<(), String>(())
                };
                match tokio::time::timeout(Duration::from_secs(90), run).await {
                    Ok(Ok(())) => TestResult::passed(start.elapsed()),
                    Ok(Err(error)) => TestResult::failed(start.elapsed(), error),
                    Err(_) => TestResult::failed(start.elapsed(), "inspection limit matrix exceeded deadline".into()),
                }
            })
        },
    )
}

pub(super) fn ordinary_body_limit_case() -> TestCase {
    TestCase::new(
        "ordinary_body_limits_without_capture",
        "Gateway physical admission bounds ordinary HTTP independently of WAF and storage",
        |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let run = async {
                    let address = tokio::net::lookup_host((ctx.target_host.as_str(), ctx.http_port))
                        .await
                        .map_err(|e| e.to_string())?
                        .next()
                        .ok_or("missing test address")?;
                    let recorder_host = env_host_or("EDGION_TEST_TRANSPORT_RECORDER_HOST", &ctx.target_host);
                    for downstream_h2 in [false, true] {
                        for upstream_h2 in [false, true] {
                            let host = format!("body-limit-h{}.example.com", if upstream_h2 { 2 } else { 1 });
                            let builder = reqwest::Client::builder()
                                .no_proxy()
                                .resolve(&host, address)
                                .timeout(Duration::from_secs(10));
                            let client = if downstream_h2 {
                                builder.http2_prior_knowledge()
                            } else {
                                builder.http1_only()
                            }
                            .build()
                            .map_err(|e| e.to_string())?;
                            ordinary_selected_response(&ctx, &host, &recorder_host, downstream_h2).await?;
                            for size in [127usize, 128, 129] {
                                for unknown in [false, true] {
                                    for upgrade in [None, Some("websocket"), Some("unrecognized")] {
                                        if downstream_h2 && upgrade.is_some() {
                                            continue;
                                        }
                                        let probe =
                                            TransportProbe::start(&recorder_host, RECORDER_PORT, "ordinary-body-limit")
                                                .await?;
                                        let body = vec![b'x'; size];
                                        let payload = if unknown {
                                            let chunks: Vec<_> = body
                                                .chunks(3)
                                                .map(|chunk| {
                                                    Ok::<_, std::io::Error>(bytes::Bytes::copy_from_slice(chunk))
                                                })
                                                .collect();
                                            reqwest::Body::wrap_stream(futures::stream::iter(chunks))
                                        } else {
                                            reqwest::Body::from(body.clone())
                                        };
                                        let mut request = client
                                            .post(format!("http://{host}:{}/body-limit", ctx.http_port))
                                            .header(
                                                "traceparent",
                                                crate::access_log_client::traceparent_for_marker(probe.trace_id()),
                                            )
                                            .body(payload);
                                        if let Some(upgrade) = upgrade {
                                            request =
                                                request.header("upgrade", upgrade).header("connection", "upgrade");
                                        }
                                        let response = request.send().await.map_err(|e| e.to_string())?;
                                        let expected = if size > 128 { 413 } else { 200 };
                                        if response.status().as_u16() != expected {
                                            return Err(format!("ordinary body size={size} unknown={unknown} upgrade={upgrade:?} h2={downstream_h2}/{upstream_h2}: expected {expected}, got {}", response.status()));
                                        }
                                        if expected == 200 {
                                            let digest = format!("{:x}", Sha256::digest(&body));
                                            if response
                                                .headers()
                                                .get("x-up-req-body-sha256")
                                                .and_then(|v| v.to_str().ok())
                                                != Some(digest.as_str())
                                            {
                                                return Err("ordinary admitted body changed".into());
                                            }
                                        } else if !unknown {
                                            assert_no_forward(&probe).await?;
                                        }
                                        response.bytes().await.map_err(|e| e.to_string())?;
                                    }
                                }
                            }
                        }
                    }
                    Ok::<(), String>(())
                };
                match tokio::time::timeout(Duration::from_secs(60), run).await {
                    Ok(Ok(())) => TestResult::passed(start.elapsed()),
                    Ok(Err(error)) => TestResult::failed(start.elapsed(), error),
                    Err(_) => {
                        TestResult::failed(start.elapsed(), "ordinary body limit matrix exceeded deadline".into())
                    }
                }
            })
        },
    )
}

// The response owns delivery before the oversized tail is released. The native
// callback unit test separately proves admission executes after response selection:
// a completed wire response may legitimately cancel the upload before tail reads.
async fn ordinary_selected_response(
    ctx: &TestContext,
    host: &str,
    recorder_host: &str,
    downstream_h2: bool,
) -> Result<(), String> {
    let probe = TransportProbe::start(recorder_host, RECORDER_PORT, "ordinary-selected-response").await?;
    let traceparent = crate::access_log_client::traceparent_for_marker(probe.trace_id());
    let controls = [
        ("x-up-behavior", "early-response"),
        ("x-up-at-bytes", "1"),
        ("x-up-drain", "true"),
        ("x-up-status", "200"),
        ("x-up-resp-len", "16"),
        ("traceparent", traceparent.as_str()),
    ];
    if downstream_h2 {
        let mut conn = crate::transport::RawH2Conn::connect(&ctx.target_host, ctx.http_port).await?;
        let (response, mut upload) = conn.send_request(
            "POST",
            &format!("http://{host}:{}/early", ctx.http_port),
            &controls,
            false,
        )?;
        upload
            .send_data(bytes::Bytes::from_static(b"x"), false)
            .map_err(|e| e.to_string())?;
        let response = tokio::time::timeout(Duration::from_secs(5), response)
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        if response.status().as_u16() != 200 {
            return Err("selected H2 response changed".into());
        }
        // A native early-response cancellation may already have closed upload.
        let _ = upload.send_data(bytes::Bytes::from(vec![b'x'; 128]), true);
        let mut response = response.into_body();
        let mut length = 0;
        while let Some(data) = tokio::time::timeout(Duration::from_secs(5), response.data())
            .await
            .map_err(|e| e.to_string())?
        {
            let data = data.map_err(|e| e.to_string())?;
            length += data.len();
            response
                .flow_control()
                .release_capacity(data.len())
                .map_err(|e| e.to_string())?;
        }
        if length != 16 {
            return Err("selected H2 response was not delivered completely".into());
        }
    } else {
        let mut conn = RawH1Conn::connect(&ctx.target_host, ctx.http_port).await?;
        let mut headers = controls.to_vec();
        headers.push(("transfer-encoding", "chunked"));
        conn.write_head("POST", "/early", host, &headers).await?;
        conn.write_chunk(b"x").await?;
        let response = conn.read_response_head(Duration::from_secs(5)).await?;
        if response.status != 200 {
            return Err("selected H1 response changed".into());
        }
        let _ = conn.write_chunk(&[b'x'; 128]).await;
        let _ = conn.finish_chunked().await;
        if conn.read_body(&response, Duration::from_secs(5)).await?.len() != 16 {
            return Err("selected H1 response was not delivered completely".into());
        }
    }
    probe
        .expect_none(
            "oversized tail forwarded after response selection",
            Duration::from_millis(150),
            |event| event.is(KIND_REQ_DATA) && event.len.is_some_and(|length| length >= 128),
        )
        .await?;
    let snapshot = probe.snapshot().await?;
    if snapshot.req_body_total(probe.trace_id()) > 128 {
        return Err("oversized tail reached the upstream".into());
    }
    Ok(())
}
