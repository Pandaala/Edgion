//! ResponseCache contracts and bounded shared test-origin observations.

use crate::framework::{test_backend_hosts, TestContext};
use crate::framework::{TestCase, TestResult, TestSuite};
use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::time::Duration;
use std::time::Instant;

pub(crate) const ORIGIN_FIXTURE_HEADER: &str = "x-response-cache-fixture";
const FIXTURE_IDS: [&str; 4] = [
    "response-cache-static",
    "response-cache-dynamic",
    "dsl-query-lookup-static",
    "dsl-query-lookup-dynamic",
];

fn validate_origin_target(id: &str, port: u16) -> Result<()> {
    if !FIXTURE_IDS.contains(&id) || !matches!(port, 30001 | 30002) {
        bail!("invalid origin observation target");
    }
    Ok(())
}

fn observation_url(host: &str, id: &str, port: u16, reset: bool) -> Result<reqwest::Url> {
    validate_origin_target(id, port)?;
    if host.is_empty() {
        bail!("origin observation host is absent");
    }
    let normalized;
    let host = if let Ok(std::net::IpAddr::V6(address)) = host.parse() {
        normalized = format!("[{address}]");
        normalized.as_str()
    } else {
        host
    };
    let mut url = reqwest::Url::parse("http://localhost/response-cache/observations/")?;
    url.set_host(Some(host))
        .map_err(|_| anyhow!("origin observation host is invalid"))?;
    url.set_port(Some(port))
        .map_err(|_| anyhow!("origin observation port is invalid"))?;
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| anyhow!("origin observation URL is invalid"))?;
        path.pop_if_empty().push(id);
        if reset {
            path.push("reset");
        }
    }
    Ok(url)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CountReply {
    count: u64,
}

fn decode_count(bytes: &[u8]) -> Result<u64> {
    if bytes.len() > 256 {
        bail!("origin observation response exceeds its bound");
    }
    let reply: CountReply =
        serde_json::from_slice(bytes).map_err(|_| anyhow!("origin observation reply is invalid"))?;
    Ok(reply.count)
}

async fn observe_one(ctx: &TestContext, host: &str, id: &str, port: u16, reset: bool) -> Result<u64> {
    tokio::time::timeout(Duration::from_secs(3), async {
        let url = observation_url(host, id, port, reset)?;
        let request = if reset {
            ctx.http_client.post(url)
        } else {
            ctx.http_client.get(url)
        };
        let mut response = request.send().await.context("origin observation request failed")?;
        if response.status() != reqwest::StatusCode::OK {
            bail!("origin observation returned an unexpected status");
        }
        let mut types = response.headers().get_all("content-type").iter();
        if types.next().and_then(|value| value.to_str().ok()) != Some("application/json") || types.next().is_some() {
            bail!("origin observation content type is invalid");
        }
        let mut body = Vec::with_capacity(256);
        while let Some(chunk) = response
            .chunk()
            .await
            .context("origin observation response read failed")?
        {
            if body.len().checked_add(chunk.len()).is_none_or(|size| size > 256) {
                bail!("origin observation response exceeds its bound");
            }
            body.extend_from_slice(&chunk);
        }
        decode_count(&body)
    })
    .await
    .map_err(|_| anyhow!("origin observation request timed out"))?
}

fn origin_hosts(ctx: &TestContext) -> Result<Vec<String>> {
    let mut hosts = test_backend_hosts(ctx);
    hosts.sort_unstable();
    hosts.dedup();
    if hosts.is_empty() || hosts.iter().any(String::is_empty) {
        bail!("origin observation hosts are absent");
    }
    Ok(hosts)
}

fn add_count(total: u64, count: u64) -> Result<u64> {
    total
        .checked_add(count)
        .ok_or_else(|| anyhow!("origin observation total overflow"))
}

pub(crate) async fn origin_count(ctx: &TestContext, fixture_id: &str, backend_port: u16) -> Result<u64> {
    validate_origin_target(fixture_id, backend_port)?;
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut total = 0;
        for host in origin_hosts(ctx)? {
            total = add_count(total, observe_one(ctx, &host, fixture_id, backend_port, false).await?)?;
        }
        Ok(total)
    })
    .await
    .map_err(|_| anyhow!("origin observation helper timed out"))?
}

pub(crate) async fn reset_origin(ctx: &TestContext, fixture_id: &str, backend_port: u16) -> Result<()> {
    validate_origin_target(fixture_id, backend_port)?;
    tokio::time::timeout(Duration::from_secs(10), async {
        for host in origin_hosts(ctx)? {
            if observe_one(ctx, &host, fixture_id, backend_port, true).await? != 0 {
                bail!("origin reset did not return zero");
            }
        }
        if origin_count(ctx, fixture_id, backend_port).await? != 0 {
            bail!("origin reset did not establish an idle zero observation");
        }
        Ok(())
    })
    .await
    .map_err(|_| anyhow!("origin reset helper timed out"))?
}

/// Negotiated direct-origin evidence. Reqwest text collection cannot observe
/// trailer frames; this probe finishes before the case resets its owned counter.
async fn prove_origin_trailer_wire(ctx: &TestContext, fixture_id: &str, backend_port: u16) -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    validate_origin_target(fixture_id, backend_port)?;
    tokio::time::timeout(Duration::from_secs(10), async {
        for host in origin_hosts(ctx)? {
            tokio::time::timeout(Duration::from_secs(3), async {
                let url = observation_url(&host, fixture_id, backend_port, false)?;
                let host = url.host_str().ok_or_else(|| anyhow!("origin trailer host is absent"))?;
                let host = host.trim_start_matches('[').trim_end_matches(']');
                let mut stream = tokio::net::TcpStream::connect((host, backend_port)).await
                    .context("origin trailer connection failed")?;
                let request = format!("GET /cache?variant=trailer HTTP/1.1\r\nHost: cache-fixture.local\r\nTE: trailers\r\nConnection: close, te\r\n{ORIGIN_FIXTURE_HEADER}: {fixture_id}\r\n\r\n");
                stream.write_all(request.as_bytes()).await.context("origin trailer request failed")?;
                let mut wire = Vec::with_capacity(8192);
                let mut chunk = [0u8;1024];
                loop {
                    let size = stream.read(&mut chunk).await.context("origin trailer read failed")?;
                    if size == 0 { break; }
                    if wire.len().checked_add(size).is_none_or(|size| size > 8192) { bail!("origin trailer response exceeds its bound"); }
                    wire.extend_from_slice(&chunk[..size]);
                }
                let text = std::str::from_utf8(&wire).map_err(|_| anyhow!("origin trailer wire is invalid"))?;
                let (head, body) = text.split_once("\r\n\r\n").ok_or_else(|| anyhow!("origin trailer response is truncated"))?;
                if !head.starts_with("HTTP/1.1 200 ") || !head.lines().any(|line| line.eq_ignore_ascii_case("transfer-encoding: chunked"))
                    || !body.ends_with("0\r\nx-cache-terminal: present\r\n\r\n") {
                    bail!("origin did not emit the negotiated terminal trailer");
                }
                let mut stream = tokio::net::TcpStream::connect((host, backend_port)).await?;
                let request = format!("GET /cache?variant=body-error HTTP/1.1\r\nHost: cache-fixture.local\r\nConnection: close\r\n{ORIGIN_FIXTURE_HEADER}: {fixture_id}\r\n\r\n");
                stream.write_all(request.as_bytes()).await?;
                wire.clear();
                loop {
                    let size = stream.read(&mut chunk).await?;
                    if size == 0 { break; }
                    if wire.len().checked_add(size).is_none_or(|size| size > 8192) { bail!("origin body-error wire exceeds its bound"); }
                    wire.extend_from_slice(&chunk[..size]);
                }
                let wire = std::str::from_utf8(&wire)?;
                let (head, body) = wire.split_once("\r\n\r\n").ok_or_else(|| anyhow!("origin body-error header was not committed"))?;
                if !head.starts_with("HTTP/1.1 200 ") || !body.contains("public incomplete fixture")
                    || body.ends_with("0\r\n\r\n") { bail!("origin body-error fixture did not fail after header/data flush"); }
                Ok::<(), anyhow::Error>(())
            }).await.map_err(|_| anyhow!("origin trailer exchange timed out"))??;
        }
        Ok(())
    }).await.map_err(|_| anyhow!("origin trailer helper timed out"))?
}

pub struct ResponseCacheTestSuite;
pub(crate) struct ResponseCacheDynamicSuite {
    pub phase: &'static str,
}

struct Reply {
    status: u16,
    headers: reqwest::header::HeaderMap,
    body: Vec<u8>,
    marker: String,
}

fn append_probe_chunk(bytes: &mut Vec<u8>, chunk: &[u8], id: &str, status: u16) -> Result<()> {
    // Successful finite dynamic replies have the small miss-receipt bound.
    // Ordinary error pages retain the existing bounded generic policy.
    let limit = if id == "response-cache-dynamic" && status == 200 {
        128
    } else {
        1048577 + 4096
    };
    if bytes.len().checked_add(chunk.len()).is_none_or(|size| size > limit) {
        bail!("cache probe response exceeds its bound");
    }
    bytes.extend_from_slice(chunk);
    Ok(())
}

async fn probe(
    ctx: &TestContext,
    host: &str,
    id: &str,
    target: &str,
    body: Option<&str>,
    credential: bool,
    extra: Option<(&str, &str)>,
) -> Result<Reply> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let marker = format!("response-cache-{}", uuid::Uuid::new_v4());
        let url = format!("{}{target}", ctx.http_url());
        let mut request = if let Some(body) = body {
            ctx.http_client
                .post(url)
                .header("content-type", "application/json")
                .body(body.to_owned())
        } else {
            ctx.http_client.get(url)
        };
        request = request
            .header("host", host)
            .header(ORIGIN_FIXTURE_HEADER, id)
            .header("access_log", "test_store")
            .header("traceparent", crate::access_log_client::traceparent_for_marker(&marker));
        if credential {
            request = request.header("x-response-cache-key", "test-response-cache-key");
        }
        if let Some((name, value)) = extra {
            request = request.header(name, value);
        }
        let mut response = request.send().await.context("cache probe failed")?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.context("cache probe body failed")? {
            append_probe_chunk(&mut bytes, &chunk, id, status)?;
        }
        Ok(Reply {
            status,
            headers,
            body: bytes,
            marker,
        })
    })
    .await
    .map_err(|_| anyhow!("cache probe timed out"))?
}

async fn require_origin(ctx: &TestContext, id: &str, port: u16, expected: u64) -> Result<()> {
    let actual = origin_count(ctx, id, port).await?;
    if actual != expected {
        bail!("origin attempt count expected {expected}, received {actual}");
    }
    Ok(())
}

async fn require_hit_log(ctx: &TestContext, reply: &Reply) -> Result<()> {
    let log = ctx
        .access_log_client()
        .get_access_log_with_retry(&reply.marker, 15, 200)
        .await?;
    if log.status_code() != Some(200) || log.upstream_count() != 0 {
        bail!("cache hit did not preserve final status and zero upstream attempts");
    }
    Ok(())
}

async fn require_origin_completion(ctx: &TestContext, reply: &Reply) -> Result<()> {
    let log = ctx
        .access_log_client()
        .get_access_log_with_retry(&reply.marker, 15, 200)
        .await?;
    if log.status_code() != Some(200) || log.upstream_count() != 1 {
        bail!("expected origin response did not complete through the canonical terminal log");
    }
    Ok(())
}

async fn static_contract(ctx: &TestContext) -> Result<()> {
    let id = "response-cache-static";
    let host = "response-cache.example.com";
    prove_origin_trailer_wire(ctx, id, 30001).await?;
    reset_origin(ctx, id, 30001).await?;
    let first = probe(ctx, host, id, "/cache", None, true, None).await?;
    require_origin_completion(ctx, &first).await?;
    let hit = probe(ctx, host, id, "/cache", None, true, None).await?;
    if first.status != 200 || first.body != hit.body || hit.status != 200 || !hit.headers.contains_key("age") {
        bail!("public finite response did not replay with Age");
    }
    require_origin(ctx, id, 30001, 1).await?;
    require_hit_log(ctx, &hit).await?;
    let denied = probe(ctx, host, id, "/cache", None, false, None).await?;
    if denied.status != 401 {
        bail!("authentication denial did not precede cache lookup");
    }
    require_origin(ctx, id, 30001, 1).await?;

    for (index, (target, expected_len)) in [
        ("/cache?variant=empty", Some(0)),
        ("/cache?variant=no-store", None),
        ("/cache?variant=trailer", None),
        ("/cache?variant=empty-trailer", None),
        ("/cache?variant=overflow", Some(1048577)),
        ("/cache?variant=unsupported-vary", None),
        ("/cache?variant=trace-vary", None),
        ("/cache?variant=vary-star", None),
    ]
    .into_iter()
    .enumerate()
    {
        let before = origin_count(ctx, id, 30001).await?;
        let one = probe(ctx, host, id, target, None, true, None).await?;
        require_origin_completion(ctx, &one).await?;
        let two = probe(ctx, host, id, target, None, true, None).await?;
        if one.status != 200
            || two.status != 200
            || one.body != two.body
            || expected_len.is_some_and(|len| one.body.len() != len)
        {
            bail!("finite response variant {index} changed");
        }
        require_origin(ctx, id, 30001, before + if index == 0 { 1 } else { 2 }).await?;
    }
    for color in ["blue", "red"] {
        let before = origin_count(ctx, id, 30001).await?;
        let one = probe(
            ctx,
            host,
            id,
            "/cache?variant=vary",
            None,
            true,
            Some(("x-cache-vary", color)),
        )
        .await?;
        require_origin_completion(ctx, &one).await?;
        let two = probe(
            ctx,
            host,
            id,
            "/cache?variant=vary",
            None,
            true,
            Some(("x-cache-vary", color)),
        )
        .await?;
        if one.body != format!("public cache variation {color}").as_bytes() || two.body != one.body {
            bail!("represented Vary response mixed values");
        }
        require_origin(ctx, id, 30001, before + 1).await?;
    }
    let before = origin_count(ctx, id, 30001).await?;
    for _ in 0..2 {
        if probe(ctx, host, id, "/cache?variant=body-error", None, true, None)
            .await
            .is_ok()
        {
            bail!("origin body failure became a successful complete response");
        }
    }
    require_origin(ctx, id, 30001, before + 2).await?;
    for extra in [
        ("authorization", "Bearer cache-test"),
        ("cookie", "cache_test=yes"),
        ("cache-control", "no-cache"),
    ] {
        let before = origin_count(ctx, id, 30001).await?;
        probe(ctx, host, id, "/cache", None, true, Some(extra)).await?;
        probe(ctx, host, id, "/cache", None, true, Some(extra)).await?;
        require_origin(ctx, id, 30001, before + 2).await?;
    }
    for body in [
        r#"{"query":"query Q($n:Float){ping}","operationName":"Q","variables":{"n":9007199254740993}}"#,
        r#"{"query":"query Q($n:Float){ping}","operationName":"Q","variables":{"n":9007199254740992}}"#,
    ] {
        let before = origin_count(ctx, id, 30001).await?;
        let one = probe(ctx, host, id, "/graphql", Some(body), true, None).await?;
        require_origin_completion(ctx, &one).await?;
        let two = probe(ctx, host, id, "/graphql", Some(body), true, None).await?;
        let value: serde_json::Value = serde_json::from_slice(&one.body)?;
        let digest = format!("{:x}", Sha256::digest(body.as_bytes()));
        if one.status != 200
            || two.status != 200
            || one.body != two.body
            || value["bodySha256"] != digest
            || value["bodyBytes"] != body.len()
            || value["queryCredentialAbsent"] != true
        {
            bail!("raw GraphQL body identity or cache replay changed");
        }
        require_origin(ctx, id, 30001, before + 1).await?;
        require_hit_log(ctx, &two).await?;
    }
    let before = origin_count(ctx, id, 30001).await?;
    let mutation = r#"{"query":"mutation M { update }"}"#;
    probe(ctx, host, id, "/graphql", Some(mutation), true, None).await?;
    probe(ctx, host, id, "/graphql", Some(mutation), true, None).await?;
    require_origin(ctx, id, 30001, before + 2).await?;
    let invalid = probe(ctx, host, id, "/graphql", Some(r#"{"query":"invalid @@"}"#), true, None).await?;
    if invalid.status != 400 {
        bail!("invalid GraphQL envelope did not fail locally");
    }
    require_origin(ctx, id, 30001, before + 2).await?;
    Ok(())
}

impl TestSuite for ResponseCacheTestSuite {
    fn name(&self) -> &str {
        "ResponseCache Tests"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "response_cache_static_contract",
            "Sequential public origin, denial, raw GraphQL and bounded bypass contract",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match static_contract(&ctx).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error.to_string()),
                    }
                })
            },
        )]
    }
}

fn dynamic_ordinal(body: &[u8], port: u16) -> Result<u64> {
    if body.len() > 128 {
        bail!("dynamic origin body exceeds its bound");
    }
    let text = std::str::from_utf8(body).map_err(|_| anyhow!("dynamic origin body is invalid"))?;
    let prefix = format!("public cache fixture backend {port} origin ");
    let decimal = text
        .strip_prefix(&prefix)
        .ok_or_else(|| anyhow!("dynamic backend target did not converge"))?;
    if decimal.is_empty() || decimal.len() > 20 || !decimal.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("dynamic origin ordinal is invalid");
    }
    let ordinal = decimal
        .parse::<u64>()
        .map_err(|_| anyhow!("dynamic origin ordinal is invalid"))?;
    if ordinal == 0 || decimal != ordinal.to_string() {
        bail!("dynamic origin ordinal is invalid");
    }
    Ok(ordinal)
}

struct DynamicActivity {
    before: u64,
    after: u64,
    upstreams: usize,
    age: bool,
}

#[derive(Default)]
struct DynamicMissReceipt {
    origin: Option<(u64, Vec<u8>)>,
}
impl DynamicMissReceipt {
    fn observe(&mut self, cached: bool, body: &[u8], port: u16, activity: DynamicActivity) -> Result<bool> {
        let DynamicActivity {
            before,
            after,
            upstreams,
            age,
        } = activity;
        let ordinal = dynamic_ordinal(body, port)?;
        match upstreams {
            1 => {
                if after != add_count(before, 1)? || ordinal != after {
                    bail!("dynamic origin ordinal and attempt count disagree");
                }
                let bypassed = !cached && self.origin.as_ref().is_some_and(|(last, _)| *last == before);
                self.origin = Some((ordinal, body.to_vec()));
                Ok(bypassed)
            }
            0 if after == before => {
                if !cached {
                    self.origin = None;
                    return Ok(false);
                }
                Ok(age
                    && self
                        .origin
                        .as_ref()
                        .is_some_and(|(last, previous)| *last == after && *last == ordinal && previous == body))
            }
            _ => bail!("dynamic response and actual origin activity disagree"),
        }
    }
}

fn dynamic_profile_matches(controller: &serde_json::Value, gateway: &serde_json::Value, phase: &str) -> bool {
    let matches = |resource: &serde_json::Value| {
        let cache = resource["spec"]["requestPlugins"]
            .as_array()
            .and_then(|entries| entries.iter().find(|entry| entry["type"] == "ResponseCache"));
        let cache_ok = cache.is_some_and(|entry| {
            entry["enable"].as_bool().unwrap_or(true) == (phase != "disabled")
                && entry["config"]["maxTtl"] == "60s"
                && entry["config"]["graphqlQueries"] == true
        });
        let body = resource["spec"]["upstreamResponseBodyFilterPlugins"].as_array();
        let body_ok = if phase == "body-filter" {
            body.is_some_and(|entries| {
                entries.len() == 1 && entries[0]["type"] == "BandwidthLimit" && entries[0]["config"]["rate"] == "100mb"
            })
        } else {
            body.is_none_or(Vec::is_empty)
        };
        cache_ok && body_ok
    };
    matches(controller)
        && matches(gateway)
        && controller["metadata"]["resourceVersion"]
            .as_str()
            .is_some_and(|version| !version.is_empty())
        && controller["metadata"]["resourceVersion"] == gateway["metadata"]["resourceVersion"]
        && controller["metadata"].get("generation") == gateway["metadata"].get("generation")
        && controller["metadata"].get("uid") == gateway["metadata"].get("uid")
}

async fn dynamic_resource_ready(ctx: &TestContext, phase: &str) -> Result<bool> {
    let controller = crate::suites::status::helpers::fetch_resource_status(
        ctx,
        "EdgionPlugins",
        "edgion-default",
        "response-cache-dynamic",
    )
    .await
    .map_err(|error| anyhow!(error))?;
    let accepted = crate::suites::status::helpers::controller_local_conditions(&controller["status"])
        .and_then(|conditions| conditions.iter().find(|condition| condition["type"] == "Accepted"));
    if !accepted.is_some_and(|condition| {
        condition["status"] == "True"
            && controller["metadata"]["generation"]
                .as_i64()
                .is_none_or(|generation| condition["observedGeneration"].as_i64() == Some(generation))
    }) {
        return Ok(false);
    }
    let base =
        std::env::var("EDGION_TEST_GATEWAY_ADMIN_URL").unwrap_or_else(|_| format!("http://{}:12001", ctx.target_host));
    let mut request = ctx
        .http_client
        .get(format!("{}/configclient/EdgionPlugins", base.trim_end_matches('/')))
        .query(&[("namespace", "edgion-default"), ("name", "response-cache-dynamic")])
        .timeout(Duration::from_secs(3));
    if let Ok(token) = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN") {
        request = request.bearer_auth(token);
    }
    let mut response = request
        .send()
        .await
        .context("dynamic Gateway profile request failed")?
        .error_for_status()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().checked_add(chunk.len()).is_none_or(|size| size > 65536) {
            bail!("dynamic Gateway profile exceeds its bound");
        }
        bytes.extend_from_slice(&chunk);
    }
    let gateway: serde_json::Value = serde_json::from_slice(&bytes)?;
    Ok(gateway["success"] == true && dynamic_profile_matches(&controller, &gateway["data"], phase))
}

async fn dynamic_contract(ctx: &TestContext, phase: &str) -> Result<()> {
    let id = "response-cache-dynamic";
    let host = "response-cache-dynamic.example.com";
    let target = "/cache?dynamic=publication-contract";
    if phase == "deleted" {
        let before = origin_count(ctx, id, 30002).await?;
        let reply = probe(ctx, host, id, target, None, true, None).await?;
        if reply.status != 404 {
            bail!("deleted route remains reachable");
        }
        return require_origin(ctx, id, 30002, before).await;
    }
    let cached = !matches!(phase, "disabled" | "body-filter");
    let port = if phase == "target" { 30002 } else { 30001 };
    // Every invocation is one transition: the two reenabled transitions never share this owner.
    let mut receipt = DynamicMissReceipt::default();
    loop {
        if !dynamic_resource_ready(ctx, phase).await? {
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        }
        let before = origin_count(ctx, id, port).await?;
        let reply = probe(ctx, host, id, target, None, true, None).await?;
        let log = ctx
            .access_log_client()
            .get_access_log_with_retry(&reply.marker, 15, 200)
            .await?;
        let after = origin_count(ctx, id, port).await?;
        if reply.status != 200 || log.status_code() != Some(200) {
            bail!("dynamic origin status did not converge");
        }
        // A predecessor origin may still use the old target while the mapping is received.
        if dynamic_ordinal(&reply.body, port).is_err() {
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        }
        if receipt.observe(
            cached,
            &reply.body,
            port,
            DynamicActivity {
                before,
                after,
                upstreams: log.upstream_count(),
                age: reply.headers.contains_key("age"),
            },
        )? {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

impl TestSuite for ResponseCacheDynamicSuite {
    fn name(&self) -> &str {
        "ResponseCache dynamic phase"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let phase = self.phase;
        vec![TestCase::new(
            format!("response_cache_dynamic_{phase}"),
            "Accepted publication and independent origin attempts",
            move |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = tokio::time::timeout(Duration::from_secs(20), dynamic_contract(&ctx, phase))
                        .await
                        .map_err(|_| anyhow!("dynamic phase miss-to-hit convergence timed out"))
                        .and_then(|result| result);
                    match result {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error.to_string()),
                    }
                })
            },
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dynamic_probe_caps_successful_receipts_and_preserves_bounded_ordinary_error_pages() {
        let id = "response-cache-dynamic";
        let mut success = Vec::new();
        append_probe_chunk(&mut success, &[b'x'; 64], id, 200).unwrap();
        append_probe_chunk(&mut success, &[b'y'; 64], id, 200).unwrap();
        assert_eq!(success.len(), 128);
        assert!(append_probe_chunk(&mut success, b"z", id, 200).is_err());
        assert_eq!(success.len(), 128, "overflow chunk is never copied");
        let mut error = Vec::new();
        append_probe_chunk(&mut error, &[b'x'; 100], id, 404).unwrap();
        append_probe_chunk(&mut error, &[b'y'; 100], id, 404).unwrap();
        assert_eq!(error.len(), 200, "ordinary 404 pages may exceed the finite receipt cap");
        let remaining = vec![b'z'; 1048577 + 4096 - error.len()];
        append_probe_chunk(&mut error, &remaining, id, 404).unwrap();
        assert!(append_probe_chunk(&mut error, b"z", id, 404).is_err());
        assert_eq!(error.len(), 1048577 + 4096);
        let mut one_chunk = Vec::new();
        assert!(append_probe_chunk(&mut one_chunk, &[b'x'; 129], id, 200).is_err());
        assert!(one_chunk.is_empty());
    }

    #[test]
    fn dynamic_miss_receipt_requires_a_new_ordinal_and_resets_per_transition() {
        let body = |ordinal| format!("public cache fixture backend 30001 origin {ordinal}").into_bytes();
        let activity = |before, after, upstreams, age| DynamicActivity {
            before,
            after,
            upstreams,
            age,
        };
        let mut receipt = DynamicMissReceipt::default();
        assert!(!receipt.observe(true, &body(1), 30001, activity(1, 1, 0, true)).unwrap());
        assert!(!receipt
            .observe(true, &body(2), 30001, activity(1, 2, 1, false))
            .unwrap());
        assert!(!receipt
            .observe(true, &body(3), 30001, activity(2, 3, 1, false))
            .unwrap());
        assert!(!receipt.observe(true, &body(1), 30001, activity(3, 3, 0, true)).unwrap());
        assert!(receipt.observe(true, &body(3), 30001, activity(3, 3, 0, true)).unwrap());
        let mut restored = DynamicMissReceipt::default();
        assert!(!restored
            .observe(true, &body(3), 30001, activity(3, 3, 0, true))
            .unwrap());
        assert!(!restored
            .observe(true, &body(4), 30001, activity(3, 4, 1, false))
            .unwrap());
        assert!(restored
            .observe(true, &body(4), 30001, activity(4, 4, 0, true))
            .unwrap());
        let mut bypass = DynamicMissReceipt::default();
        assert!(!bypass
            .observe(false, &body(5), 30001, activity(4, 5, 1, false))
            .unwrap());
        assert!(!bypass.observe(false, &body(5), 30001, activity(5, 5, 0, true)).unwrap());
        assert!(!bypass
            .observe(false, &body(6), 30001, activity(5, 6, 1, false))
            .unwrap());
        assert!(bypass
            .observe(false, &body(7), 30001, activity(6, 7, 1, false))
            .unwrap());
        assert!(bypass
            .observe(false, &body(8), 30001, activity(7, 9, 1, false))
            .is_err());
        for invalid in ["0", "01", "-1", "18446744073709551616", "1 extra"] {
            assert!(dynamic_ordinal(
                format!("public cache fixture backend 30001 origin {invalid}").as_bytes(),
                30001
            )
            .is_err());
        }
    }

    #[test]
    fn dynamic_resource_receipt_requires_current_authored_profile_and_matching_identity() {
        let safe = serde_json::json!({"metadata":{"resourceVersion":"2"},"spec":{"requestPlugins":[{"type":"ResponseCache","enable":true,"config":{"maxTtl":"60s","graphqlQueries":true}}]}});
        assert!(dynamic_profile_matches(&safe, &safe, "reenabled"));
        assert!(!dynamic_profile_matches(&safe, &safe, "disabled"));
        let mut old = safe.clone();
        old["metadata"]["resourceVersion"] = serde_json::json!("1");
        assert!(!dynamic_profile_matches(&safe, &old, "reenabled"));
        let mut body = safe.clone();
        body["spec"]["upstreamResponseBodyFilterPlugins"] =
            serde_json::json!([{"type":"BandwidthLimit","config":{"rate":"100mb"}}]);
        assert!(dynamic_profile_matches(&body, &body, "body-filter"));
        assert!(!dynamic_profile_matches(&body, &safe, "body-filter"));
        assert!(!dynamic_profile_matches(&body, &body, "reenabled"));
    }

    #[test]
    fn static_and_dynamic_plugin_fixtures_are_real_typed_accepted_profiles() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("integration/conf/EdgionPlugins/ResponseCache");
        for path in [
            "10_EdgionPlugins_response-cache.yaml",
            "DynamicTest/initial/10_EdgionPlugins.yaml",
            "DynamicTest/updates/disable/10_EdgionPlugins.yaml",
            "DynamicTest/updates/reenable/10_EdgionPlugins.yaml",
            "DynamicTest/updates/body-filter/10_EdgionPlugins.yaml",
        ] {
            let text = std::fs::read_to_string(root.join(path)).unwrap();
            let plugins: edgion_resources::EdgionPlugins = serde_yaml::from_str(&text).unwrap();
            assert!(plugins.validate_plugins().is_empty(), "invalid typed fixture {path}");
            assert!(plugins.spec.request_plugins.as_ref().unwrap().iter().any(|entry| {
                matches!(&entry.plugin, edgion_resources::EdgionPlugin::KeyAuth(config)
                    if config.hide_credentials && config.upstream_header_fields.is_empty() && config.anonymous.is_none())
            }));
        }
    }
    #[test]
    fn bounded_origin_reply_rejects_missing_duplicate_extra_and_non_u64_fields() {
        assert_eq!(decode_count(br#"{"count":0}"#).unwrap(), 0);
        assert_eq!(decode_count(br#"{"count":18446744073709551615}"#).unwrap(), u64::MAX);
        for invalid in [
            br#"{}"#.as_slice(),
            br#"{"count":1,"extra":0}"#,
            br#"{"count":1,"count":2}"#,
            br#"{"count":-1}"#,
            br#"{"count":1.0}"#,
            br#"{"count":18446744073709551616}"#,
            br#"{"count":0}{}"#,
        ] {
            assert!(decode_count(invalid).is_err());
        }
        assert!(decode_count(&[b' '; 257]).is_err());
        assert_eq!(add_count(u64::MAX - 1, 1).unwrap(), u64::MAX);
        assert!(add_count(u64::MAX, 1).is_err());
    }
    #[test]
    fn direct_observation_urls_preserve_ipv6_and_reject_unknown_targets_before_send() {
        let url = observation_url("::1", "dsl-query-lookup-static", 30001, false).unwrap();
        assert_eq!(
            url.as_str(),
            "http://[::1]:30001/response-cache/observations/dsl-query-lookup-static"
        );
        assert_eq!(
            observation_url("127.0.0.1", "response-cache-static", 30002, true)
                .unwrap()
                .path(),
            "/response-cache/observations/response-cache-static/reset"
        );
        assert!(observation_url("127.0.0.1", "unknown", 30001, false).is_err());
        assert!(observation_url("127.0.0.1", "response-cache-static", 30003, false).is_err());
        assert!(observation_url("", "response-cache-static", 30001, false).is_err());
    }
}
