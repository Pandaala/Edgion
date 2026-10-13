//! Authenticated query-ID recipe contracts in the shared integration topology.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::edgion_plugins::response_cache::{origin_count, reset_origin, ORIGIN_FIXTURE_HEADER};
use anyhow::{anyhow, ensure, Context, Result};
use reqwest::Method;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(crate) const DYNAMIC_HOST: &str = "dsl-query-lookup-dynamic.example.com";
pub(crate) const DYNAMIC_TAG: &str = "dsl-query-lookup-dynamic";
pub(crate) const DYNAMIC_NAMESPACE: &str = "edgion:dsl:query:v1:example-dynamic:";
pub(crate) const DYNAMIC_QUERY: &str = "query Dynamic { viewer }";
pub(crate) const DYNAMIC_EXECUTOR: &str = "dsl-query-executor-key-dynamic";
pub(crate) const ROTATED_EXECUTOR: &str = "dsl-query-executor-key-rotated";
const STATIC_HOST: &str = "dsl-query-lookup.example.com";
const STATIC_TAG: &str = "dsl-query-lookup-static";
const STATIC_NAMESPACE: &str = "edgion:dsl:query:v1:example-static:";
const STATIC_PUBLISHER: &str = "dsl-query-publisher-key";
const STATIC_EXECUTOR: &str = "dsl-query-executor-key";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Registration<'a> {
    query: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_name: Option<&'a str>,
}
fn registration(query: &str, name: Option<&str>) -> String {
    serde_json::to_string(&Registration {
        query,
        operation_name: name,
    })
    .expect("fixed string serialization")
}
fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub(crate) fn dynamic_id() -> String {
    digest(registration(DYNAMIC_QUERY, None).as_bytes())
}
fn envelope(record: &str, variables: Option<&str>) -> String {
    match variables {
        Some(raw) => format!("{},\"variables\":{raw}}}", &record[..record.len() - 1]),
        None => record.to_owned(),
    }
}
fn invocation(id: &str, variables: Option<&str>) -> String {
    envelope(&serde_json::to_string(&json!({"id":id})).unwrap(), variables)
}

fn observation_operation(operation: &str) -> &'static str {
    match operation.split('/').next() {
        Some("del") => "del",
        Some("get") => "get",
        Some("set") => "set",
        Some("hset") => "hset",
        _ => "unknown_operation",
    }
}

fn observation_failure_category(error: Option<&str>) -> &'static str {
    match error {
        Some(error) if error.contains("registered, not yet connected") => "registered_not_connected",
        Some(error) if error.contains("not found. Available:") => "client_not_registered",
        Some(error)
            if ["DEL failed:", "GET failed:", "SET failed:", "HSET failed:"]
                .iter()
                .any(|prefix| error.starts_with(prefix)) =>
        {
            "dependency_operation_failed"
        }
        _ => "unknown_failure",
    }
}

fn observation_dependency_kind(error: Option<&str>) -> &'static str {
    let Some(error) = error.and_then(|error| error.strip_prefix("DEL failed: Redis DEL: Error { details: ")) else {
        return "unknown_kind";
    };
    let Some((_, kind)) = error.rsplit_once(", kind: ") else {
        return "unknown_kind";
    };
    match kind.strip_suffix(" }") {
        Some("Config") => "config",
        Some("Auth") => "auth",
        Some("Routing") => "routing",
        Some("IO") => "io",
        Some("InvalidCommand") => "invalid_command",
        Some("InvalidArgument") => "invalid_argument",
        Some("Url") => "url",
        Some("Protocol") => "protocol",
        Some("Tls") => "tls",
        Some("Canceled") => "canceled",
        Some("Unknown") => "unknown",
        Some("Timeout") => "timeout",
        Some("Cluster") => "cluster",
        Some("Parse") => "parse",
        Some("Sentinel") => "sentinel",
        Some("NotFound") => "not_found",
        Some("Backpressure") => "backpressure",
        Some("Replica") => "replica",
        _ => "unknown_kind",
    }
}

fn observation_io_reason(error: Option<&str>) -> &'static str {
    if observation_dependency_kind(error) != "io" {
        return "not_io";
    }
    match error {
        Some("DEL failed: Redis DEL: Error { details: \"Connection closed.\", kind: IO }") => "reader_eof",
        Some("DEL failed: Redis DEL: Error { details: \"Connection Closed\", kind: IO }") => "final_write_fallback",
        Some("DEL failed: Redis DEL: Error { details: \"Unresponsive connection.\", kind: IO }") => "unresponsive",
        Some("DEL failed: Redis DEL: Error { details: \"Failed to connect.\", kind: IO }") => {
            "connection_failure_fallback"
        }
        Some("DEL failed: Redis DEL: Error { details: \"Invalid server.\", kind: IO }") => "invalid_server",
        _ => "other_io",
    }
}

fn observation_failure_message(operation: &str, status: u16, error: Option<&str>) -> String {
    format!(
        "Redis observation operation failed: operation={} status={} category={} dependency_kind={} io_reason={}",
        observation_operation(operation),
        status,
        observation_failure_category(error),
        observation_dependency_kind(error),
        observation_io_reason(error)
    )
}

pub(crate) async fn redis_api(ctx: &TestContext, operation: &str, payload: Option<Value>) -> Result<Value> {
    let base =
        std::env::var("EDGION_TEST_GATEWAY_ADMIN_URL").unwrap_or_else(|_| format!("http://{}:12001", ctx.target_host));
    let mut url = reqwest::Url::parse(base.trim_end_matches('/'))?;
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| anyhow!("invalid Redis observation URL"))?;
        path.clear()
            .extend(["api", "v1", "testing", "link-sys", "redis", "default_redis-test"]);
        for part in operation.split('/') {
            path.push(part);
        }
    }
    let request = if let Some(payload) = payload {
        ctx.http_client.post(url).json(&payload)
    } else {
        ctx.http_client.get(url)
    };
    let token = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN")
        .ok()
        .or_else(|| ctx.admin_token.clone());
    let request = match token {
        Some(token) => request.bearer_auth(token),
        None => request,
    };
    let response = request.timeout(Duration::from_secs(3)).send().await?;
    let status = response.status();
    ensure!(
        status.is_success(),
        "Redis observation HTTP failure: operation={} status={}",
        observation_operation(operation),
        status.as_u16()
    );
    let reply: Value = response.json().await?;
    ensure!(
        reply["success"] == true,
        "{}",
        observation_failure_message(operation, status.as_u16(), reply["error"].as_str())
    );
    Ok(reply["data"].clone())
}
pub(crate) async fn clean_keys(ctx: &TestContext, keys: &[String]) -> Result<()> {
    if !keys.is_empty() {
        redis_api(ctx, "del", Some(json!({"keys":keys}))).await?;
    }
    Ok(())
}
async fn get(ctx: &TestContext, key: &str) -> Result<Value> {
    Ok(redis_api(ctx, &format!("get/{key}"), None).await?["value"].clone())
}
pub(crate) async fn set(ctx: &TestContext, key: &str, raw: &str, ttl: u64) -> Result<()> {
    redis_api(ctx, "set", Some(json!({"key":key,"value":raw,"ttl_seconds":ttl}))).await?;
    Ok(())
}

struct Reply {
    status: u16,
    body: Vec<u8>,
    headers: reqwest::header::HeaderMap,
    marker: String,
}
async fn request(
    ctx: &TestContext,
    scope: (&str, &str),
    method: Method,
    target: &str,
    body: &[u8],
    key: Option<&str>,
    extra: Option<(&str, &str)>,
) -> Result<Reply> {
    let (host, tag) = scope;
    tokio::time::timeout(Duration::from_secs(10), async {
        let marker = format!("dsl-query-{}", uuid::Uuid::new_v4());
        let content_type = extra
            .filter(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map_or("application/json", |(_, value)| value);
        let mut request = ctx
            .http_client
            .request(method, format!("{}{target}", ctx.http_url()))
            .header("host", host)
            .header(ORIGIN_FIXTURE_HEADER, tag)
            .header("content-type", content_type)
            .header("access_log", "test_store")
            .header("traceparent", crate::access_log_client::traceparent_for_marker(&marker))
            .body(body.to_vec());
        if let Some(key) = key {
            request = request.header("x-dsl-query-key", key);
        }
        if let Some((name, value)) = extra {
            if !name.eq_ignore_ascii_case("content-type") {
                request = request.header(name, value);
            }
        }
        let mut response = request.send().await?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                body.len().checked_add(chunk.len()).is_some_and(|len| len <= 4096),
                "Query reply exceeds bound"
            );
            body.extend_from_slice(&chunk);
        }
        Ok(Reply {
            status,
            headers,
            body,
            marker,
        })
    })
    .await
    .map_err(|_| anyhow!("Query request timed out"))?
}
async fn post(ctx: &TestContext, host: &str, tag: &str, path: &str, body: &str, key: &str) -> Result<Reply> {
    request(ctx, (host, tag), Method::POST, path, body.as_bytes(), Some(key), None).await
}
async fn terminal(ctx: &TestContext, reply: &Reply, attempts: usize) -> Result<()> {
    let log = ctx
        .access_log_client()
        .get_access_log_with_retry(&reply.marker, 15, 200)
        .await?;
    ensure!(
        log.status_code() == Some(u64::from(reply.status)) && log.upstream_count() == attempts,
        "Query terminal status/origin attempts mismatch"
    );
    Ok(())
}
async fn no_origin(ctx: &TestContext, tag: &str, before: u64, reply: &Reply, status: u16) -> Result<()> {
    ensure!(reply.status == status, "Query expected {status}, got {}", reply.status);
    ensure!(
        origin_count(ctx, tag, 30001).await? == before,
        "Rejected/local query reached origin"
    );
    terminal(ctx, reply, 0).await
}
async fn verify_envelope(reply: &Reply, expected: &str) -> Result<()> {
    ensure!(
        reply.status == 200,
        "Expected successful GraphQL origin, got {}",
        reply.status
    );
    let value: Value = serde_json::from_slice(&reply.body)?;
    ensure!(
        value["bodySha256"] == digest(expected.as_bytes())
            && value["bodyBytes"] == expected.len()
            && value["method"] == "POST"
            && value["path"] == "/graphql"
            && value["queryCredentialAbsent"] == true
            && value["backendPort"] == 30001,
        "Raw reconstructed envelope or credential/path contract differs"
    );
    ensure!(
        reply
            .headers
            .get("content-type")
            .is_some_and(|value| value == "application/json"),
        "Wrong GraphQL content type"
    );
    Ok(())
}
async fn publish(
    ctx: &TestContext,
    scope: (&str, &str),
    namespace: &str,
    raw: &str,
    expected_record: &str,
    key: &str,
    keys: &mut Vec<String>,
) -> Result<String> {
    let (host, tag) = scope;
    let id = digest(expected_record.as_bytes());
    let store_key = format!("{namespace}{id}");
    if !keys.contains(&store_key) {
        keys.push(store_key.clone());
    }
    let before = origin_count(ctx, tag, 30001).await?;
    let reply = post(ctx, host, tag, "/graphql/operations", raw, key).await?;
    no_origin(ctx, tag, before, &reply, 201).await?;
    ensure!(
        serde_json::from_slice::<Value>(&reply.body)? == json!({"id":id}),
        "Publisher ID differs from stable bytes"
    );
    ensure!(
        get(ctx, &store_key).await? == expected_record,
        "Publication did not retain exact registration bytes"
    );
    Ok(id)
}

async fn binary_mapping(ctx: &TestContext, key: &str) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut stream = tokio::net::TcpStream::connect((ctx.target_host.as_str(), 16379)).await?;
        for parts in [
            vec![b"AUTH".as_slice(), b"edgion-test-pwd".as_slice()],
            vec![
                b"SET".as_slice(),
                key.as_bytes(),
                &[0xff],
                b"PX".as_slice(),
                b"60000".as_slice(),
            ],
        ] {
            let mut command = format!("*{}\r\n", parts.len()).into_bytes();
            for part in parts {
                command.extend_from_slice(format!("${}\r\n", part.len()).as_bytes());
                command.extend_from_slice(part);
                command.extend_from_slice(b"\r\n");
            }
            stream.write_all(&command).await?;
            let mut response = Vec::new();
            loop {
                ensure!(response.len() < 128, "Redis injection reply exceeds bound");
                let byte = stream.read_u8().await?;
                response.push(byte);
                if response.ends_with(b"\r\n") {
                    break;
                }
            }
            ensure!(response == b"+OK\r\n", "Redis injection failed");
        }
        Ok(())
    })
    .await
    .map_err(|_| anyhow!("Redis injection timed out"))?
}

async fn static_contract(ctx: &TestContext, keys: &mut Vec<String>) -> Result<()> {
    reset_origin(ctx, STATIC_TAG, 30001).await?;
    let query = "query User { viewer }";
    let record = registration(query, Some("User"));
    let id = digest(record.as_bytes());
    let key = format!("{STATIC_NAMESPACE}{id}");
    keys.push(key.clone());
    clean_keys(ctx, std::slice::from_ref(&key)).await?;
    let rejected_candidates: Vec<String> = [(query, None), ("x", None), ("y", None), ("", None)]
        .into_iter()
        .map(|(query, name)| format!("{STATIC_NAMESPACE}{}", digest(registration(query, name).as_bytes())))
        .collect();
    keys.extend(rejected_candidates.iter().cloned());
    clean_keys(ctx, &rejected_candidates).await?;
    // Both role-denial paths must stop before the first DSL marker and any write.
    for (path, body, credential) in [
        ("/graphql/operations", record.as_str(), None),
        ("/graphql/operations", record.as_str(), Some("bad-key")),
        ("/graphql/operations", record.as_str(), Some(STATIC_EXECUTOR)),
        ("/graphql/by-id", invocation(&id, None).as_str(), Some(STATIC_PUBLISHER)),
    ] {
        let before = origin_count(ctx, STATIC_TAG, 30001).await?;
        let reply = request(
            ctx,
            (STATIC_HOST, STATIC_TAG),
            Method::POST,
            path,
            body.as_bytes(),
            credential,
            Some(("x-tenant", "other")),
        )
        .await?;
        no_origin(ctx, STATIC_TAG, before, &reply, 401).await?;
        ensure!(
            !reply.headers.contains_key("x-dsl-query-stage"),
            "Authentication denial entered DSL"
        );
        let expected = if credential.is_none() {
            "401 Unauthorized - Missing API key"
        } else {
            "401 Unauthorized - Invalid API key"
        };
        ensure!(
            reply.body == expected.as_bytes(),
            "Authentication failure leaked or changed body"
        );
        ensure!(get(ctx, &key).await?.is_null(), "Denied publisher wrote a mapping");
    }
    for (raw, status) in [
        ("not-json".to_owned(), 400),
        ("[]".into(), 400),
        ("null".into(), 400),
        ("42".into(), 400),
        ("{}".into(), 400),
        ("{\"query\":null}".into(), 400),
        ("{\"query\":12}".into(), 400),
        ("{\"query\":\"\"}".into(), 400),
        (
            format!(
                "{{\"query\":{},\"namespace\":\"other\"}}",
                serde_json::to_string(query)?
            ),
            400,
        ),
        (
            format!("{{\"query\":{},\"operationName\":null}}", serde_json::to_string(query)?),
            400,
        ),
        (
            format!("{{\"query\":{},\"operationName\":\"\"}}", serde_json::to_string(query)?),
            400,
        ),
        (
            format!("{{\"query\":{},\"operationName\":1}}", serde_json::to_string(query)?),
            400,
        ),
        ("{\"query\":\"x\",\"qu\\u0065ry\":\"y\"}".into(), 400),
        ("{\"query\":\"x\",\"variables\":{\"a\":1,\"a\":2}}".into(), 400),
        ("é".repeat(33000), 413),
    ] {
        let before = origin_count(ctx, STATIC_TAG, 30001).await?;
        let reply = post(
            ctx,
            STATIC_HOST,
            STATIC_TAG,
            "/graphql/operations",
            &raw,
            STATIC_PUBLISHER,
        )
        .await?;
        no_origin(ctx, STATIC_TAG, before, &reply, status).await?;
        let expected = if status == 413 {
            "Payload too large"
        } else {
            "Invalid input"
        };
        ensure!(
            reply.body == expected.as_bytes(),
            "Publisher rejection changed fixed body"
        );
        ensure!(get(ctx, &key).await?.is_null(), "Invalid publisher wrote a mapping");
        for candidate in &rejected_candidates {
            ensure!(
                get(ctx, candidate).await?.is_null(),
                "Rejected publisher wrote an alternate-selection mapping"
            );
        }
    }
    for (method, path, bytes, credential, extra, status) in [
        (
            Method::GET,
            "/graphql/operations",
            record.as_bytes(),
            STATIC_PUBLISHER,
            None,
            405,
        ),
        (
            Method::GET,
            "/graphql/by-id",
            b"{}".as_slice(),
            STATIC_EXECUTOR,
            None,
            405,
        ),
        (
            Method::GET,
            "/graphql/rewritten-get",
            b"{}".as_slice(),
            STATIC_EXECUTOR,
            None,
            405,
        ),
        (
            Method::POST,
            "/graphql/operations",
            record.as_bytes(),
            STATIC_PUBLISHER,
            Some(("content-type", "text/plain")),
            415,
        ),
        (
            Method::POST,
            "/graphql/operations",
            record.as_bytes(),
            STATIC_PUBLISHER,
            Some(("content-encoding", "gzip")),
            415,
        ),
        (
            Method::POST,
            "/graphql/operations?tenant=other",
            record.as_bytes(),
            STATIC_PUBLISHER,
            None,
            400,
        ),
        (
            Method::POST,
            "/graphql/operations",
            b"\xff".as_slice(),
            STATIC_PUBLISHER,
            None,
            400,
        ),
    ] {
        let before = origin_count(ctx, STATIC_TAG, 30001).await?;
        let reply = request(
            ctx,
            (STATIC_HOST, STATIC_TAG),
            method,
            path,
            bytes,
            Some(credential),
            extra,
        )
        .await?;
        no_origin(ctx, STATIC_TAG, before, &reply, status).await?;
        ensure!(get(ctx, &key).await?.is_null(), "Rejected admission wrote a mapping");
    }
    publish(
        ctx,
        (STATIC_HOST, STATIC_TAG),
        STATIC_NAMESPACE,
        &record,
        &record,
        STATIC_PUBLISHER,
        keys,
    )
    .await?;
    // Alternative whitespace/string spelling has the same stable publication ID.
    let alternate = format!(
        "{{ \"operationName\" : \"User\", \"query\" : {} }}",
        serde_json::to_string(query)?
    );
    publish(
        ctx,
        (STATIC_HOST, STATIC_TAG),
        STATIC_NAMESPACE,
        &alternate,
        &record,
        STATIC_PUBLISHER,
        keys,
    )
    .await?;
    for raw in [
        "{}".to_owned(),
        "{\"id\":null}".into(),
        "{\"id\":42}".into(),
        "{\"id\":\"short\"}".into(),
        format!("{{\"id\":\"{id}\",\"query\":\"query {{ x }}\"}}"),
        format!("{{\"id\":\"{id}\",\"operationName\":\"User\"}}"),
        format!("{{\"id\":\"{id}\",\"namespace\":\"other\"}}"),
        invocation(&id, Some("[]")),
        invocation(&id, Some("1")),
        format!("{{\"id\":\"{id}\",\"variables\":{{\"x\":1,\"x\":2}}}}"),
    ] {
        let before = origin_count(ctx, STATIC_TAG, 30001).await?;
        let reply = post(ctx, STATIC_HOST, STATIC_TAG, "/graphql/by-id", &raw, STATIC_EXECUTOR).await?;
        no_origin(ctx, STATIC_TAG, before, &reply, 400).await?;
        ensure!(reply.body == b"Invalid input", "ID rejection changed fixed body");
    }
    for vars in [
        None,
        Some("null"),
        Some("{}"),
        Some(
            r#"{"n":9007199254740993,"precise":1.234567890123456789,"nested":{"space":"hello world","values":[null,true,3]}}"#,
        ),
    ] {
        let expected = envelope(&record, vars);
        let before = origin_count(ctx, STATIC_TAG, 30001).await?;
        let full = post(ctx, STATIC_HOST, STATIC_TAG, "/graphql", &expected, STATIC_EXECUTOR).await?;
        verify_envelope(&full, &expected).await?;
        terminal(ctx, &full, 1).await?;
        let hit = post(
            ctx,
            STATIC_HOST,
            STATIC_TAG,
            "/graphql/by-id",
            &invocation(&id, vars),
            STATIC_EXECUTOR,
        )
        .await?;
        verify_envelope(&hit, &expected).await?;
        terminal(ctx, &hit, 0).await?;
        ensure!(
            hit.body == full.body && hit.headers.contains_key("age"),
            "ID did not replay full envelope entry"
        );
        let again = post(ctx, STATIC_HOST, STATIC_TAG, "/graphql", &expected, STATIC_EXECUTOR).await?;
        terminal(ctx, &again, 0).await?;
        ensure!(
            origin_count(ctx, STATIC_TAG, 30001).await? == before + 1,
            "Variable/full-ID cache key contract differs"
        );
    }
    // Client tenant claims never select a different store. A mapping present only
    // in the other namespace remains missing under this authenticated route.
    let other_id = digest(registration("query Other { viewer }", None).as_bytes());
    let other_key = format!("{DYNAMIC_NAMESPACE}{other_id}");
    keys.push(other_key.clone());
    set(ctx, &other_key, &registration("query Other { viewer }", None), 60).await?;
    let before = origin_count(ctx, STATIC_TAG, 30001).await?;
    let reply = request(
        ctx,
        (STATIC_HOST, STATIC_TAG),
        Method::POST,
        "/graphql/by-id",
        invocation(&other_id, None).as_bytes(),
        Some(STATIC_EXECUTOR),
        Some(("x-tenant", "example-dynamic")),
    )
    .await?;
    no_origin(ctx, STATIC_TAG, before, &reply, 404).await?;
    // A warm response cannot outlive deletion or expiry of the query mapping.
    clean_keys(ctx, std::slice::from_ref(&key)).await?;
    let before = origin_count(ctx, STATIC_TAG, 30001).await?;
    let reply = post(
        ctx,
        STATIC_HOST,
        STATIC_TAG,
        "/graphql/by-id",
        &invocation(&id, None),
        STATIC_EXECUTOR,
    )
    .await?;
    no_origin(ctx, STATIC_TAG, before, &reply, 404).await?;
    set(ctx, &key, &record, 1).await?;
    let expires = Instant::now() + Duration::from_secs(4);
    loop {
        let reply = post(
            ctx,
            STATIC_HOST,
            STATIC_TAG,
            "/graphql/by-id",
            &invocation(&id, None),
            STATIC_EXECUTOR,
        )
        .await?;
        if reply.status == 404 {
            no_origin(ctx, STATIC_TAG, before, &reply, 404).await?;
            break;
        }
        ensure!(
            reply.status == 200 && Instant::now() < expires,
            "Mapping TTL did not expire without read renewal"
        );
        terminal(ctx, &reply, 0).await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Integrity, strict mapping admission, and binary Redis conversion failures.
    for corrupt in [
        "{}".to_owned(),
        "not-json".into(),
        "{\"query\":\"x\",\"query\":\"y\"}".into(),
        "{\"query\":null}".into(),
        "{\"query\":\"query { viewer }\",\"operationName\":null}".into(),
        "{\"query\":\"query { viewer }\",\"target\":\"other\"}".into(),
        "x".repeat(65537),
    ] {
        let corrupt_id = digest(corrupt.as_bytes());
        let corrupt_key = format!("{STATIC_NAMESPACE}{corrupt_id}");
        keys.push(corrupt_key.clone());
        set(ctx, &corrupt_key, &corrupt, 60).await?;
        let before = origin_count(ctx, STATIC_TAG, 30001).await?;
        let reply = post(
            ctx,
            STATIC_HOST,
            STATIC_TAG,
            "/graphql/by-id",
            &invocation(&corrupt_id, None),
            STATIC_EXECUTOR,
        )
        .await?;
        no_origin(ctx, STATIC_TAG, before, &reply, 502).await?;
        ensure!(
            reply.body == b"Invalid query mapping",
            "Corrupt mapping response leaked detail"
        );
    }
    set(ctx, &key, "{}", 60).await?;
    let before = origin_count(ctx, STATIC_TAG, 30001).await?;
    let reply = post(
        ctx,
        STATIC_HOST,
        STATIC_TAG,
        "/graphql/by-id",
        &invocation(&id, None),
        STATIC_EXECUTOR,
    )
    .await?;
    no_origin(ctx, STATIC_TAG, before, &reply, 502).await?;
    binary_mapping(ctx, &key).await?;
    let reply = post(
        ctx,
        STATIC_HOST,
        STATIC_TAG,
        "/graphql/by-id",
        &invocation(&id, None),
        STATIC_EXECUTOR,
    )
    .await?;
    no_origin(ctx, STATIC_TAG, before, &reply, 502).await?;
    clean_keys(ctx, std::slice::from_ref(&key)).await?;
    redis_api(ctx, "hset", Some(json!({"key":key,"field":"wrong","value":"type"}))).await?;
    let reply = post(
        ctx,
        STATIC_HOST,
        STATIC_TAG,
        "/graphql/by-id",
        &invocation(&id, None),
        STATIC_EXECUTOR,
    )
    .await?;
    no_origin(ctx, STATIC_TAG, before, &reply, 503).await?;
    ensure!(
        reply.body == b"Query store unavailable",
        "Redis dependency detail escaped fixed reply"
    );
    // Each admitted source can fit while the combined envelope does not.
    let large_record = registration(&format!("query Huge {{ viewer }}{}", " ".repeat(62000)), None);
    let large_id = digest(large_record.as_bytes());
    let large_key = format!("{STATIC_NAMESPACE}{large_id}");
    keys.push(large_key.clone());
    set(ctx, &large_key, &large_record, 60).await?;
    let variables = serde_json::to_string(&json!({"padding":"a".repeat(4000)}))?;
    let reply = post(
        ctx,
        STATIC_HOST,
        STATIC_TAG,
        "/graphql/by-id",
        &invocation(&large_id, Some(&variables)),
        STATIC_EXECUTOR,
    )
    .await?;
    no_origin(ctx, STATIC_TAG, before, &reply, 500).await?;
    for (query, name, valid) in [
        ("{ viewer }", None, true),
        ("query Named { viewer }", None, true),
        ("query A { viewer } query B { other }", None, false),
        ("query A { viewer } query B { other }", Some("A"), true),
        ("query A { viewer } query B { other }", Some("B"), true),
        ("query A { viewer }", Some("Missing"), false),
        ("not graphql", None, false),
        (r#"query Escaped { viewer(id: "a\"b") }"#, Some("Escaped"), true),
    ] {
        let registration = registration(query, name);
        let id = publish(
            ctx,
            (STATIC_HOST, STATIC_TAG),
            STATIC_NAMESPACE,
            &registration,
            &registration,
            STATIC_PUBLISHER,
            keys,
        )
        .await?;
        let before = origin_count(ctx, STATIC_TAG, 30001).await?;
        let reply = post(
            ctx,
            STATIC_HOST,
            STATIC_TAG,
            "/graphql/by-id",
            &invocation(&id, None),
            STATIC_EXECUTOR,
        )
        .await?;
        if valid {
            verify_envelope(&reply, &registration).await?;
            terminal(ctx, &reply, 1).await?;
        } else {
            no_origin(ctx, STATIC_TAG, before, &reply, 400).await?;
        }
    }
    Ok(())
}

pub(crate) fn static_case() -> TestCase {
    TestCase::new(
        "dsl_query_lookup_contract",
        "Sequential authenticated publication, raw reconstruction, revocation and cache parity",
        |ctx| {
            Box::pin(async move {
                let start = Instant::now();
                let mut keys = Vec::new();
                let result = static_contract(&ctx, &mut keys).await;
                let cleanup = clean_keys(&ctx, &keys).await;
                match (result, cleanup) {
                    (Ok(()), Ok(())) => TestResult::passed(start.elapsed()),
                    (Err(error), Ok(())) | (Ok(()), Err(error)) => {
                        TestResult::failed(start.elapsed(), format!("{error:#}"))
                    }
                    (Err(error), Err(cleanup)) => TestResult::failed(
                        start.elapsed(),
                        format!("{error:#}; mapping cleanup failed: {cleanup:#}"),
                    ),
                }
            })
        },
    )
    .with_timeout(Duration::from_secs(120))
}

pub(crate) async fn publish_dynamic(ctx: &TestContext) -> Result<()> {
    let record = registration(DYNAMIC_QUERY, None);
    let mut keys = Vec::new();
    publish(
        ctx,
        (DYNAMIC_HOST, DYNAMIC_TAG),
        DYNAMIC_NAMESPACE,
        &record,
        &record,
        "dsl-query-publisher-key-dynamic",
        &mut keys,
    )
    .await?;
    Ok(())
}

pub(crate) struct QueryLookupPhase(pub &'static str);
fn executed_plugin<'a>(entry: &'a Value, plugin: &str) -> Option<&'a Value> {
    entry["stage_logs"]
        .as_array()?
        .iter()
        .filter(|stage| stage["stage"] == "request_filters")
        .filter_map(|stage| stage["edgion_plugins"].as_array())
        .flat_map(|resources| resources.iter())
        .filter(|resource| resource["name"] == "dsl-query-executor-dynamic")
        .filter_map(|resource| resource["logs"].as_array())
        .flat_map(|logs| logs.iter())
        .find(|log| log["name"] == plugin && log.get("cond_skip").is_none())
}
fn executed_config_gate(entry: &Value) -> Option<&Value> {
    entry["stage_logs"]
        .as_array()?
        .iter()
        .filter(|stage| stage["stage"] == "request_filters")
        .filter_map(|stage| stage["filters"].as_array())
        .flat_map(|filters| filters.iter())
        .find(|filter| filter["name"] == "RejectInvalidConfig" && filter.get("cond_skip").is_none())
}
async fn current_gateway_profile(ctx: &TestContext) -> Result<Value> {
    let base =
        std::env::var("EDGION_TEST_GATEWAY_ADMIN_URL").unwrap_or_else(|_| format!("http://{}:12001", ctx.target_host));
    let request = ctx
        .http_client
        .get(format!("{}/configclient/EdgionPlugins", base.trim_end_matches('/')))
        .query(&[("namespace", "edgion-default"), ("name", "dsl-query-executor-dynamic")])
        .timeout(Duration::from_secs(3));
    let token = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN")
        .ok()
        .or_else(|| ctx.admin_token.clone());
    let request = match token {
        Some(token) => request.bearer_auth(token),
        None => request,
    };
    let mut response = request.send().await?.error_for_status()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len().checked_add(chunk.len()).is_some_and(|size| size <= 65536),
            "Dynamic profile exceeds bound"
        );
        bytes.extend_from_slice(&chunk);
    }
    let mut body: Value = serde_json::from_slice(&bytes)?;
    ensure!(body["success"] == true, "Gateway profile query failed");
    Ok(body["data"].take())
}
async fn current_missing_profile(ctx: &TestContext) -> Result<()> {
    let resource = current_gateway_profile(ctx).await?;
    let config = &resource["spec"]["requestPlugins"][2]["config"];
    ensure!(
        config["linkSysRefs"][0]["name"] == "query-store-not-ready"
            && config["linkSysRefs"][0]["namespace"] == "default"
            && config["source"]
                .as_str()
                .is_some_and(|source| source.contains("redis.get_bounded(\"query-store-not-ready\"")),
        "Current Gateway missing-provider profile has not converged"
    );
    Ok(())
}
impl TestSuite for QueryLookupPhase {
    fn name(&self) -> &str {
        "DSL query-ID dynamic phase"
    }
    fn port_key(&self) -> Option<&str> {
        Some("EdgionPlugins")
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let phase = self.0;
        vec![TestCase::new(
            format!("dsl_query_lookup_{phase}"),
            "Current publication and fail-close/recovery contract",
            move |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result: Result<()> = async {
                        let before = origin_count(&ctx, DYNAMIC_TAG, 30001).await?;
                        if phase == "published" {
                            return publish_dynamic(&ctx).await;
                        }
                        if phase == "absent" {
                            for name in ["dsl-query-publisher-dynamic", "dsl-query-executor-dynamic"] {
                                for kind in ["HTTPRoute", "EdgionPlugins", "Secret"] {
                                    let status = ctx.http_client.get(format!(
                                        "{}/api/v1/namespaced/{kind}/edgion-default/{name}",
                                        ctx.admin_api_url()
                                    ));
                                    let status = match &ctx.admin_token {
                                        Some(token) => status.bearer_auth(token),
                                        None => status,
                                    };
                                    ensure!(
                                        status.send().await?.status() == reqwest::StatusCode::NOT_FOUND,
                                        "Owned dynamic resource still exists"
                                    );
                                }
                            }
                            return Ok(());
                        }
                        if phase == "missing-provider" {
                            current_missing_profile(&ctx).await?;
                        }
                        let key = if phase == "initial" {
                            DYNAMIC_EXECUTOR
                        } else {
                            ROTATED_EXECUTOR
                        };
                        let reply = post(
                            &ctx,
                            DYNAMIC_HOST,
                            DYNAMIC_TAG,
                            "/graphql/by-id",
                            &invocation(&dynamic_id(), None),
                            key,
                        )
                        .await?;
                        let expected = match phase {
                            "fresh-invalid" | "unconfigured" => 500,
                            "missing-provider" => 503,
                            "corrupt" => 502,
                            "deleted" | "expired" => 404,
                            _ => 200,
                        };
                        ensure!(
                            reply.status == expected,
                            "Dynamic {phase} expected {expected}, got {}",
                            reply.status
                        );
                        if expected != 200 {
                            no_origin(&ctx, DYNAMIC_TAG, before, &reply, expected).await?;
                            if phase == "fresh-invalid" {
                                ensure!(
                                    reply.body == br#"{"message":"plugin configuration error"}"#,
                                    "Fresh invalid owner did not execute the canonical configuration gate"
                                );
                                let log = ctx
                                    .access_log_client()
                                    .get_access_log_with_retry(&reply.marker, 15, 200)
                                    .await?;
                                let gate = executed_config_gate(&log.data)
                                    .context("Fresh invalid DSL did not execute its owned configuration gate")?;
                                let tokens = gate["log"]
                                    .as_array()
                                    .context("Configuration gate lost private decision tokens")?;
                                ensure!(
                                    tokens.contains(&json!("err: invalid plugin config"))
                                        && tokens.contains(&json!("err: Dsl"))
                                        && log.data["match_info"]["rsc"]["name"] == "dsl-query-executor-dynamic"
                                        && log.data["match_info"]["rsc"]["ns"] == "edgion-default"
                                        && log.data["match_info"]["rsc"]["kind"] == "HTTPRoute"
                                        && log.data["request_info"]["host"] == DYNAMIC_HOST
                                        && log.data["request_info"]["path"] == "/graphql/by-id",
                                    "Fresh invalid DSL gate lacks canonical execution evidence"
                                );
                            }
                            if phase == "missing-provider" {
                                ensure!(
                                    reply.body == b"Query store unavailable",
                                    "Dependency failure changed fixed body"
                                );
                                let log = ctx
                                    .access_log_client()
                                    .get_access_log_with_retry(&reply.marker, 15, 200)
                                    .await?;
                                ensure!(
                                    log.data.to_string().contains("redis provider not ready"),
                                    "Missing-provider callback did not record its canonical private failure"
                                );
                            }
                        } else {
                            verify_envelope(&reply, &registration(DYNAMIC_QUERY, None)).await?;
                            // Both miss and hit are legal during publication convergence;
                            // static acceptance separately proves an exact first-fill count.
                            let log = ctx
                                .access_log_client()
                                .get_access_log_with_retry(&reply.marker, 15, 200)
                                .await?;
                            ensure!(
                                log.status_code() == Some(200) && log.upstream_count() <= 1,
                                "Dynamic terminal outcome differs"
                            );
                            if phase == "invalid-retained" {
                                ensure!(
                                    reply
                                        .headers
                                        .get("x-dsl-query-stage")
                                        .is_some_and(|value| value == "executor")
                                        && executed_plugin(&log.data, "dsl-query-executor-dynamic").is_some()
                                        && executed_plugin(&log.data, "RejectInvalidConfig").is_none(),
                                    "Invalid update did not retain a valid DSL executable"
                                );
                            }
                            if phase != "initial" {
                                let now = origin_count(&ctx, DYNAMIC_TAG, 30001).await?;
                                let old = post(
                                    &ctx,
                                    DYNAMIC_HOST,
                                    DYNAMIC_TAG,
                                    "/graphql/by-id",
                                    &invocation(&dynamic_id(), None),
                                    DYNAMIC_EXECUTOR,
                                )
                                .await?;
                                no_origin(&ctx, DYNAMIC_TAG, now, &old, 401).await?;
                                ensure!(
                                    !old.headers.contains_key("x-dsl-query-stage"),
                                    "Rotated credential entered DSL"
                                );
                            }
                        }
                        if matches!(
                            phase,
                            "invalid-retained"
                                | "fresh-invalid"
                                | "recovered"
                                | "missing-provider"
                                | "initial"
                                | "rotated"
                        ) {
                            let status = crate::suites::status::helpers::fetch_resource_status(
                                &ctx,
                                "EdgionPlugins",
                                "edgion-default",
                                "dsl-query-executor-dynamic",
                            )
                            .await
                            .map_err(|error| anyhow!(error))?;
                            let conditions =
                                crate::suites::status::helpers::controller_local_conditions(&status["status"])
                                    .context("Missing dynamic plugin status")?;
                            let accepted = conditions
                                .iter()
                                .find(|condition| condition["type"] == "Accepted")
                                .context("Missing Accepted condition")?;
                            ensure!(
                                accepted["status"]
                                    == if matches!(phase, "invalid-retained" | "fresh-invalid") {
                                        "False"
                                    } else {
                                        "True"
                                    },
                                "Dynamic acceptance status differs"
                            );
                            if let Some(generation) = status["metadata"]["generation"].as_i64() {
                                ensure!(
                                    accepted["observedGeneration"].as_i64() == Some(generation),
                                    "Accepted belongs to a different resource generation"
                                );
                            }
                            if matches!(phase, "invalid-retained" | "fresh-invalid") {
                                let gateway = current_gateway_profile(&ctx).await?;
                                ensure!(
                                    status["metadata"]["name"] == "dsl-query-executor-dynamic"
                                        && status["metadata"]["namespace"] == "edgion-default"
                                        && status["metadata"]["resourceVersion"].as_str().is_some()
                                        && gateway["metadata"]["resourceVersion"]
                                            == status["metadata"]["resourceVersion"]
                                        && gateway["metadata"]["generation"] == status["metadata"]["generation"]
                                        && gateway["metadata"]["name"] == status["metadata"]["name"]
                                        && gateway["metadata"]["namespace"] == status["metadata"]["namespace"]
                                        && gateway["spec"]["requestPlugins"][2]["config"]["source"]
                                            == "unknown_query_builtin()",
                                    "Invalid generation has not reached the current Gateway owner"
                                );
                            }
                        }
                        Ok(())
                    }
                    .await;
                    match result {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), format!("{error:#}")),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(20))]
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn redis_observation_diagnostics_use_only_fixed_operation_and_failure_categories() {
        for (operation, label) in [
            ("del", "del"),
            ("get/private-key", "get"),
            ("set", "set"),
            ("hset", "hset"),
            ("private-operation", "unknown_operation"),
        ] {
            assert_eq!(super::observation_operation(operation), label);
        }
        for (error, category) in [
            (
                Some("Redis client 'private-client' not found (registered, not yet connected)"),
                "registered_not_connected",
            ),
            (
                Some("Redis client 'private-client' not found. Available: [private-topology]"),
                "client_not_registered",
            ),
            (Some("DEL failed: private-credential"), "dependency_operation_failed"),
            (Some("GET failed: private-credential"), "dependency_operation_failed"),
            (Some("SET failed: private-credential"), "dependency_operation_failed"),
            (Some("HSET failed: private-credential"), "dependency_operation_failed"),
            (Some("private-unknown-failure"), "unknown_failure"),
            (None, "unknown_failure"),
        ] {
            assert_eq!(super::observation_failure_category(error), category);
            let message = super::observation_failure_message("get/private-key", 200, error);
            assert_eq!(
                message,
                format!("Redis observation operation failed: operation=get status=200 category={category} dependency_kind=unknown_kind io_reason=not_io")
            );
            assert!(!message.contains("private"));
        }
    }

    #[test]
    fn redis_dependency_kind_uses_only_the_final_pinned_fred_enum_variant() {
        for (kind, label) in [
            ("Config", "config"),
            ("Auth", "auth"),
            ("Routing", "routing"),
            ("IO", "io"),
            ("InvalidCommand", "invalid_command"),
            ("InvalidArgument", "invalid_argument"),
            ("Url", "url"),
            ("Protocol", "protocol"),
            ("Tls", "tls"),
            ("Canceled", "canceled"),
            ("Unknown", "unknown"),
            ("Timeout", "timeout"),
            ("Cluster", "cluster"),
            ("Parse", "parse"),
            ("Sentinel", "sentinel"),
            ("NotFound", "not_found"),
            ("Backpressure", "backpressure"),
            ("Replica", "replica"),
        ] {
            let error = format!(
                "DEL failed: Redis DEL: Error {{ details: {:?}, kind: {kind} }}",
                "private detail"
            );
            assert_eq!(super::observation_dependency_kind(Some(&error)), label);
        }
        for (kind, forged, label) in [("IO", "Timeout", "io"), ("Timeout", "IO", "timeout")] {
            let detail = format!("private-token redis://private-user@private-host/private-key \"\n, kind: {forged} }}");
            let error = format!("DEL failed: Redis DEL: Error {{ details: {detail:?}, kind: {kind} }}");
            assert_eq!(super::observation_dependency_kind(Some(&error)), label);
            let message = super::observation_failure_message("del", 200, Some(&error));
            assert_eq!(message, format!("Redis observation operation failed: operation=del status=200 category=dependency_operation_failed dependency_kind={label} io_reason={}", if kind == "IO" { "other_io" } else { "not_io" }));
            assert!(!message.contains("private"));
            assert!(!message.contains("redis://"));
            assert!(!message.contains('\n'));
        }
        for error in [
            None,
            Some(""),
            Some("Error { details: \"private\", kind: IO }"),
            Some("GET failed: Redis DEL: Error { details: \"private\", kind: IO }"),
            Some("DEL failed: Redis DEL: Error { details: \"private\" }"),
            Some("DEL failed: Redis DEL: Error { details: \"private\", kind: IO } trailing"),
            Some("DEL failed: Redis DEL: Error { details: \"private\", kind: FutureKind }"),
            Some("DEL failed: Redis DEL: Error { details: \"private\", kind: Io }"),
            Some("DEL failed: Redis DEL: Error { details: private, kind: IO"),
        ] {
            assert_eq!(super::observation_dependency_kind(error), "unknown_kind");
        }
    }

    #[test]
    fn redis_io_reason_requires_a_complete_fixed_envelope_and_never_discloses_detail() {
        for (detail, reason) in [
            ("Connection closed.", "reader_eof"),
            ("Connection Closed", "final_write_fallback"),
            ("Unresponsive connection.", "unresponsive"),
            ("Failed to connect.", "connection_failure_fallback"),
            ("Invalid server.", "invalid_server"),
        ] {
            let error = format!("DEL failed: Redis DEL: Error {{ details: {detail:?}, kind: IO }}");
            assert_eq!(super::observation_io_reason(Some(&error)), reason);
            assert_eq!(super::observation_failure_message("del", 200, Some(&error)),
                format!("Redis observation operation failed: operation=del status=200 category=dependency_operation_failed dependency_kind=io io_reason={reason}"));
            let timeout = format!("DEL failed: Redis DEL: Error {{ details: {detail:?}, kind: Timeout }}");
            assert_eq!(super::observation_io_reason(Some(&timeout)), "not_io");
            let forged_detail = format!("private-token redis://private-user@private-host/private-key \"\n{detail}");
            let forged = format!("DEL failed: Redis DEL: Error {{ details: {forged_detail:?}, kind: IO }}");
            assert_eq!(super::observation_io_reason(Some(&forged)), "other_io");
            let message = super::observation_failure_message("del", 200, Some(&forged));
            assert!(!message.contains("private"));
            assert!(!message.contains("redis://"));
            assert!(!message.contains('\n'));
        }
        for error in [
            "DEL failed: Redis DEL: Error { details: \"Os { code: 54, kind: ConnectionReset }\", kind: IO }",
            "DEL failed: Redis DEL: Error { details: \"Connection closed\", kind: IO }",
            "DEL failed: Redis DEL: Error { details: \"connection closed.\", kind: IO }",
            "DEL failed: Redis DEL: Error { details: \"prefix Connection closed. suffix\", kind: IO }",
        ] {
            assert_eq!(super::observation_io_reason(Some(error)), "other_io");
        }
        for error in [
            None,
            Some(""),
            Some("GET failed: Redis DEL: Error { details: \"Connection closed.\", kind: IO }"),
            Some("DEL failed: Redis DEL: Error { details: \"Connection closed.\", kind: IO } trailing"),
            Some("DEL failed: Redis DEL: Error { details: \"Connection closed.\", kind: IO }\n"),
            Some("DEL failed: Redis DEL: Error { details: \"Connection closed.\", kind: FutureKind }"),
        ] {
            assert_eq!(super::observation_io_reason(error), "not_io");
        }
    }

    #[test]
    fn execution_evidence_requires_the_inner_plugin_of_the_owned_request_resource() {
        let outer = serde_json::json!({"stage_logs":[{"stage":"request_filters","edgion_plugins":[{"name":"dsl-query-executor-dynamic","logs":[]}]}]});
        assert!(super::executed_plugin(&outer, "dsl-query-executor-dynamic").is_none());
        let executed = serde_json::json!({"stage_logs":[{"stage":"request_filters","edgion_plugins":[{"name":"dsl-query-executor-dynamic","logs":[{"name":"RejectInvalidConfig","log":["invalid plugin config","Dsl"]}]}]}]});
        assert!(super::executed_plugin(&executed, "RejectInvalidConfig").is_some());
        assert!(super::executed_plugin(&executed, "dsl-query-executor-dynamic").is_none());
        let preflight = serde_json::json!({"stage_logs":[{"stage":"request_filters","filters":[{"name":"RejectInvalidConfig","log":["err: invalid plugin config","err: Dsl"]}]}]});
        assert!(super::executed_config_gate(&preflight).is_some());
        assert!(super::executed_plugin(&preflight, "RejectInvalidConfig").is_none());
        assert!(super::executed_config_gate(&executed).is_none());
    }
    #[test]
    fn documented_indexed_boolean_comparisons_compile_after_field_selection() {
        use edgion_gateway::plugins::http::dsl::lang::bytecode::OpCode;
        use edgion_resources::plugin_configs::DslConfig;
        for (expression, index_count) in [("r[\"ok\"] == false", 1), ("r[\"value\"][\"allowed\"] == false", 2)] {
            let config=DslConfig{ name:"indexed-boolean-example".into(), source:Some(format!("let r = {{\"ok\":false,\"value\":{{\"allowed\":false}}}}\nif {expression} {{ return deny(503, \"service unavailable\") }}\nreturn next()")),..Default::default()};
            let code = config.compile_and_validate().unwrap().code;
            let selected: Vec<_> = code
                .iter()
                .enumerate()
                .filter(|(_, op)| **op == OpCode::IndexGet)
                .map(|(pc, _)| pc)
                .collect();
            assert_eq!(selected.len(), index_count);
            let comparison = code.iter().position(|op| *op == OpCode::Equal).unwrap();
            assert!(selected.iter().all(|pc| *pc < comparison));
            assert!(!code.contains(&OpCode::Not));
            assert!(code[comparison + 1..]
                .iter()
                .any(|op| matches!(op, OpCode::JumpIfFalse(_))));
        }
        // Real static oversize/corruption and dynamic missing-provider tests
        // execute this same map-field comparison through the Gateway VM.
        for path in [
            "docs/en/user-guide/http-route/filters/edgion-plugins/dsl/api-reference.md",
            "skills/03-coding/dsl/redis-builtin.md",
        ] {
            let text = std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .unwrap()
                    .join(path),
            )
            .unwrap();
            assert!(!text.lines().any(|line| line.trim_start().starts_with("if !r[")));
            assert!(text.contains("if r[\"ok\"] == false"));
            assert!(text.contains("if r[\"value\"][\"allowed\"] == false"));
        }
    }
    #[test]
    fn query_lookup_fixtures_compile_real_dsl_and_reject_invalid_generation() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("integration/conf/EdgionPlugins/Dsl");
        for path in [
            "42_EdgionPlugins_dsl-query-publisher.yaml",
            "43_EdgionPlugins_dsl-query-executor.yaml",
            "QueryLookup/DynamicTest/initial/10_EdgionPlugins_publisher.yaml",
            "QueryLookup/DynamicTest/initial/11_EdgionPlugins_executor.yaml",
            "QueryLookup/DynamicTest/updates/missing-provider/11_EdgionPlugins_executor.yaml",
            "QueryLookup/DynamicTest/updates/recovered/11_EdgionPlugins_executor.yaml",
        ] {
            let plugins: edgion_resources::EdgionPlugins =
                serde_yaml::from_str(&std::fs::read_to_string(root.join(path)).unwrap()).unwrap();
            assert!(
                plugins.validate_plugins().is_empty(),
                "{path}: {:?}",
                plugins.validate_plugins()
            );
            for entry in plugins.spec.request_plugins.as_ref().unwrap() {
                if let edgion_resources::EdgionPlugin::Dsl(config) = &entry.plugin {
                    config
                        .compile_and_validate()
                        .unwrap_or_else(|error| panic!("{path}: {error}"));
                }
            }
        }
        let bad: edgion_resources::EdgionPlugins = serde_yaml::from_str(
            &std::fs::read_to_string(
                root.join("QueryLookup/DynamicTest/updates/invalid/11_EdgionPlugins_executor.yaml"),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(!bad.validate_plugins().is_empty());
    }
    #[test]
    fn public_recipe_yaml_compiles_and_preserves_order_and_role_boundaries() {
        for language in ["en", "zh-CN"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join(format!(
                    "docs/{language}/user-guide/http-route/filters/edgion-plugins/dsl/query-id-lookup.md"
                ));
            let text = std::fs::read_to_string(path).unwrap();
            let yaml = text.split_once("```yaml\n").unwrap().1.split_once("\n```").unwrap().0;
            let mut plugin_count = 0;
            for document in serde_yaml::Deserializer::from_str(yaml) {
                let value = <serde_yaml::Value as serde::Deserialize>::deserialize(document).unwrap();
                if value["kind"].as_str() != Some("EdgionPlugins") {
                    continue;
                }
                let plugin: edgion_resources::EdgionPlugins = serde_yaml::from_value(value).unwrap();
                assert!(
                    plugin.validate_plugins().is_empty(),
                    "{language}: {:?}",
                    plugin.validate_plugins()
                );
                let entries = plugin.spec.request_plugins.as_ref().unwrap();
                assert!(
                    matches!(&entries[0].plugin,edgion_resources::EdgionPlugin::KeyAuth(config) if config.hide_credentials && config.upstream_header_fields.is_empty() && config.anonymous.is_none())
                );
                for entry in entries {
                    if let edgion_resources::EdgionPlugin::Dsl(config) = &entry.plugin {
                        config.compile_and_validate().unwrap();
                        let source = config.source.as_ref().unwrap();
                        assert!(source.contains("req.body.byte_len() > 65536"));
                        assert!(!source.contains("x-response-cache-fixture"));
                        if source.contains("redis.set(") {
                            assert!(source.contains("604800000"));
                        }
                    }
                }
                if entries.len() > 2 {
                    assert!(matches!(
                        entries[2].plugin,
                        edgion_resources::EdgionPlugin::GraphqlGuard(_)
                    ));
                    assert!(matches!(
                        entries[3].plugin,
                        edgion_resources::EdgionPlugin::ResponseCache(_)
                    ));
                }
                plugin_count += 1;
            }
            assert_eq!(plugin_count, 2);
        }
        let raw = super::registration("query Named { viewer }", Some("Named"));
        assert_eq!(raw, r#"{"query":"query Named { viewer }","operationName":"Named"}"#);
        assert_ne!(
            super::digest(raw.as_bytes()),
            super::digest(super::registration("query Named { viewer }", None).as_bytes())
        );
        let numbers = r#"{"value":9007199254740993,"decimal":1.234567890123456789}"#;
        assert!(super::envelope(&raw, Some(numbers)).contains(numbers));
    }
}
