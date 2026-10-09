use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use reqwest::Method;
use std::time::Instant;
const HOST: &str = "graphql-router.example.com";
const DEFAULT_HOST: &str = "graphql-router-default.example.com";
const DYNAMIC_HOST: &str = "graphql-router-dynamic.example.com";
const QUERY: &str = r#"{"query":"query GetUser { user { id } }"}"#;
async fn request(
    ctx: &TestContext,
    host: &str,
    method: Method,
    target: &str,
    body: &str,
    media: &str,
) -> Result<(u16, String), String> {
    let reply = ctx
        .http_client
        .request(method, format!("{}{target}", ctx.http_url()))
        .header("host", host)
        .header("content-type", media)
        .body(body.to_owned())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    Ok((reply.status().as_u16(), reply.text().await.map_err(|e| e.to_string())?))
}
fn require(reply: &(u16, String), status: u16, port: Option<u16>, echo: Option<&str>) -> Result<(), String> {
    if reply.0 != status {
        return Err(format!("Expected status {status}, got {}: {}", reply.0, reply.1));
    }
    if let Some(port) = port {
        if !reply.1.contains(&format!("Server: 0.0.0.0:{port}")) {
            return Err(format!("Wrong backend: {}", reply.1));
        }
    }
    if let Some(body) = echo {
        if reply.1.split_once("Echo: ").map(|(_, b)| b) != Some(body) {
            return Err(format!("Request bytes changed: {}", reply.1));
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
pub struct GraphqlOperationRouterTestSuite;
impl TestSuite for GraphqlOperationRouterTestSuite {
    fn name(&self) -> &str {
        "GraphqlOperationRouter Tests"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let mut cases = Vec::new();
        for (name, host, body, port) in [
            ("query_exact", HOST, QUERY, 30001),
            (
                "mutation_exact",
                HOST,
                r#"{"query":"mutation CreateOrder { createOrder { id } }"}"#,
                30002,
            ),
            (
                "multi_operation_selector",
                HOST,
                r#"{"query":"query GetUser { user { id } } mutation CreateOrder { createOrder { id } }","operationName":"CreateOrder"}"#,
                30002,
            ),
            (
                "original_body_and_variables",
                HOST,
                "{\n  \"query\" : \"query GetUser($id: ID!) { user(id:$id) { id } }\", \"variables\" : {\"id\":42}\n}",
                30001,
            ),
            (
                "anonymous_default",
                DEFAULT_HOST,
                r#"{"query":"{ user { id } }"}"#,
                30003,
            ),
            (
                "unmapped_default",
                DEFAULT_HOST,
                r#"{"query":"query Unknown { user { id } }"}"#,
                30003,
            ),
        ] {
            cases.push(TestCase::new(
                format!("graphql_route_{name}"),
                "Selected backend receives the original document",
                move |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let outcome = async {
                            require(
                                &request(&ctx, host, Method::POST, "/echo", body, "application/json").await?,
                                200,
                                Some(port),
                                Some(body),
                            )
                        }
                        .await;
                        result(start, outcome)
                    })
                },
            ));
        }
        for (name, body, status, media) in [
            (
                "anonymous_unmapped",
                r#"{"query":"{ user { id } }"}"#,
                400,
                "application/json",
            ),
            (
                "unknown_name",
                r#"{"query":"query Unknown { user { id } }"}"#,
                400,
                "application/json",
            ),
            (
                "case_sensitive",
                r#"{"query":"query getUser { user { id } }"}"#,
                400,
                "application/json",
            ),
            (
                "missing_selector",
                r#"{"query":"query GetUser { user { id } } mutation CreateOrder { createOrder { id } }"}"#,
                400,
                "application/json",
            ),
            (
                "wrong_selector",
                r#"{"query":"query GetUser { user { id } }","operationName":"Absent"}"#,
                400,
                "application/json",
            ),
            (
                "batch",
                r#"[{"query":"query GetUser { user { id } }"}]"#,
                400,
                "application/json",
            ),
            (
                "duplicate_json",
                r#"{"query":"query GetUser { user { id } }","query":"{ user }"}"#,
                400,
                "application/json",
            ),
            (
                "apq",
                r#"{"query":"query GetUser { user { id } }","extensions":{"persistedQuery":{}}}"#,
                400,
                "application/json",
            ),
            (
                "subscription",
                r#"{"query":"subscription GetUser { user { id } }"}"#,
                400,
                "application/json",
            ),
            (
                "denied_grant",
                r#"{"query":"query GetRemote { user { id } }"}"#,
                403,
                "application/json",
            ),
            ("wrong_media", QUERY, 415, "text/plain"),
        ] {
            cases.push(TestCase::new(
                format!("graphql_route_{name}"),
                "Invalid or unauthorized selection fails closed",
                move |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let outcome = async {
                            require(
                                &request(&ctx, HOST, Method::POST, "/echo", body, media).await?,
                                status,
                                None,
                                None,
                            )
                        }
                        .await;
                        result(start, outcome)
                    })
                },
            ));
        }
        cases.push(TestCase::new(
            "graphql_route_get_and_limits",
            "GET queries, GET mutation and body cap",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let outcome = async {
                        let replay = request(
                            &ctx,
                            HOST,
                            Method::GET,
                            "/graphql-routing-get?query=query%20GetUser%20%7Buser%7Bid%7D%7D",
                            "",
                            "application/json",
                        )
                        .await?;
                        if !replay.1.contains("Query: query=query%20GetUser%20%7Buser%7Bid%7D%7D\n")
                            || !replay.1.contains("Path: /graphql-routing-get\n")
                        {
                            return Err(format!("GET URI changed: {}", replay.1));
                        }
                        require(
                            &request(
                                &ctx,
                                HOST,
                                Method::GET,
                                "/graphql-routing-get?query=query%20GetUser%20%7Buser%7Bid%7D%7D",
                                "",
                                "application/json",
                            )
                            .await?,
                            200,
                            Some(30001),
                            None,
                        )?;
                        require(
                            &request(
                                &ctx,
                                HOST,
                                Method::GET,
                                "/echo?query=mutation%20CreateOrder%20%7BcreateOrder%7Bid%7D%7D",
                                "",
                                "application/json",
                            )
                            .await?,
                            405,
                            None,
                            None,
                        )?;
                        require(
                            &request(
                                &ctx,
                                HOST,
                                Method::POST,
                                "/echo?query=ignored",
                                QUERY,
                                "application/json",
                            )
                            .await?,
                            400,
                            None,
                            None,
                        )?;
                        require(
                            &request(
                                &ctx,
                                HOST,
                                Method::POST,
                                "/echo",
                                &"x".repeat(65537),
                                "application/json",
                            )
                            .await?,
                            413,
                            None,
                            None,
                        )?;
                        Ok(())
                    }
                    .await;
                    result(start, outcome)
                })
            },
        ));
        cases.push(TestCase::new(
            "graphql_route_selected_backend_owner",
            "Only selected backend AI owners are incompatible",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let outcome = async {
                        let host = "graphql-router-backend-owner.example.com";
                        require(
                            &request(&ctx, host, Method::POST, "/echo", QUERY, "application/json").await?,
                            500,
                            None,
                            None,
                        )?;
                        require(
                            &request(
                                &ctx,
                                host,
                                Method::POST,
                                "/echo",
                                r#"{"query":"mutation CreateOrder { createOrder { id } }"}"#,
                                "application/json",
                            )
                            .await?,
                            200,
                            Some(30002),
                            None,
                        )?;
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
pub struct GraphqlOperationRouterDynamicSuite {
    pub port: Option<u16>,
    pub remote_allowed: bool,
}
impl TestSuite for GraphqlOperationRouterDynamicSuite {
    fn name(&self) -> &str {
        "GraphqlOperationRouter Dynamic Tests"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let port = self.port;
        let remote_allowed = self.remote_allowed;
        vec![TestCase::new(
            "graphql_route_current_generation",
            "Plugin and route updates revoke invalid mappings",
            move |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let outcome = async {
                        require(
                            &request(&ctx, DYNAMIC_HOST, Method::POST, "/echo", QUERY, "application/json").await?,
                            if port.is_some() { 200 } else { 404 },
                            port,
                            port.map(|_| QUERY),
                        )?;
                        if port.is_some() {
                            let remote = request(
                                &ctx,
                                DYNAMIC_HOST,
                                Method::POST,
                                "/echo",
                                r#"{"query":"query GetRemote { user { id } }"}"#,
                                "application/json",
                            )
                            .await?;
                            require(
                                &remote,
                                if remote_allowed { 200 } else { 403 },
                                if remote_allowed { Some(30002) } else { None },
                                None,
                            )?;
                        }
                        let response = ctx
                            .http_client
                            .get(format!("{}/health", ctx.http_url()))
                            .header("host", DYNAMIC_HOST)
                            .send()
                            .await
                            .map_err(|e| e.to_string())?;
                        if !response.status().is_success() {
                            return Err(format!("Sibling rule failed: {}", response.status()));
                        }
                        let status = crate::suites::status::helpers::fetch_resource_status(
                            &ctx,
                            "HTTPRoute",
                            "edgion-default",
                            "graphql-router-dynamic",
                        )
                        .await?;
                        let parents = status["status"]["parents"]
                            .as_array()
                            .ok_or_else(|| "Missing HTTPRoute parent status".to_owned())?;
                        if parents.is_empty() {
                            return Err("Missing HTTPRoute attachment status".into());
                        }
                        for parent in parents {
                            let conditions = parent["conditions"]
                                .as_array()
                                .ok_or_else(|| "Missing HTTPRoute conditions".to_owned())?;
                            let accepted = conditions
                                .iter()
                                .any(|c| c["type"] == "Accepted" && c["status"] == "True");
                            let partial = conditions
                                .iter()
                                .any(|c| c["type"] == "PartiallyInvalid" && c["status"] == "True");
                            if !accepted || partial != port.is_none() {
                                return Err(format!("Unexpected attachment status: {conditions:?}"));
                            }
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
