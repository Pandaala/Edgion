use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use reqwest::Method;
use serde_json::{json, Value};
use std::time::Instant;

const QUERY: &str = "query Read($id:ID!,$n:Int!=2){user(id:$id,n:$n){id}}";

async fn probe(
    ctx: &TestContext,
    variant: &str,
    method: Method,
    path: &str,
    body: &str,
    expected: u16,
    header: (Option<&str>, bool),
) -> Result<Option<Value>, String> {
    let (header, duplicate) = header;
    let marker = format!("rest-graphql-{}", uuid::Uuid::new_v4());
    let mut request = ctx
        .http_client
        .request(method, format!("{}{path}", ctx.http_url()))
        .header("host", format!("rest-graphql-{variant}.example.com"))
        .header("content-type", "application/json")
        .header("access_log", "test_store")
        .header("traceparent", crate::access_log_client::traceparent_for_marker(&marker));
    if let Some(value) = header {
        request = request.header("x-flag", value);
    }
    if duplicate {
        request = request.header("x-flag", "false");
    }
    if !body.is_empty() {
        request = request.body(body.to_owned());
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let cost = response
        .headers()
        .get("x-graphql-query-cost")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let media = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let value = response.json::<Value>().await.map_err(|e| e.to_string())?;
    if status != expected {
        return Err(format!("Expected {expected}, got {status}: {value}"));
    }
    let log = ctx
        .access_log_client()
        .get_access_log_with_retry(&marker, 15, 200)
        .await
        .map_err(|e| e.to_string())?;
    if log.status_code() != Some(u64::from(expected)) {
        return Err("Wrong canonical final status".into());
    }
    if expected != 200 {
        let configuration_error = variant == "dynamic" && expected == 500;
        let expected_body = if configuration_error {
            json!({"message":"plugin configuration error"})
        } else {
            let code = match expected {
                400 => "GRAPHQL_REQUEST_INVALID",
                405 => "METHOD_NOT_ALLOWED",
                413 => "PAYLOAD_TOO_LARGE",
                415 => "UNSUPPORTED_MEDIA_TYPE",
                _ => "INTERNAL_SERVER_ERROR",
            };
            json!({"errors":[{"message":"GraphQL request rejected","extensions":{"code":code}}]})
        };
        if log.upstream_count() != 0
            || value != expected_body
            || (!configuration_error && media.as_deref() != Some("application/json"))
        {
            return Err("Rejection escaped to origin or violated fixed JSON framing".into());
        }
        return Ok(None);
    }
    if log.backend_name() != Some("rest-graphql") || log.upstream_count() != 1 {
        return Err("Wrong declared backend identity or attempt count".into());
    }
    if value["data"]["method"] != "POST"
        || value["data"]["path"] != "/webhook/resolve"
        || value["data"]["query"] != ""
        || value["data"]["echo_headers"]["content-type"] != "application/json"
    {
        return Err(format!("Wrong generated transport: {value}"));
    }
    let envelope = &value["data"]["echo_body"];
    let bytes = serde_json::to_vec(envelope).map_err(|e| e.to_string())?;
    let expected_length = bytes.len().to_string();
    if value["data"]["echo_headers"]["content-length"].as_str() != Some(expected_length.as_str())
        || !value["data"]["echo_headers"]["transfer-encoding"].is_null()
    {
        return Err("Wrong canonical upstream body framing".into());
    }
    if variant == "basic" && cost.as_deref() != Some(if path.contains("n=3") { "5" } else { "4" }) {
        return Err(format!("Wrong generated-document cost: {cost:?}"));
    }
    Ok(Some(envelope.clone()))
}

fn result(start: Instant, outcome: Result<(), String>) -> TestResult {
    match outcome {
        Ok(()) => TestResult::passed(start.elapsed()),
        Err(error) => TestResult::failed(start.elapsed(), error),
    }
}

pub struct RestToGraphqlTestSuite;
impl TestSuite for RestToGraphqlTestSuite {
    fn name(&self) -> &str {
        "RestToGraphql Tests"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let mut cases = Vec::new();
        for (name, variant, method, path, body, status, header, duplicate, expected) in [
            (
                "path_ctx_query",
                "basic",
                Method::GET,
                "/users/42?n=3",
                "",
                200,
                None,
                false,
                Some(json!({"query":QUERY,"operationName":"Read","variables":{"id":"42","n":3}})),
            ),
            (
                "default_omitted",
                "basic",
                Method::GET,
                "/users/42",
                "",
                200,
                None,
                false,
                Some(json!({"query":QUERY,"operationName":"Read","variables":{"id":"42"}})),
            ),
            (
                "duplicate_query",
                "basic",
                Method::GET,
                "/users/42?n=1&%6e=2",
                "",
                400,
                None,
                false,
                None,
            ),
            (
                "invalid_query_escape",
                "basic",
                Method::GET,
                "/users/42?n=%GG",
                "",
                400,
                None,
                false,
                None,
            ),
            (
                "invalid_integer",
                "basic",
                Method::GET,
                "/users/42?n=2147483648",
                "",
                400,
                None,
                false,
                None,
            ),
            (
                "generated_cost_denial",
                "basic",
                Method::GET,
                "/users/42?n=9",
                "",
                400,
                None,
                false,
                None,
            ),
            (
                "missing_ctx",
                "basic",
                Method::GET,
                "/missing",
                "",
                400,
                None,
                false,
                None,
            ),
            (
                "get_rewrite_mutation",
                "mutation",
                Method::GET,
                "/users/42",
                "",
                405,
                None,
                false,
                None,
            ),
            (
                "post_mutation",
                "mutation",
                Method::POST,
                "/users/42",
                "",
                200,
                None,
                false,
                Some(json!({"query":"mutation Write{write}","variables":{}})),
            ),
            (
                "native_input",
                "body",
                Method::POST,
                "/create",
                "{\"payload\":{\"name\":\"safe\"},\"query\":\"untrusted\"}",
                200,
                Some("true"),
                false,
                Some(
                    json!({"query":"mutation Create($input:CreateInput!,$flag:Boolean!){create(input:$input,flag:$flag){id}}","variables":{"input":{"name":"safe"},"flag":true}}),
                ),
            ),
            (
                "native_null",
                "body",
                Method::POST,
                "/create",
                "{\"payload\":null}",
                400,
                Some("true"),
                false,
                None,
            ),
            (
                "duplicate_json",
                "body",
                Method::POST,
                "/create",
                "{\"payload\":{},\"payload\":{}}",
                400,
                Some("true"),
                false,
                None,
            ),
            (
                "duplicate_header",
                "body",
                Method::POST,
                "/create",
                "{\"payload\":{}}",
                400,
                Some("true"),
                true,
                None,
            ),
            (
                "inspection_before_adapter",
                "misordered",
                Method::POST,
                "/create",
                "{\"query\":\"{ping}\"}",
                500,
                None,
                false,
                None,
            ),
            (
                "skipped_inspection_does_not_freeze",
                "skipped",
                Method::GET,
                "/users/42",
                "",
                200,
                None,
                false,
                Some(json!({"query":"{ping}","variables":{}})),
            ),
            (
                "repeated_extension_adapter",
                "double",
                Method::GET,
                "/users/42",
                "",
                200,
                None,
                false,
                Some(json!({"query":"{ping}","variables":{}})),
            ),
        ] {
            cases.push(TestCase::new(
                format!("rest_graphql_{name}"),
                "Static operation mapping and bounded local failures",
                move |ctx| {
                    let method = method.clone();
                    let expected = expected.clone();
                    Box::pin(async move {
                        let start = Instant::now();
                        let outcome = async {
                            let envelope =
                                probe(&ctx, variant, method, path, body, status, (header, duplicate)).await?;
                            if envelope != expected {
                                return Err(format!("Wrong generated envelope: {envelope:?}"));
                            }
                            Ok(())
                        }
                        .await;
                        result(start, outcome)
                    })
                },
            ));
        }
        cases.push(TestCase::new(
            "rest_graphql_bodyless_replay_framing",
            "Generated GET body uses canonical upstream replay",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let outcome = async {
                        let expected = serde_json::to_vec(&json!({"query":"{ping}","variables":{}})).unwrap();
                        let response = ctx
                            .http_client
                            .get(format!("{}/users/42?ignored=1", ctx.http_url()))
                            .header("host", "rest-graphql-framing.example.com")
                            .send()
                            .await
                            .map_err(|e| e.to_string())?;
                        if response.status() != 200
                            || response.headers().get("x-body-len").and_then(|v| v.to_str().ok())
                                != Some(expected.len().to_string().as_str())
                            || response.headers().get("x-content-length").and_then(|v| v.to_str().ok())
                                != Some(expected.len().to_string().as_str())
                            || response
                                .headers()
                                .get("x-transfer-encoding")
                                .and_then(|v| v.to_str().ok())
                                != Some("absent")
                        {
                            return Err("Generated body framing mismatch".into());
                        }
                        Ok(())
                    }
                    .await;
                    result(start, outcome)
                })
            },
        ));
        cases
    }
}

pub struct RestToGraphqlDynamicSuite {
    pub invalid: bool,
    pub recovered: bool,
}
impl TestSuite for RestToGraphqlDynamicSuite {
    fn name(&self) -> &str {
        "RestToGraphql Dynamic Tests"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let invalid = self.invalid;
        let recovered = self.recovered;
        vec![TestCase::new(
            "rest_graphql_generation",
            "Invalid generation fails closed; valid update changes operation",
            move |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let outcome = async {
                        let envelope = probe(
                            &ctx,
                            "dynamic",
                            Method::GET,
                            "/users/42",
                            "",
                            if invalid { 500 } else { 200 },
                            (None, false),
                        )
                        .await?;
                        if !invalid
                            && envelope != Some(json!({"query":if recovered {"{pong}"}else{"{ping}"},"variables":{}}))
                        {
                            return Err("Stale adapter generation".into());
                        }
                        let resource = crate::suites::status::helpers::fetch_resource_status(
                            &ctx,
                            "EdgionPlugins",
                            "edgion-default",
                            "rest-graphql-dynamic",
                        )
                        .await?;
                        let conditions =
                            crate::suites::status::helpers::controller_local_conditions(&resource["status"])
                                .ok_or("Missing conditions")?;
                        let accepted = conditions
                            .iter()
                            .find(|c| c["type"] == "Accepted")
                            .ok_or("Missing Accepted")?;
                        if accepted["status"] != if invalid { "False" } else { "True" } {
                            return Err("Wrong validation status".into());
                        }
                        Ok(())
                    }
                    .await;
                    result(start, outcome)
                })
            },
        )]
    }
}
