use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use edgion_gateway::plugins::http::common::graphql_log_consts as log_tokens;
use reqwest::Method;
use serde_json::json;
use std::time::Instant;

async fn probe(
    ctx: &TestContext,
    host: &str,
    method: Method,
    target: &str,
    body: &str,
    expected: u16,
) -> Result<(), String> {
    probe_details(ctx, host, method, target, body, expected, None).await
}

async fn probe_details(
    ctx: &TestContext,
    host: &str,
    method: Method,
    target: &str,
    body: &str,
    expected: u16,
    observation: Option<(&str, Option<u32>)>,
) -> Result<(), String> {
    let marker = format!("graphql-guard-{}", uuid::Uuid::new_v4());
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
    let cost = response
        .headers()
        .get(log_tokens::COST_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let reply = response.text().await.map_err(|e| e.to_string())?;
    if status != expected {
        return Err(format!("Expected {expected}, got {status}: {reply}"));
    }
    if expected == 200 && !body.is_empty() && reply.split_once("Echo: ").map(|(_, b)| b) != Some(body) {
        return Err("Original request bytes changed".into());
    }
    if expected == 200 && !reply.contains("Server: 0.0.0.0:30001") {
        return Err("Wrong upstream backend".into());
    }
    if expected == 400 || expected == 403 {
        let code = if expected == 403 {
            "GRAPHQL_OPERATION_FORBIDDEN"
        } else {
            "GRAPHQL_REQUEST_INVALID"
        };
        let value: serde_json::Value = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
        if content_type.as_deref() != Some("application/json")
            || value != json!({"errors":[{"message":"GraphQL request rejected","extensions":{"code":code}}]})
        {
            return Err(format!("Unexpected GraphQL local error: {reply}"));
        }
    }
    if expected != 200 || observation.is_some() {
        let log = ctx
            .access_log_client()
            .get_access_log_with_retry(&marker, 15, 200)
            .await
            .map_err(|e| e.to_string())?;
        if (expected != 200 && log.upstream_count() != 0) || log.status_code() != Some(u64::from(expected)) {
            return Err("Local GraphQL rejection reached origin or recorded the wrong final status".into());
        }
        if let Some((entry, expected_cost)) = observation {
            if cost != expected_cost.map(|n| n.to_string()) {
                return Err("Unexpected or incomplete query cost header".into());
            }
            let entries: Vec<&str> = log
                .data
                .get("stage_logs")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .flat_map(|stage| {
                    stage
                        .get("edgion_plugins")
                        .and_then(|v| v.as_array())
                        .into_iter()
                        .flatten()
                })
                .flat_map(|group| group.get("logs").and_then(|v| v.as_array()).into_iter().flatten())
                .filter(|plugin| plugin.get("name").and_then(|v| v.as_str()) == Some("GraphqlGuard"))
                .flat_map(|plugin| plugin.get("log").and_then(|v| v.as_array()).into_iter().flatten())
                .filter_map(|v| v.as_str())
                .collect();
            if !entry.is_empty() && !entries.contains(&entry) {
                return Err(format!("Missing GraphQL observation {entry}: {entries:?}"));
            }
            if expected == 200 && log.upstream_count() != 1 {
                return Err("Measured request did not reach the declared origin exactly once".into());
            }
            if let Some(cost) = expected_cost {
                if !entries.contains(&format!("{}{cost}", log_tokens::COST).as_str()) {
                    return Err("Completed cost missing from canonical plugin log".into());
                }
            } else if expected != 200 && entries.iter().any(|entry| entry.starts_with(log_tokens::COST)) {
                return Err("Incomplete evaluation claimed an exact cost".into());
            }
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
pub struct GraphqlGuardTestSuite;
impl TestSuite for GraphqlGuardTestSuite {
    fn name(&self) -> &str {
        "GraphqlGuard Tests"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let mut cases = Vec::new();
        for (name, query, status) in [
            ("equality", "{ a:user { id } ping }", 200),
            ("depth", "{ user { child { id } } }", 400),
            ("aliases", "{ a:ping b:ping }", 400),
            ("roots", "{ ping pong other }", 400),
            ("fields", "{ user { id name } ping }", 400),
            (
                "fragment_depth",
                "{ ...F ... on Query { ping } } fragment F on Query { a:user { ...G } } fragment G on User { id }",
                200,
            ),
            (
                "fragment_repetition",
                "{ ...F ...F } fragment F on Query { a:ping }",
                400,
            ),
            (
                "directive_conservative",
                "{ a:ping @skip(if:true) b:ping @include(if:false) }",
                400,
            ),
            ("introspection_alias", "{ a:__schema { queryType } }", 400),
            (
                "introspection_fragment",
                "{ ...F } fragment F on Query { __type(name:\"User\") { name } }",
                400,
            ),
            ("typename", "{ __typename }", 200),
            ("malformed", "invalid @@", 400),
        ] {
            cases.push(TestCase::new(
                format!("graphql_guard_{name}"),
                "Bounded selected operation inspection",
                move |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let body = json!({"query":query}).to_string();
                        result(
                            start,
                            probe(&ctx, "graphql-guard.example.com", Method::POST, "/echo", &body, status).await,
                        )
                    })
                },
            ));
        }
        cases.push(TestCase::new("graphql_guard_selector_and_get","Only selected operation contributes shape; GET uses the same guard",|ctx|Box::pin(async move {
            let start=Instant::now(); let outcome=async {
                let body=json!({"query":"query Safe { ping } query Large { a:ping b:ping c:ping }","operationName":"Safe"}).to_string();
                probe(&ctx,"graphql-guard.example.com",Method::POST,"/echo",&body,200).await?;
                probe(&ctx,"graphql-guard.example.com",Method::GET,"/echo?query=%7Ba%3Aping%20b%3Aping%7D","",400).await?;
                probe(&ctx,"graphql-guard.example.com",Method::GET,"/echo?query=%7Bping%7D","",200).await
            }.await;result(start,outcome)
        })));
        for (name, query, status) in [
            ("allowed", "{ ping }", 200),
            ("alias_denial", "{ a:blocked }", 403),
            ("fragment_denial", "{ ...F } fragment F on Query { blocked }", 403),
            ("inline_denial", "{ ... on Query { blocked } }", 403),
            ("all_roots", "{ ping blocked }", 403),
            ("directive_denial", "{ blocked @skip(if:true) }", 403),
            ("nested_exclusion", "{ user { blocked } }", 200),
            ("mutation_allowed", "mutation M { update }", 200),
            ("mutation_denied", "mutation M { erase }", 403),
            ("typename_allowed", "{ __typename }", 200),
            ("introspection_denied", "{ a:__schema { queryType } }", 400),
        ] {
            cases.push(TestCase::new(
                format!("graphql_root_fields_{name}"),
                "Underlying root names obey query/mutation policies",
                move |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let body = json!({"query":query}).to_string();
                        result(
                            start,
                            probe(
                                &ctx,
                                "graphql-root-fields.example.com",
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
        for (name, mode, query, status, entry, cost) in [
            (
                "measure_all",
                "measure",
                "{ a:user { id } ping }",
                200,
                "measure exceeded=31",
                Some(4),
            ),
            ("enforce_all", "enforce", "{ a:user { id } ping }", 400, "", None),
            (
                "measure_introspection",
                "measure",
                "{ __schema { queryType { name } } }",
                400,
                "",
                None,
            ),
            ("measure_root_permission", "measure", "{ a:blocked }", 403, "", None),
            ("measure_invalid_field", "measure", "{ missing }", 400, "", None),
            ("measure_malformed", "measure", "invalid @@", 400, "", None),
            (
                "measure_selected",
                "measure",
                "query Safe { ping } query Large { a:ping b:ping }",
                200,
                "measure exceeded=0",
                Some(2),
            ),
        ] {
            cases.push(TestCase::new(
                format!("graphql_guard_{name}"),
                "Calibration preserves hard admission and exact cost reporting",
                move |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let host = format!("graphql-guard-{mode}.example.com");
                        let mut envelope = json!({"query":query});
                        if name == "measure_selected" {
                            envelope["operationName"] = json!("Safe");
                        }
                        result(
                            start,
                            probe_details(
                                &ctx,
                                &host,
                                Method::POST,
                                "/echo",
                                &envelope.to_string(),
                                status,
                                Some((entry, cost)),
                            )
                            .await,
                        )
                    })
                },
            ));
        }
        cases.push(TestCase::new(
            "graphql_guard_measure_get",
            "GET measurements share canonical transport",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    result(
                        start,
                        probe_details(
                            &ctx,
                            "graphql-guard-measure.example.com",
                            Method::GET,
                            "/echo?query=%7Ba%3Aping%7D",
                            "",
                            200,
                            Some(("measure exceeded=2", Some(2))),
                        )
                        .await,
                    )
                })
            },
        ));
        cases
    }
}
pub struct GraphqlGuardDynamicSuite {
    pub invalid: bool,
    pub strict: bool,
    pub root_policy: bool,
    pub calibration: bool,
    pub measure: bool,
}
impl TestSuite for GraphqlGuardDynamicSuite {
    fn name(&self) -> &str {
        "GraphqlGuard Dynamic Tests"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let invalid = self.invalid;
        let strict = self.strict;
        let root_policy = self.root_policy;
        let calibration = self.calibration;
        let measure = self.measure;
        vec![TestCase::new(
            "graphql_guard_generation",
            "Current guard and controller acceptance converge",
            move |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let outcome = async {
                        if calibration {
                            let above = json!({"query":"{ a:user { id } ping }"}).to_string();
                            probe_details(
                                &ctx,
                                "graphql-guard-dynamic.example.com",
                                Method::POST,
                                "/echo",
                                &above,
                                if invalid {
                                    500
                                } else if measure {
                                    200
                                } else {
                                    400
                                },
                                Some((
                                    if measure && !invalid { "measure exceeded=31" } else { "" },
                                    if measure && !invalid { Some(4) } else { None },
                                )),
                            )
                            .await?;
                            let denied = json!({"query":"{ blocked }"}).to_string();
                            probe(
                                &ctx,
                                "graphql-guard-dynamic.example.com",
                                Method::POST,
                                "/echo",
                                &denied,
                                if invalid { 500 } else { 403 },
                            )
                            .await?;
                        }
                        let body = json!({"query":"{ a:ping }"}).to_string();
                        probe(
                            &ctx,
                            "graphql-guard-dynamic.example.com",
                            Method::POST,
                            "/echo",
                            &body,
                            if invalid {
                                500
                            } else if root_policy {
                                403
                            } else if strict || (calibration && !measure) {
                                400
                            } else {
                                200
                            },
                        )
                        .await?;
                        let plain = json!({"query":"{ ping }"}).to_string();
                        probe(
                            &ctx,
                            "graphql-guard-dynamic.example.com",
                            Method::POST,
                            "/echo",
                            &plain,
                            if invalid {
                                500
                            } else if root_policy {
                                403
                            } else {
                                200
                            },
                        )
                        .await?;
                        if root_policy {
                            let allowed = json!({"query":"{pong}"}).to_string();
                            probe(
                                &ctx,
                                "graphql-guard-dynamic.example.com",
                                Method::POST,
                                "/echo",
                                &allowed,
                                200,
                            )
                            .await?;
                        }
                        let status = crate::suites::status::helpers::fetch_resource_status(
                            &ctx,
                            "EdgionPlugins",
                            "edgion-default",
                            "graphql-guard-dynamic",
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
