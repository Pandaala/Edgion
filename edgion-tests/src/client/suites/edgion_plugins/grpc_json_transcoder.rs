//! REST/H1/H2 to native-gRPC wire and descriptor lifecycle acceptance.
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::status::helpers;
use edgion_gateway::plugins::http::grpc_json_transcoder::log_consts as log_t;
use std::time::{Duration, Instant};

pub struct GrpcJsonTranscoderTestSuite;
async fn call(
    ctx: &TestContext,
    h2: bool,
    host: &str,
    path: &str,
    body: Option<&str>,
    fixture: Option<&str>,
) -> Result<(u16, String, String), String> {
    call_traced(ctx, h2, host, path, body, fixture, None).await
}
#[allow(clippy::too_many_arguments)]
async fn call_traced(
    ctx: &TestContext,
    h2: bool,
    host: &str,
    path: &str,
    body: Option<&str>,
    fixture: Option<&str>,
    marker: Option<&str>,
) -> Result<(u16, String, String), String> {
    let targets = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::lookup_host((ctx.target_host.as_str(), 31180)),
    )
    .await
    .map_err(|_| "fixture target lookup timed out".to_owned())?
    .map_err(|error| format!("fixture target lookup failed: {error}"))?
    .collect::<Vec<_>>();
    if targets.is_empty() {
        return Err("fixture target resolved to no addresses".into());
    }
    let builder = reqwest::Client::builder()
        .no_proxy()
        .resolve_to_addrs(host, &targets)
        .timeout(Duration::from_secs(10));
    let client = if h2 {
        builder.http2_prior_knowledge()
    } else {
        builder.http1_only()
    }
    .build()
    .map_err(|e| e.to_string())?;
    let url = format!("http://{host}:31180{path}");
    let mut request = if let Some(body) = body {
        client
            .post(url)
            .header("content-type", "application/json")
            .body(body.to_owned())
    } else {
        client.get(url)
    };
    if let Some(fixture) = fixture {
        request = request.header("x-transcoding-fixture", fixture);
    }
    if let Some(marker) = marker {
        request = request
            .header("traceparent", crate::access_log_client::traceparent_for_marker(marker))
            .header("access_log", "test_store");
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    let expected = if h2 {
        reqwest::Version::HTTP_2
    } else {
        reqwest::Version::HTTP_11
    };
    if response.version() != expected {
        return Err(format!("unexpected downstream version {:?}", response.version()));
    }
    let status = response.status().as_u16();
    let media = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() > 1024 * 1024 {
        return Err("fixture response exceeded bound".into());
    }
    Ok((
        status,
        String::from_utf8(bytes.to_vec()).map_err(|e| e.to_string())?,
        media,
    ))
}
fn unary_case(name: &'static str, h2: bool, host: &'static str) -> TestCase {
    TestCase::new(
        name,
        "REST client receives ProtoJSON from an upstream Service without appProtocol",
        move |ctx| {
            Box::pin(async move {
                let start = Instant::now();
                let result = call(&ctx, h2, host, "/v1/hello", Some(r#"{"name":"Transcoder"}"#), None)
                    .await
                    .and_then(|(status, body, media)| {
                        let json: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
                        if status != 200
                            || media != "application/json"
                            || !json["message"]
                                .as_str()
                                .is_some_and(|m| m.starts_with("Hello, Transcoder!"))
                            || !json["serverAddr"].is_string()
                        {
                            return Err(format!("unexpected unary reply: {status} {media} {body}"));
                        }
                        Ok(())
                    });
                match result {
                    Ok(()) => TestResult::passed(start.elapsed()),
                    Err(e) => TestResult::failed(start.elapsed(), e),
                }
            })
        },
    )
}
impl TestSuite for GrpcJsonTranscoderTestSuite {
    fn name(&self) -> &str {
        "GrpcJsonTranscoder"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let mut cases = vec![
            unary_case(
                "transcoder_unary_http11_inline",
                false,
                "grpc-transcoder-inline.example.com",
            ),
            unary_case(
                "transcoder_unary_http2_configmap",
                true,
                "grpc-transcoder-configmap.example.com",
            ),
        ];
        for name in [
            "grpc-body-before-open",
            "grpc-body-before-closed",
            "grpc-body-after-open",
            "grpc-body-after-closed",
            "grpc-backend-guard-open",
            "grpc-backend-guard-closed",
            "grpc-rule-bandwidth",
            "grpc-backend-bandwidth",
            "grpc-rule-wasm-body",
            "grpc-backend-wasm-body",
            "grpc-late-conflict",
        ] {
            cases.push(TestCase::new(
                name,
                "Incompatible owners fail before origin; late header conflicts cannot publish replacement",
                move |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let result = async {
                            let (status, body, _) = call_traced(
                                &ctx,
                                false,
                                &format!("{name}.example.com"),
                                "/v1/hello",
                                Some(r#"{"name":"ownership"}"#),
                                None,
                                Some(name),
                            )
                            .await?;
                            if !matches!(status, 500 | 502) || body.contains("Hello,") {
                                return Err(format!("owner accepted: {status} {body}"));
                            }
                            let entry = ctx
                                .access_log_client()
                                .get_access_log_with_retry(name, 15, 200)
                                .await
                                .map_err(|e| e.to_string())?;
                            let expected = usize::from(name == "grpc-late-conflict");
                            if entry.upstream_count() != expected {
                                return Err(format!(
                                    "expected {expected} origin attempts, got {}: {}",
                                    entry.upstream_count(),
                                    entry.data
                                ));
                            }
                            if name == "grpc-late-conflict" && !entry.data.to_string().contains(log_t::OWNER_CONFLICT) {
                                return Err("late conflict missing from access log".into());
                            }
                            Ok(())
                        }
                        .await;
                        match result {
                            Ok(()) => TestResult::passed(start.elapsed()),
                            Err(e) => TestResult::failed(start.elapsed(), e),
                        }
                    })
                },
            ));
        }
        for (name, host) in [
            ("transcoder_header_only_backend", "grpc-backend-header-only.example.com"),
            ("transcoder_extproc_headers_before", "grpc-headers-before.example.com"),
            ("transcoder_extproc_headers_after", "grpc-headers-after.example.com"),
        ] {
            cases.push(unary_case(name, false, host));
        }
        for (mode, kind, code) in [
            ("missing-status", log_t::INVALID_STATUS, None),
            ("bad-status", log_t::INVALID_STATUS, None),
            ("error-status", log_t::RPC_FAILURE, Some(7)),
            ("success", log_t::COMPLETED, Some(0)),
        ] {
            cases.push(TestCase::new(
                format!("transcoder_stream_log_{mode}"),
                "One bounded final outcome survives stream cleanup",
                move |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let marker = format!("transcoder-stream-log-{mode}");
                        let result = async {
                            let (status, body, _) = call_traced(
                                &ctx,
                                false,
                                "grpc-transcoder-raw.example.com",
                                "/v1/numbers",
                                None,
                                Some(mode),
                                Some(&marker),
                            )
                            .await?;
                            if status != 200 || body.contains("private fixture") {
                                return Err(format!("unexpected stream: {status} {body}"));
                            }
                            let entry = ctx
                                .access_log_client()
                                .get_access_log_with_retry(&marker, 15, 200)
                                .await
                                .map_err(|e| e.to_string())?;
                            if entry.data["request_info"]["grpc_status"].as_u64() != code {
                                return Err(format!("wrong status evidence: {}", entry.data));
                            }
                            let log = entry.data.to_string();
                            if log.matches(kind).count() != 1 || log.contains("private fixture") {
                                return Err(format!("wrong terminal log: {log}"));
                            }
                            Ok(())
                        }
                        .await;
                        match result {
                            Ok(()) => TestResult::passed(start.elapsed()),
                            Err(e) => TestResult::failed(start.elapsed(), e),
                        }
                    })
                },
            ));
        }
        for (name, host, status) in [
            (
                "transcoder_streamed_handler_before",
                "grpc-streamed-before.example.com",
                500,
            ),
            (
                "transcoder_streamed_handler_after_failopen",
                "grpc-streamed-after.example.com",
                502,
            ),
        ] {
            cases.push(TestCase::new(
                name,
                "Mutable request-stream handlers cannot bypass sealed RPC ownership",
                move |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        match call(&ctx, false, host, "/v1/hello", Some(r#"{"name":"sealed"}"#), None).await {
                            Ok((actual, _, media)) if actual == status && media == "application/json" => {
                                TestResult::passed(start.elapsed())
                            }
                            other => {
                                TestResult::failed(start.elapsed(), format!("unexpected ownership reply: {other:?}"))
                            }
                        }
                    })
                },
            ));
        }
        for (name, h2, host, sse) in [
            (
                "transcoder_ndjson_http11",
                false,
                "grpc-transcoder-inline.example.com",
                false,
            ),
            ("transcoder_sse_http2", true, "grpc-transcoder-sse.example.com", true),
        ] {
            cases.push(TestCase::new(name, "Server streaming emits each message and one validated terminal", move |ctx| Box::pin(async move {
                let start = Instant::now();
                let expected = if sse { "event: rpc_message\ndata: {\"number\":1}\n\nevent: rpc_message\ndata: {\"number\":2}\n\nevent: rpc_end\ndata: {}\n\n" } else { "{\"type\":\"message\",\"data\":{\"number\":1}}\n{\"type\":\"message\",\"data\":{\"number\":2}}\n{\"type\":\"end\"}\n" };
                match call(&ctx, h2, host, "/v1/numbers?count=2", None, None).await {
                    Ok((200, body, _)) if body == expected => TestResult::passed(start.elapsed()), other => TestResult::failed(start.elapsed(), format!("unexpected streaming reply: {other:?}")),
                }
            })));
        }
        for (name, host, path, body, status) in [
            (
                "transcoder_missing_mapping",
                "grpc-transcoder-inline.example.com",
                "/unmapped",
                None,
                404,
            ),
            (
                "transcoder_encoded_payload_limit",
                "grpc-transcoder-small.example.com",
                "/v1/hello",
                Some(r#"{"name":"valid JSON over protobuf cap"}"#),
                413,
            ),
        ] {
            cases.push(TestCase::new(
                name,
                "Request error classification matches the public conversion contract",
                move |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        match call(&ctx, false, host, path, body, None).await {
                            Ok((actual, _, media)) if actual == status && media == "application/json" => {
                                TestResult::passed(start.elapsed())
                            }
                            other => {
                                TestResult::failed(start.elapsed(), format!("unexpected request rejection: {other:?}"))
                            }
                        }
                    })
                },
            ));
        }
        cases.push(TestCase::new(
            "transcoder_rejects_duplicate_json",
            "Invalid JSON never dispatches a protobuf request",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match call(
                        &ctx,
                        false,
                        "grpc-transcoder-inline.example.com",
                        "/v1/hello",
                        Some(r#"{"name":"a","name":"b"}"#),
                        None,
                    )
                    .await
                    {
                        Ok((400, _, media)) if media == "application/json" => TestResult::passed(start.elapsed()),
                        other => TestResult::failed(start.elapsed(), format!("unexpected rejection: {other:?}")),
                    }
                })
            },
        ));
        for mode in ["missing-status", "bad-status", "truncated", "compressed"] {
            cases.push(TestCase::new(
                format!("transcoder_unary_{mode}"),
                "Malformed native gRPC fails before committing success",
                move |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        match call(
                            &ctx,
                            false,
                            "grpc-transcoder-raw.example.com",
                            "/v1/hello/name",
                            None,
                            Some(mode),
                        )
                        .await
                        {
                            Ok((502, body, media))
                                if media == "application/json" && !body.contains("private fixture") =>
                            {
                                TestResult::passed(start.elapsed())
                            }
                            other => {
                                TestResult::failed(start.elapsed(), format!("unexpected malformed reply: {other:?}"))
                            }
                        }
                    })
                },
            ));
        }
        cases.push(TestCase::new(
            "transcoder_grpc_status_mapping",
            "Actual nonzero status maps to a fixed REST error",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match call(
                        &ctx,
                        true,
                        "grpc-transcoder-raw.example.com",
                        "/v1/hello/name",
                        None,
                        Some("error-status"),
                    )
                    .await
                    {
                        Ok((403, body, media)) if media == "application/json" && !body.contains("private fixture") => {
                            TestResult::passed(start.elapsed())
                        }
                        other => TestResult::failed(start.elapsed(), format!("unexpected error mapping: {other:?}")),
                    }
                })
            },
        ));
        for h2 in [false, true] {
            for (mode, status) in [
                ("header-only-error", 403),
                ("header-only-missing", 502),
                ("header-only-invalid", 502),
            ] {
                cases.push(TestCase::new(
                    format!("transcoder_{mode}_{}", if h2 { "h2" } else { "h1" }),
                    "True HEADERS END_STREAM maps or fails before unary commit",
                    move |ctx| {
                        Box::pin(async move {
                            let start = Instant::now();
                            match call(
                                &ctx,
                                h2,
                                "grpc-transcoder-raw.example.com",
                                "/v1/hello/name",
                                None,
                                Some(mode),
                            )
                            .await
                            {
                                Ok((actual, _, media)) if actual == status && media == "application/json" => {
                                    TestResult::passed(start.elapsed())
                                }
                                other => TestResult::failed(
                                    start.elapsed(),
                                    format!("unexpected header-only reply: {other:?}"),
                                ),
                            }
                        })
                    },
                ));
            }
        }
        cases.push(TestCase::new(
            "transcoder_stream_missing_status",
            "EOF has an error terminal and never a success terminal",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match call(
                        &ctx,
                        true,
                        "grpc-transcoder-raw.example.com",
                        "/v1/numbers",
                        None,
                        Some("missing-status"),
                    )
                    .await
                    {
                        Ok((200, body, _))
                            if body.contains("\"type\":\"error\"") && !body.contains("\"type\":\"end\"") =>
                        {
                            TestResult::passed(start.elapsed())
                        }
                        other => {
                            TestResult::failed(start.elapsed(), format!("unexpected missing-status stream: {other:?}"))
                        }
                    }
                })
            },
        ));
        cases.push(TestCase::new(
            "transcoder_tls_h1_fallback_and_pool_isolation",
            "A proven TLS/H1 origin cannot serve a mandatory-H2 RPC",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    for _ in 0..2 {
                        match call(
                            &ctx,
                            false,
                            "grpc-transcoder-h1-control.example.com",
                            "/echo",
                            None,
                            None,
                        )
                        .await
                        {
                            Ok((200, _, _)) => {}
                            other => {
                                return TestResult::failed(start.elapsed(), format!("TLS/H1 control failed: {other:?}"))
                            }
                        }
                        match call(
                            &ctx,
                            false,
                            "grpc-transcoder-tls-h1.example.com",
                            "/v1/hello/name",
                            None,
                            None,
                        )
                        .await
                        {
                            Ok((502, _, media)) if media == "application/json" => {}
                            other => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("H1 fallback was not rejected: {other:?}"),
                                )
                            }
                        }
                    }
                    TestResult::passed(start.elapsed())
                })
            },
        ));
        cases
    }
}

pub struct GrpcJsonTranscoderDynamicSuite {
    pub valid: bool,
    pub timeout_short: bool,
}
impl TestSuite for GrpcJsonTranscoderDynamicSuite {
    fn name(&self) -> &str {
        "GrpcJsonTranscoder descriptor reload"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let valid = self.valid;
        let timeout_short = self.timeout_short;
        vec![TestCase::new(
            "transcoder_descriptor_generation",
            "ConfigMap apply/delete replaces a mixed DSL/transcoder generation",
            move |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let resource = match helpers::fetch_resource_status(
                        &ctx,
                        "EdgionPlugins",
                        "edgion-default",
                        "grpc-transcoder-dynamic",
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let accepted =
                        helpers::controller_local_conditions(&resource["status"]).is_some_and(|conditions| {
                            conditions.iter().any(|c| {
                                c["type"] == "Accepted" && c["status"] == (if valid { "True" } else { "False" })
                            })
                        });
                    if !accepted {
                        return TestResult::failed(start.elapsed(), "descriptor status has not converged".into());
                    }
                    match call(
                        &ctx,
                        false,
                        "grpc-transcoder-dynamic.example.com",
                        "/v1/hello/name",
                        None,
                        None,
                    )
                    .await
                    {
                        Ok((status, body, _))
                            if status == (if valid { 200 } else { 500 })
                                && (!valid || body.contains("Hello, name!")) =>
                        {
                            if valid {
                                for h2 in [false, true] {
                                    match call(
                                        &ctx,
                                        h2,
                                        "grpc-transcoder-dynamic.example.com",
                                        "/v1/hello/name",
                                        None,
                                        Some("deadline-probe"),
                                    )
                                    .await
                                    {
                                        // Owner expiry fails closed with 502; an upstream I/O
                                        // timeout or native DEADLINE_EXCEEDED maps to 504.
                                        Ok((status, _, _))
                                            if if timeout_short {
                                                matches!(status, 502 | 504)
                                            } else {
                                                status == 200
                                            } => {}
                                        other => {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                format!("timeout generation has not converged (h2={h2}): {other:?}"),
                                            )
                                        }
                                    }
                                }
                            }
                            TestResult::passed(start.elapsed())
                        }
                        other => {
                            TestResult::failed(start.elapsed(), format!("unexpected descriptor generation: {other:?}"))
                        }
                    }
                })
            },
        )]
    }
}
