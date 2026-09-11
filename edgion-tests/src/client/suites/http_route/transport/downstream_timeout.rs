//! Live stalled uploads: keep the client open until the gateway's 60s read deadline.

use super::*;
use crate::metrics_helper::counter_value;

pub(super) const CASE_COUNT: usize = 2;
const EJECTIONS: &str = "edgion_circuit_breaker_ejections_total";

pub(super) fn test_cases() -> Vec<TestCase> {
    [Proto::H1, Proto::H2]
        .into_iter()
        .map(|proto| {
            TestCase::new(
                format!("downstream_body_timeout_408__{}", proto.tag()),
                "An open stalled upload returns 408 without client-close or backend-failure attribution",
                move |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        match run(&ctx, proto).await {
                            Ok(()) => TestResult::passed_with_message(
                                start.elapsed(),
                                "Wire/log 408, BodyReadTimeout, no backend error/ejection; positive control ejected"
                                    .into(),
                            ),
                            Err(error) => TestResult::failed(start.elapsed(), error),
                        }
                    })
                },
            )
            .with_timeout(Duration::from_secs(100))
        })
        .collect()
}

async fn ejections(ctx: &TestContext, service: &str) -> Result<f64, String> {
    let raw = ctx
        .metrics_client()
        .fetch_raw_metrics()
        .await
        .map_err(|e| e.to_string())?;
    Ok(counter_value(
        &raw,
        EJECTIONS,
        &[("service", service), ("class", "local_origin")],
    ))
}

async fn saw_partial_upload(probe: &TransportProbe, proto: Proto) -> Result<(), String> {
    probe
        .wait_for(
            "partial upload reached the selected backend",
            Duration::from_secs(5),
            |e| e.is(KIND_REQ_DATA) && e.tagged(probe.trace_id()) && e.len.unwrap_or(0) > 0,
        )
        .await?;
    let snapshot = probe.snapshot().await?;
    if !snapshot
        .events()
        .iter()
        .any(|e| e.is(KIND_REQ_HEADERS) && e.tagged(probe.trace_id()) && e.listener == proto.listener())
    {
        return Err(format!("wrong upstream transport for {proto:?}: {snapshot}"));
    }
    Ok(())
}

async fn run(ctx: &TestContext, proto: Proto) -> Result<(), String> {
    let name = format!("transport-stall-{}", proto.tag());
    let host = format!("{name}.example.com");
    let service = format!("edgion-test/{name}");
    let recorder_host = env_host_or("EDGION_TEST_TRANSPORT_RECORDER_HOST", &ctx.target_host);
    let probe = TransportProbe::start(&recorder_host, RECORDER_PORT, &name).await?;
    let trace = probe.trace_id();
    let baseline = ejections(ctx, &service).await?;
    let headers = [
        ("x-up-behavior", "echo"),
        ("x-up-drain", "true"),
        ("traceparent", &crate::access_log_client::traceparent_for_marker(trace)),
        ("access_log", "test_store"),
        ("Content-Length", "1024"),
    ];
    let start = Instant::now();
    let status = match proto {
        Proto::H1 => {
            let mut conn = RawH1Conn::connect(&ctx.target_host, ctx.http_port).await?;
            conn.write_head("POST", "/stalled-upload", &host, &headers).await?;
            conn.write_raw(&[b'a'; 48]).await?;
            saw_partial_upload(&probe, proto).await?;
            // No FIN, reset, remaining bytes, or chunk terminator before the response.
            let head = conn.read_response_head(Duration::from_secs(75)).await?;
            conn.read_body(&head, Duration::from_secs(5)).await?;
            if !conn.wait_for_close(Duration::from_secs(5)).await? {
                return Err("H1 timeout connection remained reusable".into());
            }
            head.status
        }
        Proto::H2 => {
            let mut conn = RawH2Conn::connect(&ctx.target_host, ctx.http_port).await?;
            let uri = format!("http://{host}:{}/stalled-upload", ctx.http_port);
            let (response, mut send) = conn.send_request("POST", &uri, &headers, false)?;
            send.send_data(Bytes::from_static(&[b'a'; 48]), false)
                .map_err(|e| e.to_string())?;
            saw_partial_upload(&probe, proto).await?;
            let response = tokio::time::timeout(Duration::from_secs(75), response)
                .await
                .map_err(|_| "H2 response deadline expired")?
                .map_err(|e| format!("H2 response failed: {e}"))?;
            let status = response.status().as_u16();
            let mut body = response.into_body();
            tokio::time::timeout(Duration::from_secs(5), async {
                while let Some(chunk) = body.data().await {
                    let chunk = chunk.map_err(|e| e.to_string())?;
                    body.flow_control()
                        .release_capacity(chunk.len())
                        .map_err(|e| e.to_string())?;
                }
                Ok::<_, String>(())
            })
            .await
            .map_err(|_| "H2 response body did not finish")??;
            // Hold SendStream until the entire reply arrives; dropping it earlier resets the request.
            drop(send);
            conn.shutdown();
            status
        }
    };
    if status != 408 || start.elapsed() < Duration::from_secs(55) || start.elapsed() > Duration::from_secs(85) {
        return Err(format!(
            "expected gateway 60s body-read timeout / 408, got {status} after {:?}",
            start.elapsed()
        ));
    }
    let entry = ctx
        .access_log_client()
        .get_access_log_with_retry(trace, 30, 200)
        .await
        .map_err(|e| format!("timeout access log: {e}"))?;
    if entry.data["request_info"]["status"].as_u64() != Some(408)
        || !entry.has_diagnostic("error", "BodyReadTimeout")
        || entry.has_diagnostic("error", "ClientClosed")
    {
        return Err(format!("incorrect timeout status/diagnostics: {}", entry.data));
    }
    let attempts = entry.data["backend_context"]["upstreams"]
        .as_array()
        .ok_or_else(|| format!("missing backend attempts: {}", entry.data))?;
    if attempts.len() != 1
        || attempts.iter().any(|attempt| {
            attempt
                .get("err")
                .is_some_and(|err| !err.is_null() && err.as_array().is_none_or(|v| !v.is_empty()))
        })
    {
        return Err(format!("client stall attributed to backend or retried: {}", entry.data));
    }
    let snapshot = probe.snapshot().await?;
    require_single_attempt(&snapshot, trace)?;
    if snapshot.req_body_total(trace) != 48
        || snapshot
            .events()
            .iter()
            .any(|e| e.is(KIND_REQ_DATA) && e.tagged(trace) && e.end_stream == Some(true))
    {
        return Err(format!(
            "upload was not the expected unfinished 48-byte prefix: {snapshot}"
        ));
    }
    if ejections(ctx, &service).await? != baseline {
        return Err("client stall incremented local-origin ejections (threshold is one)".into());
    }

    // Positive control: the same policy/endpoint must detect a real backend transport failure.
    // This prevents an absent policy or wrong metric label from making the negative check pass.
    let control_trace = probe.sibling_trace("backend-close");
    let response = ctx
        .http_client
        .post(format!("{}/backend-close", ctx.http_url()))
        .header("Host", &host)
        .header(
            "traceparent",
            crate::access_log_client::traceparent_for_marker(&control_trace),
        )
        .header("x-up-behavior", "reset")
        .header("x-up-reset-code", "INTERNAL_ERROR")
        .header("x-up-at-bytes", "1")
        .body("control")
        .send()
        .await
        .map_err(|e| format!("positive control: {e}"))?;
    if !response.status().is_server_error() {
        return Err(format!("backend close control returned {}", response.status()));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let count = ejections(ctx, &service).await?;
        if count == baseline + 1.0 {
            break;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "positive control did not record exactly one local-origin ejection: {baseline} -> {count}"
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Leave the endpoint recovered so an immediate rerun cannot inherit the control's ejection.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = ctx
            .http_client
            .get(format!("{}/recovered", ctx.http_url()))
            .header("Host", &host)
            .header("x-up-behavior", "echo")
            .send()
            .await
            .map_err(|e| format!("recovery probe: {e}"))?;
        let status = response.status();
        response.bytes().await.map_err(|e| format!("recovery body: {e}"))?;
        if status.as_u16() == 200 {
            break;
        }
        if Instant::now() >= deadline {
            return Err(format!("backend did not recover after positive control: {status}"));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}
