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
