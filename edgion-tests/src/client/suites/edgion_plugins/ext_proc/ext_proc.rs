// ExtProc Plugin Test Suite
//
// Tests the ExtProc plugin, which hands request/response phases to an external
// processing service over a bidirectional gRPC stream (Envoy ext_proc
// compatible). The fake processor (test_server --ext-proc-port 30041):
//   - request phase: adds `x-ext-proc-req: seen`, or returns 403 if the request
//     carries `x-ext-proc-block`.
//   - response phase: adds `x-ext-proc-resp: seen`.
//
// Scenarios:
//   1. Basic: request header added (seen by backend) + response header added.
//   2. ImmediateResponse: `x-ext-proc-block` short-circuits with 403.
//   3. fail-close: a dead processor target returns 500 (failureModeAllow=false).
//   4. fail-open: a dead processor target is bypassed, request reaches backend.
//
// Requires the `--gateway` flag (exercises the real data path).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::grpc_route::test::test_service_client::TestServiceClient;
use crate::suites::grpc_route::test::HelloRequest;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const BASIC_HOST: &str = "ext-proc-basic.example.com";
const FAILCLOSE_HOST: &str = "ext-proc-failclose.example.com";
const FAILOPEN_HOST: &str = "ext-proc-failopen.example.com";
const COEXIST_HOST: &str = "ext-proc-coexist.example.com";
const TLS_HOST: &str = "ext-proc-tls.example.com";
const TLS_NOVERIFY_HOST: &str = "ext-proc-tls-noverify.example.com";
const MTLS_HOST: &str = "ext-proc-mtls.example.com";
const STREAMED_HOST: &str = "ext-proc-streamed.example.com";
const STREAMED_FAILOPEN_HOST: &str = "ext-proc-streamed-failopen.example.com";
const EXTERNAL_HOST: &str = "ext-proc-external.example.com";
const EXTERNAL_BUFFERED_HOST: &str = "ext-proc-external-buffered.example.com";
const EXTERNAL_STREAMED_HOST: &str = "ext-proc-external-streamed.example.com";
const EXTERNAL_STRESS_STREAMED_HOST: &str = "ext-proc-external-stress-streamed.example.com";
const EXTERNAL_STRESS_HEADERS_HOST: &str = "ext-proc-external-stress-headers.example.com";
const EXTERNAL_TLS_HOST: &str = "ext-proc-external-tls.example.com";
const EXTERNAL_MTLS_HOST: &str = "ext-proc-external-mtls.example.com";
const EXTERNAL_DISABLED_IMMEDIATE_HOST: &str = "ext-proc-external-disabled-immediate.example.com";
const EXTERNAL_MODE_OVERRIDE_HOST: &str = "ext-proc-external-mode-override.example.com";
const EXTERNAL_RESPONSE_TRAILERS_HOST: &str = "ext-proc-external-response-trailers.example.com";
const EXTERNAL_ADMIN_URL_ENV: &str = "EDGION_EXT_PROC_EXTERNAL_ADMIN_URL";
const EXTERNAL_REQUIRED_ENV: &str = "EDGION_EXT_PROC_EXTERNAL_REQUIRED";
const EXTERNAL_TLS_MATERIAL_DIR_ENV: &str = "EDGION_EXT_PROC_EXTERNAL_TLS_MATERIAL_DIR";
const EDGION_WORK_DIR_ENV: &str = "EDGION_WORK_DIR";
const EXTERNAL_REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
const EXTERNAL_STRESS_REQUEST_TIMEOUT: Duration = Duration::from_secs(35);
const EXTERNAL_ADMIN_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const EXTERNAL_OBSERVATION_TIMEOUT: Duration = Duration::from_secs(5);
const EXTERNAL_FIXTURE_STREAM_CAPACITY: usize = 96;
const EXTERNAL_GATEWAY_SESSION_CAPACITY: usize = 64;
const SEEDED_BODY_BYTES: usize = 512 * 1024;
const SEEDED_MAX_CHUNK_BYTES: usize = 64 * 1024;
const SEEDED_MAX_PLAN_CHUNKS: usize = 4096;
const SEEDED_CHUNK_SEEDS: &[u64] = &[
    0x0123_4567_89ab_cdef,
    0x0f1e_2d3c_4b5a_6978,
    0x3141_5926_5358_9793,
    0x9e37_79b9_7f4a_7c15,
    0xd1b5_4a32_d192_ed03,
];

#[derive(Debug, Deserialize)]
struct ExternalReady {
    status: String,
    fixture: String,
    protocol_version: String,
    go_control_plane_envoy: String,
    fixture_stream_capacity: usize,
}

#[derive(Debug, Deserialize)]
struct ExternalSnapshot {
    case_id: String,
    streams: Vec<ExternalStreamSnapshot>,
}

#[derive(Debug, Deserialize)]
struct ExternalStreamSnapshot {
    stream_id: u64,
    phases: Vec<String>,
    #[serde(default)]
    transport: String,
    #[serde(default)]
    tls_generation: Option<String>,
    #[serde(default)]
    server_leaf_sha256: Option<String>,
    #[serde(default)]
    peer_leaf_sha256: Option<String>,
    request_header_messages: usize,
    request_body_messages: usize,
    request_body_bytes: usize,
    request_body_sha256: String,
    request_eos_count: usize,
    response_header_messages: usize,
    response_statuses: Vec<u16>,
    request_opaque_headers_ok: bool,
    response_opaque_headers_ok: bool,
    response_body_messages: usize,
    response_body_bytes: usize,
    response_body_sha256: String,
    response_eos_count: usize,
    response_trailer_messages: usize,
    response_trailer_fields: usize,
    saw_fixed_response_trailer: bool,
    saw_grpc_status_trailer: bool,
    client_half_closed: bool,
    cancelled: bool,
    active: bool,
}

#[derive(Debug, Deserialize)]
struct ExternalTlsSnapshot {
    server_ca_sha256: String,
    client_ca_sha256: String,
    client_a_leaf_sha256: String,
    client_b_leaf_sha256: String,
    fixture_stream_capacity: usize,
    tls: ExternalTlsListenerSnapshot,
    mtls: ExternalTlsListenerSnapshot,
}

#[derive(Debug, Deserialize)]
struct ExternalTlsListenerSnapshot {
    enabled: bool,
    ready: bool,
    current_generation: String,
    current_leaf_sha256: String,
    generation_a_leaf_sha256: String,
    generation_b_leaf_sha256: String,
    active_connections: usize,
}

#[derive(Debug, Deserialize)]
struct ExternalBarrierSnapshot {
    barrier_id: String,
    waiting: usize,
    arrivals: u64,
    releases: u64,
    released_all: bool,
    waiter_capacity: usize,
    fixture_stream_capacity: usize,
}

#[derive(Debug)]
struct ExternalHttpResult {
    status: u16,
    response_header: bool,
    body: bytes::Bytes,
}

/// Generate a deterministic chunk plan with xorshift64* (Marsaglia shifts
/// 12/25/27 and multiplier 2685821657736338717). The algorithm and seed list
/// are local test ABI: changing either must update the stability test.
fn seeded_chunk_plan(seed: u64, total_bytes: usize) -> Result<Vec<usize>, String> {
    if seed == 0 || total_bytes == 0 {
        return Err("seed and total bytes must both be nonzero".to_string());
    }
    let mut state = seed;
    let mut remaining = total_bytes;
    let mut plan = Vec::new();
    while remaining != 0 {
        if plan.len() >= SEEDED_MAX_PLAN_CHUNKS {
            return Err(format!("seed {seed:#x} exceeded the bounded chunk-plan length"));
        }
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let value = state.wrapping_mul(2_685_821_657_736_338_717);
        let proposed = (value as usize % SEEDED_MAX_CHUNK_BYTES) + 1;
        let chunk = proposed.min(remaining);
        plan.push(chunk);
        remaining -= chunk;
    }
    Ok(plan)
}

fn seeded_payload(seed: u64, total_bytes: usize) -> Vec<u8> {
    (0..total_bytes)
        .map(|index| ((index as u64).wrapping_mul(131).wrapping_add(seed) & 0xff) as u8)
        .collect()
}

fn sha256_hex(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

async fn external_admin_url(client: &reqwest::Client) -> Result<String, String> {
    if std::env::var(EXTERNAL_REQUIRED_ENV).as_deref() != Ok("1") {
        return Err(format!(
            "{}=1 is required for external ExtProc cases",
            EXTERNAL_REQUIRED_ENV
        ));
    }
    let admin_url = std::env::var(EXTERNAL_ADMIN_URL_ENV)
        .map_err(|_| format!("{} is required for external ExtProc cases", EXTERNAL_ADMIN_URL_ENV))?;
    let admin_url = admin_url.trim().trim_end_matches('/');
    if admin_url.is_empty() {
        return Err(format!("{} must not be empty", EXTERNAL_ADMIN_URL_ENV));
    }

    let response = client
        .get(format!("{admin_url}/readyz"))
        .timeout(EXTERNAL_ADMIN_REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|error| format!("external ExtProc readiness request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("external ExtProc readiness returned {}", response.status()));
    }
    let ready = response
        .json::<ExternalReady>()
        .await
        .map_err(|error| format!("external ExtProc readiness was invalid: {error}"))?;
    if ready.status != "ready"
        || ready.fixture != "edgion-ext-proc-external"
        || ready.protocol_version != "v1"
        || ready.go_control_plane_envoy != "v1.40.0"
        || ready.fixture_stream_capacity != EXTERNAL_FIXTURE_STREAM_CAPACITY
    {
        return Err(format!("unexpected external ExtProc fixture identity: {ready:?}"));
    }
    Ok(admin_url.to_string())
}

async fn reset_external_case(client: &reqwest::Client, admin_url: &str, case_id: &str) -> Result<(), String> {
    let response = client
        .post(format!("{admin_url}/v1/cases/{case_id}/reset"))
        .timeout(EXTERNAL_ADMIN_REQUEST_TIMEOUT)
        .body(Vec::new())
        .send()
        .await
        .map_err(|error| format!("failed to reset external ExtProc case {case_id}: {error}"))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!(
            "external ExtProc reset for {case_id} returned {}",
            response.status()
        ))
    }
}

async fn prepare_external_case(client: &reqwest::Client, case_id: &str) -> Result<String, String> {
    let admin_url = external_admin_url(client).await?;
    reset_external_case(client, &admin_url, case_id).await?;
    Ok(admin_url)
}

async fn external_admin_post(client: &reqwest::Client, url: String, operation: &str) -> Result<(), String> {
    let response = client
        .post(url)
        .timeout(EXTERNAL_ADMIN_REQUEST_TIMEOUT)
        .body(Vec::new())
        .send()
        .await
        .map_err(|error| format!("external ExtProc {operation} failed: {error}"))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("external ExtProc {operation} returned {}", response.status()))
    }
}

async fn reset_external_barrier(client: &reqwest::Client, admin_url: &str, barrier_id: &str) -> Result<(), String> {
    external_admin_post(
        client,
        format!("{admin_url}/v1/barriers/{barrier_id}/reset"),
        "barrier reset",
    )
    .await
}

async fn release_external_barrier_all(
    client: &reqwest::Client,
    admin_url: &str,
    barrier_id: &str,
) -> Result<(), String> {
    external_admin_post(
        client,
        format!("{admin_url}/v1/barriers/{barrier_id}/release-all"),
        "barrier release-all",
    )
    .await
}

async fn release_external_barrier_one(
    client: &reqwest::Client,
    admin_url: &str,
    barrier_id: &str,
) -> Result<(), String> {
    external_admin_post(
        client,
        format!("{admin_url}/v1/barriers/{barrier_id}/release-one"),
        "barrier release-one",
    )
    .await
}

async fn poll_external_barrier(
    client: &reqwest::Client,
    admin_url: &str,
    barrier_id: &str,
    expected_waiting: usize,
) -> Result<ExternalBarrierSnapshot, String> {
    let deadline = Instant::now() + EXTERNAL_OBSERVATION_TIMEOUT;
    loop {
        let observation = match client
            .get(format!("{admin_url}/v1/barriers/{barrier_id}"))
            .timeout(EXTERNAL_ADMIN_REQUEST_TIMEOUT)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                let snapshot = response
                    .json::<ExternalBarrierSnapshot>()
                    .await
                    .map_err(|error| format!("external ExtProc barrier snapshot was invalid: {error}"))?;
                if snapshot.barrier_id != barrier_id
                    || snapshot.waiter_capacity != EXTERNAL_FIXTURE_STREAM_CAPACITY
                    || snapshot.fixture_stream_capacity != EXTERNAL_FIXTURE_STREAM_CAPACITY
                {
                    return Err(format!("unexpected external ExtProc barrier identity: {snapshot:?}"));
                }
                if snapshot.waiting == expected_waiting {
                    return Ok(snapshot);
                }
                format!("barrier not ready: {snapshot:?}")
            }
            Ok(response) => format!("barrier snapshot returned {}", response.status()),
            Err(error) => format!("barrier snapshot request failed: {error}"),
        };
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for barrier {barrier_id} waiting={expected_waiting}: {observation}"
            ));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn assert_external_case_absent(client: &reqwest::Client, admin_url: &str, case_id: &str) -> Result<(), String> {
    let response = client
        .get(format!("{admin_url}/v1/cases/{case_id}"))
        .timeout(EXTERNAL_ADMIN_REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|error| format!("external ExtProc absent-case query failed: {error}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        Ok(())
    } else {
        Err(format!(
            "expected external ExtProc case {case_id} to be absent, got {}",
            response.status()
        ))
    }
}

async fn send_external_header_request(
    client: reqwest::Client,
    gateway_url: String,
    case_id: String,
    mode: &'static str,
    barrier_id: Option<String>,
) -> Result<ExternalHttpResult, String> {
    let mut request = client
        .get(format!("{gateway_url}/headers"))
        .header("host", EXTERNAL_STRESS_HEADERS_HOST)
        .header("x-ext-proc-case-id", &case_id)
        .header("x-ext-proc-test-mode", mode)
        .timeout(EXTERNAL_STRESS_REQUEST_TIMEOUT);
    if let Some(barrier_id) = barrier_id {
        request = request.header("x-ext-proc-barrier-id", barrier_id);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("external ExtProc request {case_id} failed: {error}"))?;
    let status = response.status().as_u16();
    let response_header = response
        .headers()
        .get("x-ext-proc-resp")
        .and_then(|value| value.to_str().ok())
        == Some("external-go");
    let body = tokio::time::timeout(EXTERNAL_ADMIN_REQUEST_TIMEOUT, response.bytes())
        .await
        .map_err(|_| format!("external ExtProc response body {case_id} timed out"))?
        .map_err(|error| format!("external ExtProc response body {case_id} failed: {error}"))?;
    Ok(ExternalHttpResult {
        status,
        response_header,
        body,
    })
}

async fn join_external_request_tasks(
    tasks: Vec<(String, tokio::task::JoinHandle<Result<ExternalHttpResult, String>>)>,
    timeout: Duration,
) -> Result<Vec<(String, ExternalHttpResult)>, String> {
    let abort_handles: Vec<_> = tasks.iter().map(|(_, task)| task.abort_handle()).collect();
    let joins = tasks.into_iter().map(|(case_id, task)| async move {
        let result = task
            .await
            .map_err(|error| format!("external ExtProc request task {case_id} failed: {error}"))??;
        Ok::<_, String>((case_id, result))
    });
    let mut all = Box::pin(futures::future::join_all(joins));
    let Ok(results) = tokio::time::timeout(timeout, &mut all).await else {
        for handle in abort_handles {
            handle.abort();
        }
        let _ = tokio::time::timeout(Duration::from_secs(1), &mut all).await;
        return Err(format!(
            "external ExtProc request-task cleanup timed out after {timeout:?}"
        ));
    };
    results.into_iter().collect()
}

async fn poll_external_snapshot(
    client: &reqwest::Client,
    admin_url: &str,
    case_id: &str,
) -> Result<ExternalSnapshot, String> {
    let deadline = Instant::now() + EXTERNAL_OBSERVATION_TIMEOUT;
    loop {
        let observation = match client
            .get(format!("{admin_url}/v1/cases/{case_id}"))
            .timeout(EXTERNAL_ADMIN_REQUEST_TIMEOUT)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => match response.json::<ExternalSnapshot>().await {
                Ok(snapshot) if snapshot.streams.len() == 1 && !snapshot.streams[0].active => return Ok(snapshot),
                Ok(snapshot) => format!("snapshot not terminal: {snapshot:?}"),
                Err(error) => return Err(format!("external ExtProc snapshot was invalid: {error}")),
            },
            Ok(response) => format!("snapshot returned {}", response.status()),
            Err(error) => format!("snapshot request failed: {error}"),
        };
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for terminal external ExtProc snapshot for {case_id}: {observation}"
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn assert_external_stream<'a>(
    snapshot: &'a ExternalSnapshot,
    case_id: &str,
    require_half_close: bool,
) -> Result<&'a ExternalStreamSnapshot, String> {
    if snapshot.case_id != case_id {
        return Err(format!(
            "snapshot case mismatch: expected {case_id}, got {}",
            snapshot.case_id
        ));
    }
    if snapshot.streams.len() != 1 {
        return Err(format!("expected exactly one stream, got {}", snapshot.streams.len()));
    }
    let stream = &snapshot.streams[0];
    if stream.active || (require_half_close && (stream.cancelled || !stream.client_half_closed)) {
        return Err(format!(
            "unexpected terminal stream state: id={} active={} cancelled={} half_closed={}",
            stream.stream_id, stream.active, stream.cancelled, stream.client_half_closed
        ));
    }
    Ok(stream)
}

fn assert_phase_groups(stream: &ExternalStreamSnapshot, expected: &[&str]) -> Result<(), String> {
    let groups: Vec<&str> = stream
        .phases
        .iter()
        .map(String::as_str)
        .fold(Vec::new(), |mut phases, phase| {
            if phases.last().copied() != Some(phase) {
                phases.push(phase);
            }
            phases
        });
    if groups == expected {
        Ok(())
    } else {
        Err(format!(
            "unexpected phase pattern: expected {expected:?}, got {:?} (raw {:?})",
            groups, stream.phases
        ))
    }
}

fn assert_exact_phases(stream: &ExternalStreamSnapshot, expected: &[&str]) -> Result<(), String> {
    let actual: Vec<&str> = stream.phases.iter().map(String::as_str).collect();
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "unexpected exact phases: expected {expected:?}, got {actual:?}"
        ))
    }
}

fn upstream_has_retry_on_status(upstream: &serde_json::Value) -> bool {
    upstream["err"]
        .as_array()
        .is_some_and(|errors| errors.iter().any(|error| error.as_str() == Some("RetryOnStatus")))
}

fn assert_no_body_observations(stream: &ExternalStreamSnapshot) -> Result<(), String> {
    let empty_sha256 = sha256_hex(b"");
    if stream.request_body_messages == 0
        && stream.request_body_bytes == 0
        && stream.request_body_sha256 == empty_sha256
        && stream.request_eos_count == 0
        && stream.response_body_messages == 0
        && stream.response_body_bytes == 0
        && stream.response_body_sha256 == empty_sha256
        && stream.response_eos_count == 0
    {
        Ok(())
    } else {
        Err(format!("unexpected body observations: {stream:?}"))
    }
}

fn assert_header_observations(
    stream: &ExternalStreamSnapshot,
    response_header_messages: usize,
    expected_statuses: &[u16],
) -> Result<(), String> {
    let statuses_match = stream.response_statuses == expected_statuses;
    if stream.request_header_messages == 1
        && stream.response_header_messages == response_header_messages
        && statuses_match
    {
        Ok(())
    } else {
        Err(format!("unexpected header observations: {stream:?}"))
    }
}

fn assert_plain_transport(stream: &ExternalStreamSnapshot) -> Result<(), String> {
    if stream.transport == "plain"
        && stream.tls_generation.as_deref().unwrap_or_default().is_empty()
        && stream.server_leaf_sha256.as_deref().unwrap_or_default().is_empty()
        && stream.peer_leaf_sha256.as_deref().unwrap_or_default().is_empty()
    {
        Ok(())
    } else {
        Err(format!("unexpected plain transport observation: {stream:?}"))
    }
}

async fn external_h2_response_trailers(
    ctx: &TestContext,
    case_id: &str,
    mode: &str,
    upstream_trailers: bool,
) -> Result<(u16, bytes::Bytes, Option<http::HeaderMap>), String> {
    let tcp = tokio::net::TcpStream::connect((ctx.target_host.as_str(), ctx.http_port))
        .await
        .map_err(|error| format!("connect h2 gateway: {error}"))?;
    let (mut client, connection) = h2::client::handshake(tcp)
        .await
        .map_err(|error| format!("h2 handshake: {error}"))?;
    let driver = tokio::spawn(connection);
    let uri = format!("http://{EXTERNAL_RESPONSE_TRAILERS_HOST}/transport")
        .parse::<http::Uri>()
        .map_err(|error| format!("build h2 URI: {error}"))?;
    let request = http::Request::builder()
        .method(http::Method::GET)
        .uri(uri)
        .header("x-ext-proc-case-id", case_id)
        .header("x-ext-proc-test-mode", mode)
        .header("x-up-response-trailers", upstream_trailers.to_string())
        .body(())
        .map_err(|error| format!("build h2 request: {error}"))?;
    let (response, _) = client
        .send_request(request, true)
        .map_err(|error| format!("send h2 request: {error}"))?;
    let response = response
        .await
        .map_err(|error| format!("receive h2 response: {error}"))?;
    let status = response.status().as_u16();
    let mut body = response.into_body();
    let mut bytes = Vec::new();
    while let Some(chunk) = body.data().await {
        let chunk = chunk.map_err(|error| format!("receive h2 body: {error}"))?;
        bytes.extend_from_slice(&chunk);
        body.flow_control()
            .release_capacity(chunk.len())
            .map_err(|error| format!("release h2 capacity: {error}"))?;
    }
    let trailers = body
        .trailers()
        .await
        .map_err(|error| format!("receive h2 trailers: {error}"))?;
    drop(client);
    driver.abort();
    let _ = driver.await;
    Ok((status, bytes::Bytes::from(bytes), trailers))
}

fn raw_header_values(headers: &[(String, Vec<u8>)], name: &str) -> Vec<Vec<u8>> {
    headers
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
        .collect()
}

type RawHeaderList = Vec<(String, Vec<u8>)>;

fn parse_h1_response_head(raw: &[u8]) -> Result<(u16, RawHeaderList), String> {
    let mut lines = raw.split(|byte| *byte == b'\n');
    let status_line = lines
        .next()
        .ok_or_else(|| "HTTP/1 response head was empty".to_string())?;
    let status_line = status_line.strip_suffix(b"\r").unwrap_or(status_line);
    let status_text =
        std::str::from_utf8(status_line).map_err(|error| format!("HTTP/1 status line was not ASCII: {error}"))?;
    let status = status_text
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| format!("HTTP/1 status line was invalid: {status_text:?}"))?;
    let mut headers = Vec::new();
    for line in lines {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            break;
        }
        let Some(colon) = line.iter().position(|byte| *byte == b':') else {
            return Err(format!("HTTP/1 response header had no colon: {line:?}"));
        };
        let name = std::str::from_utf8(&line[..colon])
            .map_err(|error| format!("HTTP/1 response header name was not ASCII: {error}"))?
            .to_string();
        let value = line[colon + 1..]
            .iter()
            .skip_while(|byte| matches!(byte, b' ' | b'\t'))
            .copied()
            .collect();
        headers.push((name, value));
    }
    Ok((status, headers))
}

async fn external_opaque_h1(
    ctx: &TestContext,
    case_id: &str,
    mode: &str,
) -> Result<(u16, Vec<(String, Vec<u8>)>), String> {
    let mut stream = tokio::net::TcpStream::connect((ctx.target_host.as_str(), ctx.http_port))
        .await
        .map_err(|error| format!("connect HTTP/1 gateway: {error}"))?;
    let mut request = format!(
        "GET /headers HTTP/1.1\r\nHost: {EXTERNAL_HOST}\r\nx-ext-proc-case-id: {case_id}\r\nx-ext-proc-test-mode: {mode}\r\n"
    )
    .into_bytes();
    for (name, value) in [
        ("x-opaque-presence", b"\xffrequest-first".as_slice()),
        ("x-opaque-presence", b"\x80request-second"),
        ("x-opaque-overwrite", b"\xferequest-old"),
    ] {
        request.extend_from_slice(name.as_bytes());
        request.extend_from_slice(b": ");
        request.extend_from_slice(value);
        request.extend_from_slice(b"\r\n");
    }
    request.extend_from_slice(b"Connection: close\r\n\r\n");
    stream
        .write_all(&request)
        .await
        .map_err(|error| format!("write HTTP/1 opaque request: {error}"))?;

    let mut response = Vec::new();
    loop {
        if let Some(end) = response.windows(4).position(|window| window == b"\r\n\r\n") {
            return parse_h1_response_head(&response[..end + 4]);
        }
        if response.len() >= 64 * 1024 {
            return Err("HTTP/1 opaque response head exceeded 64 KiB".to_string());
        }
        let mut chunk = [0_u8; 4096];
        let read = tokio::time::timeout(EXTERNAL_REQUEST_TIMEOUT, stream.read(&mut chunk))
            .await
            .map_err(|_| "HTTP/1 opaque response head timed out".to_string())?
            .map_err(|error| format!("read HTTP/1 opaque response: {error}"))?;
        if read == 0 {
            return Err("HTTP/1 connection closed before the response head".to_string());
        }
        response.extend_from_slice(&chunk[..read]);
    }
}

async fn external_opaque_h2(
    ctx: &TestContext,
    case_id: &str,
    mode: &str,
) -> Result<(u16, Vec<(String, Vec<u8>)>), String> {
    let tcp = tokio::net::TcpStream::connect((ctx.target_host.as_str(), ctx.http_port))
        .await
        .map_err(|error| format!("connect HTTP/2 gateway: {error}"))?;
    let (mut client, connection) = h2::client::handshake(tcp)
        .await
        .map_err(|error| format!("HTTP/2 opaque handshake: {error}"))?;
    let driver = tokio::spawn(connection);
    let uri = format!("http://{EXTERNAL_HOST}/headers")
        .parse::<http::Uri>()
        .map_err(|error| format!("build HTTP/2 opaque URI: {error}"))?;
    let mut request = http::Request::builder()
        .method(http::Method::GET)
        .uri(uri)
        .version(http::Version::HTTP_2)
        .body(())
        .map_err(|error| format!("build HTTP/2 opaque request: {error}"))?;
    for (name, value) in [
        ("x-ext-proc-case-id", case_id.as_bytes()),
        ("x-ext-proc-test-mode", mode.as_bytes()),
        ("x-opaque-presence", b"\xffrequest-first".as_slice()),
        ("x-opaque-presence", b"\x80request-second"),
        ("x-opaque-overwrite", b"\xferequest-old"),
    ] {
        request.headers_mut().append(
            http::HeaderName::from_bytes(name.as_bytes()).map_err(|error| format!("opaque header name: {error}"))?,
            http::HeaderValue::from_bytes(value).map_err(|error| format!("opaque header value: {error}"))?,
        );
    }
    let (response, _) = client
        .send_request(request, true)
        .map_err(|error| format!("send HTTP/2 opaque request: {error}"))?;
    let response = response
        .await
        .map_err(|error| format!("receive HTTP/2 opaque response: {error}"))?;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| (name.as_str().to_string(), value.as_bytes().to_vec()))
        .collect();
    drop(client);
    driver.abort();
    let _ = driver.await;
    Ok((status, headers))
}

fn assert_opaque_downstream_headers(headers: &[(String, Vec<u8>)], immediate: bool) -> Result<(), String> {
    let expected = if immediate {
        vec![b"\x84immediate-first".to_vec(), b"\x85immediate-second".to_vec()]
    } else {
        vec![b"\xfdresponse-first".to_vec(), b"\x80response-second".to_vec()]
    };
    let name = if immediate {
        "x-opaque-immediate"
    } else {
        "x-opaque-presence"
    };
    if raw_header_values(headers, name) != expected {
        return Err(format!("{name} values were not byte-exact: {headers:?}"));
    }
    if !immediate
        && (raw_header_values(headers, "x-opaque-overwrite") != [b"\x81response-overwrite".to_vec()]
            || raw_header_values(headers, "x-opaque-added")
                != [b"\x82response-add".to_vec(), b"\x83response-append".to_vec()]
            || raw_header_values(headers, "x-opaque-empty") != [Vec::<u8>::new()])
    {
        return Err(format!("ordinary opaque mutations were not byte-exact: {headers:?}"));
    }
    if headers
        .iter()
        .any(|(_, value)| value.windows(3).any(|window| window == [0xef, 0xbf, 0xbd]))
    {
        return Err("downstream headers contained a UTF-8 replacement character".to_string());
    }
    Ok(())
}

fn assert_external_streamed_observation(
    stream: &ExternalStreamSnapshot,
    original: &[u8],
    response_before_mutation: &[u8],
) -> Result<(), String> {
    assert_exact_phases(
        stream,
        &["request_headers", "request_body", "response_headers", "response_body"],
    )?;
    assert_plain_transport(stream)?;
    assert_header_observations(stream, 1, &[200])?;
    if stream.request_body_messages < 1
        || stream.request_body_bytes != original.len()
        || stream.request_body_sha256 != sha256_hex(original)
        || stream.request_eos_count != 1
        || stream.response_body_messages < 1
        || stream.response_body_bytes != response_before_mutation.len()
        || stream.response_body_sha256 != sha256_hex(response_before_mutation)
        || stream.response_eos_count != 1
    {
        return Err(format!("unexpected STREAMED body observation: {stream:?}"));
    }
    Ok(())
}

fn expected_echo_before_response_mutation(request_body_messages: usize) -> Vec<u8> {
    let mut expected = b"Server: 0.0.0.0:30001\nEcho: ".to_vec();
    for _ in 0..request_body_messages {
        expected.extend_from_slice(b"processed-by-external-go");
    }
    expected
}

fn expected_streamed_client_body(response_body_messages: usize) -> Vec<u8> {
    b"response-by-external-go".repeat(response_body_messages)
}

async fn run_seeded_external_chunking(ctx: &TestContext) -> Result<String, String> {
    let admin_url = external_admin_url(&ctx.http_client).await?;
    for (index, seed) in SEEDED_CHUNK_SEEDS.iter().copied().enumerate() {
        let case_id = format!("seeded-{index}-{seed:016x}");
        reset_external_case(&ctx.http_client, &admin_url, &case_id).await?;
        let plan = seeded_chunk_plan(seed, SEEDED_BODY_BYTES)?;
        if plan.is_empty()
            || plan.len() > SEEDED_MAX_PLAN_CHUNKS
            || plan.iter().any(|chunk| *chunk == 0 || *chunk > SEEDED_MAX_CHUNK_BYTES)
            || plan.iter().sum::<usize>() != SEEDED_BODY_BYTES
        {
            return Err(format!(
                "seed {seed:#x} produced an invalid bounded chunk plan: {plan:?}"
            ));
        }
        let plan_diagnostic = plan.clone();
        let payload = Arc::new(seeded_payload(seed, SEEDED_BODY_BYTES));
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(4);
        let producer_payload = Arc::clone(&payload);
        let producer = tokio::spawn(async move {
            let mut offset = 0usize;
            for chunk_len in plan {
                let next = offset + chunk_len;
                let chunk = bytes::Bytes::copy_from_slice(&producer_payload[offset..next]);
                tx.send(Ok(chunk))
                    .await
                    .map_err(|_| "seeded body receiver closed".to_string())?;
                offset = next;
            }
            Ok::<(), String>(())
        });
        let body = reqwest::Body::wrap_stream(tokio_stream::wrappers::ReceiverStream::new(rx));
        let response_result = ctx
            .http_client
            .post(format!("{}/echo", ctx.http_url()))
            .header("host", EXTERNAL_STRESS_STREAMED_HOST)
            .header("x-ext-proc-case-id", &case_id)
            .header("x-ext-proc-test-mode", "interop")
            .timeout(EXTERNAL_STRESS_REQUEST_TIMEOUT)
            .body(body)
            .send()
            .await;
        let producer_result = tokio::time::timeout(Duration::from_secs(5), producer)
            .await
            .map_err(|_| format!("seeded producer {case_id} timed out: seed={seed:#x} plan={plan_diagnostic:?}"))?
            .map_err(|error| {
                format!("seeded producer {case_id} task failed: {error}; seed={seed:#x} plan={plan_diagnostic:?}")
            })?;
        producer_result.map_err(|error| {
            format!("seeded producer {case_id} failed: {error}; seed={seed:#x} plan={plan_diagnostic:?}")
        })?;
        let response = response_result.map_err(|error| {
            format!("seeded request {case_id} failed: {error}; seed={seed:#x} plan={plan_diagnostic:?}")
        })?;
        let status = response.status().as_u16();
        let response_header = response
            .headers()
            .get("x-ext-proc-resp")
            .and_then(|value| value.to_str().ok())
            == Some("external-go");
        let body = tokio::time::timeout(EXTERNAL_ADMIN_REQUEST_TIMEOUT, response.bytes())
            .await
            .map_err(|_| format!("seeded response body {case_id} timed out: seed={seed:#x} plan={plan_diagnostic:?}"))?
            .map_err(|error| {
                format!("seeded response body {case_id} failed: {error}; seed={seed:#x} plan={plan_diagnostic:?}")
            })?;
        if status != 200 || !response_header {
            return Err(format!(
                "seeded request {case_id} response mismatch: status={status} header={response_header}; seed={seed:#x} plan={plan_diagnostic:?}"
            ));
        }
        let snapshot = poll_external_snapshot(&ctx.http_client, &admin_url, &case_id)
            .await
            .map_err(|error| {
                format!("seeded observation {case_id} failed: {error}; seed={seed:#x} plan={plan_diagnostic:?}")
            })?;
        let stream = assert_external_stream(&snapshot, &case_id, true).map_err(|error| {
            format!("seeded observation {case_id} failed: {error}; seed={seed:#x} plan={plan_diagnostic:?}")
        })?;
        let response_before_mutation = expected_echo_before_response_mutation(stream.request_body_messages);
        assert_external_streamed_observation(stream, &payload, &response_before_mutation).map_err(|error| {
            format!("seeded observation {case_id} failed: {error}; seed={seed:#x} plan={plan_diagnostic:?}")
        })?;
        let expected_body = expected_streamed_client_body(stream.response_body_messages);
        if body.as_ref() != expected_body {
            return Err(format!(
                "seeded request {case_id} response replacement mismatch: got {} bytes, expected {}; seed={seed:#x} plan={plan_diagnostic:?}",
                body.len(),
                expected_body.len()
            ));
        }
    }
    Ok(format!(
        "{} pinned chunk seeds preserved reconstructed bytes and exactly-once EOS",
        SEEDED_CHUNK_SEEDS.len()
    ))
}

async fn run_external_backpressure(ctx: &TestContext) -> Result<String, String> {
    const BULK_BYTES: usize = 16 * 1024 * 1024;
    let admin_url = external_admin_url(&ctx.http_client).await?;
    let case_id = "stress-body-backpressure";
    let barrier_id = "stress-body-backpressure";
    reset_external_case(&ctx.http_client, &admin_url, case_id).await?;
    reset_external_barrier(&ctx.http_client, &admin_url, barrier_id).await?;

    let (body_tx, body_rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(1);
    let (start_bulk_tx, start_bulk_rx) = tokio::sync::oneshot::channel::<()>();
    let (attempting_tx, attempting_rx) = tokio::sync::oneshot::channel::<()>();
    let (final_send_tx, mut final_send_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
    let producer = tokio::spawn(async move {
        body_tx
            .send(Ok(bytes::Bytes::from_static(b"a")))
            .await
            .map_err(|_| "backpressure receiver closed before the first chunk".to_string())?;
        start_bulk_rx
            .await
            .map_err(|_| "backpressure bulk-start signal was dropped".to_string())?;
        // Hyper may take ownership of the large chunk while it is still
        // blocked writing it. Queue one more byte after that handoff to fill
        // the sole mpsc slot, then observe the following concrete send.
        body_tx
            .send(Ok(bytes::Bytes::from(vec![b'b'; BULK_BYTES])))
            .await
            .map_err(|_| "backpressure receiver closed before the bulk chunk".to_string())?;
        body_tx
            .send(Ok(bytes::Bytes::from_static(b"c")))
            .await
            .map_err(|_| "backpressure receiver closed before filling the mpsc slot".to_string())?;
        let final_send = body_tx.send(Ok(bytes::Bytes::from_static(b"d")));
        tokio::pin!(final_send);
        let mut attempting_tx = Some(attempting_tx);
        let final_result = std::future::poll_fn(|cx| {
            if let Some(attempting_tx) = attempting_tx.take() {
                let _ = attempting_tx.send(());
            }
            std::future::Future::poll(final_send.as_mut(), cx)
        })
        .await
        .map_err(|_| "backpressure receiver closed before the final chunk".to_string());
        let _ = final_send_tx.send(final_result.clone());
        final_result
    });
    let request_client = ctx.http_client.clone();
    let request_url = ctx.http_url();
    let request_task = tokio::spawn(async move {
        let body = reqwest::Body::wrap_stream(tokio_stream::wrappers::ReceiverStream::new(body_rx));
        let response = request_client
            .post(format!("{request_url}/echo"))
            .header("host", EXTERNAL_STRESS_STREAMED_HOST)
            .header("x-ext-proc-case-id", case_id)
            .header("x-ext-proc-test-mode", "barrier-request-body")
            .header("x-ext-proc-barrier-id", barrier_id)
            .timeout(EXTERNAL_STRESS_REQUEST_TIMEOUT)
            .body(body)
            .send()
            .await
            .map_err(|error| format!("backpressure request failed: {error}"))?;
        let status = response.status().as_u16();
        let response_header = response
            .headers()
            .get("x-ext-proc-resp")
            .and_then(|value| value.to_str().ok())
            == Some("external-go");
        let body = response
            .bytes()
            .await
            .map_err(|error| format!("backpressure response body failed: {error}"))?;
        Ok(ExternalHttpResult {
            status,
            response_header,
            body,
        })
    });

    let exercise = async {
        let barrier = poll_external_barrier(&ctx.http_client, &admin_url, barrier_id, 1).await?;
        if barrier.arrivals != 1 || barrier.releases != 0 || barrier.released_all {
            return Err(format!("unexpected body barrier before release: {barrier:?}"));
        }
        start_bulk_tx
            .send(())
            .map_err(|_| "backpressure producer ended before bulk start".to_string())?;
        tokio::time::timeout(Duration::from_secs(2), attempting_rx)
            .await
            .map_err(|_| "producer never attempted the send beyond mpsc capacity".to_string())?
            .map_err(|_| "producer dropped its concrete-send signal".to_string())?;
        match tokio::time::timeout(Duration::from_millis(300), &mut final_send_rx).await {
            Err(_) => Ok(()),
            Ok(result) => Err(format!(
                "concrete producer send completed while processor barrier was still held: {result:?}"
            )),
        }
    }
    .await;

    // Cleanup is unconditional: even an assertion failure opens the barrier,
    // then both spawned tasks are joined (or aborted and joined) under bounds.
    let release_result = release_external_barrier_all(&ctx.http_client, &admin_url, barrier_id).await;
    let request_results =
        join_external_request_tasks(vec![(case_id.to_string(), request_task)], Duration::from_secs(12)).await;
    let mut producer = producer;
    let producer_result = match tokio::time::timeout(Duration::from_secs(12), &mut producer).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => Err(format!("backpressure producer task failed: {error}")),
        Err(_) => {
            producer.abort();
            let _ = tokio::time::timeout(Duration::from_secs(1), producer).await;
            Err("backpressure producer cleanup timed out".to_string())
        }
    };
    release_result?;
    exercise?;
    producer_result?;
    let mut request_results = request_results?;
    let (_, result) = request_results
        .pop()
        .ok_or_else(|| "backpressure request result was missing".to_string())?;
    if result.status != 200 || !result.response_header {
        return Err(format!(
            "backpressure response mismatch: status={} header={}",
            result.status, result.response_header
        ));
    }
    let snapshot = poll_external_snapshot(&ctx.http_client, &admin_url, case_id).await?;
    let stream = assert_external_stream(&snapshot, case_id, true)?;
    let mut original_hash = Sha256::new();
    original_hash.update(b"a");
    original_hash.update(vec![b'b'; BULK_BYTES]);
    original_hash.update(b"c");
    original_hash.update(b"d");
    let response_before_mutation = expected_echo_before_response_mutation(stream.request_body_messages);
    assert_exact_phases(
        stream,
        &["request_headers", "request_body", "response_headers", "response_body"],
    )?;
    if stream.request_body_messages < 1
        || stream.request_body_bytes != BULK_BYTES + 3
        || stream.request_body_sha256 != hex::encode(original_hash.finalize())
        || stream.request_eos_count != 1
        || stream.response_body_messages < 1
        || stream.response_body_bytes != response_before_mutation.len()
        || stream.response_body_sha256 != sha256_hex(&response_before_mutation)
        || stream.response_eos_count != 1
        || result.body.as_ref() != expected_streamed_client_body(stream.response_body_messages)
    {
        return Err(format!("unexpected backpressure terminal observation: {stream:?}"));
    }
    Ok("a concrete producer send stayed pending until the request-body verdict was released".to_string())
}

fn validate_header_result(case_id: &str, result: &ExternalHttpResult) -> Result<(), String> {
    if result.status == 200 && result.response_header {
        Ok(())
    } else {
        Err(format!(
            "header holder {case_id} response mismatch: status={} header={} body={}",
            result.status,
            result.response_header,
            String::from_utf8_lossy(&result.body[..result.body.len().min(200)])
        ))
    }
}

async fn assert_terminal_header_case(client: &reqwest::Client, admin_url: &str, case_id: &str) -> Result<(), String> {
    let snapshot = poll_external_snapshot(client, admin_url, case_id).await?;
    let stream = assert_external_stream(&snapshot, case_id, true)?;
    assert_exact_phases(stream, &["request_headers", "response_headers"])?;
    assert_plain_transport(stream)?;
    assert_header_observations(stream, 1, &[200])?;
    assert_no_body_observations(stream)
}

async fn run_external_concurrency_saturation(ctx: &TestContext) -> Result<String, String> {
    let admin_url = external_admin_url(&ctx.http_client).await?;
    let barrier_id = "stress-session-capacity";
    reset_external_barrier(&ctx.http_client, &admin_url, barrier_id).await?;
    let gateway_url = ctx.http_url();
    let mut tasks = Vec::new();
    let mut holder_ids = Vec::new();

    // Keep cancellation inside this boundary so every timeout still reaches
    // the unconditional release/join cleanup below.
    let exercise = tokio::time::timeout(Duration::from_secs(20), async {
        for index in 0..EXTERNAL_GATEWAY_SESSION_CAPACITY {
            let case_id = format!("sat-holder-{index:02}");
            reset_external_case(&ctx.http_client, &admin_url, &case_id).await?;
            let task = tokio::spawn(send_external_header_request(
                ctx.http_client.clone(),
                gateway_url.clone(),
                case_id.clone(),
                "barrier-headers",
                Some(barrier_id.to_string()),
            ));
            holder_ids.push(case_id.clone());
            tasks.push((case_id, task));
            let barrier = poll_external_barrier(&ctx.http_client, &admin_url, barrier_id, index + 1).await?;
            if barrier.arrivals != (index + 1) as u64 || barrier.releases != 0 || barrier.released_all {
                return Err(format!("unexpected saturation barrier state: {barrier:?}"));
            }
        }

        let rejected_id = "sat-request-65";
        reset_external_case(&ctx.http_client, &admin_url, rejected_id).await?;
        let rejected = send_external_header_request(
            ctx.http_client.clone(),
            gateway_url.clone(),
            rejected_id.to_string(),
            "barrier-headers",
            Some(barrier_id.to_string()),
        )
        .await?;
        if rejected.status != 500 || !is_sanitized_fail_close_body(&String::from_utf8_lossy(&rejected.body)) {
            return Err(format!(
                "request 65 did not fail closed cleanly: status={} body={}",
                rejected.status,
                String::from_utf8_lossy(&rejected.body)
            ));
        }
        assert_external_case_absent(&ctx.http_client, &admin_url, rejected_id).await?;

        release_external_barrier_one(&ctx.http_client, &admin_url, barrier_id).await?;
        let (first_id, first_task) = tasks.remove(0);
        let first = join_external_request_tasks(vec![(first_id.clone(), first_task)], Duration::from_secs(5)).await?;
        let (_, first_result) = first
            .into_iter()
            .next()
            .ok_or_else(|| "released holder result was missing".to_string())?;
        validate_header_result(&first_id, &first_result)?;
        assert_terminal_header_case(&ctx.http_client, &admin_url, &first_id).await?;
        let after_release = poll_external_barrier(
            &ctx.http_client,
            &admin_url,
            barrier_id,
            EXTERNAL_GATEWAY_SESSION_CAPACITY - 1,
        )
        .await?;
        if after_release.releases != 1 || after_release.arrivals != EXTERNAL_GATEWAY_SESSION_CAPACITY as u64 {
            return Err(format!("unexpected release-one barrier state: {after_release:?}"));
        }

        let replacement_id = "sat-holder-replacement".to_string();
        reset_external_case(&ctx.http_client, &admin_url, &replacement_id).await?;
        let replacement = tokio::spawn(send_external_header_request(
            ctx.http_client.clone(),
            gateway_url.clone(),
            replacement_id.clone(),
            "barrier-headers",
            Some(barrier_id.to_string()),
        ));
        holder_ids.push(replacement_id.clone());
        tasks.push((replacement_id, replacement));
        let replacement_arrival = poll_external_barrier(
            &ctx.http_client,
            &admin_url,
            barrier_id,
            EXTERNAL_GATEWAY_SESSION_CAPACITY,
        )
        .await?;
        if replacement_arrival.arrivals != (EXTERNAL_GATEWAY_SESSION_CAPACITY + 1) as u64
            || replacement_arrival.releases != 1
        {
            return Err(format!("replacement arrival was not observed: {replacement_arrival:?}"));
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|_| "concurrency saturation setup/exercise timed out after 20s".to_string())
    .and_then(|result| result);

    // Always release all admitted holders and boundedly join every remaining
    // task, including failures during incremental setup.
    let release_result = release_external_barrier_all(&ctx.http_client, &admin_url, barrier_id).await;
    let joined = join_external_request_tasks(tasks, Duration::from_secs(12)).await;
    release_result?;
    exercise?;
    let joined = joined?;
    for (case_id, result) in joined {
        validate_header_result(&case_id, &result)?;
    }
    for case_id in &holder_ids {
        assert_terminal_header_case(&ctx.http_client, &admin_url, case_id).await?;
    }

    let probe_id = "sat-ordinary-probe";
    reset_external_case(&ctx.http_client, &admin_url, probe_id).await?;
    let probe = send_external_header_request(
        ctx.http_client.clone(),
        gateway_url,
        probe_id.to_string(),
        "interop",
        None,
    )
    .await?;
    validate_header_result(probe_id, &probe)?;
    assert_terminal_header_case(&ctx.http_client, &admin_url, probe_id).await?;
    Ok(
        "64 holders saturated the Gateway, request 65 failed before the fixture, and released capacity was reusable"
            .to_string(),
    )
}

fn validate_external_tls_listener(name: &str, listener: &ExternalTlsListenerSnapshot) -> Result<(), String> {
    let current_matches = match listener.current_generation.as_str() {
        "a" => listener.current_leaf_sha256 == listener.generation_a_leaf_sha256,
        "b" => listener.current_leaf_sha256 == listener.generation_b_leaf_sha256,
        _ => false,
    };
    if listener.enabled
        && listener.ready
        && !listener.current_leaf_sha256.is_empty()
        && !listener.generation_a_leaf_sha256.is_empty()
        && !listener.generation_b_leaf_sha256.is_empty()
        && listener.generation_a_leaf_sha256 != listener.generation_b_leaf_sha256
        && current_matches
        && listener.active_connections <= EXTERNAL_FIXTURE_STREAM_CAPACITY
    {
        Ok(())
    } else {
        Err(format!(
            "external ExtProc {name} listener identity is invalid: {listener:?}"
        ))
    }
}

async fn external_tls_snapshot(client: &reqwest::Client, admin_url: &str) -> Result<ExternalTlsSnapshot, String> {
    let response = client
        .get(format!("{admin_url}/v1/tls"))
        .timeout(EXTERNAL_ADMIN_REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|error| format!("external ExtProc TLS identity request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("external ExtProc TLS identity returned {}", response.status()));
    }
    let snapshot = response
        .json::<ExternalTlsSnapshot>()
        .await
        .map_err(|error| format!("external ExtProc TLS identity was invalid: {error}"))?;
    if snapshot.fixture_stream_capacity != EXTERNAL_FIXTURE_STREAM_CAPACITY
        || snapshot.server_ca_sha256.is_empty()
        || snapshot.client_ca_sha256.is_empty()
        || snapshot.client_a_leaf_sha256.is_empty()
        || snapshot.client_b_leaf_sha256.is_empty()
        || snapshot.client_a_leaf_sha256 == snapshot.client_b_leaf_sha256
    {
        return Err(format!(
            "external ExtProc TLS material identity is invalid: {snapshot:?}"
        ));
    }
    validate_external_tls_listener("tls", &snapshot.tls)?;
    validate_external_tls_listener("mtls", &snapshot.mtls)?;
    Ok(snapshot)
}

async fn rotate_external_tls_listener(
    client: &reqwest::Client,
    admin_url: &str,
    listener: &str,
    generation: &str,
) -> Result<(), String> {
    external_admin_post(
        client,
        format!("{admin_url}/v1/tls/{listener}/rotate/{generation}"),
        &format!("{listener} rotation to generation {generation}"),
    )
    .await
}

#[derive(Clone, Copy)]
struct SecureIdentityExpectation<'a> {
    transport: &'a str,
    generation: &'a str,
    server_leaf: &'a str,
    peer_leaf: Option<&'a str>,
}

async fn observe_external_secure_header_request(
    ctx: &TestContext,
    admin_url: &str,
    host: &str,
    case_id: &str,
    expected: SecureIdentityExpectation<'_>,
) -> Result<(), String> {
    reset_external_case(&ctx.http_client, admin_url, case_id).await?;
    let response = ctx
        .http_client
        .get(format!("{}/headers", ctx.http_url()))
        .header("host", host)
        .header("x-ext-proc-case-id", case_id)
        .header("x-ext-proc-test-mode", "interop")
        .timeout(Duration::from_secs(4))
        .send()
        .await
        .map_err(|error| format!("secure external request {case_id} failed: {error}"))?;
    let status = response.status().as_u16();
    let response_header = response
        .headers()
        .get("x-ext-proc-resp")
        .and_then(|value| value.to_str().ok())
        == Some("external-go");
    let body = tokio::time::timeout(EXTERNAL_ADMIN_REQUEST_TIMEOUT, response.text())
        .await
        .map_err(|_| format!("secure external response {case_id} timed out"))?
        .map_err(|error| format!("secure external response {case_id} failed: {error}"))?;
    let request_header =
        body.to_ascii_lowercase().contains("x-ext-proc-req") && body.to_ascii_lowercase().contains("external-go");
    if status != 200 || !request_header || !response_header {
        return Err(format!(
            "secure external mutation mismatch for {case_id}: status={status} request={request_header} response={response_header}"
        ));
    }

    let snapshot = poll_external_snapshot(&ctx.http_client, admin_url, case_id).await?;
    let stream = assert_external_stream(&snapshot, case_id, true)?;
    assert_exact_phases(stream, &["request_headers", "response_headers"])?;
    assert_header_observations(stream, 1, &[200])?;
    assert_no_body_observations(stream)?;
    let peer_matches = match expected.peer_leaf {
        Some(expected) => stream.peer_leaf_sha256.as_deref() == Some(expected),
        None => stream.peer_leaf_sha256.as_deref().unwrap_or_default().is_empty(),
    };
    if stream.transport != expected.transport
        || stream.tls_generation.as_deref() != Some(expected.generation)
        || stream.server_leaf_sha256.as_deref() != Some(expected.server_leaf)
        || !peer_matches
    {
        return Err(format!(
            "secure external transport identity mismatch for {case_id}: {stream:?}"
        ));
    }
    Ok(())
}

async fn poll_external_secure_identity(
    ctx: &TestContext,
    admin_url: &str,
    host: &str,
    case_prefix: &str,
    expected: SecureIdentityExpectation<'_>,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let mut attempt = 0usize;
    let mut last_error = "no request attempted".to_string();
    while Instant::now() < deadline {
        let case_id = format!("{case_prefix}-{attempt}-{}", uuid::Uuid::new_v4());
        attempt += 1;
        match tokio::time::timeout(
            Duration::from_secs(5),
            observe_external_secure_header_request(ctx, admin_url, host, &case_id, expected),
        )
        .await
        {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(error)) => last_error = error,
            Err(_) => last_error = format!("secure external attempt {case_id} exceeded 5s"),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!(
        "secure external identity did not converge for {case_prefix} after {attempt} attempts: {last_error}"
    ))
}

fn load_external_client_manifests() -> Result<(String, String), String> {
    let work_dir = std::env::var(EDGION_WORK_DIR_ENV)
        .map(PathBuf::from)
        .map_err(|_| format!("{EDGION_WORK_DIR_ENV} is required for mTLS rotation"))?;
    let material_dir = std::env::var(EXTERNAL_TLS_MATERIAL_DIR_ENV)
        .map(PathBuf::from)
        .map_err(|_| format!("{EXTERNAL_TLS_MATERIAL_DIR_ENV} is required for mTLS rotation"))?;
    if !material_dir.is_absolute() {
        return Err(format!("{EXTERNAL_TLS_MATERIAL_DIR_ENV} must be absolute"));
    }
    let canonical_work = std::fs::canonicalize(&work_dir)
        .map_err(|error| format!("failed to canonicalize {EDGION_WORK_DIR_ENV}: {error}"))?;
    let expected = std::fs::canonicalize(canonical_work.join("ext-proc-external-tls"))
        .map_err(|error| format!("failed to canonicalize expected external TLS material directory: {error}"))?;
    let actual = std::fs::canonicalize(&material_dir)
        .map_err(|error| format!("failed to canonicalize {EXTERNAL_TLS_MATERIAL_DIR_ENV}: {error}"))?;
    if actual != expected {
        return Err(format!(
            "{EXTERNAL_TLS_MATERIAL_DIR_ENV} must resolve to the canonical ext-proc-external-tls child of {EDGION_WORK_DIR_ENV}"
        ));
    }
    let initial = std::fs::read_to_string(actual.join("ext-proc-external-client-initial.yaml"))
        .map_err(|error| format!("failed to read initial external mTLS client manifest: {error}"))?;
    let updated = std::fs::read_to_string(actual.join("ext-proc-external-client-updated.yaml"))
        .map_err(|error| format!("failed to read updated external mTLS client manifest: {error}"))?;
    Ok((initial, updated))
}

fn combine_rotation_results(
    exercise: Result<(), String>,
    cleanup: Result<(), String>,
    success: &str,
) -> Result<String, String> {
    match (exercise, cleanup) {
        (Ok(()), Ok(())) => Ok(success.to_string()),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(()), Err(cleanup)) => Err(format!("rotation cleanup failed: {cleanup}")),
        (Err(primary), Err(cleanup)) => Err(format!("{primary}; rotation cleanup also failed: {cleanup}")),
    }
}

async fn run_external_tls_server_rotation(ctx: &TestContext) -> Result<String, String> {
    let admin_url = external_admin_url(&ctx.http_client).await?;
    let exercise = tokio::time::timeout(Duration::from_secs(25), async {
        let identity = external_tls_snapshot(&ctx.http_client, &admin_url).await?;
        rotate_external_tls_listener(&ctx.http_client, &admin_url, "tls", "a").await?;
        observe_external_secure_header_request(
            ctx,
            &admin_url,
            EXTERNAL_TLS_HOST,
            &format!("tls-rotation-a1-{}", uuid::Uuid::new_v4()),
            SecureIdentityExpectation {
                transport: "tls",
                generation: "a",
                server_leaf: &identity.tls.generation_a_leaf_sha256,
                peer_leaf: None,
            },
        )
        .await?;
        rotate_external_tls_listener(&ctx.http_client, &admin_url, "tls", "b").await?;
        observe_external_secure_header_request(
            ctx,
            &admin_url,
            EXTERNAL_TLS_HOST,
            &format!("tls-rotation-b-{}", uuid::Uuid::new_v4()),
            SecureIdentityExpectation {
                transport: "tls",
                generation: "b",
                server_leaf: &identity.tls.generation_b_leaf_sha256,
                peer_leaf: None,
            },
        )
        .await?;
        rotate_external_tls_listener(&ctx.http_client, &admin_url, "tls", "a").await?;
        observe_external_secure_header_request(
            ctx,
            &admin_url,
            EXTERNAL_TLS_HOST,
            &format!("tls-rotation-a2-{}", uuid::Uuid::new_v4()),
            SecureIdentityExpectation {
                transport: "tls",
                generation: "a",
                server_leaf: &identity.tls.generation_a_leaf_sha256,
                peer_leaf: None,
            },
        )
        .await
    })
    .await
    .map_err(|_| "TLS rotation exercise exceeded 25s".to_string())
    .and_then(|result| result);

    let cleanup = tokio::time::timeout(Duration::from_secs(15), async {
        rotate_external_tls_listener(&ctx.http_client, &admin_url, "tls", "a").await?;
        let identity = external_tls_snapshot(&ctx.http_client, &admin_url).await?;
        observe_external_secure_header_request(
            ctx,
            &admin_url,
            EXTERNAL_TLS_HOST,
            &format!("tls-cleanup-a-{}", uuid::Uuid::new_v4()),
            SecureIdentityExpectation {
                transport: "tls",
                generation: "a",
                server_leaf: &identity.tls.generation_a_leaf_sha256,
                peer_leaf: None,
            },
        )
        .await
    })
    .await
    .map_err(|_| "TLS rotation cleanup exceeded 15s".to_string())
    .and_then(|result| result);
    combine_rotation_results(
        exercise,
        cleanup,
        "TLS streams proved server generations A, B, A and cleanup restored A",
    )
}

async fn run_external_mtls_client_rotation(ctx: &TestContext) -> Result<String, String> {
    let admin_url = external_admin_url(&ctx.http_client).await?;
    let exercise = tokio::time::timeout(Duration::from_secs(30), async {
        let (initial, updated) = load_external_client_manifests()?;
        let identity = external_tls_snapshot(&ctx.http_client, &admin_url).await?;
        rotate_external_tls_listener(&ctx.http_client, &admin_url, "mtls", "a").await?;
        ctx.apply_yaml(&initial)
            .await
            .map_err(|error| format!("initial mTLS client apply failed: {error}"))?;
        poll_external_secure_identity(
            ctx,
            &admin_url,
            EXTERNAL_MTLS_HOST,
            "mtls-converge-a",
            SecureIdentityExpectation {
                transport: "mtls",
                generation: "a",
                server_leaf: &identity.mtls.generation_a_leaf_sha256,
                peer_leaf: Some(&identity.client_a_leaf_sha256),
            },
            Duration::from_secs(10),
        )
        .await?;
        ctx.apply_yaml(&updated)
            .await
            .map_err(|error| format!("updated mTLS client apply failed: {error}"))?;
        poll_external_secure_identity(
            ctx,
            &admin_url,
            EXTERNAL_MTLS_HOST,
            "mtls-converge-b",
            SecureIdentityExpectation {
                transport: "mtls",
                generation: "a",
                server_leaf: &identity.mtls.generation_a_leaf_sha256,
                peer_leaf: Some(&identity.client_b_leaf_sha256),
            },
            Duration::from_secs(10),
        )
        .await
    })
    .await
    .map_err(|_| "mTLS client rotation exercise exceeded 30s".to_string())
    .and_then(|result| result);

    let cleanup = tokio::time::timeout(Duration::from_secs(20), async {
        let (initial, _) = load_external_client_manifests()?;
        rotate_external_tls_listener(&ctx.http_client, &admin_url, "mtls", "a").await?;
        ctx.apply_yaml(&initial)
            .await
            .map_err(|error| format!("cleanup mTLS client apply failed: {error}"))?;
        let identity = external_tls_snapshot(&ctx.http_client, &admin_url).await?;
        poll_external_secure_identity(
            ctx,
            &admin_url,
            EXTERNAL_MTLS_HOST,
            "mtls-cleanup-a",
            SecureIdentityExpectation {
                transport: "mtls",
                generation: "a",
                server_leaf: &identity.mtls.generation_a_leaf_sha256,
                peer_leaf: Some(&identity.client_a_leaf_sha256),
            },
            Duration::from_secs(12),
        )
        .await
    })
    .await
    .map_err(|_| "mTLS client rotation cleanup exceeded 20s".to_string())
    .and_then(|result| result);
    combine_rotation_results(
        exercise,
        cleanup,
        "mTLS streams proved client certificate A to B without restart and cleanup restored A",
    )
}

fn is_sanitized_fail_close_body(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    let generic =
        body.is_empty() || lower.contains("plugin configuration error") || lower.contains("internal server error");
    generic
        && !["ext_proc", "grpc", "unavailable", "127.0.0.1", "target", "processor"]
            .iter()
            .any(|detail| lower.contains(detail))
}

async fn observe_sanitized_fail_close_response(response: reqwest::Response) -> Result<(), &'static str> {
    if response.status().as_u16() != 500 {
        return Err("Expected sanitized fail-closed HTTP 500");
    }
    let body = response
        .text()
        .await
        .map_err(|_| "ExtProc fail-closed response body read failed")?;
    if !is_sanitized_fail_close_body(&body) {
        return Err("Fail-closed response was not generic or exposed internal detail");
    }
    Ok(())
}

async fn observe_fail_open_response(response: reqwest::Response) -> Result<(), &'static str> {
    if response.status().as_u16() != 200 {
        return Err("Expected HTTP 200 for the ExtProc fail-open echo observation");
    }
    let body = response
        .text()
        .await
        .map_err(|_| "ExtProc fail-open response body read failed")?;
    let expected_backend = body.lines().any(|line| {
        line.strip_prefix("Server: ")
            .and_then(|address| address.rsplit_once(':'))
            .and_then(|(_, port)| port.trim().parse::<u16>().ok())
            == Some(30001)
    });
    if !expected_backend
        || !["Method: GET", "Path: /get", "Headers:"]
            .iter()
            .all(|expected| body.lines().any(|line| line == *expected))
    {
        return Err("ExtProc fail-open response did not identify the expected backend echo");
    }
    if body.to_lowercase().contains("x-ext-proc-req") {
        return Err("fail-open but processor header present (unexpected)");
    }
    Ok(())
}

pub struct ExtProcTestSuite {
    full_test: bool,
}

/// Shared assertion for the over-TLS scenarios: the processor adds a request
/// header (echoed by the backend `/headers`) and a response header (visible to
/// the client). Both phases dial the processor over TLS, so success proves the
/// rustls ext_proc connector completed the TLS+h2 handshake on request AND
/// upstream-response stages.
async fn assert_headers_applied_over_tls(ctx: TestContext, host: &str) -> TestResult {
    let start = Instant::now();
    let url = format!("{}/headers", ctx.http_url());
    let response = match ctx.http_client.get(&url).header("host", host).send().await {
        Ok(r) => r,
        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
    };

    let status = response.status().as_u16();
    if status != 200 {
        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
    }

    // Response header added by the response phase over TLS.
    let resp_header_ok = response
        .headers()
        .get("x-ext-proc-resp")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == "seen")
        .unwrap_or(false);

    // Request header added by the request phase over TLS, echoed by /headers.
    let body = response.text().await.unwrap_or_default();
    let body_lower = body.to_lowercase();
    let req_header_ok = body_lower.contains("x-ext-proc-req") && body_lower.contains("seen");

    if req_header_ok && resp_header_ok {
        TestResult::passed_with_message(
            start.elapsed(),
            "request and response headers both applied over TLS".to_string(),
        )
    } else {
        TestResult::failed(
            start.elapsed(),
            format!(
                "req_header_ok={} resp_header_ok={}; body: {}",
                req_header_ok,
                resp_header_ok,
                &body[..body.len().min(400)]
            ),
        )
    }
}

impl ExtProcTestSuite {
    pub fn new(full_test: bool) -> Self {
        Self { full_test }
    }

    fn test_external_seeded_chunk_boundaries() -> TestCase {
        TestCase::new(
            "external_seeded_chunk_boundaries",
            "pinned xorshift64* chunk plans preserve reconstructed STREAMED bytes and EOS",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match run_seeded_external_chunking(&ctx).await {
                        Ok(message) => TestResult::passed_with_message(start.elapsed(), message),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(60))
    }

    fn test_external_request_body_backpressure() -> TestCase {
        TestCase::new(
            "external_request_body_backpressure",
            "a barrier-held request-body verdict keeps a concrete 16 MiB producer send pending",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match run_external_backpressure(&ctx).await {
                        Ok(message) => TestResult::passed_with_message(start.elapsed(), message),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(60))
    }

    fn test_external_concurrency_saturation_and_release() -> TestCase {
        TestCase::new(
            "external_concurrency_saturation_and_release",
            "64 held sessions reject request 65, then one exact release admits a replacement without leaks",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match run_external_concurrency_saturation(&ctx).await {
                        Ok(message) => TestResult::passed_with_message(start.elapsed(), message),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(75))
    }

    fn test_external_tls_server_certificate_rotation() -> TestCase {
        TestCase::new(
            "external_tls_server_certificate_rotation",
            "stream-bound fingerprints prove TLS server certificate rotation A to B to A with verified cleanup",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match run_external_tls_server_rotation(&ctx).await {
                        Ok(message) => TestResult::passed_with_message(start.elapsed(), message),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(60))
    }

    fn test_external_mtls_client_certificate_rotation() -> TestCase {
        TestCase::new(
            "external_mtls_client_certificate_rotation",
            "real mTLS streams prove hot client certificate convergence A to B and verified cleanup to A",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match run_external_mtls_client_rotation(&ctx).await {
                        Ok(message) => TestResult::passed_with_message(start.elapsed(), message),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(75))
    }

    /// Basic: the processor adds a request header (echoed by the backend) and a
    /// response header (visible to the client).
    fn test_basic_request_and_response_headers() -> TestCase {
        TestCase::new(
            "basic_request_and_response_headers",
            "ext_proc adds a request header (seen by backend) and a response header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/headers", ctx.http_url());
                    let response = match ctx.http_client.get(&url).header("host", BASIC_HOST).send().await {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    let status = response.status().as_u16();
                    if status != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                    }

                    // Response header added by the response phase.
                    let resp_header_ok = response
                        .headers()
                        .get("x-ext-proc-resp")
                        .and_then(|v| v.to_str().ok())
                        .map(|v| v == "seen")
                        .unwrap_or(false);

                    // Request header added by the request phase, echoed by /headers.
                    let body = response.text().await.unwrap_or_default();
                    let body_lower = body.to_lowercase();
                    let req_header_ok = body_lower.contains("x-ext-proc-req") && body_lower.contains("seen");

                    if req_header_ok && resp_header_ok {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "request and response headers both applied".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "req_header_ok={} resp_header_ok={}; body: {}",
                                req_header_ok,
                                resp_header_ok,
                                &body[..body.len().min(400)]
                            ),
                        )
                    }
                })
            },
        )
    }

    /// ExtProc over TLS (Scenario A): caSource SecretRef + verify. The processor
    /// is reached over an HTTPS/h2 stream (rustls connector trusting the private
    /// CA); both request and response header mutations must apply over TLS.
    fn test_tls_request_and_response_headers() -> TestCase {
        TestCase::new(
            "tls_request_and_response_headers",
            "ext_proc over TLS (caSource SecretRef + verify) applies request and response headers",
            |ctx: TestContext| Box::pin(async move { assert_headers_applied_over_tls(ctx, TLS_HOST).await }),
        )
    }

    /// ExtProc over TLS (Scenario B): verify=false against the self-signed
    /// processor cert. The stream still establishes (AcceptAll verifier, no env
    /// gate after M-A1) and mutates headers.
    fn test_tls_noverify_request_and_response_headers() -> TestCase {
        TestCase::new(
            "tls_noverify_request_and_response_headers",
            "ext_proc over TLS (verify=false) still establishes the stream and applies headers",
            |ctx: TestContext| Box::pin(async move { assert_headers_applied_over_tls(ctx, TLS_NOVERIFY_HOST).await }),
        )
    }

    /// ExtProc over mTLS: the server requires a client identity signed by the
    /// integration client CA, while the gateway also verifies the server CA.
    fn test_mtls_request_and_response_headers() -> TestCase {
        TestCase::new(
            "mtls_request_and_response_headers",
            "ext_proc over mTLS presents a client identity and applies request and response headers",
            |ctx: TestContext| Box::pin(async move { assert_headers_applied_over_tls(ctx, MTLS_HOST).await }),
        )
    }

    /// A clean processor half-close without a request-header verdict is a bad
    /// dependency outcome and must follow fail-close.
    fn test_processor_closes_before_request_verdict() -> TestCase {
        TestCase::new(
            "processor_closes_before_request_verdict",
            "a processor clean-close before the request verdict fails closed",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/headers", ctx.http_url());
                    let Ok(response) = ctx
                        .http_client
                        .get(&url)
                        .header("host", BASIC_HOST)
                        .header("x-ext-proc-test-mode", "close-before-verdict")
                        .send()
                        .await
                    else {
                        return TestResult::failed(start.elapsed(), "ExtProc observation request failed".to_string());
                    };
                    match observe_sanitized_fail_close_response(response).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "clean-close before request verdict failed closed".to_string(),
                        ),
                        Err(message) => TestResult::failed(start.elapsed(), message.to_string()),
                    }
                })
            },
        )
    }

    /// The attached transaction also fails closed if the processor accepts the
    /// request phase but closes before the response-header verdict.
    fn test_processor_closes_before_response_verdict() -> TestCase {
        TestCase::new(
            "processor_closes_before_response_verdict",
            "a processor clean-close before the response verdict fails closed",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/headers", ctx.http_url());
                    let Ok(response) = ctx
                        .http_client
                        .get(&url)
                        .header("host", BASIC_HOST)
                        .header("x-ext-proc-test-mode", "close-on-response-headers")
                        .send()
                        .await
                    else {
                        return TestResult::failed(start.elapsed(), "ExtProc observation request failed".to_string());
                    };
                    match observe_sanitized_fail_close_response(response).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "clean-close before response verdict failed closed".to_string(),
                        ),
                        Err(message) => TestResult::failed(start.elapsed(), message.to_string()),
                    }
                })
            },
        )
    }

    fn test_external_headers_and_terminal_half_close() -> TestCase {
        TestCase::new(
            "external_headers_and_terminal_half_close",
            "the independent Go processor mutates request/response headers and observes terminal half-close",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let case_id = "external-headers-half-close";
                    let admin_url = match prepare_external_case(&ctx.http_client, case_id).await {
                        Ok(url) => url,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let url = format!("{}/headers", ctx.http_url());
                    let response = match ctx
                        .http_client
                        .get(&url)
                        .header("host", EXTERNAL_HOST)
                        .header("x-ext-proc-case-id", case_id)
                        .header("x-ext-proc-test-mode", "interop")
                        .timeout(EXTERNAL_REQUEST_TIMEOUT)
                        .send()
                        .await
                    {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    };
                    let status = response.status().as_u16();
                    let response_header = response
                        .headers()
                        .get("x-ext-proc-resp")
                        .and_then(|value| value.to_str().ok())
                        == Some("external-go");
                    let body = match tokio::time::timeout(EXTERNAL_ADMIN_REQUEST_TIMEOUT, response.text()).await {
                        Ok(Ok(body)) => body,
                        Ok(Err(error)) => {
                            return TestResult::failed(start.elapsed(), format!("Response body failed: {error}"));
                        }
                        Err(_) => return TestResult::failed(start.elapsed(), "Response body timed out".to_string()),
                    };
                    let request_header = body.to_ascii_lowercase().contains("x-ext-proc-req")
                        && body.to_ascii_lowercase().contains("external-go");
                    if status != 200 || !response_header || !request_header {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "external header mutation mismatch: status={status} request={request_header} response={response_header}"
                            ),
                        );
                    }
                    let snapshot = match poll_external_snapshot(&ctx.http_client, &admin_url, case_id).await {
                        Ok(snapshot) => snapshot,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let stream = match assert_external_stream(&snapshot, case_id, true) {
                        Ok(stream) => stream,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if let Err(error) = assert_phase_groups(stream, &["request_headers", "response_headers"])
                        .and_then(|_| assert_header_observations(stream, 1, &[200]))
                        .and_then(|_| assert_no_body_observations(stream))
                    {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "independent header mutations and terminal half-close observed".to_string(),
                    )
                })
            },
        )
    }

    fn test_external_opaque_header_wire_fidelity() -> TestCase {
        TestCase::new(
            "external_opaque_header_wire_fidelity",
            "HTTP/1 and HTTP/2 preserve opaque repeated headers through an independent ExtProc exchange",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    for (protocol, h2, mode, immediate) in [
                        ("h1", false, "opaque-headers", false),
                        ("h2", true, "opaque-headers", false),
                        ("h1", false, "opaque-immediate", true),
                        ("h2", true, "opaque-immediate", true),
                    ] {
                        let case_id = format!(
                            "external-opaque-{protocol}-{}",
                            if immediate { "immediate" } else { "headers" }
                        );
                        let admin_url = match prepare_external_case(&ctx.http_client, &case_id).await {
                            Ok(url) => url,
                            Err(error) => return TestResult::failed(start.elapsed(), error),
                        };
                        let result = if h2 {
                            external_opaque_h2(&ctx, &case_id, mode).await
                        } else {
                            external_opaque_h1(&ctx, &case_id, mode).await
                        };
                        let (status, headers) = match result {
                            Ok(result) => result,
                            Err(error) => return TestResult::failed(start.elapsed(), error),
                        };
                        let expected_status = if immediate { 403 } else { 200 };
                        if status != expected_status {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("{protocol} {mode} status = {status}, want {expected_status}"),
                            );
                        }
                        if let Err(error) = assert_opaque_downstream_headers(&headers, immediate) {
                            return TestResult::failed(start.elapsed(), format!("{protocol} {mode}: {error}"));
                        }

                        let snapshot = match poll_external_snapshot(&ctx.http_client, &admin_url, &case_id).await {
                            Ok(snapshot) => snapshot,
                            Err(error) => return TestResult::failed(start.elapsed(), error),
                        };
                        let stream = match assert_external_stream(&snapshot, &case_id, !immediate) {
                            Ok(stream) => stream,
                            Err(error) => return TestResult::failed(start.elapsed(), error),
                        };
                        let expected_phases: &[&str] = if immediate {
                            &["request_headers"]
                        } else {
                            &["request_headers", "response_headers"]
                        };
                        if let Err(error) = assert_exact_phases(stream, expected_phases) {
                            return TestResult::failed(start.elapsed(), error);
                        }
                        if !stream.request_opaque_headers_ok
                            || stream.response_opaque_headers_ok == immediate
                            || (immediate && (!stream.cancelled || stream.client_half_closed))
                        {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("{protocol} {mode} opaque observation mismatch: {stream:?}"),
                            );
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "HTTP/1 and HTTP/2 preserved request, response, and ImmediateResponse opaque header bytes"
                            .to_string(),
                    )
                })
            },
        )
    }

    fn test_external_buffered_request_body() -> TestCase {
        TestCase::new(
            "external_buffered_request_body",
            "the independent Go processor decodes and replaces one BUFFERED request body",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let case_id = "external-buffered-request";
                    let original = b"external-buffered-original";
                    let admin_url = match prepare_external_case(&ctx.http_client, case_id).await {
                        Ok(url) => url,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let response = match ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", EXTERNAL_BUFFERED_HOST)
                        .header("x-ext-proc-case-id", case_id)
                        .header("x-ext-proc-test-mode", "interop")
                        .timeout(EXTERNAL_REQUEST_TIMEOUT)
                        .body(original.to_vec())
                        .send()
                        .await
                    {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    };
                    let status = response.status().as_u16();
                    let response_header = response
                        .headers()
                        .get("x-ext-proc-resp")
                        .and_then(|value| value.to_str().ok())
                        == Some("external-go");
                    let body = match tokio::time::timeout(EXTERNAL_ADMIN_REQUEST_TIMEOUT, response.bytes()).await {
                        Ok(Ok(body)) => body,
                        Ok(Err(error)) => {
                            return TestResult::failed(start.elapsed(), format!("Response body failed: {error}"));
                        }
                        Err(_) => return TestResult::failed(start.elapsed(), "Response body timed out".to_string()),
                    };
                    let expected_body = b"Server: 0.0.0.0:30001\nEcho: processed-by-external-go";
                    if status != 200 || !response_header || body.as_ref() != expected_body {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "buffered mutation mismatch: status={status} header={response_header} body={}",
                                String::from_utf8_lossy(&body)
                            ),
                        );
                    }
                    let snapshot = match poll_external_snapshot(&ctx.http_client, &admin_url, case_id).await {
                        Ok(snapshot) => snapshot,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let stream = match assert_external_stream(&snapshot, case_id, true) {
                        Ok(stream) => stream,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if let Err(error) =
                        assert_phase_groups(stream, &["request_headers", "request_body", "response_headers"])
                    {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    if let Err(error) = assert_header_observations(stream, 1, &[200]) {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    if stream.request_body_messages != 1
                        || stream.request_body_bytes != original.len()
                        || stream.request_body_sha256 != sha256_hex(original)
                        || stream.request_eos_count != 1
                        || stream.response_body_messages != 0
                        || stream.response_body_bytes != 0
                        || stream.response_body_sha256 != sha256_hex(b"")
                        || stream.response_eos_count != 0
                    {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("unexpected BUFFERED body observation: {stream:?}"),
                        );
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "independent BUFFERED request replacement and EOS observed".to_string(),
                    )
                })
            },
        )
    }

    fn test_external_buffered_status_retry_discards_intermediate() -> TestCase {
        TestCase::new(
            "external_buffered_status_retry_discards_intermediate",
            "BUFFERED mutation is replayed while the retryable 503 is hidden from the independent processor",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let case_id = format!("ext-buffer-retry-{}", uuid::Uuid::new_v4());
                    let original = b"external-buffered-retry-original";
                    let processed = b"processed-by-external-go";
                    let admin_url = match prepare_external_case(&ctx.http_client, &case_id).await {
                        Ok(url) => url,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let response = match ctx
                        .http_client
                        .post(format!("{}/retry-sequence/{case_id}/503,200", ctx.http_url()))
                        .header("host", EXTERNAL_BUFFERED_HOST)
                        .header("x-ext-proc-case-id", &case_id)
                        .header("x-ext-proc-test-mode", "interop")
                        .header("x-expected-body-sha256", sha256_hex(processed))
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&case_id),
                        )
                        .header("access_log", "test_store")
                        .timeout(EXTERNAL_REQUEST_TIMEOUT)
                        .body(original.to_vec())
                        .send()
                        .await
                    {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    };
                    let status = response.status().as_u16();
                    let attempt = response
                        .headers()
                        .get("x-attempt-number")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    let history = response
                        .headers()
                        .get("x-attempt-history")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    let response_header = response
                        .headers()
                        .get("x-ext-proc-resp")
                        .and_then(|value| value.to_str().ok())
                        == Some("external-go");
                    let body = match tokio::time::timeout(EXTERNAL_ADMIN_REQUEST_TIMEOUT, response.text()).await {
                        Ok(Ok(body)) => body,
                        Ok(Err(error)) => {
                            return TestResult::failed(start.elapsed(), format!("Response body failed: {error}"));
                        }
                        Err(_) => return TestResult::failed(start.elapsed(), "Response body timed out".to_string()),
                    };
                    if status != 200
                        || attempt != "2"
                        || history != "503,200"
                        || !response_header
                        || body != "attempt=2, status=200"
                    {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "buffered retry response mismatch: status={status} attempt={attempt} history={history} header={response_header} body={body}"
                            ),
                        );
                    }

                    let snapshot = match poll_external_snapshot(&ctx.http_client, &admin_url, &case_id).await {
                        Ok(snapshot) => snapshot,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let stream = match assert_external_stream(&snapshot, &case_id, true) {
                        Ok(stream) => stream,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if let Err(error) =
                        assert_exact_phases(stream, &["request_headers", "request_body", "response_headers"])
                    {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    if let Err(error) = assert_header_observations(stream, 1, &[200]) {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    if stream.request_body_messages != 1
                        || stream.request_body_bytes != original.len()
                        || stream.request_body_sha256 != sha256_hex(original)
                        || stream.request_eos_count != 1
                        || stream.response_body_messages != 0
                        || stream.response_body_bytes != 0
                        || stream.response_body_sha256 != sha256_hex(b"")
                        || stream.response_eos_count != 0
                    {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("unexpected BUFFERED retry observation: {stream:?}"),
                        );
                    }

                    let access_log = match ctx
                        .access_log_client()
                        .get_access_log_with_retry(&case_id, 10, 200)
                        .await
                    {
                        Ok(entry) => entry,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("Access log fetch failed: {error}"));
                        }
                    };
                    let Some(upstreams) = access_log.data["backend_context"]["upstreams"].as_array() else {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Access log has no upstream attempt array: {}",
                                access_log.data["backend_context"]["upstreams"]
                            ),
                        );
                    };
                    let attempts_ok = upstreams.len() == 2
                        && upstreams[0]["status"].as_u64() == Some(503)
                        && upstream_has_retry_on_status(&upstreams[0])
                        && upstreams[1]["status"].as_u64() == Some(200)
                        && !upstream_has_retry_on_status(&upstreams[1]);
                    if !attempts_ok {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("unexpected BUFFERED retry attempts: {upstreams:?}"),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "one Process stream replayed the mutated body and observed only the final 200".to_string(),
                    )
                })
            },
        )
    }

    fn test_external_streamed_request_and_response_body() -> TestCase {
        TestCase::new(
            "external_streamed_request_and_response_body",
            "the independent Go processor decodes and replaces STREAMED request and response bodies",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let case_id = "external-streamed-bidirectional";
                    let original = b"external-streamed-original";
                    let observed_response = b"Server: 0.0.0.0:30001\nEcho: processed-by-external-go";
                    let admin_url = match prepare_external_case(&ctx.http_client, case_id).await {
                        Ok(url) => url,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let response = match ctx
                        .http_client
                        .post(format!("{}/echo", ctx.http_url()))
                        .header("host", EXTERNAL_STREAMED_HOST)
                        .header("x-ext-proc-case-id", case_id)
                        .header("x-ext-proc-test-mode", "interop")
                        .timeout(EXTERNAL_REQUEST_TIMEOUT)
                        .body(original.to_vec())
                        .send()
                        .await
                    {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    };
                    let status = response.status().as_u16();
                    let response_header = response
                        .headers()
                        .get("x-ext-proc-resp")
                        .and_then(|value| value.to_str().ok())
                        == Some("external-go");
                    let body = match tokio::time::timeout(EXTERNAL_ADMIN_REQUEST_TIMEOUT, response.bytes()).await {
                        Ok(Ok(body)) => body,
                        Ok(Err(error)) => {
                            return TestResult::failed(start.elapsed(), format!("Response body failed: {error}"));
                        }
                        Err(_) => return TestResult::failed(start.elapsed(), "Response body timed out".to_string()),
                    };
                    if status != 200 || !response_header || body.as_ref() != b"response-by-external-go" {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "streamed mutation mismatch: status={status} header={response_header} body={}",
                                String::from_utf8_lossy(&body)
                            ),
                        );
                    }
                    let snapshot = match poll_external_snapshot(&ctx.http_client, &admin_url, case_id).await {
                        Ok(snapshot) => snapshot,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let stream = match assert_external_stream(&snapshot, case_id, true) {
                        Ok(stream) => stream,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if let Err(error) = assert_phase_groups(
                        stream,
                        &["request_headers", "request_body", "response_headers", "response_body"],
                    ) {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    if let Err(error) = assert_header_observations(stream, 1, &[200]) {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    if stream.request_body_messages < 1
                        || stream.request_body_bytes != original.len()
                        || stream.request_body_sha256 != sha256_hex(original)
                        || stream.request_eos_count != 1
                        || stream.response_body_messages < 1
                        || stream.response_body_bytes != observed_response.len()
                        || stream.response_body_sha256 != sha256_hex(observed_response)
                        || stream.response_eos_count != 1
                    {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("unexpected STREAMED body observation: {stream:?}"),
                        );
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "independent STREAMED byte hashes and exactly-once EOS observed".to_string(),
                    )
                })
            },
        )
    }

    fn test_external_streamed_status_retry_is_suppressed() -> TestCase {
        TestCase::new(
            "external_streamed_status_retry_is_suppressed",
            "STREAMED request handling makes the first retryable 503 final without replay",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let case_id = format!("ext-stream-no-retry-{}", uuid::Uuid::new_v4());
                    let original = b"external-streamed-no-retry-original";
                    let backend_response = b"attempt=1, status=503";
                    let admin_url = match prepare_external_case(&ctx.http_client, &case_id).await {
                        Ok(url) => url,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let response = match ctx
                        .http_client
                        .post(format!("{}/retry-sequence/{case_id}/503,200", ctx.http_url()))
                        .header("host", EXTERNAL_STREAMED_HOST)
                        .header("x-ext-proc-case-id", &case_id)
                        .header("x-ext-proc-test-mode", "interop")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&case_id),
                        )
                        .header("access_log", "test_store")
                        .timeout(EXTERNAL_REQUEST_TIMEOUT)
                        .body(original.to_vec())
                        .send()
                        .await
                    {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    };
                    let status = response.status().as_u16();
                    let attempt = response
                        .headers()
                        .get("x-attempt-number")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    let history = response
                        .headers()
                        .get("x-attempt-history")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    let response_header = response
                        .headers()
                        .get("x-ext-proc-resp")
                        .and_then(|value| value.to_str().ok())
                        == Some("external-go");
                    let body = match tokio::time::timeout(EXTERNAL_ADMIN_REQUEST_TIMEOUT, response.bytes()).await {
                        Ok(Ok(body)) => body,
                        Ok(Err(error)) => {
                            return TestResult::failed(start.elapsed(), format!("Response body failed: {error}"));
                        }
                        Err(_) => return TestResult::failed(start.elapsed(), "Response body timed out".to_string()),
                    };
                    if status != 503
                        || attempt != "1"
                        || history != "503"
                        || !response_header
                        || body.as_ref() != b"response-by-external-go"
                    {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "streamed retry suppression mismatch: status={status} attempt={attempt} history={history} header={response_header} body={}",
                                String::from_utf8_lossy(&body)
                            ),
                        );
                    }

                    let snapshot = match poll_external_snapshot(&ctx.http_client, &admin_url, &case_id).await {
                        Ok(snapshot) => snapshot,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let stream = match assert_external_stream(&snapshot, &case_id, true) {
                        Ok(stream) => stream,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if let Err(error) = assert_exact_phases(
                        stream,
                        &["request_headers", "request_body", "response_headers", "response_body"],
                    ) {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    if let Err(error) = assert_header_observations(stream, 1, &[503]) {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    if stream.request_body_messages < 1
                        || stream.request_body_bytes != original.len()
                        || stream.request_body_sha256 != sha256_hex(original)
                        || stream.request_eos_count != 1
                        || stream.response_body_messages < 1
                        || stream.response_body_bytes != backend_response.len()
                        || stream.response_body_sha256 != sha256_hex(backend_response)
                        || stream.response_eos_count != 1
                    {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("unexpected STREAMED no-retry observation: {stream:?}"),
                        );
                    }

                    let access_log = match ctx
                        .access_log_client()
                        .get_access_log_with_retry(&case_id, 10, 200)
                        .await
                    {
                        Ok(entry) => entry,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("Access log fetch failed: {error}"));
                        }
                    };
                    let Some(upstreams) = access_log.data["backend_context"]["upstreams"].as_array() else {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Access log has no upstream attempt array: {}",
                                access_log.data["backend_context"]["upstreams"]
                            ),
                        );
                    };
                    let attempts_ok = upstreams.len() == 1
                        && upstreams[0]["status"].as_u64() == Some(503)
                        && !upstream_has_retry_on_status(&upstreams[0]);
                    if !attempts_ok {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("unexpected STREAMED upstream attempts: {upstreams:?}"),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "STREAMED request kept one upstream attempt and processed the final first 503".to_string(),
                    )
                })
            },
        )
    }

    fn test_external_immediate_response() -> TestCase {
        TestCase::new(
            "external_immediate_response",
            "the independent Go processor returns an ordinary ImmediateResponse",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let case_id = "external-immediate-response";
                    let admin_url = match prepare_external_case(&ctx.http_client, case_id).await {
                        Ok(url) => url,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let response = match ctx
                        .http_client
                        .get(format!("{}/headers", ctx.http_url()))
                        .header("host", EXTERNAL_HOST)
                        .header("x-ext-proc-case-id", case_id)
                        .header("x-ext-proc-test-mode", "immediate")
                        .timeout(EXTERNAL_REQUEST_TIMEOUT)
                        .send()
                        .await
                    {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    };
                    let status = response.status().as_u16();
                    let denied_header = response
                        .headers()
                        .get("x-ext-proc-denied")
                        .and_then(|value| value.to_str().ok())
                        == Some("1");
                    let body = match tokio::time::timeout(EXTERNAL_ADMIN_REQUEST_TIMEOUT, response.text()).await {
                        Ok(Ok(body)) => body,
                        Ok(Err(error)) => {
                            return TestResult::failed(start.elapsed(), format!("Response body failed: {error}"));
                        }
                        Err(_) => return TestResult::failed(start.elapsed(), "Response body timed out".to_string()),
                    };
                    if status != 403 || !denied_header || body != "blocked by external ext_proc" {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("ImmediateResponse mismatch: status={status} header={denied_header} body={body}"),
                        );
                    }
                    let snapshot = match poll_external_snapshot(&ctx.http_client, &admin_url, case_id).await {
                        Ok(snapshot) => snapshot,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let stream = match assert_external_stream(&snapshot, case_id, false) {
                        Ok(stream) => stream,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if !stream.cancelled || stream.client_half_closed {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "ImmediateResponse stream did not end by cancellation: cancelled={} half_closed={}",
                                stream.cancelled, stream.client_half_closed
                            ),
                        );
                    }
                    if let Err(error) = assert_phase_groups(stream, &["request_headers"])
                        .and_then(|_| assert_no_body_observations(stream))
                    {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "independent ImmediateResponse status, header, and body observed".to_string(),
                    )
                })
            },
        )
    }

    fn test_external_exact_grpc_immediate_response() -> TestCase {
        TestCase::new(
            "external_exact_grpc_immediate_response",
            "official Go grpc_status field selects exact gRPC code and message",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let case_id = "external-exact-grpc-immediate";
                    let admin_url = match prepare_external_case(&ctx.http_client, case_id).await {
                        Ok(url) => url,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let endpoint_url = format!("http://{}:{}", ctx.target_host, ctx.http_port);
                    let mut endpoint = match tonic::transport::Endpoint::from_shared(endpoint_url) {
                        Ok(endpoint) => endpoint,
                        Err(error) => return TestResult::failed(start.elapsed(), format!("invalid endpoint: {error}")),
                    };
                    let origin = match format!("http://{EXTERNAL_HOST}").parse() {
                        Ok(origin) => origin,
                        Err(error) => return TestResult::failed(start.elapsed(), format!("invalid origin: {error}")),
                    };
                    endpoint = endpoint.origin(origin);
                    let mut client = match TestServiceClient::connect(endpoint).await {
                        Ok(client) => client,
                        Err(error) => return TestResult::failed(start.elapsed(), format!("connect failed: {error}")),
                    };
                    let mut request = tonic::Request::new(HelloRequest {
                        name: "exact-grpc-immediate".to_string(),
                    });
                    request.metadata_mut().insert(
                        "x-ext-proc-case-id",
                        case_id.parse().expect("fixed case ID is valid metadata"),
                    );
                    request.metadata_mut().insert(
                        "x-ext-proc-test-mode",
                        "grpc-immediate".parse().expect("fixed mode is valid metadata"),
                    );
                    let Err(status) = client.say_hello(request).await else {
                        return TestResult::failed(start.elapsed(), "expected gRPC error, got success".to_string());
                    };
                    if status.code() != tonic::Code::PermissionDenied
                        || status.message() != "processor denied exact grpc request"
                    {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "exact gRPC reply mismatch: code={:?} message={}",
                                status.code(),
                                status.message()
                            ),
                        );
                    }
                    let snapshot = match poll_external_snapshot(&ctx.http_client, &admin_url, case_id).await {
                        Ok(snapshot) => snapshot,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let stream = match assert_external_stream(&snapshot, case_id, false) {
                        Ok(stream) => stream,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if let Err(error) = assert_exact_phases(stream, &["request_headers"]) {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "official grpc_status produced exact PERMISSION_DENIED and message".to_string(),
                    )
                })
            },
        )
    }

    fn test_external_disabled_immediate_continues_backend() -> TestCase {
        TestCase::new(
            "external_disabled_immediate_continues_backend",
            "disabled ImmediateResponse reaches the backend and deactivates every later processor phase",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let case_id = "external-disabled-immediate";
                    let admin_url = match prepare_external_case(&ctx.http_client, case_id).await {
                        Ok(url) => url,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let response = match ctx
                        .http_client
                        .get(format!("{}/headers", ctx.http_url()))
                        .header("host", EXTERNAL_DISABLED_IMMEDIATE_HOST)
                        .header("x-ext-proc-case-id", case_id)
                        .header("x-ext-proc-test-mode", "disabled-immediate")
                        .timeout(EXTERNAL_REQUEST_TIMEOUT)
                        .send()
                        .await
                    {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), format!("request failed: {error}")),
                    };
                    let status = response.status().as_u16();
                    let has_processor_response_header = response.headers().contains_key("x-ext-proc-resp");
                    let has_denied_header = response.headers().contains_key("x-ext-proc-denied");
                    let body = response.text().await.unwrap_or_default();
                    if status != 200
                        || has_processor_response_header
                        || has_denied_header
                        || !body.contains("x-ext-proc-test-mode")
                    {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("disabled ImmediateResponse did not preserve backend exchange: status={status} processor_header={has_processor_response_header} denied_header={has_denied_header}"),
                        );
                    }
                    let snapshot = match poll_external_snapshot(&ctx.http_client, &admin_url, case_id).await {
                        Ok(snapshot) => snapshot,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let stream = match assert_external_stream(&snapshot, case_id, true) {
                        Ok(stream) => stream,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if let Err(error) = assert_exact_phases(stream, &["request_headers"])
                        .and_then(|_| assert_no_body_observations(stream))
                    {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "backend continued and no processor phase followed the ignored ImmediateResponse".to_string(),
                    )
                })
            },
        )
    }

    fn test_external_reduction_only_mode_override() -> TestCase {
        TestCase::new(
            "external_reduction_only_mode_override",
            "allowed STREAMED to NONE suppresses request body while a forbidden BUFFERED override is ignored",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    for (case_id, mode, expect_body_phase) in [
                        ("external-mode-reduce-none", "mode-reduce-none", false),
                        ("external-mode-forbidden-expand", "mode-forbidden-expand", true),
                    ] {
                        let admin_url = match prepare_external_case(&ctx.http_client, case_id).await {
                            Ok(url) => url,
                            Err(error) => return TestResult::failed(start.elapsed(), error),
                        };
                        let response = match ctx
                            .http_client
                            .post(format!("{}/echo", ctx.http_url()))
                            .header("host", EXTERNAL_MODE_OVERRIDE_HOST)
                            .header("x-ext-proc-case-id", case_id)
                            .header("x-ext-proc-test-mode", mode)
                            .body("mode-override-body")
                            .send()
                            .await
                        {
                            Ok(response) => response,
                            Err(error) => {
                                return TestResult::failed(start.elapsed(), format!("request failed: {error}"))
                            }
                        };
                        if response.status() != reqwest::StatusCode::OK {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("{mode} returned {}", response.status()),
                            );
                        }
                        let _ = response.bytes().await;
                        let snapshot = match poll_external_snapshot(&ctx.http_client, &admin_url, case_id).await {
                            Ok(snapshot) => snapshot,
                            Err(error) => return TestResult::failed(start.elapsed(), error),
                        };
                        let stream = match assert_external_stream(&snapshot, case_id, true) {
                            Ok(stream) => stream,
                            Err(error) => return TestResult::failed(start.elapsed(), error),
                        };
                        if stream.phases.iter().any(|phase| phase == "request_body") != expect_body_phase {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("{mode} body-phase expectation failed: {stream:?}"),
                            );
                        }
                        let expected_phases = if expect_body_phase {
                            &["request_headers", "request_body", "response_headers"][..]
                        } else {
                            &["request_headers", "response_headers"][..]
                        };
                        if let Err(error) = assert_exact_phases(stream, expected_phases) {
                            return TestResult::failed(start.elapsed(), error);
                        }
                        if expect_body_phase {
                            if stream.request_body_messages == 0
                                || stream.request_body_bytes != b"mode-override-body".len()
                                || stream.request_eos_count != 1
                                || stream.response_body_messages != 0
                            {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("forbidden override observation mismatch: {stream:?}"),
                                );
                            }
                        } else if let Err(error) = assert_no_body_observations(stream) {
                            return TestResult::failed(start.elapsed(), error);
                        }
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "allow-listed reduction suppressed body and forbidden acquisition change was ignored"
                            .to_string(),
                    )
                })
            },
        )
    }

    fn test_external_response_trailers_and_clean_absence() -> TestCase {
        TestCase::new(
            "external_response_trailers_and_clean_absence",
            "official response_trailers mutation reaches H2 while no-trailer completion fabricates no phase",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    for (case_id, mode, upstream_trailers) in [
                        ("external-real-response-trailers", "trailer-mutate", true),
                        ("external-no-response-trailers", "no-trailer", false),
                    ] {
                        let admin_url = match prepare_external_case(&ctx.http_client, case_id).await {
                            Ok(url) => url,
                            Err(error) => return TestResult::failed(start.elapsed(), error),
                        };
                        let (status, body, trailers) =
                            match external_h2_response_trailers(&ctx, case_id, mode, upstream_trailers).await {
                                Ok(result) => result,
                                Err(error) => return TestResult::failed(start.elapsed(), error),
                            };
                        if status != 200 || body.len() != 16 {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("{mode} response mismatch: status={status} body_len={}", body.len()),
                            );
                        }
                        let fixed = trailers
                            .as_ref()
                            .and_then(|map| map.get("x-edgion-upstream-trailer"))
                            .and_then(|value| value.to_str().ok());
                        if upstream_trailers && fixed != Some("processed-by-external-go") {
                            return TestResult::failed(start.elapsed(), format!("mutated trailer = {fixed:?}"));
                        }
                        if !upstream_trailers && trailers.is_some() {
                            return TestResult::failed(
                                start.elapsed(),
                                "no-trailer response fabricated trailers".to_string(),
                            );
                        }
                        let snapshot = match poll_external_snapshot(&ctx.http_client, &admin_url, case_id).await {
                            Ok(snapshot) => snapshot,
                            Err(error) => return TestResult::failed(start.elapsed(), error),
                        };
                        let stream = match assert_external_stream(&snapshot, case_id, true) {
                            Ok(stream) => stream,
                            Err(error) => return TestResult::failed(start.elapsed(), error),
                        };
                        if upstream_trailers {
                            if stream.response_trailer_messages != 1
                                || stream.response_trailer_fields != 1
                                || !stream.saw_fixed_response_trailer
                                || stream.saw_grpc_status_trailer
                            {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("trailer observation = {stream:?}"),
                                );
                            }
                        } else if stream.response_trailer_messages != 0
                            || stream.response_trailer_fields != 0
                            || stream.saw_fixed_response_trailer
                            || stream.saw_grpc_status_trailer
                        {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("fabricated trailer observation = {stream:?}"),
                            );
                        }
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "real H2 trailer was mutated and absent trailers closed cleanly without a phase".to_string(),
                    )
                })
            },
        )
    }

    /// Full-test-only wall-clock idle coverage. Short timeout behavior belongs
    /// in deterministic unit tests; this case deliberately holds the external
    /// side stream idle for three seconds and verifies the transaction survives.
    fn test_external_long_idle_stream_survives() -> TestCase {
        TestCase::new(
            "external_long_idle_stream_survives",
            "an independent ext_proc stream survives a three-second idle verdict window",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let case_id = "external-long-idle";
                    let admin_url = match prepare_external_case(&ctx.http_client, case_id).await {
                        Ok(url) => url,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let response = match ctx
                        .http_client
                        .get(format!("{}/headers", ctx.http_url()))
                        .header("host", EXTERNAL_HOST)
                        .header("x-ext-proc-case-id", case_id)
                        .header("x-ext-proc-test-mode", "long-idle")
                        .timeout(EXTERNAL_REQUEST_TIMEOUT)
                        .send()
                        .await
                    {
                        Ok(response) => response,
                        Err(error) => return TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    };
                    let elapsed_after_headers = start.elapsed();
                    let status = response.status().as_u16();
                    let response_header = response
                        .headers()
                        .get("x-ext-proc-resp")
                        .and_then(|value| value.to_str().ok())
                        == Some("external-go");
                    let body = match tokio::time::timeout(EXTERNAL_ADMIN_REQUEST_TIMEOUT, response.text()).await {
                        Ok(Ok(body)) => body,
                        Ok(Err(error)) => {
                            return TestResult::failed(start.elapsed(), format!("Response body failed: {error}"));
                        }
                        Err(_) => return TestResult::failed(start.elapsed(), "Response body timed out".to_string()),
                    };
                    let request_header = body.to_ascii_lowercase().contains("x-ext-proc-req")
                        && body.to_ascii_lowercase().contains("external-go");
                    if status != 200
                        || !response_header
                        || !request_header
                        || elapsed_after_headers < Duration::from_secs(3)
                    {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "long-idle mismatch: status={status} request={request_header} response={response_header} elapsed={elapsed_after_headers:?}"
                            ),
                        );
                    }
                    let snapshot = match poll_external_snapshot(&ctx.http_client, &admin_url, case_id).await {
                        Ok(snapshot) => snapshot,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    let stream = match assert_external_stream(&snapshot, case_id, true) {
                        Ok(stream) => stream,
                        Err(error) => return TestResult::failed(start.elapsed(), error),
                    };
                    if let Err(error) = assert_phase_groups(stream, &["request_headers", "response_headers"])
                        .and_then(|_| assert_header_observations(stream, 1, &[200]))
                        .and_then(|_| assert_no_body_observations(stream))
                    {
                        return TestResult::failed(start.elapsed(), error);
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "independent long-idle stream completed and half-closed normally".to_string(),
                    )
                })
            },
        )
    }

    /// ImmediateResponse: the `x-ext-proc-block` trigger short-circuits with 403.
    fn test_immediate_response_403() -> TestCase {
        TestCase::new(
            "immediate_response_403",
            "ext_proc returns an ImmediateResponse 403 when triggered",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/headers", ctx.http_url());
                    let response = match ctx
                        .http_client
                        .get(&url)
                        .header("host", BASIC_HOST)
                        .header("x-ext-proc-block", "1")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    let status = response.status().as_u16();
                    if status != 403 {
                        return TestResult::failed(start.elapsed(), format!("Expected 403, got {}", status));
                    }
                    // The ImmediateResponse also carries a custom header mutation;
                    // verify it reached the client (request-stage immediate replies
                    // honor processor header mutations).
                    let denied_header_ok = response
                        .headers()
                        .get("x-ext-proc-denied")
                        .and_then(|v| v.to_str().ok())
                        .map(|v| v == "1")
                        .unwrap_or(false);
                    let body = response.text().await.unwrap_or_default();
                    if body.contains("blocked by ext_proc") && denied_header_ok {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "short-circuited with 403 + custom header".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("403 unexpected: denied_header_ok={} body={}", denied_header_ok, body),
                        )
                    }
                })
            },
        )
    }

    /// fail-close: an unreachable processor returns 500.
    fn test_fail_close_returns_500() -> TestCase {
        TestCase::new(
            "fail_close_returns_500",
            "ext_proc with failureModeAllow=false returns 500 when the processor is unreachable",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/get", ctx.http_url());
                    let Ok(response) = ctx.http_client.get(&url).header("host", FAILCLOSE_HOST).send().await else {
                        return TestResult::failed(start.elapsed(), "ExtProc observation request failed".to_string());
                    };
                    match observe_sanitized_fail_close_response(response).await {
                        Ok(()) => {
                            TestResult::passed_with_message(start.elapsed(), "fail-close returned 500".to_string())
                        }
                        Err(message) => TestResult::failed(start.elapsed(), message.to_string()),
                    }
                })
            },
        )
    }

    /// fail-open: an unreachable processor is bypassed; the request reaches the backend.
    fn test_fail_open_passes() -> TestCase {
        TestCase::new(
            "fail_open_passes",
            "ext_proc with failureModeAllow=true forwards to the backend when the processor is unreachable",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/get", ctx.http_url());
                    let Ok(response) = ctx.http_client.get(&url).header("host", FAILOPEN_HOST).send().await else {
                        return TestResult::failed(start.elapsed(), "ExtProc observation request failed".to_string());
                    };
                    match observe_fail_open_response(response).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "fail-open bypassed, reached backend".to_string(),
                        ),
                        Err(message) => TestResult::failed(start.elapsed(), message.to_string()),
                    }
                })
            },
        )
    }

    /// A malformed/empty verdict from the processor must honor fail-close
    /// (the basic route is failureModeAllow=false) and 500, not silently pass.
    fn test_bad_verdict_fail_closes() -> TestCase {
        TestCase::new(
            "bad_verdict_fail_closes",
            "a malformed processor verdict triggers fail-close (500), not silent pass-through",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/headers", ctx.http_url());
                    let Ok(response) = ctx
                        .http_client
                        .get(&url)
                        .header("host", BASIC_HOST)
                        .header("x-ext-proc-badverdict", "1")
                        .send()
                        .await
                    else {
                        return TestResult::failed(start.elapsed(), "ExtProc observation request failed".to_string());
                    };
                    match observe_sanitized_fail_close_response(response).await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "bad verdict fail-closed (500)".to_string(),
                        ),
                        Err(message) => TestResult::failed(start.elapsed(), message.to_string()),
                    }
                })
            },
        )
    }

    /// Attached STREAMED mode: the fake processor retains per-Process state
    /// from request headers through response body, replaces both directions,
    /// and marks response headers. A response verdict is emitted only when the
    /// same Process stream previously observed the request phase.
    fn test_streamed_body_mutation() -> TestCase {
        TestCase::new(
            "streamed_body_mutation",
            "requestBodyMode STREAMED mutates body chunks in flight (upstream sees the replacement)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.http_url());
                    let response = match ctx
                        .http_client
                        .post(&url)
                        .header("host", STREAMED_HOST)
                        .body("original-client-payload")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    let status = response.status().as_u16();
                    if status != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                    }
                    let attached_header = response
                        .headers()
                        .get("x-ext-proc-resp")
                        .and_then(|value| value.to_str().ok())
                        == Some("seen");
                    let content_length_removed = response.headers().get("content-length").is_none();
                    let body = response.text().await.unwrap_or_default();
                    if !body.contains("response-by-attached-ext-proc") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "attached response mutation was not observed: {}",
                                &body[..body.len().min(400)]
                            ),
                        );
                    }
                    if !attached_header || !content_length_removed {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "response contract mismatch: header={} content_length_removed={}",
                                attached_header, content_length_removed
                            ),
                        );
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "same attached Process stream mutated request and response with safe framing".to_string(),
                    )
                })
            },
        )
    }

    /// STREAMED mid-body rejection: the processor answers the first body chunk
    /// with an ImmediateResponse; the client must observe that status instead
    /// of a proxied response.
    fn test_streamed_immediate_response_mid_body() -> TestCase {
        TestCase::new(
            "streamed_immediate_response_mid_body",
            "requestBodyMode STREAMED terminates mid-body on an ImmediateResponse verdict",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.http_url());
                    let response = match ctx
                        .http_client
                        .post(&url)
                        .header("host", STREAMED_HOST)
                        .header("x-ext-proc-test-mode", "body-block")
                        .body("payload-to-block")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    let status = response.status().as_u16();
                    if status != 403 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected 403 from mid-body immediate response, got {}", status),
                        );
                    }
                    // The immediate response's header mutation must reach the
                    // client on the mid-body terminate path too.
                    let denied_header_ok = response
                        .headers()
                        .get("x-ext-proc-denied")
                        .and_then(|v| v.to_str().ok())
                        .map(|v| v == "1")
                        .unwrap_or(false);
                    let body = response.text().await.unwrap_or_default();
                    if body.contains("blocked by ext_proc") && denied_header_ok {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "mid-body 403 + custom header delivered".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("403 unexpected: denied_header_ok={} body={}", denied_header_ok, body),
                        )
                    }
                })
            },
        )
    }

    /// STREAMED multi-chunk upload: a channel-backed streaming body forces
    /// several transport chunks; the processor replaces each one, so the
    /// backend echo must contain only replacement markers.
    fn test_streamed_multi_chunk_mutation() -> TestCase {
        TestCase::new(
            "streamed_multi_chunk_mutation",
            "requestBodyMode STREAMED exchanges every chunk of a multi-chunk upload",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(1);
                    let sender = tokio::spawn(async move {
                        for index in 0u8..3 {
                            if tx.send(Ok(bytes::Bytes::from(vec![b'a' + index; 64]))).await.is_err() {
                                return;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        }
                    });
                    let body = reqwest::Body::wrap_stream(tokio_stream::wrappers::ReceiverStream::new(rx));
                    let url = format!("{}/echo", ctx.http_url());
                    let response = match ctx
                        .http_client
                        .post(&url)
                        .header("host", STREAMED_HOST)
                        .body(body)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    let _ = sender.await;
                    let status = response.status().as_u16();
                    if status != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                    }
                    let text = response.text().await.unwrap_or_default();
                    let replaced = text.matches("response-by-attached-ext-proc").count();
                    if replaced == 0 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "expected an attached response replacement window, saw {}: {}",
                                replaced,
                                &text[..text.len().min(300)]
                            ),
                        );
                    }
                    if text.contains("aaaa") {
                        return TestResult::failed(start.elapsed(), "original chunk bytes leaked".to_string());
                    }
                    TestResult::passed_with_message(start.elapsed(), format!("{} chunks replaced in flight", replaced))
                })
            },
        )
    }

    /// STREAMED + bodyless request + a CONTINUE_AND_REPLACE headers verdict:
    /// the degenerate header-only path must reject it per the failure policy
    /// (fail-close 500), never silently ignore it.
    fn test_streamed_degenerate_continue_and_replace_fail_closes() -> TestCase {
        TestCase::new(
            "streamed_degenerate_continue_and_replace_fail_closes",
            "CONTINUE_AND_REPLACE on a bodyless streamed request fail-closes (500)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/headers", ctx.http_url());
                    let Ok(response) = ctx
                        .http_client
                        .get(&url)
                        .header("host", STREAMED_HOST)
                        .header("x-ext-proc-test-mode", "continue-and-replace")
                        .send()
                        .await
                    else {
                        return TestResult::failed(start.elapsed(), "ExtProc observation request failed".to_string());
                    };
                    match observe_sanitized_fail_close_response(response).await {
                        Ok(()) => {
                            TestResult::passed_with_message(start.elapsed(), "CR rejected fail-close".to_string())
                        }
                        Err(message) => TestResult::failed(start.elapsed(), message.to_string()),
                    }
                })
            },
        )
    }

    /// STREAMED + dead processor + failureModeAllow=true: the header phase
    /// fails open, no handler is installed, and the original body reaches the
    /// backend unmodified.
    fn test_streamed_fail_open_passes_original_body() -> TestCase {
        TestCase::new(
            "streamed_fail_open_passes_original_body",
            "streamed mode with a dead processor fails open and forwards the original body",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.http_url());
                    let response = match ctx
                        .http_client
                        .post(&url)
                        .header("host", STREAMED_FAILOPEN_HOST)
                        .body("untouched-payload")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    let status = response.status().as_u16();
                    if status != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                    }
                    let body = response.text().await.unwrap_or_default();
                    if body.contains("untouched-payload") {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "fail-open forwarded original body".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("original body missing at backend: {}", &body[..body.len().min(300)]),
                        )
                    }
                })
            },
        )
    }

    /// Complete-body capture cannot compose with request streaming.
    fn test_streamed_after_capture_follows_failure_policy() -> TestCase {
        TestCase::new(
            "streamed_after_capture_follows_failure_policy",
            "complete capture refuses STREAMED install; fail-open preserves captured bytes and fail-close denies",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let original = "captured-original";
                    for (mode, expected_status) in [("close", 500), ("open", 200)] {
                        let response = match ctx
                            .http_client
                            .post(format!("{}/mirror/capture", ctx.http_url()))
                            .header("host", format!("ext-proc-streamed-after-capture-{mode}.example.com"))
                            .body(original)
                            .send()
                            .await
                        {
                            Ok(response) => response,
                            Err(error) => {
                                return TestResult::failed(start.elapsed(), format!("{mode} request: {error}"))
                            }
                        };
                        let digest = hex::encode(Sha256::digest(original.as_bytes()));
                        if response.status().as_u16() != expected_status
                            || (mode == "open"
                                && response
                                    .headers()
                                    .get("x-captured-body-sha256")
                                    .and_then(|v| v.to_str().ok())
                                    != Some(digest.as_str()))
                        {
                            return TestResult::failed(
                                start.elapsed(),
                                format!(
                                    "{mode} incompatible composition status/body mismatch: {}",
                                    response.status()
                                ),
                            );
                        }
                        if let Err(error) = response.bytes().await {
                            return TestResult::failed(start.elapsed(), format!("{mode} incomplete response: {error}"));
                        }
                    }
                    TestResult::passed(start.elapsed())
                })
            },
        )
    }

    fn test_streamed_chain_and_mirror() -> TestCase {
        TestCase::new(
            "streamed_chain_and_mirror",
            "two request processors compose in order and mirror the exact upstream body over H1 and H2",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let host = "ext-proc-streamed-chain.example.com";
                    let address = match tokio::net::lookup_host((ctx.target_host.as_str(), ctx.http_port)).await {
                        Ok(mut addresses) => match addresses.next() {
                            Some(address) => address,
                            None => return TestResult::failed(start.elapsed(), "gateway address is empty".to_string()),
                        },
                        Err(error) => return TestResult::failed(start.elapsed(), format!("gateway address: {error}")),
                    };
                    for h2 in [false, true] {
                        let marker = format!("ext-proc-chain-{}", uuid::Uuid::new_v4());
                        let mut builder = reqwest::Client::builder().no_proxy().resolve(host, address);
                        if h2 {
                            builder = builder.http2_prior_knowledge();
                        } else {
                            builder = builder.http1_only();
                        }
                        let client = match builder.build() {
                            Ok(client) => client,
                            Err(error) => return TestResult::failed(start.elapsed(), format!("client build: {error}")),
                        };
                        let original = "a".repeat(128);
                        let expected = "c".repeat(128);
                        let response = match client
                            .post(format!("http://{host}:{}/mirror/capture", ctx.http_port))
                            .header("x-ext-proc-test-mode", "chain")
                            .header("traceparent", crate::access_log_client::traceparent_for_marker(&marker))
                            .body(original)
                            .send()
                            .await
                        {
                            Ok(response) => response,
                            Err(error) => {
                                return TestResult::failed(start.elapsed(), format!("H2={h2} request: {error}"))
                            }
                        };
                        let digest = hex::encode(Sha256::digest(expected.as_bytes()));
                        if response.status().as_u16() != 200
                            || response
                                .headers()
                                .get("x-captured-body-sha256")
                                .and_then(|v| v.to_str().ok())
                                != Some(digest.as_str())
                        {
                            return TestResult::failed(
                                start.elapsed(),
                                format!(
                                    "H2={h2} upstream received incorrect processor order/body: status={} digest={:?}",
                                    response.status(),
                                    response.headers().get("x-captured-body-sha256")
                                ),
                            );
                        }
                        if h2 && response.version() != reqwest::Version::HTTP_2 {
                            return TestResult::failed(
                                start.elapsed(),
                                "H2 chain test did not negotiate H2".to_string(),
                            );
                        }
                        if let Err(error) = response.bytes().await {
                            return TestResult::failed(start.elapsed(), format!("chain response incomplete: {error}"));
                        }
                        match super::super::body_plugins::poll_mirrored_body(&ctx, &marker).await {
                            Ok(body) if body == expected => {}
                            Ok(_) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("H2={h2} mirror body differs from final upstream body"),
                                )
                            }
                            Err(error) => return TestResult::failed(start.elapsed(), error),
                        }
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "H1/H2: two ordered processors and mirror produced identical final bytes".to_string(),
                    )
                })
            },
        )
    }

    /// Coexistence: ForwardAuth + ExtProc on the same request chain both apply.
    fn test_coexist_with_forward_auth() -> TestCase {
        TestCase::new(
            "coexist_with_forward_auth",
            "ForwardAuth and ExtProc on one route both apply (no interference)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/headers", ctx.http_url());
                    let response = match ctx
                        .http_client
                        .get(&url)
                        .header("host", COEXIST_HOST)
                        .header("Authorization", "Bearer valid-token")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    let status = response.status().as_u16();
                    if status != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                    }
                    let body = response.text().await.unwrap_or_default();
                    let body_lower = body.to_lowercase();
                    // ForwardAuth copied X-User-ID; ExtProc added x-ext-proc-req.
                    let auth_ok = body_lower.contains("x-user-id") && body_lower.contains("user-123");
                    let ext_proc_ok = body_lower.contains("x-ext-proc-req") && body_lower.contains("seen");
                    if auth_ok && ext_proc_ok {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "ForwardAuth + ExtProc both applied".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "auth_ok={} ext_proc_ok={}; body: {}",
                                auth_ok,
                                ext_proc_ok,
                                &body[..body.len().min(400)]
                            ),
                        )
                    }
                })
            },
        )
    }
}

impl TestSuite for ExtProcTestSuite {
    fn name(&self) -> &str {
        "ExtProc"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let mut cases = vec![
            Self::test_basic_request_and_response_headers(),
            Self::test_tls_request_and_response_headers(),
            Self::test_tls_noverify_request_and_response_headers(),
            Self::test_mtls_request_and_response_headers(),
            Self::test_processor_closes_before_request_verdict(),
            Self::test_processor_closes_before_response_verdict(),
            Self::test_immediate_response_403(),
            Self::test_fail_close_returns_500(),
            Self::test_fail_open_passes(),
            Self::test_bad_verdict_fail_closes(),
            Self::test_streamed_body_mutation(),
            Self::test_streamed_chain_and_mirror(),
            Self::test_streamed_after_capture_follows_failure_policy(),
            Self::test_streamed_immediate_response_mid_body(),
            Self::test_streamed_multi_chunk_mutation(),
            Self::test_streamed_degenerate_continue_and_replace_fail_closes(),
            Self::test_streamed_fail_open_passes_original_body(),
            Self::test_coexist_with_forward_auth(),
        ];
        if self.full_test {
            cases.extend([
                Self::test_external_headers_and_terminal_half_close(),
                Self::test_external_opaque_header_wire_fidelity(),
                Self::test_external_buffered_request_body(),
                Self::test_external_buffered_status_retry_discards_intermediate(),
                Self::test_external_streamed_request_and_response_body(),
                Self::test_external_streamed_status_retry_is_suppressed(),
                Self::test_external_immediate_response(),
                Self::test_external_exact_grpc_immediate_response(),
                Self::test_external_disabled_immediate_continues_backend(),
                Self::test_external_reduction_only_mode_override(),
                Self::test_external_response_trailers_and_clean_absence(),
                Self::test_external_long_idle_stream_survives(),
                Self::test_external_seeded_chunk_boundaries(),
                Self::test_external_request_body_backpressure(),
                Self::test_external_concurrency_saturation_and_release(),
                Self::test_external_tls_server_certificate_rotation(),
                Self::test_external_mtls_client_certificate_rotation(),
            ]);
        }
        cases
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXTERNAL_CASES: &[&str] = &[
        "external_headers_and_terminal_half_close",
        "external_opaque_header_wire_fidelity",
        "external_buffered_request_body",
        "external_buffered_status_retry_discards_intermediate",
        "external_streamed_request_and_response_body",
        "external_streamed_status_retry_is_suppressed",
        "external_immediate_response",
        "external_exact_grpc_immediate_response",
        "external_disabled_immediate_continues_backend",
        "external_reduction_only_mode_override",
        "external_response_trailers_and_clean_absence",
        "external_long_idle_stream_survives",
        "external_seeded_chunk_boundaries",
        "external_request_body_backpressure",
        "external_concurrency_saturation_and_release",
        "external_tls_server_certificate_rotation",
        "external_mtls_client_certificate_rotation",
    ];

    #[test]
    fn external_cases_are_full_test_only() {
        let fast_names: Vec<String> = ExtProcTestSuite::new(false)
            .test_cases()
            .into_iter()
            .map(|case| case.name)
            .collect();
        let full_names: Vec<String> = ExtProcTestSuite::new(true)
            .test_cases()
            .into_iter()
            .map(|case| case.name)
            .collect();

        for name in EXTERNAL_CASES {
            assert!(
                !fast_names.iter().any(|case| case == name),
                "fast suite contains {name}"
            );
            assert!(
                full_names.iter().any(|case| case == name),
                "full suite is missing {name}"
            );
        }
    }

    #[test]
    fn seeded_chunk_plan_is_stable_and_bounded() {
        let plan = seeded_chunk_plan(SEEDED_CHUNK_SEEDS[0], SEEDED_BODY_BYTES).unwrap();
        assert_eq!(plan.iter().sum::<usize>(), SEEDED_BODY_BYTES);
        assert!(!plan.is_empty());
        assert!(plan.len() <= SEEDED_MAX_PLAN_CHUNKS);
        assert!(plan.iter().all(|chunk| *chunk != 0 && *chunk <= SEEDED_MAX_CHUNK_BYTES));
        assert_eq!(&plan[..6], &[28_813, 10_463, 1_473, 52_199, 54_634, 55_439]);

        for seed in SEEDED_CHUNK_SEEDS {
            let plan = seeded_chunk_plan(*seed, SEEDED_BODY_BYTES).unwrap();
            assert_eq!(plan.iter().sum::<usize>(), SEEDED_BODY_BYTES);
            assert!(plan.iter().all(|chunk| *chunk != 0 && *chunk <= SEEDED_MAX_CHUNK_BYTES));
        }
    }
}

#[cfg(test)]
mod observation_tests {
    use super::{observe_fail_open_response, observe_sanitized_fail_close_response};

    use bytes::Bytes;
    use futures::stream;
    use http::Response as HttpResponse;
    use reqwest::{Body, Response};
    use std::io;

    fn response(status: u16, body: &str) -> Response {
        HttpResponse::builder()
            .status(status)
            .body(body.to_string())
            .unwrap()
            .into()
    }

    fn interrupted_response(status: u16, prefix: &str) -> Response {
        let chunks = [
            Ok(Bytes::copy_from_slice(prefix.as_bytes())),
            Err(io::Error::other("sensitive-observation-sentinel")),
        ];
        HttpResponse::builder()
            .status(status)
            .body(Body::wrap_stream(stream::iter(chunks)))
            .unwrap()
            .into()
    }

    const ECHO: &str = "Server: 0.0.0.0:30001\nClient: 127.0.0.1:40000\nMethod: GET\nPath: /get\nQuery: \n\nHeaders:\n  Host: ext-proc-failopen.example.com\n";

    #[tokio::test]
    async fn ext_proc_fail_closed_preserves_completed_empty_and_generic_shapes() {
        for body in ["", "Plugin configuration error", "Internal server error"] {
            assert_eq!(observe_sanitized_fail_close_response(response(500, body)).await, Ok(()));
        }
        assert!(observe_sanitized_fail_close_response(response(200, "")).await.is_err());
        assert!(observe_sanitized_fail_close_response(response(500, "unrelated"))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn ext_proc_fail_closed_preserves_every_prohibited_detail() {
        for detail in ["EXT_PROC", "gRPC", "UNAVAILABLE", "127.0.0.1", "target", "processor"] {
            let body = format!("Internal server error: {detail} sensitive-observation-sentinel");
            assert_eq!(
                observe_sanitized_fail_close_response(response(500, &body)).await,
                Err("Fail-closed response was not generic or exposed internal detail"),
            );
        }
    }

    #[tokio::test]
    async fn ext_proc_fail_closed_read_errors_are_not_successful_empty_bodies() {
        for prefix in ["", "Internal server error"] {
            assert_eq!(
                observe_sanitized_fail_close_response(interrupted_response(500, prefix)).await,
                Err("ExtProc fail-closed response body read failed"),
            );
        }
    }

    #[tokio::test]
    async fn ext_proc_fail_open_requires_the_completed_expected_echo() {
        assert_eq!(observe_fail_open_response(response(200, ECHO)).await, Ok(()));
        for body in [
            String::new(),
            "unrelated success".to_string(),
            ECHO.replace("30001", "30002"),
            ECHO.replace("Method: GET", "Method: POST"),
            ECHO.replace("Path: /get", "Path: /other"),
            ECHO.replace("Headers:", "Other:"),
        ] {
            assert!(observe_fail_open_response(response(200, &body)).await.is_err());
        }
        assert!(observe_fail_open_response(response(500, ECHO)).await.is_err());
    }

    #[tokio::test]
    async fn ext_proc_fail_open_rejects_processor_headers_without_disclosing_the_body() {
        let body = format!("{ECHO}  X-Ext-Proc-Req: sensitive-observation-sentinel\n");
        assert_eq!(
            observe_fail_open_response(response(200, &body)).await,
            Err("fail-open but processor header present (unexpected)"),
        );
    }

    #[tokio::test]
    async fn ext_proc_fail_open_read_errors_fail_even_after_valid_echo_bytes() {
        for prefix in ["", ECHO] {
            assert_eq!(
                observe_fail_open_response(interrupted_response(200, prefix)).await,
                Err("ExtProc fail-open response body read failed"),
            );
        }
    }
}
