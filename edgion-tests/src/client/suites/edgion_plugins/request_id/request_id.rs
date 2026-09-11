//! W3C core propagation and incremental RequestId wire behavior.
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use serde_json::Value;
use std::time::Instant;

async fn check_log(ctx: &TestContext, trace: &str) -> Result<Value, String> {
    let entry = ctx
        .access_log_client()
        .get_access_log_with_retry(trace, 30, 50)
        .await
        .map_err(|e| e.to_string())?;
    let ids = &entry.data["request_info"]["trace_id"];
    if ids["x-trace-id"] != trace || !ids.as_object().is_some_and(|map| map.values().all(Value::is_string)) {
        return Err(format!("expected flat ID map with canonical W3C identity: {ids}"));
    }
    Ok(ids.clone())
}

pub struct RequestIdPluginTestSuite;

fn valid_parent(value: &str) -> bool {
    let parts: Vec<_> = value.split('-').collect();
    parts.len() == 4
        && parts[0] == "00"
        && parts[1].len() == 32
        && parts[2].len() == 16
        && parts[3].len() == 2
        && parts[1..]
            .iter()
            .all(|p| p.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        && parts[1].bytes().any(|b| b != b'0')
        && parts[2].bytes().any(|b| b != b'0')
}

fn case(name: &'static str, mode: &'static str) -> TestCase {
    TestCase::new(
        name,
        "Verify canonical tracing and configured correlation headers on the wire",
        move |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let result: Result<(), String> = async {
                    let trace = uuid::Uuid::new_v4().simple().to_string();
                    let incoming_parent = format!("00-{trace}-00f067aa0ba902b7-01");
                    let host = match mode {
                        "custom" => "request-id-custom.example.com",
                        "multiple" | "source" => "request-id-multiple.example.com",
                        "nogen" => "request-id-nogen.example.com",
                        "early" => "request-id-unmatched.invalid",
                        _ => "request-id.example.com",
                    };
                    let mut request = ctx
                        .http_client
                        .get(format!("{}/headers", ctx.http_url()))
                        .header("access_log", "test_store")
                        .header("host", host);
                    if matches!(mode, "passthrough" | "multiple" | "source" | "custom") {
                        request = request
                            .header("traceparent", &incoming_parent)
                            .header("tracestate", "vendor=value");
                    }
                    if mode == "passthrough" {
                        request = request.header("x-trace-id", "first-gateway-id");
                    }
                    if mode == "custom" {
                        request = request.header("x-trace-id", "remove-me");
                    }
                    if mode == "multiple" {
                        request = request
                            .header("x-business-id", "business-42")
                            .header("x-order-id", "first-order");
                    }
                    if mode == "source" {
                        request = request.header("x-business-id", "business-42");
                    }
                    if mode == "malformed" {
                        request = request
                            .header("traceparent", "garbage")
                            .header("tracestate", "vendor=orphan");
                    }
                    if mode == "duplicate" {
                        request = request
                            .header("traceparent", &incoming_parent)
                            .header("traceparent", &incoming_parent);
                    }
                    let response = request.send().await.map_err(|e| e.to_string())?;
                    let status = response.status().as_u16();
                    let headers = response.headers().clone();
                    let response_id = headers.get("x-trace-id").and_then(|v| v.to_str().ok());
                    if headers.contains_key("traceparent") {
                        return Err("W3C response echo must not be enabled implicitly".into());
                    }
                    if mode == "early" {
                        if status != 404
                            || !response_id.is_some_and(|v| v.len() == 32 && v.bytes().all(|b| b.is_ascii_hexdigit()))
                        {
                            return Err(format!(
                                "early 404 must retain default ID: status={status}, id={response_id:?}"
                            ));
                        }
                        let trace = response_id.unwrap().to_owned();
                        response.bytes().await.map_err(|e| e.to_string())?;
                        check_log(&ctx, &trace).await?;
                        return Ok(());
                    }
                    if status != 200 {
                        return Err(format!("expected 200, got {status}"));
                    }
                    let body: Value = response.json().await.map_err(|e| e.to_string())?;
                    let upstream = body["headers"].as_object().ok_or("missing backend headers")?;
                    let get = |name: &str| upstream.get(name).and_then(Value::as_str);
                    let parent = get("traceparent").ok_or("missing core traceparent")?;
                    if !valid_parent(parent) {
                        return Err(format!("invalid parent: {parent}"));
                    }
                    if matches!(mode, "passthrough" | "multiple" | "source" | "custom") {
                        if parent != incoming_parent || get("tracestate") != Some("vendor=value") {
                            return Err("valid W3C context changed".into());
                        }
                    } else if !parent.ends_with("-00") {
                        return Err("new root must be unsampled".into());
                    }
                    if matches!(mode, "malformed" | "duplicate") && get("tracestate").is_some() {
                        return Err("orphan tracestate survived".into());
                    }
                    let ids = check_log(&ctx, &parent[3..35]).await?;
                    match mode {
                        "passthrough" => {
                            if response_id != Some(trace.as_str()) || get("x-trace-id") != response_id {
                                return Err("conflicting inbound x-trace-id must be replaced by the W3C ID".into());
                            }
                        }
                        "custom" => {
                            if response_id.is_some() || get("x-trace-id").is_some() {
                                return Err("disabled default survived".into());
                            }
                            let order = headers
                                .get("x-order-id")
                                .and_then(|v| v.to_str().ok())
                                .ok_or("missing custom response ID")?;
                            if !order.starts_with("order-") || order.len() != 38 || get("x-order-id") != Some(order) {
                                return Err("custom MD5 prefix/forwarding differs".into());
                            }
                            if ids["x-order-id"] != order {
                                return Err("generated custom ID missing from flat log map".into());
                            }
                        }
                        "multiple" | "source" => {
                            if get("x-otel-id") != Some(trace.as_str())
                                || headers.get("x-otel-id").and_then(|v| v.to_str().ok()) != Some(trace.as_str())
                            {
                                return Err("traceId source differs".into());
                            }
                            let expected_order = if mode == "source" { "business-42" } else { "first-order" };
                            if get("x-order-id") != Some(expected_order)
                                || headers.get("x-order-id").and_then(|v| v.to_str().ok()) != Some(expected_order)
                            {
                                return Err("target must win before source".into());
                            }
                            if get("x-private-id").is_none()
                                || headers.contains_key("x-private-id")
                                || get("x-response-id").is_some()
                                || !headers.contains_key("x-response-id")
                            {
                                return Err("forwardUpstream/echoResponse flags differ".into());
                            }
                        }
                        _ => {
                            if response_id != Some(&parent[3..35]) || get("x-trace-id") != response_id {
                                return Err("default must equal canonical W3C trace ID upstream and downstream".into());
                            }
                            if mode == "nogen"
                                && (get("x-optional-id").is_some() || headers.contains_key("x-optional-id"))
                            {
                                return Err("generateWhenMissing=false created custom ID".into());
                            }
                        }
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
    )
}

impl TestSuite for RequestIdPluginTestSuite {
    fn name(&self) -> &str {
        "RequestId Plugin Tests"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            case("request_id_default", "default"),
            case("request_id_conflicting_default_uses_w3c", "passthrough"),
            case("request_id_malformed_w3c", "malformed"),
            case("request_id_duplicate_w3c", "duplicate"),
            case("request_id_custom_no_generate", "nogen"),
            case("request_id_disable_and_custom_md5", "custom"),
            case("request_id_multiple_sources_and_flags", "multiple"),
            case("request_id_header_source", "source"),
            case("request_id_early_404", "early"),
        ]
    }
}
