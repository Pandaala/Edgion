//! JSON snapshot admission, exact forwarding and hot-reload wire assertions.

use super::*;
use crate::framework::env_host_or;
use crate::transport::{RawH1Conn, TransportProbe, KIND_REQ_DATA, KIND_REQ_HEADERS, RECORDER_PORT};
use sha2::{Digest, Sha256};

pub(super) async fn no_forward(probe: &TransportProbe) -> Result<(), String> {
    let snapshot = probe.snapshot().await?;
    if snapshot.dropped != 0
        || snapshot.count_matching(|event| event.is(KIND_REQ_HEADERS) || event.is(KIND_REQ_DATA)) != 0
    {
        return Err(format!("JSON rejection exposed upstream bytes: {snapshot}"));
    }
    probe
        .expect_none("JSON upstream headers or body", Duration::from_millis(150), |event| {
            event.is(KIND_REQ_HEADERS) || event.is(KIND_REQ_DATA)
        })
        .await
}

async fn verify(
    ctx: &TestContext,
    upstream_h2: bool,
    downstream_h2: bool,
    version: &str,
    path: &str,
    body: &[u8],
    expected_status: u16,
) -> Result<(), String> {
    verify_structured(
        ctx,
        upstream_h2,
        downstream_h2,
        version,
        path,
        StructuredBody {
            content_type: "application/json",
            bytes: body,
        },
        expected_status,
    )
    .await
}

pub(super) struct StructuredBody<'a> {
    pub(super) content_type: &'a str,
    pub(super) bytes: &'a [u8],
}

pub(super) async fn verify_structured(
    ctx: &TestContext,
    upstream_h2: bool,
    downstream_h2: bool,
    version: &str,
    path: &str,
    body: StructuredBody<'_>,
    expected_status: u16,
) -> Result<(), String> {
    verify_structured_with_user_agent(
        ctx,
        upstream_h2,
        downstream_h2,
        version,
        path,
        body,
        expected_status,
        None,
    )
    .await
}

/// Optional request context for full-profile tests; legacy callers retain their exact headers.
#[allow(clippy::too_many_arguments)]
pub(super) async fn verify_structured_with_user_agent(
    ctx: &TestContext,
    upstream_h2: bool,
    downstream_h2: bool,
    version: &str,
    path: &str,
    body: StructuredBody<'_>,
    expected_status: u16,
    user_agent: Option<&str>,
) -> Result<(), String> {
    let StructuredBody {
        content_type,
        bytes: body,
    } = body;
    let recorder = env_host_or("EDGION_TEST_TRANSPORT_RECORDER_HOST", &ctx.target_host);
    let probe = TransportProbe::start(&recorder, RECORDER_PORT, "waf-json").await?;
    let host = format!("waf-transport-h{}.example.com", if upstream_h2 { 2 } else { 1 });
    let address = tokio::net::lookup_host((ctx.target_host.as_str(), ctx.http_port))
        .await
        .map_err(|error| format!("resolve JSON test target: {error}"))?
        .next()
        .ok_or_else(|| "JSON test target has no address".to_string())?;
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .resolve(&host, address)
        .timeout(Duration::from_secs(10));
    builder = if downstream_h2 {
        builder.http2_prior_knowledge()
    } else {
        builder.http1_only()
    };
    if let Some(agent) = user_agent {
        builder = builder.user_agent(agent);
    }
    let response = builder
        .build()
        .map_err(|error| error.to_string())?
        .post(format!("http://{host}:{}{path}", ctx.http_port))
        .header(
            "traceparent",
            crate::access_log_client::traceparent_for_marker(probe.trace_id()),
        )
        .header("x-waf-version", version)
        .header("content-type", content_type)
        .body(body.to_vec())
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status().as_u16() != expected_status {
        return Err(format!(
            "JSON {version} expected {expected_status}, got {}",
            response.status()
        ));
    }
    if expected_status != 200 {
        response.bytes().await.map_err(|error| error.to_string())?;
        return no_forward(&probe).await;
    }
    let length = body.len().to_string();
    let digest = format!("{:x}", Sha256::digest(body));
    for (name, expected) in [
        ("x-up-req-body-len", length.as_str()),
        ("x-up-req-body-sha256", digest.as_str()),
        ("x-up-http-version", if upstream_h2 { "2" } else { "1.1" }),
    ] {
        if response.headers().get(name).and_then(|value| value.to_str().ok()) != Some(expected) {
            return Err(format!(
                "JSON {name} expected {expected}, got {:?}",
                response.headers().get(name)
            ));
        }
    }
    response.bytes().await.map_err(|error| error.to_string())?;
    let snapshot = probe.snapshot().await?;
    if snapshot.dropped != 0
        || snapshot.count_matching(|event| event.is(KIND_REQ_HEADERS)) != 1
        || snapshot.req_body_total(probe.trace_id()) != body.len()
    {
        return Err(format!("JSON expected one unchanged upstream request: {snapshot}"));
    }
    Ok(())
}

pub(crate) struct PausedJsonUpload {
    inner: PausedStructuredUpload,
}
impl PausedJsonUpload {
    pub(crate) async fn start(ctx: &TestContext) -> Result<Self, String> {
        Ok(Self {
            inner: PausedStructuredUpload::start(
                ctx,
                "application/json",
                "json-v1",
                br#"{"a":"blo\u00"#,
                br#"63ked"}"#,
                406,
            )
            .await?,
        })
    }
    pub(crate) async fn finish(self) -> Result<(), String> {
        self.inner.finish().await
    }
    async fn abort(self) -> Result<(), String> {
        self.inner.abort().await
    }
}

pub(crate) struct PausedStructuredUpload {
    connection: RawH1Conn,
    probe: TransportProbe,
    last: &'static [u8],
    expected_status: u16,
}
impl PausedStructuredUpload {
    pub(crate) async fn start(
        ctx: &TestContext,
        content_type: &str,
        version: &str,
        first: &[u8],
        last: &'static [u8],
        expected_status: u16,
    ) -> Result<Self, String> {
        if expected_status == 200 {
            return Err("Paused rejection helper requires a denied response".to_string());
        }
        let recorder = env_host_or("EDGION_TEST_TRANSPORT_RECORDER_HOST", &ctx.target_host);
        let probe = TransportProbe::start(&recorder, RECORDER_PORT, "waf-structured-pinned").await?;
        let traceparent = crate::access_log_client::traceparent_for_marker(probe.trace_id());
        let mut connection = RawH1Conn::connect(&ctx.target_host, ctx.http_port).await?;
        connection
            .write_head(
                "POST",
                "/waf-wire",
                "waf-transport-h1.example.com",
                &[
                    ("transfer-encoding", "chunked"),
                    ("expect", "100-continue"),
                    ("content-type", content_type),
                    ("x-waf-version", version),
                    ("traceparent", &traceparent),
                ],
            )
            .await?;
        let interim = connection.read_interim(Duration::from_secs(5)).await?;
        if interim.as_ref().map(|head| head.status) != Some(100) {
            return Err(format!("Paused upload expected local 100 Continue, got {interim:?}"));
        }
        // Continue pins the generation after Phase 1. EOF remains outstanding.
        connection.write_chunk(first).await?;
        no_forward(&probe).await?;
        Ok(Self {
            connection,
            probe,
            last,
            expected_status,
        })
    }
    pub(crate) async fn finish(mut self) -> Result<(), String> {
        self.connection.write_chunk(self.last).await?;
        no_forward(&self.probe).await?;
        self.connection.finish_chunked().await?;
        let response = self.connection.read_response_head(Duration::from_secs(10)).await?;
        if response.status != self.expected_status {
            return Err(format!(
                "Pinned split upload expected {}, got {response}",
                self.expected_status
            ));
        }
        no_forward(&self.probe).await
    }
    pub(crate) async fn abort(self) -> Result<(), String> {
        self.connection.abort();
        no_forward(&self.probe).await
    }
}

async fn duplicate_occurrences_across_chunks_are_denied(ctx: &TestContext) -> Result<(), String> {
    let recorder = env_host_or("EDGION_TEST_TRANSPORT_RECORDER_HOST", &ctx.target_host);
    let probe = TransportProbe::start(&recorder, RECORDER_PORT, "waf-json-duplicate-chunks").await?;
    let traceparent = crate::access_log_client::traceparent_for_marker(probe.trace_id());
    let mut connection = RawH1Conn::connect(&ctx.target_host, ctx.http_port).await?;
    connection
        .write_head(
            "POST",
            "/waf-wire",
            "waf-transport-h1.example.com",
            &[
                ("transfer-encoding", "chunked"),
                ("content-type", "application/json"),
                ("x-waf-version", "json-v1"),
                ("traceparent", &traceparent),
            ],
        )
        .await?;
    // Every duplicate-key occurrence is inspected, so the first one denies
    // even though a later occurrence in another chunk is safe.
    let first = br#"{"a":"blocked","#;
    let last = br#""a":"safe"}"#;
    connection.write_chunk(first).await?;
    no_forward(&probe).await?;
    connection.write_chunk(last).await?;
    no_forward(&probe).await?;
    connection.finish_chunked().await?;
    let response = connection.read_response_head(Duration::from_secs(10)).await?;
    if response.status != 406 {
        return Err(format!(
            "JSON cross-chunk duplicate key expected 406 for its first occurrence, got {response}"
        ));
    }
    no_forward(&probe).await
}

async fn allowed_body_across_chunks_is_forwarded_byte_exact(ctx: &TestContext) -> Result<(), String> {
    let recorder = env_host_or("EDGION_TEST_TRANSPORT_RECORDER_HOST", &ctx.target_host);
    let probe = TransportProbe::start(&recorder, RECORDER_PORT, "waf-json-allowed-chunks").await?;
    let traceparent = crate::access_log_client::traceparent_for_marker(probe.trace_id());
    let mut connection = RawH1Conn::connect(&ctx.target_host, ctx.http_port).await?;
    connection
        .write_head(
            "POST",
            "/waf-wire",
            "waf-transport-h1.example.com",
            &[
                ("transfer-encoding", "chunked"),
                ("content-type", "application/json"),
                ("x-waf-version", "json-v1"),
                ("traceparent", &traceparent),
            ],
        )
        .await?;
    // A benign body split across two chunks is replayed upstream unchanged.
    let first = br#"{"a":"safe","#;
    let last = br#""b":"safe"}"#;
    connection.write_chunk(first).await?;
    no_forward(&probe).await?;
    connection.write_chunk(last).await?;
    no_forward(&probe).await?;
    connection.finish_chunked().await?;
    let response = connection.read_response_head(Duration::from_secs(10)).await?;
    let body = [first.as_slice(), last.as_slice()].concat();
    let length = body.len().to_string();
    let digest = format!("{:x}", Sha256::digest(&body));
    if response.status != 200
        || response.header("x-up-req-body-len") != Some(length.as_str())
        || response.header("x-up-req-body-sha256") != Some(digest.as_str())
    {
        return Err(format!(
            "JSON cross-chunk upload did not preserve allowed wire bytes: {response}"
        ));
    }
    let snapshot = probe.snapshot().await?;
    if snapshot.dropped != 0
        || snapshot.count_matching(|event| event.is(KIND_REQ_HEADERS)) != 1
        || snapshot.req_body_total(probe.trace_id()) != body.len()
    {
        return Err(format!(
            "JSON cross-chunk upload expected one unchanged request: {snapshot}"
        ));
    }
    Ok(())
}

pub(super) fn matrix(version: &'static str, detection: bool, zero_work: bool) -> TestCase {
    TestCase::new(
        match (version, detection, zero_work) {
            (_, _, true) => "waf_json_zero_work_wire",
            (_, true, _) => "waf_json_detection_wire",
            ("json-v1", _, _) => "waf_json_v1_wire",
            _ => "waf_json_v2_wire",
        },
        "JSON admission preserves wire bytes or rejects before backend exposure across H1/H2",
        move |ctx: TestContext| {
            Box::pin(async move {
                let started = Instant::now();
                let run = async {
                    let marker = if version == "json-v2" { "forbidden" } else { "blocked" };
                    let positive = format!("{{\"a\":\"{marker}\"}}");
                    let first_hit = format!("{{\"a\":\"{marker}\",\"a\":\"safe\"}}");
                    let last_hit = format!("{{\"a\":\"safe\",\"a\":\"{marker}\"}}");
                    let array = format!("[\"{marker}\"]");
                    let mut large = br#"{"padding":""#.to_vec();
                    large.resize(192 * 1024, b'x');
                    large.extend_from_slice(br#""}"#);
                    for downstream_h2 in [false, true] {
                        for upstream_h2 in [false, true] {
                            let allowed = if zero_work { 500 } else { 200 };
                            let denied = if zero_work {
                                500
                            } else if detection {
                                200
                            } else {
                                406
                            };
                            verify(
                                &ctx,
                                upstream_h2,
                                downstream_h2,
                                version,
                                "/waf-wire",
                                br#"{"a":"safe"}"#,
                                allowed,
                            )
                            .await?;
                            verify(
                                &ctx,
                                upstream_h2,
                                downstream_h2,
                                version,
                                "/waf-wire",
                                positive.as_bytes(),
                                denied,
                            )
                            .await?;
                            if zero_work {
                                continue;
                            }
                            verify(
                                &ctx,
                                upstream_h2,
                                downstream_h2,
                                version,
                                "/waf-wire",
                                first_hit.as_bytes(),
                                denied,
                            )
                            .await?;
                            verify(
                                &ctx,
                                upstream_h2,
                                downstream_h2,
                                version,
                                "/waf-wire",
                                last_hit.as_bytes(),
                                denied,
                            )
                            .await?;
                            verify(
                                &ctx,
                                upstream_h2,
                                downstream_h2,
                                version,
                                "/waf-wire",
                                array.as_bytes(),
                                denied,
                            )
                            .await?;
                            verify(
                                &ctx,
                                upstream_h2,
                                downstream_h2,
                                version,
                                "/waf-wire",
                                br#"{"a":"safe""#,
                                if detection {
                                    200
                                } else if version == "json-v2" {
                                    422
                                } else {
                                    400
                                },
                            )
                            .await?;
                            verify(
                                &ctx,
                                upstream_h2,
                                downstream_h2,
                                version,
                                "/waf-wire?q=query-deny",
                                positive.as_bytes(),
                                if detection { 200 } else { 409 },
                            )
                            .await?;
                            verify(&ctx, upstream_h2, downstream_h2, version, "/waf-wire", &large, 200).await?;
                            // A fresh V2 request permits the old generation's marker.
                            if version == "json-v2" {
                                verify(
                                    &ctx,
                                    upstream_h2,
                                    downstream_h2,
                                    version,
                                    "/waf-wire",
                                    br#"{"a":"blocked"}"#,
                                    200,
                                )
                                .await?;
                            }
                            // Wrong versions exercise ordinary Phase 1 before capture.
                            if !detection {
                                verify(
                                    &ctx,
                                    upstream_h2,
                                    downstream_h2,
                                    "stale-json",
                                    "/waf-wire",
                                    br#"{"a":"safe"}"#,
                                    451,
                                )
                                .await?;
                            }
                        }
                    }
                    if version == "json-v1" && !detection && !zero_work {
                        allowed_body_across_chunks_is_forwarded_byte_exact(&ctx).await?;
                        duplicate_occurrences_across_chunks_are_denied(&ctx).await?;
                        PausedJsonUpload::start(&ctx).await?.finish().await?;
                        PausedJsonUpload::start(&ctx).await?.abort().await?;
                        // A cancelled capture must not poison an independent request.
                        verify(&ctx, false, false, version, "/waf-wire", br#"{"a":"safe"}"#, 200).await?;
                    }
                    Ok::<(), String>(())
                };
                match tokio::time::timeout(Duration::from_secs(90), run).await {
                    Ok(Ok(())) => TestResult::passed(started.elapsed()),
                    Ok(Err(error)) => TestResult::failed(started.elapsed(), error),
                    Err(_) => {
                        TestResult::failed(started.elapsed(), "JSON wire matrix exceeded its deadline".to_string())
                    }
                }
            })
        },
    )
}
