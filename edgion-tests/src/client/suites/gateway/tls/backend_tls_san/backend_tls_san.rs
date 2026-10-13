use crate::access_log_client::{trace_id_for_marker, traceparent_for_marker, AccessLogEntry};
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::{Duration, Instant};

fn verify_san_rejection(status: u16, entry: &AccessLogEntry, trace_id: &str) -> Result<(), String> {
    if !(500..600).contains(&status) {
        return Err(format!("Expected 5xx for SAN mismatch, got {status}"));
    }
    if entry.get_nested_str("request_info.trace_id") != Some(trace_id)
        || !entry.has_diagnostic("error", "BackendTlsSanValidationFailed")
    {
        return Err("Missing trace-correlated BackendTlsSanValidationFailed evidence".to_string());
    }
    Ok(())
}

pub struct BackendTlsSanTestSuite;

impl BackendTlsSanTestSuite {
    fn test_san_hostname_match() -> TestCase {
        TestCase::new(
            "san_hostname_match",
            "Backend SAN validation passes when cert DNS SAN matches policy Hostname",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:{}/backend-san-match/health", ctx.target_host, ctx.http_port);
                    let mut request = ctx.http_client.get(&url);
                    if let Some(ref host) = ctx.http_host {
                        request = request.header("Host", host);
                    }
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            let body = response.text().await.unwrap_or_default();
                            if status.is_success() && body.contains("Server: 0.0.0.0:30051") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("SAN match succeeded. Status: {}", status),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected success, got status: {}, body: {}", status, body),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_san_hostname_mismatch() -> TestCase {
        TestCase::new(
            "san_hostname_mismatch",
            "Backend SAN validation rejects when cert DNS SAN does not match policy Hostname",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!(
                        "http://{}:{}/backend-san-mismatch/health",
                        ctx.target_host, ctx.http_port
                    );
                    let mut request = ctx.http_client.get(&url);
                    if let Some(ref host) = ctx.http_host {
                        request = request.header("Host", host);
                    }
                    let marker = format!("backend-san-denial-{}", uuid::Uuid::new_v4());
                    let trace_id = trace_id_for_marker(&marker);
                    let access_logs = ctx.access_log_client();
                    request = request
                        .header("traceparent", traceparent_for_marker(&marker))
                        .header("access_log", "test_store");
                    let exchange = tokio::time::timeout(Duration::from_secs(10), async {
                        let response = request
                            .send()
                            .await
                            .map_err(|_| "SAN probe request failed".to_string())?;
                        let status = response.status().as_u16();
                        response
                            .bytes()
                            .await
                            .map_err(|_| "SAN probe response body read failed".to_string())?;
                        Ok::<_, String>(status)
                    })
                    .await;
                    let outcome = match exchange {
                        Ok(Ok(status)) => match tokio::time::timeout(
                            Duration::from_secs(5),
                            access_logs.get_access_log_with_retry(&marker, 20, 100),
                        )
                        .await
                        {
                            Ok(Ok(entry)) => verify_san_rejection(status, &entry, &trace_id),
                            _ => Err("Failed to observe SAN probe access log".to_string()),
                        },
                        Ok(Err(error)) => Err(error),
                        Err(_) => Err("SAN probe exchange timed out".to_string()),
                    };
                    // This is a bounded attempt, not a guarantee against outer cancellation
                    // or a terminal entry published after a timed-out exchange.
                    let cleanup_ok = matches!(
                        tokio::time::timeout(Duration::from_secs(5), access_logs.delete_access_log(&marker)).await,
                        Ok(Ok(true))
                    );
                    match (outcome, cleanup_ok) {
                        (Ok(()), true) => TestResult::passed_with_message(
                            start.elapsed(),
                            "SAN mismatch rejected with the canonical request diagnostic".to_string(),
                        ),
                        (Err(error), true) => TestResult::failed(start.elapsed(), error),
                        (Ok(()), false) => {
                            TestResult::failed(start.elapsed(), "Failed to clean up SAN probe access log".to_string())
                        }
                        (Err(error), false) => TestResult::failed(
                            start.elapsed(),
                            format!("{error}; failed to clean up SAN probe access log"),
                        ),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for BackendTlsSanTestSuite {
    fn name(&self) -> &str {
        "backend_tls_san"
    }

    fn port_key(&self) -> Option<&str> {
        Some("Gateway/TLS/BackendTLSSAN")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_san_hostname_match(), Self::test_san_hostname_mismatch()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn san_rejection_requires_current_specific_diagnostic() {
        let trace = trace_id_for_marker("san-probe-verdict");
        let mut entry = AccessLogEntry {
            data: json!({"request_info": {"trace_id": trace}, "diagnostics": []}),
        };
        for status in [502, 503] {
            assert!(verify_san_rejection(status, &entry, &trace).is_err());
        }
        entry.data["diagnostics"] = json!([{"kind": "error", "code": "BackendTlsSanValidationFailed"}]);
        assert!(verify_san_rejection(502, &entry, &trace).is_ok());
        assert!(verify_san_rejection(200, &entry, &trace).is_err());
        assert!(verify_san_rejection(502, &entry, "stale-trace").is_err());
        entry.data["diagnostics"] = json!([{"kind": "info", "code": "BackendTlsSanValidationFailed"}]);
        assert!(verify_san_rejection(502, &entry, &trace).is_err());
        assert!(verify_san_rejection(502, &AccessLogEntry { data: json!({}) }, &trace).is_err());
    }
}
