use std::time::Instant;

use serde_json::{json, Value};

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};

const CARD_HOST: &str = "a2a-card.example.com";
const RPC_CLOSE_HOST: &str = "a2a-rpc-close.example.com";
const RPC_OPEN_HOST: &str = "a2a-rpc-open.example.com";
const DYNAMIC_HOST: &str = "a2a-card-dynamic.example.com";
const CARD_PATH: &str = "/.well-known/agent-card.json";
const EDGION_PLUGINS_PORT: u16 = 31180;

async fn get_card(ctx: &TestContext, host: &str) -> Result<reqwest::Response, String> {
    let base = if host == DYNAMIC_HOST {
        format!("http://{}:{EDGION_PLUGINS_PORT}", ctx.target_host)
    } else {
        ctx.http_url()
    };
    ctx.http_client
        .get(format!("{base}{CARD_PATH}"))
        .header("host", host)
        .send()
        .await
        .map_err(|error| format!("Agent Card request failed: {error}"))
}

async fn rpc(ctx: &TestContext, host: &str, version: &str, body: &'static str) -> Result<reqwest::Response, String> {
    ctx.http_client
        .post(format!("{}/echo", ctx.http_url()))
        .header("host", host)
        .header("content-type", "application/json")
        .header("a2a-version", version)
        .body(body)
        .send()
        .await
        .map_err(|error| format!("A2A JSON-RPC request failed: {error}"))
}

async fn assert_dynamic_card(ctx: &TestContext, version: &str, endpoint: &str) -> Result<(), String> {
    let response = get_card(ctx, DYNAMIC_HOST).await?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(format!("expected dynamic card 200, got {}", response.status()));
    }
    let card: Value = response
        .json()
        .await
        .map_err(|error| format!("dynamic Agent Card is invalid JSON: {error}"))?;
    if card["version"] != version || card["supportedInterfaces"][0]["url"] != endpoint {
        return Err(format!("unexpected dynamic Agent Card: {card}"));
    }
    Ok(())
}

pub struct A2aGatewayTestSuite;

impl A2aGatewayTestSuite {
    fn local_card_is_filtered_and_rewritten() -> TestCase {
        TestCase::new(
            "a2a_local_card_is_filtered_and_rewritten",
            "The backendless discovery route returns a bounded card with only JSONRPC/1.0 interfaces",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let response = match get_card(&ctx, CARD_HOST).await {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if response.status() != reqwest::StatusCode::OK {
                        return TestResult::failed(start.elapsed(), format!("expected 200, got {}", response.status()));
                    }
                    if response
                        .headers()
                        .get("cache-control")
                        .and_then(|value| value.to_str().ok())
                        != Some("no-store")
                    {
                        return TestResult::failed(start.elapsed(), "missing Cache-Control: no-store".to_string());
                    }
                    let card: Value = match response.json().await {
                        Ok(card) => card,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("invalid Agent Card JSON: {error}"))
                        }
                    };
                    let expected_urls = json!([
                        "https://public.agent.example:8443/rpc?tenant=blue",
                        "https://public.agent.example:8443/rpc-secondary"
                    ]);
                    let actual_urls = card["supportedInterfaces"]
                        .as_array()
                        .map(|interfaces| {
                            Value::Array(interfaces.iter().map(|interface| interface["url"].clone()).collect())
                        })
                        .unwrap_or(Value::Null);
                    if actual_urls != expected_urls
                        || card["supportedInterfaces"][0]["extensionField"] != "preserved"
                        || card["extensionObject"]["retained"] != true
                    {
                        return TestResult::failed(start.elapsed(), format!("unexpected rewritten card: {card}"));
                    }
                    TestResult::passed(start.elapsed())
                })
            },
        )
    }

    fn valid_json_rpc_is_forwarded_byte_for_byte() -> TestCase {
        const BODY: &str = "{\n  \"jsonrpc\" : \"2.0\", \"method\" : \"message/send\", \"id\" : 7, \"params\" : {}\n}";
        TestCase::new(
            "a2a_valid_json_rpc_is_forwarded_byte_for_byte",
            "A valid A2A protocol request is observed through BodyStorage and replayed unchanged",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match rpc(&ctx, RPC_CLOSE_HOST, "1.0", BODY).await {
                        Ok(response) if response.status() == reqwest::StatusCode::OK => match response.text().await {
                            Ok(body) if body.split_once("Echo: ").map(|(_, value)| value) == Some(BODY) => {
                                TestResult::passed(start.elapsed())
                            }
                            Ok(body) => TestResult::failed(
                                start.elapsed(),
                                format!("backend did not receive exact body: {body:?}"),
                            ),
                            Err(error) => TestResult::failed(start.elapsed(), format!("response read failed: {error}")),
                        },
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("expected 200, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    fn wrong_version_fails_closed() -> TestCase {
        TestCase::new(
            "a2a_wrong_version_fails_closed",
            "A mismatched A2A-Version is rejected before the body is observed when failOpen is false",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match rpc(&ctx, RPC_CLOSE_HOST, "0.3", r#"{"method":"message/send"}"#).await {
                        Ok(response) if response.status() == reqwest::StatusCode::BAD_REQUEST => {
                            TestResult::passed(start.elapsed())
                        }
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("expected 400, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    fn wrong_version_can_fail_open_without_body_observation() -> TestCase {
        const BODY: &str = "not-json-and-must-not-be-observed";
        TestCase::new(
            "a2a_wrong_version_can_fail_open_without_body_observation",
            "failOpen forwards a protocol-header mismatch without parsing or changing its body",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match rpc(&ctx, RPC_OPEN_HOST, "0.3", BODY).await {
                        Ok(response) if response.status() == reqwest::StatusCode::OK => match response.text().await {
                            Ok(body) if body.split_once("Echo: ").map(|(_, value)| value) == Some(BODY) => {
                                TestResult::passed(start.elapsed())
                            }
                            Ok(body) => {
                                TestResult::failed(start.elapsed(), format!("fail-open changed the body: {body:?}"))
                            }
                            Err(error) => TestResult::failed(start.elapsed(), format!("response read failed: {error}")),
                        },
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("expected 200, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }
}

impl TestSuite for A2aGatewayTestSuite {
    fn name(&self) -> &str {
        "A2aGateway"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::local_card_is_filtered_and_rewritten(),
            Self::valid_json_rpc_is_forwarded_byte_for_byte(),
            Self::wrong_version_fails_closed(),
            Self::wrong_version_can_fail_open_without_body_observation(),
        ]
    }
}

pub struct A2aGatewayDynamicV1TestSuite;

impl TestSuite for A2aGatewayDynamicV1TestSuite {
    fn name(&self) -> &str {
        "A2aGateway Dynamic V1"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "a2a_dynamic_card_v1",
            "The initial card snapshot serves v1",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match assert_dynamic_card(&ctx, "1.0.0", "https://dynamic.agent.example/rpc-v1?source=card").await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )]
    }
}

pub struct A2aGatewayDynamicV2TestSuite;

impl TestSuite for A2aGatewayDynamicV2TestSuite {
    fn name(&self) -> &str {
        "A2aGateway Dynamic V2"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "a2a_dynamic_card_v2",
            "The hot-reloaded card snapshot serves v2",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match assert_dynamic_card(&ctx, "2.0.0", "https://dynamic.agent.example/rpc-v2?source=card").await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )]
    }
}
