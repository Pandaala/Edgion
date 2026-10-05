//! P090-008 acceptance against the real Edgion override and controlled origins.
//! EffectApplied is a simulated application commit, separate from received bytes.

use super::{Combo, Proto};
use crate::framework::{env_host_or, TestCase, TestContext, TestResult};
use crate::transport::{
    TransportProbe, KIND_CONN_ACCEPTED, KIND_CONN_CLOSED, KIND_EFFECT_APPLIED, KIND_REQ_HEADERS, KIND_RESP_DONE,
    KIND_RESP_HEAD, RECORDER_PORT,
};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

pub(super) const CASE_COUNT: usize = 65;

#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    method: &'static str,
    behavior: &'static str,
    route: &'static str,
    path: &'static str,
    warm: bool,
    large: bool,
    fail_first: usize,
    attempts: usize,
    effects: usize,
    success: bool,
}

fn reset_case(name: &'static str, method: &'static str, route: &'static str, warm: bool, attempts: usize) -> Case {
    Case {
        name,
        method,
        route,
        warm,
        attempts,
        behavior: "retry-tcp-reset",
        path: "/",
        large: false,
        fail_first: 1,
        effects: attempts,
        success: attempts == 2,
    }
}

pub(super) fn test_cases() -> Vec<TestCase> {
    let mut cases = Vec::new();
    for down in [Proto::H1, Proto::H2] {
        for up in [Proto::H1, Proto::H2] {
            let mut resets = vec![
                reset_case("fresh_get_reset", "GET", "ordinary", false, 1),
                reset_case("fresh_optin_post_reset", "POST", "optin", false, 1),
                reset_case("reused_get_reset", "GET", "ordinary", true, 2),
                reset_case("reused_post_reset", "POST", "ordinary", true, 1),
                reset_case("reused_optin_post_reset", "POST", "optin", true, 2),
                reset_case("attempt_budget_exhausted_reset", "GET", "policy", true, 2),
                reset_case("truncated_native_reset", "GET", "ordinary", true, 1),
                reset_case("registered_body_reset", "POST", "optin", true, 2),
                reset_case("streamed_reset", "GET", "streamed", true, 1),
            ];
            resets[5].path = "/limited";
            resets[5].fail_first = 0;
            resets[5].success = false;
            resets[6].large = true;
            resets[7].large = true;
            resets[7].path = "/cached";
            for case in resets {
                cases.push(make_case(case, down, up));
            }
            for (name, behavior, path, attempts, success) in [
                ("explicit_post_status", "retry-status", "/", 2, true),
                ("status_budget_exhausted", "retry-status", "/limited", 2, false),
                ("explicit_post_timeout", "retry-timeout", "/", 2, true),
                ("total_deadline_post", "retry-timeout", "/deadline", 1, false),
            ] {
                let mut case = reset_case(name, "POST", "policy", false, attempts);
                case.behavior = behavior;
                case.path = path;
                case.success = success;
                if name == "status_budget_exhausted" {
                    case.fail_first = 0;
                }
                cases.push(make_case(case, down, up));
            }
        }
        for (name, route, fail_first, attempts, effects, success) in [
            ("refused_post", "ordinary", 1, 2, 1, true),
            ("refused_bodyless_post", "bodyless", 1, 2, 1, true),
            ("refused_ceiling", "ordinary", 0, 17, 0, false),
            ("refused_streamed", "streamed", 1, 1, 0, false),
        ] {
            let mut case = reset_case(name, "POST", route, false, attempts);
            case.behavior = "retry-refused";
            case.fail_first = fail_first;
            case.effects = effects;
            case.success = success;
            cases.push(make_case(case, down, Proto::H2));
        }
        for route in ["ordinary", "bodyless"] {
            let mut case = reset_case(
                if route == "ordinary" {
                    "goaway_post"
                } else {
                    "goaway_bodyless_post"
                },
                "POST",
                route,
                false,
                2,
            );
            case.behavior = "retry-goaway";
            case.effects = 1;
            cases.push(make_case(case, down, Proto::H2));
        }
    }
    cases.push(TestCase::new(
        "retry_policy__direct_fork_refused_sibling_health",
        "Pinned fork connector: REFUSED_STREAM preserves an active sibling on one origin connection",
        |ctx| {
            Box::pin(async move {
                let start = Instant::now();
                match sibling_health(&ctx).await {
                    Ok(()) => TestResult::passed(start.elapsed()),
                    Err(error) => TestResult::failed(start.elapsed(), error),
                }
            })
        },
    ));
    assert_eq!(cases.len(), CASE_COUNT, "retry policy matrix must not silently shrink");
    cases
}

fn make_case(case: Case, down: Proto, up: Proto) -> TestCase {
    TestCase::new(
        format!("retry_policy__{}__{}_to_{}", case.name, down.tag(), up.tag()),
        "Exact origin request/effect counts, replay digest, and final outcome",
        move |ctx| {
            Box::pin(async move {
                let start = Instant::now();
                match run(&ctx, case, down, up).await {
                    Ok(()) => TestResult::passed(start.elapsed()),
                    Err(error) => TestResult::failed(start.elapsed(), error),
                }
            })
        },
    )
    .with_timeout(Duration::from_secs(20))
}

async fn client(down: Proto, host: &str, ctx: &TestContext) -> Result<reqwest::Client, String> {
    let address = tokio::net::lookup_host((ctx.target_host.as_str(), ctx.http_port))
        .await
        .map_err(|error| format!("gateway address: {error}"))?
        .next()
        .ok_or("gateway address resolved to no IP")?;
    let builder = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(8))
        .resolve(host, address);
    let builder = match down {
        Proto::H1 => builder.http1_only(),
        Proto::H2 => builder.http2_prior_knowledge(),
    };
    builder.build().map_err(|error| error.to_string())
}

fn host(case: Case, up: Proto) -> String {
    match case.route {
        "optin" => format!("transport-retry-optin-{}.example.com", up.tag()),
        "policy" => format!("transport-retry-policy-{}.example.com", up.tag()),
        "streamed" => Combo::new(Proto::H1, up, true).host().into(),
        _ => Combo::new(Proto::H1, up, false).host().into(),
    }
}

async fn run(ctx: &TestContext, case: Case, down: Proto, up: Proto) -> Result<(), String> {
    let recorder = env_host_or("EDGION_TEST_TRANSPORT_RECORDER_HOST", &ctx.target_host);
    let probe = TransportProbe::start(&recorder, RECORDER_PORT, case.name).await?;
    let result = exercise(ctx, &probe, case, down, up).await;
    // Cleanup also runs after an assertion fails. Recorder resets are per case;
    // only logs explicitly stored by this case are removed.
    let cleanup = ctx.access_log_client().delete_access_log(probe.trace_id()).await;
    if let Err(error) = cleanup {
        return Err(format!("{result:?}; access-log cleanup: {error}"));
    }
    result
}

async fn exercise(ctx: &TestContext, probe: &TransportProbe, case: Case, down: Proto, up: Proto) -> Result<(), String> {
    let host = host(case, up);
    let client = client(down, &host, ctx).await?;
    let url = format!("http://{host}:{}{}", ctx.http_port, case.path);
    let traceparent = crate::access_log_client::traceparent_for_marker(probe.trace_id());
    let warm_trace = probe.sibling_trace("warm");
    if case.warm {
        let response = client
            .get(&url)
            .header(
                "traceparent",
                crate::access_log_client::traceparent_for_marker(&warm_trace),
            )
            .header("x-up-behavior", "echo")
            .header("x-expected-body-sha256", format!("{:x}", Sha256::digest([])))
            .send()
            .await
            .map_err(|error| format!("warm request: {error}"))?;
        if response.status() != 200 {
            return Err(format!("warm request returned {}", response.status()));
        }
        response.bytes().await.map_err(|error| error.to_string())?;
    }
    let body = if case.route == "bodyless" {
        Vec::new()
    } else {
        vec![b'x'; if case.large { 131_072 } else { 32 }]
    };
    let digest = format!("{:x}", Sha256::digest(&body));
    let response = client
        .request(case.method.parse::<reqwest::Method>().unwrap(), &url)
        .header("traceparent", traceparent)
        .header("access_log", "test_store")
        .header("x-up-behavior", case.behavior)
        .header("x-up-reset-first", case.fail_first.to_string())
        .header("x-up-after-response", "close")
        .header("x-expected-body-sha256", &digest)
        .body(body.clone())
        .send()
        .await;
    match response {
        Ok(response) => {
            let status = response.status();
            if case.success != status.is_success() {
                return Err(format!(
                    "expected success={}, received {status}\n{}",
                    case.success,
                    probe.dump().await
                ));
            }
            response
                .bytes()
                .await
                .map_err(|error| format!("response body: {error}"))?;
        }
        Err(error) => {
            return Err(format!(
                "gateway request did not complete: {error}\n{}",
                probe.dump().await
            ))
        }
    }
    let entry = ctx
        .access_log_client()
        .get_access_log_with_retry(probe.trace_id(), 30, 100)
        .await
        .map_err(|error| format!("final access log missing: {error}"))?;
    if case.name == "refused_ceiling" && !entry.has_diagnostic("error", "UpstreamTransportRetryCeiling") {
        return Err(format!("missing transport-ceiling diagnostic: {}", entry.data));
    }
    // A completed request plus a bounded observation window prevents a late
    // successor from escaping the exact whole-log counts below.
    probe
        .expect_none("late upstream request", Duration::from_millis(150), |event| {
            event.is(KIND_REQ_HEADERS)
        })
        .await?;
    let snapshot = probe.snapshot().await?;
    let requests: Vec<_> = snapshot
        .events()
        .iter()
        .filter(|event| event.tagged(probe.trace_id()) && event.is(KIND_REQ_HEADERS))
        .collect();
    let effects: Vec<_> = snapshot
        .events()
        .iter()
        .filter(|event| event.tagged(probe.trace_id()) && event.is(KIND_EFFECT_APPLIED))
        .collect();
    if requests.len() != case.attempts || effects.len() != case.effects || snapshot.dropped != 0 {
        return Err(format!(
            "expected attempts/effects={}/{}, got {}/{}\n{snapshot}",
            case.attempts,
            case.effects,
            requests.len(),
            effects.len()
        ));
    }
    if requests.iter().any(|event| {
        event.listener != up.listener()
            || event.method.as_deref() != Some(case.method)
            || event.behavior.as_deref() != Some(case.behavior)
    }) {
        return Err(format!("wrong upstream protocol, method, or script\n{snapshot}"));
    }
    let code = match case.behavior {
        "retry-tcp-reset" => Some("TCP_RST"),
        "retry-refused" => Some("REFUSED_STREAM"),
        "retry-goaway" => Some("GOAWAY_UNPROCESSED"),
        "retry-timeout" => Some("HANG"),
        _ => None,
    };
    if let Some(code) = code {
        if !snapshot.events().iter().any(|event| {
            event.tagged(probe.trace_id()) && event.is(KIND_RESP_DONE) && event.code.as_deref() == Some(code)
        }) {
            return Err(format!("the claimed fault {code} was not observed\n{snapshot}"));
        }
    }
    if case.route != "streamed"
        && effects
            .iter()
            .any(|event| event.len != Some(body.len()) || event.detail.as_deref() != Some(digest.as_str()))
    {
        return Err(format!("application input length/digest changed on replay\n{snapshot}"));
    }
    if case.warm {
        let warm = probe.snapshot_of(&warm_trace).await?;
        let first = requests.first().ok_or("request did not reach origin")?;
        if !warm.events().iter().any(|event| {
            event.tagged(&warm_trace)
                && event.is(KIND_REQ_HEADERS)
                && event.conn == first.conn
                && event.listener == first.listener
        }) {
            return Err(format!(
                "failure did not exercise the warmed origin connection\n{warm}\n{snapshot}"
            ));
        }
    }
    if case.name.starts_with("fresh_") {
        let first = requests.first().ok_or("fresh request did not reach origin")?;
        if first.stream != 1
            || !snapshot.events().iter().any(|event| {
                event.is(KIND_CONN_ACCEPTED)
                    && event.conn == first.conn
                    && event.listener == first.listener
                    && event.seq < first.seq
            })
        {
            return Err(format!(
                "fresh fault did not use a newly accepted connection's first request\n{snapshot}"
            ));
        }
    }
    Ok(())
}

async fn sibling_health(ctx: &TestContext) -> Result<(), String> {
    let recorder = env_host_or("EDGION_TEST_TRANSPORT_RECORDER_HOST", &ctx.target_host);
    let probe = TransportProbe::start(&recorder, RECORDER_PORT, "retry-sibling").await?;
    tokio::time::timeout(Duration::from_secs(8), exercise_sibling(ctx, &probe))
        .await
        .map_err(|_| "direct fork sibling probe timed out".to_string())?
}

fn origin_request(
    method: &str,
    trace: &str,
    headers: &[(&'static str, &str)],
) -> Result<Box<pingora_http::RequestHeader>, String> {
    let mut request = pingora_http::RequestHeader::build(method, b"http://transport-h2.example.com/", Some(8))
        .map_err(|error| error.to_string())?;
    request
        .insert_header("host", "transport-h2.example.com")
        .map_err(|error| error.to_string())?;
    request
        .insert_header("content-length", "0")
        .map_err(|error| error.to_string())?;
    request
        .insert_header("x-trace-id", crate::access_log_client::trace_id_for_marker(trace))
        .map_err(|error| error.to_string())?;
    for (name, value) in headers {
        request
            .insert_header(*name, *value)
            .map_err(|error| error.to_string())?;
    }
    Ok(Box::new(request))
}

async fn exercise_sibling(ctx: &TestContext, probe: &TransportProbe) -> Result<(), String> {
    // Gateway peer construction retains the fork default of one concurrent
    // upstream stream. Exercise protocol isolation through the same pinned
    // connector with two slots, against the existing shared origin.
    let origin = env_host_or("EDGION_TEST_TRANSPORT_H2_HOST", &ctx.target_host);
    let address = tokio::net::lookup_host((origin.as_str(), 30031))
        .await
        .map_err(|error| error.to_string())?
        .next()
        .ok_or("origin resolved to no IP")?;
    let mut peer = pingora_core::upstreams::peer::HttpPeer::new(address, false, String::new());
    peer.options.set_http_version(2, 2);
    peer.options.max_h2_streams = 2;
    peer.options.read_timeout = Some(Duration::from_secs(3));
    let connector = pingora_core::connectors::http::Connector::new(None);
    let sibling = probe.sibling_trace("healthy");
    let (mut healthy, _) = connector
        .get_http_session(&peer)
        .await
        .map_err(|error| error.to_string())?;
    healthy
        .write_request_header(origin_request(
            "GET",
            &sibling,
            &[("x-up-delay-ms", "750"), ("x-up-after-response", "close")],
        )?)
        .await
        .map_err(|error| error.to_string())?;
    healthy.finish_request_body().await.map_err(|error| error.to_string())?;
    probe
        .wait_for_trace(&sibling, "active sibling headers", Duration::from_secs(5), |event| {
            event.is(KIND_REQ_HEADERS) && event.tagged(&sibling)
        })
        .await?;
    let (mut refused, reused) = connector
        .get_http_session(&peer)
        .await
        .map_err(|error| error.to_string())?;
    if !reused {
        return Err("direct fork connector did not reuse the active connection".into());
    }
    refused
        .write_request_header(origin_request(
            "POST",
            probe.trace_id(),
            &[("x-up-behavior", "retry-refused"), ("x-up-reset-first", "1")],
        )?)
        .await
        .map_err(|error| error.to_string())?;
    refused.finish_request_body().await.map_err(|error| error.to_string())?;
    let error = refused
        .read_response_header()
        .await
        .err()
        .ok_or("refused stream unexpectedly succeeded")?;
    if error.etype != pingora_core::ErrorType::H2Error || !matches!(error.retry, pingora_core::RetryType::Decided(true))
    {
        return Err(format!(
            "REFUSED_STREAM must arrive as H2Error Decided(true): {error:?}"
        ));
    }
    healthy
        .read_response_header()
        .await
        .map_err(|error| error.to_string())?;
    if healthy.response_header().map(|header| header.status.as_u16()) != Some(200) {
        return Err("active sibling did not return 200".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = healthy.read_response_body().await.map_err(|error| error.to_string())? {
        body.extend_from_slice(&chunk);
        if body.len() > 16 {
            return Err("sibling response exceeded its exact 16-byte body".into());
        }
    }
    if body != b"edgion-transport" {
        return Err(format!("sibling response body changed: {body:?}"));
    }
    let snapshot = probe.snapshot().await?;
    let sibling_snapshot = probe.snapshot_of(&sibling).await?;
    let requests: Vec<_> = sibling_snapshot
        .events()
        .iter()
        .filter(|event| event.tagged(&sibling) && event.is(KIND_REQ_HEADERS))
        .collect();
    let failed_requests: Vec<_> = snapshot
        .events()
        .iter()
        .filter(|event| event.tagged(probe.trace_id()) && event.is(KIND_REQ_HEADERS))
        .collect();
    let failed = failed_requests.first().ok_or("refused request missing")?;
    if requests.len() != 1
        || failed_requests.len() != 1
        || requests[0].conn != failed.conn
        || snapshot.dropped != 0
        || sibling_snapshot.dropped != 0
        || snapshot
            .events()
            .iter()
            .any(|event| event.tagged(probe.trace_id()) && event.is(KIND_EFFECT_APPLIED))
    {
        return Err(format!(
            "sibling must survive on the same upstream connection\n{snapshot}\n{sibling_snapshot}"
        ));
    }
    let fault = snapshot
        .events()
        .iter()
        .find(|event| {
            event.tagged(probe.trace_id())
                && event.is(KIND_RESP_DONE)
                && event.code.as_deref() == Some("REFUSED_STREAM")
        })
        .ok_or("refused fault missing")?;
    if !sibling_snapshot
        .events()
        .iter()
        .any(|event| event.tagged(&sibling) && event.is(KIND_RESP_HEAD) && event.seq > fault.seq)
    {
        return Err(format!(
            "sibling response must follow the fault while sharing its connection\n{snapshot}\n{sibling_snapshot}"
        ));
    }
    probe
        .wait_for("direct origin connection closed", Duration::from_secs(3), |event| {
            event.is(KIND_CONN_CLOSED) && event.conn == failed.conn
        })
        .await?;
    Ok(())
}
