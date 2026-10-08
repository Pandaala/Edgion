use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use reqwest::Method;
use serde_json::{json, Value};
use std::time::Instant;

async fn probe(
    ctx: &TestContext,
    host: &str,
    method: Method,
    target: &str,
    body: &str,
    expected: u16,
) -> Result<(), String> {
    let marker = format!("graphql-safelist-{}", uuid::Uuid::new_v4());
    let response = ctx
        .http_client
        .request(method, format!("{}{target}", ctx.http_url()))
        .header("host", host)
        .header("content-type", "application/json")
        .header("access_log", "test_store")
        .header("traceparent", crate::access_log_client::traceparent_for_marker(&marker))
        .body(body.to_owned())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let reply = response.text().await.map_err(|e| e.to_string())?;
    if status != expected {
        return Err(format!("Expected {expected}, got {status}: {reply}"));
    }
    if expected == 200 {
        if !reply.contains("Server: 0.0.0.0:30001") {
            return Err("Wrong upstream backend".into());
        }
        if !body.is_empty() && reply.split_once("Echo: ").map(|(_, b)| b) != Some(body) {
            return Err("Original request bytes changed".into());
        }
    }
    if [400, 403, 405].contains(&expected) {
        let code = match expected {
            403 => "GRAPHQL_OPERATION_FORBIDDEN",
            405 => "METHOD_NOT_ALLOWED",
            _ => "GRAPHQL_REQUEST_INVALID",
        };
        let value: Value = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
        if content_type.as_deref() != Some("application/json")
            || value != json!({"errors":[{"message":"GraphQL request rejected","extensions":{"code":code}}]})
        {
            return Err(format!("Unexpected GraphQL local error: {reply}"));
        }
    }
    if expected != 200 {
        let log = ctx
            .access_log_client()
            .get_access_log_with_retry(&marker, 15, 200)
            .await
            .map_err(|e| e.to_string())?;
        if log.upstream_count() != 0 || log.status_code() != Some(u64::from(expected)) {
            return Err("Local GraphQL rejection reached origin or recorded the wrong final status".into());
        }
    }
    Ok(())
}
fn result(start: Instant, outcome: Result<(), String>) -> TestResult {
    match outcome {
        Ok(()) => TestResult::passed(start.elapsed()),
        Err(e) => TestResult::failed(start.elapsed(), e),
    }
}

pub struct GraphqlSafelistTestSuite;
impl TestSuite for GraphqlSafelistTestSuite {
    fn name(&self) -> &str {
        "GraphqlSafelist Tests"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let mut cases = Vec::new();
        for (name, query, selector, variables, status) in [
            ("formatting", "# ignored\n{ping}", None, json!({}), 200),
            ("other_field", "{ pong }", None, json!({}), 403),
            ("alias_change", "{ a:ping }", None, json!({}), 403),
            ("literal_change", "{ ping(n:1) }", None, json!({}), 403),
            (
                "variables_one",
                "query Read($n:Int) { ping(n:$n) }",
                Some("Read"),
                json!({"n":1}),
                200,
            ),
            (
                "variables_other",
                "query Read($n:Int) { ping(n:$n) }",
                None,
                json!({"n":100}),
                200,
            ),
            (
                "variable_definition",
                "query Read($n:Int=1) { ping(n:$n) }",
                None,
                json!({}),
                403,
            ),
            (
                "registered_selection",
                "query Chosen { ping } mutation Hidden { update }",
                Some("Chosen"),
                json!({}),
                200,
            ),
            (
                "unregistered_selection",
                "query Chosen { ping } mutation Hidden { update }",
                Some("Hidden"),
                json!({}),
                403,
            ),
            (
                "selector_mismatch",
                "query Chosen { ping } mutation Hidden { update }",
                Some("Missing"),
                json!({}),
                400,
            ),
            ("malformed", "invalid @@", None, json!({}), 400),
        ] {
            cases.push(TestCase::new(
                format!("graphql_safelist_{name}"),
                "Whole document and selected operation authorization",
                move |ctx| {
                    let variables = variables.clone();
                    Box::pin(async move {
                        let start = Instant::now();
                        let body = json!({"query":query,"operationName":selector,"variables":variables}).to_string();
                        result(
                            start,
                            probe(
                                &ctx,
                                "graphql-safelist.example.com",
                                Method::POST,
                                "/echo",
                                &body,
                                status,
                            )
                            .await,
                        )
                    })
                },
            ));
        }
        cases.push(TestCase::new(
            "graphql_safelist_get",
            "GET uses the same document trust and mutation rules",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let outcome = async {
                        probe(
                            &ctx,
                            "graphql-safelist.example.com",
                            Method::GET,
                            "/echo?query=%7Bping%7D",
                            "",
                            200,
                        )
                        .await?;
                        probe(
                            &ctx,
                            "graphql-safelist.example.com",
                            Method::GET,
                            "/echo?query=%7Bpong%7D",
                            "",
                            403,
                        )
                        .await?;
                        probe(
                            &ctx,
                            "graphql-safelist.example.com",
                            Method::GET,
                            "/echo?query=mutation%20M%7Bupdate%7D",
                            "",
                            405,
                        )
                        .await
                    }
                    .await;
                    result(start, outcome)
                })
            },
        ));
        cases
    }
}

pub struct GraphqlSafelistDynamicSuite {
    pub invalid: bool,
    pub revoked: bool,
}
impl TestSuite for GraphqlSafelistDynamicSuite {
    fn name(&self) -> &str {
        "GraphqlSafelist Dynamic Tests"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let invalid = self.invalid;
        let revoked = self.revoked;
        vec![TestCase::new(
            "graphql_safelist_generation",
            "Invalid updates fail closed and valid updates revoke documents",
            move |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let outcome = async {
                        for (query, status) in [
                            ("{ping}", if invalid { 500 } else { 200 }),
                            (
                                "{pong}",
                                if invalid {
                                    500
                                } else if revoked {
                                    403
                                } else {
                                    200
                                },
                            ),
                        ] {
                            let body = json!({"query":query}).to_string();
                            probe(
                                &ctx,
                                "graphql-safelist-dynamic.example.com",
                                Method::POST,
                                "/echo",
                                &body,
                                status,
                            )
                            .await?;
                        }
                        let status = crate::suites::status::helpers::fetch_resource_status(
                            &ctx,
                            "EdgionPlugins",
                            "edgion-default",
                            "graphql-safelist-dynamic",
                        )
                        .await?;
                        let accepted = status["status"]["conditions"]
                            .as_array()
                            .ok_or("Missing plugin conditions")?
                            .iter()
                            .find(|c| c["type"] == "Accepted")
                            .ok_or("Missing Accepted")?;
                        if accepted["status"] != if invalid { "False" } else { "True" } {
                            return Err("Controller acceptance did not converge".into());
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
