use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::status::helpers;
use std::time::Instant;

const STATIC_HOST: &str = "json-schema-validation.example.com";
const DEFAULT_LIMIT_HOST: &str = "json-schema-validation-default-limit.example.com";
const DYNAMIC_HOST: &str = "json-schema-validation-dynamic.example.com";
const DYNAMIC_PLUGIN: &str = "json-schema-validation-dynamic";
const NAMESPACE: &str = "edgion-default";
const EDGION_PLUGINS_PORT: u16 = 31180;

async fn post_to(
    ctx: &TestContext,
    base_url: &str,
    host: &str,
    body: Vec<u8>,
    content_type: Option<&str>,
    content_encoding: Option<&str>,
) -> Result<(u16, String), String> {
    let mut request = ctx
        .http_client
        .post(format!("{base_url}/echo"))
        .header("host", host)
        .body(body);
    if let Some(content_type) = content_type {
        request = request.header("content-type", content_type);
    }
    if let Some(content_encoding) = content_encoding {
        request = request.header("content-encoding", content_encoding);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("Request failed: {error}"))?;
    let status = response.status().as_u16();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Read response failed: {error}"))?;
    Ok((status, body))
}

async fn post(
    ctx: &TestContext,
    host: &str,
    body: Vec<u8>,
    content_type: Option<&str>,
    content_encoding: Option<&str>,
) -> Result<(u16, String), String> {
    // Dynamic scenarios default `ctx.http_url()` to the Gateway's default
    // listener (31000). Dynamic JSON Schema traffic belongs to the EdgionPlugins
    // listener, matching the other EdgionPlugins hot-reload suites.
    let base_url = if host == DYNAMIC_HOST {
        format!("http://{}:{EDGION_PLUGINS_PORT}", ctx.target_host)
    } else {
        ctx.http_url()
    };
    post_to(ctx, &base_url, host, body, content_type, content_encoding).await
}

async fn post_with_content_types(
    ctx: &TestContext,
    host: &str,
    body: Vec<u8>,
    content_types: &[&str],
) -> Result<(u16, String), String> {
    let mut request = ctx
        .http_client
        .post(format!("{}/echo", ctx.http_url()))
        .header("host", host)
        .body(body);
    for content_type in content_types {
        request = request.header("content-type", *content_type);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("Request failed: {error}"))?;
    let status = response.status().as_u16();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Read response failed: {error}"))?;
    Ok((status, body))
}

fn expect_status(start: Instant, result: Result<(u16, String), String>, status: u16, body: Option<&str>) -> TestResult {
    match result {
        Ok((actual_status, actual_body))
            if actual_status == status && body.is_none_or(|expected| actual_body == expected) =>
        {
            TestResult::passed(start.elapsed())
        }
        Ok((actual_status, actual_body)) => TestResult::failed(
            start.elapsed(),
            format!("Expected status {status} and body {body:?}, got status {actual_status} and body {actual_body:?}"),
        ),
        Err(error) => TestResult::failed(start.elapsed(), error),
    }
}

fn echo_body(response: &str) -> Option<&str> {
    response.split_once("Echo: ").map(|(_, body)| body)
}

async fn accepted_status(ctx: &TestContext) -> Result<String, String> {
    let resource = helpers::fetch_resource_status(ctx, "EdgionPlugins", NAMESPACE, DYNAMIC_PLUGIN).await?;
    let conditions = resource["status"]["conditions"]
        .as_array()
        .ok_or_else(|| format!("{DYNAMIC_PLUGIN} has no status.conditions"))?;
    conditions
        .iter()
        .find(|condition| condition["type"].as_str() == Some("Accepted"))
        .and_then(|condition| condition["status"].as_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{DYNAMIC_PLUGIN} has no Accepted condition"))
}

async fn expect_dynamic_status_and_traffic(
    ctx: &TestContext,
    expected_accepted: &str,
    version: &str,
    expected_status: u16,
) -> Result<(), String> {
    let accepted = accepted_status(ctx).await?;
    if accepted != expected_accepted {
        return Err(format!(
            "Expected Accepted={expected_accepted}, got Accepted={accepted}"
        ));
    }
    let body = format!(r#"{{"version":"{version}"}}"#).into_bytes();
    let (status, response_body) = post(ctx, DYNAMIC_HOST, body, Some("application/json"), None).await?;
    if status != expected_status {
        return Err(format!(
            "Expected traffic status {expected_status}, got {status} with body {response_body:?}"
        ));
    }
    if expected_status == 500 && response_body != r#"{"message":"plugin configuration error"}"# {
        return Err(format!("Expected generic fail-closed body, got {response_body:?}"));
    }
    Ok(())
}

pub struct JsonSchemaValidationTestSuite;

impl JsonSchemaValidationTestSuite {
    fn valid_body_replayed_byte_for_byte() -> TestCase {
        TestCase::new(
            "json_schema_valid_body_replayed_byte_for_byte",
            "A valid JSON body reaches the backend without re-encoding",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let body = b"{\n  \"payload\" : \"preserve spacing\",\n  \"name\" : \"edge\"\n}".to_vec();
                    match post(&ctx, STATIC_HOST, body.clone(), Some("application/json"), None).await {
                        Ok((200, response)) if echo_body(&response) == std::str::from_utf8(&body).ok() => {
                            TestResult::passed(start.elapsed())
                        }
                        Ok((status, response)) => TestResult::failed(
                            start.elapsed(),
                            format!("Expected byte-identical backend echo, got status {status} and body {response:?}"),
                        ),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    fn vendor_json_is_accepted() -> TestCase {
        TestCase::new(
            "json_schema_vendor_json_is_accepted",
            "An application/*+json media type with UTF-8 is accepted",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let body = br#"{"name":"edge","payload":"ok"}"#.to_vec();
                    let result = post(
                        &ctx,
                        STATIC_HOST,
                        body,
                        Some("application/problem+json; charset=UTF-8; profile=example"),
                        None,
                    )
                    .await;
                    expect_status(start, result, 200, None)
                })
            },
        )
    }

    fn malformed_json_is_rejected() -> TestCase {
        TestCase::new(
            "json_schema_malformed_json_is_rejected",
            "Malformed JSON returns the fixed 400 response",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = post(
                        &ctx,
                        STATIC_HOST,
                        br#"{"name":"edge","payload":"unterminated}"#.to_vec(),
                        Some("application/json"),
                        None,
                    )
                    .await;
                    expect_status(start, result, 400, Some("Invalid request body"))
                })
            },
        )
    }

    fn schema_mismatch_is_rejected() -> TestCase {
        TestCase::new(
            "json_schema_mismatch_is_rejected",
            "A schema mismatch returns the same fixed 400 response",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = post(
                        &ctx,
                        STATIC_HOST,
                        br#"{"name":"","payload":"ok"}"#.to_vec(),
                        Some("application/json"),
                        None,
                    )
                    .await;
                    expect_status(start, result, 400, Some("Invalid request body"))
                })
            },
        )
    }

    fn duplicate_json_key_is_rejected() -> TestCase {
        TestCase::new(
            "json_schema_duplicate_json_key_is_rejected",
            "Duplicate JSON object keys are rejected instead of using last-value-wins",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = post(
                        &ctx,
                        STATIC_HOST,
                        br#"{"name":"first","name":"second","payload":"ok"}"#.to_vec(),
                        Some("application/json"),
                        None,
                    )
                    .await;
                    expect_status(start, result, 400, Some("Invalid request body"))
                })
            },
        )
    }

    fn empty_body_is_rejected_before_media_type() -> TestCase {
        TestCase::new(
            "json_schema_empty_body_is_rejected_before_media_type",
            "An empty body returns 400 even without Content-Type",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = post(&ctx, STATIC_HOST, Vec::new(), None, None).await;
                    expect_status(start, result, 400, Some("Invalid request body"))
                })
            },
        )
    }

    fn missing_content_type_is_rejected() -> TestCase {
        TestCase::new(
            "json_schema_missing_content_type_is_rejected",
            "A non-empty body without Content-Type returns 415",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = post(
                        &ctx,
                        STATIC_HOST,
                        br#"{"name":"edge","payload":"ok"}"#.to_vec(),
                        None,
                        None,
                    )
                    .await;
                    expect_status(start, result, 415, Some("Unsupported media type"))
                })
            },
        )
    }

    fn wrong_content_type_is_rejected() -> TestCase {
        TestCase::new(
            "json_schema_wrong_content_type_is_rejected",
            "A non-JSON media type returns 415",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = post(
                        &ctx,
                        STATIC_HOST,
                        br#"{"name":"edge","payload":"ok"}"#.to_vec(),
                        Some("text/plain"),
                        None,
                    )
                    .await;
                    expect_status(start, result, 415, Some("Unsupported media type"))
                })
            },
        )
    }

    fn non_utf8_charset_is_rejected() -> TestCase {
        TestCase::new(
            "json_schema_non_utf8_charset_is_rejected",
            "A non-UTF-8 JSON charset returns 415",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = post(
                        &ctx,
                        STATIC_HOST,
                        br#"{"name":"edge","payload":"ok"}"#.to_vec(),
                        Some("application/json; charset=utf-16"),
                        None,
                    )
                    .await;
                    expect_status(start, result, 415, Some("Unsupported media type"))
                })
            },
        )
    }

    fn duplicate_content_type_is_rejected() -> TestCase {
        TestCase::new(
            "json_schema_duplicate_content_type_is_rejected",
            "Duplicate Content-Type fields return 415",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = post_with_content_types(
                        &ctx,
                        STATIC_HOST,
                        br#"{"name":"edge","payload":"ok"}"#.to_vec(),
                        &["application/json", "application/problem+json"],
                    )
                    .await;
                    expect_status(start, result, 415, Some("Unsupported media type"))
                })
            },
        )
    }

    fn encoded_body_is_rejected() -> TestCase {
        TestCase::new(
            "json_schema_encoded_body_is_rejected",
            "A non-identity Content-Encoding returns 415",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = post(
                        &ctx,
                        STATIC_HOST,
                        br#"{"name":"edge","payload":"ok"}"#.to_vec(),
                        Some("application/json"),
                        Some("gzip"),
                    )
                    .await;
                    expect_status(start, result, 415, Some("Unsupported media type"))
                })
            },
        )
    }

    fn default_limit_is_enforced() -> TestCase {
        TestCase::new(
            "json_schema_default_limit_is_enforced",
            "The default 64 KiB JSON body limit returns 413",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let body = serde_json::to_vec(&serde_json::json!({"payload": "x".repeat(70 * 1024)}))
                        .expect("test JSON is serializable");
                    let result = post(&ctx, DEFAULT_LIMIT_HOST, body, Some("application/json"), None).await;
                    expect_status(start, result, 413, Some(""))
                })
            },
        )
    }

    fn forced_spill_replays_byte_for_byte() -> TestCase {
        TestCase::new(
            "json_schema_forced_spill_replays_byte_for_byte",
            "A valid body above the 128 KiB memory threshold is validated from spill storage and replayed unchanged",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let body = serde_json::to_vec(&serde_json::json!({
                        "name": "spill",
                        "payload": "s".repeat(192 * 1024)
                    }))
                    .expect("test JSON is serializable");
                    match post(&ctx, STATIC_HOST, body.clone(), Some("application/json"), None).await {
                        Ok((200, response)) if echo_body(&response) == std::str::from_utf8(&body).ok() => {
                            TestResult::passed(start.elapsed())
                        }
                        Ok((status, response)) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected byte-identical spill replay, got status {status} and {} response bytes",
                                response.len()
                            ),
                        ),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    fn explicit_limit_is_enforced() -> TestCase {
        TestCase::new(
            "json_schema_explicit_limit_is_enforced",
            "A JSON body above the configured 512 KiB limit returns 413",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let body = serde_json::to_vec(&serde_json::json!({
                        "name": "too-large",
                        "payload": "x".repeat(520 * 1024)
                    }))
                    .expect("test JSON is serializable");
                    let result = post(&ctx, STATIC_HOST, body, Some("application/json"), None).await;
                    expect_status(start, result, 413, Some(""))
                })
            },
        )
    }
}

impl TestSuite for JsonSchemaValidationTestSuite {
    fn name(&self) -> &str {
        "JsonSchemaValidation Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::valid_body_replayed_byte_for_byte(),
            Self::vendor_json_is_accepted(),
            Self::malformed_json_is_rejected(),
            Self::schema_mismatch_is_rejected(),
            Self::duplicate_json_key_is_rejected(),
            Self::empty_body_is_rejected_before_media_type(),
            Self::missing_content_type_is_rejected(),
            Self::wrong_content_type_is_rejected(),
            Self::non_utf8_charset_is_rejected(),
            Self::duplicate_content_type_is_rejected(),
            Self::encoded_body_is_rejected(),
            Self::default_limit_is_enforced(),
            Self::forced_spill_replays_byte_for_byte(),
            Self::explicit_limit_is_enforced(),
        ]
    }
}

pub struct JsonSchemaValidationDynamicInvalidTestSuite;

impl TestSuite for JsonSchemaValidationDynamicInvalidTestSuite {
    fn name(&self) -> &str {
        "JsonSchemaValidation Dynamic - Invalid"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "json_schema_dynamic_invalid",
            "Invalid schema configuration is Accepted=False and fails traffic closed",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match expect_dynamic_status_and_traffic(&ctx, "False", "v1", 500).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )]
    }
}

pub struct JsonSchemaValidationDynamicV1TestSuite;

impl TestSuite for JsonSchemaValidationDynamicV1TestSuite {
    fn name(&self) -> &str {
        "JsonSchemaValidation Dynamic - Valid V1"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            TestCase::new(
                "json_schema_dynamic_v1_accepts_v1",
                "The valid v1 schema is Accepted=True and accepts v1",
                |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        match expect_dynamic_status_and_traffic(&ctx, "True", "v1", 200).await {
                            Ok(()) => TestResult::passed(start.elapsed()),
                            Err(error) => TestResult::failed(start.elapsed(), error),
                        }
                    })
                },
            ),
            TestCase::new(
                "json_schema_dynamic_v1_rejects_v2",
                "The valid v1 schema rejects v2",
                |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let body = br#"{"version":"v2"}"#.to_vec();
                        let result = post(&ctx, DYNAMIC_HOST, body, Some("application/json"), None).await;
                        expect_status(start, result, 400, Some("Invalid request body"))
                    })
                },
            ),
        ]
    }
}

pub struct JsonSchemaValidationDynamicV2TestSuite;

impl TestSuite for JsonSchemaValidationDynamicV2TestSuite {
    fn name(&self) -> &str {
        "JsonSchemaValidation Dynamic - Valid V2"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            TestCase::new(
                "json_schema_dynamic_v2_accepts_v2",
                "The recovered v2 schema is Accepted=True and accepts v2",
                |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        match expect_dynamic_status_and_traffic(&ctx, "True", "v2", 200).await {
                            Ok(()) => TestResult::passed(start.elapsed()),
                            Err(error) => TestResult::failed(start.elapsed(), error),
                        }
                    })
                },
            ),
            TestCase::new(
                "json_schema_dynamic_v2_rejects_v1",
                "The recovered v2 schema rejects v1",
                |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let body = br#"{"version":"v1"}"#.to_vec();
                        let result = post(&ctx, DYNAMIC_HOST, body, Some("application/json"), None).await;
                        expect_status(start, result, 400, Some("Invalid request body"))
                    })
                },
            ),
        ]
    }
}
