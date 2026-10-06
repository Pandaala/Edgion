//! Real-Gateway H2 lifecycle contracts. Recorder gates establish ordering;
//! backend wire proofs establish what the scripted controls actually emit.

use super::{Combo, Proto};
use crate::framework::{env_host_or, TestCase, TestContext, TestResult};
use crate::transport::{
    TransportEvent, TransportProbe, TransportSnapshot, KIND_PEER_RESET, KIND_REQ_DATA, KIND_REQ_HEADERS,
    KIND_RESP_DONE, KIND_RESP_HEAD, LISTENER_H2, RECORDER_PORT,
};
use futures::StreamExt;
use std::time::{Duration, Instant};

pub(super) const CASE_COUNT: usize = 18;
const RESPONSE_LEN: usize = 64;
const UPLOAD_LEN: usize = 1024 * 1024;
const UPLOAD_WINDOW: usize = 65_535;
const HOST: &str = "transport-h2.example.com";
const WAIT: Duration = Duration::from_secs(5);

pub(super) fn test_cases() -> Vec<TestCase> {
    let mut cases = Vec::new();
    for combo in Combo::UP_H2 {
        for mode in [
            "stall",
            "goaway-descending",
            "goaway-excluding",
            "goaway-raising",
            "goaway-malformed",
            "goaway-nonzero",
            "goaway-truncated",
            "shutdown",
        ] {
            cases.push(
                TestCase::new(
                    format!("h2_lifecycle__{mode}__{}", combo.suffix()),
                    "Controlled H2 lifecycle trigger, bounded outcome, and upstream connection identity",
                    move |ctx| Box::pin(run(ctx, combo, mode)),
                )
                .with_timeout(Duration::from_secs(40)),
            );
        }
        // A second stall on the surviving pool proves its returned capacity is
        // usable, rather than merely proving one request completed.
        cases.push(
            TestCase::new(
                format!("h2_lifecycle__stall_reuse__{}", combo.suffix()),
                "Two window-exhausted uploads complete on the same healthy H2 connection",
                move |ctx| Box::pin(run(ctx, combo, "stall-reuse")),
            )
            .with_timeout(Duration::from_secs(40)),
        );
    }
    assert_eq!(cases.len(), CASE_COUNT, "H2 lifecycle matrix must not silently shrink");
    cases
}

#[derive(Debug)]
struct Reply {
    status: u16,
    body: Vec<u8>,
}

async fn collect(response: reqwest::Response) -> Result<Reply, String> {
    let status = response.status().as_u16();
    let mut chunks = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(exchange_error)?;
        if body.len() + chunk.len() > 4096 {
            return Err("lifecycle response exceeded the test's 4096-byte observation cap".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Reply { status, body })
}

async fn request(
    client: &reqwest::Client,
    ctx: &TestContext,
    trace: &str,
    path: &str,
    mode: &str,
    release_trace: Option<&str>,
    body: Vec<u8>,
) -> Result<Reply, String> {
    let mut request = client
        .post(format!("http://{HOST}:{}{path}", ctx.http_port))
        .header("traceparent", crate::access_log_client::traceparent_for_marker(trace))
        .header("x-up-resp-len", RESPONSE_LEN.to_string())
        .header("x-up-behavior", if mode == "hang" { "hang" } else { "echo" })
        .body(body);
    if !matches!(mode, "echo" | "hang") {
        request = request.header("x-up-lifecycle", mode);
    }
    if let Some(release_trace) = release_trace {
        request = request.header(
            "x-up-release-trace",
            crate::access_log_client::trace_id_for_marker(release_trace),
        );
    }
    collect(request.send().await.map_err(exchange_error)?).await
}

fn exchange_error(error: reqwest::Error) -> String {
    if error.is_timeout() {
        format!("test-client deadline expired: {error}")
    } else {
        format!("peer transport failure: {error}")
    }
}

fn full_response(reply: &Reply) -> Result<(), String> {
    expected_response(reply, 200)
}

fn expected_response(reply: &Reply, status: u16) -> Result<(), String> {
    let pattern = b"edgion-transport-";
    let expected: Vec<_> = (0..RESPONSE_LEN).map(|index| pattern[index % pattern.len()]).collect();
    if reply.status != status || reply.body != expected {
        return Err(format!(
            "expected complete {status} with {RESPONSE_LEN} scripted bytes, got {reply:?}"
        ));
    }
    Ok(())
}

fn one_attempt(snapshot: &TransportSnapshot, trace: &str) -> Result<(), String> {
    if snapshot.dropped != 0 {
        return Err(format!("lifecycle recorder lost events\n{snapshot}"));
    }
    let count = snapshot.count_matching(|event| event.is(KIND_REQ_HEADERS) && event.tagged(trace));
    if count != 1 {
        return Err(format!(
            "expected one upstream attempt for {trace}, saw {count}\n{snapshot}"
        ));
    }
    Ok(())
}

async fn head(probe: &TransportProbe, trace: &str) -> Result<TransportEvent, String> {
    let head = probe
        .wait_for_trace(trace, "H2 upstream request headers", WAIT, |event| {
            event.is(KIND_REQ_HEADERS) && event.tagged(trace)
        })
        .await?;
    if head.listener != LISTENER_H2 {
        return Err(format!("lifecycle request used the wrong upstream: {head}"));
    }
    Ok(head)
}

async fn run(ctx: TestContext, combo: Combo, mode: &'static str) -> TestResult {
    let start = Instant::now();
    match exercise(&ctx, combo, mode).await {
        Ok(message) => TestResult::passed_with_message(start.elapsed(), message),
        Err(error) => TestResult::failed(start.elapsed(), error),
    }
}

async fn exercise(ctx: &TestContext, combo: Combo, mode: &str) -> Result<String, String> {
    let recorder = env_host_or("EDGION_TEST_TRANSPORT_RECORDER_HOST", &ctx.target_host);
    let probe = TransportProbe::start(&recorder, RECORDER_PORT, &format!("h2-lifecycle-{mode}")).await?;
    let address = tokio::net::lookup_host((ctx.target_host.as_str(), ctx.http_port))
        .await
        .map_err(|error| error.to_string())?
        .next()
        .ok_or("gateway address resolved to no IP")?;
    let builder = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .resolve(HOST, address);
    let client = match combo.down {
        Proto::H1 => builder.http1_only(),
        Proto::H2 => builder.http2_prior_knowledge(),
    }
    .build()
    .map_err(|error| error.to_string())?;
    if mode.starts_with("stall") {
        return stalled_upload(ctx, &client, &probe, mode == "stall-reuse").await;
    }
    retired_connection(ctx, &client, &probe, mode).await
}

async fn stall_once(
    ctx: &TestContext,
    client: &reqwest::Client,
    probe: &TransportProbe,
    trace: &str,
) -> Result<TransportEvent, String> {
    let started = Instant::now();
    let reply = request(
        client,
        ctx,
        trace,
        "/lifecycle/stall",
        "stall",
        None,
        vec![b'u'; UPLOAD_LEN],
    )
    .await?;
    full_response(&reply)?;
    let exhausted = probe
        .wait_for_trace(
            trace,
            "a complete upload window held without WINDOW_UPDATE",
            WAIT,
            |event| event.tagged(trace) && event.detail.as_deref() == Some("lifecycle-stall-window-exhausted"),
        )
        .await?;
    let done = probe
        .wait_for_trace(trace, "the complete upstream response", WAIT, |event| {
            event.tagged(trace) && event.is(KIND_RESP_DONE) && event.len == Some(RESPONSE_LEN)
        })
        .await?;
    let reset = probe
        .wait_for_trace(trace, "gateway upload cancellation after response EOS", WAIT, |event| {
            event.tagged(trace) && event.is(KIND_PEER_RESET) && event.code.as_deref() == Some("CANCEL")
        })
        .await?;
    let snapshot = probe.snapshot_of(trace).await?;
    one_attempt(&snapshot, trace)?;
    if snapshot.req_body_total(trace) != UPLOAD_WINDOW || exhausted.seq >= done.seq || done.seq >= reset.seq {
        return Err(format!(
            "expected exactly {UPLOAD_WINDOW} held upload bytes, then response EOS, then CANCEL\n{snapshot}"
        ));
    }
    if snapshot.count_matching(|event| event.tagged(trace) && event.is(KIND_REQ_DATA) && event.end_stream == Some(true))
        != 0
    {
        return Err(format!("the supposedly blocked upload completed\n{snapshot}"));
    }
    // The unconfigured writer's EOS probe is 10s. This bound excludes the
    // ordinary 60s timeout and includes both downstream delivery and cleanup.
    if started.elapsed() > Duration::from_secs(18) {
        return Err(format!(
            "EOS stall did not exit in the 18s test bound: {:?}",
            started.elapsed()
        ));
    }
    head(probe, trace).await
}

async fn stalled_upload(
    ctx: &TestContext,
    client: &reqwest::Client,
    probe: &TransportProbe,
    repeat: bool,
) -> Result<String, String> {
    let first = stall_once(ctx, client, probe, probe.trace_id()).await?;
    let second_trace = probe.sibling_trace("followup");
    let before = probe.snapshot().await?.conn_accepted_total(LISTENER_H2);
    let second = if repeat {
        stall_once(ctx, client, probe, &second_trace).await?
    } else {
        let reply = request(
            client,
            ctx,
            &second_trace,
            "/lifecycle/echo",
            "echo",
            None,
            b"followup".to_vec(),
        )
        .await?;
        full_response(&reply)?;
        head(probe, &second_trace).await?
    };
    let snapshot = probe.snapshot_of(&second_trace).await?;
    one_attempt(&snapshot, &second_trace)?;
    if first.conn != second.conn || before != snapshot.conn_accepted_total(LISTENER_H2) {
        return Err(format!(
            "capacity abandonment did not preserve the healthy connection {} -> {}\n{snapshot}",
            first.conn, second.conn
        ));
    }
    Ok(format!(
        "{UPLOAD_WINDOW}/{UPLOAD_LEN} upload bytes held; complete response and CANCEL within 18s; connection {} reused",
        first.conn
    ))
}

async fn retired_connection(
    ctx: &TestContext,
    client: &reqwest::Client,
    probe: &TransportProbe,
    mode: &str,
) -> Result<String, String> {
    // Gateway peers currently retain max_h2_streams=1. Real product requests
    // therefore exercise serial retirement; the supplemental pinned connector
    // cases separately exercise the active-sibling allocation boundary.
    let fault_trace = probe.trace_id();
    let next_trace = probe.sibling_trace("next");
    let started = Instant::now();
    let fault = request(
        client,
        ctx,
        fault_trace,
        if mode == "shutdown" {
            "/timeout/lifecycle-hang"
        } else {
            "/lifecycle/goaway"
        },
        if mode == "shutdown" { "hang" } else { mode },
        None,
        b"fault".to_vec(),
    )
    .await;
    let first = head(probe, fault_trace).await?;
    let excluded = matches!(mode, "goaway-excluding" | "goaway-raising");
    if excluded {
        probe
            .wait_for("eligible retry response recorded", WAIT, |event| {
                event.tagged(fault_trace)
                    && event.is(KIND_RESP_DONE)
                    && event.detail.as_deref() == Some("lifecycle-goaway-followup")
                    && event.status == Some(201)
                    && event.trace_request_ordinal == Some(2)
            })
            .await?;
    }
    let snapshot = probe.snapshot().await?;
    if snapshot.dropped != 0 {
        return Err(format!("lifecycle recorder lost events\n{snapshot}"));
    }
    let heads: Vec<_> = snapshot
        .events()
        .iter()
        .filter(|event| event.is(KIND_REQ_HEADERS) && event.tagged(fault_trace))
        .collect();
    if heads.len() != if excluded { 2 } else { 1 } {
        return Err(format!(
            "wrong lifecycle attempt count: excluded={excluded}, heads={}\n{snapshot}",
            heads.len()
        ));
    }
    if mode == "shutdown" {
        let reply = fault
            .as_ref()
            .map_err(|error| format!("expected read-timeout 504: {error}"))?;
        if reply.status != 504 || started.elapsed() > Duration::from_secs(8) {
            return Err(format!(
                "expected the route's 2s read-timeout 504, got {fault:?} after {:?}",
                started.elapsed()
            ));
        }
        probe
            .wait_for("upstream CANCEL from the read-timeout shutdown path", WAIT, |event| {
                event.tagged(fault_trace) && event.is(KIND_PEER_RESET) && event.code.as_deref() == Some("CANCEL")
            })
            .await?;
    } else {
        let expected_len = match mode {
            "goaway-descending" | "goaway-excluding" | "goaway-raising" => 34,
            "goaway-malformed" => 13,
            "goaway-nonzero" => 17,
            "goaway-truncated" => 15,
            _ => return Err(format!("unknown lifecycle mode {mode}")),
        };
        let injection = snapshot
            .find(|event| event.detail.as_deref() == Some(&format!("lifecycle-{mode}")))
            .ok_or_else(|| format!("missing acknowledged GOAWAY wire injection\n{snapshot}"))?;
        if injection.len != Some(expected_len) || injection.conn != first.conn {
            return Err(format!("wrong injected GOAWAY sequence: {injection}"));
        }
        if excluded {
            // The origin promises above-ceiling streams were not processed.
            // Its first response is 200; the eligible retry responds 201, so
            // success cannot come from the excluded stream's attempted reply.
            expected_response(fault.as_ref().map_err(Clone::clone)?, 201)?;
            if heads[1].conn == first.conn || heads[1].seq <= injection.seq {
                return Err(format!(
                    "excluded stream was re-admitted on the old connection\n{snapshot}"
                ));
            }
            let recovered = snapshot
                .find(|event| event.detail.as_deref() == Some("lifecycle-goaway-followup"))
                .ok_or("eligible retry never completed its response")?;
            if recovered.conn != heads[1].conn || recovered.trace_request_ordinal != Some(2) {
                return Err(format!(
                    "retry response did not belong to the eligible second attempt: {recovered}"
                ));
            }
        } else if mode == "goaway-descending" {
            full_response(fault.as_ref().map_err(Clone::clone)?)?;
        } else {
            permitted_inflight(&fault)?;
            if fault.as_ref().is_ok_and(|reply| reply.status == 200) {
                return Err(format!(
                    "invalid GOAWAY unexpectedly delivered a successful response: {fault:?}"
                ));
            }
        }
    }
    let before = probe.snapshot().await?.conn_accepted_total(LISTENER_H2);
    let next = request(
        client,
        ctx,
        &next_trace,
        "/lifecycle/next",
        "echo",
        None,
        b"next".to_vec(),
    )
    .await?;
    full_response(&next)?;
    let next_head = head(probe, &next_trace).await?;
    let next_snapshot = probe.snapshot_of(&next_trace).await?;
    one_attempt(&next_snapshot, &next_trace)?;
    let expected_accepts = before + u64::from(!excluded);
    if next_head.conn == first.conn
        || next_snapshot.conn_accepted_total(LISTENER_H2) != expected_accepts
        || (excluded && next_head.conn != heads[1].conn)
    {
        return Err(format!("faulted connection was eligible again, or the healthy replacement was lost: first={first}, next={next_head}\n{next_snapshot}"));
    }
    Ok(format!(
        "{mode}: first conn {}, {} attempt(s), next conn {}; fault={fault:?}",
        first.conn,
        heads.len(),
        next_head.conn
    ))
}

fn permitted_inflight(reply: &Result<Reply, String>) -> Result<(), String> {
    match reply {
        Ok(reply) if reply.status == 200 => full_response(reply),
        Ok(reply) if matches!(reply.status, 502 | 504) => Ok(()),
        Err(error) if error.starts_with("peer transport failure:") => Ok(()),
        _ => Err(format!("unexpected in-flight result for invalid GOAWAY: {reply:?}")),
    }
}
