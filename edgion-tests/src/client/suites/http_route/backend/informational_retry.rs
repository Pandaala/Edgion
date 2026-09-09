use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::transport::{
    RawH1Conn, TransportProbe, TransportSnapshot, KIND_REQ_HEADERS, KIND_RESP_HEAD, LISTENER_H1, RECORDER_PORT,
};
use std::time::{Duration, Instant};

const HOST: &str = "informational-retry.example.com";
const LOCAL_RECORDER_HOST: &str = "127.0.0.1";
const BEHAVIOR: &str = "informational-then-hang-first";

pub struct InformationalRetryTestSuite;

fn recorder_host_from(value: Option<String>) -> String {
    value
        .filter(|host| !host.trim().is_empty())
        .unwrap_or_else(|| LOCAL_RECORDER_HOST.to_string())
}

fn recorder_host() -> String {
    recorder_host_from(std::env::var("EDGION_TEST_TRANSPORT_RECORDER_HOST").ok())
}

async fn assert_wire(ctx: &TestContext, trace: &str) -> Result<(), String> {
    let mut conn = RawH1Conn::connect(&ctx.target_host, ctx.http_port).await?;
    conn.write_head(
        "GET",
        "/retry-after-informational",
        HOST,
        &[
            ("Connection", "close"),
            ("x-up-behavior", BEHAVIOR),
            ("x-trace-id", trace),
            ("access_log", "test_store"),
        ],
    )
    .await?;

    let interim = conn.read_response_head(Duration::from_secs(5)).await?;
    if interim.status != 100 || !interim.is_interim() {
        return Err(format!("expected one downstream 100 Continue head, got {interim}"));
    }

    let final_head = conn.read_response_head(Duration::from_secs(10)).await?;
    if final_head.status != 200 || final_head.is_interim() {
        return Err(format!("expected final downstream 200 head, got {final_head}"));
    }
    let body = conn.read_body(&final_head, Duration::from_secs(5)).await?;
    if body != b"edgion-transport" {
        return Err(format!(
            "unexpected final response body: {:?}",
            String::from_utf8_lossy(&body)
        ));
    }
    conn.expect_close_without_response(Duration::from_secs(5)).await
}

fn assert_recorder(snapshot: &TransportSnapshot, trace: &str) -> Result<(), String> {
    let requests = snapshot
        .events()
        .iter()
        .filter(|event| event.is(KIND_REQ_HEADERS) && event.listener == LISTENER_H1 && event.tagged(trace))
        .collect::<Vec<_>>();
    if requests.len() != 2
        || requests
            .iter()
            .map(|event| event.trace_request_ordinal)
            .collect::<Vec<_>>()
            != [Some(1), Some(2)]
    {
        return Err(format!(
            "expected exactly two ordered trace-keyed upstream requests, got {}\n{snapshot}",
            requests.len()
        ));
    }

    let responses = snapshot
        .events()
        .iter()
        .filter(|event| event.is(KIND_RESP_HEAD) && event.listener == LISTENER_H1 && event.tagged(trace))
        .collect::<Vec<_>>();
    let statuses = responses.iter().map(|event| event.status).collect::<Vec<_>>();
    let ordinals = responses
        .iter()
        .map(|event| event.trace_request_ordinal)
        .collect::<Vec<_>>();
    if statuses != [Some(100), Some(200)] || ordinals != [Some(1), Some(2)] {
        return Err(format!(
            "expected trace requests 1/2 to emit upstream heads 100/200, got statuses={statuses:?} ordinals={ordinals:?}\n{snapshot}"
        ));
    }
    if responses
        .iter()
        .any(|event| event.behavior.as_deref() != Some(BEHAVIOR))
    {
        return Err(format!("unexpected response behavior in recorder\n{snapshot}"));
    }
    Ok(())
}

async fn cleanup_access_log(ctx: &TestContext, trace: &str) -> Result<(), String> {
    let client = ctx.access_log_client();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut absent = 0;
    let mut api_errors = 0;
    let mut last_error = None;
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), client.delete_access_log(trace)).await {
            Ok(Ok(true)) => return Ok(()),
            Ok(Ok(false)) => absent += 1,
            Ok(Err(error)) => {
                api_errors += 1;
                last_error = Some(error.to_string());
            }
            Err(_) => {
                api_errors += 1;
                last_error = Some("delete request exceeded 500 ms".to_string());
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!(
        "informational retry access-log deletion was not confirmed within 5 seconds: absent={absent}, \
         api_errors={api_errors}, last_error={}",
        last_error.unwrap_or_else(|| "none".to_string())
    ))
}

async fn reset_recorder(ctx: &TestContext) -> Result<(), String> {
    let recorder_host = recorder_host();
    ctx.http_client
        .get(format!("http://{recorder_host}:{RECORDER_PORT}/transport/reset"))
        .send()
        .await
        .map_err(|error| format!("transport reset request failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("transport reset failed: {error}"))?;
    Ok(())
}

async fn collect_evidence(ctx: &TestContext, probe: &TransportProbe) -> Result<(), String> {
    let trace = probe.trace_id();
    let mut failures = Vec::new();

    match tokio::time::timeout(Duration::from_secs(16), assert_wire(ctx, trace)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => failures.push(error),
        Err(_) => failures.push("downstream wire assertions exceeded 16 seconds".to_string()),
    }

    let snapshot = match probe
        .wait_for("the second upstream response head", Duration::from_secs(5), |event| {
            event.is(KIND_RESP_HEAD)
                && event.tagged(trace)
                && event.trace_request_ordinal == Some(2)
                && event.status == Some(200)
        })
        .await
    {
        Ok(_) => match probe.snapshot().await {
            Ok(snapshot) => Some(snapshot),
            Err(error) => {
                failures.push(format!("transport snapshot failed: {error}\n{}", probe.dump().await));
                None
            }
        },
        Err(error) => {
            failures.push(format!("transport evidence missing: {error}\n{}", probe.dump().await));
            None
        }
    };
    if let Some(snapshot) = snapshot.as_ref() {
        if let Err(error) = assert_recorder(snapshot, trace) {
            failures.push(error);
        }
    }

    match ctx.access_log_client().get_access_log_with_retry(trace, 20, 200).await {
        Ok(entry) => {
            let upstreams = entry.data["backend_context"]["upstreams"].as_array();
            let valid = entry.status_code() == Some(200)
                && upstreams.is_some_and(|attempts| {
                    attempts.len() == 2
                        && attempts[0]["status"].as_u64() == Some(504)
                        && attempts[0]["err"]
                            .as_array()
                            .is_some_and(|errors| errors.iter().any(|error| error.as_str() == Some("ReadTimedout")))
                        && attempts[1]["status"].as_u64() == Some(200)
                })
                && entry.has_diagnostic("error", "BackendRequestTimeoutRetry");
            if !valid {
                failures.push(format!("unexpected informational-retry access log: {}", entry.data));
            }
        }
        Err(error) => failures.push(format!("access log missing: {error}")),
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

impl InformationalRetryTestSuite {
    fn informational_head_does_not_suppress_timeout_retry() -> TestCase {
        TestCase::new(
            "informational_head_does_not_suppress_timeout_retry",
            "a flushed upstream 100 head preserves the backend-request timeout retry and one final response",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let probe =
                        match TransportProbe::start(&recorder_host(), RECORDER_PORT, "informational-retry").await {
                            Ok(probe) => probe,
                            Err(error) => return TestResult::failed(start.elapsed(), error),
                        };
                    let mut failures = Vec::new();
                    match tokio::time::timeout(Duration::from_secs(32), collect_evidence(&ctx, &probe)).await {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => failures.push(error),
                        Err(_) => {
                            failures.push("informational retry evidence collection exceeded 32 seconds".to_string())
                        }
                    }
                    match tokio::time::timeout(Duration::from_secs(6), cleanup_access_log(&ctx, probe.trace_id())).await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => failures.push(error),
                        Err(_) => {
                            failures.push("informational retry access-log cleanup exceeded 6 seconds".to_string())
                        }
                    }
                    match tokio::time::timeout(Duration::from_secs(6), reset_recorder(&ctx)).await {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => failures.push(error),
                        Err(_) => failures.push("informational retry recorder reset exceeded 6 seconds".to_string()),
                    }
                    if failures.is_empty() {
                        TestResult::passed(start.elapsed())
                    } else {
                        TestResult::failed(start.elapsed(), failures.join("; "))
                    }
                })
            },
        )
    }
}

impl TestSuite for InformationalRetryTestSuite {
    fn name(&self) -> &str {
        "Informational timeout retry"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/InformationalRetry")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::informational_head_does_not_suppress_timeout_retry()]
    }
}

#[cfg(test)]
mod tests {
    use super::{recorder_host_from, LOCAL_RECORDER_HOST};

    #[test]
    fn recorder_host_uses_local_default_and_nonblank_override() {
        assert_eq!(recorder_host_from(None), LOCAL_RECORDER_HOST);
        assert_eq!(recorder_host_from(Some(" ".to_string())), LOCAL_RECORDER_HOST);
        assert_eq!(
            recorder_host_from(Some("transport-recorder.edgion-test.svc.cluster.local".to_string())),
            "transport-recorder.edgion-test.svc.cluster.local"
        );
    }
}
