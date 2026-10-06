//! Supplemental pinned-fork connector contracts with two concurrent streams.
//! The Gateway uses one upstream stream per connection; these probes do not
//! claim that it exposes configurable upstream multiplexing.

use crate::framework::{env_host_or, TestCase, TestContext, TestResult};
use crate::transport::{
    TransportEvent, TransportProbe, KIND_PEER_RESET, KIND_REQ_HEADERS, KIND_RESP_HEAD, LISTENER_H2, RECORDER_PORT,
};
use pingora_core::connectors::http::Connector;
use pingora_core::protocols::http::client::HttpSession;
use pingora_core::upstreams::peer::HttpPeer;
use std::time::{Duration, Instant};

pub(super) const CASE_COUNT: usize = 2;
const WAIT: Duration = Duration::from_secs(3);

pub(super) fn test_cases() -> Vec<TestCase> {
    [("shutdown_allocation", true), ("descending_goaway_sibling", false)]
        .into_iter()
        .map(|(name, shutdown)| {
            TestCase::new(
                format!("h2_lifecycle__direct_fork_{name}"),
                "Pinned public connector: retired connection rejects new streams while an accepted sibling completes",
                move |ctx| {
                    Box::pin(async move {
                        let started = Instant::now();
                        let result = tokio::time::timeout(Duration::from_secs(12), exercise(&ctx, shutdown)).await;
                        match result {
                            Ok(Ok(message)) => TestResult::passed_with_message(started.elapsed(), message),
                            Ok(Err(error)) => TestResult::failed(started.elapsed(), error),
                            Err(_) => {
                                TestResult::failed(started.elapsed(), "direct fork lifecycle exceeded 12s".into())
                            }
                        }
                    })
                },
            )
            .with_timeout(Duration::from_secs(15))
        })
        .collect()
}

fn request(trace: &str, mode: &str, release: Option<&str>) -> Result<Box<pingora_http::RequestHeader>, String> {
    let mut request = pingora_http::RequestHeader::build("POST", b"http://transport-h2.example.com/lifecycle", Some(8))
        .map_err(|error| error.to_string())?;
    for (name, value) in [
        ("host", "transport-h2.example.com".to_string()),
        ("content-length", "0".to_string()),
        ("x-trace-id", crate::access_log_client::trace_id_for_marker(trace)),
        ("x-up-resp-len", "64".to_string()),
        (
            "x-up-behavior",
            if mode == "hang" { "hang" } else { "echo" }.to_string(),
        ),
    ] {
        request.insert_header(name, value).map_err(|error| error.to_string())?;
    }
    if !matches!(mode, "echo" | "hang") {
        request
            .insert_header("x-up-lifecycle", mode)
            .map_err(|error| error.to_string())?;
    }
    if let Some(release) = release {
        request
            .insert_header(
                "x-up-release-trace",
                crate::access_log_client::trace_id_for_marker(release),
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(Box::new(request))
}

async fn send(session: &mut HttpSession, trace: &str, mode: &str, release: Option<&str>) -> Result<(), String> {
    session
        .write_request_header(request(trace, mode, release)?)
        .await
        .map_err(|error| error.to_string())?;
    session.finish_request_body().await.map_err(|error| error.to_string())
}

async fn complete(session: &mut HttpSession) -> Result<(), String> {
    session
        .read_response_header()
        .await
        .map_err(|error| error.to_string())?;
    if session.response_header().map(|header| header.status.as_u16()) != Some(200) {
        return Err("direct fork response was not 200".into());
    }
    let mut body = Vec::with_capacity(64);
    while let Some(chunk) = session.read_response_body().await.map_err(|error| error.to_string())? {
        if body.len() + chunk.len() > 64 {
            return Err("direct fork response exceeded its exact 64-byte bound".into());
        }
        body.extend_from_slice(&chunk);
    }
    let pattern = b"edgion-transport-";
    let expected: Vec<_> = (0..64).map(|index| pattern[index % pattern.len()]).collect();
    if body != expected {
        return Err(format!("direct fork response bytes changed: {body:?}"));
    }
    Ok(())
}

async fn head(probe: &TransportProbe, trace: &str) -> Result<TransportEvent, String> {
    probe
        .wait_for_trace(trace, "direct fork origin headers", WAIT, |event| {
            event.tagged(trace) && event.is(KIND_REQ_HEADERS) && event.listener == LISTENER_H2
        })
        .await
}

async fn exercise(ctx: &TestContext, shutdown: bool) -> Result<String, String> {
    let recorder = env_host_or("EDGION_TEST_TRANSPORT_RECORDER_HOST", &ctx.target_host);
    let probe = TransportProbe::start(&recorder, RECORDER_PORT, "h2-direct-lifecycle").await?;
    let origin = env_host_or("EDGION_TEST_TRANSPORT_H2_HOST", &ctx.target_host);
    let address = tokio::net::lookup_host((origin.as_str(), 30031))
        .await
        .map_err(|error| error.to_string())?
        .next()
        .ok_or("direct fork origin resolved to no address")?;
    let mut peer = HttpPeer::new(address, false, String::new());
    peer.options.set_http_version(2, 2);
    peer.options.max_h2_streams = 2;
    peer.options.read_timeout = Some(Duration::from_secs(2));
    let connector = Connector::new(None);
    let held_trace = probe.trace_id();
    let fault_trace = probe.sibling_trace("fault");
    let release_trace = probe.sibling_trace("release");
    let (mut healthy, reused) = connector
        .get_http_session(&peer)
        .await
        .map_err(|error| error.to_string())?;
    if reused {
        return Err("a new direct connector unexpectedly reused a connection".into());
    }
    send(&mut healthy, held_trace, "held", Some(&release_trace)).await?;
    probe
        .wait_for("held direct sibling ready", WAIT, |event| {
            event.tagged(held_trace) && event.detail.as_deref() == Some("lifecycle-held-ready")
        })
        .await?;
    let held_head = head(&probe, held_trace).await?;
    let (mut fault, reused) = connector
        .get_http_session(&peer)
        .await
        .map_err(|error| error.to_string())?;
    // Connector allocation leaves session deadlines unset. The proxy applies
    // peer read_timeout separately; reproduce that public setup here too.
    fault.set_read_timeout(peer.options.read_timeout);
    if !reused {
        return Err("two-slot connector did not reuse the active sibling's connection".into());
    }
    let conn = fault.as_http2().ok_or("direct fork fault did not use H2")?.conn.clone();
    send(
        &mut fault,
        &fault_trace,
        if shutdown { "hang" } else { "goaway-descending" },
        None,
    )
    .await?;
    let fault_head = head(&probe, &fault_trace).await?;
    if held_head.conn != fault_head.conn || fault_head.stream <= held_head.stream {
        return Err(format!(
            "direct fault did not share a later stream: {held_head}, {fault_head}"
        ));
    }
    if shutdown {
        let error = fault
            .read_response_header()
            .await
            .err()
            .ok_or("hanging direct stream unexpectedly responded")?;
        if error.etype != pingora_core::ErrorType::ReadTimedout {
            return Err(format!("expected upstream ReadTimedout, got {error:?}"));
        }
        if conn.is_closed() || conn.is_shutting_down() {
            return Err("read timeout alone already closed or marked the direct connection".into());
        }
        // Explicitly reproduce the proxy failure arm's mark. This connector
        // probe verifies allocation/pooling; the real Gateway case verifies
        // that a proxy timeout actually reaches its production mark path.
        conn.mark_shutdown();
    } else {
        probe
            .wait_for_trace(&fault_trace, "descending GOAWAY injected", WAIT, |event| {
                event.tagged(&fault_trace)
                    && event.detail.as_deref() == Some("lifecycle-goaway-descending")
                    && event.len == Some(34)
            })
            .await?;
        complete(&mut fault).await?;
    }
    drop(fault);
    if shutdown {
        probe
            .wait_for_trace(&fault_trace, "timed-out direct stream cancelled", WAIT, |event| {
                event.tagged(&fault_trace) && event.is(KIND_PEER_RESET) && event.code.as_deref() == Some("CANCEL")
            })
            .await?;
        if conn.is_closed() || conn.is_idle() || !conn.is_shutting_down() {
            return Err("shutdown allocation probe requires a marked, live, non-idle connection".into());
        }
        if conn.spawn_stream().await.map_err(|error| error.to_string())?.is_some() {
            return Err("marked connection allocated a new stream with an active sibling".into());
        }
    }
    {
        let pending = healthy.read_response_header();
        tokio::pin!(pending);
        if !matches!(futures::poll!(pending.as_mut()), std::task::Poll::Pending) {
            return Err("accepted direct sibling ended before its release request".into());
        }
    }
    let before = probe.snapshot().await?.conn_accepted_total(LISTENER_H2);
    let (mut release, reused) = connector
        .get_http_session(&peer)
        .await
        .map_err(|error| error.to_string())?;
    if reused {
        return Err("direct connector reused a retired connection for the release request".into());
    }
    send(&mut release, &release_trace, "echo", None).await?;
    complete(&mut release).await?;
    let release_head = head(&probe, &release_trace).await?;
    complete(&mut healthy).await?;
    let snapshot = probe.snapshot().await?;
    let release_snapshot = probe.snapshot_of(&release_trace).await?;
    let fault_snapshot = probe.snapshot_of(&fault_trace).await?;
    for (snapshot, trace) in [
        (&snapshot, held_trace),
        (&fault_snapshot, &fault_trace),
        (&release_snapshot, &release_trace),
    ] {
        if snapshot.dropped != 0
            || snapshot.count_matching(|event| event.tagged(trace) && event.is(KIND_REQ_HEADERS)) != 1
        {
            return Err(format!(
                "direct lifecycle requires exactly one recorded attempt per stream\n{snapshot}"
            ));
        }
    }
    let healthy_response = snapshot
        .find(|event| event.tagged(held_trace) && event.is(KIND_RESP_HEAD))
        .ok_or("accepted direct sibling response missing")?;
    if release_head.conn == held_head.conn
        || release_snapshot.conn_accepted_total(LISTENER_H2) != before + 1
        || healthy_response.seq <= release_head.seq
    {
        return Err(format!("release must use exactly one new connection before accepted sibling responds\n{snapshot}\n{release_snapshot}"));
    }
    Ok(format!(
        "direct fork only: {} retires conn {} with stream {} active; conn {} releases its complete 64-byte response",
        if shutdown {
            "explicit shutdown after ReadTimedout"
        } else {
            "descending GOAWAY"
        },
        held_head.conn,
        held_head.stream,
        release_head.conn,
    ))
}
