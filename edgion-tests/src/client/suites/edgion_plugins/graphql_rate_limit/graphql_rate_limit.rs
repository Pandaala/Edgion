use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::status::helpers;
use reqwest::Method;
use std::time::Instant;

const HOST: &str = "graphql-rate-limit.example.com";
const SHARED_HOST: &str = "graphql-rate-limit-shared.example.com";
const INDEPENDENT_HOST: &str = "graphql-rate-limit-independent.example.com";
const DYNAMIC_HOST: &str = "graphql-rate-limit-dynamic.example.com";
const SIMPLE: &str = r#"{"query":"{ users(first:10) { name } }"}"#;

struct Reply {
    status: u16,
    remaining: Option<u64>,
    retry_after: Option<String>,
    body: String,
}

#[allow(clippy::too_many_arguments)]
async fn request(
    ctx: &TestContext,
    host: &str,
    method: Method,
    target: &str,
    body: &str,
    key: Option<&str>,
    media: &str,
    encoding: Option<&str>,
) -> Result<Reply, String> {
    let mut builder = ctx
        .http_client
        .request(method, format!("{}{target}", ctx.http_url()))
        .header("host", host)
        .header("content-type", media)
        .body(body.to_owned());
    if let Some(key) = key {
        builder = builder.header("X-Graphql-Key", key);
    }
    if let Some(encoding) = encoding {
        builder = builder.header("content-encoding", encoding);
    }
    let response = builder
        .send()
        .await
        .map_err(|error| format!("Request failed: {error}"))?;
    let status = response.status().as_u16();
    let remaining = response
        .headers()
        .get("x-ratelimit-remaining")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok());
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response
        .text()
        .await
        .map_err(|error| format!("Read response failed: {error}"))?;
    Ok(Reply {
        status,
        remaining,
        retry_after,
        body,
    })
}

async fn post(ctx: &TestContext, host: &str, body: &str, key: &str) -> Result<Reply, String> {
    request(
        ctx,
        host,
        Method::POST,
        "/echo",
        body,
        Some(key),
        "application/json",
        None,
    )
    .await
}

fn require(reply: &Reply, status: u16, remaining: Option<u64>) -> Result<(), String> {
    if reply.status != status || remaining.is_some_and(|expected| reply.remaining != Some(expected)) {
        return Err(format!(
            "Expected status {status}, remaining {remaining:?}; got {}, {:?}, body {:?}",
            reply.status, reply.remaining, reply.body
        ));
    }
    Ok(())
}

fn result(start: Instant, outcome: Result<(), String>) -> TestResult {
    match outcome {
        Ok(()) => TestResult::passed(start.elapsed()),
        Err(error) => TestResult::failed(start.elapsed(), error),
    }
}

async fn accepted(ctx: &TestContext, resource: &str, expected: &str) -> Result<(), String> {
    let status = helpers::fetch_resource_status(ctx, "EdgionPlugins", "edgion-default", resource).await?;
    let actual = status["status"]["conditions"]
        .as_array()
        .and_then(|conditions| conditions.iter().find(|condition| condition["type"] == "Accepted"))
        .and_then(|condition| condition["status"].as_str());
    if actual != Some(expected) {
        return Err(format!("{resource}: expected Accepted={expected}, got {actual:?}"));
    }
    Ok(())
}

pub struct GraphqlRateLimitTestSuite;

impl TestSuite for GraphqlRateLimitTestSuite {
    fn name(&self) -> &str {
        "GraphqlRateLimit Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let mut cases = vec![
            TestCase::new(
                "graphql_cost_before_operation_routing",
                "Routing failure keeps the weighted debit and successful routing preserves bytes",
                |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let outcome = async {
                            let key = uuid::Uuid::new_v4().to_string();
                            let host = "graphql-rate-limit-routing.example.com";
                            let unknown = r#"{"query":"query Unmapped { users(first:10) { name } }"}"#;
                            require(&post(&ctx, host, unknown, &key).await?, 400, Some(12))?;
                            let body = r#"{"query":"query GetUser { users(first:10) { name } }"}"#;
                            let reply = post(&ctx, host, body, &key).await?;
                            require(&reply, 200, Some(0))?;
                            if reply.body.split_once("Echo: ").map(|(_, body)| body) != Some(body) {
                                return Err("Combined routing changed POST bytes".into());
                            }
                            require(&post(&ctx, host, body, &key).await?, 429, Some(0))?;
                            Ok(())
                        }
                        .await;
                        result(start, outcome)
                    })
                },
            ),
            TestCase::new(
                "graphql_weighted_debit_and_replay",
                "Redis charges the actual cost and preserves POST bytes",
                |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let outcome = async {
                            let key = uuid::Uuid::new_v4().to_string();
                            let body = "{\n  \"query\" : \"{ users(first:10) { name } }\"\n}";
                            for remaining in [12, 0] {
                                let reply = post(&ctx, HOST, body, &key).await?;
                                require(&reply, 200, Some(remaining))?;
                                if reply.body.split_once("Echo: ").map(|(_, body)| body) != Some(body) {
                                    return Err(format!("POST body changed: {:?}", reply.body));
                                }
                            }
                            let denied = post(&ctx, HOST, body, &key).await?;
                            require(&denied, 429, Some(0))?;
                            if denied.retry_after.is_none() {
                                return Err("Budget exhaustion did not include Retry-After".to_owned());
                            }
                            Ok(())
                        }
                        .await;
                        result(start, outcome)
                    })
                },
            ),
            TestCase::new(
                "graphql_shared_and_isolated_budgets",
                "Routes share one plugin budget while independent resources are isolated",
                |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let outcome = async {
                            let key = uuid::Uuid::new_v4().to_string();
                            require(&post(&ctx, HOST, SIMPLE, &key).await?, 200, Some(12))?;
                            require(&post(&ctx, SHARED_HOST, SIMPLE, &key).await?, 200, Some(0))?;
                            require(&post(&ctx, HOST, SIMPLE, &key).await?, 429, Some(0))?;
                            require(&post(&ctx, INDEPENDENT_HOST, SIMPLE, &key).await?, 200, Some(12))?;
                            Ok(())
                        }
                        .await;
                        result(start, outcome)
                    })
                },
            ),
            TestCase::new(
                "graphql_impossible_cost_does_not_debit",
                "A cost greater than policy capacity rejects without debiting or retry advice",
                |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let outcome = async {
                            let key = uuid::Uuid::new_v4().to_string();
                            let reply = post(&ctx, HOST, r#"{"query":"{ users(first:100) { name } }"}"#, &key).await?;
                            require(&reply, 429, None)?;
                            if reply.retry_after.is_some() {
                                return Err("An impossible policy cost emitted Retry-After".to_owned());
                            }
                            require(&post(&ctx, HOST, SIMPLE, &key).await?, 200, Some(12))?;
                            Ok(())
                        }
                        .await;
                        result(start, outcome)
                    })
                },
            ),
            TestCase::new(
                "graphql_concurrent_atomic_weighted_admission",
                "Concurrent weighted calls admit exactly two requests into a 24-unit budget",
                |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let outcome = async {
                            let key = uuid::Uuid::new_v4().to_string();
                            let mut calls = Vec::new();
                            // Stay within the parser's four evaluation permits
                            // so this case isolates Redis atomic budget admission.
                            for _ in 0..4 {
                                let ctx = ctx.clone();
                                let key = key.clone();
                                calls.push(tokio::spawn(async move { post(&ctx, HOST, SIMPLE, &key).await }));
                            }
                            let mut allowed = 0;
                            let mut denied = 0;
                            for call in calls {
                                let reply = call.await.map_err(|error| error.to_string())??;
                                match reply.status {
                                    200 => allowed += 1,
                                    429 => denied += 1,
                                    status => return Err(format!("Unexpected concurrent status {status}")),
                                }
                            }
                            if allowed != 2 || denied != 2 {
                                return Err(format!("Expected 2 allowed and 2 denied, got {allowed}/{denied}"));
                            }
                            Ok(())
                        }
                        .await;
                        result(start, outcome)
                    })
                },
            ),
            TestCase::new(
                "graphql_get_and_post_share_cost",
                "GET variables and POST share the same cost-unit budget",
                |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let outcome = async {
                            let key = uuid::Uuid::new_v4().to_string();
                            let query = "query($n:Int!){users(first:$n){name}}";
                            let mut target = reqwest::Url::parse("http://test.invalid/echo").expect("static URL");
                            target
                                .query_pairs_mut()
                                .append_pair("query", query)
                                .append_pair("variables", r#"{"n":10}"#);
                            let encoded = target.query().expect("query was set");
                            let target = format!("/graphql-query?{encoded}");
                            let reply = request(
                                &ctx,
                                HOST,
                                Method::GET,
                                &target,
                                "",
                                Some(&key),
                                "application/json",
                                None,
                            )
                            .await?;
                            require(&reply, 200, Some(12))?;
                            if !reply.body.contains(&format!("Query: {encoded}\n")) {
                                return Err(format!("GET query changed: {:?}", reply.body));
                            }
                            require(&post(&ctx, HOST, SIMPLE, &key).await?, 200, Some(0))?;
                            Ok(())
                        }
                        .await;
                        result(start, outcome)
                    })
                },
            ),
            TestCase::new(
                "graphql_selected_operation_and_defaults",
                "Selected operation and variable defaults determine the charge",
                |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let outcome = async {
                        let key = uuid::Uuid::new_v4().to_string();
                        let body = r#"{"query":"query Small($n:Int=2){users(first:$n){name}} query Big{users(first:20){name}}","operationName":"Small"}"#;
                        require(&post(&ctx, HOST, body, &key).await?, 200, Some(20))?;
                        let default = r#"{"query":"{ users { name } }"}"#;
                        require(&post(&ctx, HOST, default, &key).await?, 200, Some(8))?;
                        Ok(())
                    }.await;
                        result(start, outcome)
                    })
                },
            ),
            TestCase::new(
                "graphql_invalid_request_never_consumes_budget",
                "Invalid query rejects before Redis and leaves a fresh budget intact",
                |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let outcome = async {
                            let key = uuid::Uuid::new_v4().to_string();
                            require(&post(&ctx, HOST, r#"{"query":"{ unknown }"}"#, &key).await?, 400, None)?;
                            require(&post(&ctx, HOST, SIMPLE, &key).await?, 200, Some(12))?;
                            Ok(())
                        }
                        .await;
                        result(start, outcome)
                    })
                },
            ),
            TestCase::new(
                "graphql_get_uses_current_rewritten_query",
                "A rewrite adds the current GET query used by the limiter and forwarded unchanged",
                |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let outcome = async {
                            let key = uuid::Uuid::new_v4().to_string();
                            let reply = request(
                                &ctx,
                                "graphql-rate-limit-rewrite.example.com",
                                Method::GET,
                                "/graphql-query",
                                "",
                                Some(&key),
                                "application/json",
                                None,
                            )
                            .await?;
                            require(&reply, 200, Some(12))?;
                            if !reply
                                .body
                                .contains("Query: query=%7Busers%28first%3A10%29%7Bname%7D%7D\n")
                            {
                                return Err(format!("Expected rewritten query at backend, got {:?}", reply.body));
                            }
                            Ok(())
                        }
                        .await;
                        result(start, outcome)
                    })
                },
            ),
            TestCase::new(
                "graphql_cap_and_lowered_get_post_limits",
                "Hard envelope cap and lowered plugin cap apply to both GET and POST",
                |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let outcome = async {
                            let key = uuid::Uuid::new_v4().to_string();
                            let large = format!(
                                r#"{{"query":"{{users{{name}}}}","variables":{{"padding":"{}"}}}}"#,
                                "x".repeat(65536)
                            );
                            require(&post(&ctx, HOST, &large, &key).await?, 413, None)?;
                            let lowered = format!(
                                r#"{{"query":"{{users{{name}}}}","variables":{{"padding":"{}"}}}}"#,
                                "x".repeat(1024)
                            );
                            require(
                                &post(&ctx, "graphql-rate-limit-small.example.com", &lowered, &key).await?,
                                413,
                                None,
                            )?;
                            let target = format!("/echo?query=%7Busers%7Bname%7D%7D&variables={}", "x".repeat(1024));
                            require(
                                &request(
                                    &ctx,
                                    "graphql-rate-limit-small.example.com",
                                    Method::GET,
                                    &target,
                                    "",
                                    Some(&key),
                                    "application/json",
                                    None,
                                )
                                .await?,
                                413,
                                None,
                            )?;
                            Ok(())
                        }
                        .await;
                        result(start, outcome)
                    })
                },
            ),
        ];

        for (id, method, target, body, media, encoding, expected) in [
            (
                "duplicate_json",
                "POST",
                "/echo",
                r#"{"query":"{users{name}}","query":"{users{name}}"}"#,
                "application/json",
                None,
                400,
            ),
            (
                "duplicate_nested_json",
                "POST",
                "/echo",
                r#"{"query":"{users{name}}","variables":{"n":1,"n":2}}"#,
                "application/json",
                None,
                400,
            ),
            (
                "malformed_json",
                "POST",
                "/echo",
                r#"{"query":"#,
                "application/json",
                None,
                400,
            ),
            (
                "batch",
                "POST",
                "/echo",
                r#"[{"query":"{users{name}}"}]"#,
                "application/json",
                None,
                400,
            ),
            (
                "subscription",
                "POST",
                "/echo",
                r#"{"query":"subscription {users{name}}"}"#,
                "application/json",
                None,
                400,
            ),
            (
                "fragment_cycle",
                "POST",
                "/echo",
                r#"{"query":"{users{...F}} fragment F on User {...F}"}"#,
                "application/json",
                None,
                400,
            ),
            (
                "null_multiplier",
                "POST",
                "/echo",
                r#"{"query":"{users(first:null){name}}"}"#,
                "application/json",
                None,
                400,
            ),
            (
                "max_cost",
                "POST",
                "/echo",
                r#"{"query":"{users(first:10000){name}}"}"#,
                "application/json",
                None,
                400,
            ),
            (
                "ambiguous_operations",
                "POST",
                "/echo",
                r#"{"query":"query A{users{name}} query B{users{name}}"}"#,
                "application/json",
                None,
                400,
            ),
            (
                "post_url_query",
                "POST",
                "/echo?query=%7Busers%7Bname%7D%7D",
                SIMPLE,
                "application/json",
                None,
                400,
            ),
            (
                "extensions",
                "POST",
                "/echo",
                r#"{"query":"{users{name}}","extensions":{"persistedQuery":{"sha256Hash":"abc"}}}"#,
                "application/json",
                None,
                400,
            ),
            ("media", "POST", "/echo", SIMPLE, "text/plain", None, 415),
            (
                "encoding",
                "POST",
                "/echo",
                SIMPLE,
                "application/json",
                Some("gzip"),
                415,
            ),
            (
                "duplicate_get_query",
                "GET",
                "/echo?query=%7Busers%7Bname%7D%7D&%71uery=%7Busers%7Bname%7D%7D",
                "",
                "application/json",
                None,
                400,
            ),
            (
                "get_bad_percent",
                "GET",
                "/echo?query=%ZZ",
                "",
                "application/json",
                None,
                400,
            ),
            (
                "get_unknown_parameter",
                "GET",
                "/echo?query=%7Busers%7Bname%7D%7D&extra=1",
                "",
                "application/json",
                None,
                400,
            ),
            (
                "get_mutation",
                "GET",
                "/echo?query=mutation%7Busers%7Bname%7D%7D",
                "",
                "application/json",
                None,
                405,
            ),
            (
                "get_body",
                "GET",
                "/echo?query=%7Busers%7Bname%7D%7D",
                "unexpected",
                "application/json",
                None,
                400,
            ),
            ("method", "PUT", "/echo", SIMPLE, "application/json", None, 405),
        ] {
            cases.push(TestCase::new(
                format!("graphql_reject_{id}"),
                "Unsupported or ambiguous input never bypasses the selected parser",
                move |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let key = uuid::Uuid::new_v4().to_string();
                        let reply = request(
                            &ctx,
                            HOST,
                            Method::from_bytes(method.as_bytes()).expect("static method"),
                            target,
                            body,
                            Some(&key),
                            media,
                            encoding,
                        )
                        .await;
                        result(start, reply.and_then(|reply| require(&reply, expected, None)))
                    })
                },
            ));
        }

        for (suffix, expected) in [("fail-close", 429), ("fail-open", 200), ("disabled", 200)] {
            cases.push(TestCase::new(
                format!("graphql_{suffix}"),
                "Dependency policy and disabled entries retain their documented behavior",
                move |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let outcome = async {
                            let key = uuid::Uuid::new_v4().to_string();
                            let resource = format!("graphql-rate-limit-{suffix}");
                            let host = format!("{resource}.example.com");
                            require(&post(&ctx, &host, SIMPLE, &key).await?, expected, None)?;
                            let malformed = r#"{"query":"{unknown}"}"#;
                            if suffix == "disabled" {
                                accepted(&ctx, &resource, "True").await?;
                                require(&post(&ctx, &host, malformed, &key).await?, 200, None)?;
                            } else {
                                require(&post(&ctx, &host, malformed, &key).await?, 400, None)?;
                            }
                            Ok(())
                        }
                        .await;
                        result(start, outcome)
                    })
                },
            ));
        }

        for suffix in ["invalid", "wrong-stage", "invalid-body"] {
            cases.push(TestCase::new(
                format!("graphql_{suffix}_fails_closed"),
                "Controller rejection and Gateway rejection agree on invalid configuration",
                move |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let outcome = async {
                            let resource = format!("graphql-rate-limit-{suffix}");
                            accepted(&ctx, &resource, "False").await?;
                            let key = uuid::Uuid::new_v4().to_string();
                            let reply = post(&ctx, &format!("{resource}.example.com"), SIMPLE, &key).await?;
                            require(&reply, 500, None)?;
                            if reply.body != r#"{"message":"plugin configuration error"}"# {
                                return Err(format!("Unexpected fail-closed body {:?}", reply.body));
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
            "graphql_missing_identity_rejects",
            "Missing all configured identity sources rejects",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let reply = request(
                        &ctx,
                        HOST,
                        Method::POST,
                        "/echo",
                        SIMPLE,
                        None,
                        "application/json",
                        None,
                    )
                    .await;
                    result(start, reply.and_then(|reply| require(&reply, 429, None)))
                })
            },
        ));
        cases
    }
}

pub struct GraphqlRateLimitDynamicSuite {
    pub valid: bool,
    pub expected_remaining: u64,
}

impl TestSuite for GraphqlRateLimitDynamicSuite {
    fn name(&self) -> &str {
        "GraphqlRateLimit Dynamic"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let valid = self.valid;
        let expected_remaining = self.expected_remaining;
        vec![TestCase::new(
            "graphql_dynamic_status_and_cost",
            "Schema and cost-rule reload changes actual admission and debit",
            move |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let outcome = async {
                        accepted(&ctx, "graphql-rate-limit-dynamic", if valid { "True" } else { "False" }).await?;
                        let key = uuid::Uuid::new_v4().to_string();
                        let reply = post(&ctx, DYNAMIC_HOST, SIMPLE, &key).await?;
                        require(
                            &reply,
                            if valid { 200 } else { 500 },
                            valid.then_some(expected_remaining),
                        )?;
                        if !valid && reply.body != r#"{"message":"plugin configuration error"}"# {
                            return Err(format!("Unexpected fail-closed body {:?}", reply.body));
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
